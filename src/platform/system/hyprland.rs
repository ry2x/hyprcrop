use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use crate::domain::constants::FREEZE_LAYER_NAMESPACE;
use crate::domain::error::{AppError, Result};
use crate::domain::types::{
    BorderStyle, LayerSurface, MonitorInfo, ScreenRect, WindowInfo, WorkspaceId,
};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HyprMonitor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub name: String,
    pub focused: bool,
    pub active_workspace: WorkspaceId,
    pub special_workspace: Option<WorkspaceId>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HyprClient {
    pub hidden: bool,
    pub workspace: WorkspaceId,
    pub at: [i32; 2],
    pub size: [i32; 2],
    pub title: String,
    pub class: String,
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
    #[serde(default)]
    pub address: String,
}

impl HyprActiveWindow {
    pub(crate) fn find_window<'a>(&self, windows: &'a [WindowInfo]) -> Option<&'a WindowInfo> {
        let active_address = parse_window_address(&self.address).filter(|address| *address != 0);
        windows.iter().find(|window| match active_address {
            Some(address) => window.address == address,
            None => {
                window.rect.x == self.at[0]
                    && window.rect.y == self.at[1]
                    && window.rect.w == self.size[0]
                    && window.rect.h == self.size[1]
            }
        })
    }
}

fn parse_window_address(address: &str) -> Option<u64> {
    u64::from_str_radix(address.trim_start_matches("0x"), 16).ok()
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
    const MAX_RETRIES: u32 = 5;
    const RETRY_DELAY_MS: u64 = 200;
    let mut last_err: Option<(String, std::io::Error)> = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
        }

        let mut stream = match UnixStream::connect(&path) {
            Ok(s) => s,
            Err(e) => {
                last_err = Some(("connecting to socket".into(), e));
                continue;
            }
        };

        if let Err(e) = write!(stream, "j/{}", cmd) {
            last_err = Some(("writing to socket".into(), e));
            continue;
        }

        let mut buf = Vec::new();
        match stream.read_to_end(&mut buf) {
            Ok(_) => return Ok(buf),
            Err(e) => {
                last_err = Some(("reading from socket".into(), e));
                continue;
            }
        }
    }

    let (ctx, err) = last_err.unwrap_or_else(|| {
        (
            "unknown socket error".into(),
            std::io::Error::other("IPC loop terminated without errors"),
        )
    });
    Err(AppError::HyprlandIpc(format!("{}: {}", cmd, ctx), err))
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
                .filter(|s| s.namespace != FREEZE_LAYER_NAMESPACE)
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
            active_workspace: m.active_workspace,
            // Hyprland uses an empty address (or legacy ID 0) for a closed special workspace.
            special_workspace: m.special_workspace.filter(|workspace| match workspace {
                WorkspaceId::Address { address } => !address.is_empty(),
                WorkspaceId::Legacy { id } => *id != 0,
            }),
        })
        .collect()
}

pub(crate) fn parse_windows(clients: Vec<HyprClient>, monitors: &[MonitorInfo]) -> Vec<WindowInfo> {
    clients
        .into_iter()
        .filter_map(|c| {
            if c.hidden {
                return None;
            }
            let on_special_workspace = monitors
                .iter()
                .any(|m| m.special_workspace.as_ref() == Some(&c.workspace));
            if !on_special_workspace && !monitors.iter().any(|m| m.active_workspace == c.workspace) {
                return None;
            }
            let w = c.size[0];
            let h = c.size[1];
            if w <= 0 || h <= 0 {
                return None;
            }
            let address = if c.address.is_empty() {
                0
            } else {
                parse_window_address(&c.address).unwrap_or_else(|| {
                    eprintln!(
                        "[hyprcrop] warning: failed to parse window address '{}' for '{}', defaulting to 0",
                        c.address, c.title
                    );
                    0
                })
            };
            Some(WindowInfo {
                rect: ScreenRect {
                    x: c.at[0],
                    y: c.at[1],
                    w,
                    h,
                },
                title: c.title,
                class: c.class,
                floating: c.floating,
                on_special_workspace,
                focus_history_id: c.focus_history_id,
                address,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_monitor() -> HyprMonitor {
        serde_json::from_str(
            r#"{
                "x": 0, "y": 0, "width": 1920, "height": 1080,
                "name": "DP-1", "focused": true,
                "activeWorkspace": {"id": 1, "name": "1"}
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn test_monitor_parsing() {
        let parsed = parse_monitors(vec![legacy_monitor()]);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "DP-1");
        assert_eq!(parsed[0].rect.w, 1920);
        assert!(parsed[0].focused);
        assert_eq!(parsed[0].active_workspace, WorkspaceId::Legacy { id: 1 });
    }

    #[test]
    fn test_named_workspace_without_id() {
        let monitors = serde_json::from_str(
            r#"[{
                "x": 0, "y": 0, "width": 1920, "height": 1080,
                "name": "DP-1", "focused": true,
                "activeWorkspace": {"address": "game", "type": "normal", "name": "game"}
            }]"#,
        )
        .unwrap();
        let clients = serde_json::from_str(
            r#"[
                {
                    "hidden": false, "at": [0, 0], "size": [800, 600],
                    "title": "Steam", "class": "steam", "floating": false, "focusHistoryID": 0,
                    "workspace": {"address": "game", "type": "normal", "name": "game"}
                },
                {
                    "hidden": false, "at": [0, 0], "size": [800, 600],
                    "title": "Other", "class": "app", "floating": false, "focusHistoryID": 1,
                    "workspace": {"address": "2", "id": 2, "type": "normal", "name": "game"}
                }
            ]"#,
        )
        .unwrap();

        let windows = parse_windows(clients, &parse_monitors(monitors));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].class, "steam");
    }

    #[test]
    fn test_windows_on_special_workspaces() {
        use serde_json::json;

        let modern_special =
            json!({"address": "special:tools", "type": "special", "name": "special:tools"});
        let legacy_special = json!({"id": -98, "name": "special:tools"});
        for (active, shown, closed) in [
            (
                json!({"address": "1", "id": 1, "type": "normal", "name": "1"}),
                modern_special,
                json!({"address": "", "type": "", "name": ""}),
            ),
            (
                json!({"id": 1, "name": "1"}),
                legacy_special,
                json!({"id": 0, "name": ""}),
            ),
        ] {
            let client_payload: Vec<_> = [&active, &shown, &closed]
                .into_iter()
                .enumerate()
                .map(|(i, workspace)| {
                    json!({
                        "hidden": false, "at": [0, 0], "size": [800, 600],
                        "title": i.to_string(), "class": "app", "floating": false,
                        "focusHistoryID": i, "workspace": workspace,
                        "address": format!("0x{:x}", i + 1),
                    })
                })
                .collect();

            for (special, expected) in [(shown, vec![false, true]), (closed, vec![false])] {
                let monitor = serde_json::from_value(json!({
                    "x": 0, "y": 0, "width": 1920, "height": 1080,
                    "name": "DP-1", "focused": true,
                    "activeWorkspace": active, "specialWorkspace": special,
                }))
                .unwrap();
                let clients = serde_json::from_value(json!(client_payload)).unwrap();
                let windows = parse_windows(clients, &parse_monitors(vec![monitor]));
                let on_special: Vec<_> = windows.iter().map(|w| w.on_special_workspace).collect();
                assert_eq!(on_special, expected);

                // The regular and special windows have identical geometry.
                let active: HyprActiveWindow = serde_json::from_value(json!({
                    "at": [0, 0], "size": [800, 600], "address": "0x2",
                }))
                .unwrap();
                let address = active.find_window(&windows).map(|window| window.address);
                assert_eq!(address, expected.contains(&true).then_some(2));
            }
        }
    }

    #[test]
    fn test_overlay_layer_parsing() {
        // Representative `hyprctl -j layers` payload with multiple monitors and levels.
        let json = r#"{
            "DP-2": {
                "levels": {
                    "0": [{"x":0,"y":1050,"w":1920,"h":30,"namespace":"waybar-bottom"}],
                    "3": [
                        {"x":0,"y":0,"w":1920,"h":30,"namespace":"waybar"}
                    ]
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
    fn test_overlay_layer_parsing_with_freeze_layer() {
        let json = format!(
            r#"{{
            "DP-1": {{
                "levels": {{
                    "3": [
                        {{"x":0,"y":0,"w":1920,"h":40,"namespace":"waybar"}},
                        {{"x":0,"y":40,"w":1920,"h":40,"namespace":"{}"}}
                    ]
                }}
            }}
        }}"#,
            FREEZE_LAYER_NAMESPACE
        );
        let monitors: HashMap<String, HyprLayerMonitor> = serde_json::from_str(&json).unwrap();
        let surfaces = parse_overlay_layers(monitors);

        // hyprcrop-freeze surface must be filtered out
        assert_eq!(surfaces.len(), 1);
        assert_eq!(surfaces[0].namespace, "waybar");
        assert_eq!(surfaces[0].rect.x, 0);
        assert_eq!(surfaces[0].rect.w, 1920);
    }

    #[test]
    fn test_window_parsing() {
        let clients = vec![
            HyprClient {
                hidden: false,
                workspace: WorkspaceId::Legacy { id: 1 },
                at: [100, 100],
                size: [800, 600],
                title: "Visible Window".to_string(),
                class: "kitty".to_string(),
                floating: false,
                focus_history_id: 1,
                address: "0xd161e7b0".to_string(),
            },
            HyprClient {
                hidden: true,
                workspace: WorkspaceId::Legacy { id: 1 },
                at: [200, 200],
                size: [800, 600],
                title: "Hidden Window".to_string(),
                class: "kitty".to_string(),
                floating: false,
                focus_history_id: 2,
                address: "0xd161e7c0".to_string(),
            },
            HyprClient {
                hidden: false,
                workspace: WorkspaceId::Legacy { id: 2 },
                at: [300, 300],
                size: [800, 600],
                title: "Other Workspace Window".to_string(),
                class: "firefox".to_string(),
                floating: false,
                focus_history_id: 3,
                address: "0xd161e7d0".to_string(),
            },
        ];

        let parsed = parse_windows(clients, &parse_monitors(vec![legacy_monitor()]));

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Visible Window");
        assert_eq!(parsed[0].class, "kitty");
        assert_eq!(parsed[0].rect.x, 100);
        assert_eq!(parsed[0].rect.w, 800);
        assert_eq!(parsed[0].address, 0xd161e7b0_u64);
    }

    #[test]
    fn test_window_address_parsing() {
        let clients = vec![
            HyprClient {
                hidden: false,
                workspace: WorkspaceId::Legacy { id: 1 },
                at: [0, 0],
                size: [100, 100],
                title: "Win".to_string(),
                class: "app".to_string(),
                floating: false,
                focus_history_id: 0,
                address: "0xdeadbeef".to_string(),
            },
            HyprClient {
                hidden: false,
                workspace: WorkspaceId::Legacy { id: 1 },
                at: [0, 0],
                size: [100, 100],
                title: "BadAddr".to_string(),
                class: "app".to_string(),
                floating: false,
                focus_history_id: 1,
                address: "not_a_hex".to_string(),
            },
        ];
        let parsed = parse_windows(clients, &parse_monitors(vec![legacy_monitor()]));
        // Valid address is parsed correctly.
        assert_eq!(parsed[0].address, 0xdeadbeef_u64);
        // Unparseable address falls back to 0; window is still included (not dropped).
        // capture_toplevel_to_path will reject address=0 with HyprlandProtocol error
        // if the toplevel-export path is actually invoked.
        assert_eq!(parsed[1].address, 0_u64);
    }
}
