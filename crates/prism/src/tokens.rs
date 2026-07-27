//! Metadata for the theme tokens prism edits: each token's value kind (so a
//! scalar field never turns into a color field), the group it belongs to (for
//! structure), and a one-line description (shown on hover).
//!
//! This mirrors atlas's token set. Like `model.rs` and `color.rs`, it is a
//! deliberate copy pending a shared crate both atlas and prism depend on.

/// Whether a token holds a color or a scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Color,
    Scalar,
}

/// A token's metadata.
pub struct Spec {
    pub name: &'static str,
    pub kind: Kind,
    pub group: &'static str,
    pub doc: &'static str,
}

use Kind::{Color as C, Scalar as S};

/// The groups, in display order.
pub const GROUPS: &[&str] = &[
    "Surfaces", "Text", "Accents", "Borders", "Inputs", "Widgets", "Shape", "Viewport",
];

/// Every known token with its kind, group, and description.
pub const TOKENS: &[Spec] = &[
    // Surfaces.
    s(
        "root_bg",
        C,
        "Surfaces",
        "Window background behind the panels",
    ),
    s(
        "panel_bg",
        C,
        "Surfaces",
        "Docked-panel background (and the active tab)",
    ),
    s("bar_bg", C, "Surfaces", "The tab strip's background"),
    s(
        "header_bg",
        C,
        "Surfaces",
        "An inspector section's title-bar background",
    ),
    s("card_bg", C, "Surfaces", "Inspector card fill"),
    s("menu_bg", C, "Surfaces", "Menu and popup background"),
    s("scrim", C, "Surfaces", "The dim behind a modal dialog"),
    // Text.
    s("heading", C, "Text", "Primary text color"),
    s("subhead", C, "Text", "Secondary / dimmed text color"),
    s("console_warn", C, "Text", "Console WARN-level line color"),
    // Accents.
    s(
        "accent",
        C,
        "Accents",
        "Focus accent: focused-field border, slider fill, text selection",
    ),
    s("row_selected", C, "Accents", "Selected tree-row background"),
    s("row_drop", C, "Accents", "Reparent drop-target highlight"),
    s(
        "mode_active",
        C,
        "Accents",
        "Active gizmo-mode / toggled toolbar button",
    ),
    s(
        "axis_x",
        C,
        "Accents",
        "X axis color (red) - gizmo and inspector",
    ),
    s(
        "axis_y",
        C,
        "Accents",
        "Y axis color (green) - gizmo and inspector",
    ),
    // Borders.
    s(
        "card_border",
        C,
        "Borders",
        "Inspector card outline (and section header divider)",
    ),
    s(
        "panel_border",
        C,
        "Borders",
        "Splitters between panels, and bar underlines",
    ),
    // Inputs.
    s("field_bg", C, "Inputs", "Text-input fill"),
    s(
        "field_border",
        C,
        "Inputs",
        "Text-input resting outline (the search box)",
    ),
    s("caret", C, "Inputs", "Text caret color"),
    s(
        "placeholder",
        C,
        "Inputs",
        "Placeholder text in an empty field",
    ),
    s("selection", C, "Inputs", "Text-selection highlight"),
    // Widgets.
    s("widget_bg", C, "Widgets", "Default button fill"),
    s("row_hover", C, "Widgets", "Hover overlay on list/tree rows"),
    s(
        "row_pressed",
        C,
        "Widgets",
        "Press overlay on list/tree rows",
    ),
    s("checkbox_box", C, "Widgets", "Checkbox box fill"),
    s("checkbox_mark", C, "Widgets", "Checkbox checkmark"),
    s("icon_hover", C, "Widgets", "Icon-button icon on hover"),
    s("slider_track", C, "Widgets", "Slider track (unfilled)"),
    s(
        "slider_fill",
        C,
        "Widgets",
        "Slider filled portion and thumb",
    ),
    s(
        "splitter",
        C,
        "Widgets",
        "Aurora's default splitter-bar fill",
    ),
    s("scrollbar_thumb", C, "Widgets", "Scrollbar thumb"),
    s(
        "disabled_fade",
        S,
        "Widgets",
        "How far a disabled widget fades toward the background",
    ),
    // Shape (scalars).
    s(
        "card_radius",
        S,
        "Shape",
        "Corner radius of cards, the color picker, and modals",
    ),
    s(
        "control_radius",
        S,
        "Shape",
        "Corner radius of buttons and fields",
    ),
    s(
        "inset_radius",
        S,
        "Shape",
        "Corner radius of an inline rename field",
    ),
    s(
        "splitter_width",
        S,
        "Shape",
        "Thickness of the draggable panel dividers",
    ),
    s("border_width", S, "Shape", "Thickness of drawn UI borders"),
    // Viewport (the scene view).
    s("viewport_bg", C, "Viewport", "The scene's clear color"),
    s("grid_line", C, "Viewport", "Minor grid line"),
    s("grid_line_strong", C, "Viewport", "Major grid line"),
    s(
        "selection_outline",
        C,
        "Viewport",
        "Selected sprite's outline",
    ),
    s("gizmo_rotate", C, "Viewport", "Rotate gizmo handle"),
    s("gizmo_scale", C, "Viewport", "Scale gizmo handle"),
];

/// A `const`-friendly [`Spec`] constructor.
const fn s(name: &'static str, kind: Kind, group: &'static str, doc: &'static str) -> Spec {
    Spec {
        name,
        kind,
        group,
        doc,
    }
}

/// The spec for a token, if known.
pub fn spec(name: &str) -> Option<&'static Spec> {
    TOKENS.iter().find(|s| s.name == name)
}

/// A token's kind (defaulting to color for an unknown token).
pub fn kind_of(name: &str) -> Kind {
    spec(name).map(|s| s.kind).unwrap_or(Kind::Color)
}
