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
pub use codegen::{HOST_MODULE, emit, exported_globals, format_f32, write_str};

/// One `@export`ed variable: what the inspector shows, and which globals hold
/// it in a compiled module.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub name: String,
    pub ty: Type,
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
            hints: state.hints.clone(),
            globals: codegen::exported_globals(&state.name, state.ty),
        })
        .collect()
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
