//! atlas theming: resolve an authored theme document into the concrete
//! [`EditorTheme`] the UI draws with.
//!
//! The document model + token registry live in the shared [`spectrum`] crate;
//! this crate owns the *mapping* from each token to an [`EditorTheme`] field
//! (that mapping is atlas-specific - it names aurora colors and atlas's own
//! fields). A `theme_tokens!` macro writes it once and generates [`resolve`]
//! (document -> theme) and `snapshot` (theme -> token values). [`default_doc`]
//! authors the built-in dark theme as a document - what a fresh settings file is
//! seeded with. A missing token, a dangling variable, or a wrong-kinded value
//! falls back to the built-in dark default, so a partial file still works.

use std::collections::BTreeMap;

use aurora::Color;
pub use spectrum::theme::ThemeDoc;
use spectrum::theme::{Bind, Value};

use crate::ui::EditorTheme;

/// An aurora color as a theme [`Value`].
fn color_value(c: Color) -> Value {
    Value::Color(c.r, c.g, c.b, c.a)
}

/// A token as a color, falling back to `default` when it is missing or not a
/// color.
fn doc_color(doc: &ThemeDoc, token: &str, default: Color) -> Color {
    doc.resolved(token)
        .and_then(Value::channels)
        .map(|[r, g, b, a]| Color::rgba(r, g, b, a))
        .unwrap_or(default)
}

/// A token as a scalar, falling back to `default` when it is missing or not a
/// scalar.
fn doc_scalar(doc: &ThemeDoc, token: &str, default: f32) -> f32 {
    doc.resolved(token)
        .and_then(Value::scalar)
        .unwrap_or(default)
}

// One source of truth for the token <-> EditorTheme-field mapping; the macro
// expands it into resolve (doc -> theme) and snapshot (theme -> tokens). Adding
// a slot is one line here (and its metadata one line in spectrum's registry).

macro_rules! snap_value {
    (Color, $e:expr) => {
        color_value($e)
    };
    (Scalar, $e:expr) => {
        Value::Scalar($e)
    };
}

macro_rules! read_token {
    (Color, $doc:expr, $name:expr, $default:expr) => {
        doc_color($doc, $name, $default)
    };
    (Scalar, $doc:expr, $name:expr, $default:expr) => {
        doc_scalar($doc, $name, $default)
    };
}

macro_rules! theme_tokens {
    ( $( $name:literal : $kind:ident => $($field:ident).+ ),* $(,)? ) => {
        /// Snapshot a concrete theme into token values (the inverse of resolve,
        /// used to author a default document from the built-in dark theme).
        fn snapshot(d: &EditorTheme) -> BTreeMap<String, Value> {
            let mut m = BTreeMap::new();
            $( m.insert($name.to_string(), snap_value!($kind, d.$($field).+)); )*
            m
        }

        /// Resolve a document into the concrete runtime theme: every field starts
        /// from the built-in dark default and is overridden by its token when the
        /// document provides one.
        pub fn resolve(doc: &ThemeDoc) -> EditorTheme {
            let mut t = EditorTheme::dark();
            $( t.$($field).+ = read_token!($kind, doc, $name, t.$($field).+); )*
            t
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
    "icon_active": Color => icon_active,
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
    "panel_border": Color => panel_border,
    "field_border": Color => field_border,
    "tab_border": Color => tab_border,
    "mode_active": Color => mode_active,
    "axis_x": Color => axis_x,
    "axis_y": Color => axis_y,
    "console_warn": Color => console_warn,
    "scrim": Color => scrim,
    // shape scalars.
    "card_radius": Scalar => card_radius,
    "component_radius": Scalar => component_radius,
    "tab_radius": Scalar => tab_radius,
    "control_radius": Scalar => control_radius,
    "inset_radius": Scalar => inset_radius,
    "splitter_width": Scalar => splitter_width,
    "border_width": Scalar => border_width,
    // viewport (scene view) colors.
    "viewport_bg": Color => viewport_bg,
    "grid_line": Color => grid_line,
    "grid_line_strong": Color => grid_line_strong,
    "selection_outline": Color => selection_outline,
    "gizmo_rotate": Color => gizmo_rotate,
    "gizmo_scale": Color => gizmo_scale,
}

/// The authored default dark theme: a small palette of variables, and every
/// token bound to a variable when its value matches one (so the palette is
/// shared) or to a literal otherwise. This is what is written to a fresh
/// settings file - a complete, editable starting point.
pub fn default_doc() -> ThemeDoc {
    let d = EditorTheme::dark();
    // The palette. A token whose default equals one of these binds to it.
    let variables = BTreeMap::from([
        ("accent".to_string(), color_value(d.aurora.focus)),
        ("surface".to_string(), color_value(d.panel_bg)),
        ("surface_dim".to_string(), color_value(d.root_bg)),
        ("bar".to_string(), color_value(d.bar_bg)),
        ("header".to_string(), color_value(d.header_bg)),
        ("text".to_string(), color_value(d.heading)),
        ("text_dim".to_string(), color_value(d.subhead)),
        ("select".to_string(), color_value(d.row_selected)),
        ("border".to_string(), color_value(d.panel_border)),
        ("radius".to_string(), Value::Scalar(d.control_radius)),
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
    ThemeDoc { variables, tokens }
}

/// Bring an older document up to date: add any themeable tokens it lacks (ones
/// introduced after the file was written), each bound to its built-in default,
/// so a theming tool can edit every token. Returns whether anything was added.
/// When a missing token's default is a variable the document also lacks, that
/// variable is re-added too, so the binding still resolves.
pub fn backfill_missing_tokens(doc: &mut ThemeDoc) -> bool {
    let dflt = default_doc();
    let mut added = false;
    for (name, bind) in &dflt.tokens {
        if doc.tokens.contains_key(name) {
            continue;
        }
        if let Bind::Var(var) = bind
            && !doc.variables.contains_key(var)
            && let Some(value) = dflt.variables.get(var)
        {
            doc.variables.insert(var.clone(), *value);
        }
        doc.tokens.insert(name.clone(), bind.clone());
        added = true;
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectrum::theme::{Kind, TOKENS};

    #[test]
    fn the_default_document_resolves_to_the_built_in_dark_theme() {
        // The authored dark doc (palette + per-token binds) must resolve back to
        // exactly EditorTheme::dark() - keeping resolve, snapshot, and the palette
        // in agreement.
        assert_eq!(resolve(&default_doc()), EditorTheme::dark());
    }

    #[test]
    fn an_empty_document_resolves_to_all_defaults() {
        assert_eq!(resolve(&ThemeDoc::default()), EditorTheme::dark());
    }

    #[test]
    fn a_variable_reference_and_a_literal_both_resolve() {
        let doc = ThemeDoc {
            variables: BTreeMap::from([("primary".to_string(), Value::Color(0.1, 0.2, 0.3, 1.0))]),
            tokens: BTreeMap::from([
                ("panel_bg".to_string(), Bind::Var("primary".to_string())),
                (
                    "root_bg".to_string(),
                    Bind::Lit(Value::Color(0.9, 0.8, 0.7, 1.0)),
                ),
                ("control_radius".to_string(), Bind::Lit(Value::Scalar(9.0))),
            ]),
        };
        let theme = resolve(&doc);
        assert_eq!(theme.panel_bg, Color::rgb(0.1, 0.2, 0.3), "var reference");
        assert_eq!(theme.root_bg, Color::rgb(0.9, 0.8, 0.7), "literal");
        assert_eq!(theme.control_radius, 9.0, "scalar literal");
        assert_eq!(theme.bar_bg, EditorTheme::dark().bar_bg, "unset falls back");
    }

    #[test]
    fn a_dangling_variable_or_wrong_kind_falls_back() {
        let doc = ThemeDoc {
            variables: BTreeMap::new(),
            tokens: BTreeMap::from([
                ("panel_bg".to_string(), Bind::Var("missing".to_string())),
                ("root_bg".to_string(), Bind::Lit(Value::Scalar(3.0))),
            ]),
        };
        let theme = resolve(&doc);
        assert_eq!(theme.panel_bg, EditorTheme::dark().panel_bg);
        assert_eq!(theme.root_bg, EditorTheme::dark().root_bg);
    }

    #[test]
    fn the_registry_and_the_field_mapping_agree() {
        // spectrum's registry (what a tool enumerates) and atlas's field mapping
        // (what resolve reads) must cover the same tokens with the same kinds.
        let doc = default_doc();
        for token in TOKENS {
            let value = doc
                .resolved(token.name)
                .unwrap_or_else(|| panic!("default doc missing {}", token.name));
            assert_eq!(
                value.is_color(),
                token.kind == Kind::Color,
                "{} kind mismatch",
                token.name
            );
        }
        // No extra field-mapping tokens beyond the registry.
        assert_eq!(doc.tokens.len(), TOKENS.len());
    }

    #[test]
    fn the_document_round_trips_through_ron() {
        let doc = default_doc();
        let text = ron::ser::to_string_pretty(&doc, ron::ser::PrettyConfig::default()).unwrap();
        let back: ThemeDoc = ron::from_str(&text).unwrap();
        assert_eq!(back, doc);
        assert_eq!(resolve(&back), EditorTheme::dark());
    }

    #[test]
    fn backfill_adds_missing_tokens_and_leaves_a_complete_doc_untouched() {
        // A doc missing a token (an older file) gets it back, resolving to the
        // built-in default; a doc that already has every token is unchanged.
        let mut doc = default_doc();
        doc.tokens.remove("tab_radius");
        assert!(backfill_missing_tokens(&mut doc), "a token was added back");
        assert_eq!(
            resolve(&doc).tab_radius,
            EditorTheme::dark().tab_radius,
            "backfilled token resolves to its default"
        );
        assert_eq!(doc, default_doc(), "backfill restores the full default doc");
        assert!(
            !backfill_missing_tokens(&mut doc),
            "a complete doc needs no backfill"
        );
    }
}
