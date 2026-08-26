use super::*;

impl OcHerdrView {
    pub(super) fn listen_events(mut events: EventSubscription, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let batch = events.next_batch().await;
                let keep = this
                    .update(cx, |this, cx| this.apply_event_batch(batch, cx))
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
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
        mut events: EventSubscription,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let batch = events.next_batch().await;
                let keep = this
                    .update(cx, |this, cx| this.apply_agent_status_batch(batch, cx))
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
    }

    pub(super) fn apply_agent_status_batch(
        &mut self,
        batch: Option<Vec<std::result::Result<HerdrEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let panel_refresh = agent_panel_refresh_from_batch(&self.overlay, batch.as_deref());
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
                            Some(Self::listen_agent_status(subscription, cx));
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
