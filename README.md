# HyprCrop

A fast, Hyprland-native screenshot tool written in Rust.

日本語版README: [README.ja.md](./README.ja.md)

## Features

- **Immediate capture** — crop region, active window, focused monitor, or all monitors
- **Portal capture** — select any window or monitor via xdg-desktop-portal's WM source-picker
- **Freeze mode** — freeze the screen and interactively select what to capture via an overlay UI (similar to Windows Win+Shift+S Clipping Tool)
- Automatic clipboard copy via `wl-copy`
- Desktop notification on success/failure
- Configurable save path, filename pattern, freeze toolbar glyphs (including size), toolbar position, per-button visibility, window border inclusion, and full UI color theming

## Requirements

The following tools must be available on `$PATH`:

| Tool      | Purpose                                  |
| --------- | ---------------------------------------- |
| `slurp`   | Interactive region selection (crop mode) |
| `wl-copy` | Copy image to Wayland clipboard          |

Desktop notifications are sent natively via D-Bus (no `notify-send` required). A running
notification daemon (e.g. `mako`, `dunst`) is needed to display them.

Screen capture is performed natively via the **`zwlr_screencopy_manager_v1`** Wayland protocol
(wlroots-based compositors: Hyprland, sway, etc.) — no external capture tool is required.

Window and monitor metadata is fetched directly via the **Hyprland IPC socket**
(`$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock`).

> [!CAUTION]
> A [Nerd Font](https://www.nerdfonts.com/) is required to display default glyphs in the freeze mode toolbar. Check the [configuration section](#configuration) for details on customizing icons.

## Installation

### Arch Linux (AUR)

```sh
yay -S hyprcrop
# or
paru -S hyprcrop
```

### Build from source (Manual)

```sh
git clone https://github.com/ry2x/hyprcrop.git
cd hyprcrop
cargo build --release
cp target/release/hyprcrop ~/.local/bin/
```

## Usage

```sh
hyprcrop [--config <FILE>] <SUBCOMMAND>
```

| Subcommand        | Description                                                                          |
| ----------------- | ------------------------------------------------------------------------------------ |
| `crop`            | Select a region with slurp and capture it                                            |
| `window`          | Capture the active window (geometry via Hyprland IPC)                                |
| `portal`          | Capture a selected window or monitor via xdg-desktop-portal (shows WM source-picker) |
| `monitor`         | Capture the focused monitor                                                          |
| `all`             | Capture all monitors                                                                 |
| `freeze`          | Freeze screen and select interactively                                               |
| `generate-config` | Write a default config file                                                          |

### Global flag

`--config <FILE>` / `-c <FILE>` — Load config from a custom path instead of the default.
Works with every subcommand, including `generate-config`.

```sh
hyprcrop --config ~/.config/hyprcrop/work.toml freeze
```

### Freeze mode

Freeze mode overlays the screen and lets you switch capture type via a toolbar:

![bar-image](./bar.png)

| Mode    | Behaviour                       |
| ------- | ------------------------------- |
| Crop    | Drag to draw a custom rectangle |
| Window  | Hover and click a window        |
| Monitor | Hover and click a monitor       |
| All     | Capture everything instantly    |
| Close   | Cancel (same as Escape)         |

Icon glyphs can be customized in the config file. Check the [configuration section](#configuration) for details.

**Keyboard:** `Escape` cancels and exits.

### Hyprland keybind example

```ini
# ~/.config/hypr/hyprland.conf
bindd = SUPER, S, ScreenshotMonitor,    exec, hyprcrop monitor
bindd = SUPER SHIFT, S, FreezeMode,     exec, hyprcrop freeze
bindd = , Print, ScreenshotFull,        exec, hyprcrop all
```

## Configuration

Config file location: `~/.config/hyprcrop/config.toml`

Generate a default config with:

```sh
hyprcrop generate-config
# Already exists? Use --force to overwrite:
hyprcrop generate-config --force
# Write to a custom path:
hyprcrop --config ~/my-config.toml generate-config
```

### Sample config

```toml
# Directory where screenshots are saved.
# Default: ~/Screenshots
save_path = "~/Pictures/Screenshots"

# strftime-style filename template (no extension — .png is appended automatically).
# Default: "hyprsnap_%Y%m%d_%H%M%S"
filename_pattern = "screenshot_%Y-%m-%d_%H-%M-%S"

# Edge of the screen where the freeze mode toolbar is docked.
# Options: "top" | "bottom" | "left" | "right"  (default: "top")
toolbar_position = "top"

# When true, window captures (both immediate `window` and freeze-mode Window
# selection) include the Hyprland border, expanding the crop area by
# `general:border_size` on each side. The freeze-mode overlay also draws
# rounded highlight frames matching `decoration:rounding`.
# Default: false
capture_window_border = false

# When `true`, freeze-mode window capture uses `hyprland-toplevel-export-v1` to
# directly capture the window surface instead of cropping from the frozen monitor
# image. Incompatible with `capture_window_border`; that option is forced `false` when this is enabled.
# Default: false
freeze_window_use_toplevel_export = false

# Glyphs shown in the freeze mode toolbar.
# Requires a Nerd Font. Override individual icons as needed.
[freeze_glyphs]
crop    = "󰆟"
window  = ""
monitor = "󰍹"
all     = "󰁌"
cancel  = "󰖭"
# size = 26.0  # glyph text size inside toolbar buttons (pixels)

# Controls which buttons are visible in the freeze mode toolbar.
# Set any button to false to hide it. If all capture-mode buttons (crop/window/monitor/all)
# are false, freeze mode defaults to Crop canvas selection (drag-to-select still works);
# the toolbar is hidden unless cancel = true.
[freeze_buttons]
crop    = true
window  = true
monitor = true
all     = true
cancel  = true

# ── Notifications ─────────────────────────────────────────────────────────────
# Variables: {path} = saved file path (success_summary, success_body, success_action); {error} = error message (error_summary, error_body).
[notifications]
enabled          = true
success_action   = "xdg-open"   # command run when notification is clicked; shell-split, use {path} placeholder or path is appended
success_timeout  = 5000         # ms to wait for action; 0 = fire-and-forget (no "Open" button)
success_summary  = "Screenshot saved"
success_body     = "{path}"
error_summary    = "Screenshot failed"
error_body       = "{error}"

# ── Freeze mode UI colors ─────────────────────────────────────────────────────
# All colors are CSS-style hex strings: "#RRGGBBAA" (or "#RRGGBB", "#RGBA", "#RGB").
# Every key is optional; omitted keys fall back to the built-in defaults shown below.

# [freeze_colors.overlay]
# background = "#00000059"     # dim over frozen screen

# [freeze_colors.toolbar]
# background = "#141414D9"  # toolbar pill background

# [freeze_colors.button]
# idle_background   = "#797A7DFF"
# idle_text         = "#E6E6E6FF"
# active_background = "#5865F2FF"
# active_text       = "#FFFFFFFF"
# hover_background  = "#6B79F5FF"
# hover_text        = "#FFFFFFFF"

# [freeze_colors.cancel_button]
# idle_background  = "#C3423FFF"
# idle_text        = "#FFFFFFFF"
# hover_background = "#D44A47FF"
# hover_text       = "#FFFFFFFF"

# [freeze_colors.window_frame]
# fill_idle      = "#4585FF33"
# fill_hovered   = "#4585FF8C"
# stroke_idle    = "#4D99FFB3"
# stroke_hovered = "#4D99FFFF"
# label_text     = "#FFFFFFFF"
# hint_text      = "#CCE6FFE6"  # "Click to capture"

# [freeze_colors.monitor_frame]
# fill_idle      = "#4585FF14"
# fill_hovered   = "#4585FF66"
# stroke_idle    = "#4D99FF59"
# stroke_hovered = "#4D99FFFF"
# label_text     = "#FFFFFFFF"
# hint_text      = "#CCE6FFE6"  # "Click to capture"
# name_text_idle = "#FFFFFF80"  # monitor name when not hovered

# [freeze_colors.crop_frame]
# stroke     = "#FFFFFFFF"
# label_text = "#FFFFFFFF"      # "W × H" size label
```

## License

[MIT](./LICENSE)
