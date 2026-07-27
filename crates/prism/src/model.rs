//! The theme document and settings model prism reads and writes.
//!
//! This mirrors atlas's `theme` + `settings` serde format exactly, so prism can
//! round-trip the whole `settings.ron` (preserving atlas's non-theme fields like
//! the grid/snap toggles while editing only the theme). It is a deliberate copy
//! for now; the shared model wants to move into its own crate that both atlas
//! and prism depend on - tracked as follow-up cleanup.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A value a theme holds: an RGBA color, or a scalar (a radius/width factor).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Color(f32, f32, f32, f32),
    Scalar(f32),
}

/// How a token gets its value: a literal, or a reference to a named variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Bind {
    Lit(Value),
    Var(String),
}

/// An authored theme: a palette of named `variables` and the `tokens` the editor
/// reads, each a [`Bind`] onto a variable or a literal.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeDoc {
    pub variables: BTreeMap<String, Value>,
    pub tokens: BTreeMap<String, Bind>,
}

impl ThemeDoc {
    /// The concrete value a token resolves to (following a [`Bind::Var`]), or
    /// `None` if the token is absent or its variable is unknown. Prism uses this
    /// to show a live swatch of a var-bound token.
    pub fn resolved(&self, token: &str) -> Option<Value> {
        match self.tokens.get(token)? {
            Bind::Lit(v) => Some(*v),
            Bind::Var(name) => self.variables.get(name).copied(),
        }
    }
}

/// Transform snapping - carried through untouched so saving the theme does not
/// disturb atlas's snap settings.
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

/// The full editor settings file. Prism edits `theme` and preserves the rest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeDoc,
    #[serde(default = "default_true")]
    pub show_grid: bool,
    #[serde(default = "default_true")]
    pub show_axes: bool,
    pub snap: SnapSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeDoc::default(),
            show_grid: true,
            show_axes: true,
            snap: SnapSettings::default(),
        }
    }
}

/// The `orbit` settings file: `$XDG_CONFIG_HOME/orbit/settings.ron`, else
/// `~/.config/orbit/settings.ron`.
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

/// Read and parse the settings file, or `None` if it is missing or invalid.
pub fn read() -> Option<Settings> {
    let text = std::fs::read_to_string(settings_path()?).ok()?;
    match ron::from_str::<Settings>(&text) {
        Ok(s) => Some(s),
        Err(err) => {
            tracing::warn!("settings are invalid: {err}");
            None
        }
    }
}

/// Write `settings` back to the file (creating the config dir).
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

    #[test]
    fn a_theme_document_round_trips_through_ron() {
        let mut doc = ThemeDoc::default();
        doc.variables
            .insert("accent".into(), Value::Color(0.3, 0.55, 0.9, 1.0));
        doc.variables.insert("radius".into(), Value::Scalar(4.0));
        doc.tokens
            .insert("panel_bg".into(), Bind::Var("accent".into()));
        doc.tokens.insert(
            "root_bg".into(),
            Bind::Lit(Value::Color(0.1, 0.1, 0.1, 1.0)),
        );
        let settings = Settings {
            theme: doc,
            ..Settings::default()
        };
        let text =
            ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default()).unwrap();
        let back: Settings = ron::from_str(&text).unwrap();
        assert_eq!(back.theme, settings.theme);
        assert_eq!(
            back.theme.resolved("panel_bg"),
            Some(Value::Color(0.3, 0.55, 0.9, 1.0)),
            "a var-bound token resolves through its variable"
        );
    }
}
