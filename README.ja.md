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

## ライセンス

[MIT](./LICENSE)
