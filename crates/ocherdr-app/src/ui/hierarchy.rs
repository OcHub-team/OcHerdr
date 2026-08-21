use super::super::*;

impl OcHerdrView {
    pub(super) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let session_rows = self
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let selected = self.session_index == Some(index);
                let running = session.running;
                let display_name = if session.default {
                    i18n.text("Default").to_owned()
                } else {
                    session.display_name().to_owned()
                };
                div()
                    .id(("session", index))
                    .role(ochub_ui::gpui::Role::Button)
                    .aria_label(display_name.clone())
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
                    .child(div().flex_1().truncate().text_sm().child(display_name))
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
                hierarchy.push(
                    tree_row(
                        ("workspace", workspace.number),
                        &workspace.label,
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
                agent_rows.push(
                    div()
                        .id(ochub_ui::gpui::ElementId::Name(
                            format!("agent-{pane_id}").into(),
                        ))
                        .role(ochub_ui::gpui::Role::Button)
                        .aria_label(agent_name.to_owned())
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

        div()
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
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(i18n.text("Spaces")),
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
                    .child(section_label(i18n.text("CONNECTIONS")))
                    .children(session_rows)
                    .child(section_label(i18n.text("WORKSPACES")))
                    .children(hierarchy)
                    .child(
                        div()
                            .id("new-workspace")
                            .role(ochub_ui::gpui::Role::Button)
                            .aria_label(i18n.text("New workspace"))
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
                        div()
                            .id("agent-scroll")
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
        let mut tabs = Vec::new();
        if let (Some(snapshot), Some(workspace_id)) =
            (&self.snapshot, self.selection.workspace_id.as_deref())
        {
            for tab in snapshot.tabs_for(workspace_id) {
                let tab_id = tab.tab_id.clone();
                let tab_target = HierarchyTarget::Tab {
                    id: tab.tab_id.clone(),
                    label: tab.label.clone(),
                };
                let close_target = tab_target.clone();
                let selected = self.selection.tab_id.as_deref() == Some(&tab_id);
                tabs.push(
                    div()
                        .id(("main-tab", tab.number))
                        .role(ochub_ui::gpui::Role::Button)
                        .flex()
                        .items_center()
                        .h_full()
                        .min_w(px(108.))
                        .max_w(px(180.))
                        .px_3()
                        .gap_2()
                        .border_b_2()
                        .border_color(if selected {
                            theme::accent()
                        } else {
                            theme::surface().alpha(0.)
                        })
                        .bg(if selected {
                            theme::selection()
                        } else {
                            theme::surface().alpha(0.)
                        })
                        .text_sm()
                        .text_color(if selected {
                            theme::text()
                        } else {
                            theme::muted()
                        })
                        .hover(|style| style.bg(theme::surface_hover()))
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
                        .child(icon(IconName::Terminal, theme::muted(), 13.))
                        .child(div().flex_1().truncate().child(tab.label.clone()))
                        .when(selected, |row| {
                            row.child(
                                icon_only_button_tone(
                                    ("close-tab", tab.number),
                                    i18n.text("Close tab"),
                                    IconName::Close,
                                    ButtonTone::Ghost,
                                    ButtonSize::Sm,
                                )
                                .size(px(22.))
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
        let node_manager_open = self.node_manager_open;
        let herdr_settings_open = self.herdr_settings_open;
        div()
            .flex()
            .items_center()
            .h(px(HEADER_HEIGHT))
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h_full()
                    .min_w_0()
                    .overflow_hidden()
                    .children(tabs),
            )
            .child(
                icon_only_button_tone(
                    "new-tab",
                    i18n.text("New tab"),
                    IconName::Add,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.create_tab(cx))),
            )
            .child(div().flex_1())
            .child(div().flex().items_center().gap_1().px_2()
            .child(
                icon_only_button_tone(
                    "split-right",
                    i18n.text("Split pane right"),
                    IconName::Blocks,
                    ButtonTone::Primary,
                    ButtonSize::Sm,
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
                icon_only_button_tone(
                    "split-down",
                    i18n.text("Split pane down"),
                    IconName::ChevronDown,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
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
                icon_only_button_tone(
                    "zoom-pane",
                    i18n.text("Zoom pane"),
                    IconName::Eye,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
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
                icon_only_button_tone(
                    "close-pane",
                    i18n.text("Close pane"),
                    IconName::Close,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
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
                icon_only_button_tone(
                    "open-appearance",
                    i18n.text("Appearance"),
                    IconName::Palette,
                    if self.appearance_open {
                        ButtonTone::Primary
                    } else {
                        ButtonTone::Ghost
                    },
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.open_appearance(cx))),
            )
            .child(
                icon_only_button_tone(
                    "open-herdr-settings",
                    i18n.text("Herdr settings"),
                    IconName::Settings,
                    if herdr_settings_open {
                        ButtonTone::Primary
                    } else {
                        ButtonTone::Ghost
                    },
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.open_herdr_settings(cx))),
            )
            .child(icon_button_tone(
                "manage-nodes",
                i18n.text("Remote"),
                IconName::Globe,
                if node_manager_open { ButtonTone::Primary } else { ButtonTone::Neutral },
                ButtonSize::Sm,
            ).mr_3().on_click(cx.listener(|this, _, _window, cx| this.open_node_manager(cx))))
    }

    pub(super) fn render_status_bar(&self) -> impl IntoElement {
        let i18n = self.i18n;
        let profile = self.current_profile();
        let profile_icon = if matches!(profile, ConnectionProfile::Local { .. }) {
            IconName::Desktop
        } else {
            IconName::Cloud
        };
        let profile_label = if matches!(profile, ConnectionProfile::Local { .. }) {
            i18n.text("Local").to_owned()
        } else {
            profile.label().to_owned()
        };
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
                        .child(i18n.text("PREFIX")),
                )
                .child(i18n.text("C new tab · ⇧N new workspace · S settings · 1–9 switch tab"))
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
        } else if let Some(snapshot) = &self.snapshot {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(theme::green()))
                .child(i18n.herdr_status(
                    &snapshot.version,
                    snapshot.protocol,
                    self.events.is_some(),
                    snapshot.workspaces.len(),
                ))
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
        div()
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
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(px(SIDEBAR_WIDTH))
                    .h_full()
                    .px_3()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(icon(profile_icon, theme::muted(), 13.))
                    .child(div().truncate().child(profile_label)),
            )
            .child(div().flex().items_center().min_w_0().px_3().child(status))
    }

    pub(super) fn render_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        self.resize_visible_terminals(window);
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
            let pane_target = HierarchyTarget::Pane {
                id: pane.pane_id.clone(),
                label: pane.display_name().to_owned(),
            };
            let frame = self
                .panes
                .get(&pane.pane_id)
                .and_then(|runtime| runtime.frame.clone());
            elements.push(
                render_pane(pane, frame, geometry, selected, i18n)
                    .on_click(cx.listener(
                        move |this, _event: &ochub_ui::gpui::ClickEvent, window, cx| {
                            this.select_pane(pane_id.clone(), window, cx);
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
        div()
            .id("terminal-surface")
            .relative()
            .focusable()
            .tab_stop(true)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.send_key(event, window, cx)))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(theme::content_background())
            .children(elements)
            .into_any_element()
    }
}

fn section_label(label: &'static str) -> impl IntoElement {
    div()
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
    label: &str,
    indent: f32,
    icon_name: IconName,
    selected: bool,
    color: ochub_ui::gpui::Rgba,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    div()
        .id(id)
        .role(ochub_ui::gpui::Role::Button)
        .aria_label(label.to_owned())
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
                .child(label.to_owned()),
        )
        .child(status_dot(color))
}

fn render_pane(
    pane: PaneInfo,
    frame: Option<RenderedFrame>,
    geometry: (f32, f32, f32, f32),
    selected: bool,
    i18n: I18n,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    let (left, top, width, height) = geometry;
    let pane_name = pane.display_name().to_owned();
    let waiting_for_frame = frame.is_none();
    div()
        .id(ochub_ui::gpui::ElementId::Name(
            format!("terminal-pane-{}", pane.pane_id).into(),
        ))
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
                .bg(ochub_ui::gpui::rgb(0x1e1e1e))
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
