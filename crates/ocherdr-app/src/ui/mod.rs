use super::*;

mod appearance;
mod hierarchy;
mod overlays;
mod remote;

impl Render for OcHerdrView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut main = div()
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
        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.render_sidebar(cx))
            .child(main);
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(theme::window_base_background())
            .on_key_down(cx.listener(|this, event, window, cx| {
                if this.rename_target.is_none() && this.handle_app_shortcut(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(body)
            .child(self.render_status_bar());
        if self.node_manager_open {
            root = root.child(self.render_node_manager(cx));
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
        if self.pending_remove_profile.is_some() {
            root = root.child(self.render_remove_node(cx));
        } else if self.pending_close.is_some() {
            root = root.child(self.render_close_target(cx));
        } else if self.rename_target.is_some() {
            root = root.child(self.render_rename(cx));
        }
        root
    }
}
