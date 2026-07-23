//! aurora - retained GUI framework: widget arena, taffy layout, bubbling events, draw-list output. Engine-independent.

mod color;
mod draw;
mod error;
mod rect;
mod style;
mod ui;
mod widget;

pub use color::Color;
pub use draw::{DrawCommand, DrawList};
pub use error::AuroraError;
pub use rect::Rect;
pub use style::Style;
pub use ui::Ui;
pub use widget::{WidgetId, WidgetKind};

/// The layout engine. Re-exported so callers can reach taffy types without a
/// direct dependency, e.g. `aurora::taffy::AlignItems`.
pub use taffy;

pub mod prelude {
    //! Convenient imports for building a UI: Aurora's core types plus taffy's
    //! style vocabulary.
    pub use crate::{Color, DrawCommand, DrawList, Rect, Style, Ui, WidgetId, WidgetKind};
    pub use taffy::prelude::*;
}
