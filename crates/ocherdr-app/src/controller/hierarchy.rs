use super::*;

impl OcHerdrView {
    pub(crate) fn cancel_remove_node(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ConfirmRemoveProfile(_)) {
            self.set_overlay(Overlay::NodeManager, cx);
        }
    }

    pub(crate) fn confirm_remove_node(&mut self, cx: &mut Context<Self>) {
        let Overlay::ConfirmRemoveProfile(id) = &self.overlay else {
            return;
        };
        let id = id.clone();
        self.set_overlay(Overlay::NodeManager, cx);
        self.host_center
            .update(cx, |center, cx| center.confirm_remove_node(&id, cx));
    }

    pub(crate) fn close_add_remote(&mut self, cx: &mut Context<Self>) {
        self.set_overlay(Overlay::NodeManager, cx);
    }

    pub(crate) fn request_close(&mut self, target: HierarchyTarget, cx: &mut Context<Self>) {
        // Closing a pane out from under a pending relocation would leave the
        // prediction pointing at a pane Herdr is about to drop.
        if let HierarchyTarget::Pane { id, .. } = &target
            && self
                .pane_tab_id(id)
                .is_some_and(|tab_id| self.tab_relocation_locked(&tab_id))
        {
            return;
        }
        self.set_overlay(Overlay::ConfirmClose(target), cx);
    }

    pub(crate) fn cancel_close(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ConfirmClose(_)) {
            self.set_overlay(Overlay::None, cx);
        }
    }

    /// Tracks Command for the tab strip's ⌘N hints.
    pub(crate) fn set_command_held(&mut self, held: bool, cx: &mut Context<Self>) {
        if self.command_held == held {
            return;
        }
        self.command_held = held;
        cx.notify();
    }

    /// Performs the focus move queued by `set_overlay`, from render where a
    /// `Window` is at hand. Deferred so the dialog is in the rendered frame
    /// before it is focused.
    pub(crate) fn apply_pending_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_focus.take() else {
            return;
        };
        let handle = match pending {
            PendingFocus::Dialog => self.dialog_focus.clone(),
            PendingFocus::Surface => self.focus.clone(),
        };
        window.defer(cx, move |window, cx| {
            if !handle.is_focused(window) {
                window.focus(&handle, cx);
            }
        });
    }

    pub(crate) fn handle_overlay_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(confirm) = overlay_confirm_or_cancel(event) else {
            return false;
        };
        match (self.overlay.clone(), confirm) {
            (Overlay::ConfirmClose(_), true) => self.confirm_close(cx),
            (Overlay::ConfirmClose(_), false) => self.cancel_close(cx),
            (Overlay::ConfirmRemoveWorktree { .. }, true) => self.confirm_remove_worktree(cx),
            (Overlay::ConfirmRemoveWorktree { .. }, false) => self.cancel_remove_worktree(cx),
            (Overlay::WorktreeCreate { .. }, true) => self.submit_worktree_create(window, cx),
            (Overlay::WorktreeCreate { .. }, false) => self.close_worktree_overlay(window, cx),
            (Overlay::WorktreeOpen(_), false) => self.close_worktree_overlay(window, cx),
            (Overlay::ConfirmRemoveProfile(_), true) => self.confirm_remove_node(cx),
            (Overlay::ConfirmRemoveProfile(_), false) => self.cancel_remove_node(cx),
            (Overlay::ConfirmBulkRemove, true) => self.confirm_bulk_remove(cx),
            (Overlay::ConfirmBulkRemove, false) => self.cancel_bulk_remove(cx),
            (Overlay::Rename(_), true) => self.submit_rename(window, cx),
            (Overlay::Rename(_), false) => self.cancel_rename(window, cx),
            (Overlay::ConfirmSwitchProfile { .. }, true) => self.confirm_switch_profile(cx),
            (Overlay::ConfirmSwitchProfile { .. }, false) => self.cancel_switch_profile(cx),
            (Overlay::RemoteForm(_), false) => self.close_add_remote(cx),
            (Overlay::HostSwitcher, false) => self.close_host_switcher(cx),
            (Overlay::Appearance, false) => self.close_appearance(window, cx),
            (Overlay::AgentPanel { .. }, false) => self.close_agent_panel(window, cx),
            (Overlay::Update(_), true) => self.confirm_update_dialog(cx),
            (Overlay::Update(_), false) => self.close_update_dialog(window, cx),
            (Overlay::ContextMenu(_) | Overlay::NodeManager, false) => {
                self.set_overlay(Overlay::None, cx);
                self.focus.focus(window, cx);
            }
            _ => return false,
        }
        cx.stop_propagation();
        true
    }

    pub(crate) fn confirm_close(&mut self, cx: &mut Context<Self>) {
        let Overlay::ConfirmClose(target) = &self.overlay else {
            return;
        };
        let target = target.clone();
        self.set_overlay(Overlay::None, cx);
        match target {
            HierarchyTarget::Workspace { id, .. } => {
                self.invoke("workspace.close", json!({ "workspace_id": id }), cx)
            }
            HierarchyTarget::Tab { id, .. } => {
                self.invoke("tab.close", json!({ "tab_id": id }), cx)
            }
            HierarchyTarget::Pane { id, .. } => {
                self.invoke("pane.close", json!({ "pane_id": id }), cx)
            }
        }
    }

    pub(crate) fn open_context_menu(
        &mut self,
        target: HierarchyTarget,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = window.viewport_size();
        self.set_overlay(
            Overlay::ContextMenu(HierarchyContextMenu {
                target,
                x: f32::from(event.position.x)
                    .min((f32::from(viewport.width) - 220.).max(8.))
                    .max(8.),
                y: f32::from(event.position.y)
                    .min((f32::from(viewport.height) - 260.).max(8.))
                    .max(8.),
                agent_details: false,
            }),
            cx,
        );
        cx.stop_propagation();
    }

    /// Secondary click on a sidebar agent row: the pane menu led by
    /// "Details", which is how the row still reaches the agent panel.
    pub(crate) fn open_agent_context_menu(
        &mut self,
        pane_id: String,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let label = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pane(&pane_id))
            .map(|pane| pane.display_name().to_owned())
            .unwrap_or_else(|| pane_id.clone());
        self.open_context_menu(
            HierarchyTarget::Pane { id: pane_id, label },
            event,
            window,
            cx,
        );
        if let Overlay::ContextMenu(menu) = &mut self.overlay {
            menu.agent_details = true;
        }
    }

    /// Click on a sidebar agent row: select its workspace, tab and pane
    /// locally (the same path as clicking the pane) and ask Herdr to focus
    /// it, so the TUI and other clients follow too.
    pub(crate) fn jump_to_agent_pane(
        &mut self,
        pane_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((workspace_id, tab_id)) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pane(&pane_id))
            .map(|pane| (pane.workspace_id.clone(), pane.tab_id.clone()))
        else {
            return;
        };
        if self.selection.workspace_id.as_deref() != Some(workspace_id.as_str())
            || self.selection.tab_id.as_deref() != Some(tab_id.as_str())
        {
            self.selection.workspace_id = Some(workspace_id);
            self.select_tab(tab_id, cx);
        }
        self.select_pane(pane_id.clone(), window, cx);
        self.invoke("agent.focus", json!({ "target": pane_id }), cx);
    }

    pub(crate) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ContextMenu(_)) {
            self.set_overlay(Overlay::None, cx);
        }
    }

    pub(crate) fn open_rename(
        &mut self,
        target: HierarchyTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = target.label().to_owned();
        self.rename_input
            .update(cx, |input, cx| input.set_content(label, cx));
        self.set_overlay(Overlay::Rename(target), cx);
        self.rename_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    pub(crate) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Rename(_)) {
            self.set_overlay(Overlay::None, cx);
        }
        self.focus.focus(window, cx);
    }

    pub(crate) fn submit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Rename(target) = &self.overlay else {
            return;
        };
        let target = target.clone();
        let label = self.rename_input.read(cx).content().trim().to_owned();
        if label.is_empty() && !matches!(target, HierarchyTarget::Pane { .. }) {
            self.notify_failure(
                FailureKind::EmptyWorkspaceOrTabName,
                self.i18n.text(k::NOTIFY_DETAIL_EMPTY_NAME),
                cx,
            );
            cx.notify();
            return;
        }
        self.set_overlay(Overlay::None, cx);
        match target {
            HierarchyTarget::Workspace { id, .. } => {
                self.invoke(
                    "workspace.rename",
                    json!({ "workspace_id": id, "label": label }),
                    cx,
                );
            }
            HierarchyTarget::Tab { id, .. } => {
                self.invoke_with_response(
                    "tab.rename",
                    json!({ "tab_id": id, "label": label }),
                    Self::apply_tab_rename_response,
                    cx,
                );
            }
            HierarchyTarget::Pane { id, .. } => {
                self.invoke(
                    "pane.rename",
                    json!({ "pane_id": id, "label": (!label.is_empty()).then_some(label) }),
                    cx,
                );
            }
        }
        self.focus.focus(window, cx);
    }

    fn apply_tab_rename_response(
        &mut self,
        result: std::result::Result<Value, HerdrError>,
        cx: &mut Context<Self>,
    ) {
        let Ok(result) = result else {
            return;
        };
        let Some(tab) = result.get("tab") else {
            self.resync_snapshot(self.event_epoch, cx);
            return;
        };
        let (Some(tab_id), Some(label)) = (
            tab.get("tab_id").and_then(Value::as_str),
            tab.get("label").and_then(Value::as_str),
        ) else {
            self.resync_snapshot(self.event_epoch, cx);
            return;
        };
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        let Some(current) = snapshot.tabs.iter_mut().find(|tab| tab.tab_id == tab_id) else {
            self.resync_snapshot(self.event_epoch, cx);
            return;
        };
        current.label = label.to_owned();
    }

    pub(crate) fn open_native_tui(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.current_session() else {
            self.notify_failure(
                FailureKind::NoSessionSelected,
                self.i18n.text(k::NOTIFY_DETAIL_NO_SESSION),
                cx,
            );
            cx.notify();
            return;
        };
        let command = attach_command(&self.current_profile(), &session.name);
        if let Err(error) = open_system_terminal(&command) {
            self.notify_failure(FailureKind::OpenTerminal, error, cx);
        }
        cx.notify();
    }

    pub(crate) fn select_workspace(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        self.cancel_split_drag();
        self.cancel_reorder_drag();
        self.cancel_pane_drag();
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        self.selection.workspace_id = Some(workspace_id.clone());
        self.selection.tab_id = snapshot
            .tabs_for(&workspace_id)
            .find(|tab| tab.focused)
            .or_else(|| snapshot.tabs_for(&workspace_id).next())
            .map(|tab| tab.tab_id.clone());
        self.selection.pane_id = self.selection.tab_id.as_deref().and_then(|tab_id| {
            snapshot
                .panes_for(tab_id)
                .find(|pane| pane.focused)
                .or_else(|| snapshot.panes_for(tab_id).next())
                .map(|pane| pane.pane_id.clone())
        });
        self.ensure_session_terminals(cx);
        cx.notify();
    }

    pub(crate) fn select_tab(&mut self, tab_id: String, cx: &mut Context<Self>) {
        self.cancel_split_drag();
        self.cancel_reorder_drag();
        self.cancel_pane_drag();
        self.cancel_keyboard_pane_move();
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let tab_index = self
            .selection
            .workspace_id
            .as_deref()
            .and_then(|workspace_id| {
                snapshot
                    .tabs_for(workspace_id)
                    .position(|tab| tab.tab_id == tab_id)
            });
        self.selection.tab_id = Some(tab_id.clone());
        self.selection.pane_id = snapshot
            .panes_for(&tab_id)
            .find(|pane| pane.focused)
            .or_else(|| snapshot.panes_for(&tab_id).next())
            .map(|pane| pane.pane_id.clone());
        if let Some(tab_index) = tab_index {
            self.tab_scroll.scroll_to_item(tab_index);
        }
        self.ensure_session_terminals(cx);
        cx.notify();
    }

    pub(crate) fn set_tab_hovered(
        &mut self,
        tab_id: String,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = if hovered {
            if self.hovered_tab_id.as_deref() == Some(tab_id.as_str()) {
                false
            } else {
                self.hovered_tab_id = Some(tab_id);
                true
            }
        } else if self.hovered_tab_id.as_deref() == Some(tab_id.as_str()) {
            self.hovered_tab_id = None;
            true
        } else {
            false
        };
        self.sync_tab_preview(cx);
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn set_tab_preview_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.tab_preview_hovered == hovered {
            return;
        }
        self.tab_preview_hovered = hovered;
        self.sync_tab_preview(cx);
        cx.notify();
    }

    pub(super) fn tab_preview_target(&self) -> Option<String> {
        if matches!(self.surface_drag, SurfaceDrag::Reorder(_)) {
            return None;
        }
        if let Some(id) = self.hovered_tab_id.clone() {
            return Some(id);
        }
        if self.tab_preview_hovered {
            self.tab_preview_id.clone()
        } else {
            None
        }
    }

    pub(crate) fn dismiss_tab_preview(&mut self) {
        self.tab_preview_task = None;
        self.tab_preview_id = None;
        self.tab_preview_goal = None;
        self.tab_preview_hovered = false;
    }

    pub(super) fn sync_tab_preview(&mut self, cx: &mut Context<Self>) {
        let target = self.tab_preview_target();
        if target.as_deref() == self.tab_preview_id.as_deref() {
            self.tab_preview_task = None;
            self.tab_preview_goal = target;
            return;
        }
        if self.tab_preview_goal.as_deref() == target.as_deref() && self.tab_preview_task.is_some()
        {
            return;
        }
        if target.is_some() && self.tab_preview_id.is_some() {
            self.tab_preview_id = None;
            self.tab_preview_hovered = false;
        }
        let delay = if target.is_some() {
            TAB_PREVIEW_DELAY
        } else {
            TAB_PREVIEW_HIDE_DELAY
        };
        self.tab_preview_goal = target.clone();
        self.tab_preview_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                this.tab_preview_task = None;
                let current = this.tab_preview_target();
                if current.as_deref() != this.tab_preview_goal.as_deref() {
                    return;
                }
                this.tab_preview_id = current;
                if this.tab_preview_id.is_none() {
                    this.tab_preview_hovered = false;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn select_pane(
        &mut self,
        pane_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_context = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .pane(&pane_id)
                .map(|pane| (pane.workspace_id.clone(), pane.tab_id.clone()))
        });
        let changed = self.selection.pane_id.as_deref() != Some(&pane_id)
            || pane_context.as_ref().is_some_and(|(workspace_id, tab_id)| {
                self.selection.workspace_id.as_deref() != Some(workspace_id)
                    || self.selection.tab_id.as_deref() != Some(tab_id)
            });
        let leave_split_tab = match (&self.surface_drag, pane_context.as_ref()) {
            (SurfaceDrag::Split(drag), context) => {
                let (workspace_id, tab_id) = match context {
                    Some((workspace_id, tab_id)) => {
                        (Some(workspace_id.as_str()), Some(tab_id.as_str()))
                    }
                    None => (None, None),
                };
                split_drag_voided_by_pane(drag, workspace_id, tab_id)
            }
            _ => false,
        };
        if leave_split_tab {
            self.cancel_split_drag();
        }
        if let Some((workspace_id, tab_id)) = pane_context {
            self.selection.workspace_id = Some(workspace_id);
            self.selection.tab_id = Some(tab_id);
        }
        self.selection.pane_id = Some(pane_id);
        if changed {
            self.ensure_session_terminals(cx);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }
}
