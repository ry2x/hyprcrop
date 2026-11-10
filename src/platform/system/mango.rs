//! MangoWM IPC backend.
//!
//! Mango exposes compositor state through a small JSON-over-Unix-socket IPC
//! interface.  The socket path is provided by `MANGO_INSTANCE_SIGNATURE`.

use serde::Deserialize;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use crate::domain::error::{AppError, Result};
use crate::domain::types::{BorderStyle, LayerSurface, MonitorInfo, ScreenRect, WindowInfo};

const IPC_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RETRIES: u32 = 3;

#[derive(Debug, Deserialize)]
struct MangoMonitor {
    name: String,
    active: bool,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    #[serde(default)]
    active_tags: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct MangoMonitorsResponse {
    monitors: Vec<MangoMonitor>,
}

#[derive(Debug, Deserialize)]
struct MangoClient {
    id: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    appid: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    #[serde(default)]
    is_floating: bool,
    #[serde(default)]
    is_visible: bool,
    #[serde(default)]
    is_focused: bool,
}

#[derive(Debug, Deserialize)]
struct MangoClientsResponse {
    clients: Vec<MangoClient>,
}

fn socket_path() -> Result<PathBuf> {
    std::env::var_os("MANGO_INSTANCE_SIGNATURE")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            AppError::CompositorEnvVar("MANGO_INSTANCE_SIGNATURE", std::env::VarError::NotPresent)
        })
}

/// Return whether Mango's IPC socket can be reached without issuing a query.
pub fn is_available() -> bool {
    let Ok(path) = socket_path() else {
        return false;
    };
    UnixStream::connect(&path).is_ok()
}

fn ipc_raw(cmd: &str) -> Result<Vec<u8>> {
    let path = socket_path()?;
    let mut last_error: Option<(String, std::io::Error)> = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(100));
        }

        let mut stream = match UnixStream::connect(&path) {
            Ok(stream) => stream,
            Err(error) => {
                last_error = Some(("connecting to socket".to_owned(), error));
                continue;
            }
        };
        let _ = stream.set_read_timeout(Some(IPC_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IPC_TIMEOUT));

        if let Err(error) = writeln!(stream, "{cmd}") {
            last_error = Some(("writing to socket".to_owned(), error));
            continue;
        }

        let mut response = Vec::new();
        match stream.read_to_end(&mut response) {
            Ok(_) => return Ok(response),
            Err(error) => {
                last_error = Some(("reading from socket".to_owned(), error));
            }
        }
    }

    let (context, error) = last_error.unwrap_or_else(|| {
        (
            "unknown socket error".to_owned(),
            std::io::Error::other("IPC loop terminated without errors"),
        )
    });
    Err(AppError::CompositorIpc(
        format!("mango: {cmd}: {context}"),
        error,
    ))
}

fn ipc_json(cmd: &str) -> Result<Value> {
    let response = ipc_raw(cmd)?;
    let value: Value = serde_json::from_slice(&response)
        .map_err(|error| AppError::JsonParse(format!("mango {cmd}"), error))?;

    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(AppError::CompositorProtocol(format!("{cmd}: {error}")));
    }
    Ok(value)
}

fn parse_response<T: for<'de> Deserialize<'de>>(cmd: &str, value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|error| {
        AppError::CompositorProtocol(format!("invalid response for {cmd}: {error}"))
    })
}

pub fn get_monitors() -> Result<Vec<MonitorInfo>> {
    let response: MangoMonitorsResponse =
        parse_response("get all-monitors", ipc_json("get all-monitors")?)?;
    Ok(response
        .monitors
        .into_iter()
        .map(|monitor| MonitorInfo {
            rect: ScreenRect {
                x: monitor.x,
                y: monitor.y,
                w: monitor.width,
                h: monitor.height,
            },
            name: monitor.name,
            focused: monitor.active,
            // Mango uses tags rather than numbered workspaces.  The first active
            // tag is sufficient for the shared field and 0 represents overview.
            active_workspace_id: monitor.active_tags.first().copied().unwrap_or(0),
        })
        .collect())
}

fn parse_client(client: MangoClient, require_visible: bool) -> Option<WindowInfo> {
    if require_visible && !client.is_visible {
        return None;
    }
    if client.width <= 0 || client.height <= 0 {
        return None;
    }

    Some(WindowInfo {
        rect: ScreenRect {
            x: client.x,
            y: client.y,
            w: client.width,
            h: client.height,
        },
        title: client.title,
        class: client.appid,
        floating: client.is_floating,
        // Mango exposes the focused client but does not expose its fstack order
        // through IPC. Do not pretend that all-clients order is focus history.
        focus_history_id: client.is_focused.then_some(0),
        // WindowInfo already has a stable numeric identifier suitable for
        // compositor-specific backends.  It is not used by Mango's fallback
        // screencopy path.
        address: client.id,
    })
}

pub fn get_clients() -> Result<Vec<WindowInfo>> {
    let response: MangoClientsResponse =
        parse_response("get all-clients", ipc_json("get all-clients")?)?;
    Ok(response
        .clients
        .into_iter()
        .filter_map(|client| parse_client(client, true))
        .collect())
}

pub fn get_active_window() -> Result<WindowInfo> {
    let client: MangoClient =
        parse_response("get focusing-client", ipc_json("get focusing-client")?)?;
    parse_client(client, false).ok_or_else(|| {
        AppError::CompositorProtocol("focused Mango client has invalid geometry".to_owned())
    })
}

/// Mango does not currently expose layer-surface geometry through its IPC API.
/// Returning an empty list keeps freeze mode functional; normal window hit
/// testing still uses the compositor's visible clients.
pub fn get_overlay_layers() -> Result<Vec<LayerSurface>> {
    Ok(Vec::new())
}

/// Mango has no Hyprland-compatible border-style query.  Zero is the safe
/// shared default and leaves window geometry unchanged.
pub fn get_border_style() -> BorderStyle {
    BorderStyle::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_monitor_response() {
        let value = serde_json::json!({
            "monitors": [{
                "name": "DP-1",
                "active": true,
                "x": -1920,
                "y": 0,
                "width": 1920,
                "height": 1080,
                "scale": 1.25,
                "active_tags": [2]
            }]
        });
        let response: MangoMonitorsResponse = parse_response("test", value).unwrap();
        assert_eq!(response.monitors[0].name, "DP-1");
        assert_eq!(response.monitors[0].active_tags, vec![2]);
    }

    #[test]
    fn parses_visible_clients_and_focus_order() {
        let value = serde_json::json!({
            "clients": [
                {"id": 7, "title": "focused", "appid": "kitty", "x": 0, "y": 0,
                 "width": 800, "height": 600, "is_visible": true, "is_focused": true},
                {"id": 8, "title": "unfocused", "appid": "firefox", "x": 0, "y": 0,
                 "width": 800, "height": 600, "is_visible": true, "is_focused": false},
                {"id": 9, "title": "hidden", "appid": "foot", "x": 0, "y": 0,
                 "width": 800, "height": 600, "is_visible": false, "is_focused": false}
            ]
        });
        let response: MangoClientsResponse = parse_response("test", value).unwrap();
        let windows: Vec<_> = response
            .clients
            .into_iter()
            .filter_map(|client| parse_client(client, true))
            .collect();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].class, "kitty");
        assert_eq!(windows[0].address, 7);
        assert_eq!(windows[0].focus_history_id, Some(0));
        assert_eq!(windows[1].class, "firefox");
        assert_eq!(windows[1].focus_history_id, None);
    }
}
