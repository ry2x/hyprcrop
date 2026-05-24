# HyprCrop

Hyprland向けに作られた、高速なRust製スクリーンショットツールです。

## 特徴

- **即時キャプチャ**: 範囲、アクティブウィンドウ、フォーカス中モニター、全モニターを撮影
- **Portalキャプチャ**: xdg-desktop-portal のソースピッカー経由で任意のウィンドウ/モニターを選択
- **フリーズモード**: 画面を凍結し、オーバーレイUIで対話的に撮影対象を選択（Windowsの Win+Shift+S に近い操作感）
- `wl-copy` によるクリップボード自動コピー
- 成功/失敗のデスクトップ通知
- 保存先、ファイル名パターン、フリーズツールバーのグリフ・グリフサイズ・ボタン表示・表示位置、ウィンドウ枠の取り込み、フリーズモード全体のUIカラーテーマを設定可能

## 必要要件

以下のツールが `$PATH` から実行できる状態が必要です。

| ツール    | 用途                                |
| --------- | ----------------------------------- |
| `slurp`   | 範囲選択（cropモード）              |
| `wl-copy` | Waylandクリップボードへ画像をコピー |

デスクトップ通知はD-Bus経由でネイティブに送信されます（`notify-send` は不要）。
表示には通知デーモン（`mako`、`dunst` 等）が必要です。

画面キャプチャは **`zwlr_screencopy_manager_v1`** Waylandプロトコルでネイティブに実行されます（Hyprland / sway など wlroots 系コンポジター対応）。

ウィンドウ・モニター情報は **Hyprland IPCソケット**
（`$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock`）から直接取得します。

> [!CAUTION]
> フリーズモードのデフォルトグリフ表示には [Nerd Font](https://www.nerdfonts.com/) が必要です。アイコンは設定ファイルで変更できます。[設定](#設定)を参照してください。

## インストール

### Arch Linux (AUR)

AURから以下のコマンドでインストールできます。

```sh
yay -S hyprcrop
# or
paru -S hyprcrop
```

### ソースコードからのビルド (手動)

```sh
git clone https://github.com/ry2x/hyprcrop.git
cd hyprcrop
cargo build --release
cp target/release/hyprcrop ~/.local/bin/
```

## 使い方

```sh
hyprcrop [--config <FILE>] <SUBCOMMAND>
```

| サブコマンド      | 説明                                                         |
| ----------------- | ------------------------------------------------------------ |
| `crop`            | `slurp` で範囲選択して撮影                                   |
| `window`          | アクティブウィンドウを撮影（Hyprland IPCのジオメトリを使用） |
| `portal`          | xdg-desktop-portal のソースピッカーで選択して撮影            |
| `monitor`         | フォーカス中モニターを撮影                                   |
| `all`             | 全モニターを撮影                                             |
| `freeze`          | 画面を凍結して対話的に選択                                   |
| `generate-config` | デフォルト設定ファイルを出力                                 |

### グローバルオプション

`--config <FILE>` / `-c <FILE>`: 既定パスではなく任意の設定ファイルを読み込みます。
すべてのサブコマンド（`generate-config` 含む）で利用できます。

```sh
hyprcrop --config ~/.config/hyprcrop/work.toml freeze
```

### フリーズモード

フリーズモードでは画面全体にオーバーレイを表示し、ツールバーから撮影方式を切り替えられます。

![bar-image](./bar.png)

| モード  | 動作                           |
| ------- | ------------------------------ |
| Crop    | ドラッグで任意矩形を作成       |
| Window  | ウィンドウにホバーしてクリック |
| Monitor | モニターにホバーしてクリック   |
| All     | 画面全体を即時撮影             |
| Close   | キャンセル（Escapeと同じ）     |

アイコングリフは設定ファイルで変更可能です。[設定](#設定)を参照してください。

**キーボード:** `Escape` でキャンセル終了。

### Hyprland キーバインド例

```ini
# ~/.config/hypr/hyprland.conf
bindd = SUPER, S, ScreenshotMonitor,    exec, hyprcrop monitor
bindd = SUPER SHIFT, S, FreezeMode,     exec, hyprcrop freeze
bindd = , Print, ScreenshotFull,        exec, hyprcrop all
```

## 設定

設定ファイルの既定場所: `~/.config/hyprcrop/config.toml`

デフォルト設定は以下で生成できます。

```sh
hyprcrop generate-config
# 既存ファイルを上書きする場合:
hyprcrop generate-config --force
# 任意パスへ出力する場合:
hyprcrop --config ~/my-config.toml generate-config
```

### 設定サンプル

```toml
# スクリーンショットを保存するディレクトリ
# 既定値: ~/Screenshots
save_path = "~/Pictures/Screenshots"

# strftimeパターンで指定するファイル名（拡張子なし .pngは自動付与されます）
# 既定値: "hyprsnap_%Y%m%d_%H%M%S"
filename_pattern = "screenshot_%Y-%m-%d_%H-%M-%S"

# フリーズモードのツールバーを表示する画面の端。
# 選択肢: "top" | "bottom" | "left" | "right"  (既定値: "top")
toolbar_position = "top"

# trueの場合、ウィンドウキャプチャ（即時`window`コマンドとフリーズモードのWindow選択）に
# Hyprlandのウィンドウ枠を含めます。`general:border_size`分だけ各辺を拡張してキャプチャします。
# フリーズモードのオーバーレイでは`decoration:rounding`に合わせた角丸のハイライト枠を表示します。
# 既定値: false
capture_window_border = false

# trueの場合、フリーズモードのウィンドウキャプチャが `hyprland-toplevel-export-v1` を使用して
# Hyprlandが描画するウィンドウのサーフェスを直接キャプチャするようになります。
# このオプションが有効な場合、`capture_window_border` は強制的に false になります。
# 既定値: false
freeze_window_use_toplevel_export = false

# フリーズモードのツールバーに表示されるグリフ。
# 既定値はNerd Fontが必要です。必要に応じて個別のアイコンを上書きしてください。
[freeze_glyphs]
crop    = "󰆟"
window  = ""
monitor = "󰍹"
all     = "󰁌"
cancel  = "󰖭"
# size = 26.0  # ツールバーボタン内のグリフテキストサイズ（ピクセル）

# フリーズモードのツールバーに表示するボタンを個別にON/OFFできます。
# falseにしたボタンはツールバーから消えます。
# キャプチャモードボタン（crop/window/monitor/all）がすべてfalseの場合、
# フリーズモードはCropキャンバス選択（ドラッグ選択）にフォールバックします。
# cancel = true の場合はキャンセルボタンのみツールバーに表示されます。
[freeze_buttons]
crop    = true
window  = true
monitor = true
all     = true
cancel  = true

# ── 通知設定 ─────────────────────────────────────────────────────────────────
# 変数: {path} = 保存先ファイルパス。success_summary、success_body、success_action で使用可能。
# 変数: {error} = エラーメッセージ。error_summary、error_body で使用可能。
[notifications]
enabled          = true
success_action   = "xdg-open"   # 通知クリック時に実行するコマンド（シェル分割対応、{path}プレースホルダーまたは末尾に自動追加）
success_timeout  = 5000         # アクション待機時間（ms）。0 = 即終了（"Open" ボタンなし）
success_summary  = "Screenshot saved"
success_body     = "{path}"
error_summary    = "Screenshot failed"
error_body       = "{error}"

# ── フリーズモード UI カラー ──────────────────────────────────────────────────
# 色は CSS 形式の 16 進 RGBA 文字列 `"#RRGGBBAA"` で指定します。
# すべてのキーは省略可能で、省略した場合は以下の既定値が使用されます。

# [freeze_colors.overlay]
# background = "#00000059"     # 凍結画面上のディム

# [freeze_colors.toolbar]
# background = "#141414D9"  # ツールバーの背景

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
# name_text_idle = "#FFFFFF80"  # ホバーしていないときのモニター名

# [freeze_colors.crop_frame]
# stroke     = "#FFFFFFFF"
# label_text = "#FFFFFFFF"      # "W × H" サイズラベル
```

### 設定項目リファレンス

| キー                                           | 型           | 既定値                                                                      | 説明                                                                                                                                         |
| ---------------------------------------------- | ------------ | --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `save_path`                                    | path         | XDG Pictures ディレクトリ + `/Screenshots`（fallback: `$HOME/Screenshots`） | 保存先ディレクトリ                                                                                                                           |
| `filename_pattern`                             | string       | `hyprsnap_%Y%m%d_%H%M%S`                                                    | ファイル名のstrftimeパターン（拡張子なし）                                                                                                   |
| `notifications.enabled`                        | bool         | `true`                                                                      | 成功・失敗時にデスクトップ通知を送る                                                                                                         |
| `notifications.success_action`                 | string       | `"xdg-open"`                                                                | 「Open」クリック時のコマンド。シェル分割対応（例: `"satty -f {path}"`）。`{path}` でパス埋め込み、なければ末尾に自動追加                     |
| `notifications.success_timeout`                | 整数 (ms)    | `5000`                                                                      | アクション待機時間。`0` = 即終了（「Open」ボタンは表示されない）                                                                             |
| `notifications.success_summary`                | string       | `"Screenshot saved"`                                                        | 成功時の通知タイトル。`{path}` は保存先パスに置換                                                                                            |
| `notifications.success_body`                   | string       | `"{path}"`                                                                  | 成功時の通知本文。`{path}` は保存先パスに置換                                                                                                |
| `notifications.error_summary`                  | string       | `"Screenshot failed"`                                                       | 失敗時の通知タイトル。`{error}` はエラーメッセージに置換                                                                                     |
| `notifications.error_body`                     | string       | `"{error}"`                                                                 | 失敗時の通知本文。`{error}` はエラーメッセージに置換                                                                                         |
| `toolbar_position`                             | string       | `top`                                                                       | フリーズツールバーの表示位置: `top`, `bottom`, `left`, `right`                                                                               |
| `capture_window_border`                        | bool         | `false`                                                                     | ウィンドウキャプチャにHyprlandのウィンドウ枠を含める。フリーズモードでは角丸ハイライト枠も表示                                               |
| `freeze_glyphs.crop`                           | string       | `󰆟` (U+F019F)                                                               | cropモードのアイコン                                                                                                                         |
| `freeze_glyphs.window`                         | string       | `` (U+EB7F)                                                                | windowモードのアイコン                                                                                                                       |
| `freeze_glyphs.monitor`                        | string       | `󰍹` (U+F0379)                                                               | monitorモードのアイコン                                                                                                                      |
| `freeze_glyphs.all`                            | string       | `󰁌` (U+F004C)                                                               | allモードのアイコン                                                                                                                          |
| `freeze_glyphs.cancel`                         | string       | `󰖭` (U+F05AD)                                                               | cancelボタンのアイコン                                                                                                                       |
| `freeze_glyphs.size`                           | float        | `26.0`                                                                      | ツールバーボタン内のグリフテキストサイズ（ピクセル）                                                                                         |
| `freeze_buttons.crop`                          | bool         | `true`                                                                      | cropボタンの表示                                                                                                                             |
| `freeze_buttons.window`                        | bool         | `true`                                                                      | windowボタンの表示                                                                                                                           |
| `freeze_buttons.monitor`                       | bool         | `true`                                                                      | monitorボタンの表示                                                                                                                          |
| `freeze_buttons.all`                           | bool         | `true`                                                                      | allボタンの表示                                                                                                                              |
| `freeze_buttons.cancel`                        | bool         | `true`                                                                      | cancelボタンの表示。キャプチャモードボタンがすべて`false`の場合はCropキャンバス選択にフォールバック。cancel=trueならキャンセルボタンのみ表示 |
| `freeze_colors.overlay.background`             | string (hex) | `"#00000059"`                                                               | 凍結画面上のディムフィル                                                                                                                     |
| `freeze_colors.toolbar.background`             | string (hex) | `"#141414D9"`                                                               | ツールバー背景                                                                                                                               |
| `freeze_colors.button.idle_background`         | string (hex) | `"#797A7DFF"`                                                               | モードボタン・非選択時の背景                                                                                                                 |
| `freeze_colors.button.idle_text`               | string (hex) | `"#E6E6E6FF"`                                                               | モードボタン・非選択時のテキスト                                                                                                             |
| `freeze_colors.button.active_background`       | string (hex) | `"#5865F2FF"`                                                               | モードボタン・選択時の背景                                                                                                                   |
| `freeze_colors.button.active_text`             | string (hex) | `"#FFFFFFFF"`                                                               | モードボタン・選択時のテキスト                                                                                                               |
| `freeze_colors.button.hover_background`        | string (hex) | `"#6B79F5FF"`                                                               | モードボタン・ホバー時の背景                                                                                                                 |
| `freeze_colors.button.hover_text`              | string (hex) | `"#FFFFFFFF"`                                                               | モードボタン・ホバー時のテキスト                                                                                                             |
| `freeze_colors.cancel_button.idle_background`  | string (hex) | `"#C3423FFF"`                                                               | キャンセルボタン・通常背景                                                                                                                   |
| `freeze_colors.cancel_button.idle_text`        | string (hex) | `"#FFFFFFFF"`                                                               | キャンセルボタン・通常テキスト                                                                                                               |
| `freeze_colors.cancel_button.hover_background` | string (hex) | `"#D44A47FF"`                                                               | キャンセルボタン・ホバー背景                                                                                                                 |
| `freeze_colors.cancel_button.hover_text`       | string (hex) | `"#FFFFFFFF"`                                                               | キャンセルボタン・ホバーテキスト                                                                                                             |
| `freeze_colors.window_frame.fill_idle`         | string (hex) | `"#4585FF33"`                                                               | ウィンドウ枠フィル（非ホバー）                                                                                                               |
| `freeze_colors.window_frame.fill_hovered`      | string (hex) | `"#4585FF8C"`                                                               | ウィンドウ枠フィル（ホバー）                                                                                                                 |
| `freeze_colors.window_frame.stroke_idle`       | string (hex) | `"#4D99FFB3"`                                                               | ウィンドウ枠ストローク（非ホバー）                                                                                                           |
| `freeze_colors.window_frame.stroke_hovered`    | string (hex) | `"#4D99FFFF"`                                                               | ウィンドウ枠ストローク（ホバー）                                                                                                             |
| `freeze_colors.window_frame.label_text`        | string (hex) | `"#FFFFFFFF"`                                                               | ウィンドウタイトルテキスト（ホバー時）                                                                                                       |
| `freeze_colors.window_frame.hint_text`         | string (hex) | `"#CCE6FFE6"`                                                               | "Click to capture" ヒント（ホバー時）                                                                                                        |
| `freeze_colors.monitor_frame.fill_idle`        | string (hex) | `"#4585FF14"`                                                               | モニター枠フィル（非ホバー）                                                                                                                 |
| `freeze_colors.monitor_frame.fill_hovered`     | string (hex) | `"#4585FF66"`                                                               | モニター枠フィル（ホバー）                                                                                                                   |
| `freeze_colors.monitor_frame.stroke_idle`      | string (hex) | `"#4D99FF59"`                                                               | モニター枠ストローク（非ホバー）                                                                                                             |
| `freeze_colors.monitor_frame.stroke_hovered`   | string (hex) | `"#4D99FFFF"`                                                               | モニター枠ストローク（ホバー）                                                                                                               |
| `freeze_colors.monitor_frame.label_text`       | string (hex) | `"#FFFFFFFF"`                                                               | モニター名テキスト（ホバー時）                                                                                                               |
| `freeze_colors.monitor_frame.hint_text`        | string (hex) | `"#CCE6FFE6"`                                                               | "Click to capture" ヒント（ホバー時）                                                                                                        |
| `freeze_colors.monitor_frame.name_text_idle`   | string (hex) | `"#FFFFFF80"`                                                               | モニター名テキスト（非ホバー時）                                                                                                             |
| `freeze_colors.crop_frame.stroke`              | string (hex) | `"#FFFFFFFF"`                                                               | クロップ選択枠のストローク                                                                                                                   |
| `freeze_colors.crop_frame.label_text`          | string (hex) | `"#FFFFFFFF"`                                                               | クロップモードの "W × H" サイズラベル                                                                                                        |

## ライセンス

[MIT](./LICENSE)
