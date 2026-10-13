use super::terminal::HERDR_NOTIFICATION_ID;
use super::*;
use ocherdr_herdr::ApiClient;
use std::sync::atomic::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentSystemNotification {
    pub(super) status: AgentStatus,
    pub(super) agent: String,
    pub(super) workspace: String,
    pub(super) tab: String,
    pub(super) pane_id: String,
    pub(super) terminal_id: String,
}

fn fetch_snapshot(client: &ApiClient) -> std::result::Result<HierarchySnapshot, HerdrError> {
    let result = client.request("session.snapshot", json!({}))?;
    let snapshot = result
        .get("snapshot")
        .cloned()
        .ok_or_else(|| HerdrError::Protocol("snapshot result is missing `snapshot`".into()))?;
    Ok(serde_json::from_value(snapshot)?)
}

impl OcHerdrView {
    /// Pull a fresh `session.snapshot` for a parked host into
    /// `ParkedHostRuntime::snapshot`. Single-flight per host, with one queued
    /// re-request when endpoint pushes keep arriving during an in-flight
    /// refresh.
    pub(super) fn refresh_parked_snapshot(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        let Some(runtime) = self.parked_hosts.get_mut(profile_id) else {
            return;
        };
        if runtime.snapshot_refreshing {
            runtime.snapshot_refresh_pending = true;
            return;
        }
        runtime.snapshot_refreshing = true;
        runtime.snapshot_refresh_pending = false;
        let client = runtime.connection.api_client();
        let profile_id = profile_id.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { fetch_snapshot(&client) })
                .await;
            this.update(cx, |this, cx| {
                let Some(runtime) = this.parked_hosts.get_mut(&profile_id) else {
                    return;
                };
                runtime.snapshot_refreshing = false;
                if let Ok(snapshot) = result {
                    runtime.selection.reconcile(&snapshot);
                    runtime.snapshot = Some(snapshot);
                    cx.notify();
                }
                if runtime.snapshot_refresh_pending {
                    this.refresh_parked_snapshot(&profile_id, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Snapshot pull for every parked host; used when the aggregate sidebar
    /// turns on so rows from machines parked a while ago are not stale.
    pub(super) fn refresh_all_parked_snapshots(&mut self, cx: &mut Context<Self>) {
        for profile_id in self.parked_hosts.keys().cloned().collect::<Vec<_>>() {
            self.refresh_parked_snapshot(&profile_id, cx);
        }
    }

    pub(super) fn post_agent_system_notification(
        &mut self,
        owner: &SessionKey,
        host: &str,
        notice: AgentSystemNotification,
        cx: &mut Context<Self>,
    ) {
        let title = match notice.status {
            AgentStatus::Done => tf!(self.i18n, k::AGENT_NOTIFICATION_DONE, agent = notice.agent),
            AgentStatus::Blocked => tf!(
                self.i18n,
                k::AGENT_NOTIFICATION_BLOCKED,
                agent = notice.agent
            ),
            _ => return,
        };
        let body = tf!(
            self.i18n,
            k::AGENT_NOTIFICATION_LOCATION,
            host = host,
            workspace = notice.workspace,
            tab = notice.tab
        );
        let id = HERDR_NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed);
        let tag = format!("ocherdr-agent-{id}");
        self.remember_agent_notification_target(
            tag.clone(),
            AgentNotificationTarget {
                profile_id: owner.profile_id.clone(),
                session_name: owner.session_name.clone(),
                pane_id: notice.pane_id,
                terminal_id: notice.terminal_id,
            },
        );
        cx.show_system_notification(SystemNotification {
            tag: tag.into(),
            title: title.into(),
            body: body.into(),
            actions: Vec::new(),
        });
    }

    pub(crate) fn handle_system_notification_response(
        &mut self,
        response: SystemNotificationResponse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self
            .agent_notification_targets
            .remove(response.tag.as_ref())
        else {
            return;
        };
        let Some(profile_index) = self
            .profiles
            .iter()
            .position(|profile| profile.id() == target.profile_id)
        else {
            return;
        };
        if profile_index != self.profile_index {
            self.select_profile(profile_index, cx);
        }
        if self.current_session().map(|session| session.name.as_str())
            != Some(target.session_name.as_str())
        {
            return;
        }
        let pane_id = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .panes
                .iter()
                .find(|pane| pane.terminal_id == target.terminal_id)
                .or_else(|| snapshot.pane(&target.pane_id))
                .map(|pane| pane.pane_id.clone())
        });
        let Some(pane_id) = pane_id else {
            return;
        };
        self.set_overlay(Overlay::None, cx);
        self.select_pane(pane_id.clone(), window, cx);
        self.invoke("pane.focus", json!({ "pane_id": pane_id }), cx);
    }

    /// Applies an authoritative hierarchy after an endpoint invalidation or reconnect.
    pub(crate) fn apply_session_snapshot(
        &mut self,
        snapshot: HierarchySnapshot,
        cx: &mut Context<Self>,
    ) {
        let old_tab = self.selection.tab_id.clone();
        let old_selected = self.selection.pane_id.clone();
        let old_panes = self
            .snapshot
            .as_ref()
            .map(snapshot_pane_ids)
            .unwrap_or_default();
        let panel_pane = agent_panel_pane(&self.overlay).map(str::to_owned);
        let old_panel_status = panel_pane
            .as_deref()
            .and_then(|pane_id| self.snapshot.as_ref().and_then(|s| s.pane(pane_id)))
            .map(|pane| pane.agent_status);
        self.snapshot = Some(snapshot);
        self.restore_parked_recovery(cx);
        self.reconcile_split_drag(cx);
        self.reconcile_split_commit(cx);
        self.reconcile_reorder_drag(cx);
        self.reconcile_pane_drag(cx);
        self.reconcile_keyboard_pane_move(cx);
        self.reconcile_pane_relocations(cx);
        // A fresh authoritative snapshot settles the reorder
        // projection: either it carries the requested order or
        // it conflicts and wins.
        self.pending_reorder = None;
        if worktree_open_target_is_missing(&self.overlay, self.snapshot.as_ref()) {
            self.abandon_worktree_list();
        }
        if let Some(snapshot) = &self.snapshot {
            self.selection.reconcile(snapshot);
            let closed_stream = self.session_panes.as_ref().is_some_and(|session| {
                session
                    .panes
                    .values()
                    .any(|runtime| runtime.exit_seen || runtime.endpoint.is_closed())
            });
            if session_terminals_need_rebuild(
                old_tab.as_deref(),
                old_selected.as_deref(),
                &old_panes,
                &self.selection,
                snapshot,
                closed_stream,
            ) {
                self.ensure_session_terminals(cx);
            }
            self.settle_pending_created_tab(cx);
        }
        // The endpoint pushes a snapshot on every agent status
        // change; a status flip on the panel's pane is the cue
        // to re-read its output.
        let panel_refresh =
            agent_panel_refresh_needed(&self.overlay, old_panel_status, self.snapshot.as_ref());
        self.reconcile_open_agent_panel(panel_refresh, cx);
        cx.notify();
    }

    /// Pull the authoritative `session.snapshot` after an invalidation: an
    /// endpoint snapshot push, a command whose result Herdr does not push, or
    /// a reconnect. Single-flight with one queued re-request.
    pub(crate) fn resync_snapshot(&mut self, epoch: u64, cx: &mut Context<Self>) {
        if snapshot_refresh_should_queue(self.snapshot_refreshing) {
            self.snapshot_refresh_pending = true;
            return;
        }
        let Some(client) = self.connection.as_ref().map(SessionConnection::api_client) else {
            return;
        };
        self.snapshot_refreshing = true;
        self.snapshot_refresh_pending = false;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { fetch_snapshot(&client) })
                .await;
            this.update(cx, |this, cx| {
                if this.event_epoch != epoch {
                    return;
                }
                this.snapshot_refreshing = false;
                match result {
                    Ok(snapshot) => this.apply_session_snapshot(snapshot, cx),
                    Err(error) => {
                        this.notify_failure(FailureKind::RefreshSnapshot, error, cx);
                        cx.notify();
                    }
                }
                if this.snapshot_refresh_pending {
                    this.resync_snapshot(epoch, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}
