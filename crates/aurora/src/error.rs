//! aurora error type.

/// Errors from laying out a [`Ui`](crate::Ui).
#[derive(Debug, thiserror::Error)]
pub enum AuroraError {
    /// The layout engine (taffy) reported an error, e.g. an invalid node.
    #[error("layout failed")]
    Layout(#[from] taffy::TaffyError),
}
