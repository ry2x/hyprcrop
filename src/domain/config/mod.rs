//! # domain::config
//!
//! TOML configuration file loading and validation.
//! All default values are defined here; `Config::load()` reads from
//! `~/.config/crop-hypr/config.toml` and falls back to defaults if the file is absent.
//!
//! ## Key configuration sections
//!
//! - `save_path` / `filename_pattern` — output directory and file naming
//! - `[freeze_glyphs]` / `[freeze_buttons]` — freeze toolbar appearance
//! - `[freeze_colors]` — overlay, toolbar, and frame colors
//! - `[notifications]` — enable/disable notifications and icon settings

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::domain::error::{AppError, Result};

pub mod colors;
pub use colors::{
    ButtonColors, CancelButtonColors, CropFrameColors, FreezeColors, MonitorFrameColors,
    OverlayColors, RgbaColor, ToolbarColors, WindowFrameColors,
};

pub mod freeze;
pub use freeze::{FreezeButtons, FreezeGlyphs, ToolbarPosition};

pub mod notifications;
pub use notifications::Notifications;

fn default_capture_window_border() -> bool {
    false
}

fn default_window_use_toplevel_export() -> bool {
    false
}

fn default_save_path() -> PathBuf {
    dirs::picture_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("Screenshots")
}

fn default_filename_pattern() -> String {
    "hyprsnap_%Y%m%d_%H%M%S".to_string()
}

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_save_path")]
    pub save_path: PathBuf,

    #[serde(default = "default_filename_pattern")]
    pub filename_pattern: String,

    #[serde(default)]
    pub freeze_glyphs: FreezeGlyphs,

    #[serde(default)]
    pub toolbar_position: ToolbarPosition,

    /// When `true`, window captures include the Hyprland border (expanded by
    /// `general:border_size` on each side) and the freeze-mode overlay draws
    /// rounded highlight frames matching `decoration:rounding`.
    #[serde(default = "default_capture_window_border")]
    pub capture_window_border: bool,

    #[serde(default)]
    pub notifications: Notifications,

    /// Colors for every element of the freeze-mode overlay UI.
    /// All keys are optional; omitted keys fall back to the built-in defaults.
    #[serde(default)]
    pub freeze_colors: FreezeColors,

    /// Controls which buttons are shown in the freeze-mode toolbar.
    /// Buttons set to `false` are omitted; if all are `false`, the toolbar is hidden.
    #[serde(default)]
    pub freeze_buttons: FreezeButtons,

    /// When `true`, freeze-mode window capture and `hyprcrop window` capture
    /// use `hyprland-toplevel-export-v1` to directly capture
    /// the window surface instead of cropping from the frozen monitor image.
    /// `capture_window_border` has no effect when toplevel export succeeds.
    #[serde(default = "default_window_use_toplevel_export")]
    pub window_use_toplevel_export: bool,

    /// Deprecated alias for `window_use_toplevel_export`.
    /// If `freeze_window_use_toplevel_export` is `true`, it forces `window_use_toplevel_export` to `true`
    /// for backward compatibility with older config files and emits a warning.
    /// This field is deprecated and should not be used in new config files. Next major version will remove this field and the associated compatibility logic.
    #[serde(default, skip_serializing)]
    pub freeze_window_use_toplevel_export: bool, // Deprecated alias for `window_use_toplevel_export`.
}

impl Default for Config {
    fn default() -> Self {
        Self {
            save_path: default_save_path(),
            filename_pattern: default_filename_pattern(),
            freeze_glyphs: FreezeGlyphs::default(),
            toolbar_position: ToolbarPosition::default(),
            capture_window_border: default_capture_window_border(),
            notifications: Notifications::default(),
            freeze_colors: FreezeColors::default(),
            freeze_buttons: FreezeButtons::default(),
            window_use_toplevel_export: default_window_use_toplevel_export(),
            freeze_window_use_toplevel_export: default_window_use_toplevel_export(), // Deprecated alias for `window_use_toplevel_export`.
        }
    }
}

impl Config {
    /// Load config from the default path (`~/.config/hyprcrop/config.toml`).
    /// Falls back to defaults if the file does not exist.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::default_config_path())
    }

    /// Returns the default config file path (`~/.config/hyprcrop/config.toml`).
    pub fn default_config_path() -> PathBuf {
        default_config_path()
    }

    /// Load config from an explicit path.
    /// Falls back to defaults if the file does not exist.
    pub fn load_from(path: &Path) -> Result<Self> {
        let mut cfg = if !path.exists() {
            Self::default()
        } else {
            let raw = fs::read_to_string(path)
                .map_err(|e| AppError::FileSystem(path.to_path_buf(), e))?;
            let mut parsed: Config = toml::from_str(&raw)?;

            if let Ok(toml::Value::Table(table)) = toml::from_str::<toml::Value>(&raw)
                && table.contains_key("freeze_window_use_toplevel_export")
            {
                eprintln!(
                    "[hyprcrop] warning: 'freeze_window_use_toplevel_export' is deprecated and will be removed in a future version. Please use 'window_use_toplevel_export' instead."
                );
                if !table.contains_key("window_use_toplevel_export")
                    && let Some(toml::Value::Boolean(b)) =
                        table.get("freeze_window_use_toplevel_export")
                {
                    parsed.window_use_toplevel_export = *b;
                }
            }

            parsed
        };

        cfg.save_path = expand_tilde(&cfg.save_path);

        cfg.validate()?;

        Ok(cfg)
    }

    /// Serialize the default config to a TOML string, suitable for writing to
    /// a config file. Used by the `generate-config` command.
    pub fn generate_default_toml() -> Result<String> {
        toml::to_string_pretty(&Self::default())
            .map_err(|e| AppError::Config(format!("failed to serialize default config: {e}")))
    }

    fn validate(&self) -> Result<()> {
        if self.filename_pattern.trim().is_empty() {
            return Err(AppError::Config(
                "filename_pattern cannot be empty".to_string(),
            ));
        }
        Ok(())
    }

    pub fn output_filename(&self) -> String {
        let ts = chrono::Local::now()
            .format(&self.filename_pattern)
            .to_string();
        format!("{ts}.png")
    }

    pub fn output_path(&self) -> PathBuf {
        self.save_path.join(self.output_filename())
    }
}

fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hyprcrop")
        .join("config.toml")
}

fn expand_tilde(path: &std::path::Path) -> PathBuf {
    let s = path.to_string_lossy();
    let (expanded, from_tilde_or_relative) = if let Some(stripped) = s.strip_prefix("~/") {
        (
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(stripped),
            true,
        )
    } else if s == "~" {
        (dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")), true)
    } else {
        (path.to_path_buf(), false)
    };

    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        // Relative paths are anchored to home.
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(expanded)
    };

    let normalized = normalize_path(resolved);

    // For tilde-expanded or relative paths, guard against `..` traversal
    // that escapes the home directory (e.g. `~/foo/../../etc`).
    // Explicit absolute paths (e.g. `/tmp/screenshots`) are passed through as-is.
    if (from_tilde_or_relative || !path.is_absolute())
        && let Some(home) = dirs::home_dir()
        && !normalized.starts_with(&home)
    {
        eprintln!(
            "[hyprcrop] warning: save_path '{}' resolves outside home directory, falling back to ~/Screenshots",
            path.display()
        );
        return home.join("Screenshots");
    }

    normalized
}

/// Resolve `.` and `..` path components without touching the filesystem.
///
/// Correctly preserves Prefix/RootDir anchors so that e.g. `/../etc` stays
/// as `/etc` (not a relative `etc`). Leading `..` on relative paths are kept
/// as-is (they cannot be resolved without a base).
fn normalize_path(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                let last_is_normal =
                    matches!(out.components().next_back(), Some(Component::Normal(_)));
                if last_is_normal {
                    out.pop();
                } else if !out.has_root() {
                    // Keep leading `..` on relative paths.
                    out.push(component.as_os_str());
                }
                // For absolute paths at root, `..` is a no-op (silently dropped).
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::freeze::{
        default_glyph_all, default_glyph_cancel, default_glyph_crop, default_glyph_monitor,
        default_glyph_size, default_glyph_window,
    };
    use std::io::Write;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn write_toml(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(content.as_bytes()).expect("write");
        f
    }

    #[test]
    fn test_default_config_save_path() {
        let cfg = Config::default();
        assert!(
            cfg.save_path.to_string_lossy().contains("Screenshots"),
            "save_path should contain 'Screenshots'"
        );
    }

    #[test]
    fn test_default_config_filename_pattern() {
        assert_eq!(
            Config::default().filename_pattern,
            default_filename_pattern()
        );
    }

    #[test]
    fn test_default_freeze_glyphs() {
        let g = FreezeGlyphs::default();
        assert_eq!(g.crop, default_glyph_crop());
        assert_eq!(g.window, default_glyph_window());
        assert_eq!(g.monitor, default_glyph_monitor());
        assert_eq!(g.all, default_glyph_all());
        assert_eq!(g.cancel, default_glyph_cancel());
    }

    #[test]
    fn test_validation_accepts_non_empty_pattern() {
        let cfg: Config = toml::from_str("filename_pattern = 'test'").unwrap();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validation_rejects_empty_pattern() {
        let cfg: Config = toml::from_str("filename_pattern = ''").unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("filename_pattern cannot be empty"));
    }

    #[test]
    fn test_validation_rejects_whitespace_only_pattern() {
        let cfg: Config = toml::from_str("filename_pattern = '   '").unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_freeze_glyphs_partial_override() {
        let cfg: Config = toml::from_str("[freeze_glyphs]\ncrop = \"X\"").unwrap();
        assert_eq!(cfg.freeze_glyphs.crop, "X");
        assert_eq!(cfg.freeze_glyphs.window, default_glyph_window());
        assert_eq!(cfg.freeze_glyphs.monitor, default_glyph_monitor());
        assert_eq!(cfg.freeze_glyphs.all, default_glyph_all());
        assert_eq!(cfg.freeze_glyphs.cancel, default_glyph_cancel());
    }

    #[test]
    fn test_freeze_glyphs_full_override() {
        let toml = r#"
[freeze_glyphs]
crop    = "A"
window  = "B"
monitor = "C"
all     = "D"
cancel  = "E"
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.freeze_glyphs.crop, "A");
        assert_eq!(cfg.freeze_glyphs.window, "B");
        assert_eq!(cfg.freeze_glyphs.monitor, "C");
        assert_eq!(cfg.freeze_glyphs.all, "D");
        assert_eq!(cfg.freeze_glyphs.cancel, "E");
    }

    #[test]
    fn test_toolbar_position_default_is_top() {
        assert_eq!(Config::default().toolbar_position, ToolbarPosition::Top);
    }

    #[test]
    fn test_toolbar_position_deserializes_all_variants() {
        for (s, expected) in [
            ("top", ToolbarPosition::Top),
            ("bottom", ToolbarPosition::Bottom),
            ("left", ToolbarPosition::Left),
            ("right", ToolbarPosition::Right),
        ] {
            let toml = format!("toolbar_position = \"{s}\"");
            let cfg: Config = toml::from_str(&toml).unwrap();
            assert_eq!(cfg.toolbar_position, expected, "failed for {s}");
        }
    }

    #[test]
    fn test_toolbar_position_missing_defaults_to_top() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.toolbar_position, ToolbarPosition::Top);
    }

    #[test]
    fn test_generate_default_toml_is_valid_toml() {
        let s = Config::generate_default_toml().expect("serialize");
        toml::from_str::<Config>(&s).expect("generated TOML must parse cleanly");
    }

    #[test]
    fn test_generate_default_toml_round_trips_all_fields() {
        let original = Config::default();
        let parsed: Config =
            toml::from_str(&Config::generate_default_toml().expect("serialize")).expect("parse");

        assert_eq!(parsed.filename_pattern, original.filename_pattern);
        assert_eq!(parsed.freeze_glyphs.crop, original.freeze_glyphs.crop);
        assert_eq!(parsed.freeze_glyphs.window, original.freeze_glyphs.window);
        assert_eq!(parsed.freeze_glyphs.monitor, original.freeze_glyphs.monitor);
        assert_eq!(parsed.freeze_glyphs.all, original.freeze_glyphs.all);
        assert_eq!(parsed.freeze_glyphs.cancel, original.freeze_glyphs.cancel);
        assert_eq!(parsed.freeze_glyphs.size, original.freeze_glyphs.size);
        assert_eq!(parsed.toolbar_position, original.toolbar_position);
        assert_eq!(parsed.freeze_buttons.crop, original.freeze_buttons.crop);
        assert_eq!(parsed.freeze_buttons.window, original.freeze_buttons.window);
        assert_eq!(
            parsed.freeze_buttons.monitor,
            original.freeze_buttons.monitor
        );
        assert_eq!(parsed.freeze_buttons.all, original.freeze_buttons.all);
        assert_eq!(parsed.freeze_buttons.cancel, original.freeze_buttons.cancel);
        assert_eq!(
            parsed.window_use_toplevel_export,
            original.window_use_toplevel_export
        );
    }

    #[test]
    fn test_window_use_toplevel_export_default_false() {
        assert!(!Config::default().window_use_toplevel_export);
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.window_use_toplevel_export);
    }

    #[test]
    fn test_freeze_window_use_toplevel_export_fallback() {
        let f = write_toml("freeze_window_use_toplevel_export = true");
        let cfg = Config::load_from(f.path()).expect("load");
        assert!(cfg.window_use_toplevel_export);
    }

    #[test]
    fn test_freeze_window_use_toplevel_export_no_fallback_when_window_export_exists() {
        let f = write_toml(
            "freeze_window_use_toplevel_export = true\nwindow_use_toplevel_export = false",
        );
        let cfg = Config::load_from(f.path()).expect("load");
        assert!(!cfg.window_use_toplevel_export);
    }

    #[test]
    fn test_capture_window_border_independent_of_toplevel_export() {
        // Both flags are independent; load_from must not mutate one based on the other.
        let f = write_toml("capture_window_border = true\nwindow_use_toplevel_export = true");
        let cfg = Config::load_from(f.path()).expect("load");
        assert!(cfg.window_use_toplevel_export);
        assert!(
            cfg.capture_window_border,
            "capture_window_border must not be mutated by load_from when window_use_toplevel_export is true"
        );
    }

    #[test]
    fn test_capture_window_border_unaffected_when_toplevel_export_off() {
        let f = write_toml("capture_window_border = true\nwindow_use_toplevel_export = false");
        let cfg = Config::load_from(f.path()).expect("load");
        assert!(cfg.capture_window_border);
        assert!(!cfg.window_use_toplevel_export);
    }

    #[test]
    fn test_glyph_size_default() {
        assert_eq!(Config::default().freeze_glyphs.size, default_glyph_size());
    }

    #[test]
    fn test_glyph_size_override() {
        let cfg: Config = toml::from_str("[freeze_glyphs]\nsize = 40.0").unwrap();
        assert!((cfg.freeze_glyphs.size - 40.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_freeze_buttons_all_default_true() {
        let b = FreezeButtons::default();
        assert!(b.crop && b.window && b.monitor && b.all && b.cancel);
        assert!(b.any_visible());
    }

    #[test]
    fn test_freeze_buttons_missing_section_defaults_to_all_true() {
        let cfg: Config = toml::from_str("").unwrap();
        let b = cfg.freeze_buttons;
        assert!(b.crop && b.window && b.monitor && b.all && b.cancel);
    }

    #[test]
    fn test_freeze_buttons_partial_override() {
        let cfg: Config = toml::from_str("[freeze_buttons]\ncrop = false\nall = false").unwrap();
        assert!(!cfg.freeze_buttons.crop);
        assert!(cfg.freeze_buttons.window);
        assert!(cfg.freeze_buttons.monitor);
        assert!(!cfg.freeze_buttons.all);
        assert!(cfg.freeze_buttons.cancel);
        assert!(cfg.freeze_buttons.any_visible());
    }

    #[test]
    fn test_freeze_buttons_all_false_not_visible() {
        let toml = r#"
[freeze_buttons]
crop    = false
window  = false
monitor = false
all     = false
cancel  = false
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(!cfg.freeze_buttons.any_visible());
    }

    #[test]
    fn test_load_from_nonexistent_returns_defaults() {
        let cfg = Config::load_from(std::path::Path::new("/nonexistent/path/config.toml"))
            .expect("missing file should yield defaults");
        assert_eq!(cfg.filename_pattern, default_filename_pattern());
    }

    #[test]
    fn test_load_from_file_overrides_fields() {
        let f = write_toml(
            r#"
filename_pattern = "snap_%Y"
[freeze_glyphs]
cancel = "Z"
"#,
        );
        let cfg = Config::load_from(f.path()).expect("load");
        assert_eq!(cfg.filename_pattern, "snap_%Y");
        assert_eq!(cfg.freeze_glyphs.cancel, "Z");
        assert_eq!(cfg.freeze_glyphs.crop, default_glyph_crop());
    }

    #[test]
    fn test_load_from_invalid_toml_returns_error() {
        let f = write_toml("not valid toml [[[");
        assert!(Config::load_from(f.path()).is_err());
    }

    #[test]
    fn test_output_filename_has_png_extension() {
        assert!(Config::default().output_filename().ends_with(".png"));
    }

    #[test]
    fn test_output_path_is_under_save_path() {
        let cfg = Config::default();
        let path = cfg.output_path();
        assert_eq!(path.parent().unwrap(), cfg.save_path);
    }

    #[test]
    fn test_tilde_expansion() {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

        assert_eq!(
            expand_tilde(&PathBuf::from("~/test/dir")),
            home.join("test/dir")
        );
        assert_eq!(expand_tilde(&PathBuf::from("~")), home);
        assert_eq!(
            expand_tilde(&PathBuf::from("/tmp/test")),
            PathBuf::from("/tmp/test")
        );
        assert_eq!(
            expand_tilde(&PathBuf::from("Screenshots")),
            home.join("Screenshots")
        );
    }

    #[test]
    fn test_normalize_path_dotdot_collapses_normal() {
        assert_eq!(
            normalize_path(PathBuf::from("/home/user/foo/../bar")),
            PathBuf::from("/home/user/bar"),
        );
    }

    #[test]
    fn test_normalize_path_root_dotdot_stays_at_root() {
        assert_eq!(
            normalize_path(PathBuf::from("/../etc")),
            PathBuf::from("/etc"),
        );
    }

    #[test]
    fn test_normalize_path_relative_leading_dotdot_preserved() {
        assert_eq!(
            normalize_path(PathBuf::from("../outside")),
            PathBuf::from("../outside"),
        );
    }

    #[test]
    fn test_expand_tilde_traversal_clamped_to_screenshots() {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let result = expand_tilde(&PathBuf::from("~/foo/../../etc"));
        assert_eq!(result, home.join("Screenshots"));
    }

    #[test]
    fn test_expand_tilde_absolute_path_is_passed_through() {
        assert_eq!(
            expand_tilde(&PathBuf::from("/tmp/screenshots")),
            PathBuf::from("/tmp/screenshots"),
        );
    }
}
