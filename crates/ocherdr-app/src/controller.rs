use std::collections::HashSet;
use std::task::Poll;

use ocherdr_core::{WorkspaceInfo, WorktreeInfo, WorktreeList, WorktreeSourceInfo};

use futures::channel::mpsc::UnboundedReceiver;
use futures::future::{self, Either, poll_fn};
use futures::pin_mut;
use ocherdr_core::{
    AGENT_OUTPUT_SOURCE, AGENT_STATUS_HANDOFF_LIMIT, AgentStatusHandoff,
    agent_output_should_refresh, agent_status_panes_after_stream_closed,
    agent_status_stream_should_rebuild, event_panes_after_failed_subscribe, parse_agent_name,
    reorder_hover_along_axis, reorder_insert_index,
};
use ocherdr_herdr::{TerminalFrame, next_batch, subscribe_agent_status};

use ochub_ui::notifications::NotificationHost;

use super::*;
use crate::notify::{FailureKind, FailureNotice, command_notification, notification_for};

impl OcHerdrView {
    #[cfg(test)]
    pub(super) fn new(settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with(
            crate::config::LoadedApp {
                settings,
                appearance: AppearanceSettings::default(),
                language: Language::default(),
                document: crate::config::ConfigDocument::new(),
            },
            window,
            cx,
        )
    }

    pub(super) fn new_with(
        loaded: crate::config::LoadedApp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let i18n = I18n::new(loaded.language);
        let appearance = loaded.appearance;
        let settings = loaded.settings;
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
            surface_drag: SurfaceDrag::Idle,
            pending_reorder: None,
            reorder_metrics: ReorderMetrics::default(),
            terminal_surface_bounds: None,
            ime_marked: None,
            rename_input: cx.new(|cx| TextInput::new(cx, i18n.text(k::COMMON_NAME))),
            worktree_label_input: cx
                .new(|cx| TextInput::new(cx, i18n.text(k::WORKTREE_FIELD_LABEL_HINT))),
            worktree_branch_input: cx
                .new(|cx| TextInput::new(cx, i18n.text(k::WORKTREE_FIELD_BRANCH_HINT))),
            worktree_base_input: cx
                .new(|cx| TextInput::new(cx, i18n.text(k::WORKTREE_FIELD_BASE_HINT))),
            worktree_path_input: cx
                .new(|cx| TextInput::new(cx, i18n.text(k::WORKTREE_FIELD_PATH_HINT))),
            agent_name_input: cx.new(|cx| TextInput::new(cx, i18n.text(k::COMMON_NAME))),
            agent_prompt_input: cx.new(|cx| {
                TextInput::new(cx, i18n.text(k::AGENT_PROMPT_PLACEHOLDER)).multiline(true)
            }),
            agent_output_scroll: ScrollHandle::new(),
            agent_name: AgentNameState::Idle,
            agent_output: AgentOutputState::Idle,
            agent_prompts: HashMap::new(),
            agent_name_error: None,
            agent_keys: HashMap::new(),
            agent_renames: HashMap::new(),
            worktree_list_task: None,
            appearance,
            config: loaded.document,
            i18n,
            host_center,
            pending_persist: None,
            persist_task: None,
        };
        let host = cx.weak_entity();
        bind_enter_submit(&view.rename_input, host.clone(), cx, |this, window, cx| {
            this.submit_rename(window, cx);
        });
        bind_enter_submit(
            &view.worktree_label_input,
            host.clone(),
            cx,
            |this, window, cx| this.submit_worktree_create(window, cx),
        );
        bind_enter_submit(&view.agent_name_input, host, cx, |this, window, cx| {
            this.submit_agent_rename(window, cx);
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
        let kind = match method {
            "layout.set_split_ratio" => Some(FailureKind::SetSplitRatio),
            "workspace.move" => Some(FailureKind::MoveWorkspace),
            "tab.move" => Some(FailureKind::MoveTab),
            _ => None,
        };
        if let Some(kind) = kind {
            self.notify_failure(kind, detail, cx);
            return;
        }
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
        self.surface_drag = SurfaceDrag::Idle;
        self.snapshot_refreshing = false;
        self.snapshot_refresh_pending = false;
        self.abandon_worktree_list();
        if matches!(self.overlay, Overlay::AgentPanel { .. }) {
            self.overlay = Overlay::None;
            self.reset_agent_panel_state();
        }
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
                this.abandon_worktree_list();
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
        self.abandon_worktree_list();
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
        let settings =
            crate::host_center::assemble_settings(&self.host_center.read(cx).persist_state());
        let document = self.config.clone();
        self.persist_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    crate::host_center::write_settings(&settings)?;
                    let paths = crate::config::AppPaths::user()
                        .ok_or_else(|| "Application Support directory is unavailable".to_owned())?;
                    crate::config::write_config(&paths, &document)
                })
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

    fn abandon_worktree_list(&mut self) {
        self.worktree_list_task = None;
        self.overlay = overlay_after_abandoning_worktree_list(self.overlay.clone());
    }

    fn set_overlay(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        if !matches!(overlay, Overlay::WorktreeOpen(_)) {
            self.worktree_list_task = None;
        }
        let leaving_agent_panel = matches!(self.overlay, Overlay::AgentPanel { .. })
            && !matches!(overlay, Overlay::AgentPanel { .. });
        if leaving_agent_panel {
            self.reset_agent_panel_state();
        }
        if !matches!(overlay, Overlay::None) {
            self.cancel_reorder_drag();
        }
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
                apply_event_stream(snapshot, &mut self.selection, || match items.next() {
                    Some(Ok(event)) => Ok(Some(event)),
                    Some(Err(err)) => Err(err),
                    None => Ok(None),
                })
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
        }
        if effects.settle_reorder {
            self.pending_reorder = None;
        }
        if effects.notify {
            cx.notify();
        }
        if matches!(action, EventPollAction::Disconnect(_)) {
            self.cancel_split_drag();
            self.cancel_reorder_drag();
        }
        self.reconcile_split_drag(cx);
        self.reconcile_reorder_drag(cx);
        if let Some(stream) = action.event_stream() {
            self.event_stream = stream;
            cx.notify();
        }
        self.ensure_agent_status_stream(cx);
        self.reconcile_open_agent_panel(panel_refresh, cx);
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
        if resync {
            self.resync_snapshot(self.event_epoch, cx);
        }
        self.reconcile_open_agent_panel(panel_refresh, cx);
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
                        this.reconcile_split_drag(cx);
                        this.reconcile_reorder_drag(cx);
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
        self.config.set(
            "theme",
            &crate::config::values::ThemeRef::Name(family_id.clone()).to_config(),
        );
        self.appearance.theme_family = family_id;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_appearance_mode(
        &mut self,
        mode: AppearanceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set("appearance-mode", mode.as_config());
        self.appearance.mode = mode;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_backdrop_mode(
        &mut self,
        backdrop: BackdropMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set("window-backdrop", backdrop.as_config());
        self.appearance.backdrop = backdrop;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_background_opacity(
        &mut self,
        opacity: OpacityChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set(
            "background-opacity",
            &crate::config::format_opacity_percent(opacity.value()),
        );
        self.appearance.background_opacity = opacity;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_font_family(
        &mut self,
        family: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if family.is_empty() {
            self.config.set_repeatable("font-family", &[]);
        } else {
            self.config
                .set_repeatable("font-family", std::slice::from_ref(&family));
        }
        self.appearance.font.family = family;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_font_size(
        &mut self,
        size: FontSizeChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set("font-size", &size.value().to_string());
        self.appearance.font.size = size;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_font_ligatures(
        &mut self,
        ligatures: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut features: Vec<String> = self
            .config
            .get_all("font-feature")
            .into_iter()
            .filter(|feature| !matches!(feature.trim(), "-liga" | "-calt" | "-dlig"))
            .map(str::to_owned)
            .collect();
        if !ligatures {
            features.extend(["-calt".to_owned(), "-liga".to_owned(), "-dlig".to_owned()]);
        }
        self.config.set_repeatable("font-feature", &features);
        self.appearance.font.ligatures = ligatures;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_font_thicken(
        &mut self,
        thicken: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config
            .set("font-thicken", if thicken { "true" } else { "false" });
        self.appearance.font.thicken = thicken;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_cell_width(
        &mut self,
        percent: CellWidthChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set(
            "adjust-cell-width",
            &crate::config::values::MetricModifier::Percent(f64::from(percent.value())).to_config(),
        );
        self.appearance.font.cell_width_percent = percent;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_cell_height(
        &mut self,
        percent: CellHeightChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set(
            "adjust-cell-height",
            &crate::config::values::MetricModifier::Percent(f64::from(percent.value())).to_config(),
        );
        self.appearance.font.cell_height_percent = percent;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.config.set("language", language.as_config());
        self.i18n.set_preference(language);
        theme::reload_registry();
        let i18n = self.i18n;
        self.host_center
            .update(cx, |center, cx| center.apply_language(i18n, cx));
        self.rename_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::COMMON_NAME), cx)
        });
        self.worktree_label_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::WORKTREE_FIELD_LABEL_HINT), cx)
        });
        self.worktree_branch_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::WORKTREE_FIELD_BRANCH_HINT), cx)
        });
        self.worktree_base_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::WORKTREE_FIELD_BASE_HINT), cx)
        });
        self.worktree_path_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::WORKTREE_FIELD_PATH_HINT), cx)
        });
        self.agent_name_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::COMMON_NAME), cx)
        });
        self.agent_prompt_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::AGENT_PROMPT_PLACEHOLDER), cx)
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
            (Overlay::ConfirmRemoveWorktree { .. }, true) => self.confirm_remove_worktree(cx),
            (Overlay::ConfirmRemoveWorktree { .. }, false) => self.cancel_remove_worktree(cx),
            (Overlay::WorktreeCreate { .. }, true) => self.submit_worktree_create(window, cx),
            (Overlay::WorktreeCreate { .. }, false) => self.close_worktree_overlay(window, cx),
            (Overlay::WorktreeOpen(_), false) => self.close_worktree_overlay(window, cx),
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
            (Overlay::AgentPanel { .. }, false) => self.close_agent_panel(window, cx),
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
        self.abandon_worktree_list();
        self.session_index = Some(index);
        self.cancel_split_drag();
        self.cancel_reorder_drag();
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
        self.cancel_split_drag();
        self.cancel_reorder_drag();
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
        self.cancel_split_drag();
        self.cancel_reorder_drag();
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
        let leave_split_tab = match (&self.surface_drag, pane_context.as_ref()) {
            (SurfaceDrag::Split(drag), context) => {
                let (workspace_id, tab_id) = match context {
                    Some((workspace_id, tab_id)) => {
                        (Some(workspace_id.as_str()), Some(tab_id.as_str()))
                    }
                    None => (None, None),
                };
                split_drag_voided_by_pane(drag, workspace_id, tab_id)
            }
            _ => false,
        };
        if leave_split_tab {
            self.cancel_split_drag();
        }
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

    pub(super) fn open_agent_panel(
        &mut self,
        pane_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_pane(pane_id.clone(), window, cx);
        let same =
            matches!(&self.overlay, Overlay::AgentPanel { pane_id: open } if open == &pane_id);
        if !same {
            self.reset_agent_panel_state();
            self.agent_name_input
                .update(cx, |input, cx| input.set_content("", cx));
            self.set_overlay(Overlay::AgentPanel { pane_id }, cx);
        }
        self.fetch_agent_name(cx);
        self.fetch_agent_output(cx);
        self.agent_prompt_input
            .update(cx, |input, cx| input.set_content("", cx));
        self.agent_prompt_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    pub(super) fn close_agent_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::AgentPanel { .. }) {
            self.set_overlay(Overlay::None, cx);
        }
        self.focus.focus(window, cx);
    }

    pub(super) fn submit_agent_prompt(&mut self, cx: &mut Context<Self>) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        let pane_id = pane_id.clone();
        if matches!(
            self.agent_prompts.get(&pane_id),
            Some(AgentPromptPhase::Sending { .. })
        ) {
            return;
        }
        let raw = self.agent_prompt_input.read(cx).content();
        let Some(text) = agent_prompt_text_to_send(raw.as_ref()) else {
            self.post_notice(
                FailureNotice {
                    level: ochub_ui::notifications::NotificationLevel::Warning,
                    title: self.i18n.text(k::AGENT_PROMPT_SEND).to_owned(),
                    message: self.i18n.text(k::AGENT_PROMPT_EMPTY).to_owned(),
                },
                cx,
            );
            return;
        };
        let Some(connection) = &self.connection else {
            return;
        };
        let socket = connection.socket_path().to_owned();
        let sent_text = text.clone();
        let params = json!({ "target": pane_id, "text": text });
        let target = pane_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "agent.prompt", params) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        this.agent_prompts
                            .insert(target.clone(), AgentPromptPhase::Sent);
                        if agent_panel_pane(&this.overlay) == Some(target.as_str())
                            && this.agent_prompt_input.read(cx).content().as_ref() == sent_text
                        {
                            this.agent_prompt_input
                                .update(cx, |input, cx| input.set_content("", cx));
                        }
                        if agent_panel_pane(&this.overlay) == Some(target.as_str()) {
                            this.fetch_agent_output(cx);
                        }
                    }
                    Err(HerdrError::Api { code, message }) if code == "agent_blocked" => {
                        this.agent_prompts.insert(
                            target.clone(),
                            AgentPromptPhase::Blocked {
                                message: message.clone(),
                            },
                        );
                        this.notify_failure(
                            FailureKind::AgentBlocked,
                            this.i18n.text(k::AGENT_BLOCKED_DETAIL),
                            cx,
                        );
                    }
                    Err(error) => {
                        let message = error.to_string();
                        this.agent_prompts.insert(
                            target.clone(),
                            AgentPromptPhase::Failed {
                                message: message.clone(),
                            },
                        );
                        this.notify_command_failure("agent.prompt", message, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.agent_prompts
            .insert(pane_id, AgentPromptPhase::Sending { _task: task });
        cx.notify();
    }

    pub(super) fn refresh_agent_output(&mut self, cx: &mut Context<Self>) {
        self.fetch_agent_output(cx);
    }

    pub(super) fn submit_agent_rename(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        if self.agent_renames.contains_key(pane_id)
            || !matches!(self.agent_name, AgentNameState::Ready)
        {
            return;
        }
        let pane_id = pane_id.clone();
        let raw = self.agent_name_input.read(cx).content();
        match parse_agent_name(raw.as_ref()) {
            Err(error) => {
                self.agent_name_error = Some(error);
                cx.notify();
            }
            Ok(name) => {
                self.agent_name_error = None;
                let Some(connection) = &self.connection else {
                    return;
                };
                let socket = connection.socket_path().to_owned();
                let params = match name {
                    Some(name) => json!({ "target": pane_id, "name": name }),
                    None => json!({ "target": pane_id }),
                };
                let target = pane_id.clone();
                let task = cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_spawn(
                            async move { request_socket(&socket, "agent.rename", params) },
                        )
                        .await;
                    this.update(cx, |this, cx| {
                        this.agent_renames.remove(&target);
                        match result {
                            Ok(value) => match parse_agent_info_result(value) {
                                Ok(agent) => {
                                    if agent_panel_pane(&this.overlay) == Some(target.as_str()) {
                                        this.agent_name = AgentNameState::Ready;
                                        this.agent_name_input.update(cx, |input, cx| {
                                            input.set_content(
                                                agent.name.as_deref().unwrap_or(""),
                                                cx,
                                            )
                                        });
                                    }
                                    this.resync_snapshot(this.event_epoch, cx);
                                }
                                Err(message) => {
                                    this.notify_command_failure("agent.rename", message, cx)
                                }
                            },
                            Err(error) => this.notify_command_failure("agent.rename", error, cx),
                        }
                        cx.notify();
                    })
                    .ok();
                });
                self.agent_renames.insert(pane_id, task);
                cx.notify();
            }
        }
    }

    pub(super) fn send_agent_keys(
        &mut self,
        keys: &'static [&'static str],
        cx: &mut Context<Self>,
    ) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        if self.agent_keys.contains_key(pane_id) {
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        let pane_id = pane_id.clone();
        let socket = connection.socket_path().to_owned();
        let params = json!({ "target": pane_id, "keys": keys });
        let target = pane_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "agent.send_keys", params) })
                .await;
            this.update(cx, |this, cx| {
                this.agent_keys.remove(&target);
                match result {
                    Ok(_) if agent_panel_pane(&this.overlay) == Some(target.as_str()) => {
                        this.fetch_agent_output(cx)
                    }
                    Ok(_) => {}
                    Err(error) => this.notify_command_failure("agent.send_keys", error, cx),
                }
                cx.notify();
            })
            .ok();
        });
        self.agent_keys.insert(pane_id, task);
        cx.notify();
    }

    fn fetch_agent_name(&mut self, cx: &mut Context<Self>) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        let Some(connection) = &self.connection else {
            return;
        };
        let pane_id = pane_id.clone();
        let socket = connection.socket_path().to_owned();
        let params = json!({ "target": pane_id });
        let target = pane_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "agent.get", params) })
                .await;
            this.update(cx, |this, cx| {
                if agent_panel_pane(&this.overlay) != Some(target.as_str()) {
                    return;
                }
                match result {
                    Ok(value) => match parse_agent_info_result(value) {
                        Ok(agent) => {
                            this.agent_name = AgentNameState::Ready;
                            this.agent_name_input.update(cx, |input, cx| {
                                input.set_content(agent.name.as_deref().unwrap_or(""), cx)
                            });
                        }
                        Err(message) => {
                            this.agent_name = AgentNameState::Failed(message.clone());
                            this.notify_command_failure("agent.get", message, cx);
                        }
                    },
                    Err(error) => {
                        let message = error.to_string();
                        this.agent_name = AgentNameState::Failed(message.clone());
                        this.notify_command_failure("agent.get", message, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.agent_name = AgentNameState::Loading { _task: task };
        cx.notify();
    }

    fn fetch_agent_output(&mut self, cx: &mut Context<Self>) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        let Some(connection) = &self.connection else {
            return;
        };
        let pane_id = pane_id.clone();
        let socket = connection.socket_path().to_owned();
        let params = json!({
            "target": pane_id,
            "source": AGENT_OUTPUT_SOURCE,
            "format": "text",
        });
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "agent.read", params) })
                .await;
            this.update(cx, |this, cx| {
                if agent_panel_pane(&this.overlay) != Some(pane_id.as_str()) {
                    return;
                }
                this.agent_output = match result {
                    Ok(value) => match parse_agent_read_result(&value) {
                        Ok((text, truncated)) => AgentOutputState::Ready { text, truncated },
                        Err(message) => AgentOutputState::Failed { message },
                    },
                    Err(error) => {
                        this.notify_command_failure("agent.read", &error, cx);
                        AgentOutputState::Failed {
                            message: error.to_string(),
                        }
                    }
                };
                cx.notify();
            })
            .ok();
        });
        self.agent_output = AgentOutputState::Loading { _task: task };
        cx.notify();
    }

    fn reset_agent_panel_state(&mut self) {
        self.agent_name = AgentNameState::Idle;
        self.agent_output = AgentOutputState::Idle;
        self.agent_prompts
            .retain(|_, phase| matches!(phase, AgentPromptPhase::Sending { .. }));
        self.agent_name_error = None;
    }

    fn reconcile_open_agent_panel(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if agent_panel_target_missing(&self.overlay, self.snapshot.as_ref()) {
            self.set_overlay(Overlay::None, cx);
            return;
        }
        if refresh {
            self.fetch_agent_output(cx);
        }
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

    pub(super) fn open_worktree_create_for_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.source_workspace_id() {
            Some(workspace_id) => self.open_worktree_create(workspace_id, window, cx),
            None => self.notify_need_workspace(cx),
        }
    }

    pub(super) fn open_worktree_picker_for_selection(&mut self, cx: &mut Context<Self>) {
        match self.source_workspace_id() {
            Some(workspace_id) => self.open_worktree_picker(workspace_id, cx),
            None => self.notify_need_workspace(cx),
        }
    }

    /// `workspace_id` is the workspace the user pointed at (sidebar selection
    /// or the right-clicked row), not "whatever is selected at submit time".
    pub(super) fn open_worktree_create(
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

    pub(super) fn toggle_worktree_create_advanced(&mut self, cx: &mut Context<Self>) {
        let Overlay::WorktreeCreate { advanced, .. } = &mut self.overlay else {
            return;
        };
        *advanced = !*advanced;
        cx.notify();
    }

    pub(super) fn submit_worktree_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.invoke("worktree.create", params, cx);
    }

    pub(super) fn open_worktree_picker(&mut self, workspace_id: String, cx: &mut Context<Self>) {
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

    pub(super) fn open_listed_worktree(&mut self, path: String, cx: &mut Context<Self>) {
        let Overlay::WorktreeOpen(WorktreeOpenState::Ready { source, .. }) = &self.overlay else {
            return;
        };
        let params = worktree_open_params(source, &path);
        self.set_overlay(Overlay::None, cx);
        self.invoke("worktree.open", params, cx);
    }

    pub(super) fn request_remove_worktree(&mut self, workspace_id: String, cx: &mut Context<Self>) {
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

    pub(super) fn cancel_remove_worktree(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ConfirmRemoveWorktree { .. }) {
            self.set_overlay(Overlay::None, cx);
        }
    }

    pub(super) fn confirm_remove_worktree(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn close_worktree_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.overlay,
            Overlay::WorktreeCreate { .. } | Overlay::WorktreeOpen(_)
        ) {
            self.set_overlay(Overlay::None, cx);
        }
        self.focus.focus(window, cx);
    }

    fn source_workspace_id(&self) -> Option<String> {
        self.source_workspace()
            .map(|workspace| workspace.workspace_id.clone())
    }

    fn source_workspace(&self) -> Option<&WorkspaceInfo> {
        let snapshot = self.snapshot.as_ref()?;
        let id = self
            .selection
            .workspace_id
            .as_deref()
            .or(snapshot.focused_workspace_id.as_deref())?;
        self.workspace(id)
    }

    fn workspace(&self, workspace_id: &str) -> Option<&WorkspaceInfo> {
        self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
        })
    }

    fn current_session_key(&self) -> Option<SessionKey> {
        Some(SessionKey {
            profile_id: self.current_profile().id().to_owned(),
            session_name: self.current_session()?.name.clone(),
        })
    }

    fn workspace_label(&self, workspace_id: &str) -> Option<String> {
        self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
                .map(|workspace| workspace.label.clone())
        })
    }

    fn notify_need_workspace(&mut self, cx: &mut Context<Self>) {
        self.post_notice(
            FailureNotice {
                level: ochub_ui::notifications::NotificationLevel::Warning,
                title: self.i18n.text(k::WORKTREE_NEW).to_owned(),
                message: self.i18n.text(k::WORKTREE_NEED_WORKSPACE).to_owned(),
            },
            cx,
        );
    }

    fn clear_worktree_create_fields(&mut self, cx: &mut Context<Self>) {
        for input in [
            &self.worktree_label_input,
            &self.worktree_branch_input,
            &self.worktree_base_input,
            &self.worktree_path_input,
        ] {
            input.update(cx, |input, cx| input.set_content("", cx));
        }
    }

    fn fetch_worktree_list(
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

    fn maybe_offer_force_remove_worktree(
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
            ("j" | "down", true) => self.move_selected_workspace(1, cx),
            ("k" | "up", true) => self.move_selected_workspace(-1, cx),
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
            if matches!(self.overlay, Overlay::AgentPanel { .. }) {
                self.close_agent_panel(window, cx);
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
            if matches!(
                self.overlay,
                Overlay::ConfirmClose(_)
                    | Overlay::ConfirmRemoveWorktree { .. }
                    | Overlay::WorktreeCreate { .. }
                    | Overlay::WorktreeOpen(_)
            ) {
                self.set_overlay(Overlay::None, cx);
                self.focus.focus(window, cx);
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
        if matches!(
            self.surface_drag,
            SurfaceDrag::Split(_) | SurfaceDrag::Reorder(_)
        ) {
            return;
        }
        self.end_text_drag_unless_pane(&pane_id);
        self.select_pane(pane_id.clone(), window, cx);
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        let mouse = mouse_point(event.position);
        if !point_in_rect(mouse, runtime.body_bounds) {
            self.surface_drag = SurfaceDrag::Idle;
            return;
        }
        let Some(surface) = map_mouse_to_surface(
            mouse,
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        ) else {
            self.surface_drag = SurfaceDrag::Idle;
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        self.surface_drag = SurfaceDrag::Text {
            pane_id: pane_id.clone(),
        };
        if let Some(runtime) = self.pane_mut(&pane_id) {
            runtime
                .terminal
                .begin_text_selection(surface.0, surface.1, modifiers);
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
        if self.update_split_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.update_reorder_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        let SurfaceDrag::Text { pane_id } = &self.surface_drag else {
            return;
        };
        let pane_id = pane_id.clone();
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
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
                .update_text_selection(surface.0, surface.1, modifiers);
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
        if self.finish_split_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.finish_reorder_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        let SurfaceDrag::Text { pane_id } =
            std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle)
        else {
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        if let Some(runtime) = self.pane_mut(&pane_id) {
            let point = map_mouse_to_surface(
                mouse_point(event.position),
                runtime.body_bounds,
                runtime.pixel_size,
                window.scale_factor(),
            );
            runtime.terminal.end_text_selection(point, modifiers);
            flush_pane_surface(runtime);
            copy_terminal_selection(runtime, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn begin_split_drag(
        &mut self,
        tab_id: String,
        split: LayoutSplit,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(surface) = self.terminal_surface_bounds else {
            cx.stop_propagation();
            return;
        };
        let Some(drag) = self.snapshot.as_ref().and_then(|snapshot| {
            let layout = snapshot.layout_for(&tab_id)?;
            split_drag_from_press(tab_id, &split, layout, surface, mouse_point(event.position))
        }) else {
            cx.stop_propagation();
            return;
        };
        self.end_text_drag();
        self.cancel_reorder_drag();
        self.surface_drag = SurfaceDrag::Split(drag);
        cx.stop_propagation();
        cx.notify();
    }

    fn end_text_drag_unless_pane(&mut self, pane_id: &str) {
        let Some(previous) = self.take_text_drag() else {
            return;
        };
        if previous == pane_id {
            self.surface_drag = SurfaceDrag::Text { pane_id: previous };
            return;
        }
        self.finish_text_drag_on(&previous);
    }

    fn end_text_drag(&mut self) {
        if let Some(previous) = self.take_text_drag() {
            self.finish_text_drag_on(&previous);
        }
    }

    fn take_text_drag(&mut self) -> Option<String> {
        match std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle) {
            SurfaceDrag::Text { pane_id } => Some(pane_id),
            other => {
                self.surface_drag = other;
                None
            }
        }
    }

    fn finish_text_drag_on(&mut self, pane_id: &str) {
        if let Some(runtime) = self.pane_mut(pane_id) {
            runtime
                .terminal
                .end_text_selection(None, KeyModifiers::default());
        }
    }

    /// Navigation and stream death void the gesture outright. Snapshot
    /// mutations go through `reconcile_split_drag` so a ratio-only
    /// `layout.updated` (including our own submit) does not self-cancel.
    fn cancel_split_drag(&mut self) {
        if matches!(self.surface_drag, SurfaceDrag::Split(_)) {
            self.surface_drag = SurfaceDrag::Idle;
        }
    }

    fn take_split_drag(&mut self) -> Option<SplitDrag> {
        match std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle) {
            SurfaceDrag::Split(drag) => Some(drag),
            other => {
                self.surface_drag = other;
                None
            }
        }
    }

    fn reconcile_split_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.take_split_drag() else {
            return;
        };
        if let SurfaceDrag::Split(drag) = reconcile_split_drag_state(drag, self.snapshot.as_ref()) {
            self.surface_drag = SurfaceDrag::Split(drag);
        } else {
            cx.notify();
        }
    }

    fn update_split_drag(&mut self, mouse: (f32, f32), cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.take_split_drag() else {
            return false;
        };
        let previous = drag.preview_ratio;
        let drag = apply_split_drag_pointer(
            drag,
            self.snapshot.as_ref(),
            self.terminal_surface_bounds,
            mouse,
        );
        if (drag.preview_ratio - previous).abs() > f32::EPSILON {
            cx.notify();
        }
        self.surface_drag = SurfaceDrag::Split(drag);
        true
    }

    fn finish_split_drag(&mut self, mouse: (f32, f32), cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.take_split_drag() else {
            return false;
        };
        let drag = apply_split_drag_pointer(
            drag,
            self.snapshot.as_ref(),
            self.terminal_surface_bounds,
            mouse,
        );
        cx.notify();
        let SurfaceDrag::Split(drag) = reconcile_split_drag_state(drag, self.snapshot.as_ref())
        else {
            return true;
        };
        if (drag.preview_ratio - drag.start_ratio).abs() > f32::EPSILON {
            self.invoke(
                "layout.set_split_ratio",
                json!({
                    "tab_id": drag.tab_id,
                    "path": drag.path,
                    "ratio": drag.preview_ratio,
                }),
                cx,
            );
        }
        true
    }

    pub(super) fn press_workspace_row(
        &mut self,
        workspace_id: String,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let order = snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.clone())
            .collect::<Vec<_>>();
        let Some(source_index) = order.iter().position(|id| id == &workspace_id) else {
            return;
        };
        if order.len() < 2 {
            self.select_workspace(workspace_id, cx);
            return;
        }
        self.begin_reorder(ReorderList::Workspaces, source_index, order, event, cx);
    }

    pub(super) fn press_tab_pill(
        &mut self,
        tab_id: String,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(workspace_id) = snapshot
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.workspace_id.clone())
        else {
            return;
        };
        let order = snapshot
            .tabs_for(&workspace_id)
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        let Some(source_index) = order.iter().position(|id| id == &tab_id) else {
            return;
        };
        if order.len() < 2 {
            self.select_tab(tab_id, cx);
            return;
        }
        self.begin_reorder(
            ReorderList::Tabs { workspace_id },
            source_index,
            order,
            event,
            cx,
        );
    }

    fn begin_reorder(
        &mut self,
        list: ReorderList,
        source_index: usize,
        order: Vec<String>,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let source_id = order[source_index].clone();
        // Herdr owns the order. While it is publishing a move, a new drag would
        // compute its index from a list that is about to be replaced.
        if self.pending_reorder.is_some() {
            self.select_reorder_source(&list, source_id, cx);
            return;
        }
        // A drag needs the row it grabbed. Without a measured rect there is no
        // grab offset and no hover, and inventing one puts the ghost somewhere
        // the pointer never was.
        let Some(rect) = self.span_for(&list, &source_id) else {
            self.select_reorder_source(&list, source_id, cx);
            return;
        };
        let pointer = mouse_point(event.position);
        let grab_offset = (pointer.0 - rect.0, pointer.1 - rect.1);
        self.end_text_drag();
        self.cancel_split_drag();
        let hover = ReorderHover::Item {
            index: source_index,
            trailing: false,
        };
        self.surface_drag = SurfaceDrag::Reorder(ReorderDrag {
            list,
            source_index,
            order,
            previous_hover: hover,
            hover,
            origin: pointer,
            pointer,
            grab_offset,
            source_rect: rect,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn move_selected_workspace(&mut self, delta: isize, cx: &mut Context<Self>) {
        // Same reason as `begin_reorder`: the index would come from a list
        // Herdr is about to replace.
        if self.pending_reorder.is_some() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let ids = snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.clone())
            .collect::<Vec<_>>();
        let Some(source) = self
            .selection
            .workspace_id
            .as_ref()
            .and_then(|id| ids.iter().position(|candidate| candidate == id))
        else {
            return;
        };
        let hover = if delta < 0 {
            if source == 0 {
                return;
            }
            ReorderHover::Item {
                index: source - 1,
                trailing: false,
            }
        } else {
            let next = source + 1;
            if next >= ids.len() {
                return;
            }
            ReorderHover::Item {
                index: next,
                trailing: true,
            }
        };
        let Some(insert_index) = reorder_insert_index(ids.len(), source, hover) else {
            return;
        };
        let source_id = ids[source].clone();
        self.submit_reorder(&ReorderList::Workspaces, source_id, insert_index, None, cx);
    }

    /// The only path that asks Herdr to change an order. Holding the request in
    /// `pending_reorder` is what stops a second reorder from being computed
    /// against the list this one is replacing.
    fn submit_reorder(
        &mut self,
        list: &ReorderList,
        id: String,
        insert_index: usize,
        settling: Option<PendingListReorder>,
        cx: &mut Context<Self>,
    ) {
        let (method, params) = match list {
            ReorderList::Workspaces => (
                "workspace.move",
                json!({ "workspace_id": id, "insert_index": insert_index }),
            ),
            ReorderList::Tabs { .. } => (
                "tab.move",
                json!({ "tab_id": id, "insert_index": insert_index }),
            ),
        };
        if let Some(request) = self.spawn_invoke(method, params, cx) {
            self.pending_reorder = Some(PendingReorder {
                _request: request,
                display: settling,
            });
        }
    }

    fn cancel_reorder_drag(&mut self) {
        if matches!(self.surface_drag, SurfaceDrag::Reorder(_)) {
            self.surface_drag = SurfaceDrag::Idle;
        }
    }

    fn take_reorder_drag(&mut self) -> Option<ReorderDrag> {
        match std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle) {
            SurfaceDrag::Reorder(drag) => Some(drag),
            other => {
                self.surface_drag = other;
                None
            }
        }
    }

    fn reconcile_reorder_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.take_reorder_drag() else {
            return;
        };
        if let SurfaceDrag::Reorder(drag) =
            reconcile_reorder_drag_state(drag, self.snapshot.as_ref())
        {
            self.surface_drag = SurfaceDrag::Reorder(drag);
        } else {
            cx.notify();
        }
    }

    fn update_reorder_drag(&mut self, mouse: (f32, f32), cx: &mut Context<Self>) -> bool {
        let Some(mut drag) = self.take_reorder_drag() else {
            return false;
        };
        drag.pointer = mouse;
        // Rows left the layout mid-drag. Keeping the last hover would aim the
        // drop at a position that is no longer on screen.
        let Some(hover) = self.reorder_hover_for(&drag) else {
            cx.notify();
            return true;
        };
        if drag.hover != hover {
            drag.previous_hover = drag.hover;
            drag.hover = hover;
        }
        self.surface_drag = SurfaceDrag::Reorder(drag);
        cx.notify();
        true
    }

    fn finish_reorder_drag(&mut self, mouse: (f32, f32), cx: &mut Context<Self>) -> bool {
        let Some(mut drag) = self.take_reorder_drag() else {
            return false;
        };
        drag.pointer = mouse;
        let source_id = drag.order[drag.source_index].clone();
        let list = drag.list.clone();
        let Some(hover) = self.reorder_hover_for(&drag) else {
            self.select_reorder_source(&list, source_id, cx);
            return true;
        };
        drag.hover = hover;
        if reorder_past_slop(&drag) {
            let SurfaceDrag::Reorder(drag) =
                reconcile_reorder_drag_state(drag, self.snapshot.as_ref())
            else {
                self.select_reorder_source(&list, source_id, cx);
                return true;
            };
            if let Some(insert_index) =
                reorder_insert_index(drag.order.len(), drag.source_index, drag.hover)
            {
                let settling = self.pending_display_for(&drag);
                self.submit_reorder(&list, source_id.clone(), insert_index, settling, cx);
            }
        }
        self.select_reorder_source(&list, source_id, cx);
        true
    }

    fn select_reorder_source(
        &mut self,
        list: &ReorderList,
        source_id: String,
        cx: &mut Context<Self>,
    ) {
        match list {
            ReorderList::Workspaces => self.select_workspace(source_id, cx),
            ReorderList::Tabs { .. } => self.select_tab(source_id, cx),
        }
    }

    fn reorder_hover_for(&self, drag: &ReorderDrag) -> Option<ReorderHover> {
        let spans = self.spans_along_axis(&drag.list, &drag.order)?;
        let pointer = match drag.list {
            ReorderList::Workspaces => drag.pointer.1,
            ReorderList::Tabs { .. } => drag.pointer.0,
        };
        Some(reorder_hover_along_axis(&spans, pointer))
    }

    fn pending_display_for(&self, drag: &ReorderDrag) -> Option<PendingListReorder> {
        let rects = drag
            .order
            .iter()
            .map(|id| self.span_for(&drag.list, id))
            .collect::<Option<Vec<_>>>()?;
        Some(PendingListReorder {
            list: drag.list.clone(),
            order: drag.order.clone(),
            source_index: drag.source_index,
            hover: drag.hover,
            released_origin: reorder_ghost_origin(
                drag.pointer,
                drag.grab_offset,
                reorder_list_bounds(&rects),
                (drag.source_rect.2, drag.source_rect.3),
                reorder_axis(&drag.list),
            ),
        })
    }

    fn spans_along_axis(&self, list: &ReorderList, order: &[String]) -> Option<Vec<(f32, f32)>> {
        let mut spans = Vec::with_capacity(order.len());
        for id in order {
            let rect = self.span_for(list, id)?;
            spans.push(match list {
                ReorderList::Workspaces => (rect.1, rect.3),
                ReorderList::Tabs { .. } => (rect.0, rect.2),
            });
        }
        Some(spans)
    }

    fn span_for(&self, list: &ReorderList, id: &str) -> Option<(f32, f32, f32, f32)> {
        let spans = match list {
            ReorderList::Workspaces => &self.reorder_metrics.workspaces,
            ReorderList::Tabs { .. } => &self.reorder_metrics.tabs,
        };
        spans
            .iter()
            .find(|span| span.id == id)
            .map(|span| span.rect)
    }

    pub(super) fn note_reorder_span(&mut self, tabs: bool, id: String, rect: (f32, f32, f32, f32)) {
        let spans = if tabs {
            &mut self.reorder_metrics.tabs
        } else {
            &mut self.reorder_metrics.workspaces
        };
        if let Some(existing) = spans.iter_mut().find(|span| span.id == id) {
            existing.rect = rect;
        } else {
            spans.push(ReorderSpan { id, rect });
        }
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
        if let Some(request) = self.spawn_invoke(method, params, cx) {
            request.detach();
        }
    }

    /// Same request as `invoke`, but the caller keeps the task so it can tie the
    /// request's lifetime to the state that request is allowed to block.
    fn spawn_invoke(
        &mut self,
        method: &'static str,
        params: Value,
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        let connection = self.connection.as_ref()?;
        let socket = connection.socket_path().to_owned();
        self.operation = Some(self.i18n.running_operation(method).into());
        Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn({
                    let params = params.clone();
                    async move { request_socket(&socket, method, params).map(|_| ()) }
                })
                .await;
            this.update(cx, |this, cx| {
                this.operation = None;
                match result {
                    Ok(()) => {
                        if command_needs_snapshot_resync(method) {
                            this.resync_snapshot(this.event_epoch, cx);
                        }
                    }
                    Err(error) => {
                        // A rejected move never produces a `moved` event, so the
                        // gate has to open here or it would never open at all.
                        this.pending_reorder = None;
                        this.maybe_offer_force_remove_worktree(&params, &error, cx);
                        this.notify_command_failure(method, error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentStatusSubscribeFailureAction {
    Resync,
    Report,
}

fn agent_status_subscribe_failure_action(error: &HerdrError) -> AgentStatusSubscribeFailureAction {
    match error {
        HerdrError::Api { code, .. } if code == "pane_not_found" => {
            AgentStatusSubscribeFailureAction::Resync
        }
        _ => AgentStatusSubscribeFailureAction::Report,
    }
}

fn snapshot_handoff_should_release(refreshing: bool) -> bool {
    !refreshing
}

// pane.rename emits nothing. pane.close can delete the parent tab and
// reshuffle focus / tab numbers without emitting tab.closed.
fn command_needs_snapshot_resync(method: &str) -> bool {
    matches!(method, "pane.rename" | "pane.close")
}

fn worktree_repo_params(workspace: &WorkspaceInfo) -> serde_json::Map<String, Value> {
    let mut params = serde_json::Map::new();
    match workspace.worktree.as_ref() {
        Some(worktree) if worktree.is_linked_worktree => {
            params.insert("cwd".into(), json!(worktree.repo_root));
        }
        _ => {
            params.insert("workspace_id".into(), json!(workspace.workspace_id));
        }
    }
    params
}

fn worktree_create_params(
    workspace: &WorkspaceInfo,
    label: &str,
    branch: &str,
    base: &str,
    path: &str,
) -> Value {
    let mut params = worktree_repo_params(workspace);
    params.insert("focus".into(), json!(true));
    for (key, value) in [
        ("label", label),
        ("branch", branch),
        ("base", base),
        ("path", path),
    ] {
        let value = value.trim();
        if !value.is_empty() {
            params.insert(key.into(), json!(value));
        }
    }
    Value::Object(params)
}

fn overlay_after_abandoning_worktree_list(overlay: Overlay) -> Overlay {
    if matches!(overlay, Overlay::WorktreeOpen(_)) {
        Overlay::None
    } else {
        overlay
    }
}

fn agent_panel_pane(overlay: &Overlay) -> Option<&str> {
    match overlay {
        Overlay::AgentPanel { pane_id } => Some(pane_id.as_str()),
        _ => None,
    }
}

fn agent_panel_target_missing(overlay: &Overlay, snapshot: Option<&HierarchySnapshot>) -> bool {
    let Some(pane_id) = agent_panel_pane(overlay) else {
        return false;
    };
    let Some(snapshot) = snapshot else {
        return true;
    };
    snapshot
        .pane(pane_id)
        .is_none_or(|pane| pane.display_agent.is_none() && pane.agent.is_none())
}

fn agent_panel_refresh_from_batch(
    overlay: &Overlay,
    batch: Option<&[std::result::Result<HerdrEvent, HerdrError>]>,
) -> bool {
    let Some(pane_id) = agent_panel_pane(overlay) else {
        return false;
    };
    let Some(items) = batch else {
        return false;
    };
    items.iter().any(|item| {
        item.as_ref()
            .is_ok_and(|event| agent_output_should_refresh(pane_id, event))
    })
}

fn agent_prompt_text_to_send(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

fn parse_agent_info_result(value: Value) -> Result<AgentInfo, String> {
    let agent = value
        .get("agent")
        .cloned()
        .ok_or_else(|| "API response is missing `agent`".to_owned())?;
    serde_json::from_value(agent).map_err(|error| format!("invalid `agent`: {error}"))
}

fn parse_agent_read_result(value: &Value) -> Result<(String, bool), String> {
    let read = value
        .get("read")
        .ok_or_else(|| "API response is missing `read`".to_owned())?;
    let text = read
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "read is missing `text`".to_owned())?;
    let truncated = read
        .get("truncated")
        .and_then(Value::as_bool)
        .ok_or_else(|| "read is missing `truncated`".to_owned())?;
    Ok((text.to_owned(), truncated))
}

fn snapshot_contains_workspace(snapshot: Option<&HierarchySnapshot>, workspace_id: &str) -> bool {
    snapshot.is_some_and(|snapshot| {
        snapshot
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == workspace_id)
    })
}

/// Workspace this picker was opened from. `worktree.open` still sends that id
/// when Herdr returned it, so a missing workspace makes the list unusable.
fn worktree_open_target_id(overlay: &Overlay) -> Option<&str> {
    match overlay {
        Overlay::WorktreeOpen(WorktreeOpenState::Loading { workspace_id, .. }) => {
            Some(workspace_id.as_str())
        }
        Overlay::WorktreeOpen(WorktreeOpenState::Ready { source, .. }) => {
            source.source_workspace_id.as_deref()
        }
        _ => None,
    }
}

fn worktree_open_target_is_missing(
    overlay: &Overlay,
    snapshot: Option<&HierarchySnapshot>,
) -> bool {
    worktree_open_target_id(overlay)
        .is_some_and(|workspace_id| !snapshot_contains_workspace(snapshot, workspace_id))
}

fn worktree_list_applies(
    overlay: &Overlay,
    live_session: Option<&SessionKey>,
    fetched_workspace_id: &str,
    fetched_session: &SessionKey,
    snapshot: Option<&HierarchySnapshot>,
) -> bool {
    let Overlay::WorktreeOpen(WorktreeOpenState::Loading {
        owner,
        workspace_id,
    }) = overlay
    else {
        return false;
    };
    live_session == Some(fetched_session)
        && owner == fetched_session
        && workspace_id == fetched_workspace_id
        && snapshot_contains_workspace(snapshot, fetched_workspace_id)
}

fn worktree_open_params(source: &WorktreeSourceInfo, path: &str) -> Value {
    let mut params = json!({ "path": path, "focus": true });
    if let Some(workspace_id) = source.source_workspace_id.as_deref() {
        params["workspace_id"] = json!(workspace_id);
    } else {
        params["cwd"] = json!(source.repo_root);
    }
    params
}

fn worktree_remove_params(workspace_id: &str, force: bool) -> Value {
    if force {
        json!({ "workspace_id": workspace_id, "force": true })
    } else {
        json!({ "workspace_id": workspace_id })
    }
}

fn dirty_worktree_remove_offer(method: &str, params: &Value, error: &HerdrError) -> Option<String> {
    if method != "worktree.remove" {
        return None;
    }
    if params.get("force").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let HerdrError::Api { code, .. } = error else {
        return None;
    };
    if code != "dirty_worktree_requires_force" {
        return None;
    }
    params
        .get("workspace_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[derive(Debug, PartialEq, Eq)]
enum EventPollAction {
    /// Replace Live with Lost. A dead stream has nothing left to poll.
    Disconnect(SharedString),
    Idle,
    Applied {
        /// A `workspace.moved` / `tab.moved` landed, so Herdr has published the
        /// order a pending reorder was waiting for.
        reordered: bool,
    },
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
    /// Drop `worktree_list_task` and its Loading overlay. A dead stream
    /// keeps the same session id, so the list callback would otherwise apply.
    abandon_worktree_list: bool,
    /// Release the reorder gate: the authoritative order has arrived, is being
    /// refetched, or will never arrive because the stream died.
    settle_reorder: bool,
    error: Option<SharedString>,
}

fn effects_for(action: &EventPollAction) -> PollEffects {
    match action {
        EventPollAction::Disconnect(_) => PollEffects {
            resync: false,
            apply_local: false,
            notify: true,
            reschedule: false,
            abandon_worktree_list: true,
            settle_reorder: true,
            error: None,
        },
        EventPollAction::Idle => PollEffects {
            resync: false,
            apply_local: false,
            notify: false,
            reschedule: true,
            abandon_worktree_list: false,
            settle_reorder: false,
            error: None,
        },
        EventPollAction::Applied { reordered } => PollEffects {
            resync: false,
            apply_local: true,
            notify: true,
            reschedule: true,
            abandon_worktree_list: false,
            settle_reorder: *reordered,
            error: None,
        },
        EventPollAction::Resync { error } => PollEffects {
            resync: true,
            apply_local: false,
            notify: false,
            reschedule: true,
            settle_reorder: true,
            abandon_worktree_list: false,
            error: error.clone(),
        },
    }
}

impl EventPollAction {
    fn event_stream(&self) -> Option<EventStreamState> {
        match self {
            Self::Disconnect(detail) => Some(EventStreamState::Lost(detail.clone())),
            Self::Idle | Self::Applied { .. } | Self::Resync { .. } => None,
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
    let mut reordered = false;
    let mut error = None;
    for _ in 0..128 {
        match next() {
            Ok(Some(event)) => {
                seen = true;
                // Every event that republishes a whole order settles a pending
                // reorder, whichever command produced it.
                reordered |= matches!(
                    event,
                    HerdrEvent::WorkspaceMoved { .. }
                        | HerdrEvent::WorkspaceReordered { .. }
                        | HerdrEvent::TabMoved { .. }
                );
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
        EventPollAction::Applied { reordered }
    } else {
        EventPollAction::Idle
    }
}

fn mouse_point(position: ochub_ui::gpui::Point<ochub_ui::gpui::Pixels>) -> (f32, f32) {
    (f32::from(position.x), f32::from(position.y))
}

fn pointer_along_split(
    direction: SplitDirection,
    area: LayoutRect,
    surface: (f32, f32, f32, f32),
    mouse: (f32, f32),
) -> Option<f32> {
    let (sx, sy, sw, sh) = surface;
    if sw <= 0. || sh <= 0. || area.width == 0 || area.height == 0 {
        return None;
    }
    Some(match direction {
        SplitDirection::Right => f32::from(area.x) + (mouse.0 - sx) / sw * f32::from(area.width),
        SplitDirection::Down => f32::from(area.y) + (mouse.1 - sy) / sh * f32::from(area.height),
    })
}

fn split_axis_line(split: &LayoutSplit) -> f32 {
    match split.direction {
        SplitDirection::Right => {
            f32::from(split.rect.x) + f32::from(split.rect.width) * split.ratio
        }
        SplitDirection::Down => {
            f32::from(split.rect.y) + f32::from(split.rect.height) * split.ratio
        }
    }
}

fn split_drag_from_press(
    tab_id: String,
    split: &LayoutSplit,
    layout: &ocherdr_core::PaneLayout,
    surface: (f32, f32, f32, f32),
    mouse: (f32, f32),
) -> Option<SplitDrag> {
    let path = split.path()?;
    let size = match split.direction {
        SplitDirection::Right => split.rect.width,
        SplitDirection::Down => split.rect.height,
    };
    if size == 0 {
        return None;
    }
    let pointer = pointer_along_split(split.direction, layout.area, surface, mouse)?;
    Some(SplitDrag {
        workspace_id: layout.workspace_id.clone(),
        tab_id,
        path,
        layout: split_layout_fingerprint(layout),
        direction: split.direction,
        rect: split.rect,
        grab_offset: split_axis_line(split) - pointer,
        preview_ratio: split
            .ratio
            .clamp(ocherdr_core::SPLIT_RATIO_MIN, ocherdr_core::SPLIT_RATIO_MAX),
        start_ratio: split.ratio,
    })
}

fn split_layout_fingerprint(layout: &ocherdr_core::PaneLayout) -> SplitLayoutFingerprint {
    SplitLayoutFingerprint {
        zoomed: layout.zoomed,
        splits: layout
            .splits
            .iter()
            .filter_map(|split| Some((split.path()?, split.direction)))
            .collect(),
        panes: layout
            .panes
            .iter()
            .map(|pane| pane.pane_id.clone())
            .collect(),
    }
}

fn split_drag_survives_layout(drag: &SplitDrag, snapshot: &HierarchySnapshot) -> bool {
    let Some(layout) = snapshot.layout_for(&drag.tab_id) else {
        return false;
    };
    if split_layout_fingerprint(layout) != drag.layout {
        return false;
    }
    // PaneCreated/PaneClosed update snapshot.panes before layout.updated.
    let mut live: Vec<&str> = snapshot
        .panes_for(&drag.tab_id)
        .map(|pane| pane.pane_id.as_str())
        .collect();
    let mut expected: Vec<&str> = drag.layout.panes.iter().map(String::as_str).collect();
    live.sort();
    expected.sort();
    live == expected
}

fn split_drag_voided_by_pane(
    drag: &SplitDrag,
    workspace_id: Option<&str>,
    tab_id: Option<&str>,
) -> bool {
    match (workspace_id, tab_id) {
        (Some(workspace_id), Some(tab_id)) => {
            tab_id != drag.tab_id || workspace_id != drag.workspace_id
        }
        _ => true,
    }
}

fn reconcile_split_drag_state(
    drag: SplitDrag,
    snapshot: Option<&HierarchySnapshot>,
) -> SurfaceDrag {
    if snapshot.is_some_and(|snapshot| split_drag_survives_layout(&drag, snapshot)) {
        SurfaceDrag::Split(drag)
    } else {
        SurfaceDrag::Idle
    }
}

fn reorder_live_ids(list: &ReorderList, snapshot: &HierarchySnapshot) -> Option<Vec<String>> {
    match list {
        ReorderList::Workspaces => Some(
            snapshot
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace_id.clone())
                .collect(),
        ),
        ReorderList::Tabs { workspace_id } => {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| &workspace.workspace_id == workspace_id)?;
            Some(
                snapshot
                    .tabs_for(workspace_id)
                    .map(|tab| tab.tab_id.clone())
                    .collect(),
            )
        }
    }
}

fn reconcile_reorder_drag_state(
    drag: ReorderDrag,
    snapshot: Option<&HierarchySnapshot>,
) -> SurfaceDrag {
    let Some(live) = snapshot.and_then(|snapshot| reorder_live_ids(&drag.list, snapshot)) else {
        return SurfaceDrag::Idle;
    };
    if live == drag.order {
        SurfaceDrag::Reorder(drag)
    } else {
        SurfaceDrag::Idle
    }
}

fn apply_split_drag_pointer(
    mut drag: SplitDrag,
    snapshot: Option<&HierarchySnapshot>,
    surface: Option<(f32, f32, f32, f32)>,
    mouse: (f32, f32),
) -> SplitDrag {
    let Some(surface) = surface else {
        return drag;
    };
    let Some(area) = snapshot
        .and_then(|snapshot| snapshot.layout_for(&drag.tab_id))
        .map(|layout| layout.area)
    else {
        return drag;
    };
    let Some(pointer) = pointer_along_split(drag.direction, area, surface, mouse) else {
        return drag;
    };
    drag.preview_ratio =
        split_ratio_from_drag(drag.direction, drag.rect, pointer + drag.grab_offset);
    drag
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
    let family = theme::find_family(&appearance.theme_family);
    terminal_palette_from_theme(
        theme::current(),
        theme::is_dark(),
        family.as_ref(),
        &appearance.font,
    )
}

/// 16-color terminal palettes, not UI tokens. UI red/green are muted for chrome;
/// `ls`, diffs, and agent CLIs need distinct bright slots.
const OCHUB_DARK_ANSI: [u32; 16] = [
    0x2C2D28, 0xB54C48, 0x2F7A4C, 0xB08A32, 0x355EA8, 0x6A56A8, 0x1F7F78, 0xB8B7AE, 0x6E6F64,
    0xFF8A82, 0x6FD496, 0xF0C46A, 0x7DB0FF, 0xC4A8FF, 0x5ED4C8, 0xF4F3EA,
];
const OCHUB_LIGHT_ANSI: [u32; 16] = [
    0x3A3A34, 0x9A322C, 0x1A5C34, 0x7A5008, 0x1A4AB0, 0x4C3C90, 0x085C54, 0x8A8982, 0x6B6A64,
    0xD4564C, 0x2D9A58, 0xC48A18, 0x4A82EE, 0x8B70D8, 0x1AA89A, 0xD0CFC6,
];
const EMBER_DARK_ANSI: [u32; 16] = [
    0x2A1E16, 0xB44A38, 0x4A7A32, 0xB07A24, 0x3A5A98, 0x8A5A9A, 0x2A7A6A, 0xC8B8A4, 0x7A5A42,
    0xF07058, 0x88C058, 0xE8B040, 0x6A8AD8, 0xC890E0, 0x58C8B0, 0xF7EEE5,
];
const EMBER_LIGHT_ANSI: [u32; 16] = [
    0x3A2A1C, 0x9A3028, 0x2A5C28, 0x7A5008, 0x2A4890, 0x5C3878, 0x0A5C50, 0x8A7A68, 0x6B5A48,
    0xD45640, 0x3A9A48, 0xC48A18, 0x4A72D0, 0x8B68C0, 0x1A9888, 0xE8D8C8,
];

fn terminal_ansi(
    family: Option<&theme::ThemeFamily>,
    theme: &theme::Theme,
    dark: bool,
) -> [u32; 16] {
    match family {
        // Missing theme: UI already falls back to OcHub chrome; keep the
        // hand-tuned OcHub table instead of deriving from whatever tokens
        // `theme::current()` happens to hold.
        None => ochub_ansi(dark),
        Some(family) if family.id == theme::DEFAULT_THEME_FAMILY => ochub_ansi(dark),
        Some(family) if family.id == theme::EMBER_THEME_FAMILY => ember_ansi(dark),
        Some(_) => ansi_from_theme(theme, dark),
    }
}

fn ochub_ansi(dark: bool) -> [u32; 16] {
    if dark {
        OCHUB_DARK_ANSI
    } else {
        OCHUB_LIGHT_ANSI
    }
}

fn ember_ansi(dark: bool) -> [u32; 16] {
    if dark {
        EMBER_DARK_ANSI
    } else {
        EMBER_LIGHT_ANSI
    }
}

fn ansi_from_theme(theme: &theme::Theme, dark: bool) -> [u32; 16] {
    let (black, bright_black) = split_gray_pair(theme.bg.0, theme.text.0, dark);
    let (red, bright_red) = split_pair(theme.red.0, dark);
    let (green, bright_green) = split_pair(theme.green.0, dark);
    let (yellow, bright_yellow) = split_pair(theme.yellow.0, dark);
    let (blue, bright_blue) = split_pair(theme.accent.0, dark);
    let (magenta, bright_magenta) = split_pair(theme.mauve.0, dark);
    let (cyan, bright_cyan) = split_pair(theme.teal.0, dark);
    let (white, bright_white) = split_pair(theme.subtext.0, dark);
    [
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    ]
}

fn split_gray_pair(bg: u32, text: u32, dark: bool) -> (u32, u32) {
    let toward_text = if dark { 22 } else { 28 };
    split_pair(mix_rgb(bg, text, toward_text), dark)
}

fn split_pair(color: u32, dark: bool) -> (u32, u32) {
    let dim = mix_rgb(color, 0x000000, if dark { 28 } else { 16 });
    let bright = mix_rgb(color, 0xFFFFFF, if dark { 16 } else { 28 });
    if ansi_luma(bright) > ansi_luma(dim) && dim != bright {
        return (dim, bright);
    }
    let bright = mix_rgb(dim, 0xFFFFFF, 42);
    if ansi_luma(bright) > ansi_luma(dim) && dim != bright {
        return (dim, bright);
    }
    (mix_rgb(bright, 0x000000, 36), bright)
}

fn mix_rgb(from: u32, to: u32, percent: u32) -> u32 {
    let percent = percent.min(100) as i32;
    let mix = |from: u32, to: u32| {
        let from = from as i32;
        let to = to as i32;
        (from + (to - from) * percent / 100).clamp(0, 255) as u32
    };
    (mix((from >> 16) & 0xff, (to >> 16) & 0xff) << 16)
        | (mix((from >> 8) & 0xff, (to >> 8) & 0xff) << 8)
        | mix(from & 0xff, to & 0xff)
}

fn ansi_luma(color: u32) -> u32 {
    299 * ((color >> 16) & 0xff) + 587 * ((color >> 8) & 0xff) + 114 * (color & 0xff)
}

fn terminal_palette_from_theme(
    theme: ochub_ui::theme::Theme,
    dark: bool,
    family: Option<&theme::ThemeFamily>,
    font: &TerminalFontSettings,
) -> TerminalPalette {
    TerminalPalette {
        dark,
        background: theme.bg.0,
        foreground: theme.text.0,
        cursor: theme.accent.0,
        selection: theme.selection.0,
        ansi: terminal_ansi(family, &theme, dark),
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
    use std::collections::HashMap;

    use ocherdr_core::WorkspaceWorktreeInfo;

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
    fn pane_not_found_agent_status_subscribe_failure_resyncs() {
        let error = HerdrError::Api {
            code: "pane_not_found".into(),
            message: "pane w19:p3 not found".into(),
        };
        assert_eq!(
            agent_status_subscribe_failure_action(&error),
            AgentStatusSubscribeFailureAction::Resync
        );
    }

    #[test]
    fn other_agent_status_subscribe_api_failures_are_reported() {
        let error = HerdrError::Api {
            code: "unknown_type".into(),
            message: "subscription rejected".into(),
        };
        assert_eq!(
            agent_status_subscribe_failure_action(&error),
            AgentStatusSubscribeFailureAction::Report
        );
    }

    #[test]
    fn non_api_agent_status_subscribe_failures_are_reported() {
        let error = HerdrError::EventStreamClosed("socket closed".into());
        assert_eq!(
            agent_status_subscribe_failure_action(&error),
            AgentStatusSubscribeFailureAction::Report
        );
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
            "layout.set_split_ratio",
            "workspace.move",
            "tab.move",
            "worktree.create",
            "worktree.open",
            "worktree.remove",
        ] {
            assert!(
                !command_needs_snapshot_resync(method),
                "{method} is pushed back as an event and must not reload the snapshot"
            );
        }
    }

    fn sample_workspace(id: &str, worktree: Option<WorkspaceWorktreeInfo>) -> WorkspaceInfo {
        WorkspaceInfo {
            workspace_id: id.into(),
            number: 1,
            label: id.into(),
            focused: true,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: format!("{id}:t1"),
            agent_status: AgentStatus::Idle,
            tokens: HashMap::new(),
            worktree,
        }
    }

    #[test]
    fn worktree_create_params_omit_empty_optionals_and_focus() {
        let workspace = sample_workspace("w1", None);
        let params = worktree_create_params(&workspace, "  ", "", " HEAD ", "");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["focus"], true);
        assert_eq!(params["base"], "HEAD");
        assert!(params.get("label").is_none());
        assert!(params.get("branch").is_none());
        assert!(params.get("path").is_none());
        assert!(params.get("force").is_none());
    }

    #[test]
    fn worktree_repo_params_use_parent_cwd_for_linked_checkouts() {
        let parent = sample_workspace("w1", None);
        assert_eq!(
            Value::Object(worktree_repo_params(&parent)),
            json!({ "workspace_id": "w1" })
        );

        let linked = sample_workspace(
            "w2",
            Some(WorkspaceWorktreeInfo {
                repo_key: "/repo/.git".into(),
                repo_name: "repo".into(),
                repo_root: "/repo".into(),
                checkout_path: "/worktrees/repo/feature".into(),
                is_linked_worktree: true,
            }),
        );
        assert_eq!(
            Value::Object(worktree_repo_params(&linked)),
            json!({ "cwd": "/repo" })
        );
    }

    #[test]
    fn worktree_remove_omits_force_unless_the_user_asked() {
        assert_eq!(
            worktree_remove_params("w1", false),
            json!({ "workspace_id": "w1" })
        );
        assert_eq!(
            worktree_remove_params("w1", true),
            json!({ "workspace_id": "w1", "force": true })
        );
    }

    #[test]
    fn dirty_remove_offers_force_only_for_the_safe_remove_error() {
        let params = json!({ "workspace_id": "w1" });
        let dirty = HerdrError::Api {
            code: "dirty_worktree_requires_force".into(),
            message: "uncommitted changes".into(),
        };
        assert_eq!(
            dirty_worktree_remove_offer("worktree.remove", &params, &dirty).as_deref(),
            Some("w1")
        );
        assert_eq!(
            dirty_worktree_remove_offer(
                "worktree.remove",
                &json!({ "workspace_id": "w1", "force": true }),
                &dirty
            ),
            None
        );
        assert_eq!(
            dirty_worktree_remove_offer("worktree.create", &params, &dirty),
            None
        );
        assert_eq!(
            dirty_worktree_remove_offer(
                "worktree.remove",
                &params,
                &HerdrError::Api {
                    code: "not_git_worktree".into(),
                    message: "nope".into(),
                }
            ),
            None
        );
    }

    fn sample_session(name: &str) -> SessionKey {
        SessionKey {
            profile_id: "local".into(),
            session_name: name.into(),
        }
    }

    fn snapshot_with_workspace(id: &str) -> HierarchySnapshot {
        HierarchySnapshot {
            workspaces: vec![sample_workspace(id, None)],
            ..Default::default()
        }
    }

    #[test]
    fn worktree_list_result_is_ignored_after_the_session_changes() {
        let session_a = sample_session("alpha");
        let session_b = sample_session("beta");
        let loading = Overlay::WorktreeOpen(WorktreeOpenState::Loading {
            owner: session_a.clone(),
            workspace_id: "w1".into(),
        });
        let present = snapshot_with_workspace("w1");
        assert!(worktree_list_applies(
            &loading,
            Some(&session_a),
            "w1",
            &session_a,
            Some(&present),
        ));
        assert!(!worktree_list_applies(
            &loading,
            Some(&session_b),
            "w1",
            &session_a,
            Some(&present),
        ));
        assert!(!worktree_list_applies(
            &loading,
            Some(&session_a),
            "w2",
            &session_a,
            Some(&present),
        ));
        assert!(!worktree_list_applies(
            &Overlay::None,
            Some(&session_a),
            "w1",
            &session_a,
            Some(&present),
        ));
    }

    #[test]
    fn worktree_list_result_is_ignored_after_the_target_workspace_is_gone() {
        let session = sample_session("alpha");
        let loading = Overlay::WorktreeOpen(WorktreeOpenState::Loading {
            owner: session.clone(),
            workspace_id: "w1".into(),
        });
        let present = snapshot_with_workspace("w1");
        let gone = HierarchySnapshot::default();
        assert!(
            worktree_list_applies(&loading, Some(&session), "w1", &session, Some(&present)),
            "a still-open target workspace must still accept the list"
        );
        assert!(
            !worktree_list_applies(&loading, Some(&session), "w1", &session, Some(&gone)),
            "injecting a list result after workspace.closed / matching worktree.removed / resync dropped the target must fail this test"
        );
        assert!(
            !worktree_list_applies(&loading, Some(&session), "w1", &session, None),
            "no snapshot means the target workspace is not known to still exist"
        );
        assert!(
            worktree_open_target_is_missing(&loading, Some(&gone)),
            "gate 1 (event/resync) must drop Loading when the pointed-at workspace is gone"
        );
        assert!(!worktree_open_target_is_missing(&loading, Some(&present)));
        let ready_bound = Overlay::WorktreeOpen(WorktreeOpenState::Ready {
            source: WorktreeSourceInfo {
                repo_key: "/repo/.git".into(),
                repo_name: "repo".into(),
                repo_root: "/repo".into(),
                source_checkout_path: "/repo".into(),
                source_workspace_id: Some("w1".into()),
            },
            worktrees: Vec::new(),
        });
        assert!(worktree_open_target_is_missing(&ready_bound, Some(&gone)));
        let ready_by_repo = Overlay::WorktreeOpen(WorktreeOpenState::Ready {
            source: WorktreeSourceInfo {
                repo_key: "/repo/.git".into(),
                repo_name: "repo".into(),
                repo_root: "/repo".into(),
                source_checkout_path: "/repo".into(),
                source_workspace_id: None,
            },
            worktrees: Vec::new(),
        });
        assert!(
            !worktree_open_target_is_missing(&ready_by_repo, Some(&gone)),
            "a cwd-only list is not bound to a workspace id"
        );
    }

    #[test]
    fn disconnecting_the_event_stream_abandons_an_in_flight_worktree_list() {
        let action = poll_event_stream(&mut HierarchySnapshot::default(), || {
            Err(HerdrError::EventStreamClosed("event worker stopped".into()))
        });
        assert!(
            effects_for(&action).abandon_worktree_list,
            "injecting Disconnect without dropping worktree_list_task must fail this test"
        );
        let loading = Overlay::WorktreeOpen(WorktreeOpenState::Loading {
            owner: sample_session("alpha"),
            workspace_id: "w1".into(),
        });
        assert!(
            matches!(
                overlay_after_abandoning_worktree_list(loading),
                Overlay::None
            ),
            "disconnect must clear the Loading overlay, not leave it for a stale list result"
        );
        assert!(!effects_for(&EventPollAction::Applied { reordered: false }).abandon_worktree_list);
        assert!(!effects_for(&EventPollAction::Idle).abandon_worktree_list);
        assert!(!effects_for(&EventPollAction::Resync { error: None }).abandon_worktree_list);
        let closed_worker = EventPollAction::Disconnect("event worker stopped".into());
        assert!(effects_for(&closed_worker).abandon_worktree_list);
    }

    #[test]
    fn abandoning_a_session_clears_only_the_worktree_open_overlay() {
        let loading = Overlay::WorktreeOpen(WorktreeOpenState::Loading {
            owner: sample_session("alpha"),
            workspace_id: "w1".into(),
        });
        assert!(matches!(
            overlay_after_abandoning_worktree_list(loading),
            Overlay::None
        ));
        assert!(matches!(
            overlay_after_abandoning_worktree_list(Overlay::Appearance),
            Overlay::Appearance
        ));
        let create = Overlay::WorktreeCreate {
            workspace_id: "w1".into(),
            advanced: false,
        };
        assert!(matches!(
            overlay_after_abandoning_worktree_list(create),
            Overlay::WorktreeCreate { .. }
        ));
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
        assert_eq!(action, EventPollAction::Applied { reordered: false });
        assert!(!effects_for(&action).resync);
        assert!(effects_for(&action).apply_local);
        assert!(effects_for(&action).reschedule);
        assert!(action.event_stream().is_none());
        assert_eq!(snapshot.panes[0], updated);
    }

    #[test]
    fn only_an_order_publishing_event_reports_a_settled_reorder() {
        let mut snapshot = two_tab_snapshot();
        let mut updated = snapshot.panes[0].clone();
        updated.revision = 9;
        let mut events = vec![
            Ok(Some(HerdrEvent::PaneUpdated { pane: updated })),
            Ok(None),
        ]
        .into_iter();
        assert_eq!(
            poll_event_stream(&mut snapshot, || events.next().unwrap()),
            EventPollAction::Applied { reordered: false },
            "a pane update leaves the order alone, so a pending reorder is still pending"
        );

        let workspaces = snapshot.workspaces.clone();
        let mut events = vec![
            Ok(Some(HerdrEvent::WorkspaceMoved {
                workspace_id: "w".into(),
                insert_index: 0,
                workspaces,
            })),
            Ok(None),
        ]
        .into_iter();
        assert_eq!(
            poll_event_stream(&mut snapshot, || events.next().unwrap()),
            EventPollAction::Applied { reordered: true }
        );
    }

    #[test]
    fn a_pending_reorder_is_released_by_everything_except_an_empty_poll() {
        assert!(effects_for(&EventPollAction::Applied { reordered: true }).settle_reorder);
        assert!(effects_for(&EventPollAction::Resync { error: None }).settle_reorder);
        assert!(
            effects_for(&EventPollAction::Disconnect("stream died".into())).settle_reorder,
            "a dead stream will never deliver the moved event, so the gate must open"
        );
        assert!(!effects_for(&EventPollAction::Applied { reordered: false }).settle_reorder);
        assert!(
            !effects_for(&EventPollAction::Idle).settle_reorder,
            "an empty poll says nothing about the request still in flight"
        );
    }

    #[test]
    fn applied_poll_effects_do_not_resync() {
        let applied = effects_for(&EventPollAction::Applied { reordered: false });
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
        let light = terminal_palette_from_theme(family.light, false, Some(&family), &font);
        let dark = terminal_palette_from_theme(family.dark, true, Some(&family), &font);
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
        let left = terminal_palette_from_theme(family.light, false, Some(&family), &default);
        let right = terminal_palette_from_theme(family.light, false, Some(&family), &menlo);
        assert_eq!(right.font_family, "Menlo");
        assert_eq!(right.font_size, 16);
        assert!(!right.ligatures);
        assert!(right.thicken);
        assert_ne!(left.signature(), right.signature());
    }

    #[test]
    fn terminal_ansi_bright_slots_are_distinct_and_lighter_than_normal() {
        let ochub = ochub_ui::theme::ochub_family();
        let ember = ochub_ui::theme::ember_family();
        let custom = custom_theme_family("scarlet");
        for (label, family, palette, dark) in [
            ("ochub-dark", Some(&ochub), ochub.dark, true),
            ("ochub-light", Some(&ochub), ochub.light, false),
            ("ember-dark", Some(&ember), ember.dark, true),
            ("ember-light", Some(&ember), ember.light, false),
            ("custom-dark", Some(&custom), custom.dark, true),
            ("missing-dark", None, ochub.dark, true),
        ] {
            let ansi = terminal_ansi(family, &palette, dark);
            let unique: HashSet<u32> = ansi.iter().copied().collect();
            assert_eq!(unique.len(), 16, "{label}");
            for slot in 0..8 {
                assert_ne!(ansi[slot], ansi[slot + 8], "{label} slot {slot}");
                assert!(
                    ansi_luma(ansi[slot + 8]) > ansi_luma(ansi[slot]),
                    "{label} slot {slot}"
                );
            }
        }
    }

    #[test]
    fn switching_theme_family_changes_the_terminal_ansi_palette() {
        let font = TerminalFontSettings::default();
        let ochub = ochub_ui::theme::ochub_family();
        let ember = ochub_ui::theme::ember_family();
        let ochub_dark = terminal_palette_from_theme(ochub.dark, true, Some(&ochub), &font);
        let ember_dark = terminal_palette_from_theme(ember.dark, true, Some(&ember), &font);
        assert_ne!(ochub_dark.ansi, ember_dark.ansi);
        assert_eq!(ochub_dark.background, ochub.dark.bg.0);
        assert_eq!(ember_dark.background, ember.dark.bg.0);
        assert_eq!(ochub_dark.foreground, ochub.dark.text.0);
        assert_eq!(ember_dark.cursor, ember.dark.accent.0);
    }

    #[test]
    fn a_valid_custom_theme_family_gets_its_own_ansi_palette() {
        let ochub = ochub_ui::theme::ochub_family();
        let custom = custom_theme_family("scarlet");
        let ochub_ansi = terminal_ansi(Some(&ochub), &ochub.dark, true);
        let custom_ansi = terminal_ansi(Some(&custom), &custom.dark, true);
        let missing_ansi = terminal_ansi(None, &ochub.dark, true);
        assert_eq!(ochub_ansi, OCHUB_DARK_ANSI);
        assert_eq!(missing_ansi, OCHUB_DARK_ANSI);
        assert_ne!(custom_ansi, ochub_ansi);
        assert_ne!(custom_ansi, missing_ansi);
    }

    fn custom_theme_family(id: &str) -> theme::ThemeFamily {
        let mut dark = theme::OCHUB_DARK;
        dark.red = theme::ThemeColor::new(0xE23D48);
        dark.green = theme::ThemeColor::new(0x2FBF71);
        dark.yellow = theme::ThemeColor::new(0xF0C400);
        dark.accent = theme::ThemeColor::new(0x3D8BFF);
        dark.mauve = theme::ThemeColor::new(0xC45CFF);
        dark.teal = theme::ThemeColor::new(0x1EC8B8);
        theme::ThemeFamily {
            schema_version: theme::THEME_SCHEMA_VERSION,
            id: id.into(),
            name: id.into(),
            author: String::new(),
            description: String::new(),
            light: theme::OCHUB_LIGHT,
            dark,
        }
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

    #[test]
    fn pointer_along_split_uses_the_layout_area_origin() {
        let area = LayoutRect {
            x: 10,
            y: 20,
            width: 80,
            height: 40,
        };
        let surface = (100., 50., 400., 200.);
        assert_eq!(
            pointer_along_split(SplitDirection::Right, area, surface, (100., 50.)),
            Some(10.)
        );
        assert_eq!(
            pointer_along_split(SplitDirection::Right, area, surface, (300., 50.)),
            Some(50.)
        );
        assert_eq!(
            pointer_along_split(SplitDirection::Down, area, surface, (100., 150.)),
            Some(40.)
        );
    }

    fn split_area() -> LayoutRect {
        LayoutRect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        }
    }

    fn layout_snapshot(splits: &[(&str, f32)], panes: &[(&str, LayoutRect)]) -> HierarchySnapshot {
        let area = split_area();
        HierarchySnapshot {
            panes: panes.iter().map(|(id, _)| test_pane(id, "t1")).collect(),
            layouts: vec![ocherdr_core::PaneLayout {
                workspace_id: "w".into(),
                tab_id: "t1".into(),
                zoomed: false,
                area,
                focused_pane_id: panes[0].0.into(),
                panes: panes
                    .iter()
                    .map(|(id, rect)| ocherdr_core::LayoutPane {
                        pane_id: (*id).into(),
                        focused: false,
                        rect: *rect,
                    })
                    .collect(),
                splits: splits
                    .iter()
                    .map(|(id, ratio)| LayoutSplit {
                        id: (*id).into(),
                        direction: SplitDirection::Right,
                        ratio: *ratio,
                        rect: area,
                    })
                    .collect(),
            }],
            ..Default::default()
        }
    }

    fn split_drag_on(snapshot: &HierarchySnapshot) -> SplitDrag {
        split_drag_at(snapshot, 0)
    }

    fn split_drag_at(snapshot: &HierarchySnapshot, split_index: usize) -> SplitDrag {
        let layout = &snapshot.layouts[0];
        let split = &layout.splits[split_index];
        SplitDrag {
            workspace_id: layout.workspace_id.clone(),
            tab_id: layout.tab_id.clone(),
            path: split.path().expect("test split ids encode a path"),
            layout: split_layout_fingerprint(layout),
            direction: split.direction,
            rect: split.rect,
            grab_offset: 0.,
            preview_ratio: split.ratio,
            start_ratio: split.ratio,
        }
    }

    fn nested_layout(root_ratio: f32) -> HierarchySnapshot {
        let area = split_area();
        let left_w = (f32::from(area.width) * root_ratio).round() as u16;
        let right_w = area.width - left_w;
        let nested = LayoutRect {
            x: area.x,
            y: area.y,
            width: left_w,
            height: area.height,
        };
        let top_h = nested.height / 2;
        HierarchySnapshot {
            panes: ["p-top", "p-bot", "p-right"]
                .into_iter()
                .map(|id| test_pane(id, "t1"))
                .collect(),
            layouts: vec![ocherdr_core::PaneLayout {
                workspace_id: "w".into(),
                tab_id: "t1".into(),
                zoomed: false,
                area,
                focused_pane_id: "p-top".into(),
                panes: vec![
                    ocherdr_core::LayoutPane {
                        pane_id: "p-top".into(),
                        focused: false,
                        rect: LayoutRect {
                            x: nested.x,
                            y: nested.y,
                            width: left_w,
                            height: top_h,
                        },
                    },
                    ocherdr_core::LayoutPane {
                        pane_id: "p-bot".into(),
                        focused: false,
                        rect: LayoutRect {
                            x: nested.x,
                            y: nested.y + top_h,
                            width: left_w,
                            height: nested.height - top_h,
                        },
                    },
                    ocherdr_core::LayoutPane {
                        pane_id: "p-right".into(),
                        focused: false,
                        rect: LayoutRect {
                            x: nested.x + left_w,
                            y: area.y,
                            width: right_w,
                            height: area.height,
                        },
                    },
                ],
                splits: vec![
                    LayoutSplit {
                        id: "split_0_root".into(),
                        direction: SplitDirection::Right,
                        ratio: root_ratio,
                        rect: area,
                    },
                    LayoutSplit {
                        id: "split_1_0".into(),
                        direction: SplitDirection::Down,
                        ratio: 0.5,
                        rect: nested,
                    },
                ],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn reconciling_a_split_drag_stays_split_when_only_the_ratio_changes() {
        let left = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        };
        let right = LayoutRect {
            x: 50,
            y: 0,
            width: 50,
            height: 50,
        };
        let before = layout_snapshot(
            &[("split_0_root", 0.5)],
            &[("p-left", left), ("p-right", right)],
        );
        let after = layout_snapshot(
            &[("split_0_root", 0.7)],
            &[
                (
                    "p-left",
                    LayoutRect {
                        x: 0,
                        y: 0,
                        width: 70,
                        height: 50,
                    },
                ),
                (
                    "p-right",
                    LayoutRect {
                        x: 70,
                        y: 0,
                        width: 30,
                        height: 50,
                    },
                ),
            ],
        );
        assert_eq!(
            split_layout_fingerprint(&before.layouts[0]),
            split_layout_fingerprint(&after.layouts[0])
        );
        assert!(matches!(
            reconcile_split_drag_state(split_drag_on(&before), Some(&after)),
            SurfaceDrag::Split(_)
        ));
    }

    #[test]
    fn reconciling_a_split_drag_goes_idle_when_a_pane_is_replaced() {
        let left = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        };
        let right = LayoutRect {
            x: 50,
            y: 0,
            width: 50,
            height: 50,
        };
        let before = layout_snapshot(
            &[("split_0_root", 0.5)],
            &[("p-left", left), ("p-right", right)],
        );
        let after = layout_snapshot(
            &[("split_0_root", 0.5)],
            &[("p-left", left), ("p-other", right)],
        );
        assert!(matches!(
            reconcile_split_drag_state(split_drag_on(&before), Some(&after)),
            SurfaceDrag::Idle
        ));
    }

    #[test]
    fn reconciling_a_split_drag_goes_idle_when_a_pane_is_added_before_layout_updated() {
        let left = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        };
        let right = LayoutRect {
            x: 50,
            y: 0,
            width: 50,
            height: 50,
        };
        let before = layout_snapshot(
            &[("split_0_root", 0.5)],
            &[("p-left", left), ("p-right", right)],
        );
        let mut after = before.clone();
        after.panes.push(test_pane("p-new", "t1"));
        assert!(matches!(
            reconcile_split_drag_state(split_drag_on(&before), Some(&after)),
            SurfaceDrag::Idle
        ));
    }

    #[test]
    fn selecting_a_pane_voids_a_split_drag_only_when_leaving_its_tab_or_workspace() {
        let left = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        };
        let right = LayoutRect {
            x: 50,
            y: 0,
            width: 50,
            height: 50,
        };
        let drag = split_drag_on(&layout_snapshot(
            &[("split_0_root", 0.5)],
            &[("p-left", left), ("p-right", right)],
        ));
        assert!(!split_drag_voided_by_pane(&drag, Some("w"), Some("t1")));
        assert!(split_drag_voided_by_pane(&drag, Some("w"), Some("t-other")));
        assert!(split_drag_voided_by_pane(
            &drag,
            Some("w-other"),
            Some("t1")
        ));
        assert!(split_drag_voided_by_pane(&drag, None, None));
    }

    #[test]
    fn reconciling_a_split_drag_stays_split_when_an_ancestor_ratio_moves_a_nested_rect() {
        let before = nested_layout(0.5);
        let after = nested_layout(0.7);
        assert_ne!(
            before.layouts[0].splits[1].rect,
            after.layouts[0].splits[1].rect
        );
        assert_eq!(
            split_layout_fingerprint(&before.layouts[0]),
            split_layout_fingerprint(&after.layouts[0])
        );
        assert!(matches!(
            reconcile_split_drag_state(split_drag_at(&before, 1), Some(&after)),
            SurfaceDrag::Split(_)
        ));
    }

    fn agent_panel(pane_id: &str) -> Overlay {
        Overlay::AgentPanel {
            pane_id: pane_id.into(),
        }
    }

    fn snapshot_with_agent(pane_id: &str, agent: Option<&str>) -> HierarchySnapshot {
        let mut snapshot = two_tab_snapshot();
        snapshot.panes[0].pane_id = pane_id.into();
        snapshot.panes[0].agent = agent.map(str::to_owned);
        snapshot.panes[0].display_agent = agent.map(str::to_owned);
        snapshot
    }

    #[test]
    fn agent_panel_closes_when_the_pane_or_agent_is_gone() {
        let overlay = agent_panel("p-a");
        assert!(!agent_panel_target_missing(
            &overlay,
            Some(&snapshot_with_agent("p-a", Some("grok"))),
        ));
        assert!(agent_panel_target_missing(
            &overlay,
            Some(&snapshot_with_agent("p-a", None)),
        ));
        assert!(agent_panel_target_missing(
            &overlay,
            Some(&snapshot_with_agent("p-b", Some("grok"))),
        ));
        assert!(agent_panel_target_missing(&overlay, None));
        assert!(!agent_panel_target_missing(&Overlay::Appearance, None));
    }

    #[test]
    fn agent_prompt_preserves_the_user_text_and_rejects_only_exact_empty() {
        assert_eq!(
            agent_prompt_text_to_send("  hello  "),
            Some("  hello  ".into())
        );
        assert_eq!(agent_prompt_text_to_send("   "), Some("   ".into()));
        assert_eq!(agent_prompt_text_to_send(""), None);
    }

    #[test]
    fn agent_read_parses_text_and_truncated() {
        let value = json!({
            "type": "pane_read",
            "read": { "text": "hello\n", "truncated": true }
        });
        assert_eq!(
            parse_agent_read_result(&value).unwrap(),
            ("hello\n".into(), true)
        );
        assert!(parse_agent_read_result(&json!({ "ok": true })).is_err());
        assert!(parse_agent_read_result(&json!({ "read": { "text": "hello" } })).is_err());
        assert!(
            parse_agent_read_result(&json!({ "read": { "text": "hello", "truncated": "false" } }))
                .is_err()
        );
    }

    #[test]
    fn agent_info_parses_the_custom_name_instead_of_display_metadata() {
        let agent = parse_agent_info_result(json!({
            "agent": {
                "pane_id": "p-a",
                "name": "reviewer",
                "agent": "claude",
                "display_agent": "Claude Code"
            }
        }))
        .unwrap();
        assert_eq!(agent.pane_id, "p-a");
        assert_eq!(agent.name.as_deref(), Some("reviewer"));
    }

    #[test]
    fn agent_panel_refreshes_output_from_that_pane_status_events() {
        let overlay = agent_panel("p-a");
        let status = HerdrEvent::PaneAgentStatusChanged {
            pane_id: "p-a".into(),
            workspace_id: "w".into(),
            agent_status: AgentStatus::Working,
            agent: Some("grok".into()),
            title: None,
            display_agent: Some("grok".into()),
            state_labels: HashMap::new(),
        };
        let other = HerdrEvent::PaneAgentStatusChanged {
            pane_id: "p-b".into(),
            workspace_id: "w".into(),
            agent_status: AgentStatus::Done,
            agent: Some("grok".into()),
            title: None,
            display_agent: Some("grok".into()),
            state_labels: HashMap::new(),
        };
        assert!(agent_panel_refresh_from_batch(
            &overlay,
            Some(&[Ok(status.clone())]),
        ));
        assert!(!agent_panel_refresh_from_batch(
            &overlay,
            Some(&[Ok(other)]),
        ));
        assert!(!agent_panel_refresh_from_batch(
            &Overlay::Appearance,
            Some(&[Ok(status)])
        ));
    }

    fn workspace_snapshot(ids: &[&str], labels: &[&str]) -> HierarchySnapshot {
        HierarchySnapshot {
            workspaces: ids
                .iter()
                .zip(labels)
                .map(|(id, label)| {
                    let mut workspace = sample_workspace(id, None);
                    workspace.label = (*label).into();
                    workspace
                })
                .collect(),
            ..Default::default()
        }
    }

    fn workspace_reorder_on(snapshot: &HierarchySnapshot, source: usize) -> ReorderDrag {
        ReorderDrag {
            list: ReorderList::Workspaces,
            source_index: source,
            order: snapshot
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace_id.clone())
                .collect(),
            previous_hover: ReorderHover::AfterLast,
            hover: ReorderHover::AfterLast,
            origin: (0., 0.),
            pointer: (0., 0.),
            grab_offset: (0., 0.),
            source_rect: (0., 0., 0., 0.),
        }
    }

    #[test]
    fn reconciling_a_reorder_drag_goes_idle_when_list_order_changes() {
        let before = workspace_snapshot(&["w1", "w2", "w3"], &["a", "b", "c"]);
        let after = workspace_snapshot(&["w2", "w1", "w3"], &["b", "a", "c"]);
        assert!(matches!(
            reconcile_reorder_drag_state(workspace_reorder_on(&before, 0), Some(&after)),
            SurfaceDrag::Idle
        ));
    }

    #[test]
    fn reconciling_a_reorder_drag_stays_when_only_a_label_changes() {
        let before = workspace_snapshot(&["w1", "w2"], &["a", "b"]);
        let after = workspace_snapshot(&["w1", "w2"], &["a", "renamed"]);
        assert!(matches!(
            reconcile_reorder_drag_state(workspace_reorder_on(&before, 0), Some(&after)),
            SurfaceDrag::Reorder(_)
        ));
    }
}

#[cfg(test)]
mod gpui_tests;
