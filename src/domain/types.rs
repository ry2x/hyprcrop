//! # domain::types
//!
//! Shared data structures used across multiple modules.
//! External dependencies are minimal — `serde` only.
//!
//! | Type | Purpose |
//! |---|---|
//! | `ScreenRect` | Rectangular region in logical pixel coordinates |
//! | `WindowInfo` | Hyprland window metadata |
//! | `MonitorInfo` | Hyprland monitor metadata |
//! | `BorderStyle` | Hyprland `border_size` and `rounding` values |
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
    pub floating: bool,
    /// Lower = more recently focused (0 = topmost floating window).
    pub focus_history_id: i64,
    /// Hyprland window address (from `hyprctl clients`), used as handle for
    /// `hyprland-toplevel-export-v1`. Zero if the address could not be parsed.
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
    /// Hyprland `general:border_size` in logical pixels.
    pub border_size: u32,
    /// Hyprland `decoration:rounding` in logical pixels.
    pub rounding: u32,
}

/// A Wayland layer-shell surface at overlay level (level 3).
#[derive(Debug, Clone)]
pub struct LayerSurface {
    pub rect: ScreenRect,
    pub namespace: String,
}
