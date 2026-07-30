//! comet diagnostics: a span, a severity, and a message - what parser error
//! recovery, the type checker, and (later) the language service all produce.
//! Diagnostics are data, not failures: nothing in comet returns `Result` over a
//! bad script (ADR 0010) - a script that does not compile still parses as far as
//! it can and reports what is wrong, so the editor always has something to show.

use crate::span::Span;

/// How serious a diagnostic is. Only `Error` is emitted anywhere in v1, but the
/// distinction is real from day one: an editor squiggle needs to know which
/// color to draw, and `Warning` will not be a new concept when something first
/// needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One thing wrong with a script, at a specific span, in a human-readable sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Error,
            message: message.into(),
        }
    }
}
