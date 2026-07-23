//! aurora text: the bundled default font, the font system, and text metrics (ADR 0013).

use cosmic_text::{Attrs, FontSystem, Metrics, fontdb};

/// Default UI font size, in pixels.
pub(crate) const FONT_SIZE: f32 = 15.0;
/// Default line height, in pixels.
pub(crate) const LINE_HEIGHT: f32 = 20.0;

/// DejaVu Sans, bundled so text is deterministic and needs no system fonts (and
/// so tests and CI do not depend on the machine's installed fonts).
const DEJAVU_SANS: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

/// Metrics for the default UI font.
pub(crate) fn metrics() -> Metrics {
    Metrics::new(FONT_SIZE, LINE_HEIGHT)
}

/// Default text attributes (resolve to the bundled sans-serif font).
pub(crate) fn default_attrs() -> Attrs<'static> {
    Attrs::new()
}

/// A font system pre-loaded with only the bundled font - no slow system-font
/// scan, and fully deterministic.
pub(crate) fn make_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(DEJAVU_SANS.to_vec());
    db.set_sans_serif_family("DejaVu Sans");
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}
