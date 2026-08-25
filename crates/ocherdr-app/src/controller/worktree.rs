use super::*;

impl OcHerdrView {
    pub(crate) fn selected_workspace_target(&self) -> Option<HierarchyTarget> {
        let workspace_id = self.selection.workspace_id.as_deref()?;
        let workspace = self
            .snapshot
            .as_ref()?
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)?;
        Some(HierarchyTarget::Workspace {
            id: workspace.workspace_id.clone(),
            label: workspace.label.clone(),
        })
    }

    pub(crate) fn selected_tab_target(&self) -> Option<HierarchyTarget> {
        let tab_id = self.selection.tab_id.as_deref()?;
        let tab = self
            .snapshot
            .as_ref()?
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)?;
        Some(HierarchyTarget::Tab {
            id: tab.tab_id.clone(),
            label: tab.label.clone(),
        })
    }

    pub(crate) fn selected_pane_target(&self) -> Option<HierarchyTarget> {
        let pane_id = self.selection.pane_id.as_deref()?;
        let pane = self.snapshot.as_ref()?.pane(pane_id)?;
        Some(HierarchyTarget::Pane {
            id: pane.pane_id.clone(),
            label: pane.display_name().to_owned(),
        })
    }

    pub(super) fn cmd_w_close_target(&self) -> Option<HierarchyTarget> {
        let snapshot = self.snapshot.as_ref()?;
        let tab_id = self.selection.tab_id.as_deref()?;
        cmd_w_close_target(snapshot, tab_id, self.selection.pane_id.as_deref())
    }

    pub(crate) fn create_workspace(&mut self, cx: &mut Context<Self>) {
        self.invoke_with_response(
            "workspace.create",
            json!({ "focus": true, "env": {} }),
            Self::follow_created_tab,
            cx,
        );
    }

    /// Switch to the tab a `*.create` / `worktree.open` response names, now
    /// or once the matching events have been applied. `Selection::reconcile`
    /// deliberately ignores `tab.focused` from other clients, so the tab
    /// this client itself asked for has to be selected explicitly.
    pub(super) fn follow_created_tab(
        &mut self,
        result: std::result::Result<Value, HerdrError>,
        cx: &mut Context<Self>,
    ) {
        let Ok(result) = result else {
            return;
        };
        if let Some(tab_id) = created_tab_id(&result) {
            self.pending_created_tab = Some(tab_id);
            self.settle_pending_created_tab(cx);
        }
    }

    /// Select the pending created tab if the snapshot has it and at least one
    /// of its panes; otherwise keep waiting for the events.
    pub(super) fn settle_pending_created_tab(&mut self, cx: &mut Context<Self>) {
        let Some(tab_id) = self.pending_created_tab.clone() else {
            return;
        };
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(tab) = snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id) else {
            return;
        };
        if snapshot.panes_for(&tab_id).next().is_none() {
            return;
        }
        self.pending_created_tab = None;
        self.selection.workspace_id = Some(tab.workspace_id.clone());
        self.select_tab(tab_id, cx);
    }

    pub(crate) fn open_worktree_create_for_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.source_workspace_id() {
            Some(workspace_id) => self.open_worktree_create(workspace_id, window, cx),
            None => self.notify_need_workspace(cx),
        }
    }

    pub(crate) fn open_worktree_picker_for_selection(&mut self, cx: &mut Context<Self>) {
        match self.source_workspace_id() {
            Some(workspace_id) => self.open_worktree_picker(workspace_id, cx),
            None => self.notify_need_workspace(cx),
        }
    }

    /// `workspace_id` is the workspace the user pointed at (sidebar selection
    /// or the right-clicked row), not "whatever is selected at submit time".
    pub(crate) fn open_worktree_create(
        &mut self,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace(&workspace_id).is_none() {
            self.notify_need_workspace(cx);
            return;
        }
        self.clear_worktree_create_fields(cx);
        self.set_overlay(
            Overlay::WorktreeCreate {
                workspace_id,
                advanced: false,
            },
            cx,
        );
        self.worktree_label_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    pub(crate) fn toggle_worktree_create_advanced(&mut self, cx: &mut Context<Self>) {
        let Overlay::WorktreeCreate { advanced, .. } = &mut self.overlay else {
            return;
        };
        *advanced = !*advanced;
        cx.notify();
    }

    pub(crate) fn submit_worktree_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::WorktreeCreate { workspace_id, .. } = &self.overlay else {
            return;
        };
        let Some(workspace) = self.workspace(workspace_id).cloned() else {
            self.notify_need_workspace(cx);
            return;
        };
        let label = self.worktree_label_input.read(cx).content();
        let branch = self.worktree_branch_input.read(cx).content();
        let base = self.worktree_base_input.read(cx).content();
        let path = self.worktree_path_input.read(cx).content();
        let params = worktree_create_params(
            &workspace,
            label.as_ref(),
            branch.as_ref(),
            base.as_ref(),
            path.as_ref(),
        );
        self.set_overlay(Overlay::None, cx);
        self.focus.focus(window, cx);
        self.invoke_with_response("worktree.create", params, Self::follow_created_tab, cx);
    }

    pub(crate) fn open_worktree_picker(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace(&workspace_id).cloned() else {
            self.notify_need_workspace(cx);
            return;
        };
        let Some(owner) = self.current_session_key() else {
            self.notify_need_workspace(cx);
            return;
        };
        let params = Value::Object(worktree_repo_params(&workspace));
        self.set_overlay(
            Overlay::WorktreeOpen(WorktreeOpenState::Loading {
                owner: owner.clone(),
                workspace_id: workspace_id.clone(),
            }),
            cx,
        );
        self.fetch_worktree_list(owner, workspace_id, params, cx);
    }

    pub(crate) fn open_listed_worktree(&mut self, path: String, cx: &mut Context<Self>) {
        let Overlay::WorktreeOpen(WorktreeOpenState::Ready { source, .. }) = &self.overlay else {
            return;
        };
        let params = worktree_open_params(source, &path);
        self.set_overlay(Overlay::None, cx);
        self.invoke_with_response("worktree.open", params, Self::follow_created_tab, cx);
    }

    pub(crate) fn request_remove_worktree(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        let Some(label) = self.workspace_label(&workspace_id) else {
            return;
        };
        self.set_overlay(
            Overlay::ConfirmRemoveWorktree {
                workspace_id,
                label,
                prompt: RemoveWorktreePrompt::Safe,
            },
            cx,
        );
    }

    pub(crate) fn cancel_remove_worktree(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ConfirmRemoveWorktree { .. }) {
            self.set_overlay(Overlay::None, cx);
        }
    }

    pub(crate) fn confirm_remove_worktree(&mut self, cx: &mut Context<Self>) {
        let Overlay::ConfirmRemoveWorktree {
            workspace_id,
            prompt,
            ..
        } = &self.overlay
        else {
            return;
        };
        let params = worktree_remove_params(
            workspace_id,
            matches!(prompt, RemoveWorktreePrompt::Force { .. }),
        );
        self.set_overlay(Overlay::None, cx);
        self.invoke("worktree.remove", params, cx);
    }

    pub(crate) fn close_worktree_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.overlay,
            Overlay::WorktreeCreate { .. } | Overlay::WorktreeOpen(_)
        ) {
            self.set_overlay(Overlay::None, cx);
        }
        self.focus.focus(window, cx);
    }

    pub(super) fn source_workspace_id(&self) -> Option<String> {
        self.source_workspace()
            .map(|workspace| workspace.workspace_id.clone())
    }

    pub(super) fn source_workspace(&self) -> Option<&WorkspaceInfo> {
        let snapshot = self.snapshot.as_ref()?;
        let id = self
            .selection
            .workspace_id
            .as_deref()
            .or(snapshot.focused_workspace_id.as_deref())?;
        self.workspace(id)
    }

    pub(super) fn workspace(&self, workspace_id: &str) -> Option<&WorkspaceInfo> {
        self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
        })
    }

    pub(super) fn current_session_key(&self) -> Option<SessionKey> {
        Some(SessionKey {
            profile_id: self.current_profile().id().to_owned(),
            session_name: self.current_session()?.name.clone(),
        })
    }

    pub(super) fn workspace_label(&self, workspace_id: &str) -> Option<String> {
        self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
                .map(|workspace| workspace.label.clone())
        })
    }

    pub(super) fn notify_need_workspace(&mut self, cx: &mut Context<Self>) {
        self.post_notice(
            FailureNotice {
                level: ochub_ui::notifications::NotificationLevel::Warning,
                title: self.i18n.text(k::WORKTREE_NEW).to_owned(),
                message: self.i18n.text(k::WORKTREE_NEED_WORKSPACE).to_owned(),
            },
            cx,
        );
    }

    pub(super) fn clear_worktree_create_fields(&mut self, cx: &mut Context<Self>) {
        for input in [
            &self.worktree_label_input,
            &self.worktree_branch_input,
            &self.worktree_base_input,
            &self.worktree_path_input,
        ] {
            input.update(cx, |input, cx| input.set_content("", cx));
        }
    }

    pub(super) fn fetch_worktree_list(
        &mut self,
        owner: SessionKey,
        workspace_id: String,
        params: Value,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = &self.connection else {
            self.abandon_worktree_list();
            return;
        };
        let socket = connection.socket_path().to_owned();
        self.worktree_list_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "worktree.list", params) })
                .await;
            this.update(cx, |this, cx| {
                this.worktree_list_task = None;
                if !worktree_list_applies(
                    &this.overlay,
                    this.current_session_key().as_ref(),
                    &workspace_id,
                    &owner,
                    this.snapshot.as_ref(),
                ) {
                    return;
                }
                this.overlay = Overlay::WorktreeOpen(match result {
                    Ok(value) => match serde_json::from_value::<WorktreeList>(value) {
                        Ok(list) => WorktreeOpenState::Ready {
                            source: list.source,
                            worktrees: list
                                .worktrees
                                .into_iter()
                                .filter(WorktreeInfo::is_openable)
                                .collect(),
                        },
                        Err(error) => WorktreeOpenState::Failed {
                            error: error.to_string(),
                        },
                    },
                    Err(error) => WorktreeOpenState::Failed {
                        error: error.to_string(),
                    },
                });
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn maybe_offer_force_remove_worktree(
        &mut self,
        params: &Value,
        error: &HerdrError,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_id) = dirty_worktree_remove_offer("worktree.remove", params, error)
        else {
            return;
        };
        let Some(label) = self.workspace_label(&workspace_id) else {
            return;
        };
        self.set_overlay(
            Overlay::ConfirmRemoveWorktree {
                workspace_id,
                label,
                prompt: RemoveWorktreePrompt::Force {
                    error: error.to_string(),
                },
            },
            cx,
        );
    }
}
