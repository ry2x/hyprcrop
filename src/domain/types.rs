//! # domain::types
//!
//! Shared data structures used across multiple modules.
//! External dependencies are minimal — `serde` only.
//!
//! | Type | Purpose |
//! |---|---|
//! | `ScreenRect` | Rectangular region in logical pixel coordinates |
//! | `WindowInfo` | Compositor window metadata |
//! | `MonitorInfo` | Compositor monitor metadata |
//! | `BorderStyle` | Compositor border and rounding values |
//! | `LayerSurface` | Wayland layer-shell surface descriptor |

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl ScreenRect {
    /// Expand the rect outward by `border_size` on every side (in logical pixels).
    pub fn expand(self, border_size: u32) -> Self {
        let b = border_size as i32;
        Self {
            x: self.x - b,
            y: self.y - b,
            w: self.w + 2 * b,
            h: self.h + 2 * b,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub rect: ScreenRect,
    pub title: String,
    /// Window class (application ID), e.g. `"kitty"`, `"firefox"`.
    /// Used to match `zwlr_foreign_toplevel_handle_v1` when capturing via v2 protocol.
    pub class: String,
    pub floating: bool,
    /// Lower = more recently focused (0 = topmost floating window). `None`
    /// means the compositor did not provide a focus/stack order.
    pub focus_history_id: Option<i64>,
    /// Stable compositor-specific window identifier. `0` when unavailable.
    pub address: u64,
}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub rect: ScreenRect,
    pub name: String,
    pub focused: bool,
    pub active_workspace_id: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BorderStyle {
    /// Compositor border size in logical pixels.
    pub border_size: u32,
    /// Compositor corner rounding in logical pixels.
    pub rounding: u32,
}

/// A Wayland layer-shell surface at overlay level (level 3).
#[derive(Debug, Clone)]
pub struct LayerSurface {
    pub rect: ScreenRect,
    pub namespace: String,
}
