//! # platform::system
//!
//! Thin wrappers around external processes, OS APIs, and compositor IPC.
//! Each submodule is responsible for exactly one external resource.
//!
//! | Module | External resource |
//! |---|---|
//! | [`clipboard`] | Wayland clipboard writes via `wl-copy` |
//! | [`cmd`] | External command name constants and generic process execution utilities |
//! | [`hyprland`] | Hyprland IPC over Unix socket |
//! | [`mango`] | MangoWM JSON IPC over Unix socket |
//! | [`compositor`] | Runtime compositor detection and shared backend interface |
//! | [`notify`] | Desktop notifications via `notify-send` |
//! | [`lock`] | Exclusive process-level lock for freeze mode via BSD flock |

pub mod clipboard;
pub mod cmd;
pub mod compositor;
pub mod hyprland;
pub mod lock;
pub mod mango;
pub mod notify;
