//! # platform::capture::toplevel_export
//!
//! Window capture via the `hyprland-toplevel-export-v1` Wayland protocol (v2).
//!
//! Uses `capture_toplevel_with_wlr_toplevel_handle` (protocol version 2) together
//! with `zwlr_foreign_toplevel_management_v1` to identify the target window by
//! title and app_id (class). This avoids the u32 truncation problem of the v1
//! `capture_toplevel` request, which is unusable on 64-bit Hyprland systems.
//!
//! The captured image is the raw window surface — no compositor decorations.

use std::path::Path;
use std::time::Duration;

use image::ImageBuffer;
use memmap2::MmapMut;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop,
    protocol::{wl_buffer, wl_registry, wl_shm, wl_shm_pool},
};
use wayland_protocols_hyprland::toplevel_export::v1::client::{
    hyprland_toplevel_export_frame_v1, hyprland_toplevel_export_manager_v1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};

use super::wl_shared;
use crate::domain::error::{AppError, Result};
use crate::domain::types::WindowInfo;

// ── State ─────────────────────────────────────────────────────────────────────

/// Metadata collected from a `zwlr_foreign_toplevel_handle_v1`.
struct ForeignToplevelEntry {
    handle: zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    title: String,
    app_id: String,
    /// Set when the compositor sends `done` — all metadata for this cycle is final.
    done: bool,
}

struct ToplevelExportState {
    shm: Option<wl_shm::WlShm>,
    manager: Option<hyprland_toplevel_export_manager_v1::HyprlandToplevelExportManagerV1>,
    foreign_manager: Option<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    toplevels: Vec<ForeignToplevelEntry>,
    frame_info: Option<ToplevelFrameInfo>,
}

struct ToplevelFrameInfo {
    frame: hyprland_toplevel_export_frame_v1::HyprlandToplevelExportFrameV1,
    width: u32,
    height: u32,
    stride: u32,
    format: Option<WEnum<wl_shm::Format>>,
    /// Set after `buffer_done` — the client must create a buffer and call `copy`.
    buffer_done: bool,
    ready: bool,
    failed: bool,
    error_msg: Option<String>,
    y_invert: bool,
    mmap: Option<MmapMut>,
    buffer: Option<wl_buffer::WlBuffer>,
}

impl ToplevelExportState {
    fn new() -> Self {
        Self {
            shm: None,
            manager: None,
            foreign_manager: None,
            toplevels: Vec::new(),
            frame_info: None,
        }
    }
}

// ── Dispatch impls ─────────────────────────────────────────────────────────────

impl Dispatch<wl_registry::WlRegistry, ()> for ToplevelExportState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_shm" => {
                    state.shm =
                        Some(registry.bind::<wl_shm::WlShm, _, _>(name, version.min(1), qh, ()));
                }
                "hyprland_toplevel_export_manager_v1" => {
                    state.manager = Some(
                        registry
                            .bind::<hyprland_toplevel_export_manager_v1::HyprlandToplevelExportManagerV1, _, _>(
                                name,
                                version.min(2),
                                qh,
                                (),
                            ),
                    );
                }
                "zwlr_foreign_toplevel_manager_v1" => {
                    state.foreign_manager = Some(
                        registry.bind::<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, _, _>(
                            name,
                            version.min(3),
                            qh,
                            (),
                        ),
                    );
                }
                _ => {}
            }
        }
    }
}

delegate_noop!(ToplevelExportState: ignore wl_shm::WlShm);
delegate_noop!(ToplevelExportState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(ToplevelExportState: ignore wl_buffer::WlBuffer);
delegate_noop!(ToplevelExportState: hyprland_toplevel_export_manager_v1::HyprlandToplevelExportManagerV1);

impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()>
    for ToplevelExportState
{
    wayland_client::event_created_child!(
        ToplevelExportState,
        zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        [
            // opcode 0 = "toplevel" event, creates a ZwlrForeignToplevelHandleV1
            0 => (zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()),
        ]
    );

    fn event(
        state: &mut Self,
        _: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.push(ForeignToplevelEntry {
                handle: toplevel,
                title: String::new(),
                app_id: String::new(),
                done: false,
            });
            // The handle events (title, app_id, done) arrive via the handle's Dispatch.
            let _ = qh;
        }
    }
}

impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()>
    for ToplevelExportState
{
    fn event(
        state: &mut Self,
        handle: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(entry) = state.toplevels.iter_mut().find(|e| &e.handle == handle) else {
            return;
        };
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                entry.title = title;
            }
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                entry.app_id = app_id;
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => {
                entry.done = true;
            }
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                // Remove closed toplevels so they can't be matched.
                state.toplevels.retain(|e| &e.handle != handle);
            }
            _ => {}
        }
    }
}

impl Dispatch<hyprland_toplevel_export_frame_v1::HyprlandToplevelExportFrameV1, ()>
    for ToplevelExportState
{
    fn event(
        state: &mut Self,
        _frame: &hyprland_toplevel_export_frame_v1::HyprlandToplevelExportFrameV1,
        event: hyprland_toplevel_export_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(fi) = state.frame_info.as_mut() else {
            return;
        };

        match event {
            hyprland_toplevel_export_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let shm_format = match format {
                    WEnum::Value(
                        v @ (wl_shm::Format::Argb8888
                        | wl_shm::Format::Xrgb8888
                        | wl_shm::Format::Abgr8888
                        | wl_shm::Format::Xbgr8888),
                    ) => v,
                    _ => {
                        fi.failed = true;
                        fi.error_msg = Some(format!("unsupported shm format: {format:?}"));
                        return;
                    }
                };
                fi.format = Some(WEnum::Value(shm_format));
                fi.width = width;
                fi.height = height;
                fi.stride = stride;
            }

            hyprland_toplevel_export_frame_v1::Event::BufferDone => {
                fi.buffer_done = true;
            }

            hyprland_toplevel_export_frame_v1::Event::Flags { flags } => {
                fi.y_invert = matches!(
                    flags,
                    WEnum::Value(f) if f.contains(hyprland_toplevel_export_frame_v1::Flags::YInvert)
                );
            }

            hyprland_toplevel_export_frame_v1::Event::Ready { .. } => {
                fi.ready = true;
            }

            hyprland_toplevel_export_frame_v1::Event::Failed => {
                fi.failed = true;
                fi.error_msg = Some("compositor rejected the toplevel export request".to_string());
            }

            _ => {}
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Allocate a wl_shm buffer for `fi` and issue `frame.copy(buffer, ignore_damage=1)`.
/// Returns an error if the buffer cannot be created.
fn attach_and_copy_buffer(
    state: &mut ToplevelExportState,
    event_queue: &mut EventQueue<ToplevelExportState>,
) -> Result<()> {
    let qh = event_queue.handle();

    let (width, height, stride, shm_format) = {
        let fi = state
            .frame_info
            .as_mut()
            .ok_or_else(|| AppError::Wayland("no frame info".to_string()))?;
        let format = fi.format.ok_or_else(|| {
            AppError::Wayland("shm format not received before buffer_done".to_string())
        })?;
        let WEnum::Value(shm_format) = format else {
            return Err(AppError::Wayland(
                "unsupported shm format enum variant".to_string(),
            ));
        };
        (fi.width, fi.height, fi.stride, shm_format)
    };

    let shm = state
        .shm
        .as_ref()
        .ok_or_else(|| AppError::Wayland("wl_shm global not available".to_string()))?;

    let (mmap, buffer) = wl_shared::alloc_shm_buffer(shm, width, height, stride, shm_format, &qh)?;

    let fi = state.frame_info.as_mut().unwrap();
    // ignore_damage = 1: capture current frame immediately without waiting for damage.
    fi.frame.copy(&buffer, 1);
    fi.mmap = Some(mmap);
    fi.buffer = Some(buffer);

    Ok(())
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Capture the toplevel window described by `window` and save the result to `out_path`.
///
/// Uses the `hyprland-toplevel-export-v1` v2 protocol together with
/// `zwlr_foreign_toplevel_management_v1` to identify the target by title and
/// class (app_id). This avoids the u32 truncation limitation of v1.
///
/// # Errors
/// - `AppError::HyprlandProtocol` — no foreign toplevel matched `window.title` + `window.class`,
///   or multiple toplevels matched (ambiguous)
/// - `AppError::Wayland` — required protocol not available, compositor rejected capture, timeout
/// - `AppError::Image` — failed to save the output image
pub fn capture_toplevel_to_path(window: &WindowInfo, out_path: &Path) -> Result<()> {
    let conn = Connection::connect_to_env()
        .map_err(|_| AppError::Wayland("Failed to connect to Wayland".to_string()))?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = ToplevelExportState::new();
    let _registry = display.get_registry(&qh, ());

    // First roundtrip: bind globals (manager, foreign_manager, shm).
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| AppError::Wayland(format!("Wayland roundtrip failed: {e}")))?;

    let manager = state
        .manager
        .as_ref()
        .ok_or_else(|| {
            AppError::Wayland(
                "hyprland_toplevel_export_manager_v1 not available — is this Hyprland?".to_string(),
            )
        })?
        .clone();

    if state.foreign_manager.is_none() {
        return Err(AppError::Wayland(
            "zwlr_foreign_toplevel_manager_v1 not available — compositor does not support it"
                .to_string(),
        ));
    }

    // Second phase: wait for all foreign toplevel handles to have finalized metadata.
    // A single roundtrip is not sufficient — the compositor sends each handle's
    // title/app_id/done events in a separate batch after the toplevel is announced.
    // Dispatch until every listed entry has received `done`, or until timeout.
    wl_shared::dispatch_until(&mut event_queue, &mut state, Duration::from_secs(5), |s| {
        !s.toplevels.is_empty() && s.toplevels.iter().all(|e| e.done)
    })?;

    // Find handles matching title + class (app_id), considering only finalized entries.
    let matches: Vec<_> = state
        .toplevels
        .iter()
        .filter(|e| e.done && e.title == window.title && e.app_id == window.class)
        .collect();

    let handle = match matches.len() {
        0 => {
            return Err(AppError::HyprlandProtocol(format!(
                "no foreign toplevel found matching title='{}' class='{}'",
                window.title, window.class
            )));
        }
        1 => matches[0].handle.clone(),
        n => {
            return Err(AppError::HyprlandProtocol(format!(
                "{n} foreign toplevels match title='{}' class='{}' — cannot determine which to capture",
                window.title, window.class
            )));
        }
    };

    let frame = manager.capture_toplevel_with_wlr_toplevel_handle(0, &handle, &qh, ());

    state.frame_info = Some(ToplevelFrameInfo {
        frame,
        width: 0,
        height: 0,
        stride: 0,
        format: None,
        buffer_done: false,
        ready: false,
        failed: false,
        error_msg: None,
        y_invert: false,
        mmap: None,
        buffer: None,
    });

    // Phase 1: wait for buffer_done (or early failure)
    wl_shared::dispatch_until(&mut event_queue, &mut state, Duration::from_secs(10), |s| {
        s.frame_info
            .as_ref()
            .is_some_and(|f| f.buffer_done || f.failed)
    })?;

    {
        let fi = state
            .frame_info
            .as_ref()
            .ok_or_else(|| AppError::Wayland("frame info missing after buffer_done".to_string()))?;
        if fi.failed {
            return Err(AppError::Wayland(fi.error_msg.clone().unwrap_or_else(
                || "toplevel export failed before buffer_done".to_string(),
            )));
        }
    }

    // Phase 2: allocate buffer and request copy
    attach_and_copy_buffer(&mut state, &mut event_queue)?;

    // Phase 3: wait for ready or failed
    wl_shared::dispatch_until(&mut event_queue, &mut state, Duration::from_secs(10), |s| {
        s.frame_info.as_ref().is_some_and(|f| f.ready || f.failed)
    })?;

    let fi = state
        .frame_info
        .as_ref()
        .ok_or_else(|| AppError::Wayland("frame info missing after capture".to_string()))?;

    if fi.failed {
        return Err(AppError::Wayland(fi.error_msg.clone().unwrap_or_else(
            || "toplevel export failed during copy".to_string(),
        )));
    }

    let mmap = fi
        .mmap
        .as_ref()
        .ok_or_else(|| AppError::Wayland("mmap missing after successful capture".to_string()))?;
    let format = fi
        .format
        .ok_or_else(|| AppError::Wayland("format not set after successful capture".to_string()))?;

    let mut img: ImageBuffer<image::Rgba<u8>, Vec<u8>> = ImageBuffer::new(fi.width, fi.height);
    let stride = fi.stride as usize;
    for y in 0..fi.height {
        let row_offset = y as usize * stride;
        for x in 0..fi.width {
            let offset = row_offset + x as usize * 4;
            img.put_pixel(x, y, wl_shared::read_pixel_rgba(mmap, offset, format));
        }
    }

    if fi.y_invert {
        image::imageops::flip_vertical_in_place(&mut img);
    }

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::FileSystem(parent.to_path_buf(), e))?;
    }
    img.save(out_path).map_err(AppError::from)?;

    Ok(())
}
