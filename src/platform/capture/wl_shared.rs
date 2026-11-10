//! # platform::capture::wl_shared
//!
//! Utilities shared across Wayland-protocol capture backends.
//!
//! All three public items were previously duplicated between `screencopy` and
//! `toplevel_export`; they live here to keep each backend focused on its own
//! protocol dialect.

use std::{
    os::fd::AsFd,
    time::{Duration, Instant},
};

use image::Rgba;
use memmap2::MmapMut;
use nix::{
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::memfd,
};
use wayland_client::{
    Dispatch, EventQueue, QueueHandle, WEnum,
    protocol::{wl_buffer, wl_shm, wl_shm_pool},
};

use crate::domain::error::{AppError, Result};

// ── dispatch_until ─────────────────────────────────────────────────────────────

/// Drive a Wayland event queue until `done` returns `true` or `timeout` elapses.
///
/// Uses `prepare_read` + `poll` so the thread yields to the OS rather than
/// spinning, and returns a timeout error if the compositor stops responding.
pub fn dispatch_until<S>(
    event_queue: &mut EventQueue<S>,
    state: &mut S,
    timeout: Duration,
    mut done: impl FnMut(&S) -> bool,
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
                "Wayland capture timed out: compositor did not respond in time".to_string(),
            ));
        }

        event_queue
            .flush()
            .map_err(|e| AppError::Wayland(format!("Wayland flush failed: {e}")))?;

        // Prepare a socket read. Guard and fd are both immutable borrows of
        // event_queue; they coexist safely since Rust allows multiple &self
        // borrows simultaneously. Drop pollfds (and its BorrowedFd) before
        // consuming the guard — the borrow checker requires both immutable
        // borrows to end before the next dispatch_pending(&mut self).
        let timeout_ms = remaining.as_millis().min(u16::MAX as u128) as u16;
        let guard = event_queue.prepare_read();
        {
            let fd = event_queue.as_fd();
            let mut pollfds = [PollFd::new(fd, PollFlags::POLLIN)];
            poll(&mut pollfds, PollTimeout::from(timeout_ms))
                .map_err(|e| AppError::Wayland(format!("poll failed: {e}")))?;
            // pollfds (and BorrowedFd) dropped here
        }
        if let Some(g) = guard {
            g.read()
                .map_err(|e| AppError::Wayland(format!("Wayland read failed: {e}")))?;
        }
    }
}

// ── read_pixel_rgba ────────────────────────────────────────────────────────────

/// Convert a pixel at byte `offset` in `data` to RGBA based on the wl_shm format.
///
/// Wayland shm format memory layout (little-endian):
/// - ARGB8888: bytes = [Blue, Green, Red, Alpha]  → real alpha
/// - XRGB8888: bytes = [Blue, Green, Red, X]      → alpha forced to 255
/// - ABGR8888: bytes = [Red, Green, Blue, Alpha]  → real alpha
/// - XBGR8888: bytes = [Red, Green, Blue, X]      → alpha forced to 255
///
/// Non-panicking: if the buffer is too small, logs a warning and returns
/// transparent black.
#[inline]
pub fn read_pixel_rgba(data: &[u8], offset: usize, format: WEnum<wl_shm::Format>) -> Rgba<u8> {
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
        // XRGB8888: [B, G, R, X] → alpha forced 255
        WEnum::Value(wl_shm::Format::Xrgb8888) => Rgba([b2, b1, b0, 255]),
        // ABGR8888: [R, G, B, A] → real alpha
        WEnum::Value(wl_shm::Format::Abgr8888) => Rgba([b0, b1, b2, b3]),
        // XBGR8888: [R, G, B, X] → alpha forced 255
        WEnum::Value(wl_shm::Format::Xbgr8888) => Rgba([b0, b1, b2, 255]),
        // Defensive fallback: the Buffer event handler whitelists supported
        // formats, so this branch should never be reached in practice.
        _ => {
            eprintln!(
                "read_pixel_rgba: unsupported wl_shm format {format:?}, falling back to ARGB8888 layout"
            );
            Rgba([b2, b1, b0, b3])
        }
    }
}

// ── Row-wise RGBA conversion ──────────────────────────────────────────────────

/// Return the number of bytes occupied by one pixel in a supported wl_shm format.
#[inline]
pub fn bytes_per_pixel(format: WEnum<wl_shm::Format>) -> usize {
    match format {
        WEnum::Value(
            wl_shm::Format::Argb8888
            | wl_shm::Format::Xrgb8888
            | wl_shm::Format::Abgr8888
            | wl_shm::Format::Xbgr8888,
        ) => 4,
        _ => 0,
    }
}

/// Append one tightly-packed source row converted to RGBA to `out`.
///
/// `out` receives exactly `width * 4` bytes and is cleared first. The caller is
/// responsible for slicing away stride padding; only `width * bytes_per_pixel`
/// bytes are read from `data`.
///
/// The channel layout is selected once per row, so the inner loop has no
/// per-pixel format branch (unlike [`read_pixel_rgba`]).
pub fn rgba_row(
    data: &[u8],
    width: u32,
    format: WEnum<wl_shm::Format>,
    out: &mut Vec<u8>,
) -> Result<()> {
    let pixel_size = bytes_per_pixel(format);
    if pixel_size == 0 {
        return Err(AppError::Wayland(format!(
            "unsupported wl_shm format for RGBA conversion: {format:?}"
        )));
    }

    let needed = (width as usize)
        .checked_mul(pixel_size)
        .ok_or_else(|| AppError::Wayland(format!("row width overflow: {width} px")))?;
    if data.len() < needed {
        return Err(AppError::Wayland(format!(
            "row buffer too small: {} bytes for {width} px at {pixel_size} bpp",
            data.len()
        )));
    }

    let data = &data[..needed];
    out.clear();
    out.reserve(width as usize * 4);

    match format {
        // ARGB8888: [B, G, R, A]
        WEnum::Value(wl_shm::Format::Argb8888) => {
            for px in data.as_chunks::<4>().0 {
                out.push(px[2]);
                out.push(px[1]);
                out.push(px[0]);
                out.push(px[3]);
            }
        }
        // XRGB8888: [B, G, R, X] → opaque
        WEnum::Value(wl_shm::Format::Xrgb8888) => {
            for px in data.as_chunks::<4>().0 {
                out.push(px[2]);
                out.push(px[1]);
                out.push(px[0]);
                out.push(255);
            }
        }
        // ABGR8888: [R, G, B, A]
        WEnum::Value(wl_shm::Format::Abgr8888) => {
            for px in data.as_chunks::<4>().0 {
                out.push(px[0]);
                out.push(px[1]);
                out.push(px[2]);
                out.push(px[3]);
            }
        }
        // XBGR8888: [R, G, B, X] → opaque
        WEnum::Value(wl_shm::Format::Xbgr8888) => {
            for px in data.as_chunks::<4>().0 {
                out.push(px[0]);
                out.push(px[1]);
                out.push(px[2]);
                out.push(255);
            }
        }
        _ => unreachable!("bytes_per_pixel rejected unsupported formats"),
    }

    debug_assert_eq!(out.len(), width as usize * 4);
    Ok(())
}

/// Convert a whole wl_shm buffer to tightly-packed RGBA, honouring stride padding.
///
/// Returns `width * height * 4` bytes. This is the fast path used by freeze mode
/// to produce the overlay's physical-resolution image in one row-wise pass.
pub fn rgba_pixels(
    data: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    format: WEnum<wl_shm::Format>,
) -> Result<Vec<u8>> {
    let pixel_size = bytes_per_pixel(format);
    if pixel_size == 0 {
        return Err(AppError::Wayland(format!(
            "unsupported wl_shm format for RGBA conversion: {format:?}"
        )));
    }

    let row_bytes = (width as usize)
        .checked_mul(pixel_size)
        .ok_or_else(|| AppError::Wayland(format!("row width overflow: {width} px")))?;
    let stride = stride as usize;
    if stride < row_bytes {
        return Err(AppError::Wayland(format!(
            "invalid stride: stride={stride} width={width} (expected ≥{row_bytes})"
        )));
    }
    let total = stride
        .checked_mul(height as usize)
        .ok_or_else(|| AppError::Wayland(format!("buffer size overflow: {stride} × {height}")))?;
    if data.len() < total {
        return Err(AppError::Wayland(format!(
            "buffer too small: {} bytes, need {total}",
            data.len()
        )));
    }

    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    let mut row = Vec::with_capacity(width as usize * 4);
    for y in 0..height as usize {
        let start = y * stride;
        rgba_row(&data[start..start + row_bytes], width, format, &mut row)?;
        out.extend_from_slice(&row);
    }
    Ok(out)
}

// ── alloc_shm_buffer ──────────────────────────────────────────────────────────

/// Allocate a shared-memory buffer for a Wayland screencopy or toplevel-export frame.
///
/// Performs: stride validation → size computation → `memfd_create` →
/// `ftruncate` → `mmap` → `wl_shm.create_pool` → `pool.create_buffer` →
/// `pool.destroy`.
///
/// # Errors
/// Returns `AppError::Wayland` if any step fails (invalid dimensions, kernel
/// allocation failure, etc.).
pub fn alloc_shm_buffer<S>(
    shm: &wl_shm::WlShm,
    width: u32,
    height: u32,
    stride: u32,
    format: wl_shm::Format,
    qh: &QueueHandle<S>,
) -> Result<(MmapMut, wl_buffer::WlBuffer)>
where
    S: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, ()> + 'static,
{
    let stride_usize = stride as usize;
    let width_usize = width as usize;
    let height_usize = height as usize;

    let bytes_per_row = width_usize.checked_mul(4).ok_or_else(|| {
        AppError::Wayland(format!(
            "invalid buffer dimensions: width {width} * 4 overflows usize"
        ))
    })?;
    if stride_usize < bytes_per_row || !stride_usize.is_multiple_of(4) {
        return Err(AppError::Wayland(format!(
            "invalid stride: stride={stride} width={width} (expected ≥{bytes_per_row} and multiple of 4)"
        )));
    }

    let size = stride_usize.checked_mul(height_usize).ok_or_else(|| {
        AppError::Wayland(format!(
            "buffer size overflow: stride={stride} * height={height}"
        ))
    })?;
    let pool_size_i32 = i32::try_from(size).map_err(|_| {
        AppError::Wayland(format!(
            "shm pool size too large: {size} bytes (max {})",
            i32::MAX
        ))
    })?;

    let fd = memfd::memfd_create(c"wl_shm_buffer", memfd::MFdFlags::MFD_CLOEXEC)
        .map_err(|e| AppError::Wayland(format!("memfd_create failed: {e}")))?;
    nix::unistd::ftruncate(&fd, size as i64)
        .map_err(|e| AppError::Wayland(format!("ftruncate failed: {e}")))?;

    let file = std::fs::File::from(fd);
    let mmap = unsafe { MmapMut::map_mut(&file) }
        .map_err(|e| AppError::Wayland(format!("mmap failed: {e}")))?;

    let pool = shm.create_pool(file.as_fd(), pool_size_i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        format,
        qh,
        (),
    );
    pool.destroy();

    Ok((mmap, buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_supported_pixel_sizes() {
        assert_eq!(bytes_per_pixel(WEnum::Value(wl_shm::Format::Xrgb8888)), 4);
        assert_eq!(bytes_per_pixel(WEnum::Value(wl_shm::Format::Argb8888)), 4);
        assert_eq!(bytes_per_pixel(WEnum::Value(wl_shm::Format::Bgr888)), 0);
    }

    #[test]
    fn rgba_row_matches_pixel_decoder_for_all_formats() {
        let cases: &[(wl_shm::Format, [u8; 4], Rgba<u8>)] = &[
            (
                wl_shm::Format::Argb8888,
                [0x10, 0x20, 0x30, 0x40],
                Rgba([0x30, 0x20, 0x10, 0x40]),
            ),
            (
                wl_shm::Format::Xrgb8888,
                [0x10, 0x20, 0x30, 0x40],
                Rgba([0x30, 0x20, 0x10, 255]),
            ),
            (
                wl_shm::Format::Abgr8888,
                [0x10, 0x20, 0x30, 0x40],
                Rgba([0x10, 0x20, 0x30, 0x40]),
            ),
            (
                wl_shm::Format::Xbgr8888,
                [0x10, 0x20, 0x30, 0x40],
                Rgba([0x10, 0x20, 0x30, 255]),
            ),
        ];

        for (format, bytes, expected) in cases {
            let format = WEnum::Value(*format);
            let mut out = vec![0xaa; 8];
            rgba_row(bytes, 1, format, &mut out).unwrap();
            assert_eq!(out, expected.0.to_vec(), "row conversion for {format:?}");

            let pixels = rgba_pixels(bytes, 1, 1, 4, format).unwrap();
            assert_eq!(
                pixels,
                expected.0.to_vec(),
                "buffer conversion for {format:?}"
            );
        }
    }

    #[test]
    fn rgba_pixels_ignores_stride_padding() {
        // Two pixels per row, stride padded to 12 bytes, two rows.
        let data = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0xff, 0xff, 0xff, 0xff, 0x09, 0x0a,
            0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0xff, 0xff, 0xff, 0xff,
        ];
        let pixels = rgba_pixels(&data, 2, 2, 12, WEnum::Value(wl_shm::Format::Xbgr8888)).unwrap();
        assert_eq!(
            pixels,
            vec![1, 2, 3, 255, 5, 6, 7, 255, 9, 10, 11, 255, 13, 14, 15, 255,]
        );
    }

    #[test]
    fn rgba_conversion_rejects_invalid_buffers() {
        let format = WEnum::Value(wl_shm::Format::Xrgb8888);
        assert!(rgba_row(&[0, 0, 0], 1, format, &mut Vec::new()).is_err());
        assert!(rgba_pixels(&[0; 8], 2, 2, 4, format).is_err());
        assert!(rgba_pixels(&[0; 8], 1, 1, 3, format).is_err());
    }
}
