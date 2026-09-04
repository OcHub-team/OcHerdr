use super::*;

impl OcHerdrView {
    pub(crate) fn render_sidebar(
        &mut self,
        chrome: &ChromeA11y,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let view = cx.entity();
        let workspace_count = chrome.workspaces.items.len();
        let authoritative_order = chrome
            .workspaces
            .items
            .iter()
            .map(|row| row.a11y.id.clone())
            .collect::<Vec<_>>();
        let drag = match &self.surface_drag {
            SurfaceDrag::Reorder(drag) => Some(drag),
            _ => None,
        };
        let tab_workspace_drop = drag.and_then(|drag| match &drag.list {
            ReorderList::Tabs { workspace_id }
                if drag.workspace_drop.as_deref() != Some(workspace_id.as_str()) =>
            {
                drag.workspace_drop.as_deref()
            }
            _ => None,
        });
        let pending = self
            .pending_reorder
            .as_ref()
            .and_then(|pending| pending.display.as_ref());
        let projection = reorder_projection(
            &ReorderList::Workspaces,
            &authoritative_order,
            drag,
            pending,
        );
        let workspace_reorder = projection.and_then(|projection| {
            let rects = authoritative_order
                .iter()
                .map(|id| {
                    self.reorder_metrics
                        .workspaces
                        .iter()
                        .find(|span| &span.id == id)
                        .map(|span| span.rect)
                })
                .collect::<Option<Vec<_>>>()?;
            let offsets = reorder_slot_offsets(
                projection.source_index,
                projection.motion,
                &projection.positions,
                &projection.previous_positions,
                &rects,
                0.,
                ReorderAxis::Vertical,
            );
            Some((
                projection.source_id,
                projection.positions,
                offsets.previous,
                offsets.current,
                projection.motion,
            ))
        });
        let hierarchy = chrome
            .workspaces
            .items
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let workspace_id = row.a11y.id.clone();
                let press_id = workspace_id.clone();
                let measure_id = workspace_id.clone();
                let drop_debug_id = workspace_id.clone();
                let measure_view = view.clone();
                let workspace_target = HierarchyTarget::Workspace {
                    id: row.a11y.id.clone(),
                    label: row.a11y.name.clone(),
                };
                let selected = row.a11y.selected == Some(true);
                let tab_drop_targeted = tab_workspace_drop == Some(workspace_id.as_str());
                let tab_drop_label = i18n.drop_move_tab_to_workspace(&row.a11y.name);
                let linked = row
                    .worktree
                    .as_ref()
                    .is_some_and(|info| info.is_linked_worktree);
                let affiliation = row.worktree.as_ref().map(|info| info.affiliation_label());
                let (hidden, display_position, previous_offset, offset, motion) =
                    if let Some((source_id, positions, previous_offsets, offsets, motion)) =
                        &workspace_reorder
                    {
                        (
                            source_id == &workspace_id && *motion == ReorderMotion::Dragging,
                            positions[index],
                            previous_offsets[index],
                            offsets[index],
                            Some(*motion),
                        )
                    } else {
                        (false, index, (0., 0.), (0., 0.), None)
                    };
                let row_el = tree_row(
                    ("workspace", row.number),
                    &row.a11y,
                    12.,
                    if linked {
                        IconName::Layers
                    } else {
                        IconName::Folder
                    },
                    selected,
                    (row.agent_status, self.status_indicators),
                    affiliation.as_deref(),
                )
                .debug_selector({
                    let workspace_id = workspace_id.clone();
                    move || format!("workspace-{workspace_id}")
                })
                .when(tab_drop_targeted, |row| row.aria_label(tab_drop_label))
                .when(workspace_count >= 2, |row| row.cursor_grab())
                .relative()
                .opacity(if hidden { 0. } else { 1. });
                let row_el = if workspace_reorder.is_some() {
                    let animation_name = match motion {
                        Some(ReorderMotion::Dragging) => "reorder-shift",
                        Some(ReorderMotion::Settling { .. }) => "reorder-settle",
                        None => unreachable!("workspace reorder animation requires a motion phase"),
                    };
                    row_el
                        .with_animation(
                            (
                                ElementId::named_usize(animation_name, display_position),
                                workspace_id.clone(),
                            ),
                            Animation::new(REORDER_ANIMATION).with_easing(ease_out_quint()),
                            move |row, delta| {
                                row.left(px(
                                    previous_offset.0 + (offset.0 - previous_offset.0) * delta
                                ))
                                .top(px(
                                    previous_offset.1 + (offset.1 - previous_offset.1) * delta
                                ))
                            },
                        )
                        .into_any_element()
                } else {
                    row_el.into_any_element()
                };
                div()
                    .id(("workspace-slot", row.number))
                    .relative()
                    .rounded(px(CORNER_COMPACT))
                    .when(tab_drop_targeted, |slot| {
                        slot.bg(theme::accent().alpha(0.14))
                            .border_1()
                            .border_color(theme::accent())
                            .debug_selector(move || format!("tab-workspace-drop-{drop_debug_id}"))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, _window, cx| {
                            this.press_workspace_row(press_id.clone(), event, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| {
                            this.pane_mouse_up(event, window, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| {
                            this.pane_mouse_up(event, window, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event, window, cx| {
                        this.pane_mouse_move(event, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event, window, cx| {
                            this.open_context_menu(workspace_target.clone(), event, window, cx)
                        }),
                    )
                    .child(row_el)
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                measure_view.update(cx, |this, _cx| {
                                    this.note_reorder_span(
                                        false,
                                        measure_id.clone(),
                                        (
                                            f32::from(bounds.origin.x),
                                            f32::from(bounds.origin.y),
                                            f32::from(bounds.size.width),
                                            f32::from(bounds.size.height),
                                        ),
                                    );
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        // Inset auto would use the static position below the
                        // in-flow row instead of the slot origin.
                        .top(px(0.))
                        .left(px(0.))
                        .size_full(),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let agent_rows = chrome
            .agents
            .items
            .iter()
            .map(|row| {
                let pane_id = row.pane_id.clone();
                let debug_pane_id = pane_id.clone();
                let menu_pane_id = pane_id.clone();
                let status = row.agent_status;
                let selected = row.a11y.selected == Some(true);
                let workspace_line = row.workspace_line.clone();
                let pane_line = row.pane_line.clone();
                let kind = row.kind.clone();
                apply_control(
                    div().id(ochub_ui::gpui::ElementId::Name(
                        format!("agent-{pane_id}").into(),
                    )),
                    &row.a11y,
                )
                .flex()
                .items_center()
                .gap_2()
                .h(px(AGENT_ROW_HEIGHT))
                .flex_none()
                .px_3()
                .rounded(px(CORNER_COMPACT))
                .bg(if selected {
                    theme::sidebar_selected()
                } else {
                    theme::surface().alpha(0.)
                })
                .hover(|style| style.bg(theme::surface_hover()))
                .cursor_pointer()
                .debug_selector(move || format!("agent-{debug_pane_id}"))
                // Single click jumps to the pane; the second click of a
                // double-click opens the agent panel (also on the context
                // menu), matching the TUI where the list is a navigator.
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.open_agent_panel(pane_id.clone(), window, cx)
                    } else {
                        this.jump_to_agent_pane(pane_id.clone(), window, cx)
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event, window, cx| {
                        this.open_agent_context_menu(menu_pane_id.clone(), event, window, cx)
                    }),
                )
                .child(status_indicator(status, self.status_indicators))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::sidebar_text())
                                .child(workspace_line),
                        )
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(div().truncate().child(pane_line))
                                .children(kind.map(|kind| div().flex_none().child(kind))),
                        ),
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();

        apply_region(div().id(chrome.sidebar.id), &chrome.sidebar)
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme::sidebar_background())
            .text_color(theme::sidebar_text())
            .border_r_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(HEADER_HEIGHT))
                    .pl(px(78.))
                    .pr_4()
                    .gap_2()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, window, _| window.start_window_move()),
                    )
                    .child(
                        div()
                            .id("sidebar-title")
                            .role(ochub_ui::gpui::Role::Heading)
                            .aria_level(1)
                            .aria_label(chrome.sidebar.name.clone())
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(chrome.sidebar.name.clone()),
                    ),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .px_2()
                    .pb_3()
                    .child(section_label(
                        "workspaces-heading",
                        i18n.text(k::TERMINAL_WORKSPACES),
                    ))
                    .child(
                        apply_list(div().id(chrome.workspaces.id), &chrome.workspaces)
                            .flex()
                            .flex_col()
                            .children(hierarchy),
                    )
                    .child(
                        apply_control(div().id("new-workspace"), &chrome.new_workspace)
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(30.))
                            .mt_1()
                            .px_3()
                            .rounded(px(CORNER_COMPACT))
                            .text_xs()
                            .text_color(theme::muted())
                            .hover(|style| {
                                style.bg(theme::surface_hover()).text_color(theme::text())
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _window, cx| this.create_workspace(cx)))
                            .child(icon(IconName::Add, theme::muted(), 12.))
                            .child(i18n.text(k::TERMINAL_NEW_WORKSPACE_SHORT)),
                    )
                    .child(
                        apply_control(div().id("new-worktree"), &chrome.new_worktree)
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(30.))
                            .px_3()
                            .rounded(px(CORNER_COMPACT))
                            .text_xs()
                            .text_color(theme::muted())
                            .hover(|style| {
                                style.bg(theme::surface_hover()).text_color(theme::text())
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_worktree_create_for_selection(window, cx)
                            }))
                            .child(icon(IconName::Layers, theme::muted(), 12.))
                            .child(i18n.text(k::WORKTREE_NEW_SHORT)),
                    )
                    .child(
                        apply_control(div().id("open-worktree"), &chrome.open_worktree)
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(30.))
                            .px_3()
                            .rounded(px(CORNER_COMPACT))
                            .text_xs()
                            .text_color(theme::muted())
                            .hover(|style| {
                                style.bg(theme::surface_hover()).text_color(theme::text())
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.open_worktree_picker_for_selection(cx)
                            }))
                            .child(icon(IconName::Folder, theme::muted(), 12.))
                            .child(i18n.text(k::WORKTREE_OPEN_SHORT)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .max_h(px(220.))
                    .px_2()
                    .pb_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_2()
                            .pt_3()
                            .pb_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::muted())
                            .child(i18n.text(k::TERMINAL_AGENTS)),
                    )
                    .child(
                        apply_list(div().id(chrome.agents.id), &chrome.agents)
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .overflow_scroll()
                            .children(agent_rows),
                    ),
            )
    }

    pub(super) fn tab_preview_card(&self, tab_id: &str, title: String) -> TabPreviewCard {
        let panes = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                let layout = snapshot.layout_for(tab_id);
                snapshot
                    .panes_for(tab_id)
                    .map(|pane| TabPreviewPane {
                        fractions: layout
                            .and_then(|layout| pane_fractions(layout, &pane.pane_id))
                            .unwrap_or((0., 0., 1., 1.)),
                        frame: self
                            .pane(&pane.pane_id)
                            .and_then(|runtime| runtime.frame.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        TabPreviewCard {
            tab_id: tab_id.to_owned(),
            title: title.into(),
            panes,
            waiting: self.i18n.text(k::TERMINAL_WAITING).into(),
        }
    }

    pub(super) fn tab_title_needs_fade(title: &str, window: &Window) -> bool {
        let title: SharedString = title.replace(['\n', '\r'], " ").into();
        let run = TextRun {
            len: title.len(),
            font: window.text_style().font(),
            color: theme::text().into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(title, px(TAB_TITLE_FONT_SIZE), &[run], None);
        shaped.width > px(TAB_PILL_WIDTH - TAB_TITLE_ACTION_WELL * 2.)
    }
}
