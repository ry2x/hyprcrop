use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool},
};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

use crate::domain::error::{AppError, Result};
use crate::domain::types::{MonitorInfo, ScreenRect};
use image::{ImageBuffer, Rgba};
use memmap2::MmapMut;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::memfd;
use std::ffi::CStr;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

pub struct CaptureState {
    pub shm: Option<wl_shm::WlShm>,
    pub screencopy_manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    pub xdg_output_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    pub outputs: Vec<OutputInfo>,
    pub frames: Vec<FrameInfo>,
}

pub struct OutputInfo {
    pub output: wl_output::WlOutput,
    pub name: Option<String>,
    pub xdg_output: Option<zxdg_output_v1::ZxdgOutputV1>,
}

pub struct FrameInfo {
    pub frame: zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: Option<WEnum<wl_shm::Format>>,
    pub ready: bool,
    pub failed: bool,
    /// Populated when `failed` is true; carries diagnostic context.
    pub error_msg: Option<String>,
    pub mmap: Option<MmapMut>,
    pub buffer: Option<wl_buffer::WlBuffer>,
    pub name: String,
}

impl Default for CaptureState {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureState {
    pub fn new() -> Self {
        Self {
            shm: None,
            screencopy_manager: None,
            xdg_output_manager: None,
            outputs: Vec::new(),
            frames: Vec::new(),
        }
    }
}

/// Convert a pixel at byte `offset` in `data` to RGBA based on the shm format.
///
/// Wayland shm format memory layout (little-endian):
/// - ARGB8888: bytes = [Blue, Green, Red, Alpha]  → RGBA uses real alpha
/// - XRGB8888: bytes = [Blue, Green, Red, X]      → alpha forced to 255
/// - ABGR8888: bytes = [Red, Green, Blue, Alpha]  → RGBA uses real alpha
/// - XBGR8888: bytes = [Red, Green, Blue, X]      → alpha forced to 255
///
/// Non-panicking: if the buffer is too small, logs a warning and returns transparent black.
fn read_pixel_rgba(data: &[u8], offset: usize, format: WEnum<wl_shm::Format>) -> Rgba<u8> {
    // Need 4 bytes (offset..offset+3); checked_add avoids wrapping on 32-bit targets.
    let ok = offset
        .checked_add(3)
        .is_some_and(|max_idx| max_idx < data.len());
    if !ok {
        eprintln!(
            "read_pixel_rgba: offset {offset} out of bounds for buffer length {} (format: {format:?})",
            data.len()
        );
        return Rgba([0, 0, 0, 0]);
    }
    let b0 = data[offset];
    let b1 = data[offset + 1];
    let b2 = data[offset + 2];
    let b3 = data[offset + 3];
    match format {
        // ARGB8888: [B, G, R, A] → real alpha
        WEnum::Value(wl_shm::Format::Argb8888) => Rgba([b2, b1, b0, b3]),
        // XRGB8888: [B, G, R, X] → alpha forced 255 (X channel is padding)
        WEnum::Value(wl_shm::Format::Xrgb8888) => Rgba([b2, b1, b0, 255]),
        // ABGR8888: [R, G, B, A] → real alpha
        WEnum::Value(wl_shm::Format::Abgr8888) => Rgba([b0, b1, b2, b3]),
        // XBGR8888: [R, G, B, X] → alpha forced 255
        WEnum::Value(wl_shm::Format::Xbgr8888) => Rgba([b0, b1, b2, 255]),
        // Defensive fallback: the Buffer event handler whitelists supported formats, so this
        // branch should never be reached in practice.
        _ => {
            eprintln!(
                "read_pixel_rgba: unsupported wl_shm format {format:?}, falling back to ARGB8888 layout"
            );
            Rgba([b2, b1, b0, b3])
        }
    }
}

/// Initialize a Wayland connection, discover globals, and resolve xdg-output names.
fn init_wayland() -> Result<(EventQueue<CaptureState>, CaptureState)> {
    let conn = Connection::connect_to_env()
        .map_err(|_| AppError::Wayland("Failed to connect to Wayland".to_string()))?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = CaptureState::new();
    let _registry = display.get_registry(&qh, ());

    event_queue
        .roundtrip(&mut state)
        .map_err(|e| AppError::Wayland(format!("Wayland roundtrip failed: {e}")))?;

    let xdg_mgr = state
        .xdg_output_manager
        .as_ref()
        .ok_or_else(|| AppError::Wayland("zxdg_output_manager_v1 not available".to_string()))?
        .clone();

    for out in &mut state.outputs {
        out.xdg_output = Some(xdg_mgr.get_xdg_output(&out.output, &qh, ()));
    }

    event_queue
        .roundtrip(&mut state)
        .map_err(|e| AppError::Wayland(format!("Wayland roundtrip failed: {e}")))?;

    Ok((event_queue, state))
}

/// Drive the event queue until `done` returns `true`, or until `timeout` elapses.
///
/// Uses `prepare_read` + `poll` so the thread yields to the OS rather than
/// spinning, and returns an error instead of hanging forever if the compositor
/// stops responding (e.g. because an output was removed mid-capture).
fn dispatch_until(
    event_queue: &mut EventQueue<CaptureState>,
    state: &mut CaptureState,
    timeout: Duration,
    mut done: impl FnMut(&CaptureState) -> bool,
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
                "Screencopy timed out: compositor did not respond in time".to_string(),
            ));
        }

        event_queue
            .flush()
            .map_err(|e| AppError::Wayland(format!("Wayland flush failed: {e}")))?;

        // Prepare a socket read.  Guard and fd are both immutable borrows of event_queue;
        // they coexist safely since Rust allows multiple &self borrows simultaneously.
        // Drop pollfds (and its BorrowedFd) before consuming the guard — the borrow checker
        // requires both immutable borrows to end before the next dispatch_pending(&mut self).
        let timeout_ms = remaining.as_millis().min(u16::MAX as u128) as u16;
        let guard = event_queue.prepare_read();
        {
            let fd = event_queue.as_fd();
            let mut pollfds = [PollFd::new(fd, PollFlags::POLLIN)];
            let _ = poll(&mut pollfds, PollTimeout::from(timeout_ms));
            // pollfds (and BorrowedFd) dropped here; immutable borrow 2 ends.
        }
        if let Some(g) = guard {
            let _ = g.read(); // consumes guard; immutable borrow 1 ends.
        }
    }
}

// --- Dispatch impls ---

impl Dispatch<wl_registry::WlRegistry, ()> for CaptureState {
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
                "wl_output" => {
                    // Bind at the version the compositor advertises, capped at v1.
                    // We only need basic output enumeration; xdg_output handles naming.
                    let output =
                        registry.bind::<wl_output::WlOutput, _, _>(name, version.min(1), qh, ());
                    state.outputs.push(OutputInfo {
                        output,
                        name: None,
                        xdg_output: None,
                    });
                }
                "wl_shm" => {
                    state.shm =
                        Some(registry.bind::<wl_shm::WlShm, _, _>(name, version.min(1), qh, ()));
                }
                "zwlr_screencopy_manager_v1" => {
                    state.screencopy_manager = Some(
                        registry.bind::<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        ),
                    );
                }
                "zxdg_output_manager_v1" => {
                    // Cap at v3 (crate maximum); bind only what the compositor supports
                    // to avoid a protocol error on compositors that expose v1/v2 only.
                    state.xdg_output_manager = Some(
                        registry.bind::<zxdg_output_manager_v1::ZxdgOutputManagerV1, _, _>(
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

impl Dispatch<wl_output::WlOutput, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_shm::WlShm, ()> for CaptureState {
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
impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _: zwlr_screencopy_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        _: zxdg_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<zxdg_output_v1::ZxdgOutputV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        xdg_output: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_output_v1::Event::Name { name } = event {
            for out in &mut state.outputs {
                if out.xdg_output.as_ref() == Some(xdg_output) {
                    out.name = Some(name.clone());
                }
            }
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(fi_idx) = state.frames.iter().position(|f| &f.frame == frame) else {
            return;
        };

        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                // --- Step 1: format whitelist (earliest rejection) ---
                // Only 32bpp formats are supported; reject everything else before
                // allocating any resources to avoid corrupt output downstream.
                let shm_format = match format {
                    WEnum::Value(
                        v @ (wl_shm::Format::Argb8888
                        | wl_shm::Format::Xrgb8888
                        | wl_shm::Format::Abgr8888
                        | wl_shm::Format::Xbgr8888),
                    ) => v,
                    _ => {
                        state.frames[fi_idx].failed = true;
                        state.frames[fi_idx].error_msg =
                            Some(format!("unsupported shm format: {format:?}"));
                        return;
                    }
                };

                // --- Step 2: stride validation ---
                // stride < width*4 would cause reads past the end of each row;
                // stride % 4 != 0 would misalign per-pixel offsets.
                let stride_usize = stride as usize;
                let width_usize = width as usize;
                let height_usize = height as usize;
                let Some(bytes_per_row) = width_usize.checked_mul(4) else {
                    state.frames[fi_idx].failed = true;
                    state.frames[fi_idx].error_msg = Some(format!(
                        "invalid buffer dimensions: width {width} * 4 overflows usize"
                    ));
                    return;
                };
                if stride_usize < bytes_per_row || !stride_usize.is_multiple_of(4) {
                    state.frames[fi_idx].failed = true;
                    state.frames[fi_idx].error_msg = Some(format!(
                        "invalid compositor stride: stride={stride} width={width} (expected ≥{bytes_per_row} and multiple of 4)"
                    ));
                    return;
                }

                // --- Step 3: buffer size ---
                // Use checked_mul to detect overflow (very large HiDPI buffers).
                let Some(size) = stride_usize.checked_mul(height_usize) else {
                    state.frames[fi_idx].failed = true;
                    state.frames[fi_idx].error_msg = Some(format!(
                        "buffer size overflow: stride={stride} * height={height}"
                    ));
                    return;
                };
                // wl_shm::create_pool takes i32; reject before truncating.
                let pool_size_i32 = match i32::try_from(size) {
                    Ok(v) => v,
                    Err(_) => {
                        state.frames[fi_idx].failed = true;
                        state.frames[fi_idx].error_msg = Some(format!(
                            "shm pool size too large: {size} bytes (max {})",
                            i32::MAX
                        ));
                        return;
                    }
                };

                let fd = match memfd::memfd_create(
                    CStr::from_bytes_with_nul(b"screencopy\0")
                        .expect("static literal is valid CStr"),
                    memfd::MFdFlags::MFD_CLOEXEC,
                ) {
                    Ok(fd) => fd,
                    Err(e) => {
                        state.frames[fi_idx].failed = true;
                        state.frames[fi_idx].error_msg = Some(format!("memfd_create failed: {e}"));
                        return;
                    }
                };

                if let Err(e) = nix::unistd::ftruncate(&fd, size as i64) {
                    state.frames[fi_idx].failed = true;
                    state.frames[fi_idx].error_msg = Some(format!("ftruncate failed: {e}"));
                    return;
                }

                let file = std::fs::File::from(fd);
                let mmap = match unsafe { MmapMut::map_mut(&file) } {
                    Ok(m) => m,
                    Err(e) => {
                        state.frames[fi_idx].failed = true;
                        state.frames[fi_idx].error_msg = Some(format!("mmap failed: {e}"));
                        return;
                    }
                };

                // Borrow shm separately; NLL ends this borrow before the mutable frame update below.
                let pool = match state.shm.as_ref() {
                    Some(shm) => shm.create_pool(file.as_fd(), pool_size_i32, qh, ()),
                    None => {
                        state.frames[fi_idx].failed = true;
                        state.frames[fi_idx].error_msg =
                            Some("wl_shm global not available".to_string());
                        return;
                    }
                };

                let buffer = pool.create_buffer(
                    0,
                    width as i32,
                    height as i32,
                    stride as i32,
                    shm_format,
                    qh,
                    (),
                );
                pool.destroy();
                frame.copy(&buffer);

                let fi = &mut state.frames[fi_idx];
                fi.format = Some(WEnum::Value(shm_format));
                fi.width = width;
                fi.height = height;
                fi.stride = stride;
                fi.buffer = Some(buffer);
                fi.mmap = Some(mmap);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                state.frames[fi_idx].ready = true;
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.frames[fi_idx].failed = true;
                state.frames[fi_idx].error_msg =
                    Some("compositor rejected the screencopy request".to_string());
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for CaptureState {
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
impl Dispatch<wl_buffer::WlBuffer, ()> for CaptureState {
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

// --- Public capture API ---

/// Helper to extract physical ImageBuffer from FrameInfo.
fn extract_image(fi: &FrameInfo) -> Result<RgbaImage> {
    let mmap = fi
        .mmap
        .as_ref()
        .ok_or_else(|| AppError::Screencopy(format!("buffer missing for monitor '{}'", fi.name)))?;
    let format = fi
        .format
        .ok_or_else(|| AppError::Screencopy(format!("format not set for monitor '{}'", fi.name)))?;
    let mut img = ImageBuffer::new(fi.width, fi.height);
    for y in 0..fi.height {
        let row_offset = y as usize * fi.stride as usize;
        for x in 0..fi.width {
            let offset = row_offset + x as usize * 4;
            img.put_pixel(x, y, read_pixel_rgba(mmap, offset, format));
        }
    }
    Ok(img)
}

/// Helper to capture a set of outputs and provide the resulting frames to a closure.
fn capture_frames<F, R>(names: &[&str], f: F) -> Result<R>
where
    F: FnOnce(&[FrameInfo]) -> Result<R>,
{
    let (mut event_queue, mut state) = init_wayland()?;
    let qh = event_queue.handle();

    let screencopy_mgr = state
        .screencopy_manager
        .as_ref()
        .ok_or_else(|| AppError::Wayland("zwlr_screencopy_manager_v1 not available".to_string()))?;

    let mut unmatched: Vec<&str> = Vec::new();
    for &name in names {
        match state
            .outputs
            .iter()
            .find(|o| o.name.as_deref() == Some(name))
        {
            Some(output) => {
                let frame = screencopy_mgr.capture_output(0, &output.output, &qh, ());
                state.frames.push(FrameInfo {
                    frame,
                    width: 0,
                    height: 0,
                    stride: 0,
                    format: None,
                    ready: false,
                    failed: false,
                    error_msg: None,
                    mmap: None,
                    buffer: None,
                    name: name.to_string(),
                });
            }
            None => unmatched.push(name),
        }
    }
    if !unmatched.is_empty() {
        if unmatched.len() == 1 {
            return Err(AppError::Wayland(format!(
                "Monitor '{}' not found",
                unmatched[0]
            )));
        } else {
            return Err(AppError::Wayland(format!(
                "No Wayland output found for monitors: {}",
                unmatched.join(", ")
            )));
        }
    }

    dispatch_until(&mut event_queue, &mut state, Duration::from_secs(10), |s| {
        s.frames.iter().all(|f| f.ready || f.failed)
    })?;

    let failures: Vec<String> = state
        .frames
        .iter()
        .filter(|f| f.failed)
        .map(|f| {
            format!(
                "'{}': {}",
                f.name,
                f.error_msg.as_deref().unwrap_or("unknown error")
            )
        })
        .collect();
    if !failures.is_empty() {
        return Err(AppError::Screencopy(format!(
            "failed for: {}",
            failures.join(", ")
        )));
    }

    f(&state.frames)
}

/// Capture a single monitor at full physical resolution.
pub fn capture_monitor(monitor_name: &str) -> Result<RgbaImage> {
    capture_frames(&[monitor_name], |frames| extract_image(&frames[0]))
}

/// Type alias to reduce verbosity of per-monitor capture return types.
pub type RgbaImage = ImageBuffer<Rgba<u8>, Vec<u8>>;

/// Capture all monitors and composite them into a single image in **logical pixel space**.
///
/// The output dimensions and pixel coordinates match what Hyprland IPC and slurp report,
/// so crop coordinates can be applied directly without coordinate conversion.
/// HiDPI monitors are downsampled to their logical size during compositing.
pub fn capture_all_monitors(monitors: &[MonitorInfo]) -> Result<RgbaImage> {
    Ok(capture_all_monitors_with_physical(monitors)?.1)
}

/// Capture all monitors in a **single Wayland session** and return:
/// - Per-monitor physical-resolution images (in the same order as `monitors`)
/// - Logical-space composite of all monitors (for crop operations)
///
/// Using one session ensures the overlay and the final crop originate from the
/// same frame, which is critical for the freeze-mode "what you see is what you
/// save" guarantee.
pub fn capture_all_monitors_with_physical(
    monitors: &[MonitorInfo],
) -> Result<(Vec<RgbaImage>, RgbaImage)> {
    if monitors.is_empty() {
        return Err(AppError::Wayland(
            "No monitors provided to capture".to_string(),
        ));
    }

    let names: Vec<&str> = monitors.iter().map(|m| m.name.as_str()).collect();

    capture_frames(&names, |frames| {
        // All monitors are matched (unmatched check above), so bounding box from monitors is safe.
        let (min_x, min_y, max_x, max_y) = monitors.iter().fold(
            (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
            |(mx, my, xx, xy), m| {
                (
                    mx.min(m.rect.x),
                    my.min(m.rect.y),
                    xx.max(m.rect.x + m.rect.w),
                    xy.max(m.rect.y + m.rect.h),
                )
            },
        );

        let total_width = (max_x - min_x).max(0) as u32;
        let total_height = (max_y - min_y).max(0) as u32;
        let mut master_img = ImageBuffer::new(total_width, total_height);

        // Slot for per-monitor physical images, indexed by position in `monitors`.
        let mut physical_images: Vec<Option<RgbaImage>> = vec![None; monitors.len()];

        for fi in frames {
            let (mon_idx, mon_info) = monitors
                .iter()
                .enumerate()
                .find(|(_, m)| m.name == fi.name)
                .ok_or_else(|| {
                    AppError::Wayland(format!("Monitor info missing for '{}'", fi.name))
                })?;

            // --- Physical-resolution image (for HiDPI overlay) ---
            let phys_img = extract_image(fi)?;
            physical_images[mon_idx] = Some(phys_img);

            let mmap = fi
                .mmap
                .as_ref()
                .ok_or_else(|| AppError::Screencopy("buffer missing".to_string()))?;
            let format = fi
                .format
                .ok_or_else(|| AppError::Screencopy("format not set".to_string()))?;

            // --- Logical-space composite ---
            let offset_x = (mon_info.rect.x - min_x) as u32;
            let offset_y = (mon_info.rect.y - min_y) as u32;
            let log_w = mon_info.rect.w;
            let log_h = mon_info.rect.h;

            if log_w <= 0 || log_h <= 0 {
                return Err(AppError::Wayland(format!(
                    "Monitor '{}' has invalid dimensions ({}x{}) in Hyprland IPC data",
                    mon_info.name, log_w, log_h
                )));
            }
            let log_w = log_w as u32;
            let log_h = log_h as u32;

            // Pre-compute the logical→physical index mapping for each axis.
            let phys_xs: Vec<u32> = (0..log_w)
                .map(|lx| {
                    ((lx as u64 * fi.width as u64 / log_w as u64) as u32)
                        .min(fi.width.saturating_sub(1))
                })
                .collect();
            let phys_ys: Vec<u32> = (0..log_h)
                .map(|ly| {
                    ((ly as u64 * fi.height as u64 / log_h as u64) as u32)
                        .min(fi.height.saturating_sub(1))
                })
                .collect();

            for (ly, &py) in phys_ys.iter().enumerate() {
                let row_offset = py as usize * fi.stride as usize;
                for (lx, &px) in phys_xs.iter().enumerate() {
                    let offset = row_offset + px as usize * 4;
                    master_img.put_pixel(
                        offset_x + lx as u32,
                        offset_y + ly as u32,
                        read_pixel_rgba(mmap, offset, format),
                    );
                }
            }
        }

        let physical_images: Vec<RgbaImage> = physical_images
            .into_iter()
            .enumerate()
            .map(|(i, opt)| {
                opt.ok_or_else(|| {
                    AppError::Wayland(format!(
                        "Physical image missing for monitor '{}'",
                        monitors[i].name
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok((physical_images, master_img))
    })
}

/// Crop an RGBA image buffer to an optional region and save to `dst`.
/// If `region` is `None`, saves the full image as-is.
/// Region bounds are clamped to image dimensions.
pub fn crop_and_save(
    img: image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    region: Option<ScreenRect>,
    dst: &std::path::Path,
) -> Result<()> {
    let cropped = match region {
        None => image::DynamicImage::ImageRgba8(img),
        Some(r) => {
            let x = r.x.max(0) as u32;
            let y = r.y.max(0) as u32;
            let w = (r.w as u32).min(img.width().saturating_sub(x));
            let h = (r.h as u32).min(img.height().saturating_sub(y));
            image::DynamicImage::ImageRgba8(image::imageops::crop_imm(&img, x, y, w, h).to_image())
        }
    };
    cropped
        .save(dst)
        .map_err(crate::domain::error::AppError::from)
}
