use super::super::*;
use crate::a11y::apply_dialog;

impl OcHerdrView {
    pub(super) fn render_switch_host(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let current = profile_display_label(&self.current_profile(), i18n);
        let Some(next) = profile_index_by_id(&self.profiles, id)
            .map(|index| profile_display_label(&self.profiles[index], i18n))
        else {
            return div().into_any_element();
        };
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
        .into_any_element()
    }

    pub(super) fn render_remove_node(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let Some(node_name) = profile_index_by_id(&self.profiles, id)
            .map(|index| self.profiles[index].label().to_owned())
        else {
            return div().into_any_element();
        };
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
        .into_any_element()
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
            HierarchyTarget::Workspace { id, .. } => {
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
                let create_id = id.clone();
                items.push(
                    context_menu_item(
                        "workspace-menu-new-worktree",
                        i18n.text(k::WORKTREE_NEW),
                        None::<&str>,
                        Some(IconName::Layers),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_worktree_create(create_id.clone(), window, cx)
                    }))
                    .into_any_element(),
                );
                let open_id = id.clone();
                items.push(
                    context_menu_item(
                        "workspace-menu-open-worktree",
                        i18n.text(k::WORKTREE_OPEN),
                        None::<&str>,
                        Some(IconName::Folder),
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.open_worktree_picker(open_id.clone(), cx)
                    }))
                    .into_any_element(),
                );
                let linked = self.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.workspaces.iter().any(|workspace| {
                        workspace.workspace_id == id
                            && workspace
                                .worktree
                                .as_ref()
                                .is_some_and(|info| info.is_linked_worktree)
                    })
                });
                if linked {
                    let workspace_id = id.clone();
                    items.push(
                        context_menu_item(
                            "workspace-menu-remove-worktree",
                            i18n.text(k::WORKTREE_REMOVE),
                            None::<&str>,
                            Some(IconName::Trash),
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.request_remove_worktree(workspace_id.clone(), cx)
                        }))
                        .into_any_element(),
                    );
                }
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
                for (suffix, label) in [
                    ("left", i18n.text(k::TERMINAL_SWAP_PANE_LEFT)),
                    ("right", i18n.text(k::TERMINAL_SWAP_PANE_RIGHT)),
                    ("up", i18n.text(k::TERMINAL_SWAP_PANE_UP)),
                    ("down", i18n.text(k::TERMINAL_SWAP_PANE_DOWN)),
                ] {
                    let pane_id = id.clone();
                    items.push(
                        context_menu_item(
                            ochub_ui::gpui::ElementId::Name(
                                format!("pane-menu-swap-{suffix}").into(),
                            ),
                            label,
                            None::<&str>,
                            Some(IconName::DragHandle),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.overlay = Overlay::None;
                            this.swap_pane_direction(pane_id.clone(), suffix, cx);
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

    pub(super) fn render_worktree_create(
        &mut self,
        advanced: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let cancel = button(
            "cancel-worktree-create",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_worktree_overlay(window, cx)))
        .into_any_element();
        let create = button(
            "confirm-worktree-create",
            i18n.text(k::WORKTREE_CREATE_ACTION),
            ButtonTone::Primary,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| this.submit_worktree_create(window, cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(
                modal_card(),
                "worktree-create-dialog",
                i18n.text(k::WORKTREE_CREATE_TITLE),
            )
            .w(px(440.))
            .rounded(px(CORNER_MODAL))
            .child(modal_header(i18n.text(k::WORKTREE_CREATE_TITLE)))
            .child(
                modal_body()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text())
                            .child(i18n.text(k::WORKTREE_CREATE_BODY)),
                    )
                    .child(field(
                        i18n.text(k::WORKTREE_FIELD_LABEL),
                        false,
                        Some(i18n.text(k::WORKTREE_FIELD_LABEL_HINT).into()),
                        self.worktree_label_input.clone(),
                    ))
                    .child(
                        div()
                            .id("worktree-advanced-toggle")
                            .role(ochub_ui::gpui::Role::Button)
                            .tab_stop(false)
                            .aria_label(i18n.text(k::COMMON_ADVANCED))
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(32.))
                            .text_sm()
                            .text_color(theme::subtext())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.toggle_worktree_create_advanced(cx)
                            }))
                            .child(icon(
                                if advanced {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                theme::muted(),
                                13.,
                            ))
                            .child(i18n.text(k::COMMON_ADVANCED)),
                    )
                    .when(advanced, |body| {
                        body.child(field(
                            i18n.text(k::WORKTREE_FIELD_BRANCH),
                            false,
                            Some(i18n.text(k::WORKTREE_FIELD_BRANCH_HINT).into()),
                            self.worktree_branch_input.clone(),
                        ))
                        .child(field(
                            i18n.text(k::WORKTREE_FIELD_BASE),
                            false,
                            Some(i18n.text(k::WORKTREE_FIELD_BASE_HINT).into()),
                            self.worktree_base_input.clone(),
                        ))
                        .child(field(
                            i18n.text(k::WORKTREE_FIELD_PATH),
                            false,
                            Some(i18n.text(k::WORKTREE_FIELD_PATH_HINT).into()),
                            self.worktree_path_input.clone(),
                        ))
                    }),
            )
            .child(modal_footer(vec![cancel, create])),
        )
        .top_0()
        .left_0()
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }

    pub(super) fn render_worktree_open(
        &mut self,
        state: &WorktreeOpenState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let cancel = button(
            "cancel-worktree-open",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_worktree_overlay(window, cx)))
        .into_any_element();
        let body = match state {
            WorktreeOpenState::Loading { .. } => div()
                .flex()
                .items_center()
                .gap_2()
                .py_4()
                .child(spinner(theme::muted(), 14.))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::muted())
                        .child(i18n.text(k::WORKTREE_OPEN_LOADING)),
                )
                .into_any_element(),
            WorktreeOpenState::Failed { error, .. } => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text())
                        .child(i18n.text(k::WORKTREE_OPEN_FAILED)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(error.clone()),
                )
                .into_any_element(),
            WorktreeOpenState::Ready { worktrees, .. } if worktrees.is_empty() => empty_state(
                IconName::Folder,
                i18n.text(k::WORKTREE_OPEN_EMPTY_TITLE),
                i18n.text(k::WORKTREE_OPEN_EMPTY_BODY),
                None,
            )
            .into_any_element(),
            WorktreeOpenState::Ready { worktrees, .. } => div()
                .id("worktree-open-list")
                .flex()
                .flex_col()
                .gap_1()
                .max_h(px(320.))
                .overflow_scroll()
                .children(worktrees.iter().enumerate().map(|(index, entry)| {
                    let path = entry.path.clone();
                    let title = entry.display_name().to_owned();
                    let leaf = entry
                        .path
                        .rsplit(['/', '\\'])
                        .find(|segment| !segment.is_empty())
                        .unwrap_or(entry.path.as_str())
                        .to_owned();
                    let open = entry.open_workspace_id.is_some();
                    let kind = if entry.is_linked_worktree {
                        i18n.text(k::WORKTREE_OPEN_LINKED)
                    } else {
                        i18n.text(k::WORKTREE_OPEN_MAIN)
                    };
                    div()
                        .id(("worktree-open-row", index))
                        .role(ochub_ui::gpui::Role::Button)
                        .tab_stop(false)
                        .aria_label(title.clone())
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(px(CORNER_COMPACT))
                        .hover(|style| style.bg(theme::surface_hover()))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.open_listed_worktree(path.clone(), cx)
                        }))
                        .child(icon(
                            if entry.is_linked_worktree {
                                IconName::Layers
                            } else {
                                IconName::Folder
                            },
                            theme::muted(),
                            13.,
                        ))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .text_color(theme::text())
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme::muted())
                                        .child(leaf),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(if open {
                                    format!("{} · {}", kind, i18n.text(k::WORKTREE_OPEN_ALREADY))
                                } else {
                                    kind.to_owned()
                                }),
                        )
                        .into_any_element()
                }))
                .into_any_element(),
        };
        modal_overlay(
            apply_dialog(
                modal_card(),
                "worktree-open-dialog",
                i18n.text(k::WORKTREE_OPEN_TITLE),
            )
            .w(px(480.))
            .rounded(px(CORNER_MODAL))
            .child(modal_header(i18n.text(k::WORKTREE_OPEN_TITLE)))
            .child(modal_body().child(body))
            .child(modal_footer(vec![cancel])),
        )
        .top_0()
        .left_0()
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }

    pub(super) fn render_remove_worktree(
        &mut self,
        label: &str,
        prompt: &RemoveWorktreePrompt,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let (title, body, detail, action) = match prompt {
            RemoveWorktreePrompt::Safe => (
                i18n.text(k::WORKTREE_REMOVE_TITLE),
                i18n.remove_worktree_prompt(label),
                i18n.text(k::WORKTREE_REMOVE_BODY).to_owned(),
                i18n.text(k::WORKTREE_REMOVE_ACTION),
            ),
            RemoveWorktreePrompt::Force { error } => (
                i18n.text(k::WORKTREE_REMOVE_FORCE_TITLE),
                i18n.force_remove_worktree_prompt(label),
                i18n.force_remove_worktree_detail(error),
                i18n.text(k::WORKTREE_REMOVE_FORCE_ACTION),
            ),
        };
        let cancel = button(
            "cancel-remove-worktree",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_remove_worktree(cx)))
        .into_any_element();
        let confirm = button(
            "confirm-remove-worktree",
            action,
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_remove_worktree(cx)))
        .into_any_element();
        modal_overlay(
            apply_dialog(modal_card(), "remove-worktree-dialog", title)
                .child(modal_header(title))
                .child(
                    modal_body()
                        .child(div().text_sm().text_color(theme::text()).child(body))
                        .child(div().text_xs().text_color(theme::muted()).child(detail)),
                )
                .child(modal_footer(vec![cancel, confirm])),
        )
        .top_0()
        .left_0()
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }
}
