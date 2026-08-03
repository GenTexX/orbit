//! helios errors: saving and loading scenes and projects.

use std::path::PathBuf;

use thiserror::Error;

/// An error saving or loading a scene or project.
#[derive(Debug, Error)]
pub enum HeliosError {
    /// A file could not be read or written.
    #[error("{action} {path:?}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A scene could not be printed as RON.
    #[error("serializing scene to RON: {0}")]
    RonSerialize(#[from] ron::Error),
    /// A scene RON file could not be parsed.
    #[error("parsing scene RON: {0}")]
    RonParse(#[from] ron::error::SpannedError),
    /// The project manifest could not be printed or parsed.
    #[error("project manifest: {0}")]
    Manifest(String),
    /// A serialized component named a kind this build does not know.
    #[error("unknown component kind {0:?}")]
    UnknownComponent(String),
    /// The scene file's format is newer than this build reads.
    #[error("{0}")]
    Format(String),
}

impl HeliosError {
    /// Build an [`Io`](Self::Io) error tagging the file and what was attempted.
    pub(crate) fn io(
        action: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}
