//! Error types for `topbar-core`.

use std::path::PathBuf;

/// Result type alias using the crate's [`Error`] type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur while loading or validating configuration.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration file not found.
    #[error("config file not found: {0}")]
    NotFound(PathBuf),

    /// Failed to read a configuration file.
    #[error("failed to read config file: {0}")]
    Read(#[from] std::io::Error),

    /// Failed to parse TOML configuration.
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),

    /// Configuration validation failed.
    #[error("config validation failed:\n{}", .0.join("\n"))]
    Validation(Vec<String>),

    /// Configuration warnings were treated as errors (`--strict`).
    #[error("config warnings treated as errors (--strict):\n{}", .0.join("\n"))]
    StrictWarnings(Vec<String>),

    /// The configuration could not be written back out.
    ///
    /// Only `topbar dump` can reach this: nothing on the panel's own paths
    /// serialises a configuration.
    #[error("failed to render the config: {0}")]
    Serialize(String),
}
