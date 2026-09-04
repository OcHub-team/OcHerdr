use super::*;

impl OcHerdrView {
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        if !runtime.terminal.has_selection() {
            return;
        }
        let Some(text) = runtime.terminal.read_selection() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.stop_propagation();
    }

    pub(crate) fn select_all_visible(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let Some(runtime) = self.pane_mut(&pane_id) else {
            return;
        };
        if !runtime.terminal.select_all_visible() {
            return;
        }
        flush_pane_surface(runtime);
        copy_terminal_selection(runtime, cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn scroll_pane(
        &mut self,
        pane_id: &str,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        // Wheel input is a direct interaction just like a click or key press:
        // promote only the pane under the pointer and keep any other pane
        // controls intact.
        self.take_terminal_control(pane_id.to_owned(), cx);
        let Some(runtime) = self.pane_mut(pane_id) else {
            return;
        };
        // GPUI wheel pixels and measured body bounds are both logical pixels.
        // pixel_size is physical: using it halves scrolling on a Retina display.
        let line_height = logical_scroll_line_height(runtime.body_bounds.3, runtime.size.1);
        let lines = wheel_scroll_lines(event.delta, line_height, &mut runtime.scroll_px);
        if lines == 0 {
            cx.stop_propagation();
            return;
        }
        let direction = if lines > 0 {
            TerminalScrollDirection::Up
        } else {
            TerminalScrollDirection::Down
        };
        let _ = runtime.session.send(TerminalCommand::Scroll {
            direction,
            lines: lines.unsigned_abs().min(u32::from(u16::MAX)) as u16,
        });
        cx.stop_propagation();
    }

    pub(crate) fn invoke(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        if let Some(request) = self.spawn_invoke(method, params, cx) {
            request.detach();
        }
    }

    /// Same request as `invoke`, but the whole `result` object (or the error)
    /// is handed to `on_response` on the main thread once the socket answers.
    /// Failure side effects (notice, reorder gate, worktree force-remove offer)
    /// happen before the callback runs, so callers only chain the next step.
    pub(crate) fn invoke_with_response(
        &mut self,
        method: &'static str,
        params: Value,
        on_response: impl FnOnce(&mut Self, std::result::Result<Value, HerdrError>, &mut Context<Self>)
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if let Some(request) =
            self.spawn_invoke_inner(method, params, Some(Box::new(on_response)), cx)
        {
            request.detach();
        }
    }

    /// Same request as `invoke`, but the caller keeps the task so it can tie the
    /// request's lifetime to the state that request is allowed to block.
    pub(super) fn spawn_invoke(
        &mut self,
        method: &'static str,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        self.spawn_invoke_inner(method, params, None, cx)
    }

    pub(super) fn spawn_invoke_inner(
        &mut self,
        method: &'static str,
        params: Value,
        on_response: Option<InvokeResponseCallback>,
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        let connection = self.connection.as_ref()?;
        let socket = connection.socket_path().to_owned();
        self.operation = Some(self.i18n.running_operation(method).into());
        Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let params = params.clone();
                    async move { request_socket(&socket, method, params) }
                })
                .await;
            this.update(cx, |this, cx| {
                this.operation = None;
                match &result {
                    Ok(_) => {
                        if command_needs_snapshot_resync(method) {
                            this.resync_snapshot(this.event_epoch, cx);
                        }
                    }
                    Err(error) => {
                        // A rejected move never produces a `moved` event, so the
                        // gate has to open here or it would never open at all.
                        this.pending_reorder = None;
                        this.note_unsupported_method(method, error);
                        this.maybe_offer_force_remove_worktree(&params, error, cx);
                        this.notify_command_failure(method, error, cx);
                    }
                }
                if let Some(on_response) = on_response {
                    on_response(this, result, cx);
                }
                cx.notify();
            })
            .ok();
        }))
    }

    #[allow(dead_code)] // consumed by the pane drag drop-zone gating (design §8)
    pub(crate) fn pane_move_supported(&self) -> bool {
        self.connection.is_some() && self.herdr_capabilities.pane_move
    }

    /// The snapshot said `pane.move` exists but Herdr rejected the method
    /// itself: degrade to swap-only for the rest of this connection.
    pub(super) fn note_unsupported_method(&mut self, method: &str, error: &HerdrError) {
        if method != "pane.move" || !self.herdr_capabilities.pane_move {
            return;
        }
        if is_unknown_method_error(error) {
            self.herdr_capabilities.pane_move = false;
            eprintln!(
                "ocherdr: Herdr rejected `pane.move` as an unknown method; disabling pane relocation for this connection ({error})"
            );
        }
    }

    pub(crate) fn accepts_ime(&self) -> bool {
        key_goes_to_terminal(&self.overlay)
            && self
                .selection
                .pane_id
                .as_deref()
                .and_then(|pane_id| self.pane(pane_id))
                .is_some()
    }

    pub(crate) fn commit_ime_text(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_ime_preedit(window, cx);
        if text.is_empty() {
            return;
        }
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        self.take_terminal_control(pane_id.clone(), cx);
        let stream_closed = {
            let Some(runtime) = self.pane_mut(&pane_id) else {
                return;
            };
            if !runtime.mode.is_controlled() {
                return;
            }
            let closed = runtime
                .session
                .send(TerminalCommand::Input(text.as_bytes().to_vec()))
                .is_err();
            if closed {
                runtime.exit_seen = true;
            }
            closed
        };
        window.invalidate_character_coordinates();
        cx.notify();
        if stream_closed {
            self.resync_snapshot(self.event_epoch, cx);
        }
    }

    pub(crate) fn set_ime_preedit(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            self.clear_ime_preedit(window, cx);
            return;
        }
        self.ime_marked = Some(text.to_owned());
        if let Some(pane_id) = self.selection.pane_id.clone()
            && let Some(runtime) = self.pane_mut(&pane_id)
        {
            runtime.terminal.set_preedit(Some(text));
            flush_pane_surface(runtime);
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    pub(crate) fn clear_ime_preedit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ime_marked.take().is_none() {
            return;
        }
        if let Some(pane_id) = self.selection.pane_id.clone()
            && let Some(runtime) = self.pane_mut(&pane_id)
        {
            runtime.terminal.set_preedit(None);
            flush_pane_surface(runtime);
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    pub(crate) fn ime_cursor_bounds(
        &self,
        window: &Window,
    ) -> Option<Bounds<ochub_ui::gpui::Pixels>> {
        let pane_id = self.selection.pane_id.as_deref()?;
        let runtime = self.pane(pane_id)?;
        let (x, y, width, height) = runtime.terminal.ime_point();
        let (left, top, w, h) = map_surface_rect_to_window(
            (x, y - height, width.max(1.0), height.max(1.0)),
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        )?;
        Some(Bounds {
            origin: point(px(left), px(top)),
            size: size(px(w.max(1.)), px(h.max(1.))),
        })
    }
}
