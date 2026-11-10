//! # hyprcrop
//!
//! A compositor-aware Wayland screenshot tool.
//!
//! ## Module layout
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`domain`] | Shared data types, configuration, and error definitions used across the entire application |
//! | [`platform`] | I/O adapters for OS/Wayland (screencopy, clipboard, compositor IPC) |
//! | [`commands`] | CLI subcommand implementations (`crop` / `window` / `monitor` / `all` / `freeze`) |
//! | [`ui`] | Interactive overlay UI for freeze mode (iced_layershell) |

pub mod commands;
pub mod domain;
pub mod platform;
pub mod ui;
