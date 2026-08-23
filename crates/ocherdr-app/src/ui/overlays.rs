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
            .unwrap_or_else(|| i18n.text(k::HOSTS_SWITCH_THIS_HOST).to_owned());
        let cancel = button(
            "cancel-switch-host",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_switch_profile(cx)))
        .into_any_element();
        let confirm = button(
            "confirm-switch-host",
            i18n.text(k::COMMON_SWITCH),
            ButtonTone::Primary,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_switch_profile(cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(
                modal_card(),
                "switch-host-dialog",
                i18n.text(k::HOSTS_SWITCH_TITLE),
            )
            .child(modal_header(i18n.text(k::HOSTS_SWITCH_TITLE)))
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
                            .child(i18n.text(k::HOSTS_SWITCH_DETAIL)),
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
            .unwrap_or(i18n.text(k::HOSTS_REMOVE_THIS_NODE))
            .to_owned();
        let cancel = button(
            "cancel-remove-node",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_remove_node(cx)))
        .into_any_element();
        let remove = button(
            "confirm-remove-node",
            i18n.text(k::HOSTS_REMOVE_NODE),
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_remove_node(cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(
                modal_card(),
                "remove-node-dialog",
                i18n.text(k::HOSTS_REMOVE_NODE_TITLE),
            )
            .child(modal_header(i18n.text(k::HOSTS_REMOVE_NODE_TITLE)))
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
                            .child(i18n.text(k::HOSTS_REMOVE_NODE_DETAIL)),
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
        let count = self.host_center.read(cx).bulk_selection_len();
        let cancel = button(
            "cancel-bulk-remove",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_bulk_remove(cx)))
        .into_any_element();
        let remove = button(
            "confirm-bulk-remove",
            i18n.text(k::HOSTS_BULK_REMOVE),
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_bulk_remove(cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(
                modal_card(),
                "bulk-remove-hosts-dialog",
                i18n.text(k::HOSTS_BULK_REMOVE_TITLE),
            )
            .child(modal_header(i18n.text(k::HOSTS_BULK_REMOVE_TITLE)))
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
                            .child(i18n.text(k::HOSTS_BULK_REMOVE_BODY)),
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
        let kind = target.kind_key();
        let label = target.label().to_owned();
        let cancel = button(
            "cancel-close-target",
            i18n.text(k::COMMON_CANCEL),
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
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(i18n.text(k::COMMON_CLOSE_PROCESSES)),
                        ),
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
        let kind = target.kind_key();
        let pane = matches!(target, HierarchyTarget::Pane { .. });
        let cancel = button(
            "cancel-rename",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| this.cancel_rename(window, cx)))
        .into_any_element();
        let save = button(
            "save-rename",
            i18n.text(k::COMMON_RENAME),
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
                        i18n.text(k::COMMON_NAME),
                        !pane,
                        Some(
                            if pane {
                                i18n.text(k::COMMON_RENAME_PANE_HINT)
                            } else {
                                i18n.text(k::COMMON_RENAME_SESSION_HINT)
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
                        i18n.text(k::COMMON_RENAME),
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
                        i18n.text(k::COMMON_CLOSE),
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
                        i18n.text(k::TERMINAL_NEW_TAB),
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
                        i18n.text(k::COMMON_RENAME),
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
                        i18n.text(k::COMMON_CLOSE),
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
                        i18n.text(k::COMMON_COPY),
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
                        i18n.text(k::TERMINAL_RENAME_PANE),
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
                    (
                        "right",
                        i18n.text(k::TERMINAL_SPLIT_RIGHT),
                        SplitDirection::Right,
                    ),
                    (
                        "down",
                        i18n.text(k::TERMINAL_SPLIT_DOWN),
                        SplitDirection::Down,
                    ),
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
                        i18n.text(k::TERMINAL_ZOOM),
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
                        i18n.text(k::TERMINAL_CLOSE_PANE),
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
