use std::collections::HashSet;
use std::task::Poll;

use ocherdr_core::{WorkspaceInfo, WorktreeInfo, WorktreeList, WorktreeSourceInfo};

use futures::future::{self, Either, poll_fn};
use futures::pin_mut;
use ocherdr_core::{
    AGENT_OUTPUT_SOURCE, AGENT_STATUS_HANDOFF_LIMIT, AgentStatusHandoff,
    agent_output_should_refresh, agent_status_panes_after_stream_closed,
    agent_status_stream_should_rebuild, event_panes_after_failed_subscribe, parse_agent_name,
    reorder_hover_along_axis, reorder_insert_index,
};
use ocherdr_herdr::{TerminalEvent, TerminalEventReceiver, next_batch, subscribe_agent_status};

use ochub_ui::notifications::NotificationHost;

use super::*;
use crate::notify::{FailureKind, FailureNotice, command_notification, notification_for};

mod agent;
mod appearance;
mod events;
mod hierarchy;
mod input;
mod pane_keyboard;
mod pane_layout;
mod pane_tab_drop;
mod pane_templates;
mod reorder;
mod runtime;
mod support;
mod terminal;
mod worktree;

pub(crate) use support::split_layout_fingerprint;
pub(super) use support::*;

/// There is no session picker in the shell: an explicit reconnect keeps its
/// current session, while a fresh host connection prefers that host's running
/// default and only then falls back to another running session.
fn preferred_session_index(
    sessions: &[SessionSummary],
    preferred_session: Option<&str>,
) -> Option<usize> {
    preferred_session
        .and_then(|name| sessions.iter().position(|session| session.name == name))
        .or_else(|| {
            sessions
                .iter()
                .position(|session| session.default && session.running)
        })
        .or_else(|| sessions.iter().position(|session| session.running))
}

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
        let pane_edge_relocation =
            crate::config::values::AppConfig::from_document(&loaded.document)
                .0
                .pane_edge_relocation;
        let focus = cx.focus_handle();
        let dialog_focus = cx.focus_handle();
        let host_center = cx.new(|cx| HostCenter::new(settings, i18n, focus.clone(), cx));
        let profiles = host_center.read(cx).profiles().to_vec();
        cx.subscribe(&host_center, |this, _center, event, cx| {
            this.handle_host_center_event(event.clone(), cx);
        })
        .detach();
        // Command released in another app never reaches this window as a
        // modifiers change, so losing key status drops the hints.
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.set_command_held(false, cx);
            }
        })
        .detach();
        // A native window resize changes every pane's measured canvas bounds.
        // Keep a render queued so each canvas can publish its final geometry
        // even when AppKit's live-resize loop otherwise goes quiet on release.
        cx.observe_window_bounds(window, |_this, _window, cx| {
            cx.notify();
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
            herdr_capabilities: HerdrCapabilities::default(),
            event_stream: EventStreamState::Idle,
            event_listen: None,
            startup_replay_sync: None,
            startup_replay_serial: 0,
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
            dialog_focus,
            pending_focus: None,
            load_epoch: 0,
            event_epoch: 0,
            snapshot_refreshing: false,
            snapshot_refresh_pending: false,
            session_panes: None,
            pane_viewports: HashMap::new(),
            pane_mount_scheduled: false,
            overlay: Overlay::None,
            open_select: None,
            appearance_scroll: ScrollHandle::new(),
            appearance_ui: Default::default(),
            tab_scroll: ScrollHandle::new(),
            hovered_tab_id: None,
            pending_created_tab: None,
            tab_preview_task: None,
            tab_preview_id: None,
            tab_preview_goal: None,
            tab_preview_hovered: false,
            tab_close_reveals: HashMap::new(),
            command_held: false,
            shortcut_reveal: Transition::settled(0., TAB_SHORTCUT_ANIMATION),
            prefix_pending: false,
            suppress_key_release: false,
            surface_drag: SurfaceDrag::Idle,
            split_commit: None,
            pane_drag_snapshot: None,
            pane_relocations: HashMap::new(),
            pane_detaches: HashMap::new(),
            pane_template_commits: HashMap::new(),
            pane_drag_return: None,
            pane_resize_frozen_tabs: HashSet::new(),
            pane_resize_serial: 0,
            pane_relocation_serial: 0,
            pane_edge_relocation,
            pane_keyboard_move: None,
            parked_recovery: None,
            #[cfg(test)]
            headless_terminals: false,
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
        self.startup_replay_sync = None;
        self.startup_replay_serial = self.startup_replay_serial.wrapping_add(1);
        self.agent_status_listen = None;
        self.agent_status_rebuild = None;
        self.agent_status_panes.clear();
        self.agent_status_handoff = None;
        self.pending_created_tab = None;
        self.surface_drag = SurfaceDrag::Idle;
        self.split_commit = None;
        self.pane_resize_frozen_tabs.clear();
        self.pane_resize_serial = self.pane_resize_serial.wrapping_add(1);
        if let Some(session) = self.session_panes.as_mut() {
            for runtime in session.panes.values_mut() {
                runtime.pending_resize = None;
            }
        }
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
                    let selected = preferred_session_index(&sessions, preferred_session.as_deref());
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
                        this.herdr_capabilities = this
                            .snapshot
                            .as_ref()
                            .map(HerdrCapabilities::from_snapshot)
                            .unwrap_or_default();
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
                                this.schedule_startup_replay_quiet(cx);
                                this.event_listen = Some(Self::listen_events(subscription, cx));
                            }
                        }
                        this.ensure_agent_status_stream(cx);
                    }
                    Err(error) => {
                        this.sessions.clear();
                        this.session_index = None;
                        this.connection = None;
                        this.herdr_capabilities = HerdrCapabilities::default();
                        this.event_stream = EventStreamState::Idle;
                        this.event_listen = None;
                        this.startup_replay_sync = None;
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
        self.herdr_capabilities = HerdrCapabilities::default();
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
        if overlay.is_confirm_dialog() {
            self.pending_focus = Some(PendingFocus::Dialog);
        } else if self.overlay.is_confirm_dialog() && matches!(overlay, Overlay::None) {
            // The dialog element goes away with its focus; keys would then
            // reach only the window root, so hand focus back to the surface.
            self.pending_focus = Some(PendingFocus::Surface);
        }
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
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod gpui_tests;
