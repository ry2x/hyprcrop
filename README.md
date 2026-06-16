# HyprCrop

[![MIT License](http://img.shields.io/badge/license-MIT-blue.svg?style=flat)](./LICENSE) [![Release](https://github.com/ry2x/hyprcrop/actions/workflows/release.yml/badge.svg)](https://github.com/ry2x/hyprcrop/actions/workflows/release.yml)

A fast, Hyprland-native screenshot tool written in Rust.
HyprCrop is not a wrapper of grim; it captures the screen directly via wayland APIs!

## Features

- **Immediate capture** — crop region, active window, focused monitor, or all monitors
- **Portal capture** — select any window or monitor via xdg-desktop-portal's WM source-picker
- **Freeze mode** — freeze the screen and interactively select what to capture via an overlay UI (similar to Windows Win+Shift+S Clipping Tool)
- Automatic clipboard copy via `wl-copy`
- Desktop notification on success/failure
- Configurable save path, filename pattern, freeze toolbar glyphs (including size), toolbar position, per-button visibility, window border inclusion, and full UI color theming

## Dependencies

| Package                         | Note                                                    |
| :------------------------------ | :------------------------------------------------------ |
| **Hyprland**                    | Tested on `v0.5.3` and later.                           |
| **libnotify**                   | Required for desktop notifications.                     |
| **xdg-desktop-portal-hyprland** | Required for portal capture mode.                       |
| **pipewire**                    | Required for screen capture.                            |
| **slurp**                       | Required for interactive region selection in crop mode. |
| **wl-clipboard**                | Required for copying screenshots to the clipboard.      |

> [!CAUTION]
> A [Nerd Font](https://www.nerdfonts.com/) is required to display default glyphs in the freeze mode toolbar. Check the [configuration section](#configuration) for details on customizing icons.

## Installation

### Arch Linux (AUR)

- `hyprcrop`: [build from source package](https://aur.archlinux.org/packages/hyprcrop)

```sh
yay -S hyprcrop        # builds from source
```

- `hyprcrop-bin`: [prebuilt binary package](https://aur.archlinux.org/packages/hyprcrop-bin)

```sh
yay -S hyprcrop-bin    # prebuilt binary
```

### Build from source (Manual)

> [!WARNING]
> This method will install the latest commit, which may be unstable. For a stable release, build from a [tagged release](https://github.com/ry2x/hyprcrop/releases).

```sh
git clone https://github.com/ry2x/hyprcrop.git
cd hyprcrop
cargo build --frozen --release
install -Dm755 target/release/hyprcrop ~/.local/bin/
# Optional: Run tests to verify everything is working before installing
cargo test --frozen
```

## Usage

```sh
hyprcrop [--config <FILE>] <SUBCOMMAND>
```

| Subcommand        | Description                                                                          |
| :---------------- | :----------------------------------------------------------------------------------- |
| `crop`            | Select a region with slurp and capture it                                            |
| `window`          | Capture the active window (geometry via Hyprland IPC)                                |
| `portal`          | Capture a selected window or monitor via xdg-desktop-portal (shows WM source-picker) |
| `monitor`         | Capture the focused monitor                                                          |
| `all`             | Capture all monitors                                                                 |
| `freeze`          | Freeze screen and select interactively                                               |
| `generate-config` | Write a default config file                                                          |

After capture, the screenshot is saved to disk and copied to the clipboard. A desktop notification is shown on success or failure.

### Global flag

`--config <FILE>` / `-c <FILE>` — Load config from a custom path instead of the default.
Works with every subcommand, including `generate-config`.

```sh
hyprcrop --config ~/.config/hyprcrop/work.toml freeze
```

### Freeze mode

Freeze mode overlays the screen and lets you switch capture type via a toolbar:

![bar-image](./bar.png)

| Mode    | Behavior                        |
| :------ | :------------------------------ |
| Crop    | Drag to draw a custom rectangle |
| Window  | Hover and click a window        |
| Monitor | Hover and click a monitor       |
| All     | Capture everything instantly    |
| Close   | Cancel (same as Escape)         |

Icon glyphs can be customized in the config file. Check the [configuration section](#configuration) for details.

**Keyboard:** `Escape` cancels and exits.

### Hyprland key-bind example

- `~/.config/hypr/hyprland.conf`

```ini
bindd = SUPER, S, ScreenshotMonitor,    exec, hyprcrop monitor
bindd = SUPER SHIFT, S, FreezeMode,     exec, hyprcrop freeze
bindd = , Print, ScreenshotFull,        exec, hyprcrop all
```

- `~/.config/hypr/hyprland.lua`

```lua
hl.bind("SUPER + S",            hl.dsp.exec_cmd("hyprcrop monitor") )
hl.bind("SUPER + SHIFT + S",    hl.dsp.exec_cmd("hyprcrop freeze") )
hl.bind("Print",                hl.dsp.exec_cmd("hyprcrop all") )
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

### Example configs

<details>

<summary>Example Config with Descriptions</summary>

> [!WARNING]
> `freeze_window_use_toplevel_export` has been renamed to `window_use_toplevel_export`.
> If you have an existing config with the old key, you will be warned about deprecated keys on end.
> Update your config by replacing `freeze_window_use_toplevel_export` with `window_use_toplevel_export` to remove the warning.

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

# When `true`, freeze-mode window capture and `hyprcrop window` capture
# uses `hyprland-toplevel-export-v1` to directly capture
# the window surface instead of cropping from the frozen monitor image.
# Incompatible with `capture_window_border`; that option is forced `false` when this is enabled.
# Default: false
window_use_toplevel_export = false

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

[freeze_colors.overlay]
background = "#00000059"  # dim over frozen screen

[freeze_colors.toolbar]
background = "#141414D9"  # toolbar pill background

[freeze_colors.button]
idle_background   = "#797A7DFF"
idle_text         = "#E6E6E6FF"
active_background = "#5865F2FF"
active_text       = "#FFFFFFFF"
hover_background  = "#6B79F5FF"
hover_text        = "#FFFFFFFF"

[freeze_colors.cancel_button]
idle_background  = "#C3423FFF"
idle_text        = "#FFFFFFFF"
hover_background = "#D44A47FF"
hover_text       = "#FFFFFFFF"

[freeze_colors.window_frame]
fill_idle      = "#4585FF33"
fill_hovered   = "#4585FF8C"
stroke_idle    = "#4D99FFB3"
stroke_hovered = "#4D99FFFF"
label_text     = "#FFFFFFFF"  # "window title" label
hint_text      = "#CCE6FFE6"  # "Click to capture"

[freeze_colors.monitor_frame]
fill_idle      = "#4585FF14"
fill_hovered   = "#4585FF66"
stroke_idle    = "#4D99FF59"
stroke_hovered = "#4D99FFFF"
label_text     = "#FFFFFFFF"  # "monitor name" label eg, "DP-1"
hint_text      = "#CCE6FFE6"  # "Click to capture"
name_text_idle = "#FFFFFF80"  # monitor name when not hovered

[freeze_colors.crop_frame]
stroke     = "#FFFFFFFF"
label_text = "#FFFFFFFF"      # "W × H" size label
```

</details>

<details>

<summary>Matugen Theme Example</summary>

- `~/.config/matugen/config.toml`

```toml
[templates.hyprcrop]
input_path = '~/.config/matugen/templates/matugen-hyprcrop.toml'
output_path = '~/.config/hyprcrop/config.toml'
```

- `~/.config/matugen/templates/matugen-hyprcrop.toml`

```toml
save_path = "~/Pictures/Screenshots"
filename_pattern = "grim-%Y-%m-%d_%H%M_%S"
toolbar_position = "top"
capture_window_border = false
window_use_toplevel_export = true

[freeze_glyphs]
crop = "󰆟"
window = ""
monitor = "󰍹"
all = "󰁌"
cancel = "󰖭"

[notifications]
enabled = true
success_action = "swayimg"
success_timeout = 5000
success_summary = "Screenshot saved"
success_body = "{path}"
error_summary = "Screenshot failed"
error_body = "{error}"

[freeze_colors.overlay]
background = "#00000059"

[freeze_colors.toolbar]
background = "{{ colors.background.default.hex }}D9"

[freeze_colors.button]
idle_background = "{{ colors.surface.default.hex }}"
idle_text = "{{ colors.on_surface.default.hex }}"
active_background = "{{ colors.primary.default.hex }}"
active_text = "{{ colors.on_primary.default.hex }}"
hover_background = "{{ colors.tertiary.default.hex }}"
hover_text = "{{ colors.on_tertiary.default.hex }}"

[freeze_colors.cancel_button]
idle_background = "{{ colors.error.default.hex }}"
idle_text = "{{ colors.on_error.default.hex }}"
hover_background = "#D44A47FF"
hover_text = "#FFFFFFFF"

[freeze_colors.window_frame]
fill_idle = "{{ colors.primary_container.default.hex }}33"
fill_hovered = "{{ colors.primary.default.hex }}8C"
stroke_idle = "{{ colors.outline.default.hex }}"
stroke_hovered = "{{ colors.outline.default.hex }}"
label_text = "{{colors.on_surface.default.hex}}"
hint_text = "{{ colors.on_surface.default.hex }}E6"

[freeze_colors.monitor_frame]
fill_idle = "{{ colors.primary_container.default.hex }}33"
fill_hovered = "{{ colors.primary.default.hex }}8C"
stroke_idle = "{{ colors.outline.default.hex }}"
stroke_hovered = "{{ colors.outline.default.hex }}"
label_text = "{{colors.on_surface.default.hex}}"
hint_text = "{{ colors.on_surface.default.hex }}E6"
name_text_idle = "{{ colors.on_surface.default.hex }}80"

[freeze_colors.crop_frame]
stroke = "{{ colors.primary.default.hex }}"
label_text = "{{ colors.tertiary.default.hex }}"
```

</details>
