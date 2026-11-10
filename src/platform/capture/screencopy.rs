use wayland_client::{
    Dispatch, QueueHandle, WEnum, delegate_noop,
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
use std::time::Duration;

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

use super::wl_shared;

/// Initialize a Wayland connection, discover globals, and resolve xdg-output names.
fn init_wayland() -> Result<(wayland_client::EventQueue<CaptureState>, CaptureState)> {
    let conn = wayland_client::Connection::connect_to_env()
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

// --- Dispatch impls ---

impl Dispatch<wl_registry::WlRegistry, ()> for CaptureState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &wayland_client::Connection,
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

delegate_noop!(CaptureState: ignore wl_output::WlOutput);
delegate_noop!(CaptureState: ignore wl_shm::WlShm);
delegate_noop!(CaptureState: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1);
delegate_noop!(CaptureState: zxdg_output_manager_v1::ZxdgOutputManagerV1);

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        xdg_output: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
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
        _: &wayland_client::Connection,
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

                let shm = match state.shm.as_ref() {
                    Some(s) => s.clone(),
                    None => {
                        state.frames[fi_idx].failed = true;
                        state.frames[fi_idx].error_msg =
                            Some("wl_shm global not available".to_string());
                        return;
                    }
                };

                match wl_shared::alloc_shm_buffer(&shm, width, height, stride, shm_format, qh) {
                    Ok((mmap, buffer)) => {
                        frame.copy(&buffer);
                        let fi = &mut state.frames[fi_idx];
                        fi.format = Some(WEnum::Value(shm_format));
                        fi.width = width;
                        fi.height = height;
                        fi.stride = stride;
                        fi.buffer = Some(buffer);
                        fi.mmap = Some(mmap);
                    }
                    Err(e) => {
                        let fi = &mut state.frames[fi_idx];
                        fi.failed = true;
                        fi.error_msg = Some(e.to_string());
                    }
                }
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

delegate_noop!(CaptureState: wl_shm_pool::WlShmPool);
delegate_noop!(CaptureState: ignore wl_buffer::WlBuffer);

// --- Public capture API ---

/// A physical-resolution monitor capture, tightly packed as RGBA.
///
/// The pixels are stored without stride padding so they can be moved directly
/// into an iced image handle or sampled by deferred composition.
#[derive(Debug)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl CapturedFrame {
    /// Borrow this capture for deferred composition.
    pub fn as_ref(&self) -> FrameRef<'_> {
        FrameRef {
            width: self.width,
            height: self.height,
            pixels: &self.pixels,
        }
    }
}

/// Borrowed physical-resolution RGBA pixels used to compose the final image.
#[derive(Clone, Copy)]
pub struct FrameRef<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
}

/// Type alias to reduce verbosity of per-monitor capture return types.
pub type RgbaImage = ImageBuffer<Rgba<u8>, Vec<u8>>;

/// Convert one captured frame to a tightly-packed RGBA buffer.
fn frame_rgba(fi: &FrameInfo) -> Result<Vec<u8>> {
    let mmap = fi
        .mmap
        .as_ref()
        .ok_or_else(|| AppError::Screencopy(format!("buffer missing for monitor '{}'", fi.name)))?;
    let format = fi
        .format
        .ok_or_else(|| AppError::Screencopy(format!("format not set for monitor '{}'", fi.name)))?;

    wl_shared::rgba_pixels(mmap, fi.width, fi.height, fi.stride, format)
        .map_err(|e| AppError::Screencopy(format!("monitor '{}': {e}", fi.name)))
}

/// Helper to extract a physical ImageBuffer from a captured frame.
fn extract_image(fi: &FrameInfo) -> Result<RgbaImage> {
    let pixels = frame_rgba(fi)?;
    RgbaImage::from_raw(fi.width, fi.height, pixels).ok_or_else(|| {
        AppError::Screencopy(format!(
            "invalid image dimensions for monitor '{}'",
            fi.name
        ))
    })
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

    wl_shared::dispatch_until(&mut event_queue, &mut state, Duration::from_secs(10), |s| {
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

/// Logical bounding box of all monitors: `(min_x, min_y, width, height)`.
fn logical_bounds(monitors: &[MonitorInfo]) -> Result<(i32, i32, u32, u32)> {
    if monitors.is_empty() {
        return Err(AppError::Wayland(
            "No monitors provided to capture".to_string(),
        ));
    }

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

    let width = (max_x - min_x).max(0) as u32;
    let height = (max_y - min_y).max(0) as u32;
    if width == 0 || height == 0 {
        return Err(AppError::Wayland(
            "Monitor layout has zero area".to_string(),
        ));
    }

    Ok((min_x, min_y, width, height))
}

/// Capture all monitors in a single session and return physical-resolution
/// frames in the same order as `monitors`.
///
/// No logical composite is built: freeze mode shows these frames directly and
/// only composes the region the user selects, after the overlay closes.
pub fn capture_all_monitors_physical(monitors: &[MonitorInfo]) -> Result<Vec<CapturedFrame>> {
    let names: Vec<&str> = monitors.iter().map(|m| m.name.as_str()).collect();
    if names.is_empty() {
        return Err(AppError::Wayland(
            "No monitors provided to capture".to_string(),
        ));
    }

    capture_frames(&names, |frames| {
        let mut captures: Vec<Option<CapturedFrame>> = (0..monitors.len()).map(|_| None).collect();

        for fi in frames {
            let index = monitors
                .iter()
                .position(|m| m.name == fi.name)
                .ok_or_else(|| {
                    AppError::Wayland(format!("Monitor info missing for '{}'", fi.name))
                })?;

            captures[index] = Some(CapturedFrame {
                width: fi.width,
                height: fi.height,
                pixels: frame_rgba(fi)?,
            });
        }

        captures
            .into_iter()
            .enumerate()
            .map(|(index, capture)| {
                capture.ok_or_else(|| {
                    AppError::Wayland(format!(
                        "Physical image missing for monitor '{}'",
                        monitors[index].name
                    ))
                })
            })
            .collect()
    })
}

/// Compose the full logical-space image directly from captured shared memory.
///
/// Used by non-freeze `all`/`crop` captures. Unlike freeze mode these commands
/// only need the composite, so no physical-resolution copy is materialised.
fn compose_from_frames(frames: &[FrameInfo], monitors: &[MonitorInfo]) -> Result<RgbaImage> {
    let (min_x, min_y, total_w, total_h) = logical_bounds(monitors)?;
    let mut output = vec![0u8; total_w as usize * total_h as usize * 4];

    for fi in frames {
        let mon = monitors
            .iter()
            .find(|m| m.name == fi.name)
            .ok_or_else(|| AppError::Wayland(format!("Monitor info missing for '{}'", fi.name)))?;

        if mon.rect.w <= 0 || mon.rect.h <= 0 {
            return Err(AppError::Wayland(format!(
                "Monitor '{}' has invalid dimensions ({}x{}) in Hyprland IPC data",
                mon.name, mon.rect.w, mon.rect.h
            )));
        }

        let mmap = fi
            .mmap
            .as_ref()
            .ok_or_else(|| AppError::Screencopy("buffer missing".to_string()))?;
        let format = fi
            .format
            .ok_or_else(|| AppError::Screencopy("format not set".to_string()))?;
        let pixel_size = wl_shared::bytes_per_pixel(format);
        if pixel_size == 0 {
            return Err(AppError::Screencopy(format!(
                "unsupported format for monitor '{}'",
                fi.name
            )));
        }

        let log_w = mon.rect.w as usize;
        let log_h = mon.rect.h as usize;
        let phys_xs: Vec<usize> = (0..log_w)
            .map(|lx| {
                ((lx as u64 * fi.width as u64 / log_w as u64) as usize)
                    .min(fi.width.saturating_sub(1) as usize)
            })
            .collect();

        let mut row = Vec::with_capacity(fi.width as usize * 4);
        for ly in 0..log_h {
            let py = ((ly as u64 * fi.height as u64 / log_h as u64) as usize)
                .min(fi.height.saturating_sub(1) as usize);
            let row_offset = py * fi.stride as usize;
            let row_bytes = fi.width as usize * pixel_size;
            let src = mmap
                .get(row_offset..row_offset + row_bytes)
                .ok_or_else(|| {
                    AppError::Screencopy(format!(
                        "buffer too small for monitor '{}' row {py}",
                        fi.name
                    ))
                })?;

            wl_shared::rgba_row(src, fi.width, format, &mut row)?;

            let dst_row = ((mon.rect.y - min_y) as usize + ly) * total_w as usize * 4;
            let dst_col = (mon.rect.x - min_x) as usize;
            for (lx, &px) in phys_xs.iter().enumerate() {
                let dst = dst_row + (dst_col + lx) * 4;
                let src = px * 4;
                output[dst..dst + 4].copy_from_slice(&row[src..src + 4]);
            }
        }
    }

    RgbaImage::from_raw(total_w, total_h, output)
        .ok_or_else(|| AppError::Screencopy("failed to build logical composite".to_string()))
}

/// Capture all monitors and composite them into a single image in **logical pixel space**.
///
/// The output dimensions and pixel coordinates match what Hyprland IPC and slurp report,
/// so crop coordinates can be applied directly without coordinate conversion.
/// HiDPI monitors are downsampled to their logical size during compositing.
pub fn capture_all_monitors(monitors: &[MonitorInfo]) -> Result<RgbaImage> {
    let names: Vec<&str> = monitors.iter().map(|m| m.name.as_str()).collect();
    if names.is_empty() {
        return Err(AppError::Wayland(
            "No monitors provided to capture".to_string(),
        ));
    }

    capture_frames(&names, |frames| compose_from_frames(frames, monitors))
}

/// Compose an optional region of the logical-space image from physical frames.
///
/// `frames` must be in the same order as `monitors`. Scanning proceeds monitor by
/// monitor, so only the requested pixels are converted and no full-screen
/// composite is allocated. Overlapping monitors behave like the previous
/// full-composite implementation: later monitors overwrite earlier ones, and
/// uncovered gaps stay transparent black.
pub fn compose_region(
    monitors: &[MonitorInfo],
    frames: &[FrameRef<'_>],
    region: Option<ScreenRect>,
) -> Result<RgbaImage> {
    if frames.len() != monitors.len() {
        return Err(AppError::Other(format!(
            "capture frame count ({}) does not match monitor count ({})",
            frames.len(),
            monitors.len()
        )));
    }

    let (min_x, min_y, total_w, total_h) = logical_bounds(monitors)?;

    let (x, y, width, height) = match region {
        None => (0, 0, total_w, total_h),
        Some(r) => {
            let x = r.x.max(0) as u32;
            let y = r.y.max(0) as u32;
            let width = (r.w.max(0) as u32).min(total_w.saturating_sub(x));
            let height = (r.h.max(0) as u32).min(total_h.saturating_sub(y));
            (x, y, width, height)
        }
    };

    if width == 0 || height == 0 {
        return Err(AppError::Other(
            "selected region is empty or outside the captured monitors".to_string(),
        ));
    }

    let region_x = min_x + x as i32;
    let region_y = min_y + y as i32;
    let region_right = region_x + width as i32;
    let region_bottom = region_y + height as i32;
    let mut output = vec![0u8; width as usize * height as usize * 4];

    for (monitor, frame) in monitors.iter().zip(frames) {
        if monitor.rect.w <= 0 || monitor.rect.h <= 0 {
            return Err(AppError::Wayland(format!(
                "Monitor '{}' has invalid dimensions ({}x{}) in Hyprland IPC data",
                monitor.name, monitor.rect.w, monitor.rect.h
            )));
        }
        if frame.width == 0 || frame.height == 0 {
            return Err(AppError::Screencopy(format!(
                "Monitor '{}' has an empty capture frame",
                monitor.name
            )));
        }

        let expected = frame.width as usize * frame.height as usize * 4;
        if frame.pixels.len() < expected {
            return Err(AppError::Screencopy(format!(
                "Monitor '{}' capture buffer is too small ({} < {expected})",
                monitor.name,
                frame.pixels.len()
            )));
        }

        // Intersect the monitor with the requested region in image space.
        let x_start = monitor.rect.x.max(region_x);
        let y_start = monitor.rect.y.max(region_y);
        let x_end = (monitor.rect.x + monitor.rect.w).min(region_right);
        let y_end = (monitor.rect.y + monitor.rect.h).min(region_bottom);
        if x_start >= x_end || y_start >= y_end {
            continue;
        }

        let log_w = monitor.rect.w as usize;
        let log_h = monitor.rect.h as usize;
        let phys_xs: Vec<usize> = (0..log_w)
            .map(|lx| {
                ((lx as u64 * frame.width as u64 / log_w as u64) as usize)
                    .min(frame.width as usize - 1)
            })
            .collect();

        for gy in y_start..y_end {
            let ly = (gy - monitor.rect.y) as usize;
            let py = ((ly as u64 * frame.height as u64 / log_h as u64) as usize)
                .min(frame.height as usize - 1);
            let src_row = py * frame.width as usize * 4;
            let dst_row = (gy - region_y) as usize * width as usize * 4;

            for gx in x_start..x_end {
                let lx = (gx - monitor.rect.x) as usize;
                let px = phys_xs[lx];
                let src = src_row + px * 4;
                let dst = dst_row + (gx - region_x) as usize * 4;
                output[dst..dst + 4].copy_from_slice(&frame.pixels[src..src + 4]);
            }
        }
    }

    RgbaImage::from_raw(width, height, output)
        .ok_or_else(|| AppError::Screencopy("failed to compose selected region".to_string()))
}

/// Compose the requested region from physical frames and save it to `dst`.
///
/// The frames are the same buffers used by the freeze overlay, so the saved
/// image always matches the frozen frame the user selected.
pub fn save_composed_region(
    monitors: &[MonitorInfo],
    frames: &[FrameRef<'_>],
    region: Option<ScreenRect>,
    dst: &std::path::Path,
) -> Result<()> {
    let image = compose_region(monitors, frames, region)?;
    image.save(dst).map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, x: i32, y: i32, w: i32, h: i32) -> MonitorInfo {
        MonitorInfo {
            rect: ScreenRect { x, y, w, h },
            name: name.to_string(),
            focused: false,
            active_workspace_id: 1,
        }
    }

    fn solid(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        color.repeat(width as usize * height as usize)
    }

    fn frame(width: u32, height: u32, pixels: &[u8]) -> FrameRef<'_> {
        FrameRef {
            width,
            height,
            pixels,
        }
    }

    fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        image.get_pixel(x, y).0
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn compose_handles_negative_origins_and_hidpi() {
        let monitors = vec![
            monitor("LEFT", -100, 0, 100, 100),
            monitor("RIGHT", 0, 0, 100, 100),
        ];
        let left = solid(100, 100, RED);
        let right = solid(200, 200, BLUE);
        let frames = [frame(100, 100, &left), frame(200, 200, &right)];

        let image = compose_region(&monitors, &frames, None).unwrap();

        assert_eq!(image.dimensions(), (200, 100));
        assert_eq!(pixel(&image, 0, 0), RED);
        assert_eq!(pixel(&image, 99, 50), RED);
        assert_eq!(pixel(&image, 100, 0), BLUE);
        assert_eq!(pixel(&image, 199, 99), BLUE);
    }

    #[test]
    fn compose_region_can_span_multiple_monitors() {
        let monitors = vec![
            monitor("LEFT", -100, 0, 100, 100),
            monitor("RIGHT", 0, 0, 100, 100),
        ];
        let left = solid(100, 100, RED);
        let right = solid(100, 100, BLUE);
        let frames = [frame(100, 100, &left), frame(100, 100, &right)];

        let region = ScreenRect {
            x: 50,
            y: 0,
            w: 100,
            h: 10,
        };
        let image = compose_region(&monitors, &frames, Some(region)).unwrap();

        assert_eq!(image.dimensions(), (100, 10));
        assert_eq!(pixel(&image, 0, 0), RED);
        assert_eq!(pixel(&image, 49, 0), RED);
        assert_eq!(pixel(&image, 50, 0), BLUE);
        assert_eq!(pixel(&image, 99, 9), BLUE);
    }

    #[test]
    fn compose_region_scales_logical_coordinates_to_physical_pixels() {
        let monitors = vec![monitor("HIDPI", 0, 0, 4, 2)];
        let mut pixels = Vec::with_capacity(8 * 4 * 4);
        for y in 0..4u8 {
            for x in 0..8u8 {
                pixels.extend_from_slice(&[x, y, 0, 255]);
            }
        }
        let frames = [frame(8, 4, &pixels)];

        let image = compose_region(&monitors, &frames, None).unwrap();

        assert_eq!(image.dimensions(), (4, 2));
        for ly in 0..2 {
            for lx in 0..4 {
                assert_eq!(
                    pixel(&image, lx, ly),
                    [(lx * 2) as u8, (ly * 2) as u8, 0, 255],
                    "logical pixel ({lx},{ly})"
                );
            }
        }
    }

    #[test]
    fn compose_region_clamps_and_rejects_invalid_requests() {
        let monitors = vec![monitor("MAIN", 0, 0, 10, 10)];
        let pixels = solid(10, 10, RED);
        let frames = [frame(10, 10, &pixels)];

        let clamped = compose_region(
            &monitors,
            &frames,
            Some(ScreenRect {
                x: -5,
                y: 0,
                w: 20,
                h: 5,
            }),
        )
        .unwrap();
        assert_eq!(clamped.dimensions(), (10, 5));

        assert!(
            compose_region(
                &monitors,
                &frames,
                Some(ScreenRect {
                    x: 20,
                    y: 0,
                    w: 5,
                    h: 5,
                }),
            )
            .is_err()
        );
        assert!(compose_region(&monitors, &[], None).is_err());
    }

    #[test]
    fn compose_region_keeps_uncovered_gaps_transparent() {
        let monitors = vec![
            monitor("LEFT", 0, 0, 10, 10),
            monitor("RIGHT", 20, 0, 10, 10),
        ];
        let left = solid(10, 10, RED);
        let right = solid(10, 10, BLUE);
        let frames = [frame(10, 10, &left), frame(10, 10, &right)];

        let image = compose_region(&monitors, &frames, None).unwrap();

        assert_eq!(image.dimensions(), (30, 10));
        assert_eq!(pixel(&image, 5, 5), RED);
        assert_eq!(pixel(&image, 15, 5), [0, 0, 0, 0]);
        assert_eq!(pixel(&image, 25, 5), BLUE);
    }
}
