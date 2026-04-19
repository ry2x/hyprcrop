//! # platform::system
//!
//! Thin wrappers around external processes, OS APIs, and Hyprland IPC.
//! Each submodule is responsible for exactly one external resource.
//!
//! | Module | External resource |
//! |---|---|
//! | [`clipboard`] | Wayland clipboard writes via `wl-copy` |
//! | [`cmd`] | External command name constants and generic process execution utilities |
//! | [`hyprland`] | Hyprland IPC over Unix socket (monitor, window, and layer surface queries) |
//! | [`notify`] | Desktop notifications via `notify-send` |
//! | [`lock`] | Exclusive process-level lock for freeze mode via BSD flock |

pub mod clipboard;
pub mod cmd;
pub mod hyprland;
pub mod lock;
pub mod notify;
