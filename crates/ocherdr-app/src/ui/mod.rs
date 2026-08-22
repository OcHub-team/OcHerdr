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
        let body = if self.node_manager_open {
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
                if this.rename_target.is_some()
                    || this.remote_form != RemoteForm::Closed
                    || this.appearance_open
                    || this.herdr_settings_open
                    || this.node_manager_open
                    || this.host_switcher_open
                    || this.pending_close.is_some()
                    || this.pending_remove_profile.is_some()
                    || this.pending_bulk_remove
                    || this.pending_switch_profile.is_some()
                    || this.context_menu.is_some()
                {
                    return;
                }
                this.send_key(event, window, cx);
            }))
            .child(body);
        if !self.node_manager_open {
            root = root.child(self.render_status_bar(cx));
        }
        if self.host_switcher_open {
            root = root.child(self.render_host_switcher(cx));
        }
        if self.appearance_open {
            root = root.child(self.render_appearance(cx));
        }
        if self.herdr_settings_open {
            root = root.child(self.render_herdr_settings(cx));
        }
        if self.context_menu.is_some() {
            root = root.child(self.render_context_menu(cx));
        }
        if self.pending_switch_profile.is_some() {
            root = root.child(self.render_switch_host(cx));
        } else if self.pending_bulk_remove {
            root = root.child(self.render_bulk_remove(cx));
        } else if self.pending_remove_profile.is_some() {
            root = root.child(self.render_remove_node(cx));
        } else if self.pending_close.is_some() {
            root = root.child(self.render_close_target(cx));
        } else if self.rename_target.is_some() {
            root = root.child(self.render_rename(cx));
        }
        root
    }
}
