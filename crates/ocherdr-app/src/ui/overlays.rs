use super::super::*;
use crate::a11y::apply_dialog;

impl OcHerdrView {
    pub(super) fn render_switch_host(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let current = profile_display_label(&self.current_profile(), i18n);
        let next = self
            .profiles
            .get(index)
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

    pub(super) fn render_remove_node(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let node_name = self
            .profiles
            .get(index)
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

    pub(super) fn render_close_target(
        &mut self,
        target: &HierarchyTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let kind = target.kind_label();
        let label = target.label().to_owned();
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

    pub(super) fn render_rename(
        &mut self,
        target: &HierarchyTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let kind = target.kind_label();
        let pane = matches!(target, HierarchyTarget::Pane { .. });
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
                        !pane,
                        Some(
                            if pane {
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

    pub(super) fn render_context_menu(
        &mut self,
        menu: HierarchyContextMenu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
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
                        this.overlay = Overlay::None;
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
                        this.overlay = Overlay::None;
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
                            this.overlay = Overlay::None;
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
                        this.overlay = Overlay::None;
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
}
