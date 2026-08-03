//! comet - the Comet scripting language: error-tolerant frontend (lexer, parser, type checker), WASM emission, language service.
//!
//! Milestone 4 brings the frontend online. The pipeline is deliberately plain -
//! lex, parse, check, emit, with no optimizing middle-end (ADR 0007): compile
//! speed comes from skipping optimization entirely and letting wasmtime's
//! Cranelift JIT optimize once at module load.
//!
//! Every stage is **error-tolerant** rather than fail-fast (ADR 0010). The editor
//! re-parses on every keystroke, so source is malformed most of the time it is
//! seen; [`parse`] therefore always returns a script plus a list of
//! [`Diagnostic`]s, never a `Result`. A typo on one line leaves the rest of the
//! file parsed and checkable, which is what makes live squiggles possible.

mod ast;
mod check;
mod codegen;
mod diagnostic;
mod lexer;
mod parser;
pub mod schema;
pub mod service;
mod span;
mod tir;

/// Compile a script to a WebAssembly module.
///
/// The whole pipeline: lex, parse, check, emit. A script with any *error*
/// diagnostic produces no module - warnings do not stop it - it is the one place in comet that refuses to
/// carry on, because emitting code for a program known to be wrong is the only
/// outcome worse than reporting it. Everything upstream stays error-tolerant, so
/// the returned diagnostics cover the whole file rather than stopping at the
/// first mistake.
pub fn compile(source: &str, schema: &HostSchema) -> Result<Vec<u8>, Vec<Diagnostic>> {
    let (script, mut diagnostics) = parse(source);
    let (typed, check_diagnostics) = check(&script, schema);
    diagnostics.extend(check_diagnostics);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
        return Err(diagnostics);
    }
    Ok(codegen::emit(&typed))
}

pub use ast::{
    AssignOp, BinaryOp, Block, Else, Expr, Function, IfStmt, Param, Script, StateDecl, Stmt,
    TypeName, UnaryOp,
};
pub use check::check;
pub use codegen::{HOST_MODULE, MAX_PAGES, emit, exported_globals, format_f32, write_str};

/// What an exported field starts at when it is first created, or reverted to.
///
/// ADR 0022 says the script's own initializer is "the default used when the
/// field is first created or explicitly reverted", the doc comment on the
/// component says the same, and the compiler warning a person actually reads
/// tells them their initializer is only a default - so it has to *be* one.
/// Before this it was thrown away and every field started at its type's zero,
/// which made three sentences untrue at once.
///
/// Only a literal is carried. An initializer that calls a function or reads
/// another variable has no value until the module runs, and guessing one would
/// be worse than the type default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExportDefault {
    F32(f32),
    Int(i32),
    Bool(bool),
    Vec2(f32, f32),
}

/// One `@export`ed variable: what the inspector shows, and which globals hold
/// it in a compiled module.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub name: String,
    pub ty: Type,
    /// What the field starts at. `None` when the initializer is not a literal;
    /// the caller falls back to the type's own default.
    pub default: Option<ExportDefault>,
    /// How the inspector should present it. The language validates these and
    /// the editor decides what they look like, so adding one is a row in the
    /// checker and a widget in atlas with nothing in between.
    pub hints: Vec<Hint>,
    /// The module's export names for the globals behind it - one for a scalar,
    /// two for a `Vec2`.
    pub globals: Vec<String>,
}

/// The variables `source` marks `@export`, in declaration order.
///
/// The engine needs this to know what to store on the component and what to
/// write back into a running module; the editor needs the same list to draw the
/// fields. Both ask rather than each working it out, so there is one answer.
/// A source that does not compile still yields what it managed to declare -
/// the inspector should not empty itself while a line is half-typed.
pub fn exports(source: &str, schema: &HostSchema) -> Vec<Exported> {
    let (script, _) = parse(source);
    let (typed, _) = check(&script, schema);
    typed
        .state
        .iter()
        .filter(|state| state.exported && !state.ty.is_error())
        .map(|state| Exported {
            name: state.name.clone(),
            ty: state.ty,
            default: const_value(&state.init),
            hints: state.hints.clone(),
            globals: codegen::exported_globals(&state.name, state.ty),
        })
        .collect()
}

/// The value of `expr`, if it has one without running anything.
///
/// The checker fills a missing initializer in with the type's default, so this
/// answers for both cases: a declaration with no initializer yields the zero it
/// was given, and one with a literal yields what was written.
fn const_value(expr: &TypedExpr) -> Option<ExportDefault> {
    use TypedExprKind as K;
    match &expr.kind {
        K::Number(v) => Some(ExportDefault::F32(*v)),
        K::Int(v) => Some(ExportDefault::Int(*v)),
        K::Bool(v) => Some(ExportDefault::Bool(*v)),
        // `let speed: f32 = 5;` - the checker widened a literal, and the value
        // is still known.
        K::Widen(inner) => match const_value(inner)? {
            ExportDefault::Int(v) => Some(ExportDefault::F32(v as f32)),
            other => Some(other),
        },
        K::MakeVec2 { x, y } => match (const_value(x)?, const_value(y)?) {
            (ExportDefault::F32(x), ExportDefault::F32(y)) => Some(ExportDefault::Vec2(x, y)),
            _ => None,
        },
        K::Unary {
            op: tir::UnaryOp::Neg,
            operand,
        } => match const_value(operand)? {
            ExportDefault::F32(v) => Some(ExportDefault::F32(-v)),
            ExportDefault::Int(v) => Some(ExportDefault::Int(-v)),
            other => Some(other),
        },
        _ => None,
    }
}
pub use diagnostic::{Diagnostic, Severity};
pub use lexer::{Token, TokenKind, lex, lex_with_comments};
pub use parser::parse;
pub use schema::{FieldRef, HostField, HostObject, HostSchema, HostType, example_schema};
pub use span::Span;
pub use tir::{
    Axis, Hint, Host, Place, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFn, TypedScript,
    TypedState, TypedStmt,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exported_variable_carries_the_value_it_was_written_with() {
        // ADR 0022 calls the initializer "the default used when the field is
        // first created", the component's doc comment says it, and the warning
        // a person reads when they write one says it. It was thrown away, so
        // every field started at its type's zero and the first thing anyone did
        // with a tunable script produced a node that did not move.
        let exported = exports(
            "@export let speed: f32 = 120.0;\n\
             @export let steps: int = 3;\n\
             @export let chases: bool = true;\n\
             @export let home: Vec2 = vec2(4.0, -5.0);\n\
             @export let widened: f32 = 7;\n\
             @export let bare: f32;\n\
             func update(dt: f32) { transform.position.x += speed * dt; }",
            &example_schema(),
        );
        let by_name = |name: &str| {
            exported
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("{name} is exported"))
                .default
        };
        assert_eq!(by_name("speed"), Some(ExportDefault::F32(120.0)));
        assert_eq!(by_name("steps"), Some(ExportDefault::Int(3)));
        assert_eq!(by_name("chases"), Some(ExportDefault::Bool(true)));
        assert_eq!(by_name("home"), Some(ExportDefault::Vec2(4.0, -5.0)));
        assert_eq!(by_name("widened"), Some(ExportDefault::F32(7.0)));
        // No initializer: the checker filled in the type's zero, and that is
        // the answer - the same one as before, by the same route as the rest.
        assert_eq!(by_name("bare"), Some(ExportDefault::F32(0.0)));
    }

    #[test]
    fn an_initializer_that_is_not_a_literal_has_no_default() {
        // It has no value until the module runs, and guessing one would be
        // worse than the type's zero the caller falls back to.
        let exported = exports(
            "let base = 2.0;\n@export let speed: f32 = base * 3.0;\n\
             func update(dt: f32) { transform.position.x += speed * dt; }",
            &example_schema(),
        );
        assert_eq!(exported[0].default, None);
    }
}
