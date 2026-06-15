use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::domain::config::Config;
use crate::domain::error::{AppError, Result};
use crate::domain::geometry::{
    clamp_crop, logical_to_physical, monitor_origin, parse_slurp_geometry,
};
use crate::domain::types::ScreenRect;
use crate::platform::capture::{screencopy, toplevel_export};
use crate::platform::system::cmd::CMD_SLURP;
use crate::platform::system::hyprland;

fn slurp_region() -> Result<String> {
    let output = Command::new(CMD_SLURP)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| AppError::CommandNotFound(CMD_SLURP.to_string(), e))?;

    if !output.status.success() {
        if output.status.code() == Some(1) {
            return Err(AppError::UserCancelled);
        }
        return Err(AppError::CommandFailed(
            CMD_SLURP.to_string(),
            output.status,
        ));
    }

    let region = String::from_utf8(output.stdout)
        .map_err(|_| AppError::Other("slurp output is not valid UTF-8".to_string()))?
        .trim()
        .to_owned();

    if region.is_empty() {
        return Err(AppError::EmptyGeometry);
    }

    Ok(region)
}

pub fn capture_crop(cfg: &Config) -> Result<PathBuf> {
    // Fetch monitor layout before blocking on slurp so the layout snapshot used to
    // interpret slurp's logical coordinates stays stable while the user selects.
    let monitors = hyprland::parse_monitors(hyprland::get_monitors()?);
    let region = slurp_region()?;
    let (slurp_x, slurp_y, req_w, req_h) = parse_slurp_geometry(&region)?;

    // capture_all_monitors places (min_x, min_y) at image pixel (0, 0).
    // Slurp returns global logical coordinates, so we must subtract the origin
    // to get image-space coordinates. This matters for multi-monitor layouts
    // where some monitors sit at negative logical positions.
    let (min_x, min_y) = monitor_origin(&monitors);
    let x = (slurp_x - min_x).max(0) as u32;
    let y = (slurp_y - min_y).max(0) as u32;
    if slurp_x < min_x || slurp_y < min_y {
        eprintln!(
            "warning: crop origin ({slurp_x},{slurp_y}) is before monitor origin ({min_x},{min_y}), clamped to ({x},{y})"
        );
    }

    let full_img = screencopy::capture_all_monitors(&monitors)?;

    let (w, h, was_clamped) = clamp_crop(x, y, req_w, req_h, full_img.width(), full_img.height());
    if was_clamped {
        eprintln!(
            "warning: crop region ({slurp_x},{slurp_y} {req_w}x{req_h}) exceeds image bounds ({}x{}), clamped to {w}x{h}",
            full_img.width(),
            full_img.height(),
        );
    }
    if w == 0 || h == 0 {
        return Err(AppError::Other(format!(
            "Crop region ({slurp_x},{slurp_y} {req_w}x{req_h}) is entirely outside the image bounds ({}x{})",
            full_img.width(),
            full_img.height(),
        )));
    }

    let cropped = ::image::imageops::crop_imm(&full_img, x, y, w, h).to_image();
    let path = cfg.output_path();
    cropped.save(&path).map_err(AppError::from)?;
    Ok(path)
}

pub fn capture_window(cfg: &Config) -> Result<PathBuf> {
    let active = hyprland::get_active_window()?;

    // Try toplevel_export first — captures the window buffer directly from the
    // compositor, so overlapping windows are NOT included in the result.
    let monitors = hyprland::parse_monitors(hyprland::get_monitors()?);
    let active_workspace_ids: Vec<i64> = monitors.iter().map(|m| m.active_workspace_id).collect();
    let clients = hyprland::get_clients()?;
    let windows = hyprland::parse_windows(clients, &active_workspace_ids);

    if let Some(win) = windows.iter().find(|w| {
        w.rect.x == active.at[0]
            && w.rect.y == active.at[1]
            && w.rect.w == active.size[0]
            && w.rect.h == active.size[1]
    }) {
        let path = cfg.output_path();
        match toplevel_export::capture_toplevel_to_path(win, &path) {
            Ok(()) => return Ok(path),
            Err(e) => {
                eprintln!(
                    "[hyprcrop] toplevel export failed ({}), falling back to screencopy",
                    e
                );
            }
        }
    }

    // Fallback: screencopy + crop (includes overlapping windows).
    let border_size = if cfg.capture_window_border {
        hyprland::get_border_style().border_size
    } else {
        0
    };

    let win_rect = ScreenRect {
        x: active.at[0],
        y: active.at[1],
        w: active.size[0],
        h: active.size[1],
    }
    .expand(border_size);

    let win_x = win_rect.x;
    let win_y = win_rect.y;
    let win_w = win_rect.w.max(0) as u32;
    let win_h = win_rect.h.max(0) as u32;

    // Identify the monitor that contains the window's top-left corner.
    // Windows spanning multiple monitors are captured from the monitor containing their top-left corner only.
    let mon = monitors
        .iter()
        .find(|m| {
            win_x >= m.rect.x
                && win_y >= m.rect.y
                && win_x < m.rect.x + m.rect.w
                && win_y < m.rect.y + m.rect.h
        })
        .ok_or_else(|| AppError::Other("Could not find monitor for active window".to_string()))?;

    let mon_img = screencopy::capture_monitor(&mon.name)?;

    // Derive scale from actual frame dimensions (handles HiDPI without a separate field).
    if mon.rect.w <= 0 || mon.rect.h <= 0 {
        return Err(AppError::Other(format!(
            "Monitor '{}' has invalid dimensions ({}x{}) in Hyprland IPC data",
            mon.name, mon.rect.w, mon.rect.h
        )));
    }
    let scale_x = f64::from(mon_img.width()) / f64::from(mon.rect.w);
    let scale_y = f64::from(mon_img.height()) / f64::from(mon.rect.h);

    // Window position relative to monitor top-left, clamped to non-negative.
    let rel_x = (win_x - mon.rect.x).max(0) as u32;
    let rel_y = (win_y - mon.rect.y).max(0) as u32;

    // Convert logical → physical pixels, then clamp to frame bounds.
    let phys_x = logical_to_physical(rel_x, scale_x);
    let phys_y = logical_to_physical(rel_y, scale_y);
    let phys_w = logical_to_physical(win_w, scale_x).min(mon_img.width().saturating_sub(phys_x));
    let phys_h = logical_to_physical(win_h, scale_y).min(mon_img.height().saturating_sub(phys_y));

    if phys_w == 0 || phys_h == 0 {
        return Err(AppError::Other(
            "Window crop region is entirely outside the monitor image bounds".to_string(),
        ));
    }

    let cropped = ::image::imageops::crop_imm(&mon_img, phys_x, phys_y, phys_w, phys_h).to_image();
    let path = cfg.output_path();
    cropped.save(&path).map_err(AppError::from)?;
    Ok(path)
}

pub fn capture_monitor(cfg: &Config) -> Result<PathBuf> {
    let monitors = hyprland::get_monitors()?;
    let focused = monitors
        .into_iter()
        .find(|m| m.focused)
        .ok_or(AppError::NoFocusedMonitor)?;

    let img = screencopy::capture_monitor(&focused.name)?;
    let path = cfg.output_path();
    img.save(&path).map_err(AppError::from)?;
    Ok(path)
}

pub fn capture_portal(cfg: &Config) -> Result<PathBuf> {
    crate::platform::capture::portal::capture(cfg)
}

pub fn capture_all(cfg: &Config) -> Result<PathBuf> {
    let monitors = hyprland::parse_monitors(hyprland::get_monitors()?);
    let img = screencopy::capture_all_monitors(&monitors)?;

    let path = cfg.output_path();
    img.save(&path).map_err(AppError::from)?;
    Ok(path)
}
