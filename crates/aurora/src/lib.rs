//! aurora - retained GUI framework: widget arena, taffy layout, bubbling events, draw-list output. Engine-independent.

mod error;
mod rect;
mod ui;
mod widget;

pub use error::AuroraError;
pub use rect::Rect;
pub use ui::Ui;
pub use widget::{WidgetId, WidgetKind};

/// The layout engine. Re-exported so callers style widgets without a direct
/// taffy dependency, e.g. `aurora::taffy::Style`.
pub use taffy;

pub mod prelude {
    //! Convenient imports for building a UI: Aurora's core types plus taffy's
    //! style vocabulary (`Style`, `Size`, `length`, `Display`, and friends).
    pub use crate::{Rect, Ui, WidgetId, WidgetKind};
    pub use taffy::prelude::*;
}
