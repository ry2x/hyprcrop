//! # domain
//!
//! Domain model shared across the entire application.
//! Primarily contains pure data definitions, calculations, and configuration parsing, with a
//! narrow exception for small local persistence in [`state`] to remember the last-used capture
//! mode across sessions.
//!
//! ## Submodules
//!
//! | Module | Contents |
//! |---|---|
//! | [`config`] | TOML configuration schema, loading, and default values |
//! | [`error`] | Application-wide error type `AppError` and `Result<T>` alias |
//! | [`geometry`] | Coordinate calculations (slurp string parsing, logical-to-physical conversion, clamping) |
//! | [`state`] | Persists the last-used capture mode for freeze mode across sessions |
//! | [`types`] | Shared data structures: `ScreenRect`, `MonitorInfo`, `WindowInfo`, etc. |

pub mod config;
pub mod constants;
pub mod error;
pub mod geometry;
pub mod state;
pub mod types;
