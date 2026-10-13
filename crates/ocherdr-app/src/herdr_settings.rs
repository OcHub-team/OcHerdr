//! Herdr's settings UI embed rode the numbered per-pane protocol that endpoint
//! generation servers (Herdr 0.9+) no longer accept. The overlay entry stays,
//! rendering the explanation instead of a doomed connect.
use super::*;

pub(crate) struct HerdrSettings {
    title: String,
    focus: FocusHandle,
    parent: WeakEntity<OcHerdrView>,
    i18n: I18n,
}

impl HerdrSettings {
    pub(crate) fn new(
        title: String,
        focus: FocusHandle,
        parent: WeakEntity<OcHerdrView>,
        i18n: I18n,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            title,
            focus,
            parent,
            i18n,
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if (key.modifiers.platform && key.key == "w")
            || (key.key == "escape" && !key.modifiers.modified())
        {
            self.parent
                .update(cx, |parent, _| {
                    parent.consumed_key_releases.record(key);
                })
                .ok();
            self.close(window, cx);
        }
        cx.stop_propagation();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.parent
            .update(cx, |parent, cx| parent.close_herdr_settings(window, cx))
            .ok();
    }
}

impl Render for HerdrSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
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
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .child(self.i18n.text(k::HERDR_SETTINGS_UNSUPPORTED)),
            )
    }
}
