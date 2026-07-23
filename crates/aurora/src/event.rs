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
}
