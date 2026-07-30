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
mod diagnostic;
mod lexer;
mod parser;
mod span;
mod tir;

pub use ast::{
    AssignOp, BinaryOp, Block, Else, Expr, Function, IfStmt, Param, Script, StateDecl, Stmt,
    TypeName, UnaryOp,
};
pub use check::check;
pub use diagnostic::{Diagnostic, Severity};
pub use lexer::{Token, TokenKind, lex};
pub use parser::parse;
pub use span::Span;
pub use tir::{
    Axis, Host, Place, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFn, TypedScript,
    TypedState, TypedStmt,
};
