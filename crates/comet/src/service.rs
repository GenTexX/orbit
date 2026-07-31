//! comet language service: what an editor asks about a script while it is being
//! typed.
//!
//! In-process and called directly, with no protocol in between (ADR 0010) -
//! atlas and comet are the same program, and putting an LSP between our own
//! editor and our own language would buy nothing but latency and a serializer.
//!
//! Every call re-runs the whole pipeline on the whole text. That is deliberate
//! for v1: a script is a few hundred lines, the compiler has no optimizing
//! middle-end to slow it down (ADR 0007), and incremental re-parsing is a large
//! amount of machinery to maintain in exchange for a saving nobody has measured.
//! Correct first, then measure - the lesson milestone 3 spent a whole report
//! learning. If a profile ever shows this on the frame budget, the frontend is
//! already error-tolerant enough to be made incremental without changing what it
//! reports.

use crate::ast::{Block, Else, Function, Script, Stmt};
use crate::diagnostic::Diagnostic;
use crate::lexer::{TokenKind, lex_with_comments};
use crate::parser::parse;
use crate::span::Span;
use crate::tir::Type;

/// Everything an editor asks about one revision of a file, computed once.
///
/// `diagnostics`, `highlight`, `completions_at` and `brackets` each lex and
/// parse from scratch, which is three or four passes over the same text per
/// keystroke to learn the same things. An editor that asks more than one
/// question per edit should build one of these and read from it.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    pub tokens: Vec<TokenSpan>,
    pub brackets: Vec<Bracket>,
}

impl Analysis {
    /// Run the whole frontend over `source` once.
    pub fn new(source: &str) -> Self {
        let (with_comments, _) = lex_with_comments(source);
        let (script, mut diagnostics) = parse(source);
        let (_, checked) = crate::check::check(&script);
        diagnostics.extend(checked);
        diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
        Self {
            diagnostics,
            tokens: classify(&with_comments),
            brackets: pair_up(&with_comments),
        }
    }

    /// The bracket touching `offset` on either side, and its partner.
    pub fn bracket_at(&self, offset: usize) -> Option<Bracket> {
        self.brackets.iter().copied().find(|b| {
            let (lo, hi) = (b.span.start as usize, b.span.end as usize);
            offset == lo || offset == hi
        })
    }
}

/// Everything wrong with `source`, sorted by position.
///
/// Never fails and never stops early: a typo on line 3 does not blank out the
/// diagnostics for line 30, which is what makes live squiggles worth having at
/// all - a file being typed is malformed most of the time it is looked at.
pub fn diagnostics(source: &str) -> Vec<Diagnostic> {
    let (script, mut found) = parse(source);
    let (_, checked) = crate::check::check(&script);
    found.extend(checked);
    found.sort_by_key(|d| (d.span.start, d.span.end));
    found
}

// --- syntax highlighting ---

/// What a stretch of source is, for coloring. Deliberately coarse: a theme has
/// a handful of code colors, not one per token kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenClass {
    Keyword,
    Number,
    Str,
    Comment,
    /// A name being defined or called.
    Function,
    /// A type name, in an annotation or a return type.
    Type,
    /// Any other identifier: a local, a parameter, script state, `pos`.
    Identifier,
    Operator,
    Punctuation,
}

/// One classified stretch of source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenSpan {
    pub span: Span,
    pub class: TokenClass,
}

/// Classify `source` for syntax coloring, in source order.
///
/// This lives here rather than in the editor because comet owns what its syntax
/// is; an editor that re-scanned the text by hand would be a second, worse lexer
/// that disagrees with the real one the first time a `//` appears inside a
/// string.
///
/// Function and type names are recognized from their neighbours - a name before
/// `(` is being called, a name after `:` or `->` is a type - rather than from
/// the parse tree, so highlighting stays right through the half-typed states a
/// tree cannot describe.
pub fn highlight(source: &str) -> Vec<TokenSpan> {
    let (tokens, _) = lex_with_comments(source);
    classify(&tokens)
}

/// Classify an already-lexed stream.
fn classify(tokens: &[crate::lexer::Token]) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(tokens.len());
    for (index, token) in tokens.iter().enumerate() {
        let next = tokens.get(index + 1).map(|t| &t.kind);
        let previous = index
            .checked_sub(1)
            .and_then(|i| tokens.get(i))
            .map(|t| &t.kind);
        let class = match &token.kind {
            TokenKind::Eof => continue,
            TokenKind::Comment => TokenClass::Comment,
            TokenKind::Number(_) => TokenClass::Number,
            TokenKind::Str(_) => TokenClass::Str,
            TokenKind::Func
            | TokenKind::Let
            | TokenKind::If
            | TokenKind::Else
            | TokenKind::While
            | TokenKind::For
            | TokenKind::In
            | TokenKind::Return
            | TokenKind::True
            | TokenKind::False => TokenClass::Keyword,
            TokenKind::Ident(_) => {
                if matches!(previous, Some(TokenKind::Colon | TokenKind::Arrow)) {
                    TokenClass::Type
                } else if matches!(next, Some(TokenKind::LParen))
                    || matches!(previous, Some(TokenKind::Func))
                {
                    TokenClass::Function
                } else {
                    TokenClass::Identifier
                }
            }
            TokenKind::LBrace
            | TokenKind::RBrace
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::Comma
            | TokenKind::Dot
            | TokenKind::Semicolon
            | TokenKind::Colon => TokenClass::Punctuation,
            _ => TokenClass::Operator,
        };
        out.push(TokenSpan {
            span: token.span,
            class,
        });
    }
    out
}

// --- brackets ---

/// One bracket in the source, and what it pairs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bracket {
    pub span: Span,
    /// The partner's span, or `None` for one that never matched.
    pub partner: Option<Span>,
    /// How deeply nested this pair is, outermost 0.
    pub depth: usize,
    pub open: bool,
}

/// Every bracket in `source`, paired where it can be.
///
/// Built off the same lexer everything else uses, so a brace inside a string or
/// a comment is not a brace - which is exactly the case a hand-rolled scan over
/// the characters gets wrong, and the reason this lives in comet rather than in
/// the editor.
pub fn brackets(source: &str) -> Vec<Bracket> {
    let (tokens, _) = lex_with_comments(source);
    pair_up(&tokens)
}

/// Pair up the brackets in an already-lexed stream.
fn pair_up(tokens: &[crate::lexer::Token]) -> Vec<Bracket> {
    let mut out: Vec<Bracket> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for token in tokens {
        let (open, closes) = match token.kind {
            TokenKind::LBrace => (true, None),
            TokenKind::LParen => (true, None),
            TokenKind::RBrace => (false, Some(TokenKind::LBrace)),
            TokenKind::RParen => (false, Some(TokenKind::LParen)),
            _ => continue,
        };
        if open {
            stack.push(out.len());
            out.push(Bracket {
                span: token.span,
                partner: None,
                depth: stack.len() - 1,
                open: true,
            });
            continue;
        }
        // Only pair with a matching opener. A `)` closing a `{` is a mistake in
        // both places, so neither gets a partner and both can be marked.
        let matched = stack
            .last()
            .copied()
            .filter(|&i| kind_of(tokens, out[i].span) == closes);
        match matched {
            Some(i) => {
                stack.pop();
                let span = token.span;
                out[i].partner = Some(span);
                out.push(Bracket {
                    span,
                    partner: Some(out[i].span),
                    depth: out[i].depth,
                    open: false,
                });
            }
            None => out.push(Bracket {
                span: token.span,
                partner: None,
                depth: stack.len(),
                open: false,
            }),
        }
    }
    out
}

fn kind_of(tokens: &[crate::lexer::Token], span: Span) -> Option<TokenKind> {
    tokens
        .iter()
        .find(|t| t.span == span)
        .map(|t| t.kind.clone())
}

/// The bracket the caret at `offset` is touching - on either side of it - and
/// its partner.
///
/// Touching either side is what makes this usable: a caret sitting just after a
/// `}` is what it feels like to have "just typed" one.
pub fn bracket_at(source: &str, offset: usize) -> Option<Bracket> {
    brackets(source).into_iter().find(|b| {
        let (lo, hi) = (b.span.start as usize, b.span.end as usize);
        offset == lo || offset == hi
    })
}

// --- completions ---

/// What kind of thing a completion offers, so an editor can icon or sort them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionKind {
    /// A local, a parameter, or script state.
    Variable,
    /// A field of the receiver being typed after a `.`.
    Field,
    /// A function defined in this script, or a host builtin.
    Function,
    /// A type name.
    Type,
    Keyword,
}

/// One thing an editor can offer to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// A short line about what it is - a type, a signature, a sentence. Shown
    /// beside the name, because a list of bare names does not teach anyone what
    /// `pos` or `print` actually are.
    pub detail: String,
}

/// The keywords a script can start a statement with.
const KEYWORDS: &[&str] = &[
    "func", "let", "if", "else", "while", "for", "in", "return", "true", "false",
];
/// The type names a script can write.
const TYPES: &[&str] = &["f32", "bool", "Vec2", "String"];
/// The engine-provided functions, with what each one is, for completion detail
/// and hover. Kept beside the checker's table by a test that compares the two.
const BUILTINS: &[(&str, &str)] = &[
    ("print", "func print(s: String)"),
    ("vec2", "func vec2(x: f32, y: f32) -> Vec2"),
    ("abs", "func abs(a: f32) -> f32"),
    ("sqrt", "func sqrt(a: f32) -> f32"),
    ("floor", "func floor(a: f32) -> f32"),
    ("ceil", "func ceil(a: f32) -> f32"),
    ("min", "func min(a: f32, b: f32) -> f32"),
    ("max", "func max(a: f32, b: f32) -> f32"),
    ("str", "func str(value: f32) -> String"),
    ("sin", "func sin(a: f32) -> f32"),
    ("cos", "func cos(a: f32) -> f32"),
    ("atan2", "func atan2(y: f32, x: f32) -> f32"),
    ("pow", "func pow(a: f32, b: f32) -> f32"),
];
/// The magic name bound to the owning node's position.
const POS_NAME: &str = "pos";
/// `Vec2`'s whole member set.
const VEC2_FIELDS: &[&str] = &["x", "y"];

/// What can be written at `offset` in `source`.
///
/// After a `.`, the receiver's fields and nothing else - offering keywords there
/// would be noise. Otherwise: the names in scope at that point, the script's own
/// functions, the builtins, the types, and the keywords.
///
/// Resolution is single-file, because a v1 script is (there is no `import`).
pub fn completions_at(source: &str, offset: usize) -> Vec<CompletionItem> {
    let offset = offset.min(source.len());
    let (script, _) = parse(source);

    if let Some(receiver) = field_receiver(source, offset) {
        return match receiver_type(&script, offset, receiver) {
            Some(Type::Vec2) => VEC2_FIELDS
                .iter()
                .map(|name| detailed(name, CompletionKind::Field, "f32".to_string()))
                .collect(),
            // An unknown receiver offers nothing rather than guessing: a wrong
            // list read as a real one is worse than an empty one read as "I do
            // not know yet".
            _ => Vec::new(),
        };
    }

    let mut items = Vec::new();
    for name in names_in_scope(&script, offset) {
        let detail = type_of_name(&script, offset, &name)
            .map(|ty| ty.name().to_string())
            .unwrap_or_default();
        items.push(detailed(&name, CompletionKind::Variable, detail));
    }
    for function in &script.functions {
        if !function.name.is_empty() {
            items.push(detailed(
                &function.name,
                CompletionKind::Function,
                signature_of(function),
            ));
        }
    }
    for (name, signature) in BUILTINS {
        items.push(detailed(
            name,
            CompletionKind::Function,
            signature.to_string(),
        ));
    }
    for name in TYPES {
        items.push(item(name, CompletionKind::Type));
    }
    for name in KEYWORDS {
        items.push(detailed(
            name,
            CompletionKind::Keyword,
            keyword_detail(name).to_string(),
        ));
    }
    // dedup_by only removes ADJACENT duplicates, and the same name reaching the
    // list from two scopes - a parameter shadowed by a local, a function whose
    // name is also a variable - is not adjacent. Keep the first, which is the
    // most specific: locals come before globals, which come before keywords.
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.label.clone()));
    items
}

fn item(label: &str, kind: CompletionKind) -> CompletionItem {
    detailed(label, kind, String::new())
}

fn detailed(label: &str, kind: CompletionKind, detail: String) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind,
        detail,
    }
}

/// What each keyword is for, in one line. This is the language's only
/// documentation that reaches a person while they are writing it.
fn keyword_detail(word: &str) -> &'static str {
    match word {
        "func" => "define a function",
        "let" => "bind a name; at the top level, script state",
        "if" => "run a block when a condition holds",
        "else" => "run a block when the `if` did not",
        "while" => "repeat a block while a condition holds",
        "for" => "repeat a block once per number in a range: `for i in 0.0..4.0`",
        "in" => "separates a `for` loop's variable from its range",
        "return" => "leave a function with a value",
        "true" | "false" => "a bool literal",
        _ => "",
    }
}

/// The type of a name visible at `offset`.
///
/// Script state goes through the checker, so `let speed = 1.0;` reports `f32`
/// rather than nothing - almost every `let` is inferred, and a hover that only
/// worked on annotated ones would almost never fire. A local inside a function
/// still needs its annotation: the checker resolves those to slots, and nothing
/// maps a slot back to the name it came from yet.
fn type_of_name(script: &Script, offset: usize, name: &str) -> Option<Type> {
    if name == POS_NAME {
        return Some(Type::Vec2);
    }
    if script.state.iter().any(|s| s.name == name) {
        let (typed, _) = crate::check::check(script);
        return typed
            .state
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.ty)
            .filter(|ty| !ty.is_error());
    }
    let function = enclosing_function(script, offset)?;
    for param in &function.params {
        if param.name == name {
            return Type::from_name(&param.ty.name);
        }
    }
    let mut found = None;
    visit_lets(&function.body, offset, &mut |n, ty, _| {
        if n == name {
            found = ty.and_then(Type::from_name);
        }
    });
    found
}

/// A function's signature as written, for a completion's detail and for hover.
fn signature_of(function: &Function) -> String {
    let params: Vec<String> = function
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, p.ty.name))
        .collect();
    match &function.ret {
        Some(ret) => format!(
            "func {}({}) -> {}",
            function.name,
            params.join(", "),
            ret.name
        ),
        None => format!("func {}({})", function.name, params.join(", ")),
    }
}

/// What the editor should say about the name at `offset`, if anything.
///
/// Hovering a name is how you find out what `dt` is without reading the
/// signature again - and in a teaching language that is most of what a person
/// needs while writing.
pub fn hover_at(source: &str, offset: usize) -> Option<String> {
    let offset = offset.min(source.len());
    let (script, _) = parse(source);
    let (start, end) = word_span(source, offset)?;
    let name = &source[start..end];

    if let Some(function) = script.functions.iter().find(|f| f.name == name) {
        return Some(signature_of(function));
    }
    if let Some((_, signature)) = BUILTINS.iter().find(|(n, _)| *n == name) {
        return Some(signature.to_string());
    }
    if let Some(ty) = Type::from_name(name) {
        return Some(format!("type {}", ty.name()));
    }
    if !keyword_detail(name).is_empty() {
        return Some(format!("{name} - {}", keyword_detail(name)));
    }
    let ty = type_of_name(&script, offset, name)?;
    Some(format!("{name}: {}", ty.name()))
}

/// The 1-based line a function is declared on, for turning a runtime trap into
/// a place in the source.
///
/// A wasm trap names a function, not a line - there is no source map yet - so
/// the function's own line is the most precise honest answer. An editor can put
/// the caret there.
pub fn function_line(source: &str, name: &str) -> Option<usize> {
    let (script, _) = parse(source);
    let function = script.functions.iter().find(|f| f.name == name)?;
    let at = (function.span.start as usize).min(source.len());
    Some(source[..at].matches('\n').count() + 1)
}

/// The identifier surrounding `offset`, if it is in one.
fn word_span(source: &str, offset: usize) -> Option<(usize, usize)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if !source.is_char_boundary(offset) {
        return None;
    }
    let start = source[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = source[offset..]
        .char_indices()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, c)| offset + i + c.len_utf8());
    (start < end).then_some((start, end))
}

/// The identifier before the `.` immediately left of `offset`, if the caret is
/// in a field position. Reads the source rather than the tree: `pos.` does not
/// parse as a field access, and that half-typed state is exactly when
/// completions are asked for.
fn field_receiver(source: &str, offset: usize) -> Option<&str> {
    let before = &source[..offset];
    // Skip the partial field name already typed.
    let name_start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |at| at + 1);
    let before = &before[..name_start];
    let dot = before.strip_suffix('.')?;
    let start = dot
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |at| at + 1);
    let receiver = &dot[start..];
    (!receiver.is_empty()).then_some(receiver)
}

/// The type of `receiver` as seen from `offset`: the magic `pos`, a declared
/// local, or a piece of script state.
fn receiver_type(script: &Script, offset: usize, receiver: &str) -> Option<Type> {
    if receiver == POS_NAME {
        return Some(Type::Vec2);
    }
    // A `let` with an explicit type says what it is; one without needs the
    // checker, which cannot run on a half-typed file - so an annotated binding
    // completes and an inferred one does not, which is a limit worth having
    // rather than a guess worth making.
    for state in &script.state {
        if state.name == receiver {
            return state.ty.as_ref().and_then(|t| Type::from_name(&t.name));
        }
    }
    let function = enclosing_function(script, offset)?;
    for param in &function.params {
        if param.name == receiver {
            return Type::from_name(&param.ty.name);
        }
    }
    let mut found = None;
    visit_lets(&function.body, offset, &mut |name, ty, _| {
        if name == receiver {
            found = ty.and_then(Type::from_name);
        }
    });
    found
}

/// Every name visible at `offset`: the enclosing function's parameters and the
/// locals declared above that point, plus the script's state.
fn names_in_scope(script: &Script, offset: usize) -> Vec<String> {
    let mut names: Vec<String> = script
        .state
        .iter()
        .filter(|state| !state.name.is_empty())
        .map(|state| state.name.clone())
        .collect();
    // `pos` is always there, and is the name a script reaches for first.
    names.insert(0, POS_NAME.to_string());

    if let Some(function) = enclosing_function(script, offset) {
        for param in &function.params {
            if !param.name.is_empty() {
                names.push(param.name.clone());
            }
        }
        visit_lets(&function.body, offset, &mut |name, _, _| {
            if !name.is_empty() {
                names.push(name.to_string());
            }
        });
    }
    names.dedup();
    names
}

fn enclosing_function(script: &Script, offset: usize) -> Option<&Function> {
    script
        .functions
        .iter()
        .find(|f| offset >= f.span.start as usize && offset <= f.span.end as usize)
}

/// Walk every `let` in `block` that is declared before `offset`, including in
/// nested blocks the caret is inside.
///
/// Scoping is deliberately loose: a local declared in an `if` body is offered
/// after that body ends, where the checker would reject it. Over-offering a name
/// that turns out not to compile is a smaller annoyance than not offering one
/// that would have, and the squiggle says so immediately either way.
fn visit_lets(block: &Block, offset: usize, out: &mut impl FnMut(&str, Option<&str>, Span)) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let {
                name,
                ty,
                span,
                name_span,
                ..
            } => {
                if (span.end as usize) <= offset {
                    out(name, ty.as_ref().map(|t| t.name.as_str()), *name_span);
                }
            }
            Stmt::If(if_stmt) => {
                visit_lets(&if_stmt.then, offset, out);
                let mut branch = if_stmt.otherwise.as_deref();
                while let Some(next) = branch {
                    match next {
                        Else::Block(block) => {
                            visit_lets(block, offset, out);
                            branch = None;
                        }
                        Else::If(nested) => {
                            visit_lets(&nested.then, offset, out);
                            branch = nested.otherwise.as_deref();
                        }
                    }
                }
            }
            Stmt::While { body, .. } => visit_lets(body, offset, out),
            Stmt::For {
                name,
                name_span,
                body,
                ..
            } => {
                // The loop variable is always an f32, and it is in scope from
                // the moment it is written - which is what makes `for i in`
                // followed by `i` inside the body complete.
                if (name_span.end as usize) <= offset {
                    out(name, Some("f32"), *name_span);
                }
                visit_lets(body, offset, out);
            }
            Stmt::Assign { .. } | Stmt::Return { .. } | Stmt::Expr { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- diagnostics ---

    #[test]
    fn a_clean_script_reports_nothing() {
        assert!(diagnostics(include_str!("../tests/fixtures/bounce.cmt")).is_empty());
        assert!(diagnostics(include_str!("../tests/fixtures/clamp.cmt")).is_empty());
    }

    #[test]
    fn a_type_error_is_reported_where_it_is() {
        let source = include_str!("../tests/fixtures/type_error.cmt");
        let found = diagnostics(source);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].span.text(source), "ready");
    }

    #[test]
    fn one_typo_does_not_blank_out_the_rest_of_the_file() {
        // The property that makes live squiggles worth having: a file being
        // typed is broken most of the time it is looked at, and an editor that
        // gave up at the first error would show nothing about the rest.
        let source = "\
func first(a: f32) -> f32 { a + }
func second(b: f32) -> f32 { b * 2.0 }
func third(c: f32) { let x = nope; }
";
        let found = diagnostics(source);
        assert!(found.len() >= 2, "both problems reported: {found:?}");
        assert!(
            found
                .iter()
                .any(|d| d.message.contains("cannot find `nope`")),
            "the error after the syntax error is still found: {found:?}"
        );
    }

    #[test]
    fn diagnostics_come_back_in_source_order() {
        let source = "func f() { let a = nope; let b = also_nope; }";
        let found = diagnostics(source);
        assert!(found.windows(2).all(|w| w[0].span.start <= w[1].span.start));
    }

    #[test]
    fn nothing_a_person_can_type_makes_the_service_panic() {
        // Every prefix of a real script - i.e. every state it passes through
        // while being typed - plus a few deliberately hostile ones.
        let script = include_str!("../tests/fixtures/bounce.cmt");
        for end in 0..=script.len() {
            if script.is_char_boundary(end) {
                diagnostics(&script[..end]);
            }
        }
        for source in [
            "",
            "   ",
            "{",
            "}",
            "func",
            "func (",
            "let = ;",
            "\"unclosed",
        ] {
            diagnostics(source);
        }
    }

    // --- highlighting ---

    /// The classified text of each token, for readable assertions.
    fn classes(source: &str) -> Vec<(&str, TokenClass)> {
        highlight(source)
            .into_iter()
            .map(|t| (t.span.text(source), t.class))
            .collect()
    }

    #[test]
    fn keywords_numbers_and_strings_are_classified() {
        assert_eq!(
            classes("let x = 1.0;"),
            vec![
                ("let", TokenClass::Keyword),
                ("x", TokenClass::Identifier),
                ("=", TokenClass::Operator),
                ("1.0", TokenClass::Number),
                (";", TokenClass::Punctuation),
            ]
        );
        assert_eq!(classes(r#""hi""#), vec![(r#""hi""#, TokenClass::Str)]);
    }

    #[test]
    fn a_comment_is_classified_and_a_slash_in_a_string_is_not() {
        // The reason this shares the real lexer instead of scanning for `//`:
        // a second scanner colors the rest of this line as a comment.
        assert_eq!(
            classes(r#"let s = "http://x"; // note"#),
            vec![
                ("let", TokenClass::Keyword),
                ("s", TokenClass::Identifier),
                ("=", TokenClass::Operator),
                (r#""http://x""#, TokenClass::Str),
                (";", TokenClass::Punctuation),
                ("// note", TokenClass::Comment),
            ]
        );
    }

    #[test]
    fn names_are_told_apart_by_their_neighbours() {
        let classes = classes("func clamp(value: f32) -> Vec2 { print(value) }");
        assert!(classes.contains(&("clamp", TokenClass::Function)));
        assert!(classes.contains(&("print", TokenClass::Function)));
        assert!(classes.contains(&("f32", TokenClass::Type)));
        assert!(classes.contains(&("Vec2", TokenClass::Type)));
        assert!(classes.contains(&("value", TokenClass::Identifier)));
    }

    #[test]
    fn highlighting_survives_a_file_that_does_not_parse() {
        // It runs on every keystroke, so most of what it sees is half-written.
        let classes = classes("func update(dt: f32) { if pos.x > { ");
        assert!(
            classes
                .iter()
                .any(|(t, c)| *t == "if" && *c == TokenClass::Keyword)
        );
    }

    #[test]
    fn every_token_is_covered_exactly_once_and_in_order() {
        let source = include_str!("../tests/fixtures/bounce.cmt");
        let spans = highlight(source);
        assert!(spans.windows(2).all(|w| w[0].span.end <= w[1].span.start));
        assert!(spans.iter().all(|s| s.span.end <= source.len() as u32));
    }

    #[test]
    fn one_analysis_answers_what_three_passes_used_to() {
        // The point of it: an editor asking more than one question per edit was
        // lexing and parsing the same text three or four times to learn the
        // same things.
        let source = include_str!("../tests/fixtures/type_error.cmt");
        let analysis = Analysis::new(source);
        assert_eq!(analysis.diagnostics, diagnostics(source));
        assert_eq!(analysis.tokens, highlight(source));
        assert_eq!(analysis.brackets, brackets(source));
    }

    #[test]
    fn an_analysis_finds_a_bracket_the_same_way_the_standalone_call_does() {
        let source = "func f() { }";
        let analysis = Analysis::new(source);
        assert_eq!(analysis.bracket_at(9), bracket_at(source, 9));
        assert_eq!(analysis.bracket_at(5), None);
    }

    // --- brackets ---

    #[test]
    fn brackets_pair_up_and_nest() {
        let source = "func f() { if a { } }";
        let all = brackets(source);
        assert_eq!(all.len(), 6, "two parens, four braces");
        let open_body = all.iter().find(|b| b.span.start == 9).expect("the body");
        assert_eq!(open_body.partner.map(|p| p.start), Some(20));
        assert_eq!(open_body.depth, 0);
        let inner = all
            .iter()
            .find(|b| b.span.start == 16)
            .expect("the if body");
        assert_eq!(inner.depth, 1, "nested one deeper");
        assert!(all.iter().all(|b| b.partner.is_some()));
    }

    #[test]
    fn a_brace_in_a_string_or_a_comment_is_not_a_brace() {
        // The reason this lives in comet: a scan over the raw characters counts
        // these, and then every brace after them pairs with the wrong partner.
        let source = "func f() { let s = \"{{{\"; // }}}\n }";
        let all = brackets(source);
        assert_eq!(all.len(), 4, "two parens and two real braces: {all:?}");
        assert!(all.iter().all(|b| b.partner.is_some()));
    }

    #[test]
    fn an_unmatched_bracket_has_no_partner() {
        let all = brackets("func f() {");
        let brace = all.iter().find(|b| b.open && b.span.start == 9).unwrap();
        assert_eq!(brace.partner, None, "never closed");

        let all = brackets("} func f() { }");
        let stray = all.first().expect("the stray closer");
        assert!(!stray.open);
        assert_eq!(stray.partner, None);
    }

    #[test]
    fn a_pair_closed_by_the_wrong_bracket_matches_neither() {
        // Both ends are wrong, and marking both is what tells you which two.
        let all = brackets("func f( }");
        assert!(all.iter().all(|b| b.partner.is_none()), "{all:?}");
    }

    #[test]
    fn the_caret_finds_a_bracket_on_either_side_of_it() {
        // Just after a `}` is what "just typed one" feels like.
        let source = "func f() { }";
        assert!(bracket_at(source, 9).is_some(), "before the brace");
        assert!(bracket_at(source, 10).is_some(), "just after it");
        assert!(bracket_at(source, 5).is_none(), "in the middle of a name");
    }

    // --- completions ---

    fn labels(source: &str, offset: usize) -> Vec<String> {
        completions_at(source, offset)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    /// Completions at the caret marked by `|` in `source`.
    fn at_caret(source: &str) -> Vec<String> {
        let offset = source.find('|').expect("mark the caret with |");
        labels(&source.replace('|', ""), offset)
    }

    #[test]
    fn every_builtin_the_checker_knows_is_offered_and_explained() {
        // Two lists that must not drift: the checker decides what compiles, this
        // one decides what is offered, and a name in one but not the other is
        // either an unusable suggestion or a working call nobody can discover.
        let offered = at_caret("func update(dt: f32) { | }");
        for (name, _) in super::BUILTINS {
            assert!(
                offered.contains(&name.to_string()),
                "`{name}` is not offered"
            );
            assert!(
                hover_at(&format!("func f() {{ {name} }}"), 11).is_some(),
                "`{name}` has no hover"
            );
        }
        // And nothing is offered that would not compile.
        for name in ["sine", "square", "vec3"] {
            assert!(!offered.contains(&name.to_string()));
        }
    }

    #[test]
    fn a_name_in_two_scopes_is_offered_once() {
        // dedup_by only removes adjacent duplicates, so a shadowed name came
        // back twice with the two entries nowhere near each other in the list.
        let offered = at_caret(
            "let speed = 1.0;
             func update(speed: f32) {
                 let speed = speed;
                 |
             }",
        );
        assert_eq!(
            offered.iter().filter(|l| *l == "speed").count(),
            1,
            "{offered:?}"
        );
        // And a function whose name is also a type name is still offered once.
        let offered = at_caret("func f32() { }\nfunc update(dt: f32) { | }");
        assert_eq!(offered.iter().filter(|l| *l == "f32").count(), 1);
    }

    #[test]
    fn keywords_and_types_are_always_offered() {
        let offered = at_caret("func update(dt: f32) { | }");
        for want in ["let", "if", "while", "return", "f32", "Vec2", "String"] {
            assert!(offered.contains(&want.to_string()), "missing {want}");
        }
    }

    #[test]
    fn locals_and_parameters_in_scope_are_offered() {
        let offered = at_caret(
            "let speed = 1.0;
             func update(dt: f32) {
                 let step = speed * dt;
                 |
             }",
        );
        for want in ["pos", "speed", "dt", "step"] {
            assert!(offered.contains(&want.to_string()), "missing {want}");
        }
    }

    #[test]
    fn a_local_declared_after_the_caret_is_not_offered() {
        let offered = at_caret(
            "func update(dt: f32) {
                 |
                 let later = 1.0;
             }",
        );
        assert!(!offered.contains(&"later".to_string()));
    }

    #[test]
    fn another_functions_locals_are_not_offered() {
        let offered = at_caret(
            "func other(x: f32) { let hidden = x; }
             func update(dt: f32) { | }",
        );
        assert!(!offered.contains(&"hidden".to_string()));
        assert!(!offered.contains(&"x".to_string()));
        assert!(
            offered.contains(&"other".to_string()),
            "but the function itself is callable"
        );
    }

    #[test]
    fn a_completion_says_what_it_is() {
        // A list of bare names does not teach anyone what `pos` or `print` are.
        let source =
            "let speed = 1.0;\nfunc helper(a: f32) -> f32 { a }\nfunc update(dt: f32) {  }";
        let at = source.len() - 2;
        let items = completions_at(source, at);
        let detail = |name: &str| {
            items
                .iter()
                .find(|i| i.label == name)
                .map(|i| i.detail.as_str())
                .unwrap_or("<missing>")
        };
        assert_eq!(detail("dt"), "f32", "a parameter shows its type");
        assert_eq!(detail("pos"), "Vec2");
        assert_eq!(detail("helper"), "func helper(a: f32) -> f32");
        assert_eq!(detail("print"), "func print(s: String)");
        assert_eq!(detail("while"), "repeat a block while a condition holds");
    }

    #[test]
    fn a_field_completion_says_its_type_too() {
        // Just after the dot, which is where the caret is when you type one.
        let items = completions_at("func update(dt: f32) { pos. }", 27);
        assert_eq!(items[0].label, "x");
        assert_eq!(items[0].detail, "f32");
    }

    // --- hover ---

    #[test]
    fn hovering_a_name_says_what_it_is() {
        let source = "let speed = 1.0;\nfunc helper(a: f32) -> f32 { a }\nfunc update(dt: f32) { pos.x += dt; }";
        let at = |needle: &str| source.find(needle).unwrap() + 1;
        assert_eq!(hover_at(source, at("dt: f32")), Some("dt: f32".to_string()));
        assert_eq!(
            hover_at(source, at("helper")),
            Some("func helper(a: f32) -> f32".to_string())
        );
        assert_eq!(hover_at(source, at("pos.x")), Some("pos: Vec2".to_string()));
        assert_eq!(
            hover_at(source, at("speed")),
            Some("speed: f32".to_string()),
            "script state, whose type is inferred from its initializer"
        );
    }

    #[test]
    fn hovering_a_keyword_or_a_type_explains_it() {
        let source = "func update(dt: f32) { while true { } }";
        assert!(
            hover_at(source, source.find("while").unwrap() + 1)
                .is_some_and(|h| h.contains("repeat")),
        );
        assert_eq!(
            hover_at(source, source.find("f32").unwrap() + 1),
            Some("type f32".to_string())
        );
    }

    #[test]
    fn hovering_whitespace_or_an_unknown_name_says_nothing() {
        let source = "func update(dt: f32) { mystery; }";
        // Between two non-word characters there is no name to describe.
        assert_eq!(hover_at(source, 20), None, "the space before the brace");
        assert_eq!(hover_at(source, 25), None, "a name nothing declares");
        // But the boundary just after a word is still that word, which is what
        // hovering the end of one feels like.
        assert_eq!(hover_at(source, 4), Some("func - define a function".into()));
    }

    #[test]
    fn hover_survives_a_file_that_does_not_parse() {
        assert_eq!(
            hover_at("func update(dt: f32) { pos.x += dt", 32),
            Some("dt: f32".to_string())
        );
    }

    #[test]
    fn script_functions_and_builtins_are_offered() {
        let offered = at_caret("func helper(a: f32) -> f32 { a }\nfunc update(dt: f32) { | }");
        assert!(offered.contains(&"helper".to_string()));
        assert!(offered.contains(&"print".to_string()));
    }

    #[test]
    fn after_a_dot_on_pos_only_its_fields_are_offered() {
        let offered = at_caret("func update(dt: f32) { pos.| }");
        assert_eq!(offered, vec!["x", "y"], "and no keywords: {offered:?}");
    }

    #[test]
    fn a_partly_typed_field_still_offers_the_whole_set() {
        // The editor filters as you type; the service reports what exists.
        let offered = at_caret("func update(dt: f32) { pos.x| }");
        assert_eq!(offered, vec!["x", "y"]);
    }

    #[test]
    fn a_dot_on_an_annotated_vec2_offers_its_fields() {
        let offered = at_caret("func update(here: Vec2) { here.| }");
        assert_eq!(offered, vec!["x", "y"]);
        let offered = at_caret("let home: Vec2 = pos;\nfunc update(dt: f32) { home.| }");
        assert_eq!(offered, vec!["x", "y"]);
    }

    #[test]
    fn a_dot_on_something_with_no_fields_offers_nothing_rather_than_guessing() {
        // An f32 has no members, and a receiver whose type is not known yet is
        // not an invitation to offer a plausible-looking list.
        assert!(at_caret("func update(dt: f32) { dt.| }").is_empty());
        assert!(at_caret("func update(dt: f32) { mystery.| }").is_empty());
    }

    #[test]
    fn completions_work_on_a_file_that_does_not_parse() {
        // Which is the only state that matters: nobody asks for a completion
        // when the file is finished.
        let offered = at_caret(
            "func update(dt: f32) {
                 let speed = 2.0;
                 pos.x += sp|",
        );
        assert!(offered.contains(&"speed".to_string()), "{offered:?}");
    }

    #[test]
    fn asking_past_the_end_or_of_nothing_is_harmless() {
        assert!(!completions_at("func f() {}", 9999).is_empty());
        assert!(
            !completions_at("", 0).is_empty(),
            "keywords are still offered"
        );
    }
}
