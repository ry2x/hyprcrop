use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use crate::domain::error::{AppError, Result};
use crate::domain::types::{BorderStyle, LayerSurface, MonitorInfo, ScreenRect, WindowInfo};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HyprMonitor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub name: String,
    pub focused: bool,
    pub active_workspace: HyprWorkspace,
}

#[derive(Deserialize, Debug)]
pub struct HyprWorkspace {
    pub id: i64,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HyprClient {
    pub hidden: bool,
    pub workspace: HyprWorkspace,
    pub at: [i32; 2],
    pub size: [i32; 2],
    pub title: String,
    pub floating: bool,
    #[serde(rename = "focusHistoryID")]
    pub focus_history_id: i64,
    /// Hex address string like `"0x..."`, used as the toplevel export handle.
    #[serde(default)]
    pub address: String,
}

#[derive(Deserialize, Debug)]
pub struct HyprActiveWindow {
    pub at: [i32; 2],
    pub size: [i32; 2],
}

fn hyprland_socket_path() -> Result<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|e| AppError::HyprlandEnvVar("HYPRLAND_INSTANCE_SIGNATURE", e))?;
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(runtime)
            .join("hypr")
            .join(&sig)
            .join(".socket.sock");
        if p.exists() {
            return Ok(p);
        }
    }
    Ok(PathBuf::from(format!("/tmp/hypr/{}/.socket.sock", sig)))
}

pub fn hyprland_ipc_raw(cmd: &str) -> Result<Vec<u8>> {
    let path = hyprland_socket_path()?;
    let mut stream =
        UnixStream::connect(&path).map_err(|e| AppError::HyprlandIpc(cmd.to_string(), e))?;
    write!(stream, "j/{}", cmd).map_err(|e| AppError::HyprlandIpc(cmd.to_string(), e))?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|e| AppError::HyprlandIpc(cmd.to_string(), e))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|e| AppError::HyprlandIpc(cmd.to_string(), e))?;
    Ok(buf)
}

pub fn hyprland_ipc<T: for<'de> Deserialize<'de>>(cmd: &str) -> Result<T> {
    let buf = hyprland_ipc_raw(cmd)?;
    let parsed: T =
        serde_json::from_slice(&buf).map_err(|e| AppError::JsonParse(cmd.to_string(), e))?;
    Ok(parsed)
}

#[derive(Deserialize, Debug)]
pub struct HyprOption {
    pub int: i64,
}

/// Fetch `general:border_size` and `decoration:rounding` from Hyprland IPC.
/// Falls back to `BorderStyle::default()` on any error.
pub fn get_border_style() -> BorderStyle {
    let bs = hyprland_ipc::<HyprOption>("getoption general:border_size")
        .map(|o| o.int.max(0) as u32)
        .unwrap_or(0);
    let rd = hyprland_ipc::<HyprOption>("getoption decoration:rounding")
        .map(|o| o.int.max(0) as u32)
        .unwrap_or(0);
    BorderStyle {
        border_size: bs,
        rounding: rd,
    }
}

pub fn get_active_window() -> Result<HyprActiveWindow> {
    hyprland_ipc("activewindow")
}

pub fn get_monitors() -> Result<Vec<HyprMonitor>> {
    hyprland_ipc("monitors")
}

pub fn get_clients() -> Result<Vec<HyprClient>> {
    hyprland_ipc("clients")
}

// ── Layer-shell surface types ─────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
struct HyprLayerSurface {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    namespace: String,
}

#[derive(Deserialize, Debug)]
pub(crate) struct HyprLayerMonitor {
    levels: HashMap<String, Vec<HyprLayerSurface>>,
}

/// Hyprland layer level for overlay surfaces (above all windows).
const OVERLAY_LEVEL: &str = "3";

/// Parse overlay-level surfaces from a `hyprctl -j layers` response.
///
/// Surfaces are returned sorted by (x, y, namespace) for deterministic ordering,
/// since `HashMap` iteration order is nondeterministic.
pub(crate) fn parse_overlay_layers(
    monitors: HashMap<String, HyprLayerMonitor>,
) -> Vec<LayerSurface> {
    let mut surfaces: Vec<LayerSurface> = monitors
        .into_values()
        .flat_map(|mon| {
            mon.levels
                .into_iter()
                .filter(|(level, _)| level == OVERLAY_LEVEL)
                .flat_map(|(_, surfaces)| surfaces)
                .map(|s| LayerSurface {
                    rect: ScreenRect {
                        x: s.x,
                        y: s.y,
                        w: s.w,
                        h: s.h,
                    },
                    namespace: s.namespace,
                })
        })
        .filter(|s| s.rect.w > 0 && s.rect.h > 0)
        .collect();
    surfaces.sort_by(|a, b| {
        a.rect
            .x
            .cmp(&b.rect.x)
            .then(a.rect.y.cmp(&b.rect.y))
            .then(a.namespace.cmp(&b.namespace))
    });
    surfaces
}

/// Fetch all overlay-level (level 3) layer surfaces from Hyprland IPC.
pub fn get_overlay_layers() -> Result<Vec<LayerSurface>> {
    let monitors: HashMap<String, HyprLayerMonitor> = hyprland_ipc("layers")?;
    Ok(parse_overlay_layers(monitors))
}

pub fn parse_monitors(monitors: Vec<HyprMonitor>) -> Vec<MonitorInfo> {
    monitors
        .into_iter()
        .map(|m| MonitorInfo {
            rect: ScreenRect {
                x: m.x,
                y: m.y,
                w: m.width,
                h: m.height,
            },
            name: m.name,
            focused: m.focused,
            active_workspace_id: m.active_workspace.id,
        })
        .collect()
}

pub(crate) fn parse_windows(
    clients: Vec<HyprClient>,
    active_workspace_ids: &[i64],
) -> Result<Vec<WindowInfo>> {
    clients
        .into_iter()
        .filter(|c| !c.hidden && active_workspace_ids.contains(&c.workspace.id))
        .filter(|c| c.size[0] > 0 && c.size[1] > 0)
        .map(|c| {
            let address =
                u64::from_str_radix(c.address.trim_start_matches("0x"), 16).map_err(|_| {
                    AppError::HyprlandProtocol(format!(
                        "window '{}' has unparseable address field: '{}'",
                        c.title, c.address
                    ))
                })?;
            Ok(WindowInfo {
                rect: ScreenRect {
                    x: c.at[0],
                    y: c.at[1],
                    w: c.size[0],
                    h: c.size[1],
                },
                title: c.title,
                floating: c.floating,
                focus_history_id: c.focus_history_id,
                address,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitor_parsing() {
        let monitors = vec![HyprMonitor {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            name: "DP-1".to_string(),
            focused: true,
            active_workspace: HyprWorkspace { id: 1 },
        }];

        let parsed = parse_monitors(monitors);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "DP-1");
        assert_eq!(parsed[0].rect.w, 1920);
        assert!(parsed[0].focused);
        assert_eq!(parsed[0].active_workspace_id, 1);
    }

    #[test]
    fn test_overlay_layer_parsing() {
        // Representative `hyprctl -j layers` payload with multiple monitors and levels.
        let json = r#"{
            "DP-2": {
                "levels": {
                    "0": [{"x":0,"y":1050,"w":1920,"h":30,"namespace":"waybar-bottom"}],
                    "3": [{"x":0,"y":0,"w":1920,"h":30,"namespace":"waybar"}]
                }
            },
            "DP-1": {
                "levels": {
                    "3": [
                        {"x":1920,"y":0,"w":2560,"h":40,"namespace":"waybar"},
                        {"x":0,"y":0,"w":0,"h":0,"namespace":"zero-size"}
                    ]
                }
            }
        }"#;
        let monitors: HashMap<String, HyprLayerMonitor> = serde_json::from_str(json).unwrap();
        let surfaces = parse_overlay_layers(monitors);

        // zero-size surface and level-0 surface must be filtered out
        assert_eq!(surfaces.len(), 2);
        // sorted by (x, y, namespace): x=0 first, then x=1920
        assert_eq!(surfaces[0].namespace, "waybar");
        assert_eq!(surfaces[0].rect.x, 0);
        assert_eq!(surfaces[0].rect.w, 1920);
        assert_eq!(surfaces[1].namespace, "waybar");
        assert_eq!(surfaces[1].rect.x, 1920);
        assert_eq!(surfaces[1].rect.w, 2560);
    }

    #[test]
    fn test_window_parsing() {
        let clients = vec![
            HyprClient {
                hidden: false,
                workspace: HyprWorkspace { id: 1 },
                at: [100, 100],
                size: [800, 600],
                title: "Visible Window".to_string(),
                floating: false,
                focus_history_id: 1,
                address: "0xd161e7b0".to_string(),
            },
            HyprClient {
                hidden: true,
                workspace: HyprWorkspace { id: 1 },
                at: [200, 200],
                size: [800, 600],
                title: "Hidden Window".to_string(),
                floating: false,
                focus_history_id: 2,
                address: "0xd161e7c0".to_string(),
            },
            HyprClient {
                hidden: false,
                workspace: HyprWorkspace { id: 2 },
                at: [300, 300],
                size: [800, 600],
                title: "Other Workspace Window".to_string(),
                floating: false,
                focus_history_id: 3,
                address: "0xd161e7d0".to_string(),
            },
        ];

        let active_workspaces = vec![1];
        let parsed = parse_windows(clients, &active_workspaces).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Visible Window");
        assert_eq!(parsed[0].rect.x, 100);
        assert_eq!(parsed[0].rect.w, 800);
        assert_eq!(parsed[0].address, 0xd161e7b0_u64);
    }

    #[test]
    fn test_window_address_parsing() {
        let clients = vec![
            HyprClient {
                hidden: false,
                workspace: HyprWorkspace { id: 1 },
                at: [0, 0],
                size: [100, 100],
                title: "Win".to_string(),
                floating: false,
                focus_history_id: 0,
                address: "0xdeadbeef".to_string(),
            },
            HyprClient {
                hidden: false,
                workspace: HyprWorkspace { id: 1 },
                at: [0, 0],
                size: [100, 100],
                title: "BadAddr".to_string(),
                floating: false,
                focus_history_id: 1,
                address: "not_a_hex".to_string(),
            },
        ];
        // Valid address parses; bad address is a protocol-level error.
        let result = parse_windows(clients, &[1]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("BadAddr"));
        assert!(msg.contains("not_a_hex"));
    }
}
