use super::super::*;
use crate::a11y::apply_dialog;

impl OcHerdrView {
    pub(super) fn render_switch_host(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let current = profile_display_label(&self.current_profile(), i18n);
        let next = self
            .pending_switch_profile
            .and_then(|index| self.profiles.get(index))
            .map(|profile| profile_display_label(profile, i18n))
            .unwrap_or_else(|| i18n.text("this host").to_owned());
        let cancel = button(
            "cancel-switch-host",
            i18n.text("Cancel"),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_switch_profile(cx)))
        .into_any_element();
        let confirm = button(
            "confirm-switch-host",
            i18n.text("Switch"),
            ButtonTone::Primary,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_switch_profile(cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(
                modal_card(),
                "switch-host-dialog",
                i18n.text("Switch host?"),
            )
            .child(modal_header(i18n.text("Switch host?")))
            .child(
                modal_body()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text())
                            .child(i18n.switch_host_prompt(&current, &next)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(i18n.text(
                                "OcHerdr will leave the current Herdr session and attach to the other machine.",
                            )),
                    ),
            )
            .child(modal_footer(vec![cancel, confirm])),
        )
        .top_0()
        .left_0()
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }

    pub(super) fn render_remove_node(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let node_name = self
            .pending_remove_profile
            .and_then(|index| self.profiles.get(index))
            .map(ConnectionProfile::label)
            .unwrap_or(i18n.text("this node"))
            .to_owned();
        let cancel = button(
            "cancel-remove-node",
            i18n.text("Cancel"),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_remove_node(cx)))
        .into_any_element();
        let remove = button(
            "confirm-remove-node",
            i18n.text("Remove node"),
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_remove_node(cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(
                modal_card(),
                "remove-node-dialog",
                i18n.text("Remove SSH node?"),
            )
                .child(modal_header(i18n.text("Remove SSH node?")))
                .child(
                    modal_body()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text())
                                .child(i18n.remove_node_prompt(&node_name)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(
                                    i18n.text("This only removes the saved node profile. SSH keys and ~/.ssh/config are not changed."),
                                ),
                        ),
                )
                .child(modal_footer(vec![cancel, remove])),
        )
        .top_0()
        .left_0()
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }

    pub(super) fn render_bulk_remove(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let count = self.host_bulk_selection.len();
        let cancel = button(
            "cancel-bulk-remove",
            i18n.text("Cancel"),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_bulk_remove(cx)))
        .into_any_element();
        let remove = button(
            "confirm-bulk-remove",
            i18n.text("Remove local data"),
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_bulk_remove(cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(
                modal_card(),
                "bulk-remove-hosts-dialog",
                i18n.text("Remove local host data?"),
            )
            .child(modal_header(i18n.text("Remove local host data?")))
            .child(
                modal_body()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text())
                            .child(i18n.selected_hosts(count)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(i18n.text(
                                "Saved hosts will be removed. SSH config entries keep their OpenSSH definitions and lose only OcHerdr metadata and overrides.",
                            )),
                    ),
            )
            .child(modal_footer(vec![cancel, remove])),
        )
        .top_0()
        .left_0()
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }

    pub(super) fn render_close_target(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let target = self.pending_close.as_ref();
        let kind = target.map(HierarchyTarget::kind_label).unwrap_or("item");
        let label = target
            .map(HierarchyTarget::label)
            .unwrap_or(i18n.text("this item"))
            .to_owned();
        let cancel = button(
            "cancel-close-target",
            i18n.text("Cancel"),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_close(cx)))
        .into_any_element();
        let close = button(
            "confirm-close-target",
            i18n.close_action(kind),
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_close(cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(modal_card(), "close-target-dialog", i18n.close_title(kind))
                .child(modal_header(i18n.close_title(kind)))
                .child(
                    modal_body()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text())
                                .child(i18n.close_prompt(&label)),
                        )
                        .child(div().text_xs().text_color(theme::muted()).child(
                            i18n.text("Processes owned by this Herdr hierarchy item may be terminated. Closing a final tab also closes its workspace."),
                        )),
                )
                .child(modal_footer(vec![cancel, close])),
        )
        .top_0()
        .left_0()
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }

    pub(super) fn render_rename(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let kind = self
            .rename_target
            .as_ref()
            .map(HierarchyTarget::kind_label)
            .unwrap_or("item");
        let cancel = button(
            "cancel-rename",
            i18n.text("Cancel"),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| this.cancel_rename(window, cx)))
        .into_any_element();
        let save = button(
            "save-rename",
            i18n.text("Rename"),
            ButtonTone::Primary,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| this.submit_rename(window, cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(modal_card(), "rename-dialog", i18n.rename_title(kind))
                .w(px(440.))
                .rounded(px(CORNER_MODAL))
                .child(modal_header(i18n.rename_title(kind)))
                .child(
                    modal_body().child(field(
                        i18n.text("Name"),
                        !matches!(self.rename_target, Some(HierarchyTarget::Pane { .. })),
                        Some(
                            if matches!(self.rename_target, Some(HierarchyTarget::Pane { .. })) {
                                i18n.text("Leave empty to clear the custom pane name.")
                            } else {
                                i18n.text("Saved directly to the active Herdr session.")
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
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }

    pub(super) fn render_context_menu(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let menu = self.context_menu.clone().expect("context menu state");
        let mut items = Vec::new();
        match menu.target.clone() {
            HierarchyTarget::Workspace { .. } => {
                let rename_target = menu.target.clone();
                items.push(
                    context_menu_item(
                        "workspace-menu-rename",
                        i18n.text("Rename"),
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
                        i18n.text("Close"),
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
                        i18n.text("New tab"),
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
                        i18n.text("Rename"),
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
                        i18n.text("Close"),
                        Some("⌃B ⇧X"),
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
                items.push(
                    context_menu_item(
                        "pane-menu-copy",
                        i18n.text("Copy"),
                        Some("⌘C"),
                        Some(IconName::Copy),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.context_menu = None;
                        this.copy_selection(cx);
                    }))
                    .into_any_element(),
                );
                let rename_target = menu.target.clone();
                items.push(
                    context_menu_item(
                        "pane-menu-rename",
                        i18n.text("Rename pane"),
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
                    ("right", i18n.text("Split right"), SplitDirection::Right),
                    ("down", i18n.text("Split down"), SplitDirection::Down),
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
                        i18n.text("Zoom"),
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
                        i18n.text("Close pane"),
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
        let i18n = self.i18n;
        let mut tabs = Vec::new();
        for (index, label) in HERDR_SETTINGS_SECTIONS.iter().enumerate() {
            let selected = self.herdr_settings_section == index;
            let localized_label = i18n.text(label);
            tabs.push(
                div()
                    .id(("herdr-settings-section", index))
                    .role(ochub_ui::gpui::Role::Tab)
                    .aria_label(localized_label)
                    .aria_selected(selected)
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
                    .child(localized_label)
                    .into_any_element(),
            );
        }
        let (title, description, options): (&str, &str, Vec<(&str, &str)>) =
            match self.herdr_settings_section {
                0 => (
                    i18n.text("theme"),
                    i18n.text("Themes exposed by Herdr's native TUI settings."),
                    vec![
                        ("catppuccin", i18n.text("dark")),
                        ("catppuccin-latte", i18n.text("light")),
                        ("terminal", i18n.text("inherit host colors")),
                        ("tokyo-night", i18n.text("dark")),
                        ("tokyo-night-day", i18n.text("light")),
                        ("dracula", i18n.text("dark")),
                        ("nord", i18n.text("dark")),
                        ("gruvbox", i18n.text("dark")),
                        ("gruvbox-light", i18n.text("light")),
                        ("one-dark", i18n.text("dark")),
                        ("one-light", i18n.text("light")),
                        ("solarized", i18n.text("dark")),
                        ("solarized-light", i18n.text("light")),
                        ("kanagawa", i18n.text("dark")),
                        ("kanagawa-lotus", i18n.text("light")),
                        ("rose-pine", i18n.text("dark")),
                        ("rose-pine-dawn", i18n.text("light")),
                        ("vesper", i18n.text("dark")),
                    ],
                ),
                1 => (
                    i18n.text("agent status indicators"),
                    i18n.text("Choose the symbols used for agent state in the TUI."),
                    vec![
                        ("color dots  ● ● ● ○ ·", i18n.text("compact color status")),
                        (
                            "distinct symbols  × ◐ ✓ ○ ·",
                            i18n.text("shape and color status"),
                        ),
                    ],
                ),
                2 => (
                    i18n.text("sound alerts"),
                    i18n.text("Play sounds when agents change state in the background."),
                    vec![
                        (i18n.text("on"), i18n.text("enable alerts")),
                        (i18n.text("off"), i18n.text("silence alerts")),
                    ],
                ),
                3 => (
                    i18n.text("notification popups"),
                    i18n.text("Choose where background notifications are delivered."),
                    vec![
                        (i18n.text("off"), i18n.text("disabled")),
                        (i18n.text("inside herdr"), i18n.text("TUI popup")),
                        (
                            i18n.text("via terminal"),
                            i18n.text("terminal notification"),
                        ),
                        (i18n.text("via system"), i18n.text("macOS notification")),
                    ],
                ),
                4 => (
                    i18n.text("agent border labels"),
                    i18n.text("Show detected agent names in split-pane borders."),
                    vec![
                        (i18n.text("on"), i18n.text("show labels")),
                        (i18n.text("off"), i18n.text("hide labels")),
                    ],
                ),
                _ => (
                    i18n.text("agent integrations"),
                    i18n.text("Let supported agents report state directly to Herdr."),
                    vec![
                        ("Claude Code", i18n.text("integration target")),
                        ("Codex", i18n.text("integration target")),
                        ("Gemini", i18n.text("integration target")),
                        ("OpenCode", i18n.text("integration target")),
                    ],
                ),
            };
        let option_rows = options
            .into_iter()
            .map(|(label, detail)| settings_view_row(label, detail).into_any_element())
            .collect::<Vec<_>>();
        let open_tui = button(
            "open-native-tui-settings",
            i18n.text("Open native TUI"),
            ButtonTone::Neutral,
            ButtonSize::Md,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.open_native_tui(cx)))
        .into_any_element();
        let done = button(
            "close-herdr-settings",
            i18n.text("Done"),
            ButtonTone::Primary,
            ButtonSize::Md,
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_herdr_settings(window, cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(modal_card(), "herdr-settings-dialog", i18n.text("Herdr settings"))
                .w(px(760.))
                .h(px(560.))
                .rounded(px(CORNER_MODAL))
                .child(
                    modal_header(i18n.text("Herdr settings")).child(
                        icon_only_button_tone(
                            "close-herdr-settings-header",
                            i18n.text("Close"),
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
                        .id("herdr-settings-tabs")
                        .role(ochub_ui::gpui::Role::TabList)
                        .aria_label(i18n.text("Herdr settings"))
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
                                    i18n.text("This mirrors Herdr's TUI settings surface. Protocol 20 does not expose live setting values; open the native TUI and press Ctrl+B, then S to inspect or apply the selected session."),
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
