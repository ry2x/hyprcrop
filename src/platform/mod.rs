//! # platform
//!
//! I/O adapter layer for OS and Wayland integration.
//! Contains no business logic — wraps external resources and returns results
//! expressed in `domain` types.
//!
//! ## Submodules
//!
//! | Module | Contents |
//! |---|---|
//! | [`capture`] | Screenshot capture backends (screencopy / portal) |
//! | [`system`] | Thin wrappers for OS commands, clipboard, compositor IPC, and notifications |

pub mod capture;
pub mod system;
