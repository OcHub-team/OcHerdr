use super::*;

mod agent;
mod appearance;
mod hierarchy;
mod overlays;
mod remote;

pub(crate) use appearance::AppearanceUi;

impl Render for OcHerdrView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.chrome_a11y();
        let main = crate::a11y::apply_region(div().id(chrome.main.id), &chrome.main)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme::surface().alpha(0.))
            .child(self.render_tab_bar(&chrome, window, cx))
            .child(self.render_terminal(window, cx));
        let workspace_body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.render_sidebar(&chrome, cx))
            .child(main)
            .into_any_element();
        let body = if self.overlay.host_center() {
            self.host_center.clone().into_any_element()
        } else {
            workspace_body
        };
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(theme::window_base_background())
            .on_key_down(cx.listener(|this, event, window, cx| {
                if this.handle_overlay_key(event, window, cx) {
                    return;
                }
                if !key_goes_to_terminal(&this.overlay) {
                    return;
                }
                // #terminal-surface also calls send_key. Duplicate dispatch is
                // GPUI bubbling: send_key stops propagation after handling.
                this.send_key(event, window, cx);
            }))
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                this.pane_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pane_mouse_up(event, window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pane_mouse_up(event, window, cx)),
            )
            .child(body);
        if !self.overlay.host_center() {
            root = root.child(self.render_status_bar(&chrome, cx));
        }
        match self.overlay.clone() {
            Overlay::None | Overlay::NodeManager | Overlay::RemoteForm(_) => {}
            Overlay::HostSwitcher => {
                root = root.child(self.render_host_switcher(cx));
            }
            Overlay::Appearance => {
                root = root.child(self.render_appearance(cx));
            }
            Overlay::ContextMenu(menu) => {
                root = root.child(self.render_context_menu(menu, cx));
            }
            Overlay::ConfirmSwitchProfile { id, .. } => {
                root = root.child(self.render_switch_host(&id, cx));
            }
            Overlay::ConfirmBulkRemove => {
                root = root.child(self.render_bulk_remove(cx));
            }
            Overlay::ConfirmRemoveProfile(id) => {
                root = root.child(self.render_remove_node(&id, cx));
            }
            Overlay::ConfirmClose(target) => {
                root = root.child(self.render_close_target(&target, cx));
            }
            Overlay::ConfirmRemoveWorktree { label, prompt, .. } => {
                root = root.child(self.render_remove_worktree(&label, &prompt, cx));
            }
            Overlay::WorktreeCreate { advanced, .. } => {
                root = root.child(self.render_worktree_create(advanced, cx));
            }
            Overlay::WorktreeOpen(state) => {
                root = root.child(self.render_worktree_open(&state, cx));
            }
            Overlay::Rename(target) => {
                root = root.child(self.render_rename(&target, cx));
            }
            Overlay::AgentPanel { pane_id } => {
                root = root.child(self.render_agent_panel(&pane_id, cx));
            }
        }
        if let SurfaceDrag::Reorder(drag) = self.surface_drag.clone()
            && reorder_past_slop(&drag)
        {
            root = root.child(self.render_reorder_overlay(&drag, cx));
        }
        root.child(self.notifications.clone())
    }
}
