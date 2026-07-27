//! spectrum: the atlas editor's theme and settings model, shared by atlas and
//! prism.
//!
//! - [`theme`] is the authored theme document ([`ThemeDoc`](theme::ThemeDoc): a
//!   palette of variables and the tokens that reference them) plus the token
//!   registry ([`TOKENS`](theme::TOKENS)) describing every themeable slot. atlas
//!   resolves a document into its concrete colors; prism edits one.
//! - [`color`] is HSV/RGB/hex math and the gradient bitmaps a color picker draws.
//! - [`settings`] is the on-disk `settings.ron` (the theme plus atlas's view
//!   toggles), and reading/writing it.
//!
//! It has no GUI dependency, so a [`Value`](theme::Value) color is a plain
//! `(f32, f32, f32, f32)` the consumer converts to its own color type.

pub mod color;
pub mod settings;
pub mod theme;
