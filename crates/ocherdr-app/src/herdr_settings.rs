//! A full Herdr client rendered in a dedicated surface, independent of pane
//! observe/control streams. Herdr owns the settings UI and persistence.
use super::*;
use futures::{
    future::{self, Either, poll_fn},
    pin_mut,
};
use ocherdr_herdr::{TerminalEndpoint, TerminalEvent, TerminalEventReceiver, next_batch};
use std::{ops::Range, task::Poll};

pub(crate) struct HerdrSettings {
    endpoint: TerminalEndpoint,
    protocol: u32,
    title: String,
    focus: FocusHandle,
    parent: WeakEntity<OcHerdrView>,
    i18n: I18n,
    terminal: Option<Terminal>,
    session: Option<TerminalSession>,
    task: Option<Task<()>>,
    frame: Option<RenderedFrame>,
    bounds: Option<Bounds<ochub_ui::gpui::Pixels>>,
    pixels: (u32, u32, u32),
    error: Option<String>,
    marked: Option<String>,
    mouse_capture: (bool, bool),
    report_all: bool,
}

impl HerdrSettings {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        endpoint: TerminalEndpoint,
        protocol: u32,
        title: String,
        palette: TerminalPalette,
        focus: FocusHandle,
        parent: WeakEntity<OcHerdrView>,
        i18n: I18n,
        _cx: &mut Context<Self>,
    ) -> Self {
        let (terminal, error) = match Terminal::new(80, 24, 1000, &palette) {
            Ok(terminal) => (Some(terminal), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            endpoint,
            protocol,
            title,
            focus,
            parent,
            i18n,
            terminal,
            error,
            session: None,
            task: None,
            frame: None,
            bounds: None,
            pixels: (0, 0, 0),
            marked: None,
            mouse_capture: (true, false),
            report_all: false,
        }
    }

    fn measure(
        &mut self,
        bounds: Bounds<ochub_ui::gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bounds = Some(bounds);
        let scale = window.scale_factor();
        let pixels = (
            (f32::from(bounds.size.width) * scale).round() as u32,
            (f32::from(bounds.size.height) * scale).round() as u32,
            (scale * 1000.) as u32,
        );
        if pixels.0 == 0 || pixels.1 == 0 || pixels == self.pixels || self.error.is_some() {
            return;
        }
        self.pixels = pixels;
        let Some(terminal) = &self.terminal else {
            return;
        };
        let grid = terminal.resize_pixels(pixels.0, pixels.1, f64::from(scale), 1);
        if self.session.is_none() {
            let (session, events) = TerminalSession::spawn_settings(
                self.endpoint.clone(),
                self.protocol,
                grid.columns,
                grid.rows,
            );
            self.session = Some(session);
            self.task = Some(Self::listen(events, cx));
        }
        self.send(TerminalCommand::Resize {
            cols: grid.columns,
            rows: grid.rows,
            cell_width_px: grid.cell_width_px,
            cell_height_px: grid.cell_height_px,
        });
    }

    fn listen(mut events: TerminalEventReceiver, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let server = next_batch(&mut events);
                let surface = poll_fn(|task_cx| {
                    this.update(cx, |this, _| {
                        this.terminal
                            .as_mut()
                            .map_or(Poll::Ready(None), |terminal| terminal.poll_frame(task_cx))
                    })
                    .unwrap_or(Poll::Ready(None))
                });
                pin_mut!(server, surface);
                let keep = match future::select(server, surface).await {
                    Either::Left((batch, _)) => this
                        .update(cx, |this, cx| {
                            let Some(batch) = batch else {
                                this.disconnected(cx);
                                return false;
                            };
                            for event in batch {
                                match event {
                                    Ok(TerminalEvent::Frame(frame)) => {
                                        if let Some(terminal) = &this.terminal {
                                            // Ignore old-size frames in flight during a resize.
                                            let size = terminal.surface_size();
                                            if (frame.width, frame.height)
                                                != (size.columns, size.rows)
                                            {
                                                continue;
                                            }
                                            terminal.apply_frame(&frame.bytes, frame.full);
                                            if frame.full {
                                                terminal.set_mouse_capture(
                                                    this.mouse_capture.0,
                                                    this.mouse_capture.1,
                                                );
                                                terminal
                                                    .set_kitty_keyboard_report_all(this.report_all);
                                            }
                                        }
                                    }
                                    Ok(TerminalEvent::MouseCapture {
                                        enabled,
                                        sgr_pixels,
                                    }) => {
                                        this.mouse_capture = (enabled, sgr_pixels);
                                        if let Some(terminal) = &this.terminal {
                                            terminal.set_mouse_capture(enabled, sgr_pixels);
                                        }
                                    }
                                    Ok(TerminalEvent::KittyKeyboardReportAll { enabled }) => {
                                        this.report_all = enabled;
                                        if let Some(terminal) = &this.terminal {
                                            terminal.set_kitty_keyboard_report_all(enabled);
                                        }
                                    }
                                    Ok(TerminalEvent::Notify { .. }) => {}
                                    Err(error) => {
                                        this.error = Some(error.to_string());
                                        this.session = None;
                                        cx.notify();
                                        return false;
                                    }
                                }
                            }
                            this.flush(cx);
                            true
                        })
                        .unwrap_or(false),
                    Either::Right((frame, _)) => this
                        .update(cx, |this, cx| match frame {
                            Some(Ok(frame)) => {
                                this.frame = Some(frame);
                                this.flush(cx);
                                true
                            }
                            Some(Err(error)) => {
                                this.error = Some(error.to_string());
                                this.session = None;
                                cx.notify();
                                false
                            }
                            None => {
                                this.disconnected(cx);
                                false
                            }
                        })
                        .unwrap_or(false),
                };
                if !keep {
                    break;
                }
            }
        })
    }

    fn disconnected(&mut self, cx: &mut Context<Self>) {
        self.error = Some(self.i18n.text(k::HERDR_SETTINGS_DISCONNECTED).to_string());
        self.session = None;
        cx.notify();
    }

    fn send(&mut self, command: TerminalCommand) {
        if let Some(session) = &self.session
            && let Err(error) = session.send(command)
        {
            self.error = Some(error.to_string());
        }
    }

    fn flush(&mut self, cx: &mut Context<Self>) {
        let _ = Terminal::tick_runtime();
        while let Some(bytes) = self.terminal.as_ref().and_then(Terminal::try_input) {
            self.send(TerminalCommand::Input(bytes));
        }
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if key.modifiers.platform && key.key == "w" {
            self.close(window, cx);
        } else if self.marked.is_none() {
            if let Some(terminal) = &self.terminal {
                if (key.modifiers.platform || key.modifiers.control) && key.key == "v" {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        terminal.paste(&text);
                    }
                } else if key.modifiers.platform && key.key == "c" {
                    if let Some(text) = terminal.read_selection() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                } else {
                    terminal.send_key(
                        if event.is_held {
                            KeyAction::Repeat
                        } else {
                            KeyAction::Press
                        },
                        &key.key,
                        key.key_char.as_deref(),
                        controller::gpui_key_modifiers(key.modifiers),
                    );
                }
            }
            self.flush(cx);
        }
        cx.stop_propagation();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The widget can be retained by a rendered frame after removal.
        // Release the connection now, rather than relying on the entity drop.
        self.task = None;
        self.session = None;
        self.parent
            .update(cx, |parent, cx| parent.close_herdr_settings(window, cx))
            .ok();
    }

    fn mouse_position(
        &self,
        position: ochub_ui::gpui::Point<ochub_ui::gpui::Pixels>,
        modifiers: ochub_ui::gpui::Modifiers,
        window: &Window,
    ) {
        if let (Some(bounds), Some(terminal)) = (self.bounds, &self.terminal) {
            let point = position - bounds.origin;
            let scale = f64::from(window.scale_factor());
            terminal.mouse_pos(
                f64::from(f32::from(point.x)) * scale,
                f64::from(f32::from(point.y)) * scale,
                controller::gpui_key_modifiers(modifiers),
            );
        }
    }
}

impl Render for HerdrSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let measure = cx.entity();
        let input = cx.entity();
        let focus = self.focus.clone();
        div()
            .id("herdr-settings")
            .role(ochub_ui::gpui::Role::Dialog)
            .aria_label(self.i18n.text(k::HERDR_SETTINGS_TITLE))
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .bg(theme::panel())
            .text_color(theme::text())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_key_up(
                cx.listener(|this, event: &ochub_ui::gpui::KeyUpEvent, _, cx| {
                    if this.marked.is_none()
                        && let Some(terminal) = &this.terminal
                    {
                        terminal.send_key(
                            KeyAction::Release,
                            &event.keystroke.key,
                            None,
                            controller::gpui_key_modifiers(event.keystroke.modifiers),
                        );
                        this.flush(cx);
                    }
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .pl(px(78.))
                    .pr_4()
                    .py_3()
                    .child(
                        div()
                            .flex_1()
                            .child(self.i18n.text(k::HERDR_SETTINGS_TITLE))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child(self.title.clone()),
                            ),
                    )
                    .child(
                        button(
                            "close-herdr-settings",
                            self.i18n.text(k::COMMON_CLOSE),
                            ButtonTone::Neutral,
                            ButtonSize::Sm,
                        )
                        .debug_selector(|| "close-herdr-settings".into())
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .pb_2()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(self.i18n.text(k::HERDR_SETTINGS_HINT)),
            )
            .child(
                div()
                    .id("herdr-settings-surface")
                    .debug_selector(|| "herdr-settings-surface".into())
                    .role(ochub_ui::gpui::Role::Terminal)
                    .aria_label(self.i18n.text(k::HERDR_SETTINGS_TITLE))
                    .aria_value(
                        self.terminal
                            .as_ref()
                            .and_then(Terminal::read_visible_text)
                            .unwrap_or_default(),
                    )
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(theme::current().bg.rgba())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.focus.focus(window, cx);
                            this.mouse_position(event.position, event.modifiers, window);
                            if let Some(terminal) = &this.terminal {
                                terminal.mouse_button(
                                    true,
                                    SurfaceMouseButton::Left,
                                    controller::gpui_key_modifiers(event.modifiers),
                                );
                            }
                            this.flush(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        this.mouse_position(event.position, event.modifiers, window);
                        this.flush(cx);
                        cx.stop_propagation();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseUpEvent, window, cx| {
                            this.mouse_position(event.position, event.modifiers, window);
                            if let Some(terminal) = &this.terminal {
                                terminal.mouse_button(
                                    false,
                                    SurfaceMouseButton::Left,
                                    controller::gpui_key_modifiers(event.modifiers),
                                );
                            }
                            this.flush(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        // Herdr's full UI consumes cell-based SGR wheel events.
                        if let (Some(bounds), Some(terminal)) = (this.bounds, &this.terminal) {
                            let size = terminal.surface_size();
                            let point = event.position - bounds.origin;
                            let scale = window.scale_factor();
                            let column = (f32::from(point.x) * scale
                                / size.cell_width_px.max(1) as f32)
                                as u32
                                + 1;
                            let row = (f32::from(point.y) * scale
                                / size.cell_height_px.max(1) as f32)
                                as u32
                                + 1;
                            let delta = event.delta.pixel_delta(px(16.));
                            if delta.y == px(0.) {
                                cx.stop_propagation();
                                return;
                            }
                            let button = if delta.y > px(0.) { 64 } else { 65 };
                            this.send(TerminalCommand::Input(
                                format!("\x1b[<{button};{column};{row}M").into_bytes(),
                            ));
                        }
                        cx.stop_propagation();
                    }))
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                measure.update(cx, |this, cx| this.measure(bounds, window, cx))
                            },
                            move |bounds, _, window, cx| {
                                if focus.is_focused(window) {
                                    window.handle_input(
                                        &focus,
                                        ElementInputHandler::new(bounds, input.clone()),
                                        cx,
                                    );
                                }
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(
                        self.frame
                            .clone()
                            .map(|frame| terminal_frame_element(frame, None)),
                    )
                    .when(self.frame.is_none() && self.error.is_none(), |surface| {
                        surface.child(div().p_4().child(self.i18n.text(k::TERMINAL_WAITING)))
                    })
                    .when_some(self.error.clone(), |surface, error| {
                        surface.child(
                            div()
                                .absolute()
                                .inset_0()
                                .p_4()
                                .bg(theme::panel())
                                .child(error),
                        )
                    }),
            )
    }
}

impl EntityInputHandler for HerdrSettings {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text: Vec<u16> = self
            .marked
            .as_deref()
            .unwrap_or("")
            .encode_utf16()
            .collect();
        let start = range.start.min(text.len());
        let end = range.end.min(text.len()).max(start);
        *adjusted = Some(start..end);
        Some(String::from_utf16_lossy(&text[start..end]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        if let Some(terminal) = &self.terminal {
            terminal.set_preedit(None);
        }
        self.flush(cx);
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.unmark_text(window, cx);
        self.send(TerminalCommand::Input(text.as_bytes().to_vec()));
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = Some(text.to_owned());
        if let Some(terminal) = &self.terminal {
            terminal.set_preedit(Some(text));
        }
        window.invalidate_character_coordinates();
        self.flush(cx);
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<ochub_ui::gpui::Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<ochub_ui::gpui::Pixels>> {
        let terminal = self.terminal.as_ref()?;
        let (x, y, w, h) = terminal.ime_point();
        let scale = f64::from(window.scale_factor());
        Some(Bounds {
            origin: bounds.origin + point(px((x / scale) as f32), px((y / scale) as f32)),
            size: size(px((w / scale) as f32), px((h / scale) as f32)),
        })
    }
    fn character_index_for_point(
        &mut self,
        _: ochub_ui::gpui::Point<ochub_ui::gpui::Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        self.error.is_none()
    }
}
