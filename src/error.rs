use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("config: {0}")]
    Config(String),

    #[error("network: {0}")]
    Network(String),

    #[error("Subsonic API ({code}): {message}")]
    Api { code: i32, message: String },

    #[error("audio: {0}")]
    Audio(String),

    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Other(String),
}

impl AppError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
