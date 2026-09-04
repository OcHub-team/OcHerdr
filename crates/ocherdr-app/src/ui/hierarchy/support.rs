use super::*;

impl OcHerdrView {
    /// Empty tab-strip space: full strip height so the hit area is the visible
    /// gap, not a zero-height line. Controls in the strip are siblings, never
    /// children, so a press here can only mean the window.
    pub(super) fn window_move_area(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::Div {
        div()
            .h_full()
            .flex_none()
            .debug_selector(move || id.to_owned())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, event: &MouseDownEvent, window, _| {
                    match tab_strip_press(event.click_count) {
                        TabStripPress::MoveWindow => window.start_window_move(),
                        TabStripPress::TitlebarDoubleClick => window.titlebar_double_click(),
                    }
                }),
            )
    }
}

pub(super) fn tab_key_equivalent(index: usize, tab_count: usize) -> Option<String> {
    if tab_count < 2 {
        return None;
    }
    let number = index + 1;
    (1..=9)
        .contains(&number)
        .then(|| format!("{PRIMARY_SHORTCUT_SYMBOL}{number}"))
}

pub(super) fn section_label(id: &'static str, label: &'static str) -> impl IntoElement {
    div()
        .id(id)
        .role(ochub_ui::gpui::Role::Heading)
        .aria_level(2)
        .aria_label(label)
        .px_2()
        .pt_4()
        .pb_1()
        .text_color(theme::muted())
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

pub(super) fn tree_row(
    id: impl Into<ochub_ui::gpui::ElementId>,
    control: &crate::a11y::ControlA11y,
    indent: f32,
    icon_name: IconName,
    selected: bool,
    status: (AgentStatus, StatusIndicatorStyle),
    affiliation: Option<&str>,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    apply_control(div().id(id), control)
        .flex()
        .items_center()
        .gap_2()
        .h(px(30.))
        .pl(px(indent))
        .pr_2()
        .rounded(px(CORNER_COMPACT))
        .bg(if selected {
            theme::sidebar_selected()
        } else {
            theme::surface().alpha(0.)
        })
        .hover(|style| style.bg(theme::surface_hover()))
        .cursor_pointer()
        .child(icon(
            icon_name,
            if selected {
                theme::accent()
            } else {
                theme::muted()
            },
            13.,
        ))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_xs()
                .text_color(theme::sidebar_text())
                .child(control.name.clone()),
        )
        .when_some(affiliation, |row, text| {
            row.child(
                div()
                    .flex_none()
                    .max_w(px(108.))
                    .truncate()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(text.to_owned()),
            )
        })
        .child(status_indicator(status.0, status.1))
}

pub(super) fn reorder_ghost(
    snapshot: Option<&HierarchySnapshot>,
    metrics: &ReorderMetrics,
    drag: &ReorderDrag,
    source_id: &str,
) -> Option<ochub_ui::gpui::Div> {
    let spans = match drag.list {
        ReorderList::Workspaces => &metrics.workspaces,
        ReorderList::Tabs { .. } => &metrics.tabs,
    };
    let rects = drag
        .order
        .iter()
        .map(|id| {
            spans
                .iter()
                .find(|span| span.id == *id)
                .map(|span| span.rect)
        })
        .collect::<Option<Vec<_>>>()?;
    let (left, top) =
        if matches!(drag.list, ReorderList::Tabs { .. }) && drag.workspace_drop.is_some() {
            (
                drag.pointer.0 - drag.grab_offset.0,
                drag.pointer.1 - drag.grab_offset.1,
            )
        } else {
            reorder_ghost_origin(
                drag.pointer,
                drag.grab_offset,
                reorder_list_bounds(&rects),
                (drag.source_rect.2, drag.source_rect.3),
                reorder_axis(&drag.list),
            )
        };
    let left = px(left);
    let top = px(top);
    Some(match &drag.list {
        ReorderList::Workspaces => {
            let workspace = snapshot?
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == source_id)?;
            let linked = workspace
                .worktree
                .as_ref()
                .is_some_and(|info| info.is_linked_worktree);
            div()
                .absolute()
                .left(left)
                .top(top)
                .w(px(drag.source_rect.2))
                .h(px(drag.source_rect.3))
                .opacity(0.85)
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .rounded(px(CORNER_COMPACT))
                .bg(theme::sidebar_selected())
                .border_1()
                .border_color(theme::accent())
                .shadow_md()
                .child(icon(
                    if linked {
                        IconName::Layers
                    } else {
                        IconName::Folder
                    },
                    theme::accent(),
                    13.,
                ))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_xs()
                        .child(workspace.label.clone()),
                )
        }
        ReorderList::Tabs { .. } => {
            let tab = snapshot?.tabs.iter().find(|tab| tab.tab_id == source_id)?;
            div()
                .absolute()
                .left(left)
                .top(top)
                .w(px(drag.source_rect.2.max(108.)))
                .h(px(TAB_PILL_HEIGHT))
                .opacity(0.85)
                .flex()
                .items_center()
                .gap_1()
                .px_3()
                .rounded_full()
                .bg(theme::current().bg.rgba())
                .border_1()
                .border_color(theme::accent())
                .shadow_md()
                .child(icon(IconName::Terminal, theme::accent(), 13.))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .child(tab.label.clone()),
                )
        }
    })
}
