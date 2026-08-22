use super::*;

mod appearance;
mod hierarchy;
mod overlays;
mod remote;

impl Render for OcHerdrView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.chrome_a11y();
        let mut main = crate::a11y::apply_region(div().id(chrome.main.id), &chrome.main)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme::surface().alpha(0.))
            .child(self.render_tab_bar(cx))
            .child(self.render_terminal(window, cx));
        if let Some(error) = &self.error {
            main = main.child(
                div()
                    .id("error-toast")
                    .role(ochub_ui::gpui::Role::Alert)
                    .aria_label(error.clone())
                    .absolute()
                    .right_4()
                    .bottom_4()
                    .max_w(px(480.))
                    .px_3()
                    .py_2()
                    .rounded(px(CORNER_CONTROL))
                    .border_1()
                    .border_color(theme::red())
                    .bg(theme::error_surface())
                    .text_xs()
                    .text_color(theme::red())
                    .child(error.clone()),
            );
        }
        let workspace_body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.render_sidebar(cx))
            .child(main)
            .into_any_element();
        let body = if self.overlay.host_center() {
            self.render_node_manager(cx).into_any_element()
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
                this.send_key(event, window, cx);
            }))
            .child(body);
        if !self.overlay.host_center() {
            root = root.child(self.render_status_bar(cx));
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
            Overlay::ConfirmSwitchProfile { index, .. } => {
                root = root.child(self.render_switch_host(index, cx));
            }
            Overlay::ConfirmBulkRemove => {
                root = root.child(self.render_bulk_remove(cx));
            }
            Overlay::ConfirmRemoveProfile(index) => {
                root = root.child(self.render_remove_node(index, cx));
            }
            Overlay::ConfirmClose(target) => {
                root = root.child(self.render_close_target(&target, cx));
            }
            Overlay::Rename(target) => {
                root = root.child(self.render_rename(&target, cx));
            }
        }
        root
    }
}
