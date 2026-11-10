//! # platform::capture
//!
//! Screenshot capture implementations, one submodule per backend.
//!
//! | Module | Method | Use case |
//! |---|---|---|
//! | [`screencopy`] | `zwlr_screencopy_manager_v1` | Standard capture (crop / window / monitor / all / freeze) |
//! | [`portal`] | `xdg-desktop-portal` + PipeWire | `portal` subcommand — handles transparent windows |
//! | [`toplevel_export`] | `hyprland-toplevel-export-v1` | Hyprland direct window capture |

pub mod portal;
pub mod screencopy;
pub mod toplevel_export;
pub(crate) mod wl_shared;
