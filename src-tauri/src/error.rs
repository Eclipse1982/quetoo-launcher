use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum LauncherError {
    #[error("network error: {0}")]
    Network(String),
    #[error("GitHub rate limit exceeded; try again later")]
    RateLimit,
    #[error("no Quetoo build available for this platform ({0})")]
    UnsupportedPlatform(String),
    #[error("expected asset not found in release: {0}")]
    AssetNotFound(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("failed to extract archive: {0}")]
    Extract(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("failed to launch Quetoo: {0}")]
    Launch(String),
}

impl Serialize for LauncherError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<std::io::Error> for LauncherError {
    fn from(e: std::io::Error) -> Self {
        LauncherError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LauncherError>;
