use super::*;

pub(super) fn pane_fractions(
    layout: &ocherdr_core::PaneLayout,
    pane_id: &str,
) -> Option<(f32, f32, f32, f32)> {
    if layout.zoomed {
        return (layout.focused_pane_id == pane_id).then_some((0., 0., 1., 1.));
    }
    squeezed_layout(layout, &[])
        .and_then(|resolved| resolved.pane(pane_id))
        .or_else(|| {
            let pane = layout.panes.iter().find(|pane| pane.pane_id == pane_id)?;
            layout_rect_fractions(layout.area, pane.rect)
        })
}

/// Divider of one split (design §5.4): a 10 px hit strip. Its line stays
/// canvas-coloured at rest, then turns accent on hover and while dragged.
/// `geometry` is the split rect and divider line as surface fractions,
/// already squeezed to the preview ratio during a drag.
pub(super) fn render_split_handle(
    split: &LayoutSplit,
    geometry: ((f32, f32, f32, f32), f32),
    dragging: bool,
    tab_id: String,
    i18n: I18n,
    cx: &mut Context<OcHerdrView>,
) -> Option<ochub_ui::gpui::AnyElement> {
    let ((x, y, w, h), line) = geometry;
    let split = split.clone();
    let label = i18n.text(k::TERMINAL_RESIZE_SPLIT);
    let group = SharedString::from(format!("split-handle-{}", split.id));
    let line_color = if dragging {
        theme::accent()
    } else {
        // Rounded panes reveal this strip. Keeping its resting colour equal
        // to the terminal canvas avoids a permanent grey gutter.
        theme::current().bg.rgba()
    };
    // Mouse-only. A tab-reachable splitter would fight terminal key
    // forwarding. Keyboard resize is the Herdr TUI and `herdr pane resize`.
    let handle = match split.direction {
        SplitDirection::Right => div()
            .id(ochub_ui::gpui::ElementId::Name(
                format!("split-handle-{}", split.id).into(),
            ))
            .group(group.clone())
            .absolute()
            .left(relative(line))
            .top(relative(y))
            .h(relative(h))
            .w(px(SPLIT_HANDLE_HIT_PX))
            .ml(px(-SPLIT_HANDLE_HIT_PX / 2.))
            .flex()
            .justify_center()
            // Empty hit strip; without this GPUI skips the 10px target.
            .occlude()
            .cursor_col_resize()
            .tab_stop(false)
            .aria_label(label)
            .child(
                div()
                    .w(px(SPLIT_HANDLE_VISUAL_PX))
                    .h_full()
                    .bg(line_color)
                    .group_hover(group, |style| style.bg(theme::accent())),
            ),
        SplitDirection::Down => div()
            .id(ochub_ui::gpui::ElementId::Name(
                format!("split-handle-{}", split.id).into(),
            ))
            .group(group.clone())
            .absolute()
            .left(relative(x))
            .top(relative(line))
            .w(relative(w))
            .h(px(SPLIT_HANDLE_HIT_PX))
            .mt(px(-SPLIT_HANDLE_HIT_PX / 2.))
            .flex()
            .items_center()
            .occlude()
            .cursor_row_resize()
            .tab_stop(false)
            .aria_label(label)
            .child(
                div()
                    .h(px(SPLIT_HANDLE_VISUAL_PX))
                    .w_full()
                    .bg(line_color)
                    .group_hover(group, |style| style.bg(theme::accent())),
            ),
    };
    Some(
        handle
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, _window, cx| {
                    this.begin_split_drag(tab_id.clone(), split.clone(), event, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, event, window, cx| {
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
            .into_any_element(),
    )
}

/// Captures the pointer while a divider is dragged. The divider itself is
/// drawn by its handle, which already sits at the preview ratio.
pub(super) fn render_split_drag_overlay(
    drag: &SplitDrag,
    cx: &mut Context<OcHerdrView>,
) -> ochub_ui::gpui::AnyElement {
    let overlay = match drag.direction {
        SplitDirection::Right => div().cursor_col_resize(),
        SplitDirection::Down => div().cursor_row_resize(),
    };
    overlay
        .id("split-drag-overlay")
        .absolute()
        .size_full()
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
        .into_any_element()
}

/// How a pane shell is drawn this frame, beyond its snapshot data.
pub(super) struct PanePresentation {
    pub(super) fractions: (f32, f32, f32, f32),
    /// The pane is the lifted source of a drag: dimmed, dashed border.
    pub(super) source_slot: bool,
    /// Logical size of the frozen terminal grid; the frame is drawn at that
    /// size and clipped instead of being refitted to the shell.
    pub(super) frozen: Option<(f32, f32)>,
    pub(super) handle: Option<ochub_ui::gpui::AnyElement>,
}

/// 20×24 grab area at the left of the title bar (design §5.1). Shown on
/// hover or when the pane is selected; mouse-only, like the split handle,
/// so it never competes with terminal key forwarding.
pub(super) fn render_pane_drag_handle(
    pane: &PaneInfo,
    selected: bool,
    i18n: I18n,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    let group = SharedString::from(format!("terminal-pane-group-{}", pane.pane_id));
    div()
        .id(ochub_ui::gpui::ElementId::Name(
            format!("pane-drag-handle-{}", pane.pane_id).into(),
        ))
        .role(ochub_ui::gpui::Role::Button)
        .tab_stop(false)
        .aria_label(pane_drag_handle_name(pane, i18n))
        .flex_none()
        .w(px(PANE_DRAG_HANDLE_WIDTH))
        .h(px(PANE_DRAG_HANDLE_HEIGHT))
        .ml(px(-6.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(CORNER_COMPACT))
        .cursor_grab()
        .occlude()
        .opacity(if selected { 1. } else { 0. })
        .group_hover(group, |style| style.opacity(1.))
        .hover(|style| style.bg(theme::border()))
        .child(icon(IconName::DragHandle, theme::subtext(), 12.))
}

pub(super) struct PaneRenderInput {
    pub(super) pane: PaneInfo,
    pub(super) frame: Option<RenderedFrame>,
    pub(super) presentation: PanePresentation,
    pub(super) a11y: crate::a11y::PaneA11y,
}

pub(super) fn render_pane(
    input: PaneRenderInput,
    i18n: I18n,
    view: Entity<OcHerdrView>,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    let PaneRenderInput {
        pane,
        frame,
        presentation,
        a11y,
    } = input;
    let PanePresentation {
        fractions,
        source_slot,
        frozen,
        handle,
    } = presentation;
    let (x, y, w, h) = fractions;
    let pane_name = a11y.name.clone();
    let selected = a11y.selected;
    let waiting_for_frame = frame.is_none();
    let measure_pane_id = pane.pane_id.clone();
    let group = SharedString::from(format!("terminal-pane-group-{}", pane.pane_id));
    div()
        .id(ochub_ui::gpui::ElementId::Name(
            format!("terminal-pane-{}", pane.pane_id).into(),
        ))
        .group(group)
        .role(a11y.role)
        .aria_label(a11y.name.clone())
        .aria_selected(selected)
        .aria_value(a11y.value.clone())
        .absolute()
        .left(relative(x))
        .top(relative(y))
        .w(relative(w))
        .h(relative(h))
        .p(px(2.))
        .flex()
        .flex_col()
        .when(source_slot, |shell| shell.opacity(PANE_DRAG_SOURCE_OPACITY))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .rounded(px(CORNER_COMPACT))
                .border_1()
                .when(source_slot, |body| body.border_dashed())
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::border_strong()
                })
                .bg(theme::surface().alpha(0.))
                .cursor_text()
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(px(PANE_HEADER_HEIGHT))
                        .px_2()
                        .gap_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .bg(if selected {
                            theme::selection()
                        } else {
                            theme::panel()
                        })
                        .text_xs()
                        .text_color(theme::subtext())
                        .children(handle)
                        .child(status_dot(status_color(pane.agent_status)))
                        .child(div().truncate().flex_1().child(pane_name)),
                )
                .child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .overflow_hidden()
                        .bg(theme::current().bg.rgba())
                        .child(
                            canvas(
                                move |bounds, window, cx| {
                                    view.update(cx, |this, cx| {
                                        this.sync_measured_pane_body(
                                            &measure_pane_id,
                                            bounds,
                                            window,
                                            cx,
                                        );
                                    });
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                        .when_some(frame, |container, frame| {
                            container.child(terminal_frame_element(frame, frozen))
                        })
                        .when(waiting_for_frame, |container| {
                            container.child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child(i18n.text(k::TERMINAL_WAITING)),
                            )
                        }),
                ),
        )
}

/// Floating copy of a pane: title bar plus its last rendered frame. Reuses
/// the current `RenderedFrame`; no screenshot, no IOSurface copy.
pub(super) fn pane_preview_card(
    pane: &PaneInfo,
    frame: Option<RenderedFrame>,
    rect: (f32, f32, f32, f32),
    opacity: f32,
    i18n: I18n,
) -> ochub_ui::gpui::Div {
    let (x, y, w, h) = rect;
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(w))
        .h(px(h))
        .opacity(opacity)
        .p(px(2.))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .rounded(px(CORNER_COMPACT))
                .border_1()
                .border_color(theme::accent())
                .shadow_lg()
                .bg(theme::current().bg.rgba())
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(px(PANE_HEADER_HEIGHT))
                        .px_2()
                        .gap_2()
                        .border_b_1()
                        .border_color(theme::border())
                        .bg(theme::selection())
                        .text_xs()
                        .text_color(theme::subtext())
                        .child(status_dot(status_color(pane.agent_status)))
                        .child(
                            div()
                                .truncate()
                                .flex_1()
                                .child(pane.display_name().to_owned()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(theme::muted())
                                .child(i18n.agent_status(pane.agent_status)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .overflow_hidden()
                        .when_some(frame, |body, frame| {
                            body.child(terminal_frame_element(frame, None))
                        }),
                ),
        )
}

/// Window-space rect expressed relative to the terminal surface's origin.
pub(super) fn surface_local(
    rect: (f32, f32, f32, f32),
    origin: (f32, f32),
) -> (f32, f32, f32, f32) {
    (rect.0 - origin.0, rect.1 - origin.1, rect.2, rect.3)
}

/// Cancelled or invalid drop: the preview flies back to its slot over
/// `PANE_DRAG_RETURN_ANIMATION` (design §10).
pub(super) fn render_pane_drag_return(
    snapshot: &HierarchySnapshot,
    flight: &PaneDragReturn,
    origin: (f32, f32),
    frame: Option<RenderedFrame>,
    i18n: I18n,
) -> Option<ochub_ui::gpui::AnyElement> {
    let pane = snapshot.pane(&flight.pane_id)?.clone();
    let (from, to) = (
        surface_local(flight.from, origin),
        surface_local(flight.to, origin),
    );
    let card = pane_preview_card(&pane, frame, from, PANE_DRAG_PREVIEW_OPACITY, i18n);
    Some(
        card.with_animation(
            ElementId::Name(format!("pane-drag-return-{}", flight.pane_id).into()),
            Animation::new(PANE_DRAG_RETURN_ANIMATION).with_easing(ease_out_quint()),
            move |card, t| {
                let (x, y, w, h) = lerp_rect(from, to, t);
                card.left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .opacity(PANE_DRAG_PREVIEW_OPACITY + (1. - PANE_DRAG_PREVIEW_OPACITY) * t)
            },
        )
        .into_any_element(),
    )
}

/// Accent outline, translucent fill and zone label over the target pane
/// (design §5.2); fades in over `PANE_DROP_ZONE_ANIMATION`.
pub(super) fn drop_zone_tone(droppable: bool) -> ochub_ui::gpui::Rgba {
    if droppable {
        theme::accent()
    } else {
        theme::border_strong()
    }
}

pub(super) fn render_drop_highlight(
    hover: &PaneDropHover,
    droppable: bool,
    origin: (f32, f32),
    reduce_motion: bool,
) -> ochub_ui::gpui::AnyElement {
    let (x, y, w, h) = surface_local(hover.target_rect, origin);
    let tone = drop_zone_tone(droppable);
    let card = div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(w))
        .h(px(h))
        .p(px(2.))
        .child(
            div()
                .size_full()
                .border_2()
                .border_color(tone)
                .bg(tone.alpha(0.14)),
        );
    if reduce_motion {
        card.into_any_element()
    } else {
        card.with_animation(
            ElementId::Name(
                format!("pane-drop-zone-{}-{:?}", hover.target_pane_id, hover.zone).into(),
            ),
            Animation::new(PANE_DROP_ZONE_ANIMATION).with_easing(ease_out_quint()),
            |card, t| card.opacity(t),
        )
        .into_any_element()
    }
}

/// The zone's name, centred along the top edge of the target highlight, in
/// the target's title-bar band. Painted after the floating preview so the
/// card cannot cover it while the pointer rests inside the target.
pub(super) fn render_drop_zone_label(
    hover: &PaneDropHover,
    droppable: bool,
    origin: (f32, f32),
    reduce_motion: bool,
    i18n: I18n,
) -> ochub_ui::gpui::AnyElement {
    let (x, y, w, _) = surface_local(hover.target_rect, origin);
    let label = if droppable {
        drop_zone_label(hover.zone, i18n)
    } else {
        i18n.text(k::TERMINAL_DROP_INVALID)
    };
    let tone = drop_zone_tone(droppable);
    let band = div()
        .absolute()
        .left(px(x))
        .top(px(y + 2.))
        .w(px(w))
        .h(px(PANE_HEADER_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .px_3()
                .py_1()
                .rounded(px(CORNER_CONTROL))
                .bg(tone)
                .shadow_md()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::current().bg.rgba())
                .child(label),
        );
    if reduce_motion {
        band.into_any_element()
    } else {
        band.with_animation(
            ElementId::Name(
                format!(
                    "pane-drop-zone-label-{}-{:?}",
                    hover.target_pane_id, hover.zone
                )
                .into(),
            ),
            Animation::new(PANE_DROP_ZONE_ANIMATION).with_easing(ease_out_quint()),
            |band, t| band.opacity(t),
        )
        .into_any_element()
    }
}

/// Step 2 of an edge relocation failed: the pane sits in a temporary tab
/// (design §7.3). Inline in the original tab, with the two recovery actions.
pub(super) fn render_parked_notice(
    tab_id: &str,
    plan: &RelocationPlan,
    i18n: I18n,
    cx: &mut Context<OcHerdrView>,
) -> ochub_ui::gpui::AnyElement {
    let retry_tab = tab_id.to_owned();
    let go_tab = tab_id.to_owned();
    div()
        .id("pane-relocation-parked")
        .role(ochub_ui::gpui::Role::Status)
        .aria_label(i18n.text(k::TERMINAL_RELOCATION_PARKED))
        .absolute()
        .top(px(12.))
        .left(px(12.))
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .rounded(px(CORNER_CONTROL))
        .bg(theme::panel())
        .border_1()
        .border_color(theme::yellow())
        .shadow_md()
        .text_sm()
        .text_color(theme::text())
        .child(status_dot(theme::yellow()))
        .child(format!(
            "{} · {}",
            i18n.text(k::TERMINAL_RELOCATION_PARKED),
            plan.source_pane_id
        ))
        .child(
            button(
                "pane-relocation-retry",
                i18n.text(k::TERMINAL_RELOCATION_RETRY),
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.retry_parked_relocation(&retry_tab, cx);
            })),
        )
        .child(
            button(
                "pane-relocation-go-to-tab",
                i18n.text(k::TERMINAL_RELOCATION_GO_TO_TAB),
                ButtonTone::Neutral,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.go_to_parked_tab(&go_tab, cx);
            })),
        )
        .into_any_element()
}

/// Sidebar agent rows are two lines tall (workspace, then pane label).
pub(super) const AGENT_ROW_HEIGHT: f32 = 44.;

/// The TUI's `●`/`○` rule: a hollow ring once the agent is idle or done, a
/// filled dot while it is working, blocked, or in an unknown state.
pub(super) fn agent_dot_filled(status: AgentStatus) -> bool {
    !matches!(status, AgentStatus::Idle | AgentStatus::Done)
}

pub(super) fn agent_state_dot(status: AgentStatus) -> ochub_ui::gpui::Div {
    let color = status_color(status);
    let dot = div().w(px(8.)).h(px(8.)).flex_none().rounded_full();
    if agent_dot_filled(status) {
        dot.bg(color)
    } else {
        dot.border_1().border_color(color)
    }
}

pub(crate) fn status_color(status: AgentStatus) -> ochub_ui::gpui::Rgba {
    match status {
        AgentStatus::Working => theme::teal(),
        AgentStatus::Blocked => theme::yellow(),
        AgentStatus::Done => theme::green(),
        AgentStatus::Idle => theme::muted(),
        AgentStatus::Unknown => theme::border_strong(),
    }
}
