//! # ui::freeze
//!
//! Entry point for freeze mode.
//! Captures all monitors, displays the result as a full-screen overlay,
//! waits for the user to select a capture region, then crops the frozen
//! image and saves the final output.
//!
//! The `app` submodule implements the iced state machine (`AppState`).
//! This module orchestrates initialization, execution, and post-processing.

mod app;

pub use app::CaptureMode;

use app::{
    AppState, AppStateConfig, FreezeSelection, Message, app_subscription, app_update, app_view,
};
use iced::Task;
use iced::widget::image as iced_image;
use iced_layershell::{
    reexport::{Anchor, IcedId, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption},
    settings::LayerShellSettings,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::domain::config::Config;
use crate::domain::error::{AppError, Result};
use crate::domain::types::{BorderStyle, ScreenRect};
use crate::platform::capture::screencopy;
use crate::platform::capture::toplevel_export;
use crate::platform::system::hyprland::{self};

pub fn run_freeze(cfg: &Config) -> Result<PathBuf> {
    let monitors_t = std::thread::spawn(hyprland::get_monitors);
    let clients_t = std::thread::spawn(hyprland::get_clients);
    let layers_t = std::thread::spawn(hyprland::get_overlay_layers);
    // When toplevel export is ON, border style is irrelevant (the protocol captures
    // the raw surface without decorations). The config already enforces
    // capture_window_border = false in that case, so this check is consistent.
    let border_style = if cfg.capture_window_border {
        hyprland::get_border_style()
    } else {
        BorderStyle::default()
    };
    let initial_mode =
        resolve_initial_mode(&cfg.freeze_buttons, crate::domain::state::load_last_mode());

    let monitors_raw = monitors_t
        .join()
        .map_err(|_| AppError::Other("monitors thread panicked".into()))??;
    let clients_raw = clients_t
        .join()
        .map_err(|_| AppError::Other("clients thread panicked".into()))??;
    let layers = layers_t
        .join()
        .unwrap_or_else(|_| {
            eprintln!("[hyprcrop] warning: overlay layers thread panicked");
            Ok(Vec::new())
        })
        .unwrap_or_else(|e| {
            eprintln!("[hyprcrop] warning: failed to fetch overlay layers: {e}");
            Vec::new()
        });

    let monitors = hyprland::parse_monitors(monitors_raw);
    let active_ws_ids: Vec<i64> = monitors.iter().map(|m| m.active_workspace_id).collect();
    let windows = hyprland::parse_windows(clients_raw, &active_ws_ids);

    // Compute origin before monitors are moved into Arc.
    // capture_all_monitors places (min_x, min_y) at image pixel (0,0); we need this
    // to translate the UI's global logical coordinates into image coordinates later.
    let (min_x, min_y) = crate::domain::geometry::monitor_origin(&monitors);

    // Capture all monitors in a single Wayland session.
    // Using one session guarantees the overlay images and the final-crop source are
    // from the same frame — two separate captures would differ in time, breaking
    // the "freeze" guarantee (user selects based on a different frame than what gets saved).
    let (physical_per_monitor, full_rgba) =
        screencopy::capture_all_monitors_with_physical(&monitors)?;

    let monitor_images: Vec<iced_image::Handle> = physical_per_monitor
        .into_iter()
        .map(|img| iced_image::Handle::from_rgba(img.width(), img.height(), img.into_raw()))
        .collect();

    let focused_monitor_idx = monitors.iter().position(|m| m.focused).unwrap_or(0);

    let (fw, fh) = {
        let m = &monitors[focused_monitor_idx];
        (m.rect.w as u32, m.rect.h as u32)
    };

    let result: Arc<Mutex<Option<Option<FreezeSelection>>>> = Arc::new(Mutex::new(None));

    {
        let result_clone = result.clone();
        let mut window_to_monitor: HashMap<IcedId, usize> = HashMap::new();

        let extra_specs: Vec<(IcedId, NewLayerShellSettings)> = monitors
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.focused)
            .map(|(idx, m)| {
                let id = IcedId::unique();
                window_to_monitor.insert(id, idx);
                let settings = NewLayerShellSettings {
                    size: Some((m.rect.w as u32, m.rect.h as u32)),
                    layer: Layer::Overlay,
                    anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
                    exclusive_zone: Some(-1),
                    keyboard_interactivity: KeyboardInteractivity::Exclusive,
                    output_option: OutputOption::OutputName(m.name.clone()),
                    namespace: Some("hyprcrop-freeze".to_string()),
                    ..Default::default()
                };
                (id, settings)
            })
            .collect();

        let extra_specs = std::sync::Arc::new(extra_specs);

        let wins = Arc::new(windows);
        let mons = Arc::new(monitors);
        let lyrs = Arc::new(layers);
        let glyphs = cfg.freeze_glyphs.clone();
        let toolbar_position = cfg.toolbar_position;
        let colors = cfg.freeze_colors;
        let freeze_buttons = cfg.freeze_buttons;
        let use_toplevel_export = cfg.freeze_window_use_toplevel_export;

        iced_layershell::daemon(
            move || {
                let spawn_tasks: Vec<Task<Message>> = extra_specs
                    .iter()
                    .map(|(id, settings)| {
                        Task::done(Message::NewLayerShell {
                            settings: settings.clone(),
                            id: *id,
                        })
                    })
                    .collect();

                let state = AppState::new(AppStateConfig {
                    monitor_images: monitor_images.clone(),
                    focused_monitor_idx,
                    window_to_monitor: window_to_monitor.clone(),
                    windows: Arc::clone(&wins),
                    monitors: Arc::clone(&mons),
                    layers: Arc::clone(&lyrs),
                    result: result_clone.clone(),
                    glyphs: glyphs.clone(),
                    toolbar_position,
                    border_style,
                    initial_mode,
                    colors,
                    freeze_buttons,
                    use_toplevel_export,
                });
                (state, Task::batch(spawn_tasks))
            },
            "hyprcrop-freeze",
            app_update,
            app_view,
        )
        .subscription(app_subscription)
        .layer_settings(LayerShellSettings {
            layer: Layer::Overlay,
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            exclusive_zone: -1,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            size: Some((fw, fh)),
            ..Default::default()
        })
        .run()
        .map_err(|e| AppError::LayerShell(e.to_string()))?;
    }

    let selected = result
        .lock()
        .expect("UI thread panicked and poisoned result mutex")
        .take();

    let out_path = cfg.output_path();

    match selected {
        None => Err(AppError::UserCancelled),
        Some(None) => {
            // "All" mode: save the full frozen image.
            screencopy::crop_and_save(full_rgba, None, &out_path)?;
            Ok(out_path)
        }
        Some(Some(FreezeSelection::Region(r))) => {
            // Translate global logical coordinates → image coordinates.
            // The image origin is (min_x, min_y) in global logical space.
            let adjusted = ScreenRect {
                x: r.x - min_x,
                y: r.y - min_y,
                w: r.w,
                h: r.h,
            };
            screencopy::crop_and_save(full_rgba, Some(adjusted), &out_path)?;
            Ok(out_path)
        }
        Some(Some(FreezeSelection::ToplevelWindow(addr))) => {
            toplevel_export::capture_toplevel_to_path(addr, &out_path)?;
            Ok(out_path)
        }
    }
}

fn resolve_initial_mode(
    buttons: &crate::domain::config::FreezeButtons,
    saved: CaptureMode,
) -> CaptureMode {
    // Saved mode may reference a button that has since been disabled.
    // Fall back to the first enabled mode so the UI starts in a valid state.
    let saved_enabled = match saved {
        CaptureMode::Crop => buttons.crop,
        CaptureMode::Window => buttons.window,
        CaptureMode::Monitor => buttons.monitor,
        CaptureMode::All => buttons.all,
    };
    if saved_enabled {
        saved
    } else if buttons.crop {
        CaptureMode::Crop
    } else if buttons.window {
        CaptureMode::Window
    } else if buttons.monitor {
        CaptureMode::Monitor
    } else if buttons.all {
        CaptureMode::All
    } else {
        // All capture-mode buttons are disabled; default to Crop so canvas
        // interactions (drag-to-select region) remain available even when the
        // toolbar is hidden or cancel-only.
        CaptureMode::Crop
    }
}
