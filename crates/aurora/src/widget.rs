//! aurora widget: one node type for the whole UI tree, tagged by kind (ADR 0014).

use slotmap::new_key_type;
use taffy::NodeId;

use crate::color::Color;

new_key_type! {
    /// A typed handle to a widget in the [`Ui`](crate::Ui) arena. Cheap to copy;
    /// stays valid until the widget is removed.
    pub struct WidgetId;
}

/// What a widget is. Every widget shares one storage type; the kind carries the
/// per-widget data (ADR 0014). Adding a widget kind means adding a variant here.
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetKind {
    /// A container with no visuals of its own; groups and lays out children.
    Panel,
    /// Static, non-interactive text.
    Label(String),
    /// A clickable button with a text caption.
    Button(String),
    /// A boolean toggle.
    Checkbox(bool),
    /// A single-line editable text field, holding its current text.
    TextInput(String),
}

impl WidgetKind {
    /// Whether this kind responds to pointer input (so hit-testing bubbles a
    /// click up to it): buttons and checkboxes activate, text inputs take focus.
    pub(crate) fn is_interactive(&self) -> bool {
        matches!(
            self,
            WidgetKind::Button(_) | WidgetKind::Checkbox(_) | WidgetKind::TextInput(_)
        )
    }
}

/// A node in the UI tree: its kind, its taffy layout node, and its tree links.
/// Internal: the outside world holds [`WidgetId`] handles, not `Widget` values.
#[derive(Debug)]
pub(crate) struct Widget {
    pub kind: WidgetKind,
    pub taffy: NodeId,
    pub parent: Option<WidgetId>,
    pub children: Vec<WidgetId>,
    /// Background fill (visual style, not layout). Transparent draws nothing.
    pub background: Color,
    /// Text color, for widgets that draw text.
    pub foreground: Color,
    /// Whether children are clipped to this widget's rectangle.
    pub clip: bool,
}
