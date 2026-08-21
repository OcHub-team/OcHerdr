use super::super::*;

impl OcHerdrView {
    pub(super) fn render_appearance(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let mut family_rows = Vec::new();
        for record in &theme::load_registry().themes {
            let family = record.family.clone();
            let family_id = family.id.clone();
            let selected = family_id == self.appearance.theme_family;
            let palette = if theme::is_dark() {
                family.dark
            } else {
                family.light
            };
            let description = if family.description.is_empty() {
                i18n.text("Light and dark variants").to_owned()
            } else {
                family.description.clone()
            };
            family_rows.push(
                div()
                    .id(ochub_ui::gpui::ElementId::Name(
                        format!("appearance-family-{family_id}").into(),
                    ))
                    .role(ochub_ui::gpui::Role::Button)
                    .aria_label(i18n.use_theme_label(&family.name))
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_h(px(52.))
                    .px_3()
                    .py_2()
                    .rounded(px(CORNER_CONTROL))
                    .border_1()
                    .border_color(if selected {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(if selected {
                        theme::selection()
                    } else {
                        theme::surface()
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme_family(family_id.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .p_1()
                            .rounded(px(CORNER_CONTROL))
                            .bg(palette.bg.rgba())
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(CORNER_COMPACT))
                                    .bg(palette.accent_fill.rgba()),
                            )
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(CORNER_COMPACT))
                                    .bg(palette.surface.rgba()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .gap(px(2.))
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(family.name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child(description),
                            ),
                    )
                    .when(selected, |row| row.child(status_dot(theme::accent())))
                    .into_any_element(),
            );
        }
        let language_listener = cx.listener(|this, index: &usize, _window, cx| {
            this.set_language(Language::from_index(*index), cx);
        });
        let language_options = [
            i18n.text("System"),
            i18n.text("English"),
            i18n.text("Simplified Chinese"),
        ];
        let language = segmented(
            "language-control",
            &language_options,
            self.i18n.preference().index(),
            move |index, window, cx| language_listener(&index, window, cx),
        );
        let mode_listener = cx.listener(|this, index: &usize, window, cx| {
            let mode = match *index {
                1 => AppearanceMode::Light,
                2 => AppearanceMode::Dark,
                _ => AppearanceMode::System,
            };
            this.set_appearance_mode(mode, window, cx);
        });
        let mode = segmented(
            "appearance-mode-control",
            &[i18n.text("System"), i18n.text("Light"), i18n.text("Dark")],
            self.appearance.mode.index(),
            move |index, window, cx| mode_listener(&index, window, cx),
        );
        let backdrop_listener = cx.listener(|this, index: &usize, window, cx| {
            let backdrop = match *index {
                1 => BackdropMode::Transparent,
                2 => BackdropMode::Blurred,
                _ => BackdropMode::Opaque,
            };
            this.set_backdrop_mode(backdrop, window, cx);
        });
        let backdrop = segmented(
            "appearance-backdrop-control",
            &[i18n.text("Opaque"), i18n.text("Clear"), i18n.text("Blur")],
            self.appearance.backdrop.index(),
            move |index, window, cx| backdrop_listener(&index, window, cx),
        );
        let opacity_values = [100_u8, 92, 84, 72];
        let opacity_index = opacity_values
            .iter()
            .position(|value| *value == self.appearance.background_opacity)
            .unwrap_or(1);
        let opacity_listener = cx.listener(|this, index: &usize, window, cx| {
            let opacity = [100_u8, 92, 84, 72].get(*index).copied().unwrap_or(92);
            this.set_background_opacity(opacity, window, cx);
        });
        let opacity = segmented(
            "appearance-opacity-control",
            &["100%", "92%", "84%", "72%"],
            opacity_index,
            move |index, window, cx| opacity_listener(&index, window, cx),
        );
        let done = button(
            "close-appearance-footer",
            i18n.text("Done"),
            ButtonTone::Primary,
            ButtonSize::Md,
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_appearance(window, cx)))
        .into_any_element();
        let card = modal_card()
            .w(px(720.))
            .h(px(600.))
            .rounded(px(CORNER_MODAL))
            .child(
                modal_header(i18n.text("Appearance")).child(
                    icon_only_button_tone(
                        "close-appearance",
                        i18n.text("Close"),
                        IconName::Close,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.close_appearance(window, cx))),
                ),
            )
            .child(
                div()
                    .id("appearance-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .gap_5()
                    .px_5()
                    .py_5()
                    .child(appearance_setting_row(
                        i18n.text("Language"),
                        i18n.text("Choose the language used by OcHerdr."),
                        language,
                    ))
                    .child(appearance_section(
                        i18n.text("Theme"),
                        i18n.text(
                            "Choose a color family. Each family includes light and dark variants.",
                        ),
                        div().grid().grid_cols(2).gap_2().children(family_rows),
                    ))
                    .child(appearance_setting_row(
                        i18n.text("Appearance"),
                        i18n.text("Follow macOS or pin a variant."),
                        mode,
                    ))
                    .child(appearance_setting_row(
                        i18n.text("Window background"),
                        i18n.text(
                            "Clear keeps true transparency; Blur uses the native macOS backdrop.",
                        ),
                        backdrop,
                    ))
                    .child(appearance_setting_row(
                        i18n.text("Background opacity"),
                        i18n.text(
                            "Applied to terminal and shell surfaces when transparency is enabled.",
                        ),
                        opacity,
                    )),
            )
            .child(modal_footer(vec![done]));
        modal_overlay(card).top_0().left_0()
    }
}

fn appearance_section(
    title: &'static str,
    hint: &'static str,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text())
                        .child(title),
                )
                .child(div().text_xs().text_color(theme::muted()).child(hint)),
        )
        .child(content)
}

fn appearance_setting_row(
    label: &'static str,
    hint: &'static str,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_5()
        .min_h(px(66.))
        .px_4()
        .py_3()
        .rounded(px(CORNER_PANEL))
        .border_1()
        .border_color(theme::border())
        .bg(theme::surface())
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(label),
                )
                .child(div().text_xs().text_color(theme::muted()).child(hint)),
        )
        .child(div().w(px(300.)).flex_none().child(control))
}
