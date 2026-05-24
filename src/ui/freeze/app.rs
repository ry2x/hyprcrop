use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use iced::{
    ContentFit, Element, Length, Point, Rectangle, Subscription, Task, Theme,
    event::listen_with,
    keyboard::{Event as KeyEvent, Key, key::Named},
    mouse,
    widget::{
        Canvas, Column, Container, Row, Text, button, canvas,
        image::{self, Image},
        stack,
    },
};

use crate::domain::config::{
    CropFrameColors, FreezeButtons, FreezeColors, FreezeGlyphs, MonitorFrameColors, RgbaColor,
    ToolbarPosition, WindowFrameColors,
};
pub use crate::domain::state::CaptureMode;
use crate::domain::types::{BorderStyle, LayerSurface, MonitorInfo, ScreenRect, WindowInfo};

// ── FreezeSelection ───────────────────────────────────────────────────────────

/// The final capture decision returned from the freeze UI.
#[derive(Debug, Clone)]
pub enum FreezeSelection {
    /// Crop a rectangular region from the frozen monitor image (crop / monitor modes).
    Region(ScreenRect),
    /// Capture a specific toplevel window via `hyprland-toplevel-export-v1`.
    /// Carries full window metadata for title+class matching in the v2 protocol.
    ToplevelWindow(WindowInfo),
}

// ── Message ───────────────────────────────────────────────────────────────────

#[iced_layershell::to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Message {
    ModeSelected(CaptureMode),
    SelectionConfirmed(ScreenRect),
    /// Emitted when `use_toplevel_export` is ON and the user clicks a window.
    /// Carries the full WindowInfo for title+class matching in the v2 protocol.
    ToplevelWindowSelected(WindowInfo),
    /// Forces a re-render tick on startup to work around wgpu surface
    /// `SurfaceError::Outdated` on first present (shows white until something
    /// triggers another frame). Decrements `repaint_ticks` until zero.
    Tick,
    Cancel,
}

// ── Hovered target (Window mode distinguishes windows from layer surfaces) ────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoveredTarget {
    Window(usize),
    Layer(usize),
    Monitor(usize),
}

// ── Canvas program (owns its data, no lifetime on AppState) ───────────────────

pub struct SelectionCanvas {
    pub mode: CaptureMode,
    pub windows: Arc<Vec<WindowInfo>>,
    pub layers: Arc<Vec<LayerSurface>>,
    pub monitors: Arc<Vec<MonitorInfo>>,
    /// Global pixel origin of the monitor this overlay window is on.
    /// Canvas coordinates are local (0,0 = top-left of this monitor).
    /// `canvas_local = global - offset`
    pub monitor_offset: Point,
    /// Hyprland border style (size + rounding). Applied when drawing and
    /// confirming window-mode selections.
    pub border_style: BorderStyle,
    /// User-configured colors for the canvas overlay.
    pub colors: FreezeColors,
    /// When `true`, window clicks emit `ToplevelWindowSelected` instead of
    /// `SelectionConfirmed`, triggering the `hyprland-toplevel-export-v1` path.
    pub use_toplevel_export: bool,
}

// Canvas-internal mutable state
#[derive(Default)]
pub struct CanvasState {
    phase: DrawPhase,
    cursor: Point,
    hovered: Option<HoveredTarget>,
}

#[derive(Default)]
enum DrawPhase {
    #[default]
    Idle,
    Cropping {
        start: Point,
    },
}

impl canvas::Program<Message> for SelectionCanvas {
    type State = CanvasState;

    fn update(
        &self,
        state: &mut CanvasState,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let pos = cursor.position_in(bounds);

        match event {
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(p) = pos {
                    state.cursor = p;
                }
                match &state.phase {
                    DrawPhase::Cropping { .. } => {
                        return Some(canvas::Action::request_redraw());
                    }
                    DrawPhase::Idle => {
                        let prev = state.hovered;
                        state.hovered = match self.mode {
                            CaptureMode::Window => {
                                // Layers are at overlay level — check them first
                                hit_index_layer(&self.layers, pos, self.monitor_offset)
                                    .map(HoveredTarget::Layer)
                                    .or_else(|| {
                                        hit_index(
                                            &self.windows,
                                            pos,
                                            self.monitor_offset,
                                            self.border_style.border_size,
                                        )
                                        .map(HoveredTarget::Window)
                                    })
                            }
                            CaptureMode::Monitor => {
                                hit_index_m(&self.monitors, pos, self.monitor_offset)
                                    .map(HoveredTarget::Monitor)
                            }
                            _ => None,
                        };
                        if state.hovered != prev {
                            return Some(canvas::Action::request_redraw());
                        }
                    }
                }
            }

            canvas::Event::Mouse(mouse::Event::CursorLeft) => {
                if state.hovered.is_some() {
                    state.hovered = None;
                    return Some(canvas::Action::request_redraw());
                }
            }

            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                match self.mode {
                    CaptureMode::Crop => {
                        if let Some(p) = pos {
                            state.phase = DrawPhase::Cropping { start: p };
                            return Some(canvas::Action::request_redraw().and_capture());
                        }
                    }
                    CaptureMode::Window => match state.hovered {
                        Some(HoveredTarget::Layer(idx)) => {
                            let rect = self.layers[idx].rect;
                            return Some(
                                canvas::Action::publish(Message::SelectionConfirmed(rect))
                                    .and_capture(),
                            );
                        }
                        Some(HoveredTarget::Window(idx)) => {
                            if self.use_toplevel_export {
                                let window = self.windows[idx].clone();
                                // Windows with empty titles can't be matched by the v2 protocol.
                                if window.title.is_empty() {
                                    return None;
                                }
                                return Some(
                                    canvas::Action::publish(Message::ToplevelWindowSelected(
                                        window,
                                    ))
                                    .and_capture(),
                                );
                            }
                            let rect = self.windows[idx].rect.expand(self.border_style.border_size);
                            return Some(
                                canvas::Action::publish(Message::SelectionConfirmed(rect))
                                    .and_capture(),
                            );
                        }
                        None => {}
                        Some(HoveredTarget::Monitor(_)) => {}
                    },
                    CaptureMode::Monitor => {
                        if let Some(HoveredTarget::Monitor(idx)) = state.hovered {
                            let rect = self.monitors[idx].rect;
                            return Some(
                                canvas::Action::publish(Message::SelectionConfirmed(rect))
                                    .and_capture(),
                            );
                        }
                    }
                    CaptureMode::All => {}
                }
            }

            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let DrawPhase::Cropping { start } = state.phase {
                    state.phase = DrawPhase::Idle;
                    let local_rect = points_to_rect(start, state.cursor);
                    if local_rect.w >= 5 && local_rect.h >= 5 {
                        // Convert canvas-local coords to global logical space for crop
                        let global_rect = ScreenRect {
                            x: local_rect.x + self.monitor_offset.x as i32,
                            y: local_rect.y + self.monitor_offset.y as i32,
                            w: local_rect.w,
                            h: local_rect.h,
                        };
                        return Some(
                            canvas::Action::publish(Message::SelectionConfirmed(global_rect))
                                .and_capture(),
                        );
                    }
                    return Some(canvas::Action::request_redraw());
                }
            }

            _ => {}
        }

        None
    }

    fn draw(
        &self,
        state: &CanvasState,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry<iced::Renderer>> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        frame.fill(
            &canvas::Path::rectangle(Point::ORIGIN, bounds.size()),
            to_iced(self.colors.overlay.background),
        );

        match self.mode {
            CaptureMode::Crop => {
                if let DrawPhase::Cropping { start } = state.phase {
                    draw_selection(&mut frame, start, state.cursor, &self.colors.crop_frame);
                }
            }
            CaptureMode::Window => {
                for (i, win) in self.windows.iter().enumerate() {
                    draw_highlight(
                        &mut frame,
                        win.rect,
                        state.hovered == Some(HoveredTarget::Window(i)),
                        &win.title,
                        self.monitor_offset,
                        self.border_style,
                        &self.colors.window_frame,
                    );
                }
                // Layer surfaces are at overlay level — draw on top of windows
                for (i, layer) in self.layers.iter().enumerate() {
                    draw_highlight(
                        &mut frame,
                        layer.rect,
                        state.hovered == Some(HoveredTarget::Layer(i)),
                        &layer.namespace,
                        self.monitor_offset,
                        BorderStyle::default(),
                        &self.colors.window_frame,
                    );
                }
            }
            CaptureMode::Monitor => {
                for (i, mon) in self.monitors.iter().enumerate() {
                    draw_monitor_highlight(
                        &mut frame,
                        mon.rect,
                        state.hovered == Some(HoveredTarget::Monitor(i)),
                        &mon.name,
                        self.monitor_offset,
                        &self.colors.monitor_frame,
                    );
                }
            }
            CaptureMode::All => {}
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &CanvasState,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match (&state.phase, self.mode) {
            (DrawPhase::Cropping { .. }, _) | (_, CaptureMode::Crop) => {
                mouse::Interaction::Crosshair
            }
            (_, CaptureMode::Window) | (_, CaptureMode::Monitor) if state.hovered.is_some() => {
                mouse::Interaction::Pointer
            }
            _ => mouse::Interaction::default(),
        }
    }
}

// ── App State ─────────────────────────────────────────────────────────────────

/// All construction data for [`AppState`], grouped to avoid a long argument list.
pub struct AppStateConfig {
    pub monitor_images: Vec<image::Handle>,
    pub focused_monitor_idx: usize,
    pub window_to_monitor: HashMap<iced::window::Id, usize>,
    pub windows: Arc<Vec<WindowInfo>>,
    pub layers: Arc<Vec<LayerSurface>>,
    pub monitors: Arc<Vec<MonitorInfo>>,
    pub result: Arc<Mutex<Option<Option<FreezeSelection>>>>,
    pub glyphs: FreezeGlyphs,
    pub toolbar_position: ToolbarPosition,
    pub border_style: BorderStyle,
    pub initial_mode: CaptureMode,
    pub colors: FreezeColors,
    pub freeze_buttons: FreezeButtons,
    pub use_toplevel_export: bool,
}

pub struct AppState {
    pub mode: CaptureMode,
    /// One pre-decoded image handle per monitor (indexed same as `monitors`)
    pub monitor_images: Vec<image::Handle>,
    /// Index into `monitors` for the focused (initial) window
    pub focused_monitor_idx: usize,
    /// Maps extra window IDs (spawned at boot) → monitor index
    pub window_to_monitor: HashMap<iced::window::Id, usize>,
    pub windows: Arc<Vec<WindowInfo>>,
    pub layers: Arc<Vec<LayerSurface>>,
    pub monitors: Arc<Vec<MonitorInfo>>,
    /// None                   = cancelled (ESC, never set)
    /// Some(None)             = "All" selected (use full screenshot)
    /// Some(Some(selection))  = region or toplevel window selected
    pub result: Arc<Mutex<Option<Option<FreezeSelection>>>>,
    /// Counts down from N on startup. While > 0, a 50 ms timer drives
    /// repeated renders so that wgpu surfaces that fail their first
    /// `compositor.present()` (SurfaceError::Outdated) get a second chance.
    repaint_ticks: u8,
    glyphs: FreezeGlyphs,
    toolbar_position: ToolbarPosition,
    border_style: BorderStyle,
    colors: FreezeColors,
    freeze_buttons: FreezeButtons,
    use_toplevel_export: bool,
}

impl AppState {
    pub fn new(cfg: AppStateConfig) -> Self {
        Self {
            mode: cfg.initial_mode,
            monitor_images: cfg.monitor_images,
            focused_monitor_idx: cfg.focused_monitor_idx,
            window_to_monitor: cfg.window_to_monitor,
            windows: cfg.windows,
            layers: cfg.layers,
            monitors: cfg.monitors,
            result: cfg.result,
            repaint_ticks: 6,
            glyphs: cfg.glyphs,
            toolbar_position: cfg.toolbar_position,
            border_style: cfg.border_style,
            colors: cfg.colors,
            freeze_buttons: cfg.freeze_buttons,
            use_toplevel_export: cfg.use_toplevel_export,
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ModeSelected(CaptureMode::All) => {
                *self.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(None);
                return iced::exit();
            }
            Message::ModeSelected(mode) => {
                self.mode = mode;
                crate::domain::state::save_last_mode(mode);
            }
            Message::SelectionConfirmed(rect) => {
                *self.result.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(Some(FreezeSelection::Region(rect)));
                return iced::exit();
            }
            Message::ToplevelWindowSelected(window) => {
                *self.result.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(Some(FreezeSelection::ToplevelWindow(window)));
                return iced::exit();
            }
            Message::Cancel => {
                return iced::exit();
            }
            Message::Tick => {
                self.repaint_ticks = self.repaint_ticks.saturating_sub(1);
            }
            // Layershell control variants generated by macro — no-op
            _ => {}
        }
        Task::none()
    }

    /// Build the view for a specific window.
    /// Looks up which monitor that window is on (defaults to focused monitor)
    /// so the correct image slice and coordinate offset are used.
    pub fn view_for_window(&self, window_id: iced::window::Id) -> Element<'_, Message> {
        let mon_idx = self
            .window_to_monitor
            .get(&window_id)
            .copied()
            .unwrap_or(self.focused_monitor_idx);

        let monitor = &self.monitors[mon_idx];
        let monitor_offset = Point {
            x: monitor.rect.x as f32,
            y: monitor.rect.y as f32,
        };

        let canvas_prog = SelectionCanvas {
            mode: self.mode,
            windows: Arc::clone(&self.windows),
            layers: Arc::clone(&self.layers),
            monitors: Arc::clone(&self.monitors),
            monitor_offset,
            border_style: self.border_style,
            colors: self.colors,
            use_toplevel_export: self.use_toplevel_export,
        };

        let toolbar = self.toolbar();

        let base_stack = stack![
            Image::new(self.monitor_images[mon_idx].clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(ContentFit::Fill),
            Canvas::new(canvas_prog)
                .width(Length::Fill)
                .height(Length::Fill),
        ];

        if let Some(tb) = toolbar {
            let positioned_toolbar: Element<'_, Message> = Container::new(tb)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(12)
                .align_x(match self.toolbar_position {
                    ToolbarPosition::Left => iced::alignment::Horizontal::Left,
                    ToolbarPosition::Right => iced::alignment::Horizontal::Right,
                    _ => iced::alignment::Horizontal::Center,
                })
                .align_y(match self.toolbar_position {
                    ToolbarPosition::Top => iced::alignment::Vertical::Top,
                    ToolbarPosition::Bottom => iced::alignment::Vertical::Bottom,
                    _ => iced::alignment::Vertical::Center,
                })
                .into();
            base_stack.push(positioned_toolbar).into()
        } else {
            base_stack.into()
        }
    }

    /// Builds the toolbar widget, or returns `None` if all buttons are disabled.
    fn toolbar(&self) -> Option<Element<'_, Message>> {
        if !self.freeze_buttons.any_visible() {
            return None;
        }

        let glyph_size = self.glyphs.size;
        let btn = |label: &str, mode: CaptureMode, active: bool| {
            let btn_colors = self.colors.button;
            let (base_bg, base_text) = if active {
                (btn_colors.active_background, btn_colors.active_text)
            } else {
                (btn_colors.idle_background, btn_colors.idle_text)
            };
            let hover_bg = btn_colors.hover_background;
            let hover_text = btn_colors.hover_text;
            button(Text::new(label.to_owned()).size(glyph_size))
                .on_press(Message::ModeSelected(mode))
                .style(move |_theme, status| {
                    let (bg, text) = match status {
                        button::Status::Hovered | button::Status::Pressed => (hover_bg, hover_text),
                        _ => (base_bg, base_text),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(to_iced(bg))),
                        text_color: to_iced(text),
                        border: iced::Border {
                            radius: 2.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .padding([10, 18])
        };

        let cancel_colors = self.colors.cancel_button;
        let cancel_btn = {
            let idle_bg = cancel_colors.idle_background;
            let idle_text = cancel_colors.idle_text;
            let hover_bg = cancel_colors.hover_background;
            let hover_text = cancel_colors.hover_text;
            button(Text::new(self.glyphs.cancel.as_str()).size(glyph_size))
                .on_press(Message::Cancel)
                .style(move |_theme, status| {
                    let (bg, text) = match status {
                        button::Status::Hovered | button::Status::Pressed => (hover_bg, hover_text),
                        _ => (idle_bg, idle_text),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(to_iced(bg))),
                        text_color: to_iced(text),
                        border: iced::Border {
                            radius: 2.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .padding([10, 18])
        };

        let vertical = matches!(
            self.toolbar_position,
            ToolbarPosition::Left | ToolbarPosition::Right
        );

        let mut children: Vec<Element<'_, Message>> = Vec::with_capacity(5);
        if self.freeze_buttons.crop {
            children.push(
                btn(
                    &self.glyphs.crop,
                    CaptureMode::Crop,
                    self.mode == CaptureMode::Crop,
                )
                .into(),
            );
        }
        if self.freeze_buttons.window {
            children.push(
                btn(
                    &self.glyphs.window,
                    CaptureMode::Window,
                    self.mode == CaptureMode::Window,
                )
                .into(),
            );
        }
        if self.freeze_buttons.monitor {
            children.push(
                btn(
                    &self.glyphs.monitor,
                    CaptureMode::Monitor,
                    self.mode == CaptureMode::Monitor,
                )
                .into(),
            );
        }
        if self.freeze_buttons.all {
            children.push(
                btn(
                    &self.glyphs.all,
                    CaptureMode::All,
                    self.mode == CaptureMode::All,
                )
                .into(),
            );
        }
        if self.freeze_buttons.cancel {
            children.push(cancel_btn.into());
        }

        let buttons: Element<'_, Message> = if vertical {
            Column::with_children(children).spacing(8).into()
        } else {
            Row::with_children(children).spacing(8).into()
        };

        let toolbar_bg = self.colors.toolbar.background;
        Some(
            Container::new(buttons)
                .style(move |_theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(to_iced(toolbar_bg))),
                    border: iced::Border {
                        radius: 12.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .padding([8, 14])
                .into(),
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let keyboard = listen_with(|event, _status, _id| match event {
            iced::Event::Keyboard(KeyEvent::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            }) => Some(Message::Cancel),
            _ => None,
        });

        if self.repaint_ticks > 0 {
            // iced_layershell calls request_refresh_all(NextFrame) whenever
            // self.messages is non-empty. Subscribing to frames() causes
            // Tick messages to arrive after each RedrawRequested broadcast
            // (including the one emitted during the initial configure), which
            // gives wgpu surfaces that failed their first present() a second
            // (and third, …) chance to show content. Once repaint_ticks
            // reaches zero the subscription is dropped and rendering becomes
            // fully event-driven again.
            Subscription::batch([keyboard, iced::window::frames().map(|_| Message::Tick)])
        } else {
            keyboard
        }
    }
}

// ── Drawing helpers ───────────────────────────────────────────────────────────

/// Convert an [`RgbaColor`] config value to an [`iced::Color`].
fn to_iced(c: RgbaColor) -> iced::Color {
    iced::Color::from_rgba(c.0[0], c.0[1], c.0[2], c.0[3])
}

fn draw_selection(frame: &mut canvas::Frame, start: Point, end: Point, colors: &CropFrameColors) {
    let x = start.x.min(end.x);
    let y = start.y.min(end.y);
    let w = (start.x - end.x).abs();
    let h = (start.y - end.y).abs();

    let path = canvas::Path::rectangle(
        Point { x, y },
        iced::Size {
            width: w,
            height: h,
        },
    );
    frame.stroke(
        &path,
        canvas::Stroke::default()
            .with_color(to_iced(colors.stroke))
            .with_width(1.5),
    );

    frame.fill_text(canvas::Text {
        content: format!("{} × {}", w as i32, h as i32),
        position: Point {
            x: x + 4.0,
            y: (y - 20.0).max(2.0),
        },
        size: iced::Pixels(13.0),
        color: to_iced(colors.label_text),
        ..canvas::Text::default()
    });
}

fn draw_highlight(
    frame: &mut canvas::Frame,
    rect: ScreenRect,
    hovered: bool,
    label: &str,
    offset: Point,
    border_style: BorderStyle,
    colors: &WindowFrameColors,
) {
    let (fill, stroke, stroke_w) = if hovered {
        (colors.fill_hovered, colors.stroke_hovered, 2.0f32)
    } else {
        (colors.fill_idle, colors.stroke_idle, 1.0f32)
    };

    let expanded = rect.expand(border_style.border_size);
    // Convert global → canvas-local by subtracting monitor origin
    let x = expanded.x as f32 - offset.x;
    let y = expanded.y as f32 - offset.y;
    let w = expanded.w as f32;
    let h = expanded.h as f32;

    let size = iced::Size {
        width: w,
        height: h,
    };
    let top_left = Point { x, y };
    let radius = iced::border::Radius::from(border_style.rounding as f32);

    let path = if border_style.rounding > 0 {
        canvas::Path::rounded_rectangle(top_left, size, radius)
    } else {
        canvas::Path::rectangle(top_left, size)
    };

    frame.fill(&path, to_iced(fill));
    frame.stroke(
        &path,
        canvas::Stroke::default()
            .with_color(to_iced(stroke))
            .with_width(stroke_w),
    );

    if hovered && !label.is_empty() {
        let font_size = (h * 0.12).clamp(12.0, 22.0);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        frame.fill_text(canvas::Text {
            content: label.to_owned(),
            position: Point { x: cx, y: cy },
            size: iced::Pixels(font_size),
            color: to_iced(colors.label_text),
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            ..canvas::Text::default()
        });

        frame.fill_text(canvas::Text {
            content: "Click to capture".to_owned(),
            position: Point {
                x: cx,
                y: cy + font_size * 0.7,
            },
            size: iced::Pixels((font_size * 0.45).clamp(10.0, 14.0)),
            color: to_iced(colors.hint_text),
            align_x: iced::alignment::Horizontal::Center.into(),
            ..canvas::Text::default()
        });
    }
}

/// Monitor-mode highlight: always shows the monitor name centred so the user
/// can see which monitor they are on even when they cannot compare both
/// canvases side-by-side.
fn draw_monitor_highlight(
    frame: &mut canvas::Frame,
    rect: ScreenRect,
    hovered: bool,
    label: &str,
    offset: Point,
    colors: &MonitorFrameColors,
) {
    let x = rect.x as f32 - offset.x;
    let y = rect.y as f32 - offset.y;
    let w = rect.w as f32;
    let h = rect.h as f32;

    let fill = if hovered {
        colors.fill_hovered
    } else {
        colors.fill_idle
    };
    let (stroke, stroke_w) = if hovered {
        (colors.stroke_hovered, 3.0f32)
    } else {
        (colors.stroke_idle, 1.0f32)
    };
    let size = iced::Size {
        width: w,
        height: h,
    };
    let top_left = Point { x, y };
    let path = canvas::Path::rectangle(top_left, size);
    frame.fill(&path, to_iced(fill));
    frame.stroke(
        &path,
        canvas::Stroke::default()
            .with_color(to_iced(stroke))
            .with_width(stroke_w),
    );

    // Monitor name — always visible, centred, sized relative to monitor
    if !label.is_empty() {
        let font_size = (h * 0.06).clamp(18.0, 48.0);
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let name_color = if hovered {
            colors.label_text
        } else {
            colors.name_text_idle
        };
        frame.fill_text(canvas::Text {
            content: label.to_owned(),
            position: Point { x: cx, y: cy },
            size: iced::Pixels(font_size),
            color: to_iced(name_color),
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            ..canvas::Text::default()
        });

        if hovered {
            frame.fill_text(canvas::Text {
                content: "Click to capture".to_owned(),
                position: Point {
                    x: cx,
                    y: cy + font_size * 0.7,
                },
                size: iced::Pixels((font_size * 0.45).clamp(12.0, 20.0)),
                color: to_iced(colors.hint_text),
                align_x: iced::alignment::Horizontal::Center.into(),
                ..canvas::Text::default()
            });
        }
    }
}

// ── Geometry helpers ──────────────────────────────────────────────────────────

fn points_to_rect(a: Point, b: Point) -> ScreenRect {
    ScreenRect {
        x: a.x.min(b.x) as i32,
        y: a.y.min(b.y) as i32,
        w: finite_abs_to_i32(a.x - b.x),
        h: finite_abs_to_i32(a.y - b.y),
    }
}

/// Convert a f32 difference to a non-negative i32 dimension.
/// Returns 0 for NaN/infinite values rather than producing garbage via `as`.
#[inline]
fn finite_abs_to_i32(v: f32) -> i32 {
    if v.is_finite() {
        v.abs().clamp(0.0, i32::MAX as f32) as i32
    } else {
        0
    }
}

fn hit_index(
    windows: &[WindowInfo],
    pos: Option<Point>,
    offset: Point,
    border_size: u32,
) -> Option<usize> {
    let p = pos?;
    // Convert canvas-local cursor to global for comparison with hyprctl rects
    let gx = p.x + offset.x;
    let gy = p.y + offset.y;

    // Single pass: floating windows sort before tiled (!floating = false < true),
    // then by focus_history_id ascending (0 = most recently focused = topmost).
    windows
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            let r = w.rect.expand(border_size);
            gx >= r.x as f32
                && gx < (r.x + r.w) as f32
                && gy >= r.y as f32
                && gy < (r.y + r.h) as f32
        })
        .min_by_key(|(_, w)| (!w.floating, w.focus_history_id))
        .map(|(i, _)| i)
}

fn hit_index_m(monitors: &[MonitorInfo], pos: Option<Point>, offset: Point) -> Option<usize> {
    let p = pos?;
    let gx = p.x + offset.x;
    let gy = p.y + offset.y;
    monitors.iter().position(|m| {
        let r = m.rect;
        gx >= r.x as f32 && gx <= (r.x + r.w) as f32 && gy >= r.y as f32 && gy <= (r.y + r.h) as f32
    })
}

fn hit_index_layer(layers: &[LayerSurface], pos: Option<Point>, offset: Point) -> Option<usize> {
    let p = pos?;
    let gx = p.x + offset.x;
    let gy = p.y + offset.y;
    layers.iter().position(|l| {
        let r = l.rect;
        gx >= r.x as f32 && gx <= (r.x + r.w) as f32 && gy >= r.y as f32 && gy <= (r.y + r.h) as f32
    })
}

// ── Top-level fn items (satisfy for<'a> HRTB that closures cannot) ────────────

pub fn app_update(state: &mut AppState, msg: Message) -> Task<Message> {
    state.update(msg)
}

pub fn app_view(state: &AppState, window: iced::window::Id) -> iced::Element<'_, Message> {
    state.view_for_window(window)
}

pub fn app_subscription(state: &AppState) -> Subscription<Message> {
    state.subscription()
}
