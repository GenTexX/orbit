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
use crate::lexer::{Token, TokenKind, lex_with_comments};
use crate::parser::parse;
use crate::schema::HostSchema;
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
    pub fn new(source: &str, schema: &HostSchema) -> Self {
        let (with_comments, _) = lex_with_comments(source);
        let (script, mut diagnostics) = parse(source);
        let (_, checked) = crate::check::check(&script, schema);
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
pub fn diagnostics(source: &str, schema: &HostSchema) -> Vec<Diagnostic> {
    let (script, mut found) = parse(source);
    let (_, checked) = crate::check::check(&script, schema);
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
    /// Any other identifier: a local, a parameter, script state, `transform`.
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
            | TokenKind::Enum
            | TokenKind::Struct
            | TokenKind::Match
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
    /// `transform` or `print` actually are.
    pub detail: String,
}

/// Every occurrence of the identifier `offset` sits in, including that one.
///
/// Token-based rather than textual: `speed` does not light up inside `speeds`,
/// inside a comment, or inside a string. It is also deliberately not
/// scope-aware - two locals called `i` in different functions both highlight -
/// because the point is "show me this word", answered instantly on every caret
/// move, and a wrong answer here costs a faint background rather than a jump to
/// the wrong place.
pub fn occurrences_at(source: &str, offset: usize) -> Vec<Span> {
    let (tokens, _) = lex_with_comments(source);
    let Some(word) = tokens.iter().find_map(|token| match &token.kind {
        TokenKind::Ident(name)
            if offset >= token.span.start as usize && offset <= token.span.end as usize =>
        {
            Some(name.clone())
        }
        _ => None,
    }) else {
        return Vec::new();
    };
    tokens
        .iter()
        .filter(|token| matches!(&token.kind, TokenKind::Ident(name) if *name == word))
        .map(|token| token.span)
        .collect()
}

/// Whether `offset` falls inside a comment or a string literal.
///
/// A token's span covers the text it was made from, so a caret strictly inside
/// one is inside that token. The far edge counts as inside too: after the
/// closing quote of `"ab"` the caret is back in code, but at `"ab|"` it is
/// still in the string. An unterminated string runs to the end of the line,
/// which is what makes a half-typed `"` suppress the list rather than offering
/// completions inside the string being written.
fn in_comment_or_string(source: &str, offset: usize) -> bool {
    let (tokens, _) = lex_with_comments(source);
    tokens.iter().any(|token| {
        let (start, end) = (token.span.start as usize, token.span.end as usize);
        let inside = offset > start && offset < end;
        match token.kind {
            TokenKind::Comment => offset > start && offset <= end,
            TokenKind::Str(_) => inside,
            _ => false,
        }
    })
}

/// The keywords a script can start a statement with.
const KEYWORDS: &[&str] = &[
    "func", "let", "if", "else", "while", "for", "in", "enum", "match", "return", "true", "false",
];
/// The type names a script can write.
const TYPES: &[&str] = &["f32", "int", "bool", "Vec2", "String", "Option", "Array"];
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
    ("int", "func int(value: f32) -> int"),
    ("len", "func len(a: Array<T>) -> int"),
    ("push", "func push(a: Array<T>, value: T)"),
    ("copy", "func copy(a: Array<T>) -> Array<T>"),
    ("sin", "func sin(a: f32) -> f32"),
    ("cos", "func cos(a: f32) -> f32"),
    ("atan2", "func atan2(y: f32, x: f32) -> f32"),
    ("pow", "func pow(a: f32, b: f32) -> f32"),
];
/// The magic name bound to the owning node's position.
/// `Vec2`'s whole member set.
const VEC2_FIELDS: &[&str] = &["x", "y"];

/// What can be written at `offset` in `source`.
///
/// After a `.`, the receiver's fields and nothing else - offering keywords there
/// would be noise. Otherwise: the names in scope at that point, the script's own
/// functions, the builtins, the types, and the keywords.
///
/// Resolution is single-file, because a v1 script is (there is no `import`).
pub fn completions_at(source: &str, schema: &HostSchema, offset: usize) -> Vec<CompletionItem> {
    let offset = offset.min(source.len());
    // Inside a comment or a string literal there is no code to complete, and a
    // list popping up over prose is pure noise. The lexer already knows which
    // is which, so this asks it rather than guessing from quote counts.
    if in_comment_or_string(source, offset) {
        return Vec::new();
    }
    let (script, _) = parse(source);

    if let Some(receiver) = field_receiver(source, offset) {
        // A host object offers its own properties. This is the completion that
        // makes a schema-driven surface discoverable at all: nobody remembers
        // whether it is `position` or `translation` without being shown.
        // `transform.position.` - a property's own type decides what follows.
        if let Some((object, property)) = receiver.split_once('.')
            && let Some(found) = schema.resolve(object, property)
        {
            return match found.ty.ty() {
                Type::Vec2 => VEC2_FIELDS
                    .iter()
                    .map(|name| detailed(name, CompletionKind::Field, "f32".to_string()))
                    .collect(),
                _ => Vec::new(),
            };
        }
        if schema.has_object(receiver) {
            return schema
                .fields_of(receiver)
                .iter()
                .map(|field| {
                    detailed(
                        &field.name,
                        CompletionKind::Field,
                        field.ty.ty().name().to_string(),
                    )
                })
                .collect();
        }
        return match receiver_type(&script, schema, offset, receiver) {
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
    for name in schema.object_names() {
        items.push(detailed(
            name,
            CompletionKind::Variable,
            "engine properties".to_string(),
        ));
    }
    for name in names_in_scope(&script, offset) {
        let detail = type_of_name(&script, schema, offset, &name)
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
        "for" => "repeat a block once per whole number in a range: `for i in 0..4`",
        "enum" => "declare a type that is one of several named variants",
        "struct" => "declare a type that groups named fields - a value, copied on assignment",
        "match" => "pick a value by which variant something is - every variant needs an arm",
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
fn type_of_name(script: &Script, schema: &HostSchema, offset: usize, name: &str) -> Option<Type> {
    if script.state.iter().any(|s| s.name == name) {
        let (typed, _) = crate::check::check(script, schema);
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
/// The call the caret is inside, for the hint shown while typing arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct SignatureHelp {
    /// The whole signature, as it would be written.
    pub signature: String,
    /// Each parameter as `name: Type`, so the caller can emphasise one.
    pub params: Vec<String>,
    /// Which parameter the caret is on. Can be past the end of `params` when
    /// more arguments have been typed than the function takes - which is worth
    /// showing rather than clamping away, because it is the mistake.
    pub active: usize,
}

/// The signature of the call `offset` is inside, and which argument it is on.
///
/// `None` when the caret is not between a call's parentheses, or the callee is
/// not something with a known signature. Nested calls resolve to the innermost:
/// in `max(1.0, min(|`, the caret is on `min`'s first argument.
pub fn signature_at(source: &str, offset: usize) -> Option<SignatureHelp> {
    let (tokens, _) = lex_with_comments(source);
    // Only tokens strictly before the caret matter, and comments are not part
    // of the call - a `(` inside one must not open anything.
    let before: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.span.end as usize <= offset && t.kind != TokenKind::Comment)
        .collect();

    // Walk back for the innermost paren that is still open, counting commas at
    // that depth on the way - which is the active argument.
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut open: Option<usize> = None;
    for (i, token) in before.iter().enumerate().rev() {
        match token.kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen if depth == 0 => {
                open = Some(i);
                break;
            }
            TokenKind::LParen => depth -= 1,
            TokenKind::Comma if depth == 0 => commas += 1,
            _ => {}
        }
    }
    let open = open?;
    // The name in front of the paren is the callee. Without one this is a
    // grouping paren, not a call.
    let TokenKind::Ident(name) = &before.get(open.checked_sub(1)?)?.kind else {
        return None;
    };

    let (script, _) = parse(source);
    if let Some(function) = script.functions.iter().find(|f| f.name == *name) {
        return Some(SignatureHelp {
            signature: signature_of(function),
            params: function
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.name))
                .collect(),
            active: commas,
        });
    }
    let (_, signature) = BUILTINS.iter().find(|(builtin, _)| builtin == name)?;
    Some(SignatureHelp {
        signature: signature.to_string(),
        params: params_of(signature),
        active: commas,
    })
}

/// The parameter list out of a written signature like
/// `func min(a: f32, b: f32) -> f32`.
fn params_of(signature: &str) -> Vec<String> {
    let Some(open) = signature.find('(') else {
        return Vec::new();
    };
    let Some(close) = signature[open..].find(')') else {
        return Vec::new();
    };
    let inner = &signature[open + 1..open + close];
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner.split(',').map(|p| p.trim().to_string()).collect()
}

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
pub fn hover_at(source: &str, schema: &HostSchema, offset: usize) -> Option<String> {
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
    if schema.has_object(name) {
        let properties: Vec<String> = schema
            .fields_of(name)
            .iter()
            .map(|f| format!("{}: {}", f.name, f.ty.ty().name()))
            .collect();
        return Some(format!("{name} - {}", properties.join(", ")));
    }
    let ty = type_of_name(&script, schema, offset, name)?;
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

/// What declares the name a jump landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionKind {
    /// A `let` inside a function body.
    Local,
    /// A function parameter.
    Param,
    /// A `for` loop's counter.
    LoopVar,
    /// A top-level `let`: script state.
    State,
    Function,
    /// A host function like `print`. Nowhere to jump to, but worth saying so
    /// rather than doing nothing.
    Builtin,
    /// A type name like `Vec2`. Also nowhere to jump to.
    Type,
}

impl DefinitionKind {
    /// Whether this is somewhere in the file, as opposed to something the
    /// engine provides.
    pub fn is_in_source(self) -> bool {
        !matches!(self, DefinitionKind::Builtin | DefinitionKind::Type)
    }

    /// What to call it in a message.
    pub fn describe(self) -> &'static str {
        match self {
            DefinitionKind::Local => "a local",
            DefinitionKind::Param => "a parameter",
            DefinitionKind::LoopVar => "a loop variable",
            DefinitionKind::State => "script state",
            DefinitionKind::Function => "a function",
            DefinitionKind::Builtin => "a built-in function",
            DefinitionKind::Type => "a built-in type",
        }
    }
}

/// What a name at some offset refers to.
#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    pub name: String,
    pub kind: DefinitionKind,
    /// Where the declaration's name is written. Empty for a builtin or a type,
    /// which are declared nowhere in the file.
    pub span: Span,
}

/// What the name at `offset` refers to, or `None` if `offset` is not in a name
/// or nothing declares it.
///
/// Real scoping, innermost first: a `let` in an inner block shadows an outer
/// one, and a declaration below the caret does not count. That is stricter than
/// [`completions_at`], which deliberately over-offers - a name it suggests that
/// turns out not to compile is a small annoyance, but a *jump* to the wrong
/// declaration teaches something false.
pub fn definition_at(source: &str, schema: &HostSchema, offset: usize) -> Option<Definition> {
    let (start, end) = word_span(source, offset)?;
    let name = &source[start..end];
    if name.is_empty() {
        return None;
    }
    // A call resolves against functions first: `print` the call is the builtin
    // even if some local is also called `print`.
    let called = source[end..].trim_start().starts_with('(');
    resolve(
        source,
        schema,
        offset,
        name,
        called,
        Span::new(start as u32, end as u32),
    )
}

/// What `name` would refer to if it were written at `offset` - which is how a
/// rename learns whether the name it is about to introduce already means
/// something there.
pub fn declaration_visible_at(
    source: &str,
    schema: &HostSchema,
    offset: usize,
    name: &str,
) -> Option<Definition> {
    resolve(source, schema, offset, name, false, Span::new(0, 0))
}

/// The shared scope walk behind [`definition_at`] and
/// [`declaration_visible_at`]. `written` is where the name appears, used only
/// as the span of an engine-provided name, which is declared nowhere.
fn resolve(
    source: &str,
    schema: &HostSchema,
    offset: usize,
    name: &str,
    called: bool,
    written: Span,
) -> Option<Definition> {
    let (script, _) = parse(source);

    // Standing on a declaration resolves to that declaration. Without this the
    // scope walk below, which only looks above the caret, finds nothing for a
    // caret on `speed` in `let speed = 1.0;` - so go-to-definition on a
    // declaration did nothing, and a rename started from one refused.
    if written.end > written.start
        && let Some(found) = declaration_at(&script, offset, name)
    {
        return Some(found);
    }

    if called {
        if let Some(function) = script.functions.iter().find(|f| f.name == *name) {
            return Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::Function,
                span: function.name_span,
            });
        }
        if BUILTINS.iter().any(|(builtin, _)| *builtin == name) {
            return Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::Builtin,
                span: written,
            });
        }
    }

    // Inside a function: its blocks innermost-first, then its parameters.
    if let Some(function) = enclosing_function(&script, offset) {
        if let Some(found) = block_definition(&function.body, offset, name) {
            return Some(found);
        }
        if let Some(param) = function.params.iter().find(|p| p.name == *name) {
            return Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::Param,
                span: param.name_span,
            });
        }
    }

    if let Some(state) = script.state.iter().find(|s| s.name == *name) {
        return Some(Definition {
            name: name.to_string(),
            kind: DefinitionKind::State,
            span: state.name_span,
        });
    }
    if let Some(function) = script.functions.iter().find(|f| f.name == *name) {
        return Some(Definition {
            name: name.to_string(),
            kind: DefinitionKind::Function,
            span: function.name_span,
        });
    }
    let engine = if BUILTINS.iter().any(|(builtin, _)| *builtin == name) {
        DefinitionKind::Builtin
    } else if TYPES.contains(&name) || schema.has_object(name) {
        DefinitionKind::Type
    } else {
        return None;
    };
    Some(Definition {
        name: name.to_string(),
        kind: engine,
        span: written,
    })
}

/// Every span a rename of the symbol at `offset` must rewrite: its declaration
/// and every use that resolves to it.
///
/// Each candidate occurrence is resolved on its own and kept only if it lands
/// on the same declaration, so a shadowed inner `x` is left alone and a field
/// `transform.position.x` is never touched. That is one resolution per occurrence, which is
/// more work than a textual sweep and is the entire point: renaming is the one
/// operation where being nearly right corrupts the file.
///
/// `None` when `offset` is not on a renameable name - a builtin or a type has
/// no declaration in this file to rename.
pub fn rename_spans(source: &str, schema: &HostSchema, offset: usize) -> Option<Vec<Span>> {
    let target = definition_at(source, schema, offset)?;
    if !target.kind.is_in_source() {
        return None;
    }
    let mut spans: Vec<Span> = occurrences_at(source, target.span.start as usize)
        .into_iter()
        .filter(|span| {
            definition_at(source, schema, span.start as usize)
                .is_some_and(|found| found.span == target.span)
        })
        .collect();
    // The declaration itself, in case it is not among the occurrences - which
    // happens when the name is written somewhere the token scan does not reach
    // back to, and costs nothing to guard against.
    if !spans.contains(&target.span) {
        spans.push(target.span);
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();
    Some(spans)
}

/// Whether renaming the symbol at `offset` to `name` is safe.
///
/// A rename onto a name that already means something where the symbol is used
/// would silently capture every use of it, so it is refused rather than done -
/// in a language whose only feedback would be `cannot find X in this scope`,
/// quietly changing which declaration a name resolves to is the worst outcome
/// available.
pub fn rename_is_safe(source: &str, schema: &HostSchema, offset: usize, name: &str) -> bool {
    is_identifier(name) && declaration_visible_at(source, schema, offset, name).is_none()
}

/// Whether `text` is a single identifier - what a rename is allowed to produce.
pub fn is_identifier(text: &str) -> bool {
    // Through the lexer, so "what is a name" has one definition: one Ident
    // token and nothing else. A keyword lexes as itself and is refused here,
    // which is what stops a rename to `while`.
    let (tokens, diagnostics) = crate::lexer::lex(text);
    diagnostics.is_empty() && tokens.len() == 2 && matches!(tokens[0].kind, TokenKind::Ident(_))
}

/// The declaration whose own name `offset` sits in, if it is on one.
fn declaration_at(script: &Script, offset: usize, name: &str) -> Option<Definition> {
    let on = |span: Span| contains(span, offset);
    for state in &script.state {
        if state.name == name && on(state.name_span) {
            return Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::State,
                span: state.name_span,
            });
        }
    }
    for function in &script.functions {
        if function.name == name && on(function.name_span) {
            return Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::Function,
                span: function.name_span,
            });
        }
        for param in &function.params {
            if param.name == name && on(param.name_span) {
                return Some(Definition {
                    name: name.to_string(),
                    kind: DefinitionKind::Param,
                    span: param.name_span,
                });
            }
        }
        if let Some(found) = block_declaration(&function.body, offset, name) {
            return Some(found);
        }
    }
    None
}

/// The `let` or `for` in `block` whose own name `offset` sits in.
fn block_declaration(block: &Block, offset: usize, name: &str) -> Option<Definition> {
    for stmt in &block.stmts {
        let found = match stmt {
            Stmt::Let {
                name: bound,
                name_span,
                ..
            } if bound == name && contains(*name_span, offset) => Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::Local,
                span: *name_span,
            }),
            Stmt::For {
                name: counter,
                name_span,
                ..
            } if counter == name && contains(*name_span, offset) => Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::LoopVar,
                span: *name_span,
            }),
            Stmt::If(if_stmt) => if_declaration(if_stmt, offset, name),
            Stmt::While { body, .. } | Stmt::For { body, .. } => {
                block_declaration(body, offset, name)
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn if_declaration(if_stmt: &crate::ast::IfStmt, offset: usize, name: &str) -> Option<Definition> {
    if let Some(found) = block_declaration(&if_stmt.then, offset, name) {
        return Some(found);
    }
    match if_stmt.otherwise.as_deref() {
        Some(Else::Block(block)) => block_declaration(block, offset, name),
        Some(Else::If(nested)) => if_declaration(nested, offset, name),
        None => None,
    }
}

/// The declaration of `name` visible at `offset` within `block`, innermost
/// scope first.
///
/// Recurses into the nested block containing `offset` before looking at this
/// one, so an inner `let` wins; and only considers declarations written above
/// the caret, because a `let` further down has not happened yet.
fn block_definition(block: &Block, offset: usize, name: &str) -> Option<Definition> {
    // Innermost first: whichever nested block holds the offset gets asked
    // before this one does. Only the one that *holds* it - recursing into every
    // nested block regardless is what makes visit_lets loose, and it resolved a
    // use after an `if` to a `let` inside that `if`.
    for stmt in &block.stmts {
        let found = match stmt {
            Stmt::If(if_stmt) => if_definition(if_stmt, offset, name),
            Stmt::While { body, .. } | Stmt::For { body, .. } if contains(body.span, offset) => {
                block_definition(body, offset, name)
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        // A `for` counter is in scope only inside its own body.
        if let Stmt::For {
            name: counter,
            name_span,
            body,
            ..
        } = stmt
            && counter == name
            && contains(body.span, offset)
        {
            return Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::LoopVar,
                span: *name_span,
            });
        }
    }
    // Then this block's own lets, last one above the caret winning - a later
    // `let` of the same name shadows the earlier one from there on.
    block
        .stmts
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Let {
                name: bound,
                name_span,
                span,
                ..
            } if bound == name && (span.end as usize) <= offset => Some(Definition {
                name: name.to_string(),
                kind: DefinitionKind::Local,
                span: *name_span,
            }),
            _ => None,
        })
        .next_back()
}

fn if_definition(if_stmt: &crate::ast::IfStmt, offset: usize, name: &str) -> Option<Definition> {
    if contains(if_stmt.then.span, offset)
        && let Some(found) = block_definition(&if_stmt.then, offset, name)
    {
        return Some(found);
    }
    match if_stmt.otherwise.as_deref() {
        Some(Else::Block(block)) if contains(block.span, offset) => {
            block_definition(block, offset, name)
        }
        Some(Else::If(nested)) => if_definition(nested, offset, name),
        _ => None,
    }
}

fn contains(span: Span, offset: usize) -> bool {
    offset >= span.start as usize && offset <= span.end as usize
}

/// What kind of thing a [`Symbol`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    /// A top-level `let`: script state, which survives across calls.
    State,
}

/// One named thing a script declares at the top level.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// The span of the name itself, so jumping to it lands on the word rather
    /// than on the `func` in front of it.
    pub span: Span,
    /// The 1-based line the declaration is on, for a "name:line" readout.
    pub line: usize,
}

/// Everything a script declares at the top level, in source order.
///
/// State first or functions first is not this function's call to make: they
/// come back in the order they were written, because a palette that reorders
/// what you wrote is harder to navigate than one that does not.
pub fn symbols(source: &str) -> Vec<Symbol> {
    let (script, _) = parse(source);
    let line_of = |at: u32| {
        source[..(at as usize).min(source.len())]
            .matches('\n')
            .count()
            + 1
    };
    let mut all: Vec<Symbol> = script
        .state
        .iter()
        .map(|state| Symbol {
            name: state.name.clone(),
            kind: SymbolKind::State,
            span: state.name_span,
            line: line_of(state.name_span.start),
        })
        .chain(script.functions.iter().map(|function| Symbol {
            name: function.name.clone(),
            kind: SymbolKind::Function,
            span: function.name_span,
            line: line_of(function.name_span.start),
        }))
        .filter(|symbol| !symbol.name.is_empty())
        .collect();
    all.sort_by_key(|symbol| symbol.span.start);
    all
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

/// The dotted path before the `.` immediately left of `offset`, if the caret is
/// in a field position - `transform`, or `transform.position`.
///
/// Reads the source rather than the tree: `transform.` does not parse as a field
/// access, and that half-typed state is exactly when completions are asked for.
/// The whole path is returned rather than the last segment, because
/// `transform.position.` and a local called `position` mean different things.
fn field_receiver(source: &str, offset: usize) -> Option<&str> {
    let before = &source[..offset];
    // Skip the partial field name already typed.
    let name_start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |at| at + 1);
    let before = &before[..name_start];
    let dot = before.strip_suffix('.')?;
    let start = dot
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .map_or(0, |at| at + 1);
    let receiver = dot[start..].trim_end_matches('.');
    (!receiver.is_empty()).then_some(receiver)
}

/// The type of `receiver` as seen from `offset`: a declared local, a parameter,
/// or a piece of script state. Host objects are handled by the caller, since
/// they complete to property names rather than to a type's fields.
fn receiver_type(
    script: &Script,
    schema: &HostSchema,
    offset: usize,
    receiver: &str,
) -> Option<Type> {
    let _ = schema;
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

    /// The service's public functions all take the host schema now. These
    /// shadow them with the example one, so a test about completions is not
    /// three quarters schema-passing noise.
    fn schema() -> HostSchema {
        crate::schema::example_schema()
    }
    fn diagnostics(source: &str) -> Vec<Diagnostic> {
        super::diagnostics(source, &schema())
    }
    fn completions_at(source: &str, offset: usize) -> Vec<CompletionItem> {
        super::completions_at(source, &schema(), offset)
    }
    fn hover_at(source: &str, offset: usize) -> Option<String> {
        super::hover_at(source, &schema(), offset)
    }
    fn definition_at(source: &str, offset: usize) -> Option<Definition> {
        super::definition_at(source, &schema(), offset)
    }
    fn rename_spans(source: &str, offset: usize) -> Option<Vec<Span>> {
        super::rename_spans(source, &schema(), offset)
    }
    fn rename_is_safe(source: &str, offset: usize, name: &str) -> bool {
        super::rename_is_safe(source, &schema(), offset, name)
    }

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
        let classes = classes("func update(dt: f32) { if transform.position.x > { ");
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
        let analysis = Analysis::new(source, &schema());
        assert_eq!(analysis.diagnostics, diagnostics(source));
        assert_eq!(analysis.tokens, highlight(source));
        assert_eq!(analysis.brackets, brackets(source));
    }

    #[test]
    fn an_analysis_finds_a_bracket_the_same_way_the_standalone_call_does() {
        let source = "func f() { }";
        let analysis = Analysis::new(source, &schema());
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
        for want in ["transform", "speed", "dt", "step"] {
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
        // A list of bare names does not teach anyone what `transform` or `print` are.
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
        assert_eq!(detail("transform"), "engine properties");
        assert_eq!(detail("helper"), "func helper(a: f32) -> f32");
        assert_eq!(detail("print"), "func print(s: String)");
        assert_eq!(detail("while"), "repeat a block while a condition holds");
    }

    #[test]
    fn a_field_completion_says_its_type_too() {
        // Just after the dot, which is where the caret is when you type one.
        let source = "func update(dt: f32) { transform.position. }";
        let at = source.find("position.").unwrap() + "position.".len();
        let items = completions_at(source, at);
        assert_eq!(items[0].label, "x");
        assert_eq!(items[0].detail, "f32");
    }

    #[test]
    fn a_dot_on_a_host_object_offers_its_properties_with_their_types() {
        // The completion that makes a schema-driven surface discoverable at
        // all: nobody remembers whether it is `position` or `translation`.
        let offered = at_caret("func update(dt: f32) { transform.| }");
        assert_eq!(offered, vec!["position", "rotation", "scale"]);

        let source = "func update(dt: f32) { transform. }";
        let at = source.find("transform.").unwrap() + "transform.".len();
        let items = completions_at(source, at);
        assert_eq!(items[0].detail, "Vec2");
        assert_eq!(items[1].detail, "f32");
    }

    // --- hover ---

    #[test]
    fn hovering_a_name_says_what_it_is() {
        let source = "let speed = 1.0;\nfunc helper(a: f32) -> f32 { a }\nfunc update(dt: f32) { transform.position.x += dt; }";
        let at = |needle: &str| source.find(needle).unwrap() + 1;
        assert_eq!(hover_at(source, at("dt: f32")), Some("dt: f32".to_string()));
        assert_eq!(
            hover_at(source, at("helper")),
            Some("func helper(a: f32) -> f32".to_string())
        );
        assert_eq!(
            hover_at(source, at("transform.position.x")),
            Some("transform - position: Vec2, rotation: f32, scale: Vec2".to_string()),
            "a host object lists what it holds, since that is the thing nobody remembers"
        );
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
            hover_at("func update(dt: f32) { transform.position.x += dt", 47),
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
    fn after_a_dot_on_a_vec2_property_only_its_axes_are_offered() {
        let offered = at_caret("func update(dt: f32) { transform.position.| }");
        assert_eq!(offered, vec!["x", "y"], "and no keywords: {offered:?}");
    }

    #[test]
    fn a_partly_typed_field_still_offers_the_whole_set() {
        // The editor filters as you type; the service reports what exists.
        let offered = at_caret("func update(dt: f32) { transform.position.x| }");
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
                 transform.position.x += sp|",
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

    #[test]
    fn a_comment_offers_no_completions() {
        let source = "func f() {\n    // let me think\n}";
        let in_comment = source.find("think").unwrap() + 2;
        assert!(completions_at(source, in_comment).is_empty());

        // A block comment counts too, and the line the comment is on still
        // completes before the `//` starts.
        let source = "func f() {\n    let x = 1.0; /* le */\n}";
        assert!(completions_at(source, source.find("le */").unwrap() + 2).is_empty());
        let before = source.find("let x").unwrap() + 2;
        assert!(!completions_at(source, before).is_empty());
    }

    #[test]
    fn a_string_literal_offers_no_completions() {
        let source = r#"func f() { print("let me in"); }"#;
        let inside = source.find("me in").unwrap();
        assert!(completions_at(source, inside).is_empty());

        // Just past the closing quote is code again.
        let after = source.find("\");").unwrap() + 1;
        assert!(!completions_at(source, after).is_empty());
    }

    #[test]
    fn a_half_typed_string_suppresses_the_list_to_the_end_of_its_line() {
        // The lexer runs an unterminated string to the line end, which is
        // exactly what should happen while one is being typed.
        let source = "func f() {\n    print(\"le\n}";
        assert!(completions_at(source, source.find("le").unwrap() + 2).is_empty());
    }

    #[test]
    fn occurrences_are_the_same_word_and_not_the_words_containing_it() {
        let source = "func f() {\n    let pos = 1.0;\n    let position = pos;\n}";
        let at = source.find("pos =").unwrap();
        let found: Vec<&str> = occurrences_at(source, at)
            .iter()
            .map(|s| &source[s.start as usize..s.end as usize])
            .collect();
        assert_eq!(found, ["pos", "pos"], "not the `pos` inside `position`");

        // The caret has to be in an identifier for there to be an answer.
        assert!(occurrences_at(source, source.find('{').unwrap()).is_empty());
    }

    #[test]
    fn occurrences_ignore_comments_and_strings() {
        let source = "func f() {\n    let x = 1.0; // x\n    print(\"x\");\n}";
        assert_eq!(
            occurrences_at(source, source.find("x =").unwrap()).len(),
            1,
            "the declaration only"
        );
    }

    #[test]
    fn symbols_come_back_in_the_order_they_were_written() {
        let source = "let speed: f32 = 1.0;\nfunc update(dt: f32) { }\nlet hits: f32 = 0.0;\nfunc reset() { }";
        let all = symbols(source);
        let found: Vec<(&str, SymbolKind, usize)> = all
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.line))
            .collect();
        assert_eq!(
            found,
            [
                ("speed", SymbolKind::State, 1),
                ("update", SymbolKind::Function, 2),
                ("hits", SymbolKind::State, 3),
                ("reset", SymbolKind::Function, 4),
            ]
        );
    }

    #[test]
    fn a_symbol_span_is_the_name_and_not_the_keyword_in_front_of_it() {
        let source = "func update(dt: f32) { }";
        let symbol = &symbols(source)[0];
        assert_eq!(
            &source[symbol.span.start as usize..symbol.span.end as usize],
            "update"
        );
    }

    #[test]
    fn a_file_that_does_not_parse_still_lists_what_it_can() {
        // The palette has to work while the file is being typed, which is most
        // of the time it is useful.
        let source = "func first() { let = ; }\nfunc second() { }";
        let all = symbols(source);
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["first", "second"]);
    }

    #[test]
    fn a_definition_is_the_innermost_declaration_above_the_caret() {
        let source = "\
func f(dt: f32) {
    let x = 1.0;
    if dt > 0.0 {
        let x = 2.0;
        print(str(x));
    }
    print(str(x));
}";
        // find() is the use inside the `if`; rfind() the one after it.
        let inner_use = source.find("str(x)").expect("in the fixture") + 4;
        let outer_use = source.rfind("str(x)").expect("in the fixture") + 4;
        // The two uses of `x` are in different scopes and must not resolve to
        // the same declaration.
        let inner = definition_at(source, inner_use).expect("resolves");
        let outer = definition_at(source, outer_use).expect("resolves");
        assert_eq!(inner.kind, DefinitionKind::Local);
        assert_ne!(inner.span, outer.span);
        assert_eq!(inner.span.start as usize, source.find("x = 2.0").unwrap());
        assert_eq!(outer.span.start as usize, source.find("x = 1.0").unwrap());
    }

    #[test]
    fn a_declaration_below_the_caret_does_not_count() {
        let source = "func f() {\n    print(str(y));\n    let y = 1.0;\n}";
        let at = source.find("str(y)").expect("in the fixture") + 4;
        assert_eq!(definition_at(source, at), None, "`y` has not happened yet");
    }

    #[test]
    fn parameters_state_and_functions_all_resolve() {
        let source = "\
let speed: f32 = 1.0;
func helper(a: f32) -> f32 { a }
func update(dt: f32) {
    print(str(helper(dt * speed)));
}";
        let kind = |needle: &str, skip: usize| {
            definition_at(source, source.rfind(needle).unwrap() + skip)
                .expect("resolves")
                .kind
        };
        assert_eq!(kind("dt *", 0), DefinitionKind::Param);
        assert_eq!(kind("speed)", 0), DefinitionKind::State);
        assert_eq!(kind("helper(dt", 0), DefinitionKind::Function);

        // The jump lands on the name, not on `func` or on `a: f32`.
        let def = definition_at(source, source.rfind("dt *").unwrap()).expect("resolves");
        assert_eq!(
            &source[def.span.start as usize..def.span.end as usize],
            "dt"
        );
    }

    #[test]
    fn a_loop_variable_resolves_only_inside_its_own_loop() {
        let source = "func f() {\n    for i in 0.0..3.0 {\n        print(str(i));\n    }\n}";
        let inside = source.find("str(i)").expect("in the fixture") + 4;
        let found = definition_at(source, inside).expect("resolves");
        assert_eq!(found.kind, DefinitionKind::LoopVar);
        assert_eq!(found.span.start as usize, source.find("i in").unwrap());
    }

    #[test]
    fn the_engines_own_names_say_what_they_are_rather_than_nothing() {
        let source = r#"func f() { print("hi"); }"#;
        let found = definition_at(source, source.find("print").unwrap()).expect("resolves");
        assert_eq!(found.kind, DefinitionKind::Builtin);
        assert!(!found.kind.is_in_source(), "nowhere in the file to jump to");
    }

    #[test]
    fn a_rename_reaches_the_declaration_and_every_use_that_resolves_to_it() {
        let source = "\
func f(dt: f32) {
    let speed = 1.0;
    if dt > 0.0 {
        let speed = 2.0;
        print(str(speed));
    }
    print(str(speed * dt));
}";
        let at = source.find("speed = 1.0").expect("in the fixture");
        let spans = rename_spans(source, at).expect("renameable");
        // The declaration and the use after the `if` - not the shadowed pair
        // inside it.
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert_eq!(spans[0].start as usize, at);
        assert_eq!(spans[1].start as usize, source.rfind("speed * dt").unwrap());
    }

    #[test]
    fn renaming_the_shadowing_binding_touches_only_its_own_scope() {
        let source = "\
func f() {
    let x = 1.0;
    if true {
        let x = 2.0;
        print(str(x));
    }
    print(str(x));
}";
        let inner = source.find("x = 2.0").expect("in the fixture");
        let spans = rename_spans(source, inner).expect("renameable");
        assert_eq!(spans.len(), 2);
        assert!(
            spans
                .iter()
                .all(|s| (s.start as usize) < source.rfind("str(x)").unwrap()),
            "the use after the block belongs to the outer binding"
        );
    }

    #[test]
    fn a_builtin_cannot_be_renamed() {
        let source = r#"func f() { print("hi"); }"#;
        assert_eq!(rename_spans(source, source.find("print").unwrap()), None);
    }

    #[test]
    fn a_rename_onto_a_name_that_already_means_something_is_refused() {
        let source = "func f(dt: f32) {\n    let speed = 1.0;\n    print(str(speed * dt));\n}";
        let at = source.find("speed = 1.0").expect("in the fixture");
        assert!(rename_is_safe(source, at, "velocity"));
        assert!(
            !rename_is_safe(source, at, "dt"),
            "would capture the parameter"
        );
        assert!(
            !rename_is_safe(source, at, "print"),
            "would shadow the builtin"
        );

        // And the result has to be a name at all.
        assert!(
            !rename_is_safe(source, at, "while"),
            "a keyword is not a name"
        );
        assert!(!rename_is_safe(source, at, "two words"));
        assert!(!rename_is_safe(source, at, ""));
        assert!(!rename_is_safe(source, at, "1st"));
    }

    #[test]
    fn signature_help_names_the_call_and_the_argument_being_typed() {
        let source = "func f(a: f32) { min(1.0, 2.0); }";
        let first = source.find("1.0").expect("in the fixture");
        let second = source.find("2.0").expect("in the fixture");

        let help = signature_at(source, first).expect("inside a call");
        assert_eq!(help.signature, "func min(a: f32, b: f32) -> f32");
        assert_eq!(help.params, ["a: f32", "b: f32"]);
        assert_eq!(help.active, 0);
        assert_eq!(
            signature_at(source, second).expect("still inside").active,
            1
        );
    }

    #[test]
    fn a_nested_call_resolves_to_the_innermost_one() {
        let source = "func f() { max(1.0, min(2.0, 3.0)); }";
        let inner = source.find("2.0").expect("in the fixture");
        let help = signature_at(source, inner).expect("inside min");
        assert!(help.signature.starts_with("func min"), "{}", help.signature);
        assert_eq!(help.active, 0);

        // And back out to the outer call after the inner one closes.
        let after = source.find("));").expect("in the fixture") + 1;
        let help = signature_at(source, after).expect("inside max");
        assert!(help.signature.starts_with("func max"), "{}", help.signature);
        assert_eq!(help.active, 1);
    }

    #[test]
    fn a_script_defined_function_gets_its_own_signature() {
        let source = "func aim(x: f32, y: f32) -> Vec2 { vec2(x, y) }\nfunc f() { aim(1.0, ); }";
        let at = source.rfind("1.0").expect("in the fixture");
        let help = signature_at(source, at).expect("inside aim");
        assert_eq!(help.signature, "func aim(x: f32, y: f32) -> Vec2");
        assert_eq!(help.params, ["x: f32", "y: f32"]);
    }

    #[test]
    fn there_is_no_signature_where_there_is_no_call() {
        // A grouping paren is not a call.
        assert_eq!(signature_at("func f() { let x = (1.0 + 2.0); }", 22), None);
        // Nor is a closed call you have stepped out of.
        let source = "func f() { min(1.0, 2.0); }";
        assert_eq!(signature_at(source, source.find(';').unwrap()), None);
        // A paren inside a comment opens nothing.
        let source = "func f() {\n    // min(\n    let x = 1.0;\n}";
        assert_eq!(signature_at(source, source.find("let x").unwrap()), None);
    }

    #[test]
    fn too_many_arguments_shows_as_being_past_the_end() {
        // Clamping would hide the mistake; the count is the useful part.
        let source = "func f() { min(1.0, 2.0, 3.0); }";
        let third = source.find("3.0").expect("in the fixture");
        let help = signature_at(source, third).expect("inside a call");
        assert_eq!(help.active, 2);
        assert_eq!(help.params.len(), 2, "one more argument than it takes");
    }
}
