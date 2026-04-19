//! # domain::error
//!
//! Application-wide error type `AppError` and `Result<T>` alias.
//!
//! `AppError` is implemented via `thiserror::Error` derive. Each variant explicitly names
//! its origin (command, IPC, configuration, Wayland, etc.).
//! `exit_code()` maps error variants to UNIX exit codes.

use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Command not found or failed to spawn: {0}: {1}")]
    CommandNotFound(String, #[source] std::io::Error),

    #[error("Command {0} failed with exit status: {1}")]
    CommandFailed(String, std::process::ExitStatus),

    #[error("Hyprland IPC error ({0}): {1}")]
    HyprlandIpc(String, #[source] std::io::Error),

    #[error("Hyprland IPC environment variable {0} error: {1}")]
    HyprlandEnvVar(&'static str, #[source] std::env::VarError),

    #[error("JSON parse error in {0}: {1}")]
    JsonParse(String, #[source] serde_json::Error),

    #[error("Invalid configuration: {0}")]
    Config(String),

    #[error("Failed to load or parse TOML config: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("User cancelled operation")]
    UserCancelled,

    #[error("Slurp returned empty geometry")]
    EmptyGeometry,

    #[error("No focused monitor found")]
    NoFocusedMonitor,

    #[error("Image processing error: {0}")]
    Image(#[from] image::ImageError),

    #[error("Iced Layershell error: {0}")]
    LayerShell(String),

    #[error("Wayland error: {0}")]
    Wayland(String),

    #[error("Screencopy failed: {0}")]
    Screencopy(String),

    #[error("File system error on path {0}: {1}")]
    FileSystem(PathBuf, #[source] std::io::Error),

    #[error("Generic I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("freeze is already running")]
    FreezeLockBusy,

    #[error("Other error: {0}")]
    Other(String),
}

impl AppError {
    pub fn exit_code(&self) -> i32 {
        match self {
            AppError::CommandNotFound(_, _) => 127,
            AppError::UserCancelled => 130,
            AppError::Config(_) | AppError::TomlParse(_) => 2,
            AppError::FreezeLockBusy => 1,
            AppError::EmptyGeometry => 1,
            _ => 1,
        }
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_error_exit_codes() {
        // Command not found -> 127
        let err = AppError::CommandNotFound(
            "test".to_string(),
            io::Error::new(io::ErrorKind::NotFound, "not found"),
        );
        assert_eq!(err.exit_code(), 127);

        // User cancelled -> 130
        let err = AppError::UserCancelled;
        assert_eq!(err.exit_code(), 130);

        // Config error -> 2
        let err = AppError::Config("test".to_string());
        assert_eq!(err.exit_code(), 2);

        // Empty geometry -> 1
        let err = AppError::EmptyGeometry;
        assert_eq!(err.exit_code(), 1);

        // Generic IO -> 1
        let err = AppError::Io(io::Error::other("io error"));
        assert_eq!(err.exit_code(), 1);
    }
}
