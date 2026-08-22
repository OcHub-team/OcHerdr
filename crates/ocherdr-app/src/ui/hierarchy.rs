use super::super::*;
use crate::a11y::{
    apply_control, apply_list, apply_region, event_stream_lost_copy, event_stream_status_copy,
    pane_a11y,
};

impl OcHerdrView {
    pub(super) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let chrome = self.chrome_a11y();
        let session_rows = self
            .sessions
            .iter()
            .enumerate()
            .zip(chrome.connections.items.iter())
            .map(|((index, session), control)| {
                let selected = control.selected == Some(true);
                let running = session.running;
                apply_control(div().id(("session", index)), control)
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
                            .child(control.name.clone()),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let mut hierarchy = Vec::new();
        let mut agent_rows = Vec::new();
        let mut seen_agents = HashSet::new();
        if let Some(snapshot) = &self.snapshot {
            for workspace in &snapshot.workspaces {
                let workspace_id = workspace.workspace_id.clone();
                let workspace_target = HierarchyTarget::Workspace {
                    id: workspace.workspace_id.clone(),
                    label: workspace.label.clone(),
                };
                let selected = self.selection.workspace_id.as_deref() == Some(&workspace_id);
                let control = chrome
                    .workspaces
                    .items
                    .iter()
                    .find(|item| item.id == workspace_id)
                    .cloned()
                    .unwrap_or_else(|| crate::a11y::ControlA11y {
                        id: workspace_id.clone(),
                        role: ochub_ui::gpui::Role::Button,
                        name: workspace.label.clone(),
                        selected: Some(selected),
                        toggled: None,
                        tab_stop: false,
                    });
                hierarchy.push(
                    tree_row(
                        ("workspace", workspace.number),
                        &control,
                        12.,
                        IconName::Folder,
                        selected,
                        status_color(workspace.agent_status),
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
                    .into_any_element(),
                );
            }
            for pane in &snapshot.panes {
                let Some(agent_name) = pane.display_agent.as_deref().or(pane.agent.as_deref())
                else {
                    continue;
                };
                if !seen_agents.insert(agent_name.to_owned()) {
                    continue;
                }
                let pane_id = pane.pane_id.clone();
                let status = pane.agent_status;
                let control = chrome
                    .agents
                    .items
                    .iter()
                    .find(|item| item.id == agent_name)
                    .cloned();
                let row = if let Some(control) = control {
                    apply_control(
                        div().id(ochub_ui::gpui::ElementId::Name(
                            format!("agent-{pane_id}").into(),
                        )),
                        &control,
                    )
                } else {
                    div()
                        .id(ochub_ui::gpui::ElementId::Name(
                            format!("agent-{pane_id}").into(),
                        ))
                        .role(ochub_ui::gpui::Role::Button)
                        .aria_label(agent_name.to_owned())
                        .tab_stop(false)
                };
                agent_rows.push(
                    row.flex()
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
                                .child(agent_name.to_owned()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(i18n.text(status.label())),
                        )
                        .into_any_element(),
                );
            }
        }

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
                    .child(section_label("connections-heading", i18n.text("SESSIONS")))
                    .child(
                        apply_list(div().id(chrome.connections.id), &chrome.connections)
                            .flex()
                            .flex_col()
                            .children(session_rows),
                    )
                    .child(section_label("workspaces-heading", i18n.text("WORKSPACES")))
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
                            .child(i18n.text("new")),
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
                            .child(i18n.text("AGENTS"))
                            .child(i18n.text("STATUS")),
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

    pub(super) fn render_tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let chrome = self.chrome_a11y();
        let mut tabs = Vec::new();
        if let (Some(snapshot), Some(workspace_id)) =
            (&self.snapshot, self.selection.workspace_id.as_deref())
        {
            let mut workspace_tabs = snapshot.tabs_for(workspace_id).cloned().collect::<Vec<_>>();
            workspace_tabs.sort_by_key(|tab| tab.number);
            let tab_count = workspace_tabs.len();
            for (index, tab) in workspace_tabs.into_iter().enumerate() {
                let shortcut = tab_key_equivalent(index, tab_count);
                let tab_id = tab.tab_id.clone();
                let tab_target = HierarchyTarget::Tab {
                    id: tab.tab_id.clone(),
                    label: tab.label.clone(),
                };
                let close_target = tab_target.clone();
                let selected = self.selection.tab_id.as_deref() == Some(&tab_id);
                let control = chrome
                    .tabs
                    .items
                    .iter()
                    .find(|item| item.id == tab_id)
                    .cloned();
                let tab_row = if let Some(control) = control.as_ref() {
                    apply_control(div().id(("main-tab", tab.number)), control)
                } else {
                    div()
                        .id(("main-tab", tab.number))
                        .role(ochub_ui::gpui::Role::Tab)
                        .aria_label(tab.label.clone())
                        .aria_selected(selected)
                };
                tabs.push(
                    tab_row
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
                        .child(div().flex_1().min_w_0().truncate().child(tab.label.clone()))
                        .when_some(shortcut, |row, shortcut| {
                            row.child(
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
                        .when(selected, |row| {
                            row.child(
                                icon_only_button_tone(
                                    ("close-tab", tab.number),
                                    i18n.text("Close tab"),
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
                        .into_any_element(),
                );
            }
        }
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
            .child(div().id("pane-actions").role(ochub_ui::gpui::Role::Toolbar).aria_label(i18n.text("Pane actions")).flex().items_center().gap_1().px_2()
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

    pub(super) fn render_status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let chrome = self.chrome_a11y();
        let profile = self.current_profile();
        let profile_icon = if matches!(profile, ConnectionProfile::Local { .. }) {
            IconName::Desktop
        } else {
            IconName::Globe
        };
        let profile_label = profile_display_label(&profile, i18n);
        let switcher_open = matches!(self.overlay, Overlay::HostSwitcher);
        let status =
            if self.prefix_pending {
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
                            .child(i18n.text("PREFIX")),
                    )
                    .child(i18n.text(
                        "C new tab · ⇧N new workspace · S settings in Terminal · 1–9 switch tab",
                    ))
                    .into_any_element()
            } else if let Some(operation) = &self.operation {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(spinner(theme::muted(), 11.))
                    .child(operation.clone())
                    .into_any_element()
            } else if self.error.is_some() {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_dot(theme::red()))
                    .child(i18n.text("Connection unavailable"))
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
                    .child(i18n.text("No Herdr session"))
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
        self.resize_session_terminals(window);
        let Some(snapshot) = self.snapshot.clone() else {
            let cta = button(
                "retry-empty",
                i18n.text("Refresh"),
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, _window, cx| this.reload(None, cx)))
            .into_any_element();
            return div()
                .id("empty-terminals")
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(i18n.text("No running Herdr session"))
                .flex()
                .items_center()
                .justify_center()
                .flex_1()
                .bg(theme::content_background())
                .child(empty_state(
                    IconName::Terminal,
                    i18n.text("No running Herdr session"),
                    i18n.text("Start Herdr locally or open Remote in the top-right."),
                    Some(cta),
                ))
                .into_any_element();
        };
        let Some(tab_id) = self.selection.tab_id.as_deref() else {
            return div()
                .id("empty-tabs")
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(i18n.text("This session has no tabs"))
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(empty_state(
                    IconName::Layers,
                    i18n.text("This session has no tabs"),
                    i18n.text("Create a workspace to open the first terminal."),
                    None,
                ))
                .into_any_element();
        };
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - SIDEBAR_WIDTH).max(320.);
        let height = (f32::from(viewport.height) - HEADER_HEIGHT - STATUS_BAR_HEIGHT).max(180.);
        let layout = snapshot.layout_for(tab_id).cloned();
        let panes = snapshot.panes_for(tab_id).cloned().collect::<Vec<_>>();
        let mut elements = Vec::new();
        for pane in panes {
            let geometry = layout
                .as_ref()
                .and_then(|layout| {
                    layout
                        .panes
                        .iter()
                        .find(|item| item.pane_id == pane.pane_id)
                        .map(|item| {
                            let area = layout.area;
                            let left = (item.rect.x.saturating_sub(area.x)) as f32
                                / area.width.max(1) as f32
                                * width;
                            let top = (item.rect.y.saturating_sub(area.y)) as f32
                                / area.height.max(1) as f32
                                * height;
                            let pane_width =
                                item.rect.width as f32 / area.width.max(1) as f32 * width;
                            let pane_height =
                                item.rect.height as f32 / area.height.max(1) as f32 * height;
                            (left, top, pane_width, pane_height)
                        })
                })
                .unwrap_or((0., 0., width, height));
            let selected = self.selection.pane_id.as_deref() == Some(&pane.pane_id);
            let pane_id = pane.pane_id.clone();
            self.store_pane_body_bounds(&pane_id, geometry);
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
                render_pane(pane, frame, geometry, a11y, i18n)
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
        let ime_view = cx.entity();
        let ime_focus = self.focus.clone();
        div()
            .id("terminal-surface")
            .relative()
            .focusable()
            .tab_stop(true)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.send_key(event, window, cx)))
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                this.pane_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
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
                    |_, _, _| (),
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

fn render_pane(
    pane: PaneInfo,
    frame: Option<RenderedFrame>,
    geometry: (f32, f32, f32, f32),
    a11y: crate::a11y::PaneA11y,
    i18n: I18n,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    let (left, top, width, height) = geometry;
    let pane_name = a11y.name.clone();
    let selected = a11y.selected;
    let waiting_for_frame = frame.is_none();
    div()
        .id(ochub_ui::gpui::ElementId::Name(
            format!("terminal-pane-{}", pane.pane_id).into(),
        ))
        .role(a11y.role)
        .aria_label(a11y.name.clone())
        .aria_selected(selected)
        .aria_value(a11y.value.clone())
        .absolute()
        .left(px(left + 2.))
        .top(px(top + 2.))
        .w(px((width - 4.).max(40.)))
        .h(px((height - 4.).max(40.)))
        .flex()
        .flex_col()
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
                .flex()
                .items_center()
                .justify_center()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .bg(theme::current().bg.rgba())
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
                            .child(i18n.text("Waiting for terminal frame…")),
                    )
                }),
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
    use super::tab_key_equivalent;

    #[test]
    fn ghostty_style_tab_hints_use_command_glyph() {
        assert_eq!(tab_key_equivalent(0, 1), None);
        assert_eq!(tab_key_equivalent(0, 2).as_deref(), Some("⌘1"));
        assert_eq!(tab_key_equivalent(1, 2).as_deref(), Some("⌘2"));
        assert_eq!(tab_key_equivalent(8, 9).as_deref(), Some("⌘9"));
        assert_eq!(tab_key_equivalent(9, 10), None);
    }
}
