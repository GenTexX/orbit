//! atlas user settings: per-user preferences (currently the editor theme),
//! persisted as RON at `~/.config/orbit/settings.ron` (or `$XDG_CONFIG_HOME`).
//!
//! Settings are a user-level concern, not a project one - they live in the
//! user's config directory, never in a project. On first run the defaults are
//! written out so the file exists and can be edited by hand; the editor reads it
//! at startup and hot-reloads it (polling its mtime) so edits apply live.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::ui::EditorTheme;

/// The user's editor settings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The editor color theme (every surface, text, and accent color). Defaults
    /// to [`EditorTheme::dark`] (which is `EditorTheme`'s `Default`).
    pub theme: EditorTheme,
    /// Show the editor grid in the viewport (default on).
    #[serde(default = "default_true")]
    pub show_grid: bool,
    /// Show the world origin axes in the viewport (default on).
    #[serde(default = "default_true")]
    pub show_axes: bool,
    /// Transform snapping (increments and whether it is on).
    #[serde(default)]
    pub snap: SnapSettings,
}

/// Transform snapping: whether a viewport drag snaps, and to what increments.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapSettings {
    /// Whether snapping is on (toggled from the toolbar, Ctrl-inverted per drag).
    pub enabled: bool,
    /// Translation snaps to this many world units.
    pub move_step: f32,
    /// Rotation snaps to this many degrees.
    pub rotate_step_deg: f32,
    /// Scale snaps to this multiple.
    pub scale_step: f32,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            move_step: 16.0,
            rotate_step_deg: 15.0,
            scale_step: 0.1,
        }
    }
}

fn default_true() -> bool {
    true
}

// A hand-written Default (not derived) so the grid/axes toggles default ON: a
// derived Default would make the bools false, and load() returns Default when no
// settings file exists yet (first run). The field-level serde defaults handle
// old files that predate these fields.
impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: EditorTheme::default(),
            show_grid: true,
            show_axes: true,
            snap: SnapSettings::default(),
        }
    }
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

/// Read and parse the settings file, or `None` if it is missing or invalid (an
/// invalid file logs a warning and is left as-is for the user to fix). Never
/// writes - used for hot-reload polling.
pub fn read() -> Option<Settings> {
    let path = settings_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    match ron::from_str::<Settings>(&text) {
        Ok(settings) => Some(settings),
        Err(err) => {
            tracing::warn!("settings at {} are invalid: {err}", path.display());
            None
        }
    }
}

/// The settings file's last-modified time, if it exists (for change detection).
pub fn modified() -> Option<SystemTime> {
    let path = settings_path()?;
    std::fs::metadata(&path).ok()?.modified().ok()
}

/// Load the user's settings for startup. If the file does not exist yet, write
/// the defaults so there is something to edit, then return them. An invalid file
/// falls back to the defaults (and is left as-is for the user to fix).
pub fn load() -> Settings {
    if let Some(settings) = read() {
        return settings;
    }
    let settings = Settings::default();
    // Only write defaults when the file is genuinely absent, never clobbering an
    // invalid one the user is mid-edit on.
    let absent = settings_path().is_some_and(|p| !p.exists());
    if absent {
        match save(&settings) {
            Ok(()) => tracing::info!("wrote default settings"),
            Err(err) => tracing::warn!("could not write default settings: {err}"),
        }
    }
    settings
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_ron() {
        let settings = Settings {
            theme: EditorTheme::light(),
            show_grid: false,
            show_axes: true,
            snap: SnapSettings {
                enabled: true,
                move_step: 8.0,
                rotate_step_deg: 45.0,
                scale_step: 0.25,
            },
        };
        let pretty = ron::ser::PrettyConfig::default();
        let text = ron::ser::to_string_pretty(&settings, pretty).unwrap();
        let back: Settings = ron::from_str(&text).unwrap();
        assert_eq!(back.theme, EditorTheme::light());
        assert!(!back.show_grid);
        assert!(back.show_axes);
        assert_eq!(back.snap, settings.snap);
    }

    #[test]
    fn an_empty_settings_file_falls_back_to_defaults() {
        // #[serde(default)] fills the theme from EditorTheme::default() (dark),
        // and the grid/axes toggles default ON.
        let back: Settings = ron::from_str("()").unwrap();
        assert_eq!(back.theme, EditorTheme::dark());
        assert!(back.show_grid, "grid defaults on");
        assert!(back.show_axes, "axes default on");
    }

    #[test]
    fn an_old_settings_file_without_the_grid_fields_defaults_them_on() {
        // A file written before show_grid/show_axes existed: only `theme`.
        let text = ron::ser::to_string_pretty(
            &EditorThemeOnly {
                theme: EditorTheme::light(),
            },
            ron::ser::PrettyConfig::default(),
        )
        .unwrap();
        let back: Settings = ron::from_str(&text).unwrap();
        assert_eq!(back.theme, EditorTheme::light());
        assert!(
            back.show_grid && back.show_axes,
            "missing fields default on"
        );
    }

    #[derive(Serialize)]
    struct EditorThemeOnly {
        theme: EditorTheme,
    }
}
