//! atlas theming: an authored, hand-editable theme document that resolves into
//! the concrete [`EditorTheme`] the UI draws with.
//!
//! A [`ThemeDoc`] holds two things:
//!
//! - **variables** - a named palette of reusable values (`"accent"`, `"surface"`,
//!   a `"radius"` scalar, ...).
//! - **tokens** - one entry per themeable slot the editor reads (`panel_bg`,
//!   `header_bg`, `control_radius`, ...). Each token is a [`Bind`]: either a
//!   literal value or a [`Bind::Var`] reference to a variable, so a palette can
//!   be defined once and reused across many tokens.
//!
//! [`ThemeDoc::resolve`] turns the document into an [`EditorTheme`]. A token that
//! is missing (or references an unknown variable, or holds the wrong value kind)
//! falls back to the built-in dark default, so a partial or hand-broken theme
//! file still yields a usable theme rather than an error. The document is what
//! the settings file stores and what a future live-theming tool would edit; the
//! resolved [`EditorTheme`] is what every widget reads.

use std::collections::BTreeMap;

use aurora::Color;
use serde::{Deserialize, Serialize};

use crate::ui::EditorTheme;

/// A value a theme can hold: an RGBA color, or a scalar (a pixel radius, a fade
/// factor, ...).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ThemeValue {
    Color(f32, f32, f32, f32),
    Scalar(f32),
}

impl ThemeValue {
    /// This value as a color, or `None` if it is a scalar.
    fn as_color(self) -> Option<Color> {
        match self {
            ThemeValue::Color(r, g, b, a) => Some(Color::rgba(r, g, b, a)),
            ThemeValue::Scalar(_) => None,
        }
    }

    /// This value as a scalar, or `None` if it is a color.
    fn as_scalar(self) -> Option<f32> {
        match self {
            ThemeValue::Scalar(s) => Some(s),
            ThemeValue::Color(..) => None,
        }
    }
}

impl From<Color> for ThemeValue {
    fn from(c: Color) -> Self {
        ThemeValue::Color(c.r, c.g, c.b, c.a)
    }
}

/// How a token gets its value: a literal, or a reference to a named variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Bind {
    /// A literal value.
    Lit(ThemeValue),
    /// A reference to a `variables` entry by name.
    Var(String),
}

/// Whether a token holds a color or a scalar (for a tool enumerating the set).
// The registry and this kind are the enumeration API a live-theming tool will
// use; nothing in the editor itself reads them yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Color,
    Scalar,
}

/// An authored theme: a palette of `variables` plus the `tokens` the editor
/// reads, each of which references a variable or gives a literal. Resolve it
/// into the concrete [`EditorTheme`] with [`resolve`](Self::resolve).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeDoc {
    /// Named, reusable values a token can reference.
    pub variables: BTreeMap<String, ThemeValue>,
    /// One entry per themeable slot (see [`token_registry`] for the full set).
    pub tokens: BTreeMap<String, Bind>,
}

impl Default for ThemeDoc {
    fn default() -> Self {
        Self::dark()
    }
}

impl ThemeDoc {
    /// The concrete value a token resolves to - following a [`Bind::Var`] to its
    /// variable - or `None` if the token is absent or its variable is unknown.
    fn value(&self, token: &str) -> Option<ThemeValue> {
        match self.tokens.get(token)? {
            Bind::Lit(v) => Some(*v),
            Bind::Var(name) => self.variables.get(name).copied(),
        }
    }

    /// A token as a color, falling back to `default` when it is missing or not a
    /// color.
    fn color(&self, token: &str, default: Color) -> Color {
        self.value(token)
            .and_then(ThemeValue::as_color)
            .unwrap_or(default)
    }

    /// A token as a scalar, falling back to `default` when it is missing or not a
    /// scalar.
    fn scalar(&self, token: &str, default: f32) -> f32 {
        self.value(token)
            .and_then(ThemeValue::as_scalar)
            .unwrap_or(default)
    }
}

// One source of truth for the token <-> EditorTheme-field mapping: the macro
// below expands it into the token registry, a snapshot (theme -> token values),
// and resolve (doc -> theme). Adding a themeable slot is one line here.

macro_rules! snap_value {
    (Color, $e:expr) => {
        ThemeValue::from($e)
    };
    (Scalar, $e:expr) => {
        ThemeValue::Scalar($e)
    };
}

macro_rules! read_token {
    (Color, $doc:expr, $name:expr, $default:expr) => {
        $doc.color($name, $default)
    };
    (Scalar, $doc:expr, $name:expr, $default:expr) => {
        $doc.scalar($name, $default)
    };
}

macro_rules! theme_tokens {
    ( $( $name:literal : $kind:ident => $($field:ident).+ ),* $(,)? ) => {
        /// Every theme token, paired with the kind of value it holds - the full
        /// set a theme (or a theming tool) can set.
        #[allow(dead_code)]
        pub fn token_registry() -> &'static [(&'static str, ValueKind)] {
            &[ $( ($name, ValueKind::$kind) ),* ]
        }

        /// Snapshot a concrete theme into token values (the inverse of resolve,
        /// used to author a default document from the built-in dark theme).
        fn snapshot(d: &EditorTheme) -> BTreeMap<String, ThemeValue> {
            let mut m = BTreeMap::new();
            $( m.insert($name.to_string(), snap_value!($kind, d.$($field).+)); )*
            m
        }

        impl ThemeDoc {
            /// Resolve into the concrete runtime theme: every field starts from
            /// the built-in dark default and is overridden by its token when the
            /// document provides one.
            pub fn resolve(&self) -> EditorTheme {
                let mut t = EditorTheme::dark();
                $( t.$($field).+ = read_token!($kind, self, $name, t.$($field).+); )*
                t
            }
        }
    };
}

theme_tokens! {
    // aurora widget palette.
    "widget_bg": Color => aurora.button,
    "row_hover": Color => aurora.row_hover,
    "row_pressed": Color => aurora.row_pressed,
    "checkbox_box": Color => aurora.checkbox_box,
    "checkbox_mark": Color => aurora.checkbox_mark,
    "field_bg": Color => aurora.field,
    "accent": Color => aurora.focus,
    "caret": Color => aurora.caret,
    "placeholder": Color => aurora.placeholder,
    "icon_hover": Color => aurora.icon_hover,
    "selection": Color => aurora.selection,
    "slider_track": Color => aurora.slider_track,
    "slider_fill": Color => aurora.slider_fill,
    "splitter": Color => aurora.splitter,
    "scrollbar_thumb": Color => aurora.scrollbar_thumb,
    "disabled_fade": Scalar => aurora.disabled_fade,
    // atlas surfaces, text, and accents.
    "panel_bg": Color => panel_bg,
    "root_bg": Color => root_bg,
    "bar_bg": Color => bar_bg,
    "header_bg": Color => header_bg,
    "heading": Color => heading,
    "subhead": Color => subhead,
    "row_selected": Color => row_selected,
    "row_drop": Color => row_drop,
    "menu_bg": Color => menu_bg,
    "card_bg": Color => card_bg,
    "card_border": Color => card_border,
    "mode_active": Color => mode_active,
    "axis_x": Color => axis_x,
    "axis_y": Color => axis_y,
    "console_warn": Color => console_warn,
    "scrim": Color => scrim,
    // shape scalars.
    "card_radius": Scalar => card_radius,
    "control_radius": Scalar => control_radius,
    "inset_radius": Scalar => inset_radius,
}

impl ThemeDoc {
    /// The authored default dark theme: a small palette of variables, and every
    /// token bound to a variable when its value matches one (so the palette is
    /// shared) or to a literal otherwise. This is what is written to a fresh
    /// settings file - a complete, editable starting point.
    pub fn dark() -> Self {
        let d = EditorTheme::dark();
        // The palette. A token whose default equals one of these binds to it.
        let variables = BTreeMap::from([
            ("accent".to_string(), ThemeValue::from(d.aurora.focus)),
            ("surface".to_string(), ThemeValue::from(d.panel_bg)),
            ("surface_dim".to_string(), ThemeValue::from(d.root_bg)),
            ("bar".to_string(), ThemeValue::from(d.bar_bg)),
            ("header".to_string(), ThemeValue::from(d.header_bg)),
            ("text".to_string(), ThemeValue::from(d.heading)),
            ("text_dim".to_string(), ThemeValue::from(d.subhead)),
            ("radius".to_string(), ThemeValue::Scalar(d.control_radius)),
        ]);
        let tokens = snapshot(&d)
            .into_iter()
            .map(|(name, value)| {
                let bind = variables
                    .iter()
                    .find(|(_, v)| **v == value)
                    .map(|(var, _)| Bind::Var(var.clone()))
                    .unwrap_or(Bind::Lit(value));
                (name, bind)
            })
            .collect();
        Self { variables, tokens }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_document_resolves_to_the_built_in_dark_theme() {
        // The authored dark doc (palette + per-token binds) must resolve back to
        // exactly EditorTheme::dark() - this keeps resolve, snapshot, and the
        // palette in agreement.
        assert_eq!(ThemeDoc::dark().resolve(), EditorTheme::dark());
    }

    #[test]
    fn an_empty_document_resolves_to_all_defaults() {
        let empty = ThemeDoc {
            variables: BTreeMap::new(),
            tokens: BTreeMap::new(),
        };
        assert_eq!(empty.resolve(), EditorTheme::dark());
    }

    #[test]
    fn a_variable_reference_and_a_literal_both_resolve() {
        let doc = ThemeDoc {
            variables: BTreeMap::from([(
                "primary".to_string(),
                ThemeValue::Color(0.1, 0.2, 0.3, 1.0),
            )]),
            tokens: BTreeMap::from([
                ("panel_bg".to_string(), Bind::Var("primary".to_string())),
                (
                    "root_bg".to_string(),
                    Bind::Lit(ThemeValue::Color(0.9, 0.8, 0.7, 1.0)),
                ),
                (
                    "control_radius".to_string(),
                    Bind::Lit(ThemeValue::Scalar(9.0)),
                ),
            ]),
        };
        let theme = doc.resolve();
        assert_eq!(theme.panel_bg, Color::rgb(0.1, 0.2, 0.3), "var reference");
        assert_eq!(theme.root_bg, Color::rgb(0.9, 0.8, 0.7), "literal");
        assert_eq!(theme.control_radius, 9.0, "scalar literal");
        // A token the doc did not set keeps the dark default.
        assert_eq!(theme.bar_bg, EditorTheme::dark().bar_bg, "unset falls back");
    }

    #[test]
    fn a_dangling_variable_or_wrong_kind_falls_back_to_the_default() {
        let doc = ThemeDoc {
            variables: BTreeMap::new(),
            tokens: BTreeMap::from([
                // References a variable that does not exist.
                ("panel_bg".to_string(), Bind::Var("missing".to_string())),
                // A scalar where a color is expected.
                ("root_bg".to_string(), Bind::Lit(ThemeValue::Scalar(3.0))),
            ]),
        };
        let theme = doc.resolve();
        assert_eq!(theme.panel_bg, EditorTheme::dark().panel_bg);
        assert_eq!(theme.root_bg, EditorTheme::dark().root_bg);
    }

    #[test]
    fn the_document_round_trips_through_ron() {
        let doc = ThemeDoc::dark();
        let text = ron::ser::to_string_pretty(&doc, ron::ser::PrettyConfig::default()).unwrap();
        let back: ThemeDoc = ron::from_str(&text).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.resolve(), EditorTheme::dark());
    }

    #[test]
    fn the_registry_lists_every_token_the_default_document_sets() {
        // The registry (what a theming tool enumerates) and the authored default
        // must cover the same tokens.
        let doc = ThemeDoc::dark();
        for (name, _) in token_registry() {
            assert!(doc.tokens.contains_key(*name), "default doc missing {name}");
        }
        assert_eq!(doc.tokens.len(), token_registry().len());
    }
}
