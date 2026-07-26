//! aurora text: the bundled default font, the font system, and text metrics (ADR 0013).

use cosmic_text::{Attrs, FontSystem, Metrics, Weight, fontdb};

use crate::widget::FontWeight;

/// Default UI font size, in pixels.
pub(crate) const FONT_SIZE: f32 = 15.0;
/// Default line height, in pixels.
pub(crate) const LINE_HEIGHT: f32 = 20.0;

/// DejaVu Sans, bundled (Regular, Bold, ExtraLight) so text is deterministic and
/// needs no system fonts - and so a widget can pick a weight without depending
/// on whatever the host has installed.
const DEJAVU_SANS: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
const DEJAVU_SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");
const DEJAVU_SANS_LIGHT: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-ExtraLight.ttf");

/// Metrics for an arbitrary font size, keeping the default's line-height ratio
/// so larger text stays proportionally spaced.
pub(crate) fn metrics_for(font_size: f32) -> Metrics {
    Metrics::new(font_size, font_size * LINE_HEIGHT / FONT_SIZE)
}

/// The glyph a checked checkbox draws: a heavy check mark (present in the
/// bundled DejaVu Sans), so the tick is a real vector glyph, not a filled box.
pub(crate) const CHECK_MARK: &str = "\u{2714}";

/// Metrics for the checkbox check glyph. The font size is the box size itself
/// (not the smaller size whose line box would fill the box), so the tick's ink
/// fills roughly three-quarters of the box rather than reading as a small mark;
/// emit_checkbox centers it by the resulting line height.
pub(crate) fn checkmark_metrics(box_size: f32) -> Metrics {
    metrics_for(box_size)
}

/// Default text attributes (the bundled sans-serif at normal weight).
pub(crate) fn default_attrs() -> Attrs<'static> {
    Attrs::new()
}

/// Text attributes at `weight`, so shaping selects the matching bundled face.
pub(crate) fn attrs_for(weight: FontWeight) -> Attrs<'static> {
    let w = match weight {
        FontWeight::Light => Weight(250),
        FontWeight::Normal => Weight::NORMAL,
        FontWeight::Bold => Weight::BOLD,
    };
    Attrs::new().weight(w)
}

/// A font system pre-loaded with only the bundled faces - no slow system-font
/// scan, and fully deterministic. All faces share the "DejaVu Sans" family, so a
/// requested weight resolves to the nearest bundled face.
pub(crate) fn make_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(DEJAVU_SANS.to_vec());
    db.load_font_data(DEJAVU_SANS_BOLD.to_vec());
    db.load_font_data(DEJAVU_SANS_LIGHT.to_vec());
    db.set_sans_serif_family("DejaVu Sans");
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}
