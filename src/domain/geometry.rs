//! # domain::geometry
//!
//! Pure coordinate transformation and parsing — no side effects.
//!
//! | Function | Description |
//! |---|---|
//! | `parse_slurp_geometry` | Parses the `"X,Y WxH"` string returned by `slurp` into numeric values |
//! | `monitor_origin` | Finds the minimum (x, y) across all monitors (multi-monitor origin) |
//! | `logical_to_physical` | Converts logical pixel coordinates to physical pixels using a scale factor |
//! | `clamp_crop` | Clamps a crop region so it does not exceed monitor boundaries |

use crate::domain::error::{AppError, Result};
use crate::domain::types::MonitorInfo;

/// Parse a slurp geometry string (`"X,Y WxH"`) into `(x, y, w, h)`.
///
/// `x` and `y` are signed because multi-monitor setups can place monitors at
/// negative logical coordinates (e.g. a monitor to the left of the primary at `−1920,0`).
pub fn parse_slurp_geometry(geom: &str) -> Result<(i32, i32, u32, u32)> {
    let (pos_part, size_part) = geom
        .trim()
        .split_once(' ')
        .ok_or_else(|| AppError::Other("Invalid geometry string: missing space".to_string()))?;

    let (x_str, y_str) = pos_part.trim().split_once(',').ok_or_else(|| {
        AppError::Other("Invalid geometry string: missing comma in position".to_string())
    })?;

    let (w_str, h_str) = size_part.trim().split_once('x').ok_or_else(|| {
        AppError::Other("Invalid geometry string: missing 'x' in size".to_string())
    })?;

    let x = x_str
        .trim()
        .parse::<i32>()
        .map_err(|_| AppError::Other(format!("Invalid x in geometry: '{}'", x_str)))?;
    let y = y_str
        .trim()
        .parse::<i32>()
        .map_err(|_| AppError::Other(format!("Invalid y in geometry: '{}'", y_str)))?;
    let w = w_str
        .trim()
        .parse::<u32>()
        .map_err(|_| AppError::Other(format!("Invalid w in geometry: '{}'", w_str)))?;
    let h = h_str
        .trim()
        .parse::<u32>()
        .map_err(|_| AppError::Other(format!("Invalid h in geometry: '{}'", h_str)))?;

    Ok((x, y, w, h))
}

/// Compute the global logical origin (min_x, min_y) across all monitors.
///
/// `capture_all_monitors` places this point at image pixel (0, 0).
/// Subtract this origin from global logical coordinates to get image-space coordinates.
/// Returns `(0, 0)` if the slice is empty.
pub fn monitor_origin(monitors: &[MonitorInfo]) -> (i32, i32) {
    let min_x = monitors.iter().map(|m| m.rect.x).min().unwrap_or(0);
    let min_y = monitors.iter().map(|m| m.rect.y).min().unwrap_or(0);
    (min_x, min_y)
}

/// Clamp a crop rectangle to image bounds.
///
/// Returns `(clamped_w, clamped_h, was_clamped)`.
/// `was_clamped` is `true` when the requested size exceeded the image, which
/// callers should surface as a warning — silent clamping hides crop mistakes.
pub fn clamp_crop(x: u32, y: u32, w: u32, h: u32, img_w: u32, img_h: u32) -> (u32, u32, bool) {
    let clamped_w = w.min(img_w.saturating_sub(x));
    let clamped_h = h.min(img_h.saturating_sub(y));
    (clamped_w, clamped_h, clamped_w != w || clamped_h != h)
}

/// Scale a single logical-space coordinate or dimension to physical pixels.
///
/// Rounding behavior:
/// - We intentionally use floor semantics (never round up) so that scaled
///   coordinates/dimensions do not overshoot the framebuffer by one pixel on
///   HiDPI setups.
/// - `logical` is non-negative (`u32`), so `floor` is equivalent to truncation
///   here; this function documents that choice explicitly.
pub fn logical_to_physical(logical: u32, scale: f64) -> u32 {
    // Use explicit floor to make the truncation semantics and HiDPI behavior clear.
    (f64::from(logical) * scale).floor() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitor_origin() {
        use crate::domain::types::{MonitorInfo, ScreenRect};
        let monitors = vec![
            MonitorInfo {
                rect: ScreenRect {
                    x: -1920,
                    y: 0,
                    w: 1920,
                    h: 1080,
                },
                name: "DP-2".to_string(),
                focused: false,
                active_workspace_id: 2,
            },
            MonitorInfo {
                rect: ScreenRect {
                    x: 0,
                    y: 0,
                    w: 2560,
                    h: 1440,
                },
                name: "DP-1".to_string(),
                focused: true,
                active_workspace_id: 1,
            },
        ];
        assert_eq!(monitor_origin(&monitors), (-1920, 0));
    }

    #[test]
    fn test_monitor_origin_empty() {
        assert_eq!(monitor_origin(&[]), (0, 0));
    }

    #[test]
    fn test_parse_slurp_geometry() {
        // Standard geometry
        let (x, y, w, h) = parse_slurp_geometry("10,20 100x200").unwrap();
        assert_eq!(x, 10);
        assert_eq!(y, 20);
        assert_eq!(w, 100);
        assert_eq!(h, 200);

        // Negative coordinates
        let (x, y, w, h) = parse_slurp_geometry("-1920,0 1920x1080").unwrap();
        assert_eq!(x, -1920);
        assert_eq!(y, 0);
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);

        // Extra whitespace (surrounding)
        let (x, y, w, h) = parse_slurp_geometry("  10,20 100x200  ").unwrap();
        assert_eq!(x, 10);
        assert_eq!(y, 20);
        assert_eq!(w, 100);
        assert_eq!(h, 200);

        // Invalid string (completely unparsable)
        assert!(parse_slurp_geometry("invalid").is_err());

        // Invalid string (missing size)
        assert!(parse_slurp_geometry("10,20 100").is_err());

        // Invalid string (extra commas)
        assert!(parse_slurp_geometry("10,20, 100x200").is_err());

        // Invalid string (missing coordinate)
        assert!(parse_slurp_geometry("10 100x200").is_err());

        // Invalid string (non-numeric parts)
        assert!(parse_slurp_geometry("a,b cxd").is_err());
    }

    #[test]
    fn test_clamp_crop() {
        // No clamping needed
        let (w, h, clamped) = clamp_crop(0, 0, 100, 100, 200, 200);
        assert_eq!(w, 100);
        assert_eq!(h, 100);
        assert!(!clamped);

        // Clamping width
        let (w, h, clamped) = clamp_crop(150, 0, 100, 100, 200, 200);
        assert_eq!(w, 50);
        assert_eq!(h, 100);
        assert!(clamped);

        // Clamping height
        let (w, h, clamped) = clamp_crop(0, 150, 100, 100, 200, 200);
        assert_eq!(w, 100);
        assert_eq!(h, 50);
        assert!(clamped);

        // Out of bounds entirely
        let (w, h, clamped) = clamp_crop(250, 250, 100, 100, 200, 200);
        assert_eq!(w, 0);
        assert_eq!(h, 0);
        assert!(clamped);

        // Exact boundary (touching right edge)
        let (w, h, clamped) = clamp_crop(100, 0, 100, 100, 200, 200);
        assert_eq!(w, 100);
        assert_eq!(h, 100);
        assert!(!clamped);

        // Overlapping boundary (starts inside, exceeds right edge)
        let (w, h, clamped) = clamp_crop(150, 0, 100, 100, 200, 200);
        assert_eq!(w, 50);
        assert_eq!(h, 100);
        assert!(clamped);
    }

    #[test]
    fn test_logical_to_physical() {
        // 1x scale
        assert_eq!(logical_to_physical(100, 1.0), 100);

        // 2x scale (HiDPI)
        assert_eq!(logical_to_physical(100, 2.0), 200);

        // Fractional scale (1.5x)
        assert_eq!(logical_to_physical(100, 1.5), 150);

        // Floor behavior
        // 100 * 1.25 = 125.0
        assert_eq!(logical_to_physical(100, 1.25), 125);

        // 101 * 1.25 = 126.25 -> 126
        assert_eq!(logical_to_physical(101, 1.25), 126);
    }
}
