use super::terminal::HERDR_NOTIFICATION_ID;
use super::*;
use std::sync::atomic::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentSystemNotification {
    status: AgentStatus,
    agent: String,
    workspace: String,
    tab: String,
    pane_id: String,
    terminal_id: String,
}

pub(super) fn agent_system_notifications(
    snapshot: &HierarchySnapshot,
    items: &[std::result::Result<HerdrEvent, HerdrError>],
    suppressed_pane: Option<&str>,
) -> Vec<AgentSystemNotification> {
    let mut statuses = snapshot
        .panes
        .iter()
        .map(|pane| (pane.pane_id.clone(), pane.agent_status))
        .collect::<HashMap<_, _>>();
    let mut notices = Vec::new();
    for item in items {
        let event = match item {
            Ok(event) => event,
            Err(error) if error.is_event_payload_error() => continue,
            Err(_) => break,
        };
        let HerdrEvent::PaneAgentStatusChanged {
            pane_id,
            agent_status,
            agent,
            display_agent,
            ..
        } = event
        else {
            continue;
        };
        let Some(pane) = snapshot.panes.iter().find(|pane| pane.pane_id == *pane_id) else {
            continue;
        };
        // Match HierarchySnapshot::apply: a status from a previous agent on
        // the same pane is stale and must never trigger a notification.
        if agent.as_deref() != pane.agent.as_deref() {
            continue;
        }
        let previous = statuses.insert(pane_id.clone(), *agent_status);
        if previous != Some(AgentStatus::Working)
            || !matches!(agent_status, AgentStatus::Done | AgentStatus::Blocked)
            || suppressed_pane == Some(pane_id.as_str())
        {
            continue;
        }
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == pane.workspace_id)
            .map(|workspace| workspace.label.as_str())
            .filter(|label| !label.is_empty())
            .unwrap_or(&pane.workspace_id)
            .to_owned();
        let tab = snapshot
            .tabs
            .iter()
            .find(|tab| tab.tab_id == pane.tab_id)
            .map(|tab| tab.label.as_str())
            .filter(|label| !label.is_empty())
            .unwrap_or(&pane.tab_id)
            .to_owned();
        let agent = display_agent
            .as_deref()
            .filter(|label| !label.is_empty())
            .or_else(|| {
                let label = pane.display_name();
                (!label.is_empty()).then_some(label)
            })
            .or(agent.as_deref())
            .unwrap_or(&pane.pane_id)
            .to_owned();
        notices.push(AgentSystemNotification {
            status: *agent_status,
            agent,
            workspace,
            tab,
            pane_id: pane.pane_id.clone(),
            terminal_id: pane.terminal_id.clone(),
        });
    }
    notices
}

impl OcHerdrView {
    pub(super) fn listen_events(
        owner: SessionKey,
        mut events: EventSubscription,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let batch = events.next_batch().await;
                let keep = this
                    .update(cx, |this, cx| this.apply_event_batch_for(&owner, batch, cx))
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
    }

    fn apply_event_batch_for(
        &mut self,
        owner: &SessionKey,
        batch: Option<Vec<std::result::Result<HerdrEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_active_session(owner) {
            return self.apply_event_batch(batch, cx);
        }
        let Some(runtime) = self.parked_hosts.get_mut(&owner.profile_id) else {
            return false;
        };
        match batch {
            None => {
                runtime.event_stream = EventStreamState::Lost(
                    HerdrError::EventStreamClosed("event worker stopped".into())
                        .to_string()
                        .into(),
                );
                cx.notify();
                false
            }
            Some(items) => {
                if let Some(error) = items.into_iter().find_map(|item| match item {
                    Err(error) if !error.is_event_payload_error() => Some(error),
                    _ => None,
                }) {
                    runtime.event_stream = EventStreamState::Lost(error.to_string().into());
                    cx.notify();
                    return false;
                }
                true
            }
        }
    }

    pub(super) fn schedule_startup_replay_quiet(&mut self, cx: &mut Context<Self>) {
        self.startup_replay_serial = self.startup_replay_serial.wrapping_add(1);
        let serial = self.startup_replay_serial;
        let epoch = self.event_epoch;
        self.startup_replay_sync = Some(StartupReplaySync::Draining { serial });
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(STARTUP_REPLAY_QUIET_DELAY)
                .await;
            this.update(cx, |this, cx| {
                if this.event_epoch != epoch
                    || this.startup_replay_sync != Some(StartupReplaySync::Draining { serial })
                {
                    return;
                }
                this.startup_replay_sync = Some(StartupReplaySync::Refreshing);
                this.resync_snapshot(epoch, cx);
            })
            .ok();
        })
        .detach();
    }

    /// During retained-history replay, payload order is not authoritative for
    /// the current UI. Treat every batch as an invalidation and immediately
    /// fetch Herdr's current snapshot. Snapshot refresh is single-flight, so a
    /// long replay becomes at most one active request plus one pending refresh.
    pub(super) fn refresh_during_startup_replay(
        &mut self,
        items: Vec<std::result::Result<HerdrEvent, HerdrError>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut payload_error = None;
        for item in items {
            match item {
                Ok(_) => {}
                Err(error) if error.is_event_payload_error() => payload_error = Some(error),
                Err(error) => {
                    self.startup_replay_sync = None;
                    self.event_stream = EventStreamState::Lost(error.to_string().into());
                    self.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
                    cx.notify();
                    return false;
                }
            }
        }
        if let Some(error) = payload_error {
            self.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
        }
        match self.startup_replay_sync {
            Some(StartupReplaySync::Draining { .. }) => {
                self.resync_snapshot(self.event_epoch, cx);
                self.schedule_startup_replay_quiet(cx);
            }
            Some(StartupReplaySync::Refreshing) => {
                self.snapshot_refresh_pending = true;
            }
            None => unreachable!("startup replay state disappeared"),
        }
        true
    }

    pub(super) fn apply_event_batch(
        &mut self,
        batch: Option<Vec<std::result::Result<HerdrEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.startup_replay_sync.is_some() {
            match batch {
                Some(items) => return self.refresh_during_startup_replay(items, cx),
                None => self.startup_replay_sync = None,
            }
        }
        let old_tab = self.selection.tab_id.clone();
        let old_selected = self.selection.pane_id.clone();
        let old_panes = self
            .snapshot
            .as_ref()
            .map(snapshot_pane_ids)
            .unwrap_or_default();
        let panel_refresh = agent_panel_refresh_from_batch(&self.overlay, batch.as_deref());
        let action = match batch {
            None => EventPollAction::Disconnect(
                HerdrError::EventStreamClosed("event worker stopped".into())
                    .to_string()
                    .into(),
            ),
            Some(items) => {
                let Some(snapshot) = self.snapshot.as_mut() else {
                    self.abandon_worktree_list();
                    self.reconcile_open_agent_panel(false, cx);
                    return false;
                };
                let mut items = items.into_iter();
                let action =
                    apply_event_stream(snapshot, &mut self.selection, || match items.next() {
                        Some(Ok(event)) => Ok(Some(event)),
                        Some(Err(err)) => Err(err),
                        None => Ok(None),
                    });
                self.pin_relocation_selection();
                action
            }
        };
        let effects = effects_for(&action);
        if effects.abandon_worktree_list {
            self.abandon_worktree_list();
        }
        if worktree_open_target_is_missing(&self.overlay, self.snapshot.as_ref()) {
            self.abandon_worktree_list();
        }
        if let Some(error) = effects.error {
            self.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
        }
        if effects.resync {
            self.resync_snapshot(self.event_epoch, cx);
        }
        if effects.apply_local
            && let Some(snapshot) = &self.snapshot
        {
            let closed_stream = self.session_panes.as_ref().is_some_and(|session| {
                session
                    .panes
                    .values()
                    .any(|runtime| runtime.exit_seen || runtime.session.is_closed())
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
        if effects.settle_reorder {
            self.pending_reorder = None;
        }
        if effects.notify {
            cx.notify();
        }
        if matches!(action, EventPollAction::Disconnect(_)) {
            self.cancel_split_drag();
            self.split_commit = None;
            self.cancel_reorder_drag();
            self.cancel_pane_drag();
            self.cancel_keyboard_pane_move();
            self.pending_created_tab = None;
            // Requests already sent cannot be cancelled; the reconnect
            // snapshot is the authority for what actually happened.
            self.abort_pane_relocations_for_disconnect();
            self.abort_tab_transfer_for_disconnect(cx);
        }
        self.reconcile_split_drag(cx);
        self.reconcile_split_commit(cx);
        self.reconcile_reorder_drag(cx);
        self.reconcile_pane_drag(cx);
        self.reconcile_keyboard_pane_move(cx);
        self.reconcile_pane_relocations(cx);
        if let Some(stream) = action.event_stream() {
            self.event_stream = stream;
            cx.notify();
        }
        self.ensure_agent_status_stream(cx);
        self.reconcile_open_agent_panel(panel_refresh, cx);
        effects.reschedule
    }

    pub(super) fn listen_agent_status(
        owner: SessionKey,
        mut events: EventSubscription,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let batch = events.next_batch().await;
                let keep = this
                    .update(cx, |this, cx| {
                        this.apply_agent_status_batch_for(&owner, batch, cx)
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
    }

    pub(super) fn apply_agent_status_batch_for(
        &mut self,
        owner: &SessionKey,
        batch: Option<Vec<std::result::Result<HerdrEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.is_active_session(owner) {
            return self.apply_agent_status_batch(batch, cx);
        }
        let enabled = self.agent_notifications;
        let profile_label = self
            .profiles
            .iter()
            .find(|profile| profile.id() == owner.profile_id)
            .map(ConnectionProfile::label)
            .unwrap_or(&owner.profile_id)
            .to_owned();
        let Some(runtime) = self.parked_hosts.get_mut(&owner.profile_id) else {
            return false;
        };
        let Some(items) = batch else {
            runtime.agent_status_panes = agent_status_panes_after_stream_closed();
            return false;
        };
        if let Some(handoff) = runtime.agent_status_handoff.as_mut() {
            let mut events = Vec::new();
            for item in items {
                match item {
                    Ok(event) => events.push(event),
                    Err(error) if error.is_event_payload_error() => handoff.note_payload_error(),
                    Err(_) => {
                        runtime.agent_status_panes = agent_status_panes_after_stream_closed();
                        return false;
                    }
                }
            }
            handoff.push(events, AGENT_STATUS_HANDOFF_LIMIT);
            return true;
        }
        let notices = if enabled {
            runtime
                .snapshot
                .as_ref()
                .map(|snapshot| agent_system_notifications(snapshot, &items, None))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let Some(snapshot) = runtime.snapshot.as_mut() else {
            return false;
        };
        let mut items = items.into_iter();
        let action = apply_event_stream(snapshot, &mut runtime.selection, || match items.next() {
            Some(Ok(event)) => Ok(Some(event)),
            Some(Err(error)) => Err(error),
            None => Ok(None),
        });
        let effects = effects_for(&action);
        if matches!(action, EventPollAction::Disconnect(_)) || effects.resync {
            runtime.agent_status_panes = agent_status_panes_after_stream_closed();
            return false;
        }
        if effects.apply_local {
            for notice in notices {
                self.post_agent_system_notification(owner, &profile_label, notice, cx);
            }
        }
        effects.reschedule
    }

    pub(super) fn apply_agent_status_batch(
        &mut self,
        batch: Option<Vec<std::result::Result<HerdrEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let panel_refresh = agent_panel_refresh_from_batch(&self.overlay, batch.as_deref());
        let notices = if self.agent_notifications && self.agent_status_handoff.is_none() {
            let suppressed = self
                .window_active
                .then_some(self.selection.pane_id.as_deref())
                .flatten();
            self.snapshot
                .as_ref()
                .zip(batch.as_deref())
                .map(|(snapshot, items)| agent_system_notifications(snapshot, items, suppressed))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let action = match batch {
            None => {
                self.agent_status_panes = agent_status_panes_after_stream_closed();
                return false;
            }
            Some(items) => {
                if let Some(handoff) = self.agent_status_handoff.as_mut() {
                    let mut fatal = None;
                    let mut payload_error = None;
                    let mut events = Vec::new();
                    for item in items {
                        match item {
                            Ok(event) => events.push(event),
                            Err(err) if err.is_event_payload_error() => {
                                handoff.note_payload_error();
                                payload_error = Some(err);
                            }
                            Err(err) => {
                                fatal = Some(err);
                                break;
                            }
                        }
                    }
                    handoff.push(events, AGENT_STATUS_HANDOFF_LIMIT);
                    if let Some(error) = payload_error {
                        self.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
                    }
                    if let Some(error) = fatal {
                        self.agent_status_panes = agent_status_panes_after_stream_closed();
                        self.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
                        return false;
                    }
                    return true;
                }
                let Some(snapshot) = self.snapshot.as_mut() else {
                    self.reconcile_open_agent_panel(false, cx);
                    return false;
                };
                let mut items = items.into_iter();
                apply_event_stream(snapshot, &mut self.selection, || match items.next() {
                    Some(Ok(event)) => Ok(Some(event)),
                    Some(Err(err)) => Err(err),
                    None => Ok(None),
                })
            }
        };
        if let EventPollAction::Disconnect(detail) = &action {
            self.agent_status_panes = agent_status_panes_after_stream_closed();
            self.notify_failure(FailureKind::ApplyLiveUpdate, detail, cx);
            return false;
        }
        let effects = effects_for(&action);
        if let Some(error) = effects.error {
            self.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
        }
        if effects.resync {
            self.resync_snapshot(self.event_epoch, cx);
        }
        if effects.notify {
            cx.notify();
        }
        if effects.apply_local {
            let host = self.current_profile().label().to_owned();
            if let Some(owner) = self.current_session_key() {
                for notice in notices {
                    self.post_agent_system_notification(&owner, &host, notice, cx);
                }
            }
        }
        self.reconcile_open_agent_panel(panel_refresh, cx);
        effects.reschedule
    }

    pub(super) fn ensure_agent_status_stream(&mut self, cx: &mut Context<Self>) {
        let panes = {
            let (Some(snapshot), Some(_)) = (&self.snapshot, &self.connection) else {
                self.agent_status_listen = None;
                self.agent_status_rebuild = None;
                self.agent_status_panes.clear();
                self.agent_status_handoff = None;
                return;
            };
            if !agent_status_stream_should_rebuild(&self.agent_status_panes, snapshot) {
                return;
            }
            snapshot_pane_ids(snapshot)
        };
        self.rebuild_agent_status_stream(panes, cx);
    }

    pub(super) fn release_agent_status_handoff(&mut self, cx: &mut Context<Self>) {
        let Some(handoff) = self.agent_status_handoff.take() else {
            return;
        };
        let (events, resync_after) = handoff.into_release();
        let panel_refresh = agent_panel_pane(&self.overlay).is_some_and(|pane_id| {
            events
                .iter()
                .any(|event| agent_output_should_refresh(pane_id, event))
        });
        let mut resync = resync_after;
        if !events.is_empty()
            && let Some(snapshot) = self.snapshot.as_mut()
        {
            let mut events = events.into_iter();
            let action = apply_event_stream(snapshot, &mut self.selection, || Ok(events.next()));
            self.pin_relocation_selection();
            let effects = effects_for(&action);
            if let Some(error) = effects.error {
                self.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
            }
            if effects.notify {
                cx.notify();
            }
            resync |= effects.resync;
        }
        self.reconcile_split_drag(cx);
        self.reconcile_reorder_drag(cx);
        self.reconcile_pane_drag(cx);
        self.reconcile_keyboard_pane_move(cx);
        self.reconcile_pane_relocations(cx);
        if resync {
            self.resync_snapshot(self.event_epoch, cx);
        }
        self.reconcile_open_agent_panel(panel_refresh, cx);
    }

    pub(super) fn rebuild_agent_status_stream(
        &mut self,
        panes: HashSet<String>,
        cx: &mut Context<Self>,
    ) {
        let previous = self.agent_status_panes.clone();
        self.agent_status_rebuild = None;
        self.agent_status_panes.clone_from(&panes);
        if panes.is_empty() {
            self.agent_status_listen = None;
            return;
        }
        let Some(connection) = &self.connection else {
            self.agent_status_panes = previous;
            return;
        };
        let Some(owner) = self.current_session_key() else {
            self.agent_status_panes = previous;
            return;
        };
        let socket = connection.socket_path().to_owned();
        let epoch = self.event_epoch;
        let pane_list: Vec<String> = panes.iter().cloned().collect();
        self.agent_status_rebuild = Some(cx.spawn(async move |this, cx| {
            let subscribed = cx
                .background_spawn(async move { subscribe_agent_status(&socket, &pane_list) })
                .await;
            this.update(cx, |this, cx| {
                if this.event_epoch != epoch {
                    return;
                }
                match subscribed {
                    Ok(subscription) => {
                        if this.agent_status_panes != panes {
                            return;
                        }
                        if this.agent_status_handoff.is_none() {
                            this.agent_status_handoff = Some(AgentStatusHandoff::new());
                        }
                        this.agent_status_listen =
                            Some(Self::listen_agent_status(owner, subscription, cx));
                        this.resync_snapshot(epoch, cx);
                    }
                    Err(error) => match agent_status_subscribe_failure_action(&error) {
                        AgentStatusSubscribeFailureAction::Resync => {
                            this.resync_snapshot(epoch, cx);
                        }
                        AgentStatusSubscribeFailureAction::Report => {
                            this.agent_status_panes = event_panes_after_failed_subscribe(
                                &this.agent_status_panes,
                                &panes,
                                &previous,
                            );
                            this.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
                        }
                    },
                }
            })
            .ok();
        }));
    }

    fn post_agent_system_notification(
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

    pub(crate) fn resync_snapshot(&mut self, epoch: u64, cx: &mut Context<Self>) {
        if snapshot_refresh_should_queue(self.snapshot_refreshing) {
            self.snapshot_refresh_pending = true;
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        self.snapshot_refreshing = true;
        self.snapshot_refresh_pending = false;
        let socket = connection.socket_path().to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let result = request_socket(&socket, "session.snapshot", json!({}))?;
                    let snapshot = result.get("snapshot").cloned().ok_or_else(|| {
                        HerdrError::Protocol("snapshot result is missing `snapshot`".into())
                    })?;
                    Ok::<HierarchySnapshot, HerdrError>(serde_json::from_value(snapshot)?)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.event_epoch != epoch {
                    return;
                }
                this.snapshot_refreshing = false;
                match result {
                    Ok(snapshot) => {
                        let old_tab = this.selection.tab_id.clone();
                        let old_selected = this.selection.pane_id.clone();
                        let old_panes = this
                            .snapshot
                            .as_ref()
                            .map(snapshot_pane_ids)
                            .unwrap_or_default();
                        this.herdr_capabilities = HerdrCapabilities::from_snapshot(&snapshot);
                        this.snapshot = Some(snapshot);
                        this.restore_parked_recovery(cx);
                        this.reconcile_split_drag(cx);
                        this.reconcile_reorder_drag(cx);
                        this.reconcile_pane_drag(cx);
                        this.reconcile_keyboard_pane_move(cx);
                        this.reconcile_pane_relocations(cx);
                        if worktree_open_target_is_missing(&this.overlay, this.snapshot.as_ref()) {
                            this.abandon_worktree_list();
                        }
                        if let Some(snapshot) = &this.snapshot {
                            this.selection.reconcile(snapshot);
                            let closed_stream =
                                this.session_panes.as_ref().is_some_and(|session| {
                                    session.panes.values().any(|runtime| {
                                        runtime.exit_seen || runtime.session.is_closed()
                                    })
                                });
                            if session_terminals_need_rebuild(
                                old_tab.as_deref(),
                                old_selected.as_deref(),
                                &old_panes,
                                &this.selection,
                                snapshot,
                                closed_stream,
                            ) {
                                this.ensure_session_terminals(cx);
                            }
                            this.settle_pending_created_tab(cx);
                        }
                        cx.notify();
                    }
                    Err(error) => {
                        this.notify_failure(FailureKind::RefreshSnapshot, error, cx);
                        cx.notify();
                    }
                }
                if this.snapshot_refresh_pending {
                    this.resync_snapshot(epoch, cx);
                } else if this.startup_replay_sync == Some(StartupReplaySync::Refreshing) {
                    this.startup_replay_sync = None;
                }
                if snapshot_handoff_should_release(this.snapshot_refreshing) {
                    this.release_agent_status_handoff(cx);
                }
                this.ensure_agent_status_stream(cx);
            })
            .ok();
        })
        .detach();
    }
}
