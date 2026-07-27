//! atlas settings: a thin layer over the shared [`spectrum::settings`] file.
//!
//! The model and IO live in spectrum (so prism shares them); atlas only owns the
//! startup default, because the default theme *document* is authored from atlas's
//! own [`EditorTheme`](crate::ui::EditorTheme) - which spectrum cannot see.

pub use spectrum::settings::{Settings, SnapSettings, modified, read, save};

use crate::theme::default_doc;

/// Load the user's settings for startup. If the file does not exist yet, write
/// the defaults - including the full authored dark theme document - so there is
/// something to edit. An invalid file falls back to the defaults (and is left
/// as-is for the user to fix).
pub fn load() -> Settings {
    if let Some(settings) = read() {
        return settings;
    }
    let settings = Settings {
        theme: default_doc(),
        ..Settings::default()
    };
    // Only write when the file is genuinely absent, never clobbering an invalid
    // one the user is mid-edit on.
    let absent = spectrum::settings::settings_path().is_some_and(|p| !p.exists());
    if absent {
        match save(&settings) {
            Ok(()) => tracing::info!("wrote default settings"),
            Err(err) => tracing::warn!("could not write default settings: {err}"),
        }
    }
    settings
}
