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
use std::time::{Duration, Instant};

use image::ImageBuffer;
use memmap2::MmapMut;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::memfd;
use std::os::fd::AsFd;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    protocol::{wl_buffer, wl_registry, wl_shm, wl_shm_pool},
};
use wayland_protocols_hyprland::toplevel_export::v1::client::{
    hyprland_toplevel_export_frame_v1, hyprland_toplevel_export_manager_v1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};

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

impl Dispatch<wl_shm::WlShm, ()> for ToplevelExportState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for ToplevelExportState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for ToplevelExportState {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<hyprland_toplevel_export_manager_v1::HyprlandToplevelExportManagerV1, ()>
    for ToplevelExportState
{
    fn event(
        _: &mut Self,
        _: &hyprland_toplevel_export_manager_v1::HyprlandToplevelExportManagerV1,
        _: hyprland_toplevel_export_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

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

fn dispatch_until(
    event_queue: &mut EventQueue<ToplevelExportState>,
    state: &mut ToplevelExportState,
    timeout: Duration,
    mut done: impl FnMut(&ToplevelExportState) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        event_queue
            .dispatch_pending(state)
            .map_err(|e| AppError::Wayland(format!("Wayland dispatch failed: {e}")))?;

        if done(state) {
            return Ok(());
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AppError::Wayland(
                "Toplevel export timed out: compositor did not respond in time".to_string(),
            ));
        }

        event_queue
            .flush()
            .map_err(|e| AppError::Wayland(format!("Wayland flush failed: {e}")))?;

        let timeout_ms = remaining.as_millis().min(u16::MAX as u128) as u16;
        let guard = event_queue.prepare_read();
        {
            let fd = event_queue.as_fd();
            let mut pollfds = [PollFd::new(fd, PollFlags::POLLIN)];
            poll(&mut pollfds, PollTimeout::from(timeout_ms))
                .map_err(|e| AppError::Wayland(format!("poll failed: {e}")))?;
        }
        if let Some(g) = guard {
            g.read()
                .map_err(|e| AppError::Wayland(format!("Wayland read failed: {e}")))?;
        }
    }
}

/// Allocate a wl_shm buffer for `fi` and issue `frame.copy(buffer, ignore_damage=1)`.
/// Returns an error if the buffer cannot be created.
fn attach_and_copy_buffer(
    state: &mut ToplevelExportState,
    event_queue: &mut EventQueue<ToplevelExportState>,
) -> Result<()> {
    let fi = state
        .frame_info
        .as_mut()
        .ok_or_else(|| AppError::Wayland("no frame info".to_string()))?;

    let stride = fi.stride as usize;
    let height = fi.height as usize;
    let width = fi.width as usize;

    let Some(bytes_per_row) = width.checked_mul(4) else {
        fi.failed = true;
        return Err(AppError::Wayland(format!(
            "invalid buffer dimensions: width {} * 4 overflows usize",
            fi.width
        )));
    };
    if stride < bytes_per_row || !stride.is_multiple_of(4) {
        fi.failed = true;
        return Err(AppError::Wayland(format!(
            "invalid stride: stride={stride} width={} (expected ≥{bytes_per_row} and multiple of 4)",
            fi.width
        )));
    }

    let Some(size) = stride.checked_mul(height) else {
        fi.failed = true;
        return Err(AppError::Wayland(format!(
            "buffer size overflow: stride={stride} * height={}",
            fi.height
        )));
    };
    let pool_size_i32 = i32::try_from(size).map_err(|_| {
        AppError::Wayland(format!(
            "shm pool size too large: {size} bytes (max {})",
            i32::MAX
        ))
    })?;

    let fd = memfd::memfd_create(c"toplevel_export", memfd::MFdFlags::MFD_CLOEXEC)
        .map_err(|e| AppError::Wayland(format!("memfd_create failed: {e}")))?;
    nix::unistd::ftruncate(&fd, size as i64)
        .map_err(|e| AppError::Wayland(format!("ftruncate failed: {e}")))?;

    let file = std::fs::File::from(fd);
    let mmap = unsafe { MmapMut::map_mut(&file) }
        .map_err(|e| AppError::Wayland(format!("mmap failed: {e}")))?;

    let qh = event_queue.handle();
    let format = fi.format.ok_or_else(|| {
        AppError::Wayland("shm format not received before buffer_done".to_string())
    })?;
    let WEnum::Value(shm_format) = format else {
        return Err(AppError::Wayland(
            "unsupported shm format enum variant".to_string(),
        ));
    };

    let pool = state
        .shm
        .as_ref()
        .ok_or_else(|| AppError::Wayland("wl_shm global not available".to_string()))?
        .create_pool(file.as_fd(), pool_size_i32, &qh, ());

    let buffer = pool.create_buffer(
        0,
        fi.width as i32,
        fi.height as i32,
        fi.stride as i32,
        shm_format,
        &qh,
        (),
    );
    pool.destroy();

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
/// - `AppError::HyprlandProtocol` — no foreign toplevel matched `window.title` + `window.class`
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

    // Second roundtrip: collect foreign toplevel metadata (title, app_id, done).
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| AppError::Wayland(format!("Wayland roundtrip (toplevels) failed: {e}")))?;

    // Find the handle matching title + class (app_id).
    let handle = state
        .toplevels
        .iter()
        .find(|e| e.title == window.title && e.app_id == window.class)
        .map(|e| e.handle.clone())
        .ok_or_else(|| {
            AppError::HyprlandProtocol(format!(
                "no foreign toplevel found matching title='{}' class='{}'",
                window.title, window.class
            ))
        })?;

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
    dispatch_until(&mut event_queue, &mut state, Duration::from_secs(10), |s| {
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
    dispatch_until(&mut event_queue, &mut state, Duration::from_secs(10), |s| {
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

    // Resolve the pixel-channel layout once before the nested loop so the
    // format match is not repeated for every pixel.
    let bgr = matches!(
        format,
        WEnum::Value(wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888)
    );
    let force_opaque = matches!(
        format,
        WEnum::Value(wl_shm::Format::Xrgb8888 | wl_shm::Format::Xbgr8888)
    );

    let mut img: ImageBuffer<image::Rgba<u8>, Vec<u8>> = ImageBuffer::new(fi.width, fi.height);
    let stride = fi.stride as usize;
    for y in 0..fi.height {
        let row_offset = y as usize * stride;
        for x in 0..fi.width {
            let offset = row_offset + x as usize * 4;
            let (r, g, b, a) = if bgr {
                (
                    mmap[offset + 2],
                    mmap[offset + 1],
                    mmap[offset],
                    mmap[offset + 3],
                )
            } else {
                (
                    mmap[offset],
                    mmap[offset + 1],
                    mmap[offset + 2],
                    mmap[offset + 3],
                )
            };
            img.put_pixel(
                x,
                y,
                image::Rgba([r, g, b, if force_opaque { 255 } else { a }]),
            );
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
