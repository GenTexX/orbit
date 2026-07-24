//! aurora widget: one node type for the whole UI tree, tagged by kind (ADR 0014).

use slotmap::new_key_type;
use taffy::NodeId;

use crate::color::Color;
use crate::draw::ImageHandle;

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
    /// A registered texture (e.g. the engine's viewport), drawn to fill this
    /// widget's rectangle. Backend-resolved, not interactive (ADR 0018).
    Image(ImageHandle),
    /// A horizontal slider holding its current value within `[min, max]`;
    /// dragging the track sets the value (emits `Event::SliderChanged`).
    Slider { value: f32, min: f32, max: f32 },
    /// A draggable divider that resizes `target` along `orientation`'s axis -
    /// the foundation resizable, then fully dockable, panels grow from.
    /// `target` starts unset (see [`Ui::splitter`](crate::Ui::splitter)'s doc)
    /// since a splitter is often placed *before* the sibling it resizes, which
    /// does not exist yet at construction time.
    Splitter {
        orientation: Orientation,
        target: Option<WidgetId>,
        /// +1.0 if dragging in the positive axis direction (right, or down)
        /// should grow `target` - true when `target` sits before this
        /// splitter in layout order (e.g. a left-docked panel with the
        /// splitter on its right edge). -1.0 for the mirror case (e.g. a
        /// right-docked panel with the splitter on its left edge).
        sign: f32,
    },
}

/// An input mask: which text a [`WidgetKind::TextInput`] will accept. A
/// keystroke or paste that would make the field's whole text stop matching the
/// mask is rejected, leaving the field unchanged (validation as you type).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mask {
    /// Accept any text (the default).
    #[default]
    None,
    /// A signed integer: an optional leading `-`, then ASCII digits.
    Integer,
    /// A signed decimal: an optional leading `-`, ASCII digits, and at most one
    /// `.` (the primitive the inspector's numeric fields build on).
    Decimal,
    /// ASCII digits only - unsigned, no sign or separator (counts, ports).
    Digits,
    /// ASCII hexadecimal digits only (`0-9`, `a-f`, `A-F`) - e.g. hex colors.
    Hex,
}

impl Mask {
    /// Whether `candidate` (the field's text after a proposed edit) is
    /// acceptable. An empty string and a lone `-` (for the signed masks) are
    /// always accepted so a field can be cleared and a number typed sign-first.
    pub fn accepts(self, candidate: &str) -> bool {
        match self {
            Mask::None => true,
            Mask::Integer => {
                let body = candidate.strip_prefix('-').unwrap_or(candidate);
                body.chars().all(|c| c.is_ascii_digit())
            }
            Mask::Decimal => {
                let body = candidate.strip_prefix('-').unwrap_or(candidate);
                let mut dots = 0;
                body.chars().all(|c| match c {
                    '.' => {
                        dots += 1;
                        dots <= 1
                    }
                    _ => c.is_ascii_digit(),
                })
            }
            Mask::Digits => candidate.chars().all(|c| c.is_ascii_digit()),
            Mask::Hex => candidate.chars().all(|c| c.is_ascii_hexdigit()),
        }
    }
}

/// Which axis a [`WidgetKind::Splitter`] drags along, and so which dimension of
/// its target it resizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// A vertical bar; dragging it horizontally resizes the target's width.
    Vertical,
    /// A horizontal bar; dragging it vertically resizes the target's height.
    Horizontal,
}

impl WidgetKind {
    /// Whether this kind responds to pointer input (so hit-testing bubbles a
    /// click up to it): buttons and checkboxes activate, text inputs take
    /// focus, splitters drag.
    pub(crate) fn is_interactive(&self) -> bool {
        matches!(
            self,
            WidgetKind::Button(_)
                | WidgetKind::Checkbox(_)
                | WidgetKind::TextInput(_)
                | WidgetKind::Slider { .. }
                | WidgetKind::Splitter { .. }
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
    /// Whether this widget scrolls its overflowing content vertically.
    pub scroll: bool,
    /// The input mask this text input enforces as you type (`Mask::None` for a
    /// plain field). Other kinds ignore it.
    pub mask: Mask,
    /// Placeholder text shown, dimmed, while a text input is empty (empty string
    /// for none). Other kinds ignore it.
    pub placeholder: String,
    /// Font size in pixels for this widget's text (the UI default when unset via
    /// style). Drives both shaping and measurement.
    pub font_size: f32,
    /// Whether this text input is multi-line (a text area): Enter inserts a
    /// newline instead of submitting, and the caret moves by line. Other kinds
    /// ignore it.
    pub multiline: bool,
    /// Whether this widget is a drag source (see [`crate::Style::draggable`]).
    pub draggable: bool,
    /// Whether this widget accepts drops (see [`crate::Style::drop_target`]).
    pub drop_target: bool,
    /// Whether this button renders flat (no raised fill; hover/press only).
    pub flat: bool,
    /// Whether hit-testing ignores this widget (decorative overlays).
    pub hit_transparent: bool,
    /// Whether this widget is disabled: drawn dimmed and inert to all input.
    pub disabled: bool,
}
