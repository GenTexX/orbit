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
}

/// The keywords a script can start a statement with.
const KEYWORDS: &[&str] = &[
    "func", "let", "if", "else", "while", "return", "true", "false",
];
/// The type names a script can write.
const TYPES: &[&str] = &["f32", "bool", "Vec2", "String"];
/// The functions the engine provides.
const BUILTINS: &[&str] = &["print"];
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
                .map(|name| item(name, CompletionKind::Field))
                .collect(),
            // An unknown receiver offers nothing rather than guessing: a wrong
            // list read as a real one is worse than an empty one read as "I do
            // not know yet".
            _ => Vec::new(),
        };
    }

    let mut items = Vec::new();
    for name in names_in_scope(&script, offset) {
        items.push(item(&name, CompletionKind::Variable));
    }
    for function in &script.functions {
        if !function.name.is_empty() {
            items.push(item(&function.name, CompletionKind::Function));
        }
    }
    for name in BUILTINS {
        items.push(item(name, CompletionKind::Function));
    }
    for name in TYPES {
        items.push(item(name, CompletionKind::Type));
    }
    for name in KEYWORDS {
        items.push(item(name, CompletionKind::Keyword));
    }
    items.dedup_by(|a, b| a.label == b.label && a.kind == b.kind);
    items
}

fn item(label: &str, kind: CompletionKind) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind,
    }
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
    if receiver == "pos" {
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
    names.insert(0, "pos".to_string());

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
