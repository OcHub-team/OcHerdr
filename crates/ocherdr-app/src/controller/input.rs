use super::*;

impl OcHerdrView {
    pub(crate) fn create_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(workspace_id) = self.selection.workspace_id.clone() {
            self.invoke_with_response(
                "tab.create",
                json!({ "workspace_id": workspace_id, "focus": true, "env": {} }),
                Self::follow_created_tab,
                cx,
            );
        }
    }

    pub(crate) fn cycle_tab(&mut self, offset: isize, cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(workspace_id) = self.selection.workspace_id.as_deref() else {
            return;
        };
        let hidden = self.hidden_tab_ids();
        let tab_ids = snapshot
            .tabs_for(workspace_id)
            .filter(|tab| !hidden.contains(&tab.tab_id))
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        if tab_ids.is_empty() {
            return;
        }
        let current = self
            .selection
            .tab_id
            .as_ref()
            .and_then(|tab_id| tab_ids.iter().position(|candidate| candidate == tab_id))
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(tab_ids.len() as isize) as usize;
        self.select_tab(tab_ids[next].clone(), cx);
    }

    pub(crate) fn select_tab_number(&mut self, number: usize, cx: &mut Context<Self>) {
        let tab_id = self.snapshot.as_ref().and_then(|snapshot| {
            self.selection
                .workspace_id
                .as_deref()
                .and_then(|workspace_id| {
                    let hidden = self.hidden_tab_ids();
                    tab_id_for_shortcut(
                        snapshot
                            .tabs_for(workspace_id)
                            .filter(|tab| !hidden.contains(&tab.tab_id)),
                        number,
                    )
                })
        });
        if let Some(tab_id) = tab_id {
            self.select_tab(tab_id, cx);
        }
    }

    pub(crate) fn focus_pane_direction(&mut self, direction: &'static str, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.selection.pane_id.clone() {
            self.invoke(
                "pane.focus_direction",
                json!({ "pane_id": pane_id, "direction": direction }),
                cx,
            );
        }
    }

    pub(crate) fn handle_prefix_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prefix_pending = false;
        let key = event.keystroke.key.as_str();
        let shift = event.keystroke.modifiers.shift;
        match (key, shift) {
            ("escape", _) => {}
            ("s", false) => self.open_native_tui(cx),
            ("c", false) => self.create_tab(cx),
            ("n", true) => self.create_workspace(cx),
            ("n", false) => self.cycle_tab(1, cx),
            ("p", false) => self.cycle_tab(-1, cx),
            ("w", true) => {
                if let Some(target) = self.selected_workspace_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("d", true) => {
                if let Some(target) = self.selected_workspace_target() {
                    self.request_close(target, cx);
                }
            }
            ("t", true) => {
                if let Some(target) = self.selected_tab_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("x", true) => {
                if let Some(target) = self.selected_tab_target() {
                    self.request_close(target, cx);
                }
            }
            ("p", true) => {
                if let Some(target) = self.selected_pane_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("m", false) => self.enter_keyboard_pane_move(cx),
            ("h", false) => self.focus_pane_direction("left", cx),
            ("j", false) => self.focus_pane_direction("down", cx),
            ("k", false) => self.focus_pane_direction("up", cx),
            ("l", false) => self.focus_pane_direction("right", cx),
            ("j" | "down", true) => self.move_selected_workspace(1, cx),
            ("k" | "up", true) => self.move_selected_workspace(-1, cx),
            _ => {
                if let Some(number) =
                    tab_index_from_keystroke(key, event.keystroke.key_char.as_deref())
                {
                    self.select_tab_number(number, cx);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn handle_app_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control && !modifiers.platform && !modifiers.alt && key == "b" {
            self.prefix_pending = true;
            if matches!(self.overlay, Overlay::ContextMenu(_)) {
                self.set_overlay(Overlay::None, cx);
            }
            cx.notify();
            return true;
        }
        if self.prefix_pending {
            self.handle_prefix_key(event, window, cx);
            return true;
        }
        if self.pane_keyboard_move.is_some() && self.handle_keyboard_pane_move_key(event, cx) {
            return true;
        }
        if key == "escape" {
            if matches!(self.surface_drag, SurfaceDrag::Pane(_)) {
                self.cancel_pane_drag();
                cx.notify();
                return true;
            }
            if matches!(self.overlay, Overlay::Appearance) {
                self.close_appearance(window, cx);
                return true;
            }
            if matches!(self.overlay, Overlay::AgentPanel { .. }) {
                self.close_agent_panel(window, cx);
                return true;
            }
            if matches!(
                self.overlay,
                Overlay::ContextMenu(_) | Overlay::NodeManager | Overlay::HostSwitcher
            ) {
                self.set_overlay(Overlay::None, cx);
                self.focus.focus(window, cx);
                return true;
            }
            if matches!(
                self.overlay,
                Overlay::ConfirmClose(_)
                    | Overlay::ConfirmRemoveWorktree { .. }
                    | Overlay::WorktreeCreate { .. }
                    | Overlay::WorktreeOpen(_)
            ) {
                self.set_overlay(Overlay::None, cx);
                self.focus.focus(window, cx);
                return true;
            }
        }
        if modifiers.platform && !modifiers.alt && !modifiers.control {
            if let Some(number) = tab_index_from_keystroke(key, event.keystroke.key_char.as_deref())
            {
                self.select_tab_number(number, cx);
                return true;
            }
            let handled = match (key, modifiers.shift) {
                ("t", false) => {
                    self.create_tab(cx);
                    true
                }
                ("w", true) => {
                    if let Some(target) = self.selected_workspace_target() {
                        self.request_close(target, cx);
                    }
                    true
                }
                ("w", false) => {
                    if let Some(target) = self.cmd_w_close_target() {
                        self.request_close(target, cx);
                    }
                    true
                }
                ("n", true) => {
                    self.create_workspace(cx);
                    true
                }
                (",", false) => {
                    self.open_native_tui(cx);
                    true
                }
                ("c", false) => {
                    self.copy_selection(cx);
                    true
                }
                ("a", false) => {
                    self.select_all_visible(cx);
                    true
                }
                ("[", false) => {
                    self.cycle_tab(-1, cx);
                    true
                }
                ("]", false) => {
                    self.cycle_tab(1, cx);
                    true
                }
                _ => false,
            };
            if handled {
                return true;
            }
        }
        if modifiers.control && key == "tab" {
            self.cycle_tab(if modifiers.shift { -1 } else { 1 }, cx);
            return true;
        }
        if key == "f2" && !modifiers.platform && !modifiers.control && !modifiers.alt {
            let target = self
                .selected_tab_target()
                .or_else(|| self.selected_workspace_target());
            if let Some(target) = target {
                self.open_rename(target, window, cx);
            }
            return true;
        }
        false
    }

    pub(crate) fn send_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.handle_app_shortcut(event, window, cx) {
            // The matching key-up must not reach the terminal either.
            self.suppress_key_release = true;
            cx.stop_propagation();
            return;
        }
        if self.ime_marked.is_some() {
            return;
        }
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        self.take_terminal_control(pane_id.clone(), cx);
        let key = &event.keystroke;
        let stream_closed = {
            let Some(runtime) = self.pane_mut(&pane_id) else {
                return;
            };
            if !runtime.mode.is_controlled() {
                return;
            }
            if key.modifiers.platform && key.key == "v" {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    runtime.terminal.paste(&text);
                    cx.stop_propagation();
                    drain_terminal_input(runtime)
                } else {
                    false
                }
            } else {
                // Ghostty encodes the key for the modes the application
                // enabled (kitty keyboard protocol, modifyOtherKeys,
                // application cursor keys) and queues the pty bytes.
                let action = if event.is_held {
                    KeyAction::Repeat
                } else {
                    KeyAction::Press
                };
                if !runtime.terminal.send_key(
                    action,
                    &key.key,
                    key.key_char.as_deref(),
                    gpui_key_modifiers(key.modifiers),
                ) {
                    return;
                }
                cx.stop_propagation();
                drain_terminal_input(runtime)
            }
        };
        if stream_closed {
            self.resync_snapshot(self.event_epoch, cx);
        }
    }

    /// Key releases matter only to applications that asked the kitty
    /// keyboard protocol to report them; Ghostty decides.
    pub(crate) fn send_key_release(
        &mut self,
        event: &ochub_ui::gpui::KeyUpEvent,
        cx: &mut Context<Self>,
    ) {
        if std::mem::take(&mut self.suppress_key_release)
            || self.ime_marked.is_some()
            || self.prefix_pending
            || self.pane_keyboard_move.is_some()
        {
            return;
        }
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let key = &event.keystroke;
        let stream_closed = {
            let Some(runtime) = self.pane_mut(&pane_id) else {
                return;
            };
            if !runtime.terminal.send_key(
                KeyAction::Release,
                &key.key,
                None,
                gpui_key_modifiers(key.modifiers),
            ) {
                return;
            }
            drain_terminal_input(runtime)
        };
        if stream_closed {
            self.resync_snapshot(self.event_epoch, cx);
        }
    }

    /// Forward whatever Ghostty has queued for every pane's pty. Tests call
    /// this in place of the frame and event polls that do it in production.
    #[cfg(test)]
    pub(crate) fn pump_terminal_input(&mut self) {
        if let Some(session) = self.session_panes.as_mut() {
            for runtime in session.panes.values_mut() {
                flush_pane_surface(runtime);
            }
        }
    }

    pub(crate) fn pane_mouse_down(
        &mut self,
        pane_id: String,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.surface_drag,
            SurfaceDrag::Split(_) | SurfaceDrag::Reorder(_) | SurfaceDrag::Pane(_)
        ) {
            return;
        }
        self.end_text_drag_unless_pane(&pane_id);
        self.select_pane(pane_id.clone(), window, cx);
        self.take_terminal_control(pane_id.clone(), cx);
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        let mouse = mouse_point(event.position);
        if !point_in_rect(mouse, runtime.body_bounds) {
            self.surface_drag = SurfaceDrag::Idle;
            return;
        }
        let Some(surface) = map_mouse_to_surface(
            mouse,
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        ) else {
            self.surface_drag = SurfaceDrag::Idle;
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        let Some(runtime) = self.pane_mut(&pane_id) else {
            return;
        };
        let captured = runtime
            .terminal
            .begin_text_selection(surface.0, surface.1, modifiers);
        flush_pane_surface(runtime);
        self.surface_drag = SurfaceDrag::Text {
            pane_id: pane_id.clone(),
            captured,
        };
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn pane_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.update_split_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.update_reorder_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.update_pane_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        let SurfaceDrag::Text { pane_id, .. } = &self.surface_drag else {
            return;
        };
        let pane_id = pane_id.clone();
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        let Some(surface) = map_mouse_to_surface(
            mouse_point(event.position),
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        ) else {
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        if let Some(runtime) = self.pane_mut(&pane_id) {
            runtime
                .terminal
                .update_text_selection(surface.0, surface.1, modifiers);
            flush_pane_surface(runtime);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn pane_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.finish_split_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.finish_reorder_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.finish_pane_drag(mouse_point(event.position), window, cx) {
            cx.stop_propagation();
            return;
        }
        let SurfaceDrag::Text { pane_id, captured } =
            std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle)
        else {
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        if let Some(runtime) = self.pane_mut(&pane_id) {
            let point = map_mouse_to_surface(
                mouse_point(event.position),
                runtime.body_bounds,
                runtime.pixel_size,
                window.scale_factor(),
            );
            runtime.terminal.end_text_selection(point, modifiers);
            flush_pane_surface(runtime);
            if !captured {
                copy_terminal_selection(runtime, cx);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}
