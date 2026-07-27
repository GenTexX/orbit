//! The authored theme document and the registry of themeable tokens.
//!
//! A [`ThemeDoc`] holds `variables` (a named palette) and `tokens` (one per
//! themeable slot), each a [`Bind`] onto a variable or a literal. atlas resolves
//! a document into its concrete `EditorTheme`; prism edits one. [`TOKENS`] lists
//! every token with the kind of value it holds, the group it belongs to, and a
//! one-line description - the metadata a theming tool enumerates.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A value a theme holds: an RGBA color (each channel `0..=1`), or a scalar (a
/// pixel radius/width, a fade factor). Colors are plain floats; a consumer
/// converts to its own color type.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Color(f32, f32, f32, f32),
    Scalar(f32),
}

impl Value {
    /// This value's four color channels, or `None` if it is a scalar.
    pub fn channels(self) -> Option<[f32; 4]> {
        match self {
            Value::Color(r, g, b, a) => Some([r, g, b, a]),
            Value::Scalar(_) => None,
        }
    }

    /// This value as a scalar, or `None` if it is a color.
    pub fn scalar(self) -> Option<f32> {
        match self {
            Value::Scalar(s) => Some(s),
            Value::Color(..) => None,
        }
    }

    /// Whether this holds a color (vs a scalar).
    pub fn is_color(self) -> bool {
        matches!(self, Value::Color(..))
    }
}

/// How a token gets its value: a literal, or a reference to a named variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Bind {
    Lit(Value),
    Var(String),
}

/// An authored theme: a palette of `variables` and the `tokens` the editor reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeDoc {
    pub variables: BTreeMap<String, Value>,
    pub tokens: BTreeMap<String, Bind>,
}

impl ThemeDoc {
    /// The concrete value a token resolves to (following a [`Bind::Var`] to its
    /// variable), or `None` if the token is absent or its variable is unknown.
    pub fn resolved(&self, token: &str) -> Option<Value> {
        match self.tokens.get(token)? {
            Bind::Lit(v) => Some(*v),
            Bind::Var(name) => self.variables.get(name).copied(),
        }
    }
}

/// Whether a token holds a color or a scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Color,
    Scalar,
}

/// A token's metadata: what it is and what it affects.
pub struct Token {
    pub name: &'static str,
    pub kind: Kind,
    pub group: &'static str,
    pub doc: &'static str,
}

use Kind::{Color as C, Scalar as S};

/// The token groups, in display order.
pub const GROUPS: &[&str] = &[
    "Surfaces", "Text", "Accents", "Borders", "Inputs", "Widgets", "Shape", "Viewport",
];

/// Every themeable token, with its kind, group, and description. This is the
/// canonical set; atlas maps each to an `EditorTheme` field (and a test keeps
/// the two in sync).
pub const TOKENS: &[Token] = &[
    // Surfaces.
    t(
        "root_bg",
        C,
        "Surfaces",
        "Window background behind the panels",
    ),
    t(
        "panel_bg",
        C,
        "Surfaces",
        "Docked-panel background (and the active tab)",
    ),
    t("bar_bg", C, "Surfaces", "The tab strip's background"),
    t(
        "header_bg",
        C,
        "Surfaces",
        "An inspector section's title-bar background",
    ),
    t("card_bg", C, "Surfaces", "Inspector card fill"),
    t("menu_bg", C, "Surfaces", "Menu and popup background"),
    t("scrim", C, "Surfaces", "The dim behind a modal dialog"),
    // Text.
    t("heading", C, "Text", "Primary text color"),
    t("subhead", C, "Text", "Secondary / dimmed text color"),
    t("console_warn", C, "Text", "Console WARN-level line color"),
    // Accents.
    t(
        "accent",
        C,
        "Accents",
        "Focus accent: focused-field border, slider fill, text selection",
    ),
    t("row_selected", C, "Accents", "Selected tree-row background"),
    t("row_drop", C, "Accents", "Reparent drop-target highlight"),
    t(
        "mode_active",
        C,
        "Accents",
        "Active gizmo-mode / toggled toolbar button",
    ),
    t(
        "axis_x",
        C,
        "Accents",
        "X axis color (red) - gizmo and inspector",
    ),
    t(
        "axis_y",
        C,
        "Accents",
        "Y axis color (green) - gizmo and inspector",
    ),
    // Borders.
    t(
        "card_border",
        C,
        "Borders",
        "Inspector card outline (and section header divider)",
    ),
    t(
        "panel_border",
        C,
        "Borders",
        "Splitters between panels, and bar underlines",
    ),
    // Inputs.
    t("field_bg", C, "Inputs", "Text-input fill"),
    t(
        "field_border",
        C,
        "Inputs",
        "Text-input resting outline (the search box)",
    ),
    t("caret", C, "Inputs", "Text caret color"),
    t(
        "placeholder",
        C,
        "Inputs",
        "Placeholder text in an empty field",
    ),
    t("selection", C, "Inputs", "Text-selection highlight"),
    // Widgets.
    t("widget_bg", C, "Widgets", "Default button fill"),
    t("row_hover", C, "Widgets", "Hover overlay on list/tree rows"),
    t(
        "row_pressed",
        C,
        "Widgets",
        "Press overlay on list/tree rows",
    ),
    t("checkbox_box", C, "Widgets", "Checkbox box fill"),
    t("checkbox_mark", C, "Widgets", "Checkbox checkmark"),
    t("icon_hover", C, "Widgets", "Icon-button icon on hover"),
    t("slider_track", C, "Widgets", "Slider track (unfilled)"),
    t(
        "slider_fill",
        C,
        "Widgets",
        "Slider filled portion and thumb",
    ),
    t(
        "splitter",
        C,
        "Widgets",
        "Aurora's default splitter-bar fill",
    ),
    t("scrollbar_thumb", C, "Widgets", "Scrollbar thumb"),
    t(
        "disabled_fade",
        S,
        "Widgets",
        "How far a disabled widget fades toward the background",
    ),
    // Shape (scalars).
    t(
        "card_radius",
        S,
        "Shape",
        "Corner radius of cards, the color picker, and modals",
    ),
    t(
        "control_radius",
        S,
        "Shape",
        "Corner radius of buttons and fields",
    ),
    t(
        "inset_radius",
        S,
        "Shape",
        "Corner radius of an inline rename field",
    ),
    t(
        "splitter_width",
        S,
        "Shape",
        "Thickness of the draggable panel dividers",
    ),
    t("border_width", S, "Shape", "Thickness of drawn UI borders"),
    // Viewport (the scene view).
    t("viewport_bg", C, "Viewport", "The scene's clear color"),
    t("grid_line", C, "Viewport", "Minor grid line"),
    t("grid_line_strong", C, "Viewport", "Major grid line"),
    t(
        "selection_outline",
        C,
        "Viewport",
        "Selected sprite's outline",
    ),
    t("gizmo_rotate", C, "Viewport", "Rotate gizmo handle"),
    t("gizmo_scale", C, "Viewport", "Scale gizmo handle"),
];

/// A `const`-friendly [`Token`] constructor.
const fn t(name: &'static str, kind: Kind, group: &'static str, doc: &'static str) -> Token {
    Token {
        name,
        kind,
        group,
        doc,
    }
}

/// The metadata for a token, if known.
pub fn spec(name: &str) -> Option<&'static Token> {
    TOKENS.iter().find(|t| t.name == name)
}

/// A token's kind (defaulting to color for an unknown token).
pub fn kind_of(name: &str) -> Kind {
    spec(name).map(|t| t.kind).unwrap_or(Kind::Color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variable_reference_and_a_literal_both_resolve() {
        let mut doc = ThemeDoc::default();
        doc.variables
            .insert("accent".into(), Value::Color(0.3, 0.55, 0.9, 1.0));
        doc.tokens
            .insert("panel_bg".into(), Bind::Var("accent".into()));
        doc.tokens
            .insert("root_bg".into(), Bind::Lit(Value::Scalar(4.0)));
        assert_eq!(
            doc.resolved("panel_bg"),
            Some(Value::Color(0.3, 0.55, 0.9, 1.0))
        );
        assert_eq!(doc.resolved("root_bg"), Some(Value::Scalar(4.0)));
        assert_eq!(doc.resolved("missing"), None);
    }

    #[test]
    fn every_token_is_in_a_declared_group() {
        for token in TOKENS {
            assert!(
                GROUPS.contains(&token.group),
                "{} has unknown group {}",
                token.name,
                token.group
            );
        }
    }
}
