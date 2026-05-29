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
