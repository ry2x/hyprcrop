//! Compositor selection and the shared compositor backend interface.
//!
//! Capture and freeze-mode code depends on this interface instead of knowing
//! whether state came from Hyprland IPC or MangoWM IPC.

use crate::domain::error::{AppError, Result};
use crate::domain::types::{BorderStyle, LayerSurface, MonitorInfo, WindowInfo};

use super::{hyprland, mango};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositorKind {
    Hyprland,
    Mango,
}

pub trait CompositorBackend: Copy + Send + 'static {
    fn kind(self) -> CompositorKind;
    fn monitors(self) -> Result<Vec<MonitorInfo>>;
    fn active_window(self) -> Result<WindowInfo>;
    fn windows(self) -> Result<Vec<WindowInfo>>;
    fn overlay_layers(self) -> Result<Vec<LayerSurface>>;
    fn border_style(self) -> BorderStyle;
}

#[derive(Debug, Clone, Copy)]
pub enum Backend {
    Hyprland,
    Mango,
}

impl CompositorBackend for Backend {
    fn kind(self) -> CompositorKind {
        match self {
            Self::Hyprland => CompositorKind::Hyprland,
            Self::Mango => CompositorKind::Mango,
        }
    }

    fn monitors(self) -> Result<Vec<MonitorInfo>> {
        match self {
            Self::Hyprland => Ok(hyprland::parse_monitors(hyprland::get_monitors()?)),
            Self::Mango => mango::get_monitors(),
        }
    }

    fn active_window(self) -> Result<WindowInfo> {
        match self {
            Self::Hyprland => {
                let active = hyprland::get_active_window()?;
                if active.size[0] <= 0 || active.size[1] <= 0 {
                    return Err(AppError::CompositorProtocol(
                        "Hyprland focused window has invalid geometry".to_owned(),
                    ));
                }
                Ok(WindowInfo {
                    rect: crate::domain::types::ScreenRect {
                        x: active.at[0],
                        y: active.at[1],
                        w: active.size[0],
                        h: active.size[1],
                    },
                    title: String::new(),
                    class: String::new(),
                    floating: false,
                    focus_history_id: Some(0),
                    address: 0,
                })
            }
            Self::Mango => mango::get_active_window(),
        }
    }

    fn windows(self) -> Result<Vec<WindowInfo>> {
        match self {
            Self::Hyprland => {
                let monitors = hyprland::parse_monitors(hyprland::get_monitors()?);
                let active_workspace_ids: Vec<i64> =
                    monitors.iter().map(|m| m.active_workspace_id).collect();
                Ok(hyprland::parse_windows(
                    hyprland::get_clients()?,
                    &active_workspace_ids,
                ))
            }
            Self::Mango => mango::get_clients(),
        }
    }

    fn overlay_layers(self) -> Result<Vec<LayerSurface>> {
        match self {
            Self::Hyprland => hyprland::get_overlay_layers(),
            Self::Mango => mango::get_overlay_layers(),
        }
    }

    fn border_style(self) -> BorderStyle {
        match self {
            Self::Hyprland => hyprland::get_border_style(),
            Self::Mango => mango::get_border_style(),
        }
    }
}

/// Select the compositor for the current session.
///
/// `HYPRCROP_COMPOSITOR=hyprland|mango` is useful for nested sessions and
/// debugging.  Automatic detection requires both the compositor environment
/// variable and a reachable IPC socket, avoiding stale environment variables.
pub fn detect() -> Result<Backend> {
    if let Ok(requested) = std::env::var("HYPRCROP_COMPOSITOR") {
        let requested = requested.to_ascii_lowercase();
        let backend = match requested.as_str() {
            "hyprland" | "hypr" => Backend::Hyprland,
            "mango" | "mangowm" => Backend::Mango,
            other => {
                return Err(AppError::UnsupportedCompositor(format!(
                    "unknown HYPRCROP_COMPOSITOR value '{other}' (use hyprland or mango)"
                )));
            }
        };
        if is_available(backend) {
            return Ok(backend);
        }
        return Err(AppError::UnsupportedCompositor(format!(
            "requested compositor '{requested}' is not reachable"
        )));
    }

    let hyprland_available = hyprland::is_available();
    let mango_available = mango::is_available();

    match (hyprland_available, mango_available) {
        (true, false) => Ok(Backend::Hyprland),
        (false, true) => Ok(Backend::Mango),
        (true, true) => Err(AppError::UnsupportedCompositor(
            "both Hyprland and MangoWM IPC sockets are reachable; set HYPRCROP_COMPOSITOR to choose one"
                .to_owned(),
        )),
        (false, false) => Err(AppError::UnsupportedCompositor(
            "no supported compositor detected (expected Hyprland or MangoWM)".to_owned(),
        )),
    }
}

fn is_available(backend: Backend) -> bool {
    match backend {
        Backend::Hyprland => hyprland::is_available(),
        Backend::Mango => mango::is_available(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kinds_are_distinct() {
        assert_ne!(Backend::Hyprland.kind(), Backend::Mango.kind());
    }
}
