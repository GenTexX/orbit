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
    "icon_active": Color => aurora.icon_active,
    "selection": Color => aurora.selection,
    "slider_handle": Color => aurora.slider_handle,
    "selection_inactive": Color => aurora.selection_inactive,
    "find_match": Color => aurora.find_match,
    "slider_track": Color => aurora.slider_track,
    "slider_fill": Color => aurora.slider_fill,
    "splitter": Color => aurora.splitter,
    "scrollbar_thumb": Color => aurora.scrollbar_thumb,
    "disabled_fade": Scalar => aurora.disabled_fade,
    // atlas surfaces, text, and accents.
    "panel_bg": Color => aurora.panel_bg,
    "root_bg": Color => aurora.root_bg,
    "bar_bg": Color => aurora.bar_bg,
    "header_bg": Color => aurora.header_bg,
    "heading": Color => aurora.heading,
    "subhead": Color => aurora.subhead,
    "row_selected": Color => aurora.row_selected,
    "row_drop": Color => row_drop,
    "menu_bg": Color => aurora.menu_bg,
    "card_bg": Color => aurora.card_bg,
    "card_border": Color => aurora.card_border,
    "panel_border": Color => aurora.panel_border,
    "field_border": Color => aurora.field_border,
    "tab_border": Color => aurora.tab_border,
    "mode_active": Color => mode_active,
    "playing": Color => playing,
    "axis_x": Color => axis_x,
    "axis_y": Color => axis_y,
    "console_warn": Color => console_warn,
    "code_keyword": Color => code_keyword,
    "code_number": Color => code_number,
    "code_string": Color => code_string,
    "code_comment": Color => code_comment,
    "code_function": Color => code_function,
    "code_type": Color => code_type,
    "code_annotation": Color => code_annotation,
    "code_error": Color => code_error,
    "code_warning": Color => code_warning,
    "code_occurrence": Color => code_occurrence,
    "scrim": Color => aurora.scrim,
    // shape scalars.
    "card_radius": Scalar => aurora.card_radius,
    "component_radius": Scalar => aurora.component_radius,
    "tab_radius": Scalar => aurora.tab_radius,
    "control_radius": Scalar => aurora.control_radius,
    "inset_radius": Scalar => aurora.inset_radius,
    "splitter_width": Scalar => aurora.splitter_width,
    "border_width": Scalar => aurora.border_width,
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
    // The authored document is the theme the editor actually ships with,
    // exported from a real settings file rather than derived from the built-in
    // colours - so what a fresh install looks like is what someone chose, and
    // changing it is editing a file rather than editing Rust.
    //
    // Anything it lacks is backfilled from the built-in dark theme below, so a
    // token added after the file was written still resolves and still appears
    // in a theming tool.
    if let Ok(mut authored) = ron::from_str::<ThemeDoc>(DEFAULT_THEME) {
        backfill_from(&mut authored, &built_in_doc());
        return authored;
    }
    built_in_doc()
}

/// The shipped theme, as a document.
const DEFAULT_THEME: &str = include_str!("../assets/default_theme.ron");

/// The theme derived from [`EditorTheme::dark`]: every token bound to a
/// variable when its value matches one, and to a literal otherwise. The
/// fallback, and the source of anything the authored document is missing.
fn built_in_doc() -> ThemeDoc {
    let d = EditorTheme::dark();
    // The palette. A token whose default equals one of these binds to it.
    let variables = BTreeMap::from([
        ("accent".to_string(), color_value(d.aurora.focus)),
        ("surface".to_string(), color_value(d.aurora.panel_bg)),
        ("surface_dim".to_string(), color_value(d.aurora.root_bg)),
        ("bar".to_string(), color_value(d.aurora.bar_bg)),
        ("header".to_string(), color_value(d.aurora.header_bg)),
        ("text".to_string(), color_value(d.aurora.heading)),
        ("text_dim".to_string(), color_value(d.aurora.subhead)),
        ("select".to_string(), color_value(d.aurora.row_selected)),
        ("border".to_string(), color_value(d.aurora.panel_border)),
        ("radius".to_string(), Value::Scalar(d.aurora.control_radius)),
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
    backfill_from(doc, &default_doc())
}

/// Add to `doc` every token `from` has and it lacks, with the variable each
/// binding needs.
fn backfill_from(doc: &mut ThemeDoc, dflt: &ThemeDoc) -> bool {
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
    fn the_built_in_document_resolves_to_the_built_in_dark_theme() {
        // The derived doc (palette + per-token binds) must resolve back to
        // exactly EditorTheme::dark() - keeping resolve, snapshot, and the
        // palette in agreement. This is the fallback, not what ships.
        assert_eq!(resolve(&built_in_doc()), EditorTheme::dark());
    }

    #[test]
    fn the_shipped_default_is_the_authored_document() {
        // What a fresh install looks like is a file someone chose, not the
        // built-in colours - so this is deliberately NOT dark().
        let doc = default_doc();
        assert_ne!(resolve(&doc), EditorTheme::dark());
        // But it is still complete: every token the registry knows resolves,
        // because anything the file lacks is backfilled from the built-in one.
        for token in TOKENS {
            assert!(
                doc.resolved(token.name).is_some(),
                "the shipped default is missing {}",
                token.name
            );
        }
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
        assert_eq!(
            theme.aurora.panel_bg,
            Color::rgb(0.1, 0.2, 0.3),
            "var reference"
        );
        assert_eq!(theme.aurora.root_bg, Color::rgb(0.9, 0.8, 0.7), "literal");
        assert_eq!(theme.aurora.control_radius, 9.0, "scalar literal");
        assert_eq!(
            theme.aurora.bar_bg,
            EditorTheme::dark().aurora.bar_bg,
            "unset falls back"
        );
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
        assert_eq!(theme.aurora.panel_bg, EditorTheme::dark().aurora.panel_bg);
        assert_eq!(theme.aurora.root_bg, EditorTheme::dark().aurora.root_bg);
    }

    #[test]
    fn the_shipped_default_binds_every_token_to_the_right_kind() {
        // A colour bound where a scalar belongs resolves to the built-in value
        // and looks like nothing is wrong - which is how the exported file
        // arrived with `disabled_fade` bound to a Color.
        let doc = default_doc();
        for token in TOKENS {
            let Some(value) = doc.resolved(token.name) else {
                continue;
            };
            assert_eq!(
                value.is_color(),
                token.kind == Kind::Color,
                "{} is bound to the wrong kind of value",
                token.name
            );
        }
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
        assert_eq!(resolve(&back), resolve(&doc));
    }

    #[test]
    fn backfill_adds_missing_tokens_and_leaves_a_complete_doc_untouched() {
        // A doc missing a token (an older file) gets it back, resolving to the
        // built-in default; a doc that already has every token is unchanged.
        let mut doc = default_doc();
        doc.tokens.remove("tab_radius");
        assert!(backfill_missing_tokens(&mut doc), "a token was added back");
        assert_eq!(
            resolve(&doc).aurora.tab_radius,
            resolve(&default_doc()).aurora.tab_radius,
            "backfilled token resolves to what the default says"
        );
        assert_eq!(doc, default_doc(), "backfill restores the full default doc");
        assert!(
            !backfill_missing_tokens(&mut doc),
            "a complete doc needs no backfill"
        );
    }
}
