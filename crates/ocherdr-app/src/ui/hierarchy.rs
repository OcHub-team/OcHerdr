use super::super::*;
use crate::a11y::{
    ChromeA11y, apply_control, apply_list, apply_region, event_stream_lost_copy,
    event_stream_status_copy, pane_a11y,
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

        let hierarchy = chrome
            .workspaces
            .items
            .iter()
            .map(|row| {
                let workspace_id = row.a11y.id.clone();
                let workspace_target = HierarchyTarget::Workspace {
                    id: row.a11y.id.clone(),
                    label: row.a11y.name.clone(),
                };
                let selected = row.a11y.selected == Some(true);
                tree_row(
                    ("workspace", row.number),
                    &row.a11y,
                    12.,
                    IconName::Folder,
                    selected,
                    status_color(row.agent_status),
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.select_workspace(workspace_id.clone(), cx)
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event, window, cx| {
                        this.open_context_menu(workspace_target.clone(), event, window, cx)
                    }),
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
                let status = row.agent_status;
                apply_control(
                    div().id(ochub_ui::gpui::ElementId::Name(
                        format!("agent-{pane_id}").into(),
                    )),
                    &row.a11y,
                )
                .flex()
                .items_center()
                .gap_2()
                .h(px(30.))
                .px_3()
                .rounded(px(CORNER_COMPACT))
                .hover(|style| style.bg(theme::surface_hover()))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_pane(pane_id.clone(), window, cx)
                }))
                .child(status_dot(status_color(status)))
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .child(row.a11y.id.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(i18n.agent_status(status)),
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
                            .child(i18n.text(k::TERMINAL_AGENTS))
                            .child(i18n.text(k::TERMINAL_STATUS)),
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

    pub(super) fn render_tab_bar(
        &mut self,
        chrome: &ChromeA11y,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let tab_count = chrome.tabs.items.len();
        let tabs =
            chrome
                .tabs
                .items
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let shortcut = tab_key_equivalent(index, tab_count);
                    let tab_id = row.a11y.id.clone();
                    let tab_target = HierarchyTarget::Tab {
                        id: row.a11y.id.clone(),
                        label: row.a11y.name.clone(),
                    };
                    let close_target = tab_target.clone();
                    let selected = row.a11y.selected == Some(true);
                    apply_control(div().id(("main-tab", row.number)), &row.a11y)
                        .flex()
                        .items_center()
                        .flex_none()
                        .h(px(TAB_PILL_HEIGHT))
                        .min_w(px(108.))
                        .max_w(px(180.))
                        .px_3()
                        .gap_1()
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
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_tab(tab_id.clone(), cx)
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event, window, cx| {
                                this.open_context_menu(tab_target.clone(), event, window, cx)
                            }),
                        )
                        .child(icon(
                            IconName::Terminal,
                            if selected {
                                theme::accent()
                            } else {
                                theme::muted()
                            },
                            13.,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(row.a11y.name.clone()),
                        )
                        .when_some(shortcut, |tab_row, shortcut| {
                            tab_row.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(if selected {
                                        theme::text()
                                    } else {
                                        theme::muted()
                                    })
                                    .child(shortcut),
                            )
                        })
                        .when(selected, |tab_row| {
                            tab_row.child(
                                icon_only_button_tone(
                                    ("close-tab", row.number),
                                    i18n.text(k::TERMINAL_CLOSE_TAB),
                                    IconName::Close,
                                    ButtonTone::Ghost,
                                    ButtonSize::Sm,
                                )
                                .size(px(18.))
                                .rounded_full()
                                .on_click(cx.listener(
                                    move |this, _, _window, cx| {
                                        this.request_close(close_target.clone(), cx)
                                    },
                                )),
                            )
                        })
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
            .pl_3()
            .pr_2()
            .gap_1()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .child(
                apply_list(div().id(chrome.tabs.id), &chrome.tabs)
                    .flex()
                    .items_center()
                    .h_full()
                    .min_w_0()
                    .gap_1()
                    .overflow_hidden()
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
            .child(div().flex_1())
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
        let Some(tab_id) = self.selection.tab_id.as_deref() else {
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
        let layout = snapshot.layout_for(tab_id).cloned();
        let panes = snapshot.panes_for(tab_id).cloned().collect::<Vec<_>>();
        let view = cx.entity();
        let mut elements = Vec::new();
        for pane in panes {
            let fractions = layout
                .as_ref()
                .and_then(|layout| pane_fractions(layout, &pane.pane_id))
                .unwrap_or((0., 0., 1., 1.));
            let selected = self.selection.pane_id.as_deref() == Some(&pane.pane_id);
            let pane_id = pane.pane_id.clone();
            let pane_target = HierarchyTarget::Pane {
                id: pane.pane_id.clone(),
                label: pane.display_name().to_owned(),
            };
            let frame = self
                .pane(&pane.pane_id)
                .and_then(|runtime| runtime.frame.clone());
            let waiting = frame.is_none();
            let screen_text = if window.is_a11y_active() && !waiting {
                self.pane(&pane.pane_id)
                    .and_then(|runtime| runtime.terminal.read_visible_text())
            } else {
                None
            };
            let a11y = pane_a11y(&pane, selected, screen_text.as_deref(), waiting, i18n);
            let scroll_pane_id = pane_id.clone();
            let mouse_pane_id = pane_id.clone();
            elements.push(
                render_pane(pane, frame, fractions, a11y, i18n, view.clone())
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
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &ScrollWheelEvent, _window, cx| {
                            this.scroll_pane(&scroll_pane_id, event, cx);
                        },
                    ))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event, window, cx| {
                            this.open_context_menu(pane_target.clone(), event, window, cx)
                        }),
                    )
                    .into_any_element(),
            );
        }
        let split_handles = layout.as_ref().map(|layout| {
            layout
                .splits
                .iter()
                .filter_map(|split| {
                    render_split_handle(split, layout.area, layout.tab_id.clone(), i18n, cx)
                })
                .collect::<Vec<_>>()
        });
        let split_overlay = match (&self.surface_drag, layout.as_ref()) {
            (SurfaceDrag::Split(drag), Some(layout)) if drag.tab_id == layout.tab_id => {
                Some(render_split_drag_overlay(layout.area, drag, cx))
            }
            _ => None,
        };
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
            .into_any_element()
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
        .child(status_dot(color))
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

fn split_line_fraction(
    area: ocherdr_core::LayoutRect,
    rect: ocherdr_core::LayoutRect,
    direction: SplitDirection,
    ratio: f32,
) -> Option<f32> {
    let (x, y, w, h) = rect_fractions(area, rect)?;
    Some(match direction {
        SplitDirection::Right => x + w * ratio,
        SplitDirection::Down => y + h * ratio,
    })
}

fn render_split_handle(
    split: &LayoutSplit,
    area: ocherdr_core::LayoutRect,
    tab_id: String,
    i18n: I18n,
    cx: &mut Context<OcHerdrView>,
) -> Option<ochub_ui::gpui::AnyElement> {
    split.path()?;
    let (x, y, w, h) = rect_fractions(area, split.rect)?;
    let line = split_line_fraction(area, split.rect, split.direction, split.ratio)?;
    let split = split.clone();
    let label = i18n.text(k::TERMINAL_RESIZE_SPLIT);
    let group = SharedString::from(format!("split-handle-{}", split.id));
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
                    .group_hover(group, |style| style.bg(theme::accent().alpha(0.45))),
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
                    .group_hover(group, |style| style.bg(theme::accent().alpha(0.45))),
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

fn render_split_drag_overlay(
    area: ocherdr_core::LayoutRect,
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
        .when_some(split_preview_line(area, drag), |overlay, line| {
            overlay.child(line)
        })
        .into_any_element()
}

fn split_preview_line(
    area: ocherdr_core::LayoutRect,
    drag: &SplitDrag,
) -> Option<ochub_ui::gpui::Div> {
    let (x, y, w, h) = rect_fractions(area, drag.rect)?;
    let line = split_line_fraction(area, drag.rect, drag.direction, drag.preview_ratio)?;
    Some(match drag.direction {
        SplitDirection::Right => div()
            .absolute()
            .left(relative(line))
            .top(relative(y))
            .h(relative(h))
            .w(px(SPLIT_HANDLE_VISUAL_PX))
            .ml(px(-SPLIT_HANDLE_VISUAL_PX / 2.))
            .bg(theme::accent()),
        SplitDirection::Down => div()
            .absolute()
            .left(relative(x))
            .top(relative(line))
            .w(relative(w))
            .h(px(SPLIT_HANDLE_VISUAL_PX))
            .mt(px(-SPLIT_HANDLE_VISUAL_PX / 2.))
            .bg(theme::accent()),
    })
}

fn render_pane(
    pane: PaneInfo,
    frame: Option<RenderedFrame>,
    fractions: (f32, f32, f32, f32),
    a11y: crate::a11y::PaneA11y,
    i18n: I18n,
    view: Entity<OcHerdrView>,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    let (x, y, w, h) = fractions;
    let pane_name = a11y.name.clone();
    let selected = a11y.selected;
    let waiting_for_frame = frame.is_none();
    let measure_pane_id = pane.pane_id.clone();
    div()
        .id(ochub_ui::gpui::ElementId::Name(
            format!("terminal-pane-{}", pane.pane_id).into(),
        ))
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
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .border_1()
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
                            container.child(
                                surface(frame.pixel_buffer)
                                    .with_frame_lifetime(frame.lifetime)
                                    .object_fit(ObjectFit::Contain)
                                    .w_full()
                                    .h_full(),
                            )
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

fn status_color(status: AgentStatus) -> ochub_ui::gpui::Rgba {
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
    use super::{pane_fractions, tab_key_equivalent};
    use ocherdr_core::{LayoutPane, LayoutRect, PaneLayout};

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
