use super::*;

pub(super) fn render_pane_template_palette(
    drag: &PaneDrag,
    pane_count: usize,
    surface: (f32, f32, f32, f32),
    i18n: I18n,
) -> Option<ochub_ui::gpui::AnyElement> {
    let geometry = pane_template_palette_geometry(surface, pane_count)?;
    let origin = (surface.0, surface.1);
    let hovered = drag.template_hover.as_ref().map(|hover| hover.placement);
    let cards = geometry
        .cards
        .into_iter()
        .map(|card| {
            let selected = hovered.is_some_and(|placement| placement.template == card.template);
            let card_origin = (card.rect.0 - origin.0, card.rect.1 - origin.1);
            let cells = card
                .slots
                .into_iter()
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
                        .border_color(if active {
                            theme::accent()
                        } else {
                            theme::border_strong()
                        })
                        .bg(if active {
                            theme::accent_fill()
                        } else {
                            theme::surface()
                        })
                })
                .collect::<Vec<_>>();
            div()
                .id(("pane-layout-template", card.template as usize))
                .role(ochub_ui::gpui::Role::Button)
                .aria_label(card.template.label(i18n))
                .absolute()
                .left(px(card_origin.0))
                .top(px(card_origin.1))
                .w(px(card.rect.2))
                .h(px(card.rect.3))
                .rounded(px(CORNER_COMPACT + 1.))
                .border_1()
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
