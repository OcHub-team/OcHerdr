use super::*;

pub(super) fn render_pane_template_palette(
    drag: &PaneDrag,
    pane_count: usize,
    surface: (f32, f32, f32, f32),
    i18n: I18n,
    cx: &mut Context<OcHerdrView>,
) -> Option<ochub_ui::gpui::AnyElement> {
    let geometry = pane_template_palette_geometry(surface, pane_count)?;
    let origin = (surface.0, surface.1);
    let hovered = drag.template_hover.as_ref().map(|hover| hover.placement);
    let cards = geometry
        .cards
        .into_iter()
        .map(|card| {
            let template_index = card.template as usize;
            let selected = hovered.is_some_and(|placement| placement.template == card.template);
            // Cards are children of the palette, so their offsets must be
            // palette-local. Subtracting the terminal origin here applies
            // the palette's own offset twice and pushes every card right.
            let card_local = pane_template_local_rect(card.rect, geometry.rect);
            let cells = card
                .slots
                .iter()
                .copied()
                .enumerate()
                .map(|(slot, rect)| {
                    let active = hovered.is_some_and(|placement| {
                        placement.template == card.template && placement.slot == slot
                    });
                    div()
                        .absolute()
                        .left(px(rect.0 - card.rect.0 + 1.))
                        .top(px(rect.1 - card.rect.1 + 1.))
                        .w(px((rect.2 - 2.).max(2.)))
                        .h(px((rect.3 - 2.).max(2.)))
                        .rounded(px(2.5))
                        .border_1()
                        .when(active, |cell| cell.border_2())
                        .border_color(if active {
                            theme::accent()
                        } else {
                            theme::border_strong()
                        })
                        .bg(if active {
                            theme::accent()
                        } else {
                            theme::surface()
                        })
                        .when(active, |cell| {
                            cell.flex().items_center().justify_center().child(icon(
                                IconName::Check,
                                theme::accent_text(),
                                9.,
                            ))
                        })
                })
                .collect::<Vec<_>>();
            // Transparent semantic regions cover the whole painted card.
            // Because their slot identity is captured directly, hit testing
            // cannot drift when the canvas publishes a newer window-space
            // origin between paint and pointer dispatch.
            let hit_regions = pane_template_slot_fractions(card.template)
                .into_iter()
                .enumerate()
                .filter_map(|(slot, fractions)| {
                    let slot_rect = *card.slots.get(slot)?;
                    let local = fractions_to_window((0., 0., card.rect.2, card.rect.3), fractions);
                    let hover = PaneTemplateHover {
                        placement: PaneTemplatePlacement {
                            template: card.template,
                            slot,
                        },
                        slot_rect,
                    };
                    Some(
                        div()
                            .absolute()
                            .left(px(local.0))
                            .top(px(local.1))
                            .w(px(local.2))
                            .h(px(local.3))
                            .on_mouse_move(cx.listener(
                                move |this, event: &MouseMoveEvent, _window, cx| {
                                    let pointer =
                                        (f32::from(event.position.x), f32::from(event.position.y));
                                    if this.update_pane_drag_over_template(
                                        pointer,
                                        hover.clone(),
                                        cx,
                                    ) {
                                        cx.stop_propagation();
                                    }
                                },
                            )),
                    )
                })
                .collect::<Vec<_>>();
            div()
                .id(("pane-layout-template", card.template as usize))
                .debug_selector(move || format!("pane-layout-template-{template_index}"))
                .role(ochub_ui::gpui::Role::Button)
                .aria_label(card.template.label(i18n))
                .absolute()
                .left(px(card_local.0))
                .top(px(card_local.1))
                .w(px(card.rect.2))
                .h(px(card.rect.3))
                .rounded(px(CORNER_COMPACT + 1.))
                .border_1()
                .when(selected, |card| card.border_2())
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::border()
                })
                .bg(if selected {
                    theme::accent_soft()
                } else {
                    theme::surface_hover()
                })
                .children(cells)
                .children(hit_regions)
        })
        .collect::<Vec<_>>();
    Some(
        div()
            .id("pane-layout-picker")
            .role(ochub_ui::gpui::Role::Toolbar)
            .aria_label(i18n.text(k::TERMINAL_LAYOUT_PICKER))
            .absolute()
            .left(px(geometry.rect.0 - origin.0))
            .top(px(geometry.rect.1 - origin.1))
            .w(px(geometry.rect.2))
            .h(px(geometry.rect.3))
            .rounded(px(CORNER_PANEL))
            .border_1()
            .border_color(theme::border_strong())
            .bg(theme::sidebar_background())
            .shadow_lg()
            .occlude()
            .children(cards)
            .into_any_element(),
    )
}

/// Paint the full terminal destination as well as the tiny palette cell.
/// The palette answers “which slot?”, while this outline answers “where will
/// my pane land?” without requiring the user to infer the scaled geometry.
pub(super) fn render_pane_template_target_highlight(
    drag: &PaneDrag,
    surface: (f32, f32, f32, f32),
    origin: (f32, f32),
    i18n: I18n,
) -> Option<ochub_ui::gpui::AnyElement> {
    let placement = drag.template_hover.as_ref()?.placement;
    let target = drag
        .layout_preview
        .as_ref()?
        .target_fractions(&drag.pane_id)?;
    let (x, y, w, h) = surface_local(fractions_to_window(surface, target), origin);
    let tone = theme::accent();
    let highlight = div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(w))
        .h(px(h))
        .p(px(2.))
        .children([
            div()
                .size_full()
                .border_2()
                .border_color(tone)
                .bg(tone.alpha(0.2)),
            div()
                .absolute()
                .top(px(10.))
                .left(px(10.))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(px(CORNER_CONTROL))
                .bg(theme::accent_fill())
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::accent_text())
                .child(icon(IconName::Check, theme::accent_text(), 11.))
                .child(placement.template.label(i18n)),
        ]);
    Some(highlight.into_any_element())
}
