use super::super::*;
use crate::a11y::{
    ChromeA11y, apply_control, apply_list, apply_region, drop_zone_label, event_stream_lost_copy,
    event_stream_status_copy, keyboard_move_state_text, pane_a11y, pane_drag_handle_name,
    pane_drag_state_text,
};

impl OcHerdrView {
    pub(super) fn render_sidebar(
        &mut self,
        chrome: &ChromeA11y,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let session_rows = chrome
            .connections
            .items
            .iter()
            .map(|row| {
                let index = row.index;
                let selected = row.a11y.selected == Some(true);
                let running = row.running;
                apply_control(div().id(("session", index)), &row.a11y)
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(32.))
                    .px_3()
                    .rounded(px(CORNER_COMPACT))
                    .bg(if selected {
                        theme::sidebar_selected()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _window, cx| this.select_session(index, cx)),
                    )
                    .child(status_dot(if running {
                        theme::green()
                    } else {
                        theme::muted()
                    }))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .child(row.a11y.name.clone()),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

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
                let measure_view = view.clone();
                let workspace_target = HierarchyTarget::Workspace {
                    id: row.a11y.id.clone(),
                    label: row.a11y.name.clone(),
                };
                let selected = row.a11y.selected == Some(true);
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
                    status_color(row.agent_status),
                    affiliation.as_deref(),
                )
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
                .child(agent_state_dot(status))
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
                        "connections-heading",
                        i18n.text(k::TERMINAL_SESSIONS),
                    ))
                    .child(
                        apply_list(div().id(chrome.connections.id), &chrome.connections)
                            .flex()
                            .flex_col()
                            .children(session_rows),
                    )
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

    fn tab_preview_card(&self, tab_id: &str, title: String) -> TabPreviewCard {
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

    fn tab_title_needs_fade(title: &str, window: &Window) -> bool {
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

    pub(super) fn render_tab_bar(
        &mut self,
        chrome: &ChromeA11y,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let view = cx.entity();
        let tab_count = chrome.tabs.items.len();
        let authoritative_order = chrome
            .tabs
            .items
            .iter()
            .map(|row| row.a11y.id.clone())
            .collect::<Vec<_>>();
        if self
            .hovered_tab_id
            .as_ref()
            .is_some_and(|hovered| !authoritative_order.contains(hovered))
        {
            self.hovered_tab_id = None;
        }
        if self
            .tab_preview_id
            .as_ref()
            .is_some_and(|id| !authoritative_order.contains(id))
        {
            self.dismiss_tab_preview();
        }
        self.tab_close_reveals
            .retain(|tab_id, _| authoritative_order.contains(tab_id));
        let now = Instant::now();
        let reduce_motion = cx.reduce_motion();
        let close_reveals = authoritative_order
            .iter()
            .map(|tab_id| {
                let target = if self.hovered_tab_id.as_deref() == Some(tab_id.as_str()) {
                    1.
                } else {
                    0.
                };
                let reveal = self
                    .tab_close_reveals
                    .entry(tab_id.clone())
                    .or_insert_with(|| Transition::settled(0., TAB_CLOSE_ANIMATION));
                reveal.retarget(target, now, reduce_motion);
                if reveal.is_animating(now, reduce_motion) {
                    window.request_animation_frame();
                }
                reveal.value(now, reduce_motion)
            })
            .collect::<Vec<_>>();
        self.shortcut_reveal
            .retarget(if self.command_held { 1. } else { 0. }, now, reduce_motion);
        if self.shortcut_reveal.is_animating(now, reduce_motion) {
            window.request_animation_frame();
        }
        let shortcut_reveal = self.shortcut_reveal.value(now, reduce_motion);
        let drag = match &self.surface_drag {
            SurfaceDrag::Reorder(drag) => Some(drag),
            _ => None,
        };
        let pending = self
            .pending_reorder
            .as_ref()
            .and_then(|pending| pending.display.as_ref());
        let projection = self
            .selection
            .workspace_id
            .as_deref()
            .and_then(|workspace_id| {
                reorder_projection(
                    &ReorderList::Tabs {
                        workspace_id: workspace_id.to_owned(),
                    },
                    &authoritative_order,
                    drag,
                    pending,
                )
            });
        let tab_reorder = projection.and_then(|projection| {
            let rects = authoritative_order
                .iter()
                .map(|id| {
                    self.reorder_metrics
                        .tabs
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
                TAB_REORDER_GAP_PX,
                ReorderAxis::Horizontal,
            );
            Some((
                projection.source_id,
                projection.positions,
                offsets.previous,
                offsets.current,
                projection.motion,
            ))
        });
        let tabs = chrome
            .tabs
            .items
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let shortcut = tab_key_equivalent(index, tab_count);
                let close_reveal = close_reveals[index];
                let tab_id = row.a11y.id.clone();
                let press_id = tab_id.clone();
                let measure_id = tab_id.clone();
                let hover_id = tab_id.clone();
                let move_hover_id = tab_id.clone();
                let debug_tab_id = tab_id.clone();
                let debug_title_id = tab_id.clone();
                let debug_close_id = tab_id.clone();
                let debug_fade_id = tab_id.clone();
                let debug_shortcut_id = tab_id.clone();
                let measure_view = view.clone();
                let tab_hover_group: SharedString = format!("tab-hover-{tab_id}").into();
                let tab_target = HierarchyTarget::Tab {
                    id: row.a11y.id.clone(),
                    label: row.a11y.name.clone(),
                };
                let close_target = tab_target.clone();
                let selected = row.a11y.selected == Some(true);
                let title_needs_fade = Self::tab_title_needs_fade(&row.a11y.name, window);
                let fade_background = if selected {
                    theme::current().bg.rgba()
                } else {
                    theme::sidebar_background()
                };
                let fade_hover_background = if selected {
                    theme::current().bg.rgba()
                } else {
                    theme::surface_hover()
                };
                let (hidden, display_position, previous_offset, offset, motion) =
                    if let Some((source_id, positions, previous_offsets, offsets, motion)) =
                        &tab_reorder
                    {
                        (
                            source_id == &tab_id && *motion == ReorderMotion::Dragging,
                            positions[index],
                            previous_offsets[index],
                            offsets[index],
                            Some(*motion),
                        )
                    } else {
                        (false, index, (0., 0.), (0., 0.), None)
                    };
                let tab = apply_control(div().id(("main-tab", row.number)), &row.a11y)
                    // The hint is visual only; assistive tech hears the
                    // shortcut whether or not Command is down.
                    .when_some(shortcut.as_deref(), |tab, shortcut| {
                        tab.aria_label(format!("{}, {shortcut}", row.a11y.name))
                    })
                    .relative()
                    .flex()
                    .items_center()
                    .flex_none()
                    .h(px(TAB_PILL_HEIGHT))
                    .w(px(TAB_PILL_WIDTH))
                    .overflow_hidden()
                    .rounded_full()
                    .border_1()
                    .border_color(if selected {
                        theme::border()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .bg(if selected {
                        theme::current().bg.rgba()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .text_sm()
                    .text_color(if selected {
                        theme::text()
                    } else {
                        theme::muted()
                    })
                    .hover(move |style| {
                        style.bg(if selected {
                            theme::current().bg.rgba()
                        } else {
                            theme::surface_hover()
                        })
                    })
                    .when(tab_count >= 2, |tab| tab.cursor_grab())
                    .when(tab_count < 2, |tab| tab.cursor_pointer())
                    .group(tab_hover_group.clone())
                    .opacity(if hidden { 0. } else { 1. })
                    .debug_selector(move || format!("tab-{debug_tab_id}"))
                    .on_hover(cx.listener(move |this, hovered, _window, cx| {
                        this.set_tab_hovered(hover_id.clone(), *hovered, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, _window, cx| {
                            this.press_tab_pill(press_id.clone(), event, cx);
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
                    .on_mouse_move(cx.listener(move |this, event, window, cx| {
                        this.set_tab_hovered(move_hover_id.clone(), true, cx);
                        this.pane_mouse_move(event, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event, window, cx| {
                            this.open_context_menu(tab_target.clone(), event, window, cx)
                        }),
                    )
                    .child(
                        div()
                            .id(("tab-title", row.number))
                            .flex_1()
                            .min_w_0()
                            .px(px(TAB_TITLE_ACTION_WELL))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_overflow(TextOverflow::Truncate("".into()))
                            .text_center()
                            .debug_selector(move || format!("tab-title-{debug_title_id}"))
                            .child(row.a11y.name.clone()),
                    )
                    // Absolutely positioned, so showing or hiding the hint
                    // never moves the title.
                    .when_some(shortcut, |tab_row, shortcut| {
                        tab_row.child(
                            div()
                                .absolute()
                                .right(px(10.))
                                .top_0()
                                .bottom_0()
                                .flex()
                                .items_center()
                                .opacity(shortcut_reveal)
                                .when(shortcut_reveal <= f32::EPSILON, |hint| hint.invisible())
                                .when(shortcut_reveal > f32::EPSILON, |hint| {
                                    hint.debug_selector(move || {
                                        format!("tab-shortcut-{debug_shortcut_id}")
                                    })
                                })
                                .text_xs()
                                .text_color(if selected {
                                    theme::text()
                                } else {
                                    theme::muted()
                                })
                                .child(shortcut),
                        )
                    })
                    .when(title_needs_fade, |tab_row| {
                        tab_row.child(
                            div()
                                .debug_selector(move || format!("tab-title-fade-{debug_fade_id}"))
                                .absolute()
                                .right(px(TAB_TITLE_ACTION_WELL))
                                .top(px(1.))
                                .bottom(px(1.))
                                .w(px(TAB_TITLE_FADE_WIDTH))
                                .bg(linear_gradient(
                                    90.,
                                    linear_color_stop(fade_background.alpha(0.), 0.),
                                    linear_color_stop(fade_background, 1.),
                                ))
                                .group_hover(tab_hover_group, move |style| {
                                    style.bg(linear_gradient(
                                        90.,
                                        linear_color_stop(fade_hover_background.alpha(0.), 0.),
                                        linear_color_stop(fade_hover_background, 1.),
                                    ))
                                }),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(7.))
                            .top_0()
                            .bottom_0()
                            .flex()
                            .items_center()
                            .opacity(close_reveal)
                            .when(close_reveal <= f32::EPSILON, |close| close.invisible())
                            .child(
                                icon_only_button_tone(
                                    ("close-tab", row.number),
                                    i18n.text(k::TERMINAL_CLOSE_TAB),
                                    IconName::Close,
                                    ButtonTone::Ghost,
                                    ButtonSize::Sm,
                                )
                                .size(px(18.))
                                .rounded_full()
                                .debug_selector(move || format!("close-tab-{debug_close_id}"))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(
                                    move |this, _, _window, cx| {
                                        this.request_close(close_target.clone(), cx)
                                    },
                                )),
                            ),
                    );
                let tab = if tab_reorder.is_some() {
                    let animation_name = match motion {
                        Some(ReorderMotion::Dragging) => "reorder-shift",
                        Some(ReorderMotion::Settling { .. }) => "reorder-settle",
                        None => unreachable!("tab reorder animation requires a motion phase"),
                    };
                    tab.with_animation(
                        (
                            ElementId::named_usize(animation_name, display_position),
                            tab_id.clone(),
                        ),
                        Animation::new(REORDER_ANIMATION).with_easing(ease_out_quint()),
                        move |tab, delta| {
                            tab.left(px(
                                previous_offset.0 + (offset.0 - previous_offset.0) * delta
                            ))
                            .top(px(
                                previous_offset.1 + (offset.1 - previous_offset.1) * delta
                            ))
                        },
                    )
                    .into_any_element()
                } else {
                    tab.into_any_element()
                };
                div()
                    .id((ElementId::from("tab-slot"), tab_id))
                    .relative()
                    .flex_none()
                    .child(tab)
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                measure_view.update(cx, |this, _cx| {
                                    this.note_reorder_span(
                                        true,
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
                        // in-flow pill instead of the slot origin.
                        .top(px(0.))
                        .left(px(0.))
                        .size_full(),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let pane_id_right = self.selection.pane_id.clone();
        let pane_id_down = self.selection.pane_id.clone();
        let pane_id_zoom = self.selection.pane_id.clone();
        let pane_id_close = self.selection.pane_id.clone();
        let node_manager_open = self.overlay.host_center();
        div()
            .flex()
            .items_center()
            .h(px(HEADER_HEIGHT))
            .pr_2()
            .gap_1()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            // The strip's leading inset is a move area rather than padding so
            // the gutter left of the first tab also drags the window.
            .child(
                self.window_move_area("tab-strip-lead", cx)
                    .w(px(TAB_STRIP_LEAD_INSET)),
            )
            .child(
                apply_list(div().id(chrome.tabs.id), &chrome.tabs)
                    .flex()
                    .items_center()
                    .h_full()
                    .min_w_0()
                    .gap(px(TAB_REORDER_GAP_PX))
                    .overflow_x_scroll()
                    .overflow_y_hidden()
                    .track_scroll(&self.tab_scroll)
                    .children(tabs),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "new-tab",
                        chrome.toolbar.new_tab.name.clone(),
                        IconName::Add,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    )
                    .rounded_full(),
                    &chrome.toolbar.new_tab,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.create_tab(cx))),
            )
            // Everything between `+` and the toolbar is empty strip, so a
            // drag there means "move the window".
            .child(self.window_move_area("tab-strip-space", cx).flex_1())
            .child(div().id("pane-actions").role(ochub_ui::gpui::Role::Toolbar).aria_label(i18n.text(k::TERMINAL_PANE_ACTIONS)).flex().items_center().gap_1().px_2()
            .child(
                apply_control(
                    icon_only_button_tone(
                    "split-right",
                    chrome.toolbar.split_right.name.clone(),
                    IconName::Blocks,
                    ButtonTone::Primary,
                    ButtonSize::Sm,
                ),
                    &chrome.toolbar.split_right,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_right.clone() {
                        this.invoke(
                            "pane.split",
                            json!({ "target_pane_id": pane_id, "direction": SplitDirection::Right, "focus": true, "right_click": "herdr", "env": {} }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "split-down",
                        chrome.toolbar.split_down.name.clone(),
                        IconName::ChevronDown,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.split_down,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_down.clone() {
                        this.invoke(
                            "pane.split",
                            json!({ "target_pane_id": pane_id, "direction": SplitDirection::Down, "focus": true, "right_click": "herdr", "env": {} }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "zoom-pane",
                        chrome.toolbar.zoom.name.clone(),
                        IconName::Eye,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.zoom,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_zoom.clone() {
                        this.invoke(
                            "pane.zoom",
                            json!({ "pane_id": pane_id, "mode": "toggle" }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "close-pane",
                        chrome.toolbar.close_pane.name.clone(),
                        IconName::Close,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.close_pane,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_close.clone() {
                        let label = this
                            .snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.pane(&pane_id))
                            .map(PaneInfo::display_name)
                            .unwrap_or("pane")
                            .to_owned();
                        this.request_close(HierarchyTarget::Pane { id: pane_id, label }, cx)
                    }
                })),
            )
            )
            .child(div().h(px(22.)).w(px(1.)).bg(theme::border()))
            .child(
                apply_control(
                    icon_only_button_tone(
                        "open-appearance",
                        chrome.toolbar.appearance.name.clone(),
                        IconName::Palette,
                        if matches!(self.overlay, Overlay::Appearance) {
                            ButtonTone::Primary
                        } else {
                            ButtonTone::Ghost
                        },
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.appearance,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.open_appearance(cx))),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "open-herdr-settings",
                        chrome.toolbar.herdr_settings.name.clone(),
                        IconName::Settings,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.herdr_settings,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.open_native_tui(cx))),
            )
            .child(apply_control(
                icon_button_tone(
                    "manage-nodes",
                    chrome.toolbar.remote.name.clone(),
                    IconName::Globe,
                    if node_manager_open { ButtonTone::Primary } else { ButtonTone::Neutral },
                    ButtonSize::Sm,
                ),
                &chrome.toolbar.remote,
            ).mr_3().on_click(cx.listener(|this, _, _window, cx| this.open_node_manager(cx))))
    }

    pub(super) fn render_status_bar(
        &mut self,
        chrome: &ChromeA11y,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let profile = self.current_profile();
        let profile_icon = if matches!(profile, ConnectionProfile::Local { .. }) {
            IconName::Desktop
        } else {
            IconName::Globe
        };
        let profile_label = profile_display_label(&profile, i18n);
        let switcher_open = matches!(self.overlay, Overlay::HostSwitcher);
        let status = if self.prefix_pending {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded(px(CORNER_COMPACT))
                        .bg(theme::accent_fill())
                        .text_color(theme::accent_text())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(i18n.text(k::TERMINAL_PREFIX)),
                )
                .child(i18n.text(k::TERMINAL_PREFIX_HINT))
                .into_any_element()
        } else if let Some(operation) = &self.operation {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(spinner(theme::muted(), 11.))
                .child(operation.clone())
                .into_any_element()
        } else if let EventStreamState::Lost(reason) = &self.event_stream {
            let message = event_stream_lost_copy(i18n);
            div()
                .id((
                    ochub_ui::gpui::ElementId::from("reconnect-live-updates"),
                    reason.clone(),
                ))
                .debug_selector(|| "reconnect-live-updates".into())
                // The enclosing `status-message` control already carries the button role
                // and the localized name; a nested one would announce twice.
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.reload(this.selection.session_name.clone(), cx);
                }))
                .child(status_dot(theme::red()))
                .child(message)
                .into_any_element()
        } else if let Some(snapshot) = &self.snapshot {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(theme::green()))
                .child(event_stream_status_copy(i18n, &self.event_stream, snapshot))
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(theme::muted()))
                .child(i18n.text(k::TERMINAL_NO_SESSION))
                .into_any_element()
        };
        apply_region(div().id(chrome.status.id), &chrome.status)
            .flex()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .text_xs()
            .text_color(theme::muted())
            .child(
                apply_control(div().id("status-profile"), &chrome.status_profile)
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(px(SIDEBAR_WIDTH))
                    .h_full()
                    .px_3()
                    .border_r_1()
                    .border_color(theme::border())
                    .bg(if switcher_open {
                        theme::surface_hover()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| this.toggle_host_switcher(cx)))
                    .child(icon(profile_icon, theme::muted(), 13.))
                    .child(div().flex_1().min_w_0().truncate().child(profile_label)),
            )
            .child(
                apply_control(div().id("status-message"), &chrome.status_message)
                    .flex()
                    .items_center()
                    .min_w_0()
                    .px_3()
                    .child(status),
            )
    }

    pub(super) fn render_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        let Some(snapshot) = self.snapshot.clone() else {
            let cta = button(
                "retry-empty",
                i18n.text(k::COMMON_REFRESH),
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, _window, cx| this.reload(None, cx)))
            .into_any_element();
            return div()
                .id("empty-terminals")
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(i18n.text(k::TERMINAL_NO_RUNNING_SESSION))
                .flex()
                .items_center()
                .justify_center()
                .flex_1()
                .bg(theme::content_background())
                .child(empty_state(
                    IconName::Terminal,
                    i18n.text(k::TERMINAL_NO_RUNNING_SESSION),
                    i18n.text(k::TERMINAL_NO_RUNNING_SESSION_BODY),
                    Some(cta),
                ))
                .into_any_element();
        };
        let Some(tab_id) = self.selection.tab_id.clone() else {
            return div()
                .id("empty-tabs")
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(i18n.text(k::TERMINAL_NO_TABS))
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(empty_state(
                    IconName::Layers,
                    i18n.text(k::TERMINAL_NO_TABS),
                    i18n.text(k::TERMINAL_NO_TABS_BODY),
                    None,
                ))
                .into_any_element();
        };
        let tab_id = tab_id.as_str();
        let layout = snapshot.layout_for(tab_id).cloned();
        let panes = self.rendered_panes_for_tab(&snapshot, tab_id);
        let view = cx.entity();
        let now = Instant::now();
        let reduce_motion = cx.reduce_motion();
        if self.expire_pane_motion(now, reduce_motion) {
            cx.notify();
        }
        let scale = window.scale_factor();
        let pane_drag = match &self.surface_drag {
            SurfaceDrag::Pane(drag) if pane_drag_past_slop(drag) => Some(drag.clone()),
            _ => None,
        };
        let keyboard_move = self
            .pane_keyboard_move
            .clone()
            .filter(|mode| mode.tab_id == tab_id);
        let drag_source_id = pane_drag
            .as_ref()
            .map(|drag| drag.pane_id.clone())
            .or_else(|| keyboard_move.as_ref().map(|mode| mode.pane_id.clone()));
        let draggable = layout
            .as_ref()
            .is_some_and(|layout| !layout.zoomed && layout.panes.len() > 1)
            && !self.tab_relocation_locked(tab_id);
        let mut elements = Vec::new();
        for pane in panes {
            let fractions = self
                .displayed_pane_fractions(layout.as_ref(), &pane.pane_id, now, reduce_motion)
                .unwrap_or((0., 0., 1., 1.));
            let selected = self.selection.pane_id.as_deref() == Some(&pane.pane_id);
            let pane_id = pane.pane_id.clone();
            let pane_target = HierarchyTarget::Pane {
                id: pane.pane_id.clone(),
                label: pane.display_name().to_owned(),
            };
            let frame = self
                .pane(&pane.pane_id)
                .and_then(|runtime| runtime.frame.clone())
                .or_else(|| {
                    drag_source_id
                        .as_deref()
                        .filter(|source| *source == pane.pane_id)
                        .and_then(|_| self.pane_drag_snapshot.clone())
                })
                .or_else(|| {
                    self.pane_relocations
                        .get(tab_id)
                        .and_then(|pending| pending.plan.frame_for(&pane.pane_id))
                });
            let control_action = self.pane_control_action(&pane.pane_id);
            let waiting = frame.is_none();
            let screen_text = if window.is_a11y_active() && !waiting {
                self.pane(&pane.pane_id)
                    .and_then(|runtime| runtime.terminal.read_visible_text())
            } else {
                None
            };
            let a11y = pane_a11y(&pane, selected, screen_text.as_deref(), waiting, i18n);
            // A frozen grid keeps its rendered size inside the clipped body so
            // the last frame stays put while only the shell moves.
            let frozen = if self.pane_resize_frozen(&pane.pane_id) {
                self.pane(&pane.pane_id).and_then(|runtime| {
                    (runtime.pixel_size.0 > 0 && runtime.pixel_size.1 > 0).then(|| {
                        (
                            runtime.pixel_size.0 as f32 / scale,
                            runtime.pixel_size.1 as f32 / scale,
                        )
                    })
                })
            } else {
                None
            };
            let handle = draggable.then(|| {
                let handle_pane_id = pane_id.clone();
                render_pane_drag_handle(&pane, selected, i18n)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.press_pane_handle(handle_pane_id.clone(), event, window, cx);
                        }),
                    )
                    .into_any_element()
            });
            let presentation = PanePresentation {
                fractions,
                source_slot: drag_source_id.as_deref() == Some(pane_id.as_str()),
                frozen,
                handle,
            };
            let scroll_pane_id = pane_id.clone();
            let mouse_pane_id = pane_id.clone();
            elements.push(
                render_pane(
                    PaneRenderInput {
                        pane,
                        frame,
                        presentation,
                        a11y,
                        control_action,
                    },
                    i18n,
                    view.clone(),
                    cx,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_mouse_down(mouse_pane_id.clone(), event, window, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_mouse_up(event, window, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_mouse_up(event, window, cx);
                    }),
                )
                .on_mouse_move(cx.listener(move |this, event, window, cx| {
                    this.pane_mouse_move(event, window, cx);
                }))
                .on_scroll_wheel(
                    cx.listener(move |this, event: &ScrollWheelEvent, _window, cx| {
                        this.scroll_pane(&scroll_pane_id, event, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event, window, cx| {
                        this.open_context_menu(pane_target.clone(), event, window, cx)
                    }),
                )
                .into_any_element(),
            );
        }
        let squeezed = layout
            .as_ref()
            .and_then(|layout| self.squeezed_tab_layout(layout));
        let dragged_split = match &self.surface_drag {
            SurfaceDrag::Split(drag) if drag.tab_id == tab_id => Some(drag.path.clone()),
            _ => None,
        };
        let split_handles = layout.as_ref().map(|layout| {
            layout
                .splits
                .iter()
                .filter_map(|split| {
                    let path = split.path()?;
                    // While a divider is dragged every handle in the tab
                    // follows the squeeze preview, including nested ones
                    // whose rects the dragged split moves.
                    let geometry = squeezed
                        .as_ref()
                        .and_then(|squeezed| squeezed.split(&path))
                        .or_else(|| {
                            Some((
                                rect_fractions(layout.area, split.rect)?,
                                split_line_fraction(
                                    layout.area,
                                    split.rect,
                                    split.direction,
                                    split.ratio,
                                )?,
                            ))
                        })?;
                    let dragging = dragged_split.as_deref() == Some(path.as_slice());
                    render_split_handle(split, geometry, dragging, layout.tab_id.clone(), i18n, cx)
                })
                .collect::<Vec<_>>()
        });
        let split_overlay = match &self.surface_drag {
            SurfaceDrag::Split(drag) if drag.tab_id == tab_id => {
                Some(render_split_drag_overlay(drag, cx))
            }
            _ => None,
        };
        let pane_drag_overlay = pane_drag
            .as_ref()
            .filter(|drag| drag.tab_id == tab_id)
            .map(|drag| self.render_pane_drag_overlay(&snapshot, drag, reduce_motion, cx));
        let keyboard_move_overlay = keyboard_move
            .as_ref()
            .map(|mode| self.render_keyboard_move_overlay(&snapshot, mode, reduce_motion));
        let parked_notice = self
            .parked_relocation(tab_id)
            .map(|pending| render_parked_notice(tab_id, &pending.plan, i18n, cx));
        let origin = self.surface_origin();
        let pane_return = self
            .pane_drag_return
            .clone()
            .filter(|_| !reduce_motion)
            .and_then(|flight| {
                window.request_animation_frame();
                render_pane_drag_return(
                    &snapshot,
                    &flight,
                    origin,
                    self.pane(&flight.pane_id)
                        .and_then(|runtime| runtime.frame.clone())
                        .or_else(|| self.pane_drag_snapshot.clone()),
                    i18n,
                )
            });
        if self
            .pane_relocations
            .get(tab_id)
            .is_some_and(|pending| matches!(pending.phase, RelocationPhase::Settling { .. }))
        {
            window.request_animation_frame();
        }
        let ime_view = cx.entity();
        let ime_focus = self.focus.clone();
        let surface_view = cx.entity();
        div()
            .id("terminal-surface")
            .relative()
            .focusable()
            .tab_stop(true)
            .track_focus(&self.focus)
            // Focused descendant of the root on_key_down. send_key stops
            // propagation after handling, so the same keystroke is not
            // dispatched twice on the bubble path.
            .on_key_down(cx.listener(|this, event, window, cx| this.send_key(event, window, cx)))
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
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(theme::content_background())
            .children(elements)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        surface_view.update(cx, |this, _cx| {
                            this.terminal_surface_bounds = Some((
                                f32::from(bounds.origin.x),
                                f32::from(bounds.origin.y),
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                            ));
                        });
                    },
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &ime_focus,
                            ElementInputHandler::new(bounds, ime_view),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(split_handles.into_iter().flatten())
            .children(split_overlay)
            .children(pane_drag_overlay)
            .children(keyboard_move_overlay)
            .children(pane_return)
            .children(parked_notice)
            .into_any_element()
    }

    /// Keyboard move mode (design §11): the chosen target's highlight and an
    /// accessible status describing the pending intent.
    fn render_keyboard_move_overlay(
        &self,
        snapshot: &HierarchySnapshot,
        mode: &KeyboardPaneMove,
        reduce_motion: bool,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        let origin = self.surface_origin();
        let droppable = mode.droppable();
        let target_name = mode.target.as_ref().map(|hover| {
            snapshot
                .pane(&hover.target_pane_id)
                .map(|pane| pane.display_name().to_owned())
                .unwrap_or_else(|| hover.target_pane_id.clone())
        });
        let state_text = keyboard_move_state_text(
            mode.target
                .as_ref()
                .zip(target_name.as_deref())
                .map(|(hover, name)| (name, hover.zone)),
            droppable,
            i18n,
        );
        let highlight = mode
            .target
            .as_ref()
            .map(|hover| render_drop_highlight(hover, droppable, origin, reduce_motion));
        let zone_label = mode
            .target
            .as_ref()
            .map(|hover| render_drop_zone_label(hover, droppable, origin, reduce_motion, i18n));
        let source_name = snapshot
            .pane(&mode.pane_id)
            .map(|pane| pane.display_name().to_owned())
            .unwrap_or_else(|| mode.pane_id.clone());
        div()
            .id("pane-keyboard-move-overlay")
            .role(ochub_ui::gpui::Role::Status)
            .aria_label(i18n.move_pane_mode(&source_name))
            .aria_value(state_text.clone())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(highlight)
            .children(zone_label)
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(8.))
                    .px_3()
                    .py_1()
                    .rounded(px(CORNER_CONTROL))
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::accent())
                    .text_xs()
                    .text_color(theme::subtext())
                    .child(format!(
                        "{} — {state_text}",
                        i18n.move_pane_mode(&source_name)
                    )),
            )
            .into_any_element()
    }

    /// Everything painted above the panes while one is lifted: the target
    /// highlight and the floating preview (design §5.2).
    fn render_pane_drag_overlay(
        &self,
        snapshot: &HierarchySnapshot,
        drag: &PaneDrag,
        reduce_motion: bool,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        // Gesture math lives in window pixels; this overlay is a child of the
        // `relative()` surface, so everything painted here is surface-local.
        let origin = self.surface_origin();
        let droppable = drag
            .hover
            .as_ref()
            .is_some_and(|hover| hover.droppable(drag.edge_drops));
        let zone = drag.hover.as_ref().map(|hover| hover.zone);
        let state_text = pane_drag_state_text(zone, droppable, i18n);
        let highlight = drag
            .hover
            .as_ref()
            .filter(|_| droppable)
            .map(|hover| render_drop_highlight(hover, true, origin, reduce_motion));
        // The zone label goes above the preview: the floating card follows
        // the pointer, which sits inside the target, so a label drawn in the
        // highlight itself would be hidden under the card.
        let zone_label = drag
            .hover
            .as_ref()
            .filter(|_| droppable)
            .map(|hover| render_drop_zone_label(hover, true, origin, reduce_motion, i18n));
        let pane = snapshot.pane(&drag.pane_id).cloned();
        let frame = self
            .pane(&drag.pane_id)
            .and_then(|runtime| runtime.frame.clone())
            .or_else(|| self.pane_drag_snapshot.clone());
        let opacity = if drag.hover.is_some() && !droppable {
            PANE_DRAG_INVALID_OPACITY
        } else {
            PANE_DRAG_PREVIEW_OPACITY
        };
        let lifted = surface_local(pane_drag_preview_rect(drag), origin);
        let resting = surface_local(
            (
                drag.pointer.0 - drag.grab_offset.0,
                drag.pointer.1 - drag.grab_offset.1,
                drag.source_rect.2,
                drag.source_rect.3,
            ),
            origin,
        );
        let preview = pane.map(|pane| {
            let card = pane_preview_card(&pane, frame, lifted, opacity, i18n);
            if reduce_motion {
                card.into_any_element()
            } else {
                card.with_animation(
                    "pane-drag-lift",
                    Animation::new(PANE_DRAG_LIFT_ANIMATION).with_easing(ease_out_quint()),
                    move |card, t| {
                        let (x, y, w, h) = lerp_rect(resting, lifted, t);
                        card.left(px(x))
                            .top(px(y))
                            .w(px(w))
                            .h(px(h))
                            .opacity(1. + (opacity - 1.) * t)
                    },
                )
                .into_any_element()
            }
        });
        div()
            .id("pane-drag-overlay")
            .role(ochub_ui::gpui::Role::Status)
            .aria_label(
                i18n.drag_pane_handle(
                    snapshot
                        .pane(&drag.pane_id)
                        .map(|pane| pane.display_name())
                        .unwrap_or(&drag.pane_id),
                ),
            )
            .aria_value(state_text)
            .absolute()
            .size_full()
            .cursor_grabbing()
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
            .children(highlight)
            .children(preview)
            .children(zone_label)
            .into_any_element()
    }

    pub(super) fn render_reorder_overlay(
        &self,
        drag: &ReorderDrag,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        div()
            .id("reorder-drag-overlay")
            .absolute()
            .size_full()
            .cursor_grabbing()
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
            .when_some(
                reorder_ghost(
                    self.snapshot.as_ref(),
                    &self.reorder_metrics,
                    drag,
                    &drag.order[drag.source_index],
                ),
                |overlay, ghost| overlay.child(ghost),
            )
            .into_any_element()
    }

    pub(super) fn render_tab_preview(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<ochub_ui::gpui::AnyElement> {
        if matches!(self.surface_drag, SurfaceDrag::Reorder(_)) {
            self.dismiss_tab_preview();
            return None;
        }
        let tab_id = self.tab_preview_id.as_ref()?;
        let rect = self
            .reorder_metrics
            .tabs
            .iter()
            .find(|span| span.id == *tab_id)
            .map(|span| span.rect)?;
        let window_width = f32::from(window.viewport_size().width);
        let (x, _) = tab_preview_origin(rect, window_width);
        let title = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .tabs
                    .iter()
                    .find(|tab| tab.tab_id == *tab_id)
                    .map(|tab| tab.label.clone())
            })
            .unwrap_or_default();
        let card = self.tab_preview_card(tab_id, title);
        Some(
            div()
                .id("tab-preview-layer")
                .absolute()
                // Inset auto would use the static in-flow position (T28).
                .top(px(rect.1 + rect.3))
                .left(px(x))
                .pt(px(TAB_PREVIEW_GAP))
                .occlude()
                .on_hover(cx.listener(|this, hovered, _window, cx| {
                    this.set_tab_preview_hovered(*hovered, cx);
                }))
                .child(card.into_element())
                .into_any_element(),
        )
    }
}

/// What a left press on empty tab-strip space does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabStripPress {
    /// Single press: hand the drag to the window server.
    MoveWindow,
    /// Second click of a double-click: the titlebar action macOS is
    /// configured for (zoom or minimise).
    TitlebarDoubleClick,
}

/// Decides between a window drag and the titlebar double-click action from
/// the press's `click_count`.
pub(crate) fn tab_strip_press(click_count: usize) -> TabStripPress {
    if click_count == 2 {
        TabStripPress::TitlebarDoubleClick
    } else {
        TabStripPress::MoveWindow
    }
}

impl OcHerdrView {
    /// Empty tab-strip space: full strip height so the hit area is the visible
    /// gap, not a zero-height line. Controls in the strip are siblings, never
    /// children, so a press here can only mean the window.
    fn window_move_area(&self, id: &'static str, cx: &mut Context<Self>) -> ochub_ui::gpui::Div {
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

fn tab_key_equivalent(index: usize, tab_count: usize) -> Option<String> {
    if tab_count < 2 {
        return None;
    }
    let number = index + 1;
    (1..=9).contains(&number).then(|| format!("⌘{number}"))
}

fn section_label(id: &'static str, label: &'static str) -> impl IntoElement {
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

fn tree_row(
    id: impl Into<ochub_ui::gpui::ElementId>,
    control: &crate::a11y::ControlA11y,
    indent: f32,
    icon_name: IconName,
    selected: bool,
    color: ochub_ui::gpui::Rgba,
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
        .child(status_dot(color))
}

fn reorder_ghost(
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
    let (left, top) = reorder_ghost_origin(
        drag.pointer,
        drag.grab_offset,
        reorder_list_bounds(&rects),
        (drag.source_rect.2, drag.source_rect.3),
        reorder_axis(&drag.list),
    );
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

fn pane_fractions(
    layout: &ocherdr_core::PaneLayout,
    pane_id: &str,
) -> Option<(f32, f32, f32, f32)> {
    let pane = layout.panes.iter().find(|pane| pane.pane_id == pane_id)?;
    rect_fractions(layout.area, pane.rect)
}

fn rect_fractions(
    area: ocherdr_core::LayoutRect,
    rect: ocherdr_core::LayoutRect,
) -> Option<(f32, f32, f32, f32)> {
    let area_w = f32::from(area.width);
    let area_h = f32::from(area.height);
    if area_w == 0. || area_h == 0. {
        return None;
    }
    Some((
        (f32::from(rect.x) - f32::from(area.x)) / area_w,
        (f32::from(rect.y) - f32::from(area.y)) / area_h,
        f32::from(rect.width) / area_w,
        f32::from(rect.height) / area_h,
    ))
}

/// Where the divider of a settled split sits: on the far edge of the first
/// child as Herdr rounds it to cells, the same boundary the pane frames
/// use, so the line and the frames never disagree by a fraction of a cell.
fn split_line_fraction(
    area: ocherdr_core::LayoutRect,
    rect: ocherdr_core::LayoutRect,
    direction: SplitDirection,
    ratio: f32,
) -> Option<f32> {
    let (first, _) = ocherdr_core::split_rect(rect, direction, ratio);
    let (x, y, w, h) = rect_fractions(area, first)?;
    Some(match direction {
        SplitDirection::Right => x + w,
        SplitDirection::Down => y + h,
    })
}

/// Divider of one split (design §5.4): a 10 px hit strip around a 4 px
/// neutral line that turns accent on hover and stays accent while dragged.
/// `geometry` is the split rect and divider line as surface fractions,
/// already squeezed to the preview ratio during a drag.
fn render_split_handle(
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
        theme::border()
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
fn render_split_drag_overlay(
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
struct PanePresentation {
    fractions: (f32, f32, f32, f32),
    /// The pane is the lifted source of a drag: dimmed, dashed border.
    source_slot: bool,
    /// Logical size of the frozen terminal grid; the frame is drawn at that
    /// size and clipped instead of being refitted to the shell.
    frozen: Option<(f32, f32)>,
    handle: Option<ochub_ui::gpui::AnyElement>,
}

/// 20×24 grab area at the left of the title bar (design §5.1). Shown on
/// hover or when the pane is selected; mouse-only, like the split handle,
/// so it never competes with terminal key forwarding.
fn render_pane_drag_handle(
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

struct PaneRenderInput {
    pane: PaneInfo,
    frame: Option<RenderedFrame>,
    presentation: PanePresentation,
    a11y: crate::a11y::PaneA11y,
    control_action: PaneControlAction,
}

fn render_pane(
    input: PaneRenderInput,
    i18n: I18n,
    view: Entity<OcHerdrView>,
    cx: &mut Context<OcHerdrView>,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    let PaneRenderInput {
        pane,
        frame,
        presentation,
        a11y,
        control_action,
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
    let control_pane_id = pane.pane_id.clone();
    let control_debug_id = pane.pane_id.clone();
    let (control_label, control_tone) = match control_action {
        PaneControlAction::Take => (i18n.text(k::TERMINAL_CONTROL_TAKE), ButtonTone::Primary),
        PaneControlAction::ForceTakeover => {
            (i18n.text(k::TERMINAL_CONTROL_FORCE), ButtonTone::Danger)
        }
        PaneControlAction::Release => (i18n.text(k::TERMINAL_CONTROL_RELEASE), ButtonTone::Ghost),
    };
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
                        .child(div().truncate().flex_1().child(pane_name))
                        .when(selected, |header| {
                            header.child(
                                button(
                                    ochub_ui::gpui::ElementId::Name(
                                        format!("terminal-control-{control_pane_id}").into(),
                                    ),
                                    control_label,
                                    control_tone,
                                    ButtonSize::Sm,
                                )
                                .debug_selector(move || {
                                    format!("terminal-control-{control_debug_id}")
                                })
                                .on_click(cx.listener(
                                    move |this, _, _window, cx| {
                                        this.run_pane_control_action(
                                            control_pane_id.clone(),
                                            control_action,
                                            cx,
                                        );
                                    },
                                )),
                            )
                        }),
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
                            let surface = surface(frame.pixel_buffer)
                                .with_frame_lifetime(frame.lifetime)
                                .object_fit(ObjectFit::Contain);
                            container.child(match frozen {
                                Some((fw, fh)) => surface
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .w(px(fw))
                                    .h(px(fh))
                                    .into_any_element(),
                                None => surface.w_full().h_full().into_any_element(),
                            })
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
fn pane_preview_card(
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
                            body.child(
                                surface(frame.pixel_buffer)
                                    .with_frame_lifetime(frame.lifetime)
                                    .object_fit(ObjectFit::Contain)
                                    .w_full()
                                    .h_full(),
                            )
                        }),
                ),
        )
}

/// Window-space rect expressed relative to the terminal surface's origin.
fn surface_local(rect: (f32, f32, f32, f32), origin: (f32, f32)) -> (f32, f32, f32, f32) {
    (rect.0 - origin.0, rect.1 - origin.1, rect.2, rect.3)
}

/// Cancelled or invalid drop: the preview flies back to its slot over
/// `PANE_DRAG_RETURN_ANIMATION` (design §10).
fn render_pane_drag_return(
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
fn drop_zone_tone(droppable: bool) -> ochub_ui::gpui::Rgba {
    if droppable {
        theme::accent()
    } else {
        theme::border_strong()
    }
}

fn render_drop_highlight(
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
fn render_drop_zone_label(
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
fn render_parked_notice(
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
const AGENT_ROW_HEIGHT: f32 = 44.;

/// The TUI's `●`/`○` rule: a hollow ring once the agent is idle or done, a
/// filled dot while it is working, blocked, or in an unknown state.
pub(super) fn agent_dot_filled(status: AgentStatus) -> bool {
    !matches!(status, AgentStatus::Idle | AgentStatus::Done)
}

fn agent_state_dot(status: AgentStatus) -> ochub_ui::gpui::Div {
    let color = status_color(status);
    let dot = div().w(px(8.)).h(px(8.)).flex_none().rounded_full();
    if agent_dot_filled(status) {
        dot.bg(color)
    } else {
        dot.border_1().border_color(color)
    }
}

pub(super) fn status_color(status: AgentStatus) -> ochub_ui::gpui::Rgba {
    match status {
        AgentStatus::Working => theme::teal(),
        AgentStatus::Blocked => theme::yellow(),
        AgentStatus::Done => theme::green(),
        AgentStatus::Idle => theme::muted(),
        AgentStatus::Unknown => theme::border_strong(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TabStripPress, agent_dot_filled, pane_fractions, tab_key_equivalent, tab_strip_press,
    };
    use ocherdr_core::{AgentStatus, LayoutPane, LayoutRect, PaneLayout};

    #[test]
    fn agent_dot_is_hollow_when_idle_or_done_and_filled_otherwise() {
        assert!(!agent_dot_filled(AgentStatus::Idle));
        assert!(!agent_dot_filled(AgentStatus::Done));
        assert!(agent_dot_filled(AgentStatus::Working));
        assert!(agent_dot_filled(AgentStatus::Blocked));
        assert!(agent_dot_filled(AgentStatus::Unknown));
    }

    #[test]
    fn empty_strip_press_moves_the_window_and_a_double_click_zooms() {
        assert_eq!(tab_strip_press(1), TabStripPress::MoveWindow);
        assert_eq!(tab_strip_press(2), TabStripPress::TitlebarDoubleClick);
        // A triple click already started a move on click one and ran the
        // titlebar action on click two; do not run it again.
        assert_eq!(tab_strip_press(3), TabStripPress::MoveWindow);
    }

    fn layout_rect(x: u16, y: u16, width: u16, height: u16) -> LayoutRect {
        LayoutRect {
            x,
            y,
            width,
            height,
        }
    }

    fn pane_layout(area: LayoutRect, panes: &[(&str, LayoutRect)]) -> PaneLayout {
        PaneLayout {
            workspace_id: "w".into(),
            tab_id: "t".into(),
            zoomed: false,
            area,
            focused_pane_id: panes[0].0.into(),
            panes: panes
                .iter()
                .map(|(id, rect)| LayoutPane {
                    pane_id: (*id).into(),
                    focused: false,
                    rect: *rect,
                })
                .collect(),
            splits: Vec::new(),
        }
    }

    #[test]
    fn ghostty_style_tab_hints_use_command_glyph() {
        assert_eq!(tab_key_equivalent(0, 1), None);
        assert_eq!(tab_key_equivalent(0, 2).as_deref(), Some("⌘1"));
        assert_eq!(tab_key_equivalent(1, 2).as_deref(), Some("⌘2"));
        assert_eq!(tab_key_equivalent(8, 9).as_deref(), Some("⌘9"));
        assert_eq!(tab_key_equivalent(9, 10), None);
    }

    #[test]
    fn pane_fractions_split_left_and_right_in_half() {
        let layout = pane_layout(
            layout_rect(0, 0, 100, 50),
            &[
                ("left", layout_rect(0, 0, 50, 50)),
                ("right", layout_rect(50, 0, 50, 50)),
            ],
        );
        assert_eq!(pane_fractions(&layout, "left"), Some((0.0, 0.0, 0.5, 1.0)));
        assert_eq!(pane_fractions(&layout, "right"), Some((0.5, 0.0, 0.5, 1.0)));
    }

    #[test]
    fn pane_fractions_split_top_and_bottom_in_half() {
        let layout = pane_layout(
            layout_rect(0, 0, 100, 80),
            &[
                ("top", layout_rect(0, 0, 100, 40)),
                ("bottom", layout_rect(0, 40, 100, 40)),
            ],
        );
        assert_eq!(pane_fractions(&layout, "top"), Some((0.0, 0.0, 1.0, 0.5)));
        assert_eq!(
            pane_fractions(&layout, "bottom"),
            Some((0.0, 0.5, 1.0, 0.5))
        );
    }

    #[test]
    fn pane_fractions_nested_split_keeps_child_ratios() {
        let layout = pane_layout(
            layout_rect(0, 0, 100, 100),
            &[
                ("left", layout_rect(0, 0, 50, 100)),
                ("right-top", layout_rect(50, 0, 50, 50)),
                ("right-bottom", layout_rect(50, 50, 50, 50)),
            ],
        );
        assert_eq!(pane_fractions(&layout, "left"), Some((0.0, 0.0, 0.5, 1.0)));
        assert_eq!(
            pane_fractions(&layout, "right-top"),
            Some((0.5, 0.0, 0.5, 0.5))
        );
        assert_eq!(
            pane_fractions(&layout, "right-bottom"),
            Some((0.5, 0.5, 0.5, 0.5))
        );
    }

    #[test]
    fn pane_fractions_are_relative_to_a_non_zero_area_origin() {
        let layout = pane_layout(
            layout_rect(10, 20, 80, 40),
            &[
                ("left", layout_rect(10, 20, 40, 40)),
                ("right", layout_rect(50, 20, 40, 40)),
            ],
        );
        assert_eq!(pane_fractions(&layout, "left"), Some((0.0, 0.0, 0.5, 1.0)));
        assert_eq!(pane_fractions(&layout, "right"), Some((0.5, 0.0, 0.5, 1.0)));
    }
}
