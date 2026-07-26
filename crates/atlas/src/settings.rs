//! atlas user settings: per-user preferences (currently the editor theme),
//! persisted as RON at `~/.config/orbit/settings.ron` (or `$XDG_CONFIG_HOME`).
//!
//! Settings are a user-level concern, not a project one - they live in the
//! user's config directory, never in a project. On first run the defaults are
//! written out so the file exists and can be edited by hand; the editor reads it
//! at startup. Editing colors there recolors the whole editor next launch.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ui::EditorTheme;

/// The user's editor settings.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The editor color theme (every surface, text, and accent color). Defaults
    /// to [`EditorTheme::dark`] (which is `EditorTheme`'s `Default`).
    pub theme: EditorTheme,
}

/// The `orbit` config directory: `$XDG_CONFIG_HOME/orbit`, else `~/.config/orbit`.
fn config_dir() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(base).join("orbit"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("orbit"))
}

/// The settings file path (`<config dir>/settings.ron`).
fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.ron"))
}

/// Load the user's settings. If the file does not exist yet, write the defaults
/// so there is something to edit, then return them. A parse error logs a warning
/// and falls back to the defaults (the file is left as-is for the user to fix).
pub fn load() -> Settings {
    let Some(path) = settings_path() else {
        return Settings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => match ron::from_str::<Settings>(&text) {
            Ok(settings) => settings,
            Err(err) => {
                tracing::warn!(
                    "settings at {} are invalid: {err}; using defaults",
                    path.display()
                );
                Settings::default()
            }
        },
        Err(_) => {
            let settings = Settings::default();
            if let Err(err) = save(&settings) {
                tracing::warn!(
                    "could not write default settings to {}: {err}",
                    path.display()
                );
            } else {
                tracing::info!("wrote default settings to {}", path.display());
            }
            settings
        }
    }
}

/// Write `settings` to the user's settings file (creating the config directory).
pub fn save(settings: &Settings) -> std::io::Result<()> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pretty = ron::ser::PrettyConfig::default();
    let text = ron::ser::to_string_pretty(settings, pretty)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, text)
}
