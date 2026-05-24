//! # platform::capture::toplevel_export
//!
//! Window capture via the `hyprland-toplevel-export-v1` Wayland protocol.
//!
//! Unlike the screencopy path (which captures a monitor region and crops), this
//! module requests the compositor to export a specific toplevel's surface pixels
//! directly, identified by its Hyprland window address handle.
//!
//! The captured image is the raw window surface — no compositor decorations, no
//! border expansion. This is why `capture_window_border` is forced off when this
//! path is enabled.

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

use crate::domain::error::{AppError, Result};

// ── State ─────────────────────────────────────────────────────────────────────

struct ToplevelExportState {
    shm: Option<wl_shm::WlShm>,
    manager: Option<hyprland_toplevel_export_manager_v1::HyprlandToplevelExportManagerV1>,
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
                fi.y_invert =
                    flags == WEnum::Value(hyprland_toplevel_export_frame_v1::Flags::YInvert);
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
            let _ = poll(&mut pollfds, PollTimeout::from(timeout_ms));
        }
        if let Some(g) = guard {
            let _ = g.read();
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

/// Capture the toplevel window identified by `address` and save the result to `out_path`.
///
/// `address` is the Hyprland window handle as returned by `hyprctl -j clients`
/// (the `address` field, e.g. `"0xdeadbeef"`), cast to `u32` for the v1 protocol.
///
/// # Errors
/// - `AppError::Wayland` — protocol not available, compositor rejected capture, timeout
/// - `AppError::Image` — failed to save the output image
pub fn capture_toplevel_to_path(address: u64, out_path: &Path) -> Result<()> {
    let conn = Connection::connect_to_env()
        .map_err(|_| AppError::Wayland("Failed to connect to Wayland".to_string()))?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = ToplevelExportState::new();
    let _registry = display.get_registry(&qh, ());

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

    // The v1 protocol handle is a u32. Hyprland window addresses on 64-bit systems
    // may exceed u32; the protocol truncates to the lower 32 bits.
    let handle = address as u32;
    let frame = manager.capture_toplevel(0, handle, &qh, ());

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

    let mut img: ImageBuffer<image::Rgba<u8>, Vec<u8>> = ImageBuffer::new(fi.width, fi.height);
    for y in 0..fi.height {
        let row_offset = y as usize * fi.stride as usize;
        for x in 0..fi.width {
            let offset = row_offset + x as usize * 4;
            img.put_pixel(x, y, read_pixel_rgba(mmap, offset, format));
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

// ── Pixel helpers (same logic as screencopy) ──────────────────────────────────

fn read_pixel_rgba(data: &[u8], offset: usize, format: WEnum<wl_shm::Format>) -> image::Rgba<u8> {
    let ok = offset
        .checked_add(3)
        .is_some_and(|max_idx| max_idx < data.len());
    if !ok {
        return image::Rgba([0, 0, 0, 0]);
    }
    let b0 = data[offset];
    let b1 = data[offset + 1];
    let b2 = data[offset + 2];
    let b3 = data[offset + 3];
    match format {
        WEnum::Value(wl_shm::Format::Argb8888) => image::Rgba([b2, b1, b0, b3]),
        WEnum::Value(wl_shm::Format::Xrgb8888) => image::Rgba([b2, b1, b0, 255]),
        WEnum::Value(wl_shm::Format::Abgr8888) => image::Rgba([b0, b1, b2, b3]),
        WEnum::Value(wl_shm::Format::Xbgr8888) => image::Rgba([b0, b1, b2, 255]),
        _ => image::Rgba([b2, b1, b0, b3]),
    }
}
