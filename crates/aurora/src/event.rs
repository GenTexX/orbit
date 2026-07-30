//! aurora events: the semantic outcomes of input, for an app to react to.

use crate::widget::WidgetId;

/// Something a user did to a widget. Produced by
/// [`Ui::handle_input`](crate::Ui::handle_input) and read once per frame via
/// [`Ui::drain_events`](crate::Ui::drain_events).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// A button was clicked: pressed and released over the same button.
    Clicked(WidgetId),
    /// A checkbox toggled to a new state. The widget already holds `checked`;
    /// it is repeated here so the app can react without querying.
    Toggled { id: WidgetId, checked: bool },
    /// The focused text input's edit was submitted (`Key::Enter`). Read the
    /// current text via `Ui::kind(id)`; the field itself is unchanged, so a
    /// rejected submission just leaves the user's typing in place.
    Submitted(WidgetId),
    /// A slider was dragged to a new value (already stored in the widget).
    SliderChanged { id: WidgetId, value: f32 },
    /// A drag from a draggable widget crossed the start threshold and is now
    /// live. `source` is the draggable widget the drag began on.
    DragStarted { source: WidgetId },
    /// A live drag was released. `target` is the drop-target widget under the
    /// pointer, or `None` if it was released over none (a cancel). The app maps
    /// `source`/`target` back to its own data to complete the drop.
    /// A click in a text area's line-number gutter, on the (0-based) line it
    /// landed on. What that means - a breakpoint, folding, selecting the line -
    /// is the app's call; aurora only reports where.
    GutterClicked { id: WidgetId, line: usize },
    Dropped {
        source: WidgetId,
        target: Option<WidgetId>,
    },
}
