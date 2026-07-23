//! aurora - retained GUI framework: widget arena, taffy layout, bubbling events, draw-list output. Engine-independent.

mod color;
mod draw;
mod error;
mod event;
mod input;
mod rect;
mod style;
mod text;
mod theme;
mod ui;
mod widget;

pub use color::Color;
pub use draw::{DrawCommand, DrawList, Glyph};
pub use error::AuroraError;
pub use event::Event;
pub use input::InputEvent;
pub use rect::Rect;
pub use style::Style;
pub use ui::Ui;
pub use widget::{WidgetId, WidgetKind};

/// The layout engine. Re-exported so callers can reach taffy types without a
/// direct dependency, e.g. `aurora::taffy::AlignItems`.
pub use taffy;

/// The text engine. Re-exported so a rendering backend shares Aurora's exact
/// cosmic-text version (the draw list carries its `CacheKey`).
pub use cosmic_text;

pub mod prelude {
    //! Convenient imports for building a UI: Aurora's core types plus taffy's
    //! style vocabulary.
    pub use crate::{
        Color, DrawCommand, DrawList, Event, InputEvent, Rect, Style, Ui, WidgetId, WidgetKind,
    };
    pub use taffy::prelude::*;
}
