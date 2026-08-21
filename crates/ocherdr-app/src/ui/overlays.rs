use super::super::*;

impl OcHerdrView {
    pub(super) fn render_remove_node(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let node_name = self
            .pending_remove_profile
            .and_then(|index| self.profiles.get(index))
            .map(ConnectionProfile::label)
            .unwrap_or("this node")
            .to_owned();
        let cancel = button(
            "cancel-remove-node",
            "Cancel",
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_remove_node(cx)))
        .into_any_element();
        let remove = button(
            "confirm-remove-node",
            "Remove node",
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_remove_node(cx)))
        .into_any_element();
        modal_overlay(
            modal_card()
                .child(modal_header("Remove SSH node?"))
                .child(
                    modal_body()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text())
                                .child(format!("Remove {node_name} from OcHerdr?")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(
                                    "This only removes the saved node profile. SSH keys and ~/.ssh/config are not changed.",
                                ),
                        ),
                )
                .child(modal_footer(vec![cancel, remove])),
        )
        .top_0()
        .left_0()
    }

    pub(super) fn render_close_target(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let target = self.pending_close.as_ref();
        let kind = target.map(HierarchyTarget::kind_label).unwrap_or("item");
        let label = target
            .map(HierarchyTarget::label)
            .unwrap_or("this item")
            .to_owned();
        let cancel = button(
            "cancel-close-target",
            "Cancel",
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_close(cx)))
        .into_any_element();
        let close = button(
            "confirm-close-target",
            format!("Close {kind}"),
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_close(cx)))
        .into_any_element();
        modal_overlay(
            modal_card()
                .child(modal_header(format!("Close {kind}?")))
                .child(
                    modal_body()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text())
                                .child(format!("Close {label}?")),
                        )
                        .child(div().text_xs().text_color(theme::muted()).child(
                            "Processes owned by this Herdr hierarchy item may be terminated. Closing a final tab also closes its workspace.",
                        )),
                )
                .child(modal_footer(vec![cancel, close])),
        )
        .top_0()
        .left_0()
    }

    pub(super) fn render_rename(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self
            .rename_target
            .as_ref()
            .map(HierarchyTarget::kind_label)
            .unwrap_or("item");
        let cancel = button(
            "cancel-rename",
            "Cancel",
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| this.cancel_rename(window, cx)))
        .into_any_element();
        let save = button("save-rename", "Rename", ButtonTone::Primary, ButtonSize::Sm)
            .on_click(cx.listener(|this, _, window, cx| this.submit_rename(window, cx)))
            .into_any_element();
        modal_overlay(
            modal_card()
                .w(px(440.))
                .rounded(px(CORNER_MODAL))
                .child(modal_header(format!("Rename {kind}")))
                .child(
                    modal_body().child(field(
                        "Name",
                        !matches!(self.rename_target, Some(HierarchyTarget::Pane { .. })),
                        Some(
                            if matches!(self.rename_target, Some(HierarchyTarget::Pane { .. })) {
                                "Leave empty to clear the custom pane name."
                            } else {
                                "Saved directly to the active Herdr session."
                            }
                            .into(),
                        ),
                        self.rename_input.clone(),
                    )),
                )
                .child(modal_footer(vec![cancel, save])),
        )
        .top_0()
        .left_0()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            match event.keystroke.key.as_str() {
                "enter" => {
                    this.submit_rename(window, cx);
                    cx.stop_propagation();
                }
                "escape" => {
                    this.cancel_rename(window, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
        }))
    }

    pub(super) fn render_context_menu(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = self.context_menu.clone().expect("context menu state");
        let mut items = Vec::new();
        match menu.target.clone() {
            HierarchyTarget::Workspace { .. } => {
                let rename_target = menu.target.clone();
                items.push(
                    context_menu_item(
                        "workspace-menu-rename",
                        "Rename",
                        Some("⌃B ⇧W"),
                        Some(IconName::Pencil),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_rename(rename_target.clone(), window, cx)
                    }))
                    .into_any_element(),
                );
                let close_target = menu.target.clone();
                items.push(
                    context_menu_item(
                        "workspace-menu-close",
                        "Close",
                        Some("⌃B ⇧D"),
                        Some(IconName::Close),
                        true,
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.request_close(close_target.clone(), cx)
                    }))
                    .into_any_element(),
                );
            }
            HierarchyTarget::Tab { .. } => {
                items.push(
                    context_menu_item(
                        "tab-menu-new",
                        "New tab",
                        Some("⌘T"),
                        Some(IconName::Add),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.context_menu = None;
                        this.create_tab(cx)
                    }))
                    .into_any_element(),
                );
                let rename_target = menu.target.clone();
                items.push(
                    context_menu_item(
                        "tab-menu-rename",
                        "Rename",
                        Some("⌃B ⇧T"),
                        Some(IconName::Pencil),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_rename(rename_target.clone(), window, cx)
                    }))
                    .into_any_element(),
                );
                let close_target = menu.target.clone();
                items.push(
                    context_menu_item(
                        "tab-menu-close",
                        "Close",
                        Some("⌘W"),
                        Some(IconName::Close),
                        true,
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.request_close(close_target.clone(), cx)
                    }))
                    .into_any_element(),
                );
            }
            HierarchyTarget::Pane { id, .. } => {
                let rename_target = menu.target.clone();
                items.push(
                    context_menu_item(
                        "pane-menu-rename",
                        "Rename pane",
                        Some("⌃B ⇧P"),
                        Some(IconName::Pencil),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_rename(rename_target.clone(), window, cx)
                    }))
                    .into_any_element(),
                );
                for (suffix, label, direction) in [
                    ("right", "Split right", SplitDirection::Right),
                    ("down", "Split down", SplitDirection::Down),
                ] {
                    let pane_id = id.clone();
                    items.push(
                        context_menu_item(
                            ochub_ui::gpui::ElementId::Name(
                                format!("pane-menu-split-{suffix}").into(),
                            ),
                            label,
                            None::<&str>,
                            Some(IconName::Blocks),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.context_menu = None;
                            this.invoke(
                                "pane.split",
                                json!({ "target_pane_id": pane_id, "direction": direction, "focus": true, "right_click": "herdr", "env": {} }),
                                cx,
                            )
                        }))
                        .into_any_element(),
                    );
                }
                let pane_id = id.clone();
                items.push(
                    context_menu_item(
                        "pane-menu-zoom",
                        "Zoom",
                        None::<&str>,
                        Some(IconName::Eye),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.context_menu = None;
                        this.invoke(
                            "pane.zoom",
                            json!({ "pane_id": pane_id, "mode": "toggle" }),
                            cx,
                        )
                    }))
                    .into_any_element(),
                );
                let close_target = menu.target.clone();
                items.push(
                    context_menu_item(
                        "pane-menu-close",
                        "Close pane",
                        None::<&str>,
                        Some(IconName::Close),
                        true,
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.request_close(close_target.clone(), cx)
                    }))
                    .into_any_element(),
                );
            }
        }
        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close_context_menu(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| this.close_context_menu(cx)),
            )
            .child(
                context_menu("hierarchy-context-menu", items)
                    .absolute()
                    .left(px(menu.x))
                    .top(px(menu.y)),
            )
    }

    pub(super) fn render_herdr_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = Vec::new();
        for (index, label) in HERDR_SETTINGS_SECTIONS.iter().enumerate() {
            let selected = self.herdr_settings_section == index;
            tabs.push(
                div()
                    .id(("herdr-settings-section", index))
                    .role(ochub_ui::gpui::Role::Tab)
                    .aria_label(*label)
                    .px_3()
                    .py_1()
                    .rounded(px(CORNER_COMPACT))
                    .bg(if selected {
                        theme::accent_fill()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .text_xs()
                    .text_color(if selected {
                        theme::accent_text()
                    } else {
                        theme::muted()
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.select_herdr_settings_section(index, cx)
                    }))
                    .child(*label)
                    .into_any_element(),
            );
        }
        let (title, description, options): (&str, &str, Vec<(&str, &str)>) =
            match self.herdr_settings_section {
                0 => (
                    "theme",
                    "Themes exposed by Herdr's native TUI settings.",
                    vec![
                        ("catppuccin", "dark"),
                        ("catppuccin-latte", "light"),
                        ("terminal", "inherit host colors"),
                        ("tokyo-night", "dark"),
                        ("tokyo-night-day", "light"),
                        ("dracula", "dark"),
                        ("nord", "dark"),
                        ("gruvbox", "dark"),
                        ("gruvbox-light", "light"),
                        ("one-dark", "dark"),
                        ("one-light", "light"),
                        ("solarized", "dark"),
                        ("solarized-light", "light"),
                        ("kanagawa", "dark"),
                        ("kanagawa-lotus", "light"),
                        ("rose-pine", "dark"),
                        ("rose-pine-dawn", "light"),
                        ("vesper", "dark"),
                    ],
                ),
                1 => (
                    "agent status indicators",
                    "Choose the symbols used for agent state in the TUI.",
                    vec![
                        ("color dots  ● ● ● ○ ·", "compact color status"),
                        ("distinct symbols  × ◐ ✓ ○ ·", "shape and color status"),
                    ],
                ),
                2 => (
                    "sound alerts",
                    "Play sounds when agents change state in the background.",
                    vec![("on", "enable alerts"), ("off", "silence alerts")],
                ),
                3 => (
                    "notification popups",
                    "Choose where background notifications are delivered.",
                    vec![
                        ("off", "disabled"),
                        ("inside herdr", "TUI popup"),
                        ("via terminal", "terminal notification"),
                        ("via system", "macOS notification"),
                    ],
                ),
                4 => (
                    "agent border labels",
                    "Show detected agent names in split-pane borders.",
                    vec![("on", "show labels"), ("off", "hide labels")],
                ),
                _ => (
                    "agent integrations",
                    "Let supported agents report state directly to Herdr.",
                    vec![
                        ("Claude Code", "integration target"),
                        ("Codex", "integration target"),
                        ("Gemini", "integration target"),
                        ("OpenCode", "integration target"),
                    ],
                ),
            };
        let option_rows = options
            .into_iter()
            .map(|(label, detail)| settings_view_row(label, detail).into_any_element())
            .collect::<Vec<_>>();
        let open_tui = button(
            "open-native-tui-settings",
            "Open native TUI",
            ButtonTone::Neutral,
            ButtonSize::Md,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.open_native_tui(cx)))
        .into_any_element();
        let done = button(
            "close-herdr-settings",
            "Done",
            ButtonTone::Primary,
            ButtonSize::Md,
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_herdr_settings(window, cx)))
        .into_any_element();
        modal_overlay(
            modal_card()
                .w(px(760.))
                .h(px(560.))
                .rounded(px(CORNER_MODAL))
                .child(
                    modal_header("Herdr settings").child(
                        icon_only_button_tone(
                            "close-herdr-settings-header",
                            "Close",
                            IconName::Close,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.close_herdr_settings(window, cx)
                            }),
                        ),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_5()
                        .py_3()
                        .border_b_1()
                        .border_color(theme::border())
                        .children(tabs),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .px_5()
                        .py_4()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(div().text_xs().text_color(theme::muted()).child(description))
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .rounded(px(CORNER_CONTROL))
                                .bg(theme::accent_soft())
                                .text_xs()
                                .text_color(theme::subtext())
                                .child(
                                    "This mirrors Herdr's TUI settings surface. Protocol 20 does not expose live setting values; open the native TUI and press Ctrl+B, then S to inspect or apply the selected session.",
                                ),
                        )
                        .child(
                            div()
                                .id("herdr-settings-options")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .overflow_scroll()
                                .rounded(px(CORNER_PANEL))
                                .border_1()
                                .border_color(theme::border())
                                .children(option_rows),
                        ),
                )
                .child(modal_footer(vec![open_tui, done])),
        )
        .top_0()
        .left_0()
    }
}

fn settings_view_row(label: &'static str, detail: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .min_h(px(38.))
        .px_3()
        .border_b_1()
        .border_color(theme::border())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(theme::text())
                .child(label),
        )
        .child(div().text_xs().text_color(theme::muted()).child(detail))
}
