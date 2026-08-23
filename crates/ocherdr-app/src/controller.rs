use std::collections::HashSet;
use std::task::Poll;

use futures::channel::mpsc::UnboundedReceiver;
use futures::future::{self, Either, poll_fn};
use futures::pin_mut;
use ocherdr_core::{
    AGENT_STATUS_HANDOFF_LIMIT, AgentStatusHandoff, agent_status_panes_after_stream_closed,
    agent_status_stream_should_rebuild, event_panes_after_failed_subscribe,
};
use ocherdr_herdr::{TerminalFrame, next_batch, subscribe_agent_status};

use ochub_ui::notifications::NotificationHost;

use super::*;
use crate::notify::{FailureKind, FailureNotice, command_notification, notification_for};

impl OcHerdrView {
    pub(super) fn new(settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let i18n = I18n::new(settings.language);
        let appearance = settings.appearance.clone();
        let focus = cx.focus_handle();
        let host_center = cx.new(|cx| HostCenter::new(settings, i18n, focus.clone(), cx));
        let profiles = host_center.read(cx).profiles().to_vec();
        cx.subscribe(&host_center, |this, _center, event, cx| {
            this.handle_host_center_event(event.clone(), cx);
        })
        .detach();
        cx.observe_window_appearance(window, |this, window, cx| {
            if this.appearance.mode != AppearanceMode::System {
                return;
            }
            install_appearance(&this.appearance, window.appearance());
            theme::apply_window_background(window);
            cx.refresh_windows();
        })
        .detach();
        let mut view = Self {
            profiles,
            profile_index: 0,
            sessions: Vec::new(),
            session_index: None,
            connection: None,
            event_stream: EventStreamState::Idle,
            event_listen: None,
            agent_status_listen: None,
            agent_status_rebuild: None,
            agent_status_panes: HashSet::new(),
            agent_status_handoff: None,
            snapshot: None,
            selection: Selection {
                connection_id: "local".into(),
                ..Default::default()
            },
            operation: None,
            notifications: cx.new(|_| NotificationHost::new()),
            focus,
            load_epoch: 0,
            event_epoch: 0,
            snapshot_refreshing: false,
            snapshot_refresh_pending: false,
            session_panes: None,
            overlay: Overlay::None,
            open_select: None,
            appearance_scroll: ScrollHandle::new(),
            prefix_pending: false,
            text_drag_pane: None,
            ime_marked: None,
            rename_input: cx.new(|cx| TextInput::new(cx, i18n.text(k::COMMON_NAME))),
            appearance,
            i18n,
            host_center,
            pending_persist: None,
            persist_task: None,
        };
        let host = cx.weak_entity();
        bind_enter_submit(&view.rename_input, host, cx, |this, window, cx| {
            this.submit_rename(window, cx);
        });
        view.reload(None, cx);
        if let Some(notice) = missing_theme_notice(&view.appearance.theme_family, view.i18n) {
            view.post_notice(notice, cx);
        }
        view
    }

    pub(super) fn current_profile(&self) -> ConnectionProfile {
        self.profiles[self.profile_index].clone()
    }

    pub(super) fn current_session(&self) -> Option<&SessionSummary> {
        self.session_index
            .and_then(|index| self.sessions.get(index))
    }

    fn notify_failure(
        &mut self,
        kind: FailureKind,
        detail: impl std::fmt::Display,
        cx: &mut Context<Self>,
    ) {
        self.post_notice(notification_for(kind, &detail.to_string(), self.i18n), cx);
    }

    fn notify_command_failure(
        &mut self,
        method: &str,
        detail: impl std::fmt::Display,
        cx: &mut Context<Self>,
    ) {
        self.post_notice(
            command_notification(method, &detail.to_string(), self.i18n),
            cx,
        );
    }

    fn post_notice(&mut self, notice: FailureNotice, cx: &mut Context<Self>) {
        self.notifications.update(cx, |host, cx| {
            host.notify(notice.request(), cx);
        });
    }

    pub(super) fn reload(&mut self, preferred_session: Option<String>, cx: &mut Context<Self>) {
        self.load_epoch = self.load_epoch.wrapping_add(1);
        self.event_epoch = self.event_epoch.wrapping_add(1);
        self.event_listen = None;
        self.event_stream = EventStreamState::Idle;
        self.agent_status_listen = None;
        self.agent_status_rebuild = None;
        self.agent_status_panes.clear();
        self.agent_status_handoff = None;
        self.snapshot_refreshing = false;
        self.snapshot_refresh_pending = false;
        let epoch = self.load_epoch;
        let profile = self.current_profile();
        self.operation = Some(self.i18n.text(k::NOTIFY_DISCOVERING).into());
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let sessions = discover_sessions(&profile)?;
                    let selected = preferred_session
                        .as_deref()
                        .and_then(|name| sessions.iter().position(|session| session.name == name))
                        .or_else(|| sessions.iter().position(|session| session.running));
                    let (connection, events, snapshot) = if let Some(index) = selected {
                        if sessions[index].running {
                            let connection =
                                SessionConnection::connect(&profile, &sessions[index])?;
                            let snapshot = connection.snapshot()?;
                            let events =
                                LoadedEvents::from_subscribe(connection.subscribe_background());
                            (Some(connection), events, Some(snapshot))
                        } else {
                            (None, LoadedEvents::Idle, None)
                        }
                    } else {
                        (None, LoadedEvents::Idle, None)
                    };
                    Ok::<_, HerdrError>(LoadedSession {
                        sessions,
                        selected,
                        connection,
                        events,
                        snapshot,
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                if this.load_epoch != epoch {
                    return;
                }
                this.operation = None;
                match loaded {
                    Ok(loaded) => {
                        this.sessions = loaded.sessions;
                        this.session_index = loaded.selected;
                        this.connection = loaded.connection;
                        this.snapshot = loaded.snapshot;
                        this.selection.connection_id = this.current_profile().id().into();
                        this.selection.session_name =
                            this.current_session().map(|s| s.name.clone());
                        if let Some(snapshot) = &this.snapshot {
                            this.selection.reconcile(snapshot);
                        }
                        this.ensure_session_terminals(cx);
                        match loaded.events {
                            LoadedEvents::Idle => {
                                this.event_stream = EventStreamState::Idle;
                            }
                            LoadedEvents::Lost(detail) => {
                                this.event_stream = EventStreamState::Lost(detail);
                            }
                            LoadedEvents::Live(subscription) => {
                                this.event_stream = EventStreamState::Live;
                                this.event_listen = Some(Self::listen_events(subscription, cx));
                                this.resync_snapshot(this.event_epoch, cx);
                            }
                        }
                        this.ensure_agent_status_stream(cx);
                    }
                    Err(error) => {
                        this.sessions.clear();
                        this.session_index = None;
                        this.connection = None;
                        this.event_stream = EventStreamState::Idle;
                        this.event_listen = None;
                        this.agent_status_listen = None;
                        this.agent_status_rebuild = None;
                        this.agent_status_panes.clear();
                        this.agent_status_handoff = None;
                        this.snapshot = None;
                        this.session_panes = None;
                        this.notify_failure(FailureKind::DiscoverSessions, error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn select_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.profile_index {
            return;
        }
        self.profile_index = index;
        self.host_center
            .update(cx, |center, _| center.set_profile_index(index));
        self.remember_current_host(cx);
        self.sessions.clear();
        self.session_index = None;
        self.connection = None;
        self.event_stream = EventStreamState::Idle;
        self.event_listen = None;
        self.agent_status_listen = None;
        self.agent_status_rebuild = None;
        self.agent_status_panes.clear();
        self.agent_status_handoff = None;
        self.snapshot = None;
        self.session_panes = None;
        self.reload(None, cx);
    }

    fn remember_current_host(&mut self, cx: &mut Context<Self>) {
        if let Some(profile) = self.profiles.get(self.profile_index) {
            let id = profile.id().to_owned();
            self.host_center
                .update(cx, |center, cx| center.remember_host(&id, cx));
        }
    }

    fn persist_settings(&mut self, kind: FailureKind, cx: &mut Context<Self>) {
        self.queue_settings_persist(
            SettingsPersist {
                error: Some(kind),
                host: None,
                rollback: None,
            },
            cx,
        );
    }

    fn queue_settings_persist(&mut self, request: SettingsPersist, cx: &mut Context<Self>) {
        if let Some(request) = enqueue_settings_persist(
            &mut self.pending_persist,
            self.persist_task.is_some(),
            request,
        ) {
            self.spawn_settings_persist(request, cx);
        }
    }

    fn spawn_settings_persist(&mut self, request: SettingsPersist, cx: &mut Context<Self>) {
        let settings = crate::host_center::assemble_settings(
            &self.host_center.read(cx).persist_state(),
            self.appearance.clone(),
            self.i18n.preference(),
        );
        self.persist_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { crate::host_center::write_settings(&settings) })
                .await;
            this.update(cx, |this, cx| {
                this.persist_task = None;
                this.apply_settings_persist_result(request, result, cx);
                if let Some(next) = this.pending_persist.take() {
                    this.spawn_settings_persist(next, cx);
                }
            })
            .ok();
        }));
    }

    fn apply_settings_persist_result(
        &mut self,
        request: SettingsPersist,
        result: std::result::Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let SettingsPersist {
            error,
            host,
            rollback,
        } = request;
        match (result, host) {
            (Ok(()), None) => {}
            (Ok(()), Some(HostPersistFollowUp::Revertible { .. })) => {
                let profiles = self.host_center.read(cx).profiles().to_vec();
                self.adopt_profiles(profiles, cx);
            }
            (Ok(()), Some(HostPersistFollowUp::Saved { index, then })) => {
                self.host_center.update(cx, |center, cx| {
                    center.invalidate_probe_for_saved_host(index, cx);
                });
                let profiles = self.host_center.read(cx).profiles().to_vec();
                self.adopt_profiles(profiles, cx);
                self.set_overlay(Overlay::NodeManager, cx);
                if then == HostSaveThen::Connect {
                    self.request_choose_node(index, cx);
                }
            }
            (Err(detail), host) => {
                let snapshot = persist_failure_rollback(&mut self.pending_persist, rollback);
                let continuing = snapshot.is_none()
                    && self
                        .pending_persist
                        .as_ref()
                        .is_some_and(|pending| pending.host.is_some());
                if let Some(snapshot) = snapshot {
                    self.host_center
                        .update(cx, |center, _| center.apply_rollback(snapshot));
                }
                if continuing {
                    return;
                }
                let kind = match host {
                    Some(HostPersistFollowUp::Revertible { error }) => error,
                    Some(HostPersistFollowUp::Saved { .. }) => FailureKind::SaveHost,
                    None => {
                        let Some(kind) = error else {
                            return;
                        };
                        kind
                    }
                };
                self.notify_failure(kind, detail, cx);
            }
        }
    }

    fn handle_host_center_event(&mut self, event: HostCenterEvent, cx: &mut Context<Self>) {
        match event {
            HostCenterEvent::PersistBestEffort => {
                self.queue_settings_persist(
                    SettingsPersist {
                        error: None,
                        host: None,
                        rollback: None,
                    },
                    cx,
                );
            }
            HostCenterEvent::PersistRevertible { rollback, error } => {
                self.queue_settings_persist(
                    SettingsPersist {
                        error: Some(error),
                        host: Some(HostPersistFollowUp::Revertible { error }),
                        rollback: Some(rollback),
                    },
                    cx,
                );
            }
            HostCenterEvent::HostSaved {
                rollback,
                index,
                then,
            } => {
                self.queue_settings_persist(
                    SettingsPersist {
                        error: Some(FailureKind::SaveHost),
                        host: Some(HostPersistFollowUp::Saved { index, then }),
                        rollback: Some(rollback),
                    },
                    cx,
                );
            }
            HostCenterEvent::CatalogChanged(profiles) => {
                self.adopt_profiles(profiles, cx);
            }
            HostCenterEvent::ProfileSelected(index) => {
                self.request_choose_node(index, cx);
            }
            HostCenterEvent::OpenCreateForm => {
                self.set_overlay(Overlay::RemoteForm(RemoteForm::Create), cx);
            }
            HostCenterEvent::OpenEditForm(index) => {
                self.set_overlay(Overlay::RemoteForm(RemoteForm::Edit(index)), cx);
            }
            HostCenterEvent::DismissForm => {
                self.set_overlay(Overlay::NodeManager, cx);
            }
            HostCenterEvent::ConfirmRemoveProfile(id) => {
                self.set_overlay(Overlay::ConfirmRemoveProfile(id), cx);
            }
            HostCenterEvent::ConfirmBulkRemove => {
                self.set_overlay(Overlay::ConfirmBulkRemove, cx);
            }
            HostCenterEvent::Failed { kind, detail } => {
                self.notify_failure(kind, detail, cx);
            }
            HostCenterEvent::CloseRequested => {
                self.set_overlay(Overlay::None, cx);
            }
        }
    }

    fn adopt_profiles(&mut self, profiles: Vec<ConnectionProfile>, cx: &mut Context<Self>) {
        let current_id = self
            .profiles
            .get(self.profile_index)
            .map(|profile| profile.id().to_owned());
        let lost_current = current_id
            .as_deref()
            .is_some_and(|id| !profiles.iter().any(|profile| profile.id() == id));
        self.profiles = profiles;
        self.profile_index = current_id
            .as_deref()
            .and_then(|id| profile_index_by_id(&self.profiles, id))
            .unwrap_or(0);
        let profile_index = self.profile_index;
        self.host_center
            .update(cx, |center, _| center.set_profile_index(profile_index));
        if lost_current {
            self.reload(None, cx);
        }
        self.dismiss_stale_host_overlay(cx);
    }

    fn dismiss_stale_host_overlay(&mut self, cx: &mut Context<Self>) {
        if confirmed_host_index(&self.overlay, &self.profiles).is_some() {
            return;
        }
        match self.overlay {
            Overlay::ConfirmSwitchProfile { .. } => self.cancel_switch_profile(cx),
            Overlay::ConfirmRemoveProfile(_) => self.cancel_remove_node(cx),
            _ => {}
        }
    }

    fn set_overlay(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        let leaving_host_center = self.overlay.host_center() && !overlay.host_center();
        let form = match overlay {
            Overlay::RemoteForm(form) => Some(form),
            _ => None,
        };
        self.overlay = overlay;
        self.host_center.update(cx, |center, _| {
            center.set_form(form);
            if leaving_host_center {
                center.dismiss();
            }
        });
        cx.notify();
    }

    pub(super) fn pane(&self, pane_id: &str) -> Option<&PaneRuntime> {
        self.session_panes.as_ref()?.panes.get(pane_id)
    }

    fn pane_mut(&mut self, pane_id: &str) -> Option<&mut PaneRuntime> {
        self.session_panes.as_mut()?.panes.get_mut(pane_id)
    }

    fn live_herdr_session(&self) -> bool {
        self.connection.is_some()
            || self.snapshot.is_some()
            || self
                .session_panes
                .as_ref()
                .is_some_and(|session| !session.panes.is_empty())
    }

    fn listen_events(mut events: EventSubscription, cx: &mut Context<Self>) -> Task<()> {
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

    fn apply_event_batch(
        &mut self,
        batch: Option<Vec<std::result::Result<HerdrEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let old_tab = self.selection.tab_id.clone();
        let old_selected = self.selection.pane_id.clone();
        let old_panes = self
            .snapshot
            .as_ref()
            .map(snapshot_pane_ids)
            .unwrap_or_default();
        let action = match batch {
            None => EventPollAction::Disconnect(
                HerdrError::EventStreamClosed("event worker stopped".into())
                    .to_string()
                    .into(),
            ),
            Some(items) => {
                let Some(snapshot) = self.snapshot.as_mut() else {
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
        let effects = effects_for(&action);
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
        }
        if effects.notify {
            cx.notify();
        }
        if let Some(stream) = action.event_stream() {
            self.event_stream = stream;
            cx.notify();
        }
        self.ensure_agent_status_stream(cx);
        effects.reschedule
    }

    fn listen_agent_status(mut events: EventSubscription, cx: &mut Context<Self>) -> Task<()> {
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

    fn apply_agent_status_batch(
        &mut self,
        batch: Option<Vec<std::result::Result<HerdrEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
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
        effects.reschedule
    }

    fn ensure_agent_status_stream(&mut self, cx: &mut Context<Self>) {
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

    fn release_agent_status_handoff(&mut self, cx: &mut Context<Self>) {
        let Some(handoff) = self.agent_status_handoff.take() else {
            return;
        };
        let (events, resync_after) = handoff.into_release();
        let mut resync = resync_after;
        if !events.is_empty()
            && let Some(snapshot) = self.snapshot.as_mut()
        {
            let mut events = events.into_iter();
            let action = apply_event_stream(snapshot, &mut self.selection, || Ok(events.next()));
            let effects = effects_for(&action);
            if let Some(error) = effects.error {
                self.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
            }
            if effects.notify {
                cx.notify();
            }
            resync |= effects.resync;
        }
        if resync {
            self.resync_snapshot(self.event_epoch, cx);
        }
    }

    fn rebuild_agent_status_stream(&mut self, panes: HashSet<String>, cx: &mut Context<Self>) {
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
                    Err(error) => {
                        this.agent_status_panes = event_panes_after_failed_subscribe(
                            &this.agent_status_panes,
                            &panes,
                            &previous,
                        );
                        this.notify_failure(FailureKind::ApplyLiveUpdate, error, cx);
                    }
                }
            })
            .ok();
        }));
    }

    pub(super) fn resync_snapshot(&mut self, epoch: u64, cx: &mut Context<Self>) {
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
                        this.snapshot = Some(snapshot);
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

    pub(super) fn open_node_manager(&mut self, cx: &mut Context<Self>) {
        let profile_index = self.profile_index;
        self.set_overlay(Overlay::NodeManager, cx);
        self.host_center
            .update(cx, |center, cx| center.open(profile_index, cx));
    }

    pub(super) fn open_appearance(&mut self, cx: &mut Context<Self>) {
        self.open_select = None;
        self.set_overlay(Overlay::Appearance, cx);
    }

    pub(super) fn close_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_select = None;
        self.set_overlay(Overlay::None, cx);
        self.focus.focus(window, cx);
    }

    pub(super) fn cancel_bulk_remove(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ConfirmBulkRemove) {
            self.set_overlay(Overlay::NodeManager, cx);
        }
    }

    pub(super) fn confirm_bulk_remove(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::ConfirmBulkRemove) {
            return;
        }
        self.set_overlay(Overlay::NodeManager, cx);
        self.host_center
            .update(cx, |center, cx| center.confirm_bulk_remove(cx));
    }

    pub(super) fn request_choose_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.profiles.len() {
            return;
        }
        if switch_requires_confirm(self.profile_index, index, self.live_herdr_session()) {
            self.set_overlay(
                Overlay::ConfirmSwitchProfile {
                    id: self.profiles[index].id().to_owned(),
                    from_hosts: self.overlay.host_center(),
                },
                cx,
            );
            return;
        }
        self.apply_profile(index, cx);
    }

    pub(super) fn cancel_switch_profile(&mut self, cx: &mut Context<Self>) {
        let from_hosts = match &self.overlay {
            Overlay::ConfirmSwitchProfile { from_hosts, .. } => Some(*from_hosts),
            _ => None,
        };
        if let Some(from_hosts) = from_hosts {
            self.set_overlay(
                if from_hosts {
                    Overlay::NodeManager
                } else {
                    Overlay::None
                },
                cx,
            );
        }
    }

    pub(super) fn confirm_switch_profile(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::ConfirmSwitchProfile { .. }) {
            return;
        }
        let Some(index) = confirmed_host_index(&self.overlay, &self.profiles) else {
            self.cancel_switch_profile(cx);
            return;
        };
        self.apply_profile(index, cx);
    }

    pub(super) fn toggle_host_switcher(&mut self, cx: &mut Context<Self>) {
        self.set_overlay(
            if matches!(self.overlay, Overlay::HostSwitcher) {
                Overlay::None
            } else {
                Overlay::HostSwitcher
            },
            cx,
        );
    }

    pub(super) fn close_host_switcher(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::HostSwitcher) {
            self.set_overlay(Overlay::None, cx);
        }
    }

    fn apply_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        self.set_overlay(Overlay::None, cx);
        if index == self.profile_index {
            self.remember_current_host(cx);
            self.reload(None, cx);
            cx.notify();
            return;
        }
        self.select_profile(index, cx);
    }

    pub(super) fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep the configured family name even when the file is missing, so a
        // temporarily absent theme can come back on the next launch.
        install_appearance(&self.appearance, window.appearance());
        theme::apply_window_background(window);
        let palette = current_terminal_palette(&self.appearance);
        let mut palette_error = None;
        for runtime in self
            .session_panes
            .iter_mut()
            .flat_map(|session| session.panes.values_mut())
        {
            if let Err(error) = runtime.terminal.apply_palette(&palette) {
                palette_error = Some(error);
            }
            runtime.color_scheme_dark = palette.dark;
            runtime.palette_signature = palette.signature();
        }
        if let Some(error) = palette_error {
            self.notify_failure(FailureKind::ApplyPalette, error, cx);
        }
        self.persist_settings(FailureKind::SaveAppearance, cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn set_theme_family(
        &mut self,
        family_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.theme_family = family_id;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_appearance_mode(
        &mut self,
        mode: AppearanceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.mode = mode;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_backdrop_mode(
        &mut self,
        backdrop: BackdropMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.backdrop = backdrop;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_background_opacity(
        &mut self,
        opacity: OpacityChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.background_opacity = opacity;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_font_family(
        &mut self,
        family: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.font.family = family;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_font_size(
        &mut self,
        size: FontSizeChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.font.size = size;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_font_ligatures(
        &mut self,
        ligatures: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.font.ligatures = ligatures;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_font_thicken(
        &mut self,
        thicken: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.font.thicken = thicken;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_cell_width(
        &mut self,
        percent: CellWidthChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.font.cell_width_percent = percent;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_cell_height(
        &mut self,
        percent: CellHeightChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.font.cell_height_percent = percent;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.i18n.set_preference(language);
        theme::reload_registry();
        let i18n = self.i18n;
        self.host_center
            .update(cx, |center, cx| center.apply_language(i18n, cx));
        self.rename_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::COMMON_NAME), cx)
        });
        self.persist_settings(FailureKind::SaveLanguage, cx);
        cx.notify();
    }

    pub(super) fn cancel_remove_node(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ConfirmRemoveProfile(_)) {
            self.set_overlay(Overlay::NodeManager, cx);
        }
    }

    pub(super) fn confirm_remove_node(&mut self, cx: &mut Context<Self>) {
        let Overlay::ConfirmRemoveProfile(id) = &self.overlay else {
            return;
        };
        let id = id.clone();
        self.set_overlay(Overlay::NodeManager, cx);
        self.host_center
            .update(cx, |center, cx| center.confirm_remove_node(&id, cx));
    }

    pub(super) fn close_add_remote(&mut self, cx: &mut Context<Self>) {
        self.set_overlay(Overlay::NodeManager, cx);
    }

    pub(super) fn request_close(&mut self, target: HierarchyTarget, cx: &mut Context<Self>) {
        self.set_overlay(Overlay::ConfirmClose(target), cx);
    }

    pub(super) fn cancel_close(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ConfirmClose(_)) {
            self.set_overlay(Overlay::None, cx);
        }
    }

    pub(super) fn handle_overlay_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(confirm) = overlay_confirm_or_cancel(event) else {
            return false;
        };
        match (self.overlay.clone(), confirm) {
            (Overlay::ConfirmClose(_), true) => self.confirm_close(cx),
            (Overlay::ConfirmClose(_), false) => self.cancel_close(cx),
            (Overlay::ConfirmRemoveProfile(_), true) => self.confirm_remove_node(cx),
            (Overlay::ConfirmRemoveProfile(_), false) => self.cancel_remove_node(cx),
            (Overlay::ConfirmBulkRemove, true) => self.confirm_bulk_remove(cx),
            (Overlay::ConfirmBulkRemove, false) => self.cancel_bulk_remove(cx),
            (Overlay::Rename(_), true) => self.submit_rename(window, cx),
            (Overlay::Rename(_), false) => self.cancel_rename(window, cx),
            (Overlay::ConfirmSwitchProfile { .. }, true) => self.confirm_switch_profile(cx),
            (Overlay::ConfirmSwitchProfile { .. }, false) => self.cancel_switch_profile(cx),
            (Overlay::RemoteForm(_), false) => self.close_add_remote(cx),
            (Overlay::HostSwitcher, false) => self.close_host_switcher(cx),
            (Overlay::Appearance, false) => self.close_appearance(window, cx),
            (Overlay::ContextMenu(_) | Overlay::NodeManager, false) => {
                self.set_overlay(Overlay::None, cx);
                self.focus.focus(window, cx);
            }
            _ => return false,
        }
        cx.stop_propagation();
        true
    }

    pub(super) fn confirm_close(&mut self, cx: &mut Context<Self>) {
        let Overlay::ConfirmClose(target) = &self.overlay else {
            return;
        };
        let target = target.clone();
        self.set_overlay(Overlay::None, cx);
        match target {
            HierarchyTarget::Workspace { id, .. } => {
                self.invoke("workspace.close", json!({ "workspace_id": id }), cx)
            }
            HierarchyTarget::Tab { id, .. } => {
                self.invoke("tab.close", json!({ "tab_id": id }), cx)
            }
            HierarchyTarget::Pane { id, .. } => {
                self.invoke("pane.close", json!({ "pane_id": id }), cx)
            }
        }
    }

    pub(super) fn open_context_menu(
        &mut self,
        target: HierarchyTarget,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = window.viewport_size();
        self.set_overlay(
            Overlay::ContextMenu(HierarchyContextMenu {
                target,
                x: f32::from(event.position.x)
                    .min((f32::from(viewport.width) - 220.).max(8.))
                    .max(8.),
                y: f32::from(event.position.y)
                    .min((f32::from(viewport.height) - 260.).max(8.))
                    .max(8.),
            }),
            cx,
        );
        cx.stop_propagation();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ContextMenu(_)) {
            self.set_overlay(Overlay::None, cx);
        }
    }

    pub(super) fn open_rename(
        &mut self,
        target: HierarchyTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = target.label().to_owned();
        self.rename_input
            .update(cx, |input, cx| input.set_content(label, cx));
        self.set_overlay(Overlay::Rename(target), cx);
        self.rename_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    pub(super) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Rename(_)) {
            self.set_overlay(Overlay::None, cx);
        }
        self.focus.focus(window, cx);
    }

    pub(super) fn submit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Rename(target) = &self.overlay else {
            return;
        };
        let target = target.clone();
        let label = self.rename_input.read(cx).content().trim().to_owned();
        if label.is_empty() && !matches!(target, HierarchyTarget::Pane { .. }) {
            self.notify_failure(
                FailureKind::EmptyWorkspaceOrTabName,
                self.i18n.text(k::NOTIFY_DETAIL_EMPTY_NAME),
                cx,
            );
            cx.notify();
            return;
        }
        self.set_overlay(Overlay::None, cx);
        match target {
            HierarchyTarget::Workspace { id, .. } => {
                self.invoke(
                    "workspace.rename",
                    json!({ "workspace_id": id, "label": label }),
                    cx,
                );
            }
            HierarchyTarget::Tab { id, .. } => {
                self.invoke("tab.rename", json!({ "tab_id": id, "label": label }), cx);
            }
            HierarchyTarget::Pane { id, .. } => {
                self.invoke(
                    "pane.rename",
                    json!({ "pane_id": id, "label": (!label.is_empty()).then_some(label) }),
                    cx,
                );
            }
        }
        self.focus.focus(window, cx);
    }

    pub(super) fn select_session(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get(index).cloned() else {
            return;
        };
        if !session.running {
            let command = attach_command(&self.current_profile(), &session.name);
            if let Err(error) = open_system_terminal(&command) {
                self.notify_failure(FailureKind::OpenTerminal, error, cx);
            }
            return;
        }
        self.session_index = Some(index);
        self.reload(Some(session.name), cx);
    }

    pub(super) fn open_native_tui(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.current_session() else {
            self.notify_failure(
                FailureKind::NoSessionSelected,
                self.i18n.text(k::NOTIFY_DETAIL_NO_SESSION),
                cx,
            );
            cx.notify();
            return;
        };
        let command = attach_command(&self.current_profile(), &session.name);
        if let Err(error) = open_system_terminal(&command) {
            self.notify_failure(FailureKind::OpenTerminal, error, cx);
        }
        cx.notify();
    }

    pub(super) fn select_workspace(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        self.selection.workspace_id = Some(workspace_id.clone());
        self.selection.tab_id = snapshot
            .tabs_for(&workspace_id)
            .find(|tab| tab.focused)
            .or_else(|| snapshot.tabs_for(&workspace_id).next())
            .map(|tab| tab.tab_id.clone());
        self.selection.pane_id = self.selection.tab_id.as_deref().and_then(|tab_id| {
            snapshot
                .panes_for(tab_id)
                .find(|pane| pane.focused)
                .or_else(|| snapshot.panes_for(tab_id).next())
                .map(|pane| pane.pane_id.clone())
        });
        self.ensure_session_terminals(cx);
        cx.notify();
    }

    pub(super) fn select_tab(&mut self, tab_id: String, cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        self.selection.tab_id = Some(tab_id.clone());
        self.selection.pane_id = snapshot
            .panes_for(&tab_id)
            .find(|pane| pane.focused)
            .or_else(|| snapshot.panes_for(&tab_id).next())
            .map(|pane| pane.pane_id.clone());
        self.ensure_session_terminals(cx);
        cx.notify();
    }

    pub(super) fn select_pane(
        &mut self,
        pane_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_context = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .pane(&pane_id)
                .map(|pane| (pane.workspace_id.clone(), pane.tab_id.clone()))
        });
        let changed = self.selection.pane_id.as_deref() != Some(&pane_id)
            || pane_context.as_ref().is_some_and(|(workspace_id, tab_id)| {
                self.selection.workspace_id.as_deref() != Some(workspace_id)
                    || self.selection.tab_id.as_deref() != Some(tab_id)
            });
        if let Some((workspace_id, tab_id)) = pane_context {
            self.selection.workspace_id = Some(workspace_id);
            self.selection.tab_id = Some(tab_id);
        }
        self.selection.pane_id = Some(pane_id);
        if changed {
            self.ensure_session_terminals(cx);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn ensure_session_terminals(&mut self, cx: &mut Context<Self>) {
        let Some(session_name) = self.current_session().map(|session| session.name.clone()) else {
            self.stop_session_terminals();
            return;
        };
        if self.snapshot.is_none() {
            self.stop_session_terminals();
            return;
        }
        let profile = self.current_profile();
        let selected_pane = self.selection.pane_id.clone();
        let visible_tab_id = self.selection.tab_id.clone();
        let snapshot = self.snapshot.as_ref().expect("snapshot checked above");
        let live_pane_ids = snapshot_pane_ids(snapshot);
        let pane_tabs = snapshot
            .panes
            .iter()
            .map(|pane| (pane.pane_id.clone(), pane.tab_id.clone()))
            .collect::<HashMap<_, _>>();
        let wanted = snapshot_runtime_targets(snapshot, selected_pane.as_deref());
        let incoming = SessionKey {
            profile_id: profile.id().to_owned(),
            session_name: session_name.clone(),
        };
        if session_panes_plan(
            self.session_panes.as_ref().map(|session| &session.owner),
            &incoming,
        ) == SessionPanesPlan::Replace
        {
            self.session_panes = Some(SessionPanes::new(incoming));
        }
        let palette = current_terminal_palette(&self.appearance);
        let color_scheme_dark = palette.dark;
        let mut palette_error = None;
        let mut spawn_error = None;
        let mut spawned = HashSet::new();
        let mut pending_listens = Vec::new();
        {
            let panes = &mut self
                .session_panes
                .as_mut()
                .expect("live session adopted panes")
                .panes;
            panes.retain(|pane_id, _| live_pane_ids.contains(pane_id));
            for (pane_id, mode) in &wanted {
                match visible_pane_plan(
                    panes.get(pane_id).map(|runtime| runtime.mode),
                    panes
                        .get(pane_id)
                        .is_some_and(|runtime| runtime.session.is_closed() || runtime.exit_seen),
                    *mode,
                ) {
                    VisiblePanePlan::Keep
                    | VisiblePanePlan::PromoteToControl
                    | VisiblePanePlan::DemoteToObserve => {
                        if let Some(runtime) = panes.get_mut(pane_id) {
                            if runtime.palette_signature != palette.signature() {
                                if let Err(error) = runtime.terminal.apply_palette(&palette) {
                                    palette_error = Some(error);
                                }
                                runtime.color_scheme_dark = palette.dark;
                                runtime.palette_signature = palette.signature();
                            }
                            if let Some(frames) = sync_pane_session(
                                runtime,
                                *mode,
                                profile.clone(),
                                session_name.clone(),
                                pane_id.clone(),
                            ) {
                                pending_listens.push((pane_id.clone(), frames));
                            }
                        }
                    }
                    VisiblePanePlan::Spawn => {
                        let cols = 80;
                        let rows = 24;
                        let (session, frames) = TerminalSession::spawn(
                            profile.clone(),
                            session_name.clone(),
                            pane_id.clone(),
                            *mode,
                            cols,
                            rows,
                        );
                        match Terminal::new(cols, rows, 10_000, &palette) {
                            Ok(terminal) => {
                                terminal.set_focus(*mode == TerminalMode::ControlTakeover);
                                panes.insert(
                                    pane_id.clone(),
                                    PaneRuntime {
                                        session,
                                        terminal,
                                        frame: None,
                                        mode: *mode,
                                        size: (cols, rows),
                                        pixel_size: (0, 0),
                                        frame_context: 0,
                                        color_scheme_dark,
                                        palette_signature: palette.signature(),
                                        listen: None,
                                        exit_seen: false,
                                        scroll_px: 0.,
                                        body_bounds: (0., 0., 0., 0.),
                                    },
                                );
                                spawned.insert(pane_id.clone());
                                pending_listens.push((pane_id.clone(), frames));
                            }
                            Err(error) => spawn_error = Some(error.to_string()),
                        }
                    }
                }
            }
            for (pane_id, _) in &wanted {
                let pane_tab = pane_tabs.get(pane_id).map(String::as_str);
                if !should_flush_session_pane(
                    pane_tab,
                    visible_tab_id.as_deref(),
                    spawned.contains(pane_id),
                ) {
                    continue;
                }
                if let Some(runtime) = panes.get_mut(pane_id) {
                    flush_pane_surface(runtime);
                }
            }
        }
        if let Some(error) = palette_error {
            self.notify_failure(FailureKind::ApplyPalette, error, cx);
        }
        if let Some(error) = spawn_error {
            self.notify_failure(FailureKind::SpawnTerminal, error, cx);
        }
        for (pane_id, frames) in pending_listens {
            let task = Self::listen_pane(pane_id.clone(), frames, cx);
            if let Some(runtime) = self.pane_mut(&pane_id) {
                runtime.listen = Some(task);
            }
        }
    }

    fn stop_session_terminals(&mut self) {
        self.session_panes = None;
    }

    fn listen_pane(
        pane_id: String,
        mut frames: UnboundedReceiver<std::result::Result<TerminalFrame, HerdrError>>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let herdr = next_batch(&mut frames);
                let ghostty = poll_fn(|task_cx| {
                    this.update(cx, |this, _| {
                        let Some(runtime) = this.pane_mut(&pane_id) else {
                            return Poll::Ready(None);
                        };
                        runtime.terminal.poll_frame(task_cx)
                    })
                    .unwrap_or(Poll::Ready(None))
                });
                pin_mut!(herdr, ghostty);
                match future::select(herdr, ghostty).await {
                    Either::Left((batch, _)) => {
                        let keep = this
                            .update(cx, |this, cx| this.apply_herdr_frames(&pane_id, batch, cx))
                            .unwrap_or(false);
                        if !keep {
                            break;
                        }
                    }
                    Either::Right((frame, _)) => {
                        let keep = this
                            .update(cx, |this, cx| this.apply_ghostty_frame(&pane_id, frame, cx))
                            .unwrap_or(false);
                        if !keep {
                            break;
                        }
                    }
                }
            }
        })
    }

    fn apply_herdr_frames(
        &mut self,
        pane_id: &str,
        batch: Option<Vec<std::result::Result<TerminalFrame, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let composing = self.ime_marked.clone();
        let selected_pane = self.selection.pane_id.clone();
        let visible_pane_ids =
            visible_pane_ids(self.snapshot.as_ref(), self.selection.tab_id.as_deref());
        let mut error = None;
        let mut hierarchy_changed = false;
        let mut changed = false;
        let keep = {
            let Some(runtime) = self.pane_mut(pane_id) else {
                return false;
            };
            match batch {
                None => {
                    runtime.exit_seen = true;
                    hierarchy_changed = true;
                    false
                }
                Some(items) => {
                    let mut closed = false;
                    for item in items {
                        match item {
                            Ok(frame) => {
                                if runtime.size != (frame.width, frame.height)
                                    && incoming_frame_should_replace_grid(runtime.pixel_size)
                                {
                                    match runtime.terminal.set_grid_size(frame.width, frame.height)
                                    {
                                        Ok(resolved) => {
                                            runtime.size = (resolved.columns, resolved.rows);
                                            runtime.pixel_size = (0, 0);
                                        }
                                        Err(resize_error) => {
                                            error = Some((
                                                FailureKind::ResizeTerminal,
                                                resize_error.to_string(),
                                            ))
                                        }
                                    }
                                }
                                runtime.terminal.apply_frame(&frame.bytes, frame.full);
                                if selected_pane.as_deref() == Some(pane_id)
                                    && let Some(preedit) = composing.as_deref()
                                {
                                    runtime.terminal.set_preedit(Some(preedit));
                                }
                            }
                            Err(stream_error) => {
                                runtime.exit_seen = true;
                                hierarchy_changed = true;
                                closed = true;
                                if !is_expected_terminal_exit(&stream_error) {
                                    error = Some((
                                        FailureKind::TerminalStream,
                                        stream_error.to_string(),
                                    ));
                                }
                                break;
                            }
                        }
                    }
                    if let Err(runtime_error) = Terminal::tick_runtime() {
                        error = Some((FailureKind::TerminalRuntime, runtime_error.to_string()));
                    }
                    if forward_terminal_input(runtime).is_err() {
                        runtime.exit_seen = true;
                        hierarchy_changed = true;
                        closed = true;
                    }
                    match runtime.terminal.try_frame() {
                        Ok(Some(frame)) if frame.host_context == runtime.frame_context => {
                            runtime.frame = Some(frame);
                            if visible_pane_ids.contains(pane_id) {
                                changed = true;
                            }
                        }
                        Ok(Some(_)) | Ok(None) => {}
                        Err(frame_error) => {
                            error = Some((FailureKind::RenderTerminal, frame_error.to_string()))
                        }
                    }
                    !closed
                }
            }
        };
        if let Some((kind, detail)) = error {
            self.notify_failure(kind, detail, cx);
        }
        if hierarchy_changed {
            self.resync_snapshot(self.event_epoch, cx);
        }
        if changed {
            cx.notify();
        }
        keep
    }

    fn apply_ghostty_frame(
        &mut self,
        pane_id: &str,
        frame: Option<std::result::Result<RenderedFrame, ocherdr_terminal::TerminalError>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let visible_pane_ids =
            visible_pane_ids(self.snapshot.as_ref(), self.selection.tab_id.as_deref());
        let mut error = None;
        let mut changed = false;
        let mut hierarchy_changed = false;
        let keep = {
            let Some(runtime) = self.pane_mut(pane_id) else {
                return false;
            };
            let Some(frame) = frame else {
                return false;
            };
            match frame {
                Ok(frame) if frame.host_context == runtime.frame_context => {
                    runtime.frame = Some(frame);
                    changed = visible_pane_ids.contains(pane_id);
                }
                Ok(_) => {}
                Err(frame_error) => {
                    error = Some((FailureKind::RenderTerminal, frame_error.to_string()))
                }
            }
            if forward_terminal_input(runtime).is_err() {
                runtime.exit_seen = true;
                hierarchy_changed = true;
                false
            } else {
                true
            }
        };
        if let Some((kind, detail)) = error {
            self.notify_failure(kind, detail, cx);
        }
        if hierarchy_changed {
            self.resync_snapshot(self.event_epoch, cx);
        }
        if changed {
            cx.notify();
        }
        keep
    }

    pub(super) fn sync_measured_pane_body(
        &mut self,
        pane_id: &str,
        bounds: Bounds<ochub_ui::gpui::Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let body = (
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        );
        let scale = window.scale_factor();
        let width_px = (body.2 * scale).round() as u32;
        let height_px = (body.3 * scale).round() as u32;
        let scale_factor = f64::from(scale);
        let palette = current_terminal_palette(&self.appearance);
        let mut palette_error = None;
        let mut resized = false;
        {
            let Some(runtime) = self.pane_mut(pane_id) else {
                return;
            };
            runtime.body_bounds = body;
            if runtime.palette_signature != palette.signature() {
                if let Err(error) = runtime.terminal.apply_palette(&palette) {
                    palette_error = Some(error);
                }
                runtime.color_scheme_dark = palette.dark;
                runtime.palette_signature = palette.signature();
            }
            if runtime.pixel_size != (width_px, height_px) {
                runtime.frame_context = runtime.frame_context.wrapping_add(1);
                let resolved = runtime.terminal.resize_pixels(
                    width_px,
                    height_px,
                    scale_factor,
                    runtime.frame_context,
                );
                let size = (resolved.columns, resolved.rows);
                if runtime.mode == TerminalMode::ControlTakeover {
                    let _ = runtime.session.send(TerminalCommand::Resize {
                        cols: resolved.columns,
                        rows: resolved.rows,
                        cell_width_px: resolved.cell_width_px,
                        cell_height_px: resolved.cell_height_px,
                    });
                }
                runtime.size = size;
                runtime.pixel_size = (width_px, height_px);
                resized = true;
            }
        }
        if let Some(error) = palette_error {
            self.notify_failure(FailureKind::ApplyPalette, error, cx);
        }
        if resized {
            cx.notify();
        }
    }

    pub(super) fn selected_workspace_target(&self) -> Option<HierarchyTarget> {
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

    pub(super) fn selected_tab_target(&self) -> Option<HierarchyTarget> {
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

    pub(super) fn selected_pane_target(&self) -> Option<HierarchyTarget> {
        let pane_id = self.selection.pane_id.as_deref()?;
        let pane = self.snapshot.as_ref()?.pane(pane_id)?;
        Some(HierarchyTarget::Pane {
            id: pane.pane_id.clone(),
            label: pane.display_name().to_owned(),
        })
    }

    fn cmd_w_close_target(&self) -> Option<HierarchyTarget> {
        let snapshot = self.snapshot.as_ref()?;
        let tab_id = self.selection.tab_id.as_deref()?;
        cmd_w_close_target(snapshot, tab_id, self.selection.pane_id.as_deref())
    }

    pub(super) fn create_workspace(&mut self, cx: &mut Context<Self>) {
        self.invoke("workspace.create", json!({ "focus": true, "env": {} }), cx);
    }

    pub(super) fn create_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(workspace_id) = self.selection.workspace_id.clone() {
            self.invoke(
                "tab.create",
                json!({ "workspace_id": workspace_id, "focus": true, "env": {} }),
                cx,
            );
        }
    }

    pub(super) fn cycle_tab(&mut self, offset: isize, cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(workspace_id) = self.selection.workspace_id.as_deref() else {
            return;
        };
        let tab_ids = snapshot
            .tabs_for(workspace_id)
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        if tab_ids.is_empty() {
            return;
        }
        let current = self
            .selection
            .tab_id
            .as_ref()
            .and_then(|tab_id| tab_ids.iter().position(|candidate| candidate == tab_id))
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(tab_ids.len() as isize) as usize;
        self.select_tab(tab_ids[next].clone(), cx);
    }

    pub(super) fn select_tab_number(&mut self, number: usize, cx: &mut Context<Self>) {
        let tab_id = self.snapshot.as_ref().and_then(|snapshot| {
            self.selection
                .workspace_id
                .as_deref()
                .and_then(|workspace_id| {
                    tab_id_for_shortcut(snapshot.tabs_for(workspace_id), number)
                })
        });
        if let Some(tab_id) = tab_id {
            self.select_tab(tab_id, cx);
        }
    }

    pub(super) fn focus_pane_direction(&mut self, direction: &'static str, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.selection.pane_id.clone() {
            self.invoke(
                "pane.focus_direction",
                json!({ "pane_id": pane_id, "direction": direction }),
                cx,
            );
        }
    }

    pub(super) fn handle_prefix_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prefix_pending = false;
        let key = event.keystroke.key.as_str();
        let shift = event.keystroke.modifiers.shift;
        match (key, shift) {
            ("escape", _) => {}
            ("s", false) => self.open_native_tui(cx),
            ("c", false) => self.create_tab(cx),
            ("n", true) => self.create_workspace(cx),
            ("n", false) => self.cycle_tab(1, cx),
            ("p", false) => self.cycle_tab(-1, cx),
            ("w", true) => {
                if let Some(target) = self.selected_workspace_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("d", true) => {
                if let Some(target) = self.selected_workspace_target() {
                    self.request_close(target, cx);
                }
            }
            ("t", true) => {
                if let Some(target) = self.selected_tab_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("x", true) => {
                if let Some(target) = self.selected_tab_target() {
                    self.request_close(target, cx);
                }
            }
            ("p", true) => {
                if let Some(target) = self.selected_pane_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("h", false) => self.focus_pane_direction("left", cx),
            ("j", false) => self.focus_pane_direction("down", cx),
            ("k", false) => self.focus_pane_direction("up", cx),
            ("l", false) => self.focus_pane_direction("right", cx),
            _ => {
                if let Some(number) =
                    tab_index_from_keystroke(key, event.keystroke.key_char.as_deref())
                {
                    self.select_tab_number(number, cx);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_app_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control && !modifiers.platform && !modifiers.alt && key == "b" {
            self.prefix_pending = true;
            if matches!(self.overlay, Overlay::ContextMenu(_)) {
                self.set_overlay(Overlay::None, cx);
            }
            cx.notify();
            return true;
        }
        if self.prefix_pending {
            self.handle_prefix_key(event, window, cx);
            return true;
        }
        if key == "escape" {
            if matches!(self.overlay, Overlay::Appearance) {
                self.close_appearance(window, cx);
                return true;
            }
            if matches!(
                self.overlay,
                Overlay::ContextMenu(_) | Overlay::NodeManager | Overlay::HostSwitcher
            ) {
                self.set_overlay(Overlay::None, cx);
                self.focus.focus(window, cx);
                return true;
            }
            if matches!(self.overlay, Overlay::ConfirmClose(_)) {
                self.set_overlay(Overlay::None, cx);
                return true;
            }
        }
        if modifiers.platform && !modifiers.alt && !modifiers.control {
            if let Some(number) = tab_index_from_keystroke(key, event.keystroke.key_char.as_deref())
            {
                self.select_tab_number(number, cx);
                return true;
            }
            let handled = match (key, modifiers.shift) {
                ("t", false) => {
                    self.create_tab(cx);
                    true
                }
                ("w", true) => {
                    if let Some(target) = self.selected_workspace_target() {
                        self.request_close(target, cx);
                    }
                    true
                }
                ("w", false) => {
                    if let Some(target) = self.cmd_w_close_target() {
                        self.request_close(target, cx);
                    }
                    true
                }
                ("n", true) => {
                    self.create_workspace(cx);
                    true
                }
                (",", false) => {
                    self.open_native_tui(cx);
                    true
                }
                ("c", false) => {
                    self.copy_selection(cx);
                    true
                }
                ("a", false) => {
                    self.select_all_visible(cx);
                    true
                }
                ("[", false) => {
                    self.cycle_tab(-1, cx);
                    true
                }
                ("]", false) => {
                    self.cycle_tab(1, cx);
                    true
                }
                _ => false,
            };
            if handled {
                return true;
            }
        }
        if modifiers.control && key == "tab" {
            self.cycle_tab(if modifiers.shift { -1 } else { 1 }, cx);
            return true;
        }
        if key == "f2" && !modifiers.platform && !modifiers.control && !modifiers.alt {
            let target = self
                .selected_tab_target()
                .or_else(|| self.selected_workspace_target());
            if let Some(target) = target {
                self.open_rename(target, window, cx);
            }
            return true;
        }
        false
    }

    pub(super) fn send_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.handle_app_shortcut(event, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.ime_marked.is_some() {
            return;
        }
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let key = &event.keystroke;
        let stream_closed = {
            let Some(runtime) = self.pane_mut(&pane_id) else {
                return;
            };
            if key.modifiers.platform && key.key == "v" {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    runtime.terminal.paste(&text);
                    let _ = Terminal::tick_runtime();
                    let closed = forward_terminal_input(runtime).is_err();
                    if closed {
                        runtime.exit_seen = true;
                    }
                    cx.stop_propagation();
                    closed
                } else {
                    false
                }
            } else {
                let modifiers = KeyModifiers {
                    control: key.modifiers.control,
                    alt: key.modifiers.alt,
                    shift: key.modifiers.shift,
                    platform: key.modifiers.platform,
                };
                let Some(bytes) = encode_pty_bytes(&key.key, key.key_char.as_deref(), modifiers)
                else {
                    return;
                };
                let closed = runtime.session.send(TerminalCommand::Input(bytes)).is_err();
                if closed {
                    runtime.exit_seen = true;
                }
                cx.stop_propagation();
                closed
            }
        };
        if stream_closed {
            self.resync_snapshot(self.event_epoch, cx);
        }
    }

    pub(super) fn pane_mouse_down(
        &mut self,
        pane_id: String,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(previous) = self.text_drag_pane.take()
            && previous != pane_id
            && let Some(runtime) = self.pane_mut(&previous)
        {
            runtime
                .terminal
                .end_text_selection(None, KeyModifiers::default(), 1.0);
        }
        self.select_pane(pane_id.clone(), window, cx);
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        let mouse = mouse_point(event.position);
        if !point_in_rect(mouse, runtime.body_bounds) {
            self.text_drag_pane = None;
            return;
        }
        let scale = f64::from(window.scale_factor());
        let Some(surface) = map_mouse_to_surface(
            mouse,
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        ) else {
            self.text_drag_pane = None;
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        self.text_drag_pane = Some(pane_id.clone());
        if let Some(runtime) = self.pane_mut(&pane_id) {
            runtime.terminal.begin_text_selection(
                surface.0,
                surface.1,
                modifiers,
                event.click_count,
                scale,
            );
            flush_pane_surface(runtime);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn pane_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.text_drag_pane.clone() else {
            return;
        };
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        let scale = f64::from(window.scale_factor());
        let Some(surface) = map_mouse_to_surface(
            mouse_point(event.position),
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        ) else {
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        if let Some(runtime) = self.pane_mut(&pane_id) {
            runtime
                .terminal
                .update_text_selection(surface.0, surface.1, modifiers, scale);
            flush_pane_surface(runtime);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn pane_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.text_drag_pane.take() else {
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        let scale = f64::from(window.scale_factor());
        if let Some(runtime) = self.pane_mut(&pane_id) {
            let point = map_mouse_to_surface(
                mouse_point(event.position),
                runtime.body_bounds,
                runtime.pixel_size,
                window.scale_factor(),
            );
            runtime.terminal.end_text_selection(point, modifiers, scale);
            flush_pane_surface(runtime);
            copy_terminal_selection(runtime, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        if !runtime.terminal.has_selection() {
            return;
        }
        let Some(text) = runtime.terminal.read_selection() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.stop_propagation();
    }

    pub(super) fn select_all_visible(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let Some(runtime) = self.pane_mut(&pane_id) else {
            return;
        };
        if !runtime.terminal.select_all_visible() {
            return;
        }
        flush_pane_surface(runtime);
        copy_terminal_selection(runtime, cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn scroll_pane(
        &mut self,
        pane_id: &str,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.pane_mut(pane_id) else {
            return;
        };
        let line_height = if runtime.size.1 == 0 {
            16.
        } else {
            (runtime.pixel_size.1 as f32 / f32::from(runtime.size.1)).max(1.)
        };
        let lines = wheel_scroll_lines(event.delta, line_height, &mut runtime.scroll_px);
        if lines == 0 {
            cx.stop_propagation();
            return;
        }
        let direction = if lines > 0 { "up" } else { "down" };
        let _ = runtime.session.send(TerminalCommand::Scroll {
            direction,
            lines: lines.unsigned_abs() as u16,
        });
        cx.stop_propagation();
    }

    pub(super) fn invoke(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        let Some(connection) = &self.connection else {
            return;
        };
        let socket = connection.socket_path().to_owned();
        self.operation = Some(self.i18n.running_operation(method).into());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { request_socket(&socket, method, params).map(|_| ()) },
                )
                .await;
            this.update(cx, |this, cx| {
                this.operation = None;
                match result {
                    Ok(()) => {
                        if command_needs_snapshot_resync(method) {
                            this.resync_snapshot(this.event_epoch, cx);
                        }
                    }
                    Err(error) => this.notify_command_failure(method, error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn accepts_ime(&self) -> bool {
        key_goes_to_terminal(&self.overlay) && self.selection.pane_id.is_some()
    }

    pub(super) fn commit_ime_text(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_ime_preedit(window, cx);
        if text.is_empty() {
            return;
        }
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let stream_closed = {
            let Some(runtime) = self.pane_mut(&pane_id) else {
                return;
            };
            let closed = runtime
                .session
                .send(TerminalCommand::Input(text.as_bytes().to_vec()))
                .is_err();
            if closed {
                runtime.exit_seen = true;
            }
            closed
        };
        window.invalidate_character_coordinates();
        cx.notify();
        if stream_closed {
            self.resync_snapshot(self.event_epoch, cx);
        }
    }

    pub(super) fn set_ime_preedit(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            self.clear_ime_preedit(window, cx);
            return;
        }
        self.ime_marked = Some(text.to_owned());
        if let Some(pane_id) = self.selection.pane_id.clone()
            && let Some(runtime) = self.pane_mut(&pane_id)
        {
            runtime.terminal.set_preedit(Some(text));
            flush_pane_surface(runtime);
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    pub(super) fn clear_ime_preedit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ime_marked.take().is_none() {
            return;
        }
        if let Some(pane_id) = self.selection.pane_id.clone()
            && let Some(runtime) = self.pane_mut(&pane_id)
        {
            runtime.terminal.set_preedit(None);
            flush_pane_surface(runtime);
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    pub(super) fn ime_cursor_bounds(
        &self,
        window: &Window,
    ) -> Option<Bounds<ochub_ui::gpui::Pixels>> {
        let pane_id = self.selection.pane_id.as_deref()?;
        let runtime = self.pane(pane_id)?;
        let (x, y, width, height) = runtime.terminal.ime_point();
        let (left, top, w, h) = map_surface_rect_to_window(
            (x, y - height, width.max(1.0), height.max(1.0)),
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        )?;
        Some(Bounds {
            origin: point(px(left), px(top)),
            size: size(px(w.max(1.)), px(h.max(1.))),
        })
    }
}

fn snapshot_pane_ids(snapshot: &HierarchySnapshot) -> HashSet<String> {
    snapshot.pane_ids()
}

fn session_terminals_need_rebuild(
    old_tab: Option<&str>,
    old_selected: Option<&str>,
    old_panes: &HashSet<String>,
    selection: &Selection,
    snapshot: &HierarchySnapshot,
    closed_stream: bool,
) -> bool {
    old_tab != selection.tab_id.as_deref()
        || old_selected != selection.pane_id.as_deref()
        || *old_panes != snapshot_pane_ids(snapshot)
        || closed_stream
}

fn snapshot_runtime_targets(
    snapshot: &HierarchySnapshot,
    selected_pane: Option<&str>,
) -> Vec<(String, TerminalMode)> {
    snapshot
        .panes
        .iter()
        .map(|pane| {
            let mode = if selected_pane == Some(pane.pane_id.as_str()) {
                TerminalMode::ControlTakeover
            } else {
                TerminalMode::Observe
            };
            (pane.pane_id.clone(), mode)
        })
        .collect()
}

fn should_flush_session_pane(
    pane_tab_id: Option<&str>,
    visible_tab_id: Option<&str>,
    newly_spawned: bool,
) -> bool {
    newly_spawned || pane_tab_id == visible_tab_id
}

fn cmd_w_close_target(
    snapshot: &HierarchySnapshot,
    tab_id: &str,
    pane_id: Option<&str>,
) -> Option<HierarchyTarget> {
    if snapshot.panes_for(tab_id).count() > 1 {
        let pane = snapshot.pane(pane_id?)?;
        return Some(HierarchyTarget::Pane {
            id: pane.pane_id.clone(),
            label: pane.display_name().to_owned(),
        });
    }
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id)?;
    Some(HierarchyTarget::Tab {
        id: tab.tab_id.clone(),
        label: tab.label.clone(),
    })
}

fn incoming_frame_should_replace_grid(pixel_size: (u32, u32)) -> bool {
    // Once layout has sized the local Metal surface, incoming 80×24 observe
    // (or takeover) frames must not shrink it. That shrink-then-grow is the
    // flash seen when clicking another pane.
    pixel_size == (0, 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPanesPlan {
    Keep,
    Replace,
}

fn session_panes_plan(current: Option<&SessionKey>, incoming: &SessionKey) -> SessionPanesPlan {
    match current {
        Some(owner) if owner == incoming => SessionPanesPlan::Keep,
        _ => SessionPanesPlan::Replace,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisiblePanePlan {
    Keep,
    PromoteToControl,
    DemoteToObserve,
    Spawn,
}

fn visible_pane_plan(
    existing: Option<TerminalMode>,
    existing_closed: bool,
    wanted: TerminalMode,
) -> VisiblePanePlan {
    if existing_closed {
        return VisiblePanePlan::Spawn;
    }
    match existing {
        None => VisiblePanePlan::Spawn,
        Some(current) if current == wanted => VisiblePanePlan::Keep,
        Some(TerminalMode::Observe) => VisiblePanePlan::PromoteToControl,
        Some(TerminalMode::ControlTakeover) => VisiblePanePlan::DemoteToObserve,
    }
}

fn is_expected_terminal_exit(error: &HerdrError) -> bool {
    matches!(error, HerdrError::TerminalClosed(_))
}

fn snapshot_refresh_should_queue(refreshing: bool) -> bool {
    refreshing
}

fn snapshot_handoff_should_release(refreshing: bool) -> bool {
    !refreshing
}

// pane.rename emits nothing. pane.close can delete the parent tab and
// reshuffle focus / tab numbers without emitting tab.closed.
fn command_needs_snapshot_resync(method: &str) -> bool {
    matches!(method, "pane.rename" | "pane.close")
}

#[derive(Debug, PartialEq, Eq)]
enum EventPollAction {
    /// Replace Live with Lost. A dead stream has nothing left to poll.
    Disconnect(SharedString),
    Idle,
    Applied,
    Resync {
        error: Option<SharedString>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PollEffects {
    resync: bool,
    apply_local: bool,
    notify: bool,
    reschedule: bool,
    error: Option<SharedString>,
}

fn effects_for(action: &EventPollAction) -> PollEffects {
    match action {
        EventPollAction::Disconnect(_) => PollEffects {
            resync: false,
            apply_local: false,
            notify: true,
            reschedule: false,
            error: None,
        },
        EventPollAction::Idle => PollEffects {
            resync: false,
            apply_local: false,
            notify: false,
            reschedule: true,
            error: None,
        },
        EventPollAction::Applied => PollEffects {
            resync: false,
            apply_local: true,
            notify: true,
            reschedule: true,
            error: None,
        },
        EventPollAction::Resync { error } => PollEffects {
            resync: true,
            apply_local: false,
            notify: false,
            reschedule: true,
            error: error.clone(),
        },
    }
}

impl EventPollAction {
    fn event_stream(&self) -> Option<EventStreamState> {
        match self {
            Self::Disconnect(detail) => Some(EventStreamState::Lost(detail.clone())),
            Self::Idle | Self::Applied | Self::Resync { .. } => None,
        }
    }
}

fn apply_event_stream(
    snapshot: &mut HierarchySnapshot,
    selection: &mut Selection,
    next: impl FnMut() -> std::result::Result<Option<HerdrEvent>, HerdrError>,
) -> EventPollAction {
    let action = poll_event_stream(snapshot, next);
    if effects_for(&action).apply_local {
        selection.reconcile(snapshot);
    }
    action
}

fn poll_event_stream(
    snapshot: &mut HierarchySnapshot,
    mut next: impl FnMut() -> std::result::Result<Option<HerdrEvent>, HerdrError>,
) -> EventPollAction {
    let mut seen = false;
    let mut resync = false;
    let mut error = None;
    for _ in 0..128 {
        match next() {
            Ok(Some(event)) => {
                seen = true;
                if snapshot.apply(&event) == SnapshotUpdate::Resync {
                    resync = true;
                }
            }
            Ok(None) => break,
            Err(err) if err.is_event_payload_error() => {
                resync = true;
                error = Some(err.to_string().into());
            }
            Err(err) => return EventPollAction::Disconnect(err.to_string().into()),
        }
    }
    if resync {
        EventPollAction::Resync { error }
    } else if seen {
        EventPollAction::Applied
    } else {
        EventPollAction::Idle
    }
}

fn mouse_point(position: ochub_ui::gpui::Point<ochub_ui::gpui::Pixels>) -> (f32, f32) {
    (f32::from(position.x), f32::from(position.y))
}

fn gpui_key_modifiers(modifiers: ochub_ui::gpui::Modifiers) -> KeyModifiers {
    KeyModifiers {
        control: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
        platform: modifiers.platform,
    }
}

fn point_in_rect(point: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    point.0 >= rect.0 && point.1 >= rect.1 && point.0 < rect.0 + rect.2 && point.1 < rect.1 + rect.3
}

struct FittedSurface {
    origin: (f32, f32),
    fitted: (f32, f32),
    surface: (f32, f32),
}

fn fitted_surface(
    body: (f32, f32, f32, f32),
    pixel_size: (u32, u32),
    scale_factor: f32,
) -> Option<FittedSurface> {
    let (bx, by, bw, bh) = body;
    if bw <= 0. || bh <= 0. || pixel_size.0 == 0 || pixel_size.1 == 0 {
        return None;
    }
    let image_w = pixel_size.0 as f32;
    let image_h = pixel_size.1 as f32;
    let image_ratio = image_w / image_h;
    let bounds_ratio = bw / bh;
    let (fitted_w, fitted_h) = if bounds_ratio > image_ratio {
        (image_w * (bh / image_h), bh)
    } else {
        (bw, image_h * (bw / image_w))
    };
    if fitted_w <= 0. || fitted_h <= 0. {
        return None;
    }
    let scale = scale_factor.max(1.);
    Some(FittedSurface {
        origin: (bx + (bw - fitted_w) / 2., by + (bh - fitted_h) / 2.),
        fitted: (fitted_w, fitted_h),
        surface: (image_w / scale, image_h / scale),
    })
}

/// Map a window-space click onto Ghostty view points, matching GPUI
/// `ObjectFit::Contain` (device pixels treated as `Pixels` 1:1).
fn map_mouse_to_surface(
    mouse: (f32, f32),
    body: (f32, f32, f32, f32),
    pixel_size: (u32, u32),
    scale_factor: f32,
) -> Option<(f64, f64)> {
    let fitted = fitted_surface(body, pixel_size, scale_factor)?;
    Some((
        f64::from((mouse.0 - fitted.origin.0) / fitted.fitted.0 * fitted.surface.0),
        f64::from((mouse.1 - fitted.origin.1) / fitted.fitted.1 * fitted.surface.1),
    ))
}

fn map_surface_rect_to_window(
    rect: (f64, f64, f64, f64),
    body: (f32, f32, f32, f32),
    pixel_size: (u32, u32),
    scale_factor: f32,
) -> Option<(f32, f32, f32, f32)> {
    let fitted = fitted_surface(body, pixel_size, scale_factor)?;
    if fitted.surface.0 <= 0. || fitted.surface.1 <= 0. {
        return None;
    }
    let left = fitted.origin.0 + (rect.0 as f32) / fitted.surface.0 * fitted.fitted.0;
    let top = fitted.origin.1 + (rect.1 as f32) / fitted.surface.1 * fitted.fitted.1;
    let width = (rect.2 as f32) / fitted.surface.0 * fitted.fitted.0;
    let height = (rect.3 as f32) / fitted.surface.1 * fitted.fitted.1;
    Some((left, top, width, height))
}

fn copy_terminal_selection(runtime: &PaneRuntime, cx: &mut Context<OcHerdrView>) {
    if !runtime.terminal.has_selection() {
        return;
    }
    let Some(text) = runtime.terminal.read_selection() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

fn flush_pane_surface(runtime: &mut PaneRuntime) {
    runtime.terminal.refresh();
    let _ = Terminal::tick_runtime();
    let _ = forward_terminal_input(runtime);
    if let Ok(Some(frame)) = runtime.terminal.try_frame()
        && frame.host_context == runtime.frame_context
    {
        runtime.frame = Some(frame);
    }
}

fn wheel_scroll_lines(delta: ScrollDelta, line_height: f32, leftover: &mut f32) -> i32 {
    match delta {
        ScrollDelta::Lines(delta) => {
            *leftover = 0.;
            delta.y.round() as i32
        }
        ScrollDelta::Pixels(delta) => {
            if line_height <= 0. {
                return 0;
            }
            *leftover += f32::from(delta.y);
            let lines = (*leftover / line_height).trunc() as i32;
            *leftover -= lines as f32 * line_height;
            lines
        }
    }
}

fn current_terminal_palette(appearance: &AppearanceSettings) -> TerminalPalette {
    terminal_palette_from_theme(theme::current(), theme::is_dark(), &appearance.font)
}

fn terminal_palette_from_theme(
    theme: ochub_ui::theme::Theme,
    dark: bool,
    font: &TerminalFontSettings,
) -> TerminalPalette {
    TerminalPalette {
        dark,
        background: theme.bg.0,
        foreground: theme.text.0,
        cursor: theme.accent.0,
        selection: theme.selection.0,
        ansi: [
            theme.overlay.0,
            theme.red.0,
            theme.green.0,
            theme.yellow.0,
            theme.accent.0,
            theme.mauve.0,
            theme.teal.0,
            theme.subtext.0,
            theme.muted.0,
            theme.red.0,
            theme.green.0,
            theme.yellow.0,
            theme.accent.0,
            theme.mauve.0,
            theme.teal.0,
            theme.text.0,
        ],
        font_family: font.family.clone(),
        font_size: font.size.value(),
        ligatures: font.ligatures,
        thicken: font.thicken,
        cell_width_percent: font.cell_width_percent.value(),
        cell_height_percent: font.cell_height_percent.value(),
    }
}

fn visible_pane_ids(snapshot: Option<&HierarchySnapshot>, tab_id: Option<&str>) -> HashSet<String> {
    snapshot
        .zip(tab_id)
        .map(|(snapshot, tab_id)| {
            snapshot
                .panes_for(tab_id)
                .map(|pane| pane.pane_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn sync_pane_session(
    runtime: &mut PaneRuntime,
    wanted: TerminalMode,
    profile: ConnectionProfile,
    session_name: String,
    pane_id: String,
) -> Option<UnboundedReceiver<std::result::Result<TerminalFrame, HerdrError>>> {
    runtime
        .terminal
        .set_focus(wanted == TerminalMode::ControlTakeover);
    if runtime.mode == wanted {
        return None;
    }
    runtime.listen = None;
    let (cols, rows) = runtime.size;
    let (session, frames) = TerminalSession::spawn(
        profile,
        session_name,
        pane_id,
        wanted,
        cols.max(1),
        rows.max(1),
    );
    runtime.session = session;
    runtime.mode = wanted;
    if wanted == TerminalMode::ControlTakeover {
        send_session_resize(runtime);
    }
    Some(frames)
}

fn send_session_resize(runtime: &PaneRuntime) {
    let size = runtime.terminal.surface_size();
    if size.columns == 0 || size.rows == 0 {
        return;
    }
    let _ = runtime.session.send(TerminalCommand::Resize {
        cols: size.columns,
        rows: size.rows,
        cell_width_px: size.cell_width_px.max(1),
        cell_height_px: size.cell_height_px.max(1),
    });
}

fn forward_terminal_input(runtime: &PaneRuntime) -> Result<(), ()> {
    while let Some(bytes) = runtime.terminal.try_input() {
        if runtime.mode == TerminalMode::ControlTakeover
            && runtime.session.send(TerminalCommand::Input(bytes)).is_err()
        {
            return Err(());
        }
    }
    Ok(())
}

fn control_letter_bytes(key: &str, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    if !modifiers.control || modifiers.alt || modifiers.platform {
        return None;
    }
    let letter = key.chars().next()?;
    if key.len() != letter.len_utf8() || !letter.is_ascii_alphabetic() {
        return None;
    }
    Some(vec![letter as u8 & 0x1f])
}

fn encode_pty_bytes(key: &str, key_char: Option<&str>, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    let key = key.strip_prefix("ctrl-").unwrap_or(key);
    if modifiers.platform && !modifiers.control {
        return encode_super_edit_bytes(key);
    }
    if modifiers.control && !modifiers.alt && !modifiers.platform {
        if let Some(bytes) = control_letter_bytes(
            key,
            KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
        ) {
            return Some(bytes);
        }
        return match key {
            "[" => Some(vec![0x1b]),
            "\\" => Some(vec![0x1c]),
            "]" => Some(vec![0x1d]),
            "6" | "^" => Some(vec![0x1e]),
            "-" | "_" => Some(vec![0x1f]),
            "/" | "?" => Some(vec![0x7f]),
            "space" => Some(vec![0x00]),
            "backspace" | "back" => Some(vec![0x08]),
            _ => None,
        };
    }
    if modifiers.alt && !modifiers.platform {
        return encode_alt_edit_bytes(key, key_char);
    }
    if modifiers.shift && key == "tab" {
        return Some(b"\x1b[Z".to_vec());
    }
    Some(match key {
        "enter" | "return" => vec![b'\r'],
        "tab" => vec![b'\t'],
        "backspace" | "back" => vec![0x7f],
        "delete" => b"\x1b[3~".to_vec(),
        "escape" => vec![0x1b],
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        _ => {
            if let Some(text) = key_char.filter(|text| !text.is_empty()) {
                return Some(text.as_bytes().to_vec());
            }
            if key.chars().count() == 1 {
                return Some(key.as_bytes().to_vec());
            }
            return None;
        }
    })
}

fn encode_super_edit_bytes(key: &str) -> Option<Vec<u8>> {
    Some(match key {
        "backspace" | "back" => vec![0x15],
        "delete" => vec![0x0b],
        "left" => vec![0x01],
        "right" => vec![0x05],
        "up" => b"\x1b[H".to_vec(),
        "down" => b"\x1b[F".to_vec(),
        _ => return None,
    })
}

fn merge_settings_persist(previous: SettingsPersist, next: SettingsPersist) -> SettingsPersist {
    SettingsPersist {
        error: next.error.or(previous.error),
        host: merge_host_follow_up(previous.host, next.host),
        rollback: previous.rollback.or(next.rollback),
    }
}

fn merge_host_follow_up(
    previous: Option<HostPersistFollowUp>,
    next: Option<HostPersistFollowUp>,
) -> Option<HostPersistFollowUp> {
    match (previous, next) {
        (
            Some(saved @ HostPersistFollowUp::Saved { .. }),
            Some(HostPersistFollowUp::Revertible { .. }),
        ) => Some(saved),
        (previous, None) => previous,
        (_, Some(next)) => Some(next),
    }
}

/// A failed write rolls live state back only when nothing else is queued to
/// save the user's latest host intent. Otherwise the earliest snapshot moves
/// onto that queued request.
fn persist_failure_rollback(
    pending: &mut Option<SettingsPersist>,
    failed_rollback: Option<HostRollback>,
) -> Option<HostRollback> {
    if let Some(queued) = pending.as_mut().filter(|queued| queued.host.is_some()) {
        queued.rollback = failed_rollback.or(queued.rollback.take());
        return None;
    }
    failed_rollback
}

/// Keep one waiting request. Start a write only when none is in flight.
fn enqueue_settings_persist(
    pending: &mut Option<SettingsPersist>,
    in_flight: bool,
    request: SettingsPersist,
) -> Option<SettingsPersist> {
    *pending = Some(match pending.take() {
        Some(previous) => merge_settings_persist(previous, request),
        None => request,
    });
    if in_flight { None } else { pending.take() }
}

fn overlay_confirm_or_cancel(event: &KeyDownEvent) -> Option<bool> {
    if event.is_held || event.keystroke.modifiers.modified() {
        return None;
    }
    match event.keystroke.key.as_str() {
        "enter" | "return" => Some(true),
        "escape" => Some(false),
        _ => None,
    }
}

fn tab_index_from_keystroke(key: &str, key_char: Option<&str>) -> Option<usize> {
    for candidate in [Some(key), key_char].into_iter().flatten() {
        if let Some(digit) = candidate.chars().rev().find_map(digit_from_char) {
            return Some(digit);
        }
    }
    None
}

fn digit_from_char(character: char) -> Option<usize> {
    if character.is_ascii_digit() {
        return Some((character as u8 - b'0') as usize);
    }
    const FULLWIDTH: [char; 10] = ['０', '１', '２', '３', '４', '５', '６', '７', '８', '９'];
    FULLWIDTH.iter().position(|&digit| digit == character)
}

fn tab_id_for_shortcut<'a>(
    tabs: impl Iterator<Item = &'a ocherdr_core::TabInfo>,
    number: usize,
) -> Option<String> {
    let mut tabs = tabs.collect::<Vec<_>>();
    if tabs.is_empty() {
        return None;
    }
    tabs.sort_by_key(|tab| tab.number);
    if number == 0 {
        return tabs.last().map(|tab| tab.tab_id.clone());
    }
    tabs.iter()
        .find(|tab| tab.number == number)
        .or_else(|| tabs.get(number.saturating_sub(1)))
        .map(|tab| tab.tab_id.clone())
}

fn encode_alt_edit_bytes(key: &str, key_char: Option<&str>) -> Option<Vec<u8>> {
    Some(match key {
        "backspace" | "back" => b"\x1b\x7f".to_vec(),
        "delete" => b"\x1bd".to_vec(),
        "left" => b"\x1bb".to_vec(),
        "right" => b"\x1bf".to_vec(),
        "up" => b"\x1b[1;3A".to_vec(),
        "down" => b"\x1b[1;3B".to_vec(),
        "enter" | "return" => b"\x1b\r".to_vec(),
        other if other.chars().count() == 1 => {
            let mut out = vec![0x1b];
            out.extend(other.as_bytes());
            out
        }
        _ => {
            let text = key_char.filter(|text| !text.is_empty())?;
            let mut out = vec![0x1b];
            out.extend(text.as_bytes());
            out
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persist_notice(kind: FailureKind) -> SettingsPersist {
        SettingsPersist {
            error: Some(kind),
            host: None,
            rollback: None,
        }
    }

    fn revertible_persist(kind: FailureKind, tag: &str) -> SettingsPersist {
        SettingsPersist {
            error: Some(kind),
            host: Some(HostPersistFollowUp::Revertible { error: kind }),
            rollback: Some(HostRollback::tagged(tag)),
        }
    }

    #[test]
    fn settings_persist_keeps_only_the_latest_unwritten_value() {
        let mut pending = None;
        let started = enqueue_settings_persist(
            &mut pending,
            false,
            persist_notice(FailureKind::SaveAppearance),
        );
        assert_eq!(
            started.and_then(|request| request.error),
            Some(FailureKind::SaveAppearance)
        );
        assert!(pending.is_none());

        assert!(
            enqueue_settings_persist(
                &mut pending,
                true,
                persist_notice(FailureKind::SaveLanguage)
            )
            .is_none()
        );
        assert!(
            enqueue_settings_persist(
                &mut pending,
                true,
                persist_notice(FailureKind::SaveAppearance)
            )
            .is_none()
        );
        assert_eq!(
            pending.and_then(|request| request.error),
            Some(FailureKind::SaveAppearance)
        );
    }

    #[test]
    fn settings_persist_keeps_a_host_follow_up_when_appearance_replaces_the_waiting_write() {
        let host = HostPersistFollowUp::Revertible {
            error: FailureKind::SaveHost,
        };
        let merged = merge_settings_persist(
            SettingsPersist {
                error: Some(FailureKind::SaveHost),
                host: Some(host),
                rollback: Some(HostRollback::tagged("before-host")),
            },
            persist_notice(FailureKind::SaveAppearance),
        );
        assert_eq!(merged.error, Some(FailureKind::SaveAppearance));
        assert!(matches!(
            merged.host,
            Some(HostPersistFollowUp::Revertible {
                error: FailureKind::SaveHost,
                ..
            })
        ));
        assert_eq!(
            merged.rollback.as_ref().and_then(HostRollback::tag),
            Some("before-host")
        );
    }

    #[test]
    fn merged_revertible_persists_keep_the_earliest_rollback() {
        let merged = merge_settings_persist(
            revertible_persist(FailureKind::UpdateFavorites, "before-first"),
            revertible_persist(FailureKind::ApplyOrganization, "before-second"),
        );
        assert!(matches!(
            merged.host,
            Some(HostPersistFollowUp::Revertible {
                error: FailureKind::ApplyOrganization,
                ..
            })
        ));
        assert_eq!(
            merged.rollback.as_ref().and_then(HostRollback::tag),
            Some("before-first")
        );
    }

    #[test]
    fn a_failed_host_write_keeps_a_queued_write_for_a_different_host() {
        let mut pending = Some(revertible_persist(
            FailureKind::UpdateFavorites,
            "after-alpha-before-beta",
        ));
        let applied =
            persist_failure_rollback(&mut pending, Some(HostRollback::tagged("before-alpha")));
        let pending = pending.expect("beta persist stays queued");
        assert!(applied.is_none());
        assert!(matches!(
            pending.host,
            Some(HostPersistFollowUp::Revertible {
                error: FailureKind::UpdateFavorites,
                ..
            })
        ));
        assert_eq!(
            pending.rollback.as_ref().and_then(HostRollback::tag),
            Some("before-alpha")
        );
    }

    fn test_pane(pane_id: &str, tab_id: &str) -> PaneInfo {
        PaneInfo {
            pane_id: pane_id.into(),
            terminal_id: pane_id.into(),
            workspace_id: "w".into(),
            tab_id: tab_id.into(),
            focused: false,
            cwd: None,
            foreground_cwd: None,
            label: None,
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: None,
            agent_status: AgentStatus::Idle,
            state_labels: HashMap::new(),
            tokens: HashMap::new(),
            revision: 0,
        }
    }

    fn test_tab(tab_id: &str, number: usize, label: &str) -> ocherdr_core::TabInfo {
        ocherdr_core::TabInfo {
            tab_id: tab_id.into(),
            workspace_id: "w".into(),
            number,
            label: label.into(),
            focused: number == 1,
            pane_count: 1,
            agent_status: AgentStatus::Idle,
        }
    }

    fn two_tab_snapshot() -> HierarchySnapshot {
        HierarchySnapshot {
            tabs: vec![test_tab("t-a", 1, "alpha"), test_tab("t-b", 2, "beta")],
            panes: vec![test_pane("p-a", "t-a"), test_pane("p-b", "t-b")],
            layouts: vec![
                ocherdr_core::PaneLayout {
                    workspace_id: "w".into(),
                    tab_id: "t-a".into(),
                    zoomed: false,
                    area: ocherdr_core::LayoutRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 50,
                    },
                    focused_pane_id: "p-a".into(),
                    panes: vec![ocherdr_core::LayoutPane {
                        pane_id: "p-a".into(),
                        focused: true,
                        rect: ocherdr_core::LayoutRect {
                            x: 0,
                            y: 0,
                            width: 100,
                            height: 50,
                        },
                    }],
                    splits: Vec::new(),
                },
                ocherdr_core::PaneLayout {
                    workspace_id: "w".into(),
                    tab_id: "t-b".into(),
                    zoomed: false,
                    area: ocherdr_core::LayoutRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 50,
                    },
                    focused_pane_id: "p-b".into(),
                    panes: vec![ocherdr_core::LayoutPane {
                        pane_id: "p-b".into(),
                        focused: true,
                        rect: ocherdr_core::LayoutRect {
                            x: 0,
                            y: 0,
                            width: 50,
                            height: 50,
                        },
                    }],
                    splits: Vec::new(),
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn cmd_w_closes_the_selected_split_pane() {
        let mut snapshot = two_tab_snapshot();
        snapshot.panes.push(test_pane("p-a2", "t-a"));
        snapshot.tabs[0].pane_count = 2;
        match cmd_w_close_target(&snapshot, "t-a", Some("p-a2")) {
            Some(HierarchyTarget::Pane { id, .. }) => assert_eq!(id, "p-a2"),
            other => panic!("expected pane close, got {other:?}"),
        }
    }

    #[test]
    fn cmd_w_closes_the_tab_when_it_is_the_last_pane() {
        let snapshot = two_tab_snapshot();
        match cmd_w_close_target(&snapshot, "t-a", Some("p-a")) {
            Some(HierarchyTarget::Tab { id, label }) => {
                assert_eq!(id, "t-a");
                assert_eq!(label, "alpha");
            }
            other => panic!("expected tab close, got {other:?}"),
        }
    }

    #[test]
    fn observe_frames_do_not_replace_the_local_display_grid() {
        assert!(!incoming_frame_should_replace_grid((800, 600)));
        assert!(incoming_frame_should_replace_grid((0, 0)));
    }

    #[test]
    fn session_targets_every_snapshot_pane_and_only_selects_one_control() {
        let snapshot = two_tab_snapshot();
        let targets = snapshot_runtime_targets(&snapshot, Some("p-a"));
        assert_eq!(
            targets,
            vec![
                ("p-a".into(), TerminalMode::ControlTakeover),
                ("p-b".into(), TerminalMode::Observe),
            ]
        );
        let switched = snapshot_runtime_targets(&snapshot, Some("p-b"));
        assert_eq!(
            switched,
            vec![
                ("p-a".into(), TerminalMode::Observe),
                ("p-b".into(), TerminalMode::ControlTakeover),
            ]
        );
        assert_eq!(
            snapshot_pane_ids(&snapshot),
            ["p-a", "p-b"]
                .into_iter()
                .map(str::to_owned)
                .collect::<HashSet<_>>()
        );
    }

    #[test]
    fn tab_switch_flushes_visible_or_newly_spawned_panes() {
        assert!(should_flush_session_pane(Some("t-a"), Some("t-a"), false));
        assert!(!should_flush_session_pane(Some("t-b"), Some("t-a"), false));
        assert!(should_flush_session_pane(Some("t-b"), Some("t-a"), true));
        assert!(should_flush_session_pane(Some("t-b"), None, true));
    }

    #[test]
    fn tab_switch_keeps_snapshot_panes_instead_of_only_the_visible_tab() {
        let cached = ["p-a", "p-b", "closed"]
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let snapshot = two_tab_snapshot();
        let live = snapshot_pane_ids(&snapshot);
        let kept = cached.intersection(&live).cloned().collect::<HashSet<_>>();
        assert!(kept.contains("p-a"));
        assert!(kept.contains("p-b"));
        assert!(!kept.contains("closed"));
    }

    #[test]
    fn switching_the_selected_pane_keeps_the_local_surface() {
        assert_eq!(
            visible_pane_plan(Some(TerminalMode::Observe), false, TerminalMode::Observe),
            VisiblePanePlan::Keep
        );
        assert_eq!(
            visible_pane_plan(
                Some(TerminalMode::ControlTakeover),
                false,
                TerminalMode::ControlTakeover
            ),
            VisiblePanePlan::Keep
        );
        assert_eq!(
            visible_pane_plan(
                Some(TerminalMode::Observe),
                false,
                TerminalMode::ControlTakeover
            ),
            VisiblePanePlan::PromoteToControl
        );
        assert_eq!(
            visible_pane_plan(
                Some(TerminalMode::ControlTakeover),
                false,
                TerminalMode::Observe
            ),
            VisiblePanePlan::DemoteToObserve
        );
        assert_eq!(
            visible_pane_plan(None, false, TerminalMode::Observe),
            VisiblePanePlan::Spawn
        );
        assert_ne!(
            visible_pane_plan(
                Some(TerminalMode::Observe),
                false,
                TerminalMode::ControlTakeover
            ),
            VisiblePanePlan::Spawn
        );
        assert_ne!(
            visible_pane_plan(
                Some(TerminalMode::ControlTakeover),
                false,
                TerminalMode::Observe
            ),
            VisiblePanePlan::Spawn
        );
    }

    #[test]
    fn switching_sessions_does_not_reuse_the_previous_session_panes() {
        let current = SessionKey {
            profile_id: "local".into(),
            session_name: "work".into(),
        };
        let incoming = SessionKey {
            profile_id: "local".into(),
            session_name: "other".into(),
        };
        assert_eq!(
            session_panes_plan(Some(&current), &incoming),
            SessionPanesPlan::Replace
        );
    }

    #[test]
    fn reloading_the_same_session_keeps_existing_session_panes() {
        let owner = SessionKey {
            profile_id: "local".into(),
            session_name: "work".into(),
        };
        assert_eq!(
            session_panes_plan(Some(&owner), &owner),
            SessionPanesPlan::Keep
        );
    }

    #[test]
    fn a_closed_stream_is_respawned_instead_of_kept() {
        assert_eq!(
            visible_pane_plan(
                Some(TerminalMode::ControlTakeover),
                true,
                TerminalMode::ControlTakeover
            ),
            VisiblePanePlan::Spawn
        );
        assert_eq!(
            visible_pane_plan(Some(TerminalMode::Observe), true, TerminalMode::Observe),
            VisiblePanePlan::Spawn
        );
    }

    #[test]
    fn a_process_exit_closes_the_stream_without_an_app_error() {
        assert!(is_expected_terminal_exit(&HerdrError::TerminalClosed(
            "terminal t1 exited".into()
        )));
        assert!(is_expected_terminal_exit(&HerdrError::TerminalClosed(
            "terminal worker stopped".into()
        )));
        assert!(!is_expected_terminal_exit(&HerdrError::Protocol(
            "frame gap".into()
        )));
    }

    #[test]
    fn snapshot_refresh_queues_when_one_is_already_in_flight() {
        assert!(snapshot_refresh_should_queue(true));
        assert!(!snapshot_refresh_should_queue(false));
    }

    #[test]
    fn snapshot_handoff_releases_only_when_no_refresh_is_in_flight() {
        assert!(snapshot_handoff_should_release(false));
        assert!(!snapshot_handoff_should_release(true));
    }

    #[test]
    fn invoke_resyncs_only_commands_that_do_not_emit_events() {
        assert!(command_needs_snapshot_resync("pane.rename"));
        assert!(command_needs_snapshot_resync("pane.close"));
        for method in [
            "workspace.create",
            "workspace.close",
            "workspace.rename",
            "tab.create",
            "tab.close",
            "tab.rename",
            "pane.split",
            "pane.zoom",
            "pane.focus_direction",
        ] {
            assert!(
                !command_needs_snapshot_resync(method),
                "{method} is pushed back as an event and must not reload the snapshot"
            );
        }
    }

    #[test]
    fn a_rejected_subscription_is_lost_instead_of_idle() {
        let loaded = LoadedEvents::from_subscribe(Err(HerdrError::Api {
            code: "unknown_type".into(),
            message: "events.subscribe rejected".into(),
        }));
        let LoadedEvents::Lost(detail) = loaded else {
            panic!("a failed subscribe is Lost, not Idle");
        };
        assert!(detail.contains("events.subscribe rejected"));
    }

    #[test]
    fn a_successful_subscription_is_live() {
        let (_tx, rx) = futures::channel::mpsc::unbounded();
        let loaded = LoadedEvents::from_subscribe(Ok(EventSubscription::new(rx)));
        assert!(matches!(loaded, LoadedEvents::Live(_)));
    }

    #[test]
    fn a_dead_event_stream_is_marked_lost_instead_of_idle() {
        let next = poll_event_stream(&mut HierarchySnapshot::default(), || {
            Err(HerdrError::EventStreamClosed("event worker stopped".into()))
        })
        .event_stream();
        assert!(
            matches!(next, Some(EventStreamState::Lost(_))),
            "a closed subscription must become Lost, not Idle"
        );
    }

    #[test]
    fn a_dead_event_stream_does_not_reschedule_the_poll() {
        let action = poll_event_stream(&mut HierarchySnapshot::default(), || {
            Err(HerdrError::EventStreamClosed("event worker stopped".into()))
        });
        assert!(
            !effects_for(&action).reschedule,
            "polling a closed stream has nothing left to wait for"
        );
    }

    #[test]
    fn a_quiet_live_stream_keeps_polling_without_refreshing() {
        let action = poll_event_stream(&mut HierarchySnapshot::default(), || Ok(None));
        assert_eq!(action, EventPollAction::Idle);
        assert!(effects_for(&action).reschedule);
        assert!(action.event_stream().is_none());
    }

    #[test]
    fn closing_the_selected_last_tab_resyncs_instead_of_selecting_the_first_remaining_tab() {
        let mut snapshot = two_tab_snapshot();
        snapshot.tabs.push(test_tab("t-c", 3, "gamma"));
        snapshot.panes.push(test_pane("p-c", "t-c"));
        snapshot.focused_workspace_id = Some("w".into());
        snapshot.focused_tab_id = Some("t-c".into());
        snapshot.focused_pane_id = Some("p-c".into());
        snapshot.workspaces.push(ocherdr_core::WorkspaceInfo {
            workspace_id: "w".into(),
            number: 1,
            label: "one".into(),
            focused: true,
            pane_count: 3,
            tab_count: 3,
            active_tab_id: "t-c".into(),
            agent_status: AgentStatus::Idle,
            tokens: HashMap::new(),
            worktree: None,
        });
        let mut selection = Selection {
            connection_id: "local".into(),
            workspace_id: Some("w".into()),
            tab_id: Some("t-c".into()),
            pane_id: Some("p-c".into()),
            session_name: None,
        };
        let mut events = vec![
            Ok(Some(HerdrEvent::PaneClosed {
                workspace_id: "w".into(),
                pane_id: "p-c".into(),
            })),
            Ok(None),
        ]
        .into_iter();
        let action = apply_event_stream(&mut snapshot, &mut selection, || events.next().unwrap());
        assert_eq!(selection.tab_id.as_deref(), Some("t-c"));
        assert_eq!(action, EventPollAction::Resync { error: None });
    }

    #[test]
    fn pane_updated_is_applied_without_resyncing_the_snapshot() {
        let mut snapshot = two_tab_snapshot();
        snapshot.panes[0].revision = 1;
        let mut updated = snapshot.panes[0].clone();
        updated.revision = 9;
        let mut events = vec![
            Ok(Some(HerdrEvent::PaneUpdated {
                pane: updated.clone(),
            })),
            Ok(None),
        ]
        .into_iter();
        let action = poll_event_stream(&mut snapshot, || events.next().unwrap());
        assert_eq!(action, EventPollAction::Applied);
        assert!(!effects_for(&action).resync);
        assert!(effects_for(&action).apply_local);
        assert!(effects_for(&action).reschedule);
        assert!(action.event_stream().is_none());
        assert_eq!(snapshot.panes[0], updated);
    }

    #[test]
    fn applied_poll_effects_do_not_resync() {
        let applied = effects_for(&EventPollAction::Applied);
        assert!(!applied.resync);
        assert!(applied.apply_local);
        assert!(applied.notify);
        assert!(applied.reschedule);
        assert!(applied.error.is_none());
        let resync = effects_for(&EventPollAction::Resync { error: None });
        assert!(resync.resync);
        assert!(!resync.apply_local);
        assert!(resync.reschedule);
    }

    #[test]
    fn a_malformed_event_resyncs_without_dropping_the_stream() {
        let mut events = vec![
            Err(HerdrError::Protocol("event is missing `data`".into())),
            Ok(None),
        ]
        .into_iter();
        let action =
            poll_event_stream(&mut HierarchySnapshot::default(), || events.next().unwrap());
        let EventPollAction::Resync { error } = &action else {
            panic!("payload errors must resync, got {action:?}");
        };
        assert!(
            error
                .as_ref()
                .is_some_and(|detail| detail.contains("`data`"))
        );
        let effects = effects_for(&action);
        assert!(effects.resync);
        assert!(effects.error.is_some());
        assert!(effects.reschedule);
        assert!(action.event_stream().is_none());
    }

    #[test]
    fn wheel_delta_accumulates_into_terminal_scroll_lines() {
        assert_eq!(
            wheel_scroll_lines(ScrollDelta::Lines(point(0., 3.)), 16., &mut 0.),
            3
        );
        assert_eq!(
            wheel_scroll_lines(ScrollDelta::Lines(point(0., -2.4)), 16., &mut 0.),
            -2
        );
        let mut leftover = 0.;
        assert_eq!(
            wheel_scroll_lines(
                ScrollDelta::Pixels(point(px(0.), px(8.))),
                16.,
                &mut leftover
            ),
            0
        );
        assert!((leftover - 8.).abs() < f32::EPSILON);
        assert_eq!(
            wheel_scroll_lines(
                ScrollDelta::Pixels(point(px(0.), px(10.))),
                16.,
                &mut leftover
            ),
            1
        );
        assert!((leftover - 2.).abs() < f32::EPSILON);
    }

    #[test]
    fn terminal_palette_follows_the_gui_light_and_dark_theme() {
        let family = ochub_ui::theme::ochub_family();
        let font = TerminalFontSettings::default();
        let light = terminal_palette_from_theme(family.light, false, &font);
        let dark = terminal_palette_from_theme(family.dark, true, &font);
        assert!(!light.dark);
        assert!(dark.dark);
        assert_eq!(light.background, family.light.bg.0);
        assert_eq!(light.foreground, family.light.text.0);
        assert_eq!(dark.background, family.dark.bg.0);
        assert_ne!(light.background, dark.background);
        assert_ne!(light.background, 0x1E1E1E);
        assert_ne!(light.signature(), dark.signature());
        assert_eq!(light.font_size, 13);
        assert!(light.ligatures);
        assert!(light.font_family.is_empty());
    }

    #[test]
    fn terminal_font_settings_change_the_ghostty_signature() {
        let family = ochub_ui::theme::ochub_family();
        let default = TerminalFontSettings::default();
        let menlo = TerminalFontSettings {
            family: "Menlo".into(),
            size: FontSizeChoice::Pt16,
            ligatures: false,
            thicken: true,
            cell_width_percent: CellWidthChoice::Tight,
            cell_height_percent: CellHeightChoice::Relaxed,
        };
        let left = terminal_palette_from_theme(family.light, false, &default);
        let right = terminal_palette_from_theme(family.light, false, &menlo);
        assert_eq!(right.font_family, "Menlo");
        assert_eq!(right.font_size, 16);
        assert!(!right.ligatures);
        assert!(right.thicken);
        assert_ne!(left.signature(), right.signature());
    }

    #[test]
    fn control_letter_bytes_encode_ctrl_c() {
        let ctrl = KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        };
        assert_eq!(control_letter_bytes("c", ctrl), Some(vec![0x03]));
        assert_eq!(control_letter_bytes("C", ctrl), Some(vec![0x03]));
        assert_eq!(control_letter_bytes("d", ctrl), Some(vec![0x04]));
        assert_eq!(control_letter_bytes("c", KeyModifiers::default()), None);
        assert_eq!(
            control_letter_bytes(
                "c",
                KeyModifiers {
                    control: true,
                    platform: true,
                    ..KeyModifiers::default()
                }
            ),
            None
        );
        assert_eq!(control_letter_bytes("enter", ctrl), None);
    }

    #[test]
    fn encode_pty_bytes_pass_through_ctrl_c_and_q() {
        let none = KeyModifiers::default();
        let ctrl = KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        };
        assert_eq!(encode_pty_bytes("c", Some("c"), ctrl), Some(vec![0x03]));
        assert_eq!(encode_pty_bytes("ctrl-c", None, ctrl), Some(vec![0x03]));
        assert_eq!(encode_pty_bytes("q", Some("q"), none), Some(b"q".to_vec()));
        assert_eq!(encode_pty_bytes("enter", None, none), Some(vec![b'\r']));
        assert_eq!(encode_pty_bytes("up", None, none), Some(b"\x1b[A".to_vec()));
    }

    #[test]
    fn encode_pty_bytes_maps_macos_edit_shortcuts() {
        let super_key = KeyModifiers {
            platform: true,
            ..KeyModifiers::default()
        };
        let alt = KeyModifiers {
            alt: true,
            ..KeyModifiers::default()
        };
        let shift = KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        };
        assert_eq!(
            encode_pty_bytes("backspace", None, super_key),
            Some(vec![0x15])
        );
        assert_eq!(
            encode_pty_bytes("delete", None, super_key),
            Some(vec![0x0b])
        );
        assert_eq!(encode_pty_bytes("left", None, super_key), Some(vec![0x01]));
        assert_eq!(encode_pty_bytes("right", None, super_key), Some(vec![0x05]));
        assert_eq!(
            encode_pty_bytes("backspace", None, alt),
            Some(b"\x1b\x7f".to_vec())
        );
        assert_eq!(encode_pty_bytes("left", None, alt), Some(b"\x1bb".to_vec()));
        assert_eq!(
            encode_pty_bytes("right", None, alt),
            Some(b"\x1bf".to_vec())
        );
        assert_eq!(
            encode_pty_bytes("delete", None, alt),
            Some(b"\x1bd".to_vec())
        );
        assert_eq!(
            encode_pty_bytes("tab", None, shift),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_pty_bytes("v", Some("v"), super_key),
            None,
            "Cmd+V stays with the clipboard paste path"
        );
    }

    #[test]
    fn overlay_enter_confirms_and_escape_cancels() {
        let enter = KeyDownEvent {
            keystroke: ochub_ui::gpui::Keystroke {
                key: "enter".into(),
                key_char: Some("\n".into()),
                modifiers: ochub_ui::gpui::Modifiers::default(),
            },
            is_held: false,
            prefer_character_input: false,
        };
        let held = KeyDownEvent {
            is_held: true,
            ..enter.clone()
        };
        let escape = KeyDownEvent {
            keystroke: ochub_ui::gpui::Keystroke {
                key: "escape".into(),
                key_char: None,
                modifiers: ochub_ui::gpui::Modifiers::default(),
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert_eq!(overlay_confirm_or_cancel(&enter), Some(true));
        assert_eq!(overlay_confirm_or_cancel(&escape), Some(false));
        assert_eq!(overlay_confirm_or_cancel(&held), None);
    }

    #[test]
    fn tab_index_reads_plain_digit_and_named_keys() {
        assert_eq!(tab_index_from_keystroke("1", None), Some(1));
        assert_eq!(tab_index_from_keystroke("9", Some("9")), Some(9));
        assert_eq!(tab_index_from_keystroke("0", None), Some(0));
        assert_eq!(tab_index_from_keystroke("digit3", None), Some(3));
        assert_eq!(tab_index_from_keystroke("numpad8", None), Some(8));
        assert_eq!(tab_index_from_keystroke("t", None), None);
        assert_eq!(tab_index_from_keystroke("w", Some("w")), None);
        assert_eq!(tab_index_from_keystroke("１", None), Some(1));
    }

    #[test]
    fn tab_shortcut_uses_number_then_visual_index_and_zero_for_last() {
        let tabs = [
            ocherdr_core::TabInfo {
                tab_id: "first".into(),
                workspace_id: "w".into(),
                number: 1,
                label: "one".into(),
                focused: true,
                pane_count: 1,
                agent_status: AgentStatus::Idle,
            },
            ocherdr_core::TabInfo {
                tab_id: "third".into(),
                workspace_id: "w".into(),
                number: 3,
                label: "three".into(),
                focused: false,
                pane_count: 1,
                agent_status: AgentStatus::Idle,
            },
            ocherdr_core::TabInfo {
                tab_id: "second".into(),
                workspace_id: "w".into(),
                number: 2,
                label: "two".into(),
                focused: false,
                pane_count: 1,
                agent_status: AgentStatus::Idle,
            },
        ];
        assert_eq!(
            tab_id_for_shortcut(tabs.iter(), 1).as_deref(),
            Some("first")
        );
        assert_eq!(
            tab_id_for_shortcut(tabs.iter(), 2).as_deref(),
            Some("second")
        );
        assert_eq!(
            tab_id_for_shortcut(tabs.iter(), 3).as_deref(),
            Some("third")
        );
        assert_eq!(
            tab_id_for_shortcut(tabs.iter(), 0).as_deref(),
            Some("third")
        );
        assert_eq!(tab_id_for_shortcut(tabs.iter(), 8).as_deref(), None);
    }

    #[test]
    fn surface_rect_mapping_inverts_mouse_mapping() {
        let body = (100., 50., 800., 400.);
        let pixel_size = (1600, 800);
        let mouse = map_mouse_to_surface((500., 250.), body, pixel_size, 2.).unwrap();
        let rect =
            map_surface_rect_to_window((mouse.0, mouse.1, 10., 16.), body, pixel_size, 2.).unwrap();
        assert!((rect.0 - 500.).abs() < 0.02);
        assert!((rect.1 - 250.).abs() < 0.02);
        assert!((rect.2 - 10.).abs() < 0.02);
        assert!((rect.3 - 16.).abs() < 0.02);
    }

    #[test]
    fn mouse_to_surface_fills_a_matching_retina_framebuffer() {
        let body = (100., 50., 800., 400.);
        let pixel_size = (1600, 800);
        assert_eq!(
            map_mouse_to_surface((100., 50.), body, pixel_size, 2.),
            Some((0., 0.))
        );
        let bottom_right = map_mouse_to_surface((900., 450.), body, pixel_size, 2.).unwrap();
        assert!((bottom_right.0 - 800.).abs() < 0.01);
        assert!((bottom_right.1 - 400.).abs() < 0.01);
        assert_eq!(map_mouse_to_surface((100., 50.), body, (0, 0), 2.), None);
    }

    #[test]
    fn mouse_to_surface_accounts_for_contain_letterboxing() {
        let body = (0., 0., 1000., 400.);
        let mapped = map_mouse_to_surface((100., 0.), body, (1600, 800), 2.).unwrap();
        assert!(mapped.0.abs() < 0.01);
        assert!(mapped.1.abs() < 0.01);
        let mapped = map_mouse_to_surface((900., 400.), body, (1600, 800), 2.).unwrap();
        assert!((mapped.0 - 800.).abs() < 0.01);
        assert!((mapped.1 - 400.).abs() < 0.01);
    }

    #[test]
    fn mouse_to_surface_uses_measured_window_bounds_without_chrome_offsets() {
        let body = (300., 80., 800., 400.);
        assert_eq!(
            map_mouse_to_surface((300., 80.), body, (1600, 800), 2.),
            Some((0., 0.))
        );
        let bottom_right = map_mouse_to_surface((1100., 480.), body, (1600, 800), 2.).unwrap();
        assert!((bottom_right.0 - 800.).abs() < 0.01);
        assert!((bottom_right.1 - 400.).abs() < 0.01);
    }
}
