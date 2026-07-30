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
pub mod service;
mod span;
mod tir;

/// Compile a script to a WebAssembly module.
///
/// The whole pipeline: lex, parse, check, emit. A script with any error
/// diagnostic produces no module - it is the one place in comet that refuses to
/// carry on, because emitting code for a program known to be wrong is the only
/// outcome worse than reporting it. Everything upstream stays error-tolerant, so
/// the returned diagnostics cover the whole file rather than stopping at the
/// first mistake.
pub fn compile(source: &str) -> Result<Vec<u8>, Vec<Diagnostic>> {
    let (script, mut diagnostics) = parse(source);
    let (typed, check_diagnostics) = check(&script);
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
pub use codegen::{HOST_MODULE, emit};
pub use diagnostic::{Diagnostic, Severity};
pub use lexer::{Token, TokenKind, lex, lex_with_comments};
pub use parser::parse;
pub use span::Span;
pub use tir::{
    Axis, Host, Place, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFn, TypedScript,
    TypedState, TypedStmt,
};
