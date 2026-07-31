//! The on-disk editor settings (`settings.ron`): the theme document plus atlas's
//! view toggles. Read and written by both atlas (which owns the defaults) and
//! prism (which edits the theme). Lives in `$XDG_CONFIG_HOME/orbit`, else
//! `~/.config/orbit`.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::theme::ThemeDoc;

/// Transform snapping: whether a viewport drag snaps, and to what increments.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapSettings {
    pub enabled: bool,
    pub move_step: f32,
    pub rotate_step_deg: f32,
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

/// The whole settings file. The `theme` defaults to an empty document (which
/// resolves to the built-in dark theme); atlas writes a full authored default on
/// first run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeDoc,
    #[serde(default = "default_true")]
    pub show_grid: bool,
    #[serde(default = "default_true")]
    pub show_axes: bool,
    /// Tidy a script on save: strip trailing whitespace from every line, and
    /// end the file with exactly one newline. On by default, and switchable
    /// because silently rewriting someone's file should be their call.
    #[serde(default = "default_true")]
    pub tidy_on_save: bool,
    pub snap: SnapSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeDoc::default(),
            show_grid: true,
            show_axes: true,
            tidy_on_save: true,
            snap: SnapSettings::default(),
        }
    }
}

/// The settings file path (`<config dir>/settings.ron`).
pub fn settings_path() -> Option<PathBuf> {
    let dir = if let Some(base) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        PathBuf::from(base).join("orbit")
    } else {
        PathBuf::from(std::env::var_os("HOME")?)
            .join(".config")
            .join("orbit")
    };
    Some(dir.join("settings.ron"))
}

/// The settings file's last-modified time, if it exists (for change detection).
pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(settings_path()?).ok()?.modified().ok()
}

/// Read and parse the settings file, or `None` if it is missing or invalid (an
/// invalid file logs a warning and is left as-is for the user to fix).
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

/// Write `settings` to the settings file (creating the config directory).
pub fn save(settings: &Settings) -> std::io::Result<()> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = ron::ser::to_string_pretty(settings, ron::ser::PrettyConfig::default())
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Bind, Value};

    #[test]
    fn settings_round_trip_through_ron() {
        let mut settings = Settings {
            show_grid: false,
            snap: SnapSettings {
                enabled: true,
                move_step: 8.0,
                ..SnapSettings::default()
            },
            ..Settings::default()
        };
        settings.theme.tokens.insert(
            "panel_bg".into(),
            Bind::Lit(Value::Color(0.1, 0.2, 0.3, 1.0)),
        );
        let text =
            ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default()).unwrap();
        let back: Settings = ron::from_str(&text).unwrap();
        assert_eq!(back.theme, settings.theme);
        assert!(!back.show_grid);
        assert!(back.show_axes, "unset field defaults on");
        assert_eq!(back.snap, settings.snap);
    }

    #[test]
    fn an_empty_settings_file_falls_back_to_defaults() {
        let back: Settings = ron::from_str("()").unwrap();
        assert_eq!(back.theme, ThemeDoc::default());
        assert!(back.show_grid && back.show_axes, "toggles default on");
    }
}
