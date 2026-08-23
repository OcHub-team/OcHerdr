use super::super::*;
use crate::a11y::apply_dialog;
use ochub_ui::layout::{
    SelectRowEvent, SelectRowState, group, section_header, select_row, switch_row,
};
use ochub_ui::scrollbar::{VerticalScrollbar, contain_vertical_scroll};

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
                i18n.text(k::APPEARANCE_THEME_FALLBACK_DESCRIPTION)
                    .to_owned()
            } else {
                family.description.clone()
            };
            family_rows.push(
                div()
                    .id(ochub_ui::gpui::ElementId::Name(
                        format!("appearance-family-{family_id}").into(),
                    ))
                    .role(ochub_ui::gpui::Role::Button)
                    .tab_stop(false)
                    .aria_label(i18n.use_theme_label(&family.name))
                    .aria_selected(selected)
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

        let language = select_row(
            "language",
            i18n.text(k::COMMON_LANGUAGE_LABEL),
            Some(i18n.text(k::COMMON_LANGUAGE_DESCRIPTION).into()),
            &[
                i18n.text(k::COMMON_SYSTEM),
                i18n.text(k::COMMON_LANGUAGE_ENGLISH),
                i18n.text(k::COMMON_LANGUAGE_SIMPLIFIED_CHINESE),
            ],
            self.i18n.preference().index(),
            self.select_row_state("language"),
            appearance_select(cx, "language", |this, index, _window, cx| {
                this.set_language(Language::from_index(index), cx);
            }),
        );
        let mode = select_row(
            "appearance-mode",
            i18n.text(k::APPEARANCE_TITLE),
            Some(i18n.text(k::APPEARANCE_MODE_DESCRIPTION).into()),
            &[
                i18n.text(k::COMMON_SYSTEM),
                i18n.text(k::APPEARANCE_MODE_LIGHT),
                i18n.text(k::APPEARANCE_MODE_DARK),
            ],
            self.appearance.mode.index(),
            self.select_row_state("appearance-mode"),
            appearance_select(cx, "appearance-mode", |this, index, window, cx| {
                let mode = match index {
                    1 => AppearanceMode::Light,
                    2 => AppearanceMode::Dark,
                    _ => AppearanceMode::System,
                };
                this.set_appearance_mode(mode, window, cx);
            }),
        );
        let backdrop = select_row(
            "appearance-backdrop",
            i18n.text(k::APPEARANCE_BACKDROP_LABEL),
            Some(i18n.text(k::APPEARANCE_BACKDROP_DESCRIPTION).into()),
            &[
                i18n.text(k::APPEARANCE_BACKDROP_OPAQUE),
                i18n.text(k::APPEARANCE_BACKDROP_CLEAR),
                i18n.text(k::APPEARANCE_BACKDROP_BLUR),
            ],
            self.appearance.backdrop.index(),
            self.select_row_state("appearance-backdrop"),
            appearance_select(cx, "appearance-backdrop", |this, index, window, cx| {
                let backdrop = match index {
                    1 => BackdropMode::Transparent,
                    2 => BackdropMode::Blurred,
                    _ => BackdropMode::Opaque,
                };
                this.set_backdrop_mode(backdrop, window, cx);
            }),
        );
        let opacity_values = [100_u8, 92, 84, 72];
        let opacity_index = opacity_values
            .iter()
            .position(|value| *value == self.appearance.background_opacity)
            .unwrap_or(1);
        let opacity = select_row(
            "appearance-opacity",
            i18n.text(k::APPEARANCE_OPACITY_LABEL),
            Some(i18n.text(k::APPEARANCE_OPACITY_DESCRIPTION).into()),
            &["100%", "92%", "84%", "72%"],
            opacity_index,
            self.select_row_state("appearance-opacity"),
            appearance_select(cx, "appearance-opacity", |this, index, window, cx| {
                let opacity = [100_u8, 92, 84, 72].get(index).copied().unwrap_or(92);
                this.set_background_opacity(opacity, window, cx);
            }),
        );
        let size_labels = FONT_SIZES.map(|size| size.to_string());
        let size_refs = size_labels.each_ref().map(String::as_str);
        let size_index = FONT_SIZES
            .iter()
            .position(|size| *size == self.appearance.font.size)
            .unwrap_or(2);
        let font_size = select_row(
            "appearance-font-size",
            i18n.text(k::APPEARANCE_FONT_SIZE_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_SIZE_DESCRIPTION).into()),
            &size_refs,
            size_index,
            self.select_row_state("appearance-font-size"),
            appearance_select(cx, "appearance-font-size", |this, index, window, cx| {
                let size = FONT_SIZES.get(index).copied().unwrap_or(13);
                this.set_font_size(size, window, cx);
            }),
        );
        let ligatures_listener = cx.listener(|this, _: &(), window, cx| {
            this.set_font_ligatures(!this.appearance.font.ligatures, window, cx);
        });
        let ligatures = switch_row(
            "appearance-ligatures",
            i18n.text(k::APPEARANCE_FONT_LIGATURES_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_LIGATURES_DESCRIPTION).into()),
            self.appearance.font.ligatures,
            false,
            move |window, cx| ligatures_listener(&(), window, cx),
        );
        let thicken_listener = cx.listener(|this, _: &(), window, cx| {
            this.set_font_thicken(!this.appearance.font.thicken, window, cx);
        });
        let thicken = switch_row(
            "appearance-thicken",
            i18n.text(k::APPEARANCE_FONT_THICKEN_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_THICKEN_DESCRIPTION).into()),
            self.appearance.font.thicken,
            false,
            move |window, cx| thicken_listener(&(), window, cx),
        );
        let width_index = CELL_WIDTHS
            .iter()
            .position(|value| *value == self.appearance.font.cell_width_percent)
            .unwrap_or(1);
        let cell_width = select_row(
            "appearance-cell-width",
            i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_DESCRIPTION).into()),
            &[
                i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_TIGHT),
                i18n.text(k::COMMON_DEFAULT),
                i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_WIDE),
            ],
            width_index,
            self.select_row_state("appearance-cell-width"),
            appearance_select(cx, "appearance-cell-width", |this, index, window, cx| {
                let percent = CELL_WIDTHS.get(index).copied().unwrap_or(0);
                this.set_cell_width(percent, window, cx);
            }),
        );
        let height_index = CELL_HEIGHTS
            .iter()
            .position(|value| *value == self.appearance.font.cell_height_percent)
            .unwrap_or(1);
        let cell_height = select_row(
            "appearance-cell-height",
            i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_DESCRIPTION).into()),
            &[
                i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_COMPACT),
                i18n.text(k::COMMON_DEFAULT),
                i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_RELAXED),
                i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_LOOSE),
            ],
            height_index,
            self.select_row_state("appearance-cell-height"),
            appearance_select(cx, "appearance-cell-height", |this, index, window, cx| {
                let percent = CELL_HEIGHTS.get(index).copied().unwrap_or(0);
                this.set_cell_height(percent, window, cx);
            }),
        );

        let done = button(
            "close-appearance-footer",
            i18n.text(k::COMMON_DONE),
            ButtonTone::Primary,
            ButtonSize::Md,
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_appearance(window, cx)))
        .into_any_element();
        let appearance_scroll = self.appearance_scroll.clone();
        let card = apply_dialog(
            modal_card(),
            "appearance-dialog",
            i18n.text(k::APPEARANCE_TITLE),
        )
        .w(px(720.))
        .h(px(640.))
        .rounded(px(CORNER_MODAL))
        .child(
            modal_header(i18n.text(k::APPEARANCE_TITLE)).child(
                icon_only_button_tone(
                    "close-appearance",
                    i18n.text(k::COMMON_CLOSE),
                    IconName::Close,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, window, cx| this.close_appearance(window, cx))),
            ),
        )
        .child(
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .child(
                    div()
                        .id("appearance-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_y_scroll()
                        .track_scroll(&appearance_scroll)
                        .on_scroll_wheel(contain_vertical_scroll(appearance_scroll.clone()))
                        .gap_5()
                        .px_5()
                        .py_5()
                        .child(group(vec![language.into_any_element()]))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(section_header(
                                    i18n.text(k::APPEARANCE_THEME_LABEL),
                                    Some(i18n.text(k::APPEARANCE_THEME_DESCRIPTION).into()),
                                ))
                                .child(
                                    div()
                                        .id("appearance-theme-list")
                                        .role(ochub_ui::gpui::Role::List)
                                        .aria_label(i18n.text(k::APPEARANCE_THEME_LABEL))
                                        .grid()
                                        .grid_cols(2)
                                        .gap_2()
                                        .children(family_rows),
                                ),
                        )
                        .child(group(vec![
                            mode.into_any_element(),
                            backdrop.into_any_element(),
                            opacity.into_any_element(),
                        ]))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(section_header(
                                    i18n.text(k::APPEARANCE_FONT_LABEL),
                                    Some(i18n.text(k::APPEARANCE_FONT_DESCRIPTION).into()),
                                ))
                                .child(self.render_font_family_list(cx)),
                        )
                        .child(group(vec![
                            font_size.into_any_element(),
                            ligatures.into_any_element(),
                            thicken.into_any_element(),
                            cell_width.into_any_element(),
                            cell_height.into_any_element(),
                        ])),
                )
                .child(VerticalScrollbar::new(
                    ochub_ui::gpui::ElementId::Name("appearance-scroll-scrollbar".into()),
                    appearance_scroll,
                )),
        )
        .child(modal_footer(vec![done]));
        modal_overlay(card).top_0().left_0()
    }

    fn select_row_state(&self, id: &str) -> SelectRowState {
        SelectRowState::new(false, self.open_select.as_deref() == Some(id))
    }

    fn render_font_family_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let selected_family = self.appearance.font.family.clone();
        let mut families = vec![(
            String::new(),
            i18n.text(k::APPEARANCE_FONT_JETBRAINS).to_owned(),
        )];
        for family in crate::fonts::monospace_families() {
            if family != "JetBrains Mono" {
                families.push((family.clone(), family.clone()));
            }
        }
        if !selected_family.is_empty()
            && !families
                .iter()
                .any(|(family, _)| family == &selected_family)
        {
            families.insert(1, (selected_family.clone(), selected_family.clone()));
        }
        let rows = families
            .into_iter()
            .map(|(family, label)| {
                let selected = family == selected_family;
                let preview_family = family.clone();
                div()
                    .id(ochub_ui::gpui::ElementId::Name(
                        format!(
                            "appearance-font-{}",
                            if family.is_empty() {
                                "ghostty"
                            } else {
                                &family
                            }
                        )
                        .into(),
                    ))
                    .role(ochub_ui::gpui::Role::Button)
                    .tab_stop(false)
                    .aria_label(label.clone())
                    .aria_selected(selected)
                    .flex()
                    .items_center()
                    .justify_between()
                    .min_h(px(36.))
                    .px_3()
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
                        this.set_font_family(family.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .when(!preview_family.is_empty(), |label| {
                                label.font_family(preview_family.clone())
                            })
                            .child(label),
                    )
                    .when(selected, |row| row.child(status_dot(theme::accent())))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .id("appearance-font-list")
            .role(ochub_ui::gpui::Role::List)
            .aria_label(i18n.text(k::APPEARANCE_FONT_LABEL))
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(220.))
            .overflow_scroll()
            .children(rows)
    }
}

fn appearance_select(
    cx: &mut Context<OcHerdrView>,
    id: &'static str,
    on_select: impl Fn(&mut OcHerdrView, usize, &mut Window, &mut Context<OcHerdrView>) + 'static,
) -> impl Fn(SelectRowEvent, &mut Window, &mut App) + 'static {
    let listener = cx.listener(move |this, event: &SelectRowEvent, window, cx| {
        if let Some(index) = apply_select_event(&mut this.open_select, id, *event) {
            on_select(this, index, window, cx);
        } else {
            cx.notify();
        }
    });
    move |event, window, cx| listener(&event, window, cx)
}

fn apply_select_event(
    open_select: &mut Option<SharedString>,
    id: &str,
    event: SelectRowEvent,
) -> Option<usize> {
    match event {
        SelectRowEvent::Open(true) => {
            *open_select = Some(id.into());
            None
        }
        SelectRowEvent::Open(false) => {
            *open_select = None;
            None
        }
        SelectRowEvent::Select(index) => {
            *open_select = None;
            Some(index)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_select_closes_the_one_that_was_already_open() {
        let mut open_select = None;
        apply_select_event(&mut open_select, "a", SelectRowEvent::Open(true));
        assert_eq!(open_select.as_deref(), Some("a"));
        apply_select_event(&mut open_select, "b", SelectRowEvent::Open(true));
        assert_eq!(open_select.as_deref(), Some("b"));
    }
}
