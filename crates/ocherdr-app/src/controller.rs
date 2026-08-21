use super::*;

impl OcHerdrView {
    pub(super) fn new(settings: Settings, cx: &mut Context<Self>) -> Self {
        let i18n = I18n::new(settings.language);
        let mut profiles = vec![ConnectionProfile::default()];
        profiles.extend(settings.connections);
        let saved_destinations = profiles
            .iter()
            .filter_map(|profile| match profile {
                ConnectionProfile::Ssh { destination, .. } => Some(destination.clone()),
                ConnectionProfile::Local { .. } => None,
            })
            .collect::<Vec<_>>();
        profiles.extend(
            ssh_host_aliases()
                .into_iter()
                .filter(|host| !saved_destinations.contains(host))
                .enumerate()
                .map(|(index, host)| ConnectionProfile::Ssh {
                    id: format!("ssh-{index}-{host}"),
                    label: host.clone(),
                    destination: host,
                    port: None,
                    identity_file: None,
                    herdr_path: "herdr".into(),
                }),
        );
        let remote_search = cx.new(|cx| {
            TextInput::new(cx, i18n.text("Search hosts"))
                .search_field()
                .compact()
        });
        cx.subscribe(&remote_search, |this, _input, _: &TextInputEvent, cx| {
            this.ensure_managed_profile_visible(cx);
        })
        .detach();
        let mut view = Self {
            profiles,
            profile_index: 0,
            sessions: Vec::new(),
            session_index: None,
            connection: None,
            events: None,
            snapshot: None,
            selection: Selection {
                connection_id: "local".into(),
                ..Default::default()
            },
            operation: None,
            error: None,
            focus: cx.focus_handle(),
            load_epoch: 0,
            event_epoch: 0,
            snapshot_refreshing: false,
            terminal_epoch: 0,
            panes: HashMap::new(),
            node_manager_open: false,
            add_remote_open: false,
            appearance_open: false,
            herdr_settings_open: false,
            herdr_settings_section: 0,
            managed_profile_index: 0,
            pending_remove_profile: None,
            pending_close: None,
            rename_target: None,
            context_menu: None,
            prefix_pending: false,
            remote_label: cx.new(|cx| TextInput::new(cx, i18n.text("Production"))),
            remote_destination: cx
                .new(|cx| TextInput::new(cx, i18n.text("user@example.com or SSH alias"))),
            remote_port: cx.new(|cx| TextInput::new(cx, i18n.text("22 (optional)"))),
            remote_identity_file: cx
                .new(|cx| TextInput::new(cx, i18n.text("~/.ssh/id_ed25519 (optional)"))),
            remote_herdr_path: cx.new(|cx| TextInput::new(cx, "herdr").with_content("herdr")),
            remote_search,
            rename_input: cx.new(|cx| TextInput::new(cx, i18n.text("Name"))),
            appearance: settings.appearance,
            i18n,
        };
        view.reload(None, cx);
        view
    }

    pub(super) fn current_profile(&self) -> ConnectionProfile {
        self.profiles[self.profile_index].clone()
    }

    pub(super) fn current_session(&self) -> Option<&SessionSummary> {
        self.session_index
            .and_then(|index| self.sessions.get(index))
    }

    pub(super) fn reload(&mut self, preferred_session: Option<String>, cx: &mut Context<Self>) {
        self.load_epoch = self.load_epoch.wrapping_add(1);
        self.event_epoch = self.event_epoch.wrapping_add(1);
        self.events = None;
        self.snapshot_refreshing = false;
        let epoch = self.load_epoch;
        let profile = self.current_profile();
        self.error = None;
        self.operation = Some(self.i18n.text("Discovering Herdr sessions…").into());
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
                            let events = connection.subscribe_background().ok();
                            (Some(connection), events, Some(snapshot))
                        } else {
                            (None, None, None)
                        }
                    } else {
                        (None, None, None)
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
                        this.events = loaded.events;
                        this.snapshot = loaded.snapshot;
                        this.selection.connection_id = this.current_profile().id().into();
                        this.selection.session_name =
                            this.current_session().map(|s| s.name.clone());
                        if let Some(snapshot) = &this.snapshot {
                            this.selection.reconcile(snapshot);
                        }
                        this.start_visible_terminals(cx);
                        if this.events.is_some() {
                            this.schedule_event_poll(this.event_epoch, cx);
                        }
                    }
                    Err(error) => {
                        this.sessions.clear();
                        this.session_index = None;
                        this.connection = None;
                        this.events = None;
                        this.snapshot = None;
                        this.panes.clear();
                        this.error = Some(error.to_string().into());
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
        self.sessions.clear();
        self.session_index = None;
        self.connection = None;
        self.events = None;
        self.snapshot = None;
        self.panes.clear();
        self.reload(None, cx);
    }

    pub(super) fn schedule_event_poll(&self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            this.update(cx, |this, cx| {
                if this.event_epoch != epoch || this.events.is_none() {
                    return;
                }
                let mut changed = false;
                let mut stream_error = None;
                if let Some(events) = &this.events {
                    for _ in 0..128 {
                        match events.try_event() {
                            Ok(Some(_)) => changed = true,
                            Ok(None) => break,
                            Err(error) => {
                                stream_error = Some(error.to_string().into());
                                break;
                            }
                        }
                    }
                }
                if let Some(error) = stream_error {
                    this.error = Some(error);
                }
                if changed {
                    this.refresh_snapshot_from_event(epoch, cx);
                }
                this.schedule_event_poll(epoch, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn refresh_snapshot_from_event(&mut self, epoch: u64, cx: &mut Context<Self>) {
        if self.snapshot_refreshing {
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        self.snapshot_refreshing = true;
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
                        let old_panes = this
                            .snapshot
                            .as_ref()
                            .zip(old_tab.as_deref())
                            .map(|(snapshot, tab)| {
                                snapshot
                                    .panes_for(tab)
                                    .map(|pane| pane.pane_id.clone())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        this.snapshot = Some(snapshot);
                        if let Some(snapshot) = &this.snapshot {
                            this.selection.reconcile(snapshot);
                        }
                        let new_panes = this
                            .snapshot
                            .as_ref()
                            .zip(this.selection.tab_id.as_deref())
                            .map(|(snapshot, tab)| {
                                snapshot
                                    .panes_for(tab)
                                    .map(|pane| pane.pane_id.clone())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        if old_tab != this.selection.tab_id || old_panes != new_panes {
                            this.start_visible_terminals(cx);
                        }
                        cx.notify();
                    }
                    Err(error) => {
                        this.error = Some(error.to_string().into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn open_add_remote(&mut self, cx: &mut Context<Self>) {
        self.add_remote_open = true;
        self.error = None;
        cx.notify();
    }

    pub(super) fn open_node_manager(&mut self, cx: &mut Context<Self>) {
        self.node_manager_open = true;
        self.appearance_open = false;
        self.herdr_settings_open = false;
        self.context_menu = None;
        self.managed_profile_index = self.profile_index;
        self.error = None;
        cx.notify();
    }

    pub(super) fn close_node_manager(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.node_manager_open = false;
        self.add_remote_open = false;
        self.pending_remove_profile = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn open_appearance(&mut self, cx: &mut Context<Self>) {
        self.appearance_open = true;
        self.node_manager_open = false;
        self.herdr_settings_open = false;
        self.context_menu = None;
        cx.notify();
    }

    pub(super) fn close_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.appearance_open = false;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn open_herdr_settings(&mut self, cx: &mut Context<Self>) {
        self.herdr_settings_open = true;
        self.node_manager_open = false;
        self.appearance_open = false;
        self.context_menu = None;
        cx.notify();
    }

    pub(super) fn close_herdr_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.herdr_settings_open = false;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn select_herdr_settings_section(&mut self, index: usize, cx: &mut Context<Self>) {
        self.herdr_settings_section = index.min(HERDR_SETTINGS_SECTIONS.len() - 1);
        cx.notify();
    }

    pub(super) fn select_managed_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.profiles.len() {
            self.managed_profile_index = index;
            self.add_remote_open = false;
            cx.notify();
        }
    }

    pub(super) fn ensure_managed_profile_visible(&mut self, cx: &mut Context<Self>) {
        let query = self.remote_search.read(cx).content().trim().to_lowercase();
        if self
            .profiles
            .get(self.managed_profile_index)
            .is_some_and(|profile| profile_matches_search(profile, &query, self.i18n))
        {
            cx.notify();
            return;
        }
        if let Some(index) = self
            .profiles
            .iter()
            .position(|profile| profile_matches_search(profile, &query, self.i18n))
        {
            self.managed_profile_index = index;
        }
        cx.notify();
    }

    pub(super) fn choose_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.node_manager_open = false;
        if index == self.profile_index {
            cx.notify();
        } else {
            self.select_profile(index, cx);
        }
    }

    pub(super) fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.appearance.background_opacity = self.appearance.background_opacity.clamp(40, 100);
        self.appearance.theme_family = install_appearance(&self.appearance, window.appearance());
        theme::apply_window_background(window);
        if let Err(error) = save_settings(&self.profiles, &self.appearance, self.i18n.preference())
        {
            self.error = Some(error.into());
        }
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
        opacity: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.background_opacity = opacity;
        self.apply_appearance(window, cx);
    }

    pub(super) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.i18n.set_preference(language);
        theme::reload_registry();
        self.remote_search.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text("Search hosts"), cx)
        });
        self.remote_label.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text("Production"), cx)
        });
        self.remote_destination.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text("user@example.com or SSH alias"), cx)
        });
        self.remote_port.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text("22 (optional)"), cx)
        });
        self.remote_identity_file.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text("~/.ssh/id_ed25519 (optional)"), cx)
        });
        self.rename_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text("Name"), cx)
        });
        if let Err(error) = save_settings(&self.profiles, &self.appearance, self.i18n.preference())
        {
            self.error = Some(error.into());
        }
        cx.notify();
    }

    pub(super) fn request_remove_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if self
            .profiles
            .get(index)
            .is_some_and(|profile| profile.id().starts_with("manual-"))
        {
            self.pending_remove_profile = Some(index);
            cx.notify();
        }
    }

    pub(super) fn cancel_remove_node(&mut self, cx: &mut Context<Self>) {
        self.pending_remove_profile = None;
        cx.notify();
    }

    pub(super) fn confirm_remove_node(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.pending_remove_profile.take() else {
            return;
        };
        if index == 0 || index >= self.profiles.len() {
            return;
        }
        let removed = self.profiles.remove(index);
        if let Err(error) = save_settings(&self.profiles, &self.appearance, self.i18n.preference())
        {
            self.profiles.insert(index, removed);
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        if index == self.profile_index {
            self.profile_index = 0;
            self.reload(None, cx);
        } else {
            if index < self.profile_index {
                self.profile_index -= 1;
            }
            cx.notify();
        }
        self.managed_profile_index = self.managed_profile_index.min(self.profiles.len() - 1);
    }

    pub(super) fn close_add_remote(&mut self, cx: &mut Context<Self>) {
        self.add_remote_open = false;
        cx.notify();
    }

    pub(super) fn request_close(&mut self, target: HierarchyTarget, cx: &mut Context<Self>) {
        self.pending_close = Some(target);
        self.context_menu = None;
        cx.notify();
    }

    pub(super) fn cancel_close(&mut self, cx: &mut Context<Self>) {
        self.pending_close = None;
        cx.notify();
    }

    pub(super) fn confirm_close(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.pending_close.take() {
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
    }

    pub(super) fn open_context_menu(
        &mut self,
        target: HierarchyTarget,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = window.viewport_size();
        self.context_menu = Some(HierarchyContextMenu {
            target,
            x: f32::from(event.position.x)
                .min((f32::from(viewport.width) - 220.).max(8.))
                .max(8.),
            y: f32::from(event.position.y)
                .min((f32::from(viewport.height) - 260.).max(8.))
                .max(8.),
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        cx.notify();
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
        self.rename_target = Some(target);
        self.context_menu = None;
        self.rename_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(super) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_target = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn submit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.rename_target.take() else {
            return;
        };
        let label = self.rename_input.read(cx).content().trim().to_owned();
        if label.is_empty() && !matches!(target, HierarchyTarget::Pane { .. }) {
            self.error = Some(
                self.i18n
                    .text("Workspace and tab names cannot be empty.")
                    .into(),
            );
            self.rename_target = Some(target);
            cx.notify();
            return;
        }
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

    pub(super) fn save_remote(&mut self, cx: &mut Context<Self>) {
        let destination = self.remote_destination.read(cx).content().trim().to_owned();
        if destination.is_empty() {
            self.error = Some(self.i18n.text("SSH destination is required.").into());
            return;
        }
        let label = self.remote_label.read(cx).content().trim().to_owned();
        let port_text = self.remote_port.read(cx).content().trim().to_owned();
        let identity_file = self
            .remote_identity_file
            .read(cx)
            .content()
            .trim()
            .to_owned();
        let herdr_path = self.remote_herdr_path.read(cx).content().trim().to_owned();
        let port = if port_text.is_empty() {
            None
        } else {
            match port_text.parse::<u16>() {
                Ok(port) if port > 0 => Some(port),
                _ => {
                    self.error = Some(
                        self.i18n
                            .text("SSH port must be a number from 1 to 65535.")
                            .into(),
                    );
                    return;
                }
            }
        };
        let next_id = self
            .profiles
            .iter()
            .filter_map(|profile| profile.id().strip_prefix("manual-"))
            .filter_map(|suffix| suffix.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let profile = ConnectionProfile::Ssh {
            id: format!("manual-{next_id}"),
            label: if label.is_empty() {
                destination.clone()
            } else {
                label
            },
            destination,
            port,
            identity_file: (!identity_file.is_empty()).then(|| PathBuf::from(identity_file)),
            herdr_path: if herdr_path.is_empty() {
                "herdr".into()
            } else {
                herdr_path
            },
        };
        self.profiles.push(profile);
        if let Err(error) = save_settings(&self.profiles, &self.appearance, self.i18n.preference())
        {
            self.profiles.pop();
            self.error = Some(error.into());
            return;
        }
        self.profile_index = self.profiles.len() - 1;
        self.add_remote_open = false;
        self.node_manager_open = false;
        self.remote_label
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_destination
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_port
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_identity_file
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_herdr_path
            .update(cx, |input, cx| input.set_content("herdr", cx));
        self.reload(None, cx);
    }

    pub(super) fn select_session(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get(index).cloned() else {
            return;
        };
        if !session.running {
            let command = attach_command(&self.current_profile(), &session.name);
            if let Err(error) = open_system_terminal(&command) {
                self.error = Some(error.to_string().into());
            }
            return;
        }
        self.session_index = Some(index);
        self.reload(Some(session.name), cx);
    }

    pub(super) fn open_native_tui(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.current_session() else {
            self.error = Some(self.i18n.text("No Herdr session is selected.").into());
            cx.notify();
            return;
        };
        let command = attach_command(&self.current_profile(), &session.name);
        if let Err(error) = open_system_terminal(&command) {
            self.error = Some(error.to_string().into());
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
        self.start_visible_terminals(cx);
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
        self.start_visible_terminals(cx);
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
            self.start_visible_terminals(cx);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn start_visible_terminals(&mut self, cx: &mut Context<Self>) {
        self.terminal_epoch = self.terminal_epoch.wrapping_add(1);
        let epoch = self.terminal_epoch;
        self.panes.clear();
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(tab_id) = self.selection.tab_id.as_deref() else {
            return;
        };
        let Some(session_name) = self.current_session().map(|session| session.name.clone()) else {
            return;
        };
        let profile = self.current_profile();
        for pane in snapshot.panes_for(tab_id) {
            let mode = if self.selection.pane_id.as_deref() == Some(&pane.pane_id) {
                TerminalMode::ControlTakeover
            } else {
                TerminalMode::Observe
            };
            let cols = 80;
            let rows = 24;
            let session = TerminalSession::spawn(
                profile.clone(),
                session_name.clone(),
                pane.pane_id.clone(),
                mode,
                cols,
                rows,
            );
            if let Ok(terminal) = Terminal::new(cols, rows, 10_000) {
                self.panes.insert(
                    pane.pane_id.clone(),
                    PaneRuntime {
                        session,
                        terminal,
                        text: self.i18n.text("Waiting for terminal frame…").into(),
                        mode,
                        size: (cols, rows),
                    },
                );
            }
        }
        self.schedule_terminal_poll(epoch, cx);
    }

    pub(super) fn schedule_terminal_poll(&self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            this.update(cx, |this, cx| {
                if this.terminal_epoch != epoch || this.panes.is_empty() {
                    return;
                }
                let mut changed = false;
                let mut error = None;
                for runtime in this.panes.values_mut() {
                    for _ in 0..64 {
                        match runtime.session.try_frame() {
                            Ok(Some(frame)) => {
                                let _ = runtime.terminal.resize(
                                    frame.width,
                                    frame.height,
                                    CELL_WIDTH as u32,
                                    CELL_HEIGHT as u32,
                                );
                                runtime.terminal.apply_frame(&frame.bytes, frame.full);
                                runtime.text = runtime.terminal.text().into();
                                runtime.size = (frame.width, frame.height);
                                changed = true;
                            }
                            Ok(None) => break,
                            Err(stream_error) => {
                                error = Some(stream_error.to_string().into());
                                break;
                            }
                        }
                    }
                }
                if let Some(error) = error {
                    this.error = Some(error);
                }
                if changed {
                    cx.notify();
                }
                this.schedule_terminal_poll(epoch, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn resize_visible_terminals(&mut self, window: &Window) {
        let viewport = window.viewport_size();
        let available_width = (f32::from(viewport.width) - SIDEBAR_WIDTH).max(320.);
        let available_height =
            (f32::from(viewport.height) - HEADER_HEIGHT - STATUS_BAR_HEIGHT).max(180.);
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(tab_id) = self.selection.tab_id.as_deref() else {
            return;
        };
        let layout = snapshot.layout_for(tab_id);
        for (pane_id, runtime) in &mut self.panes {
            let ratio = layout
                .and_then(|layout| {
                    layout
                        .panes
                        .iter()
                        .find(|pane| &pane.pane_id == pane_id)
                        .map(|pane| {
                            let width =
                                pane.rect.width.max(1) as f32 / layout.area.width.max(1) as f32;
                            let height =
                                pane.rect.height.max(1) as f32 / layout.area.height.max(1) as f32;
                            (width, height)
                        })
                })
                .unwrap_or((1., 1.));
            let cols = ((available_width * ratio.0 - 18.) / CELL_WIDTH)
                .floor()
                .max(1.) as u16;
            let rows = ((available_height * ratio.1 - PANE_HEADER_HEIGHT - 12.) / CELL_HEIGHT)
                .floor()
                .max(1.) as u16;
            if runtime.size != (cols, rows) {
                let _ = runtime
                    .terminal
                    .resize(cols, rows, CELL_WIDTH as u32, CELL_HEIGHT as u32);
                if runtime.mode == TerminalMode::ControlTakeover {
                    let _ = runtime.session.send(TerminalCommand::Resize {
                        cols,
                        rows,
                        cell_width_px: CELL_WIDTH as u32,
                        cell_height_px: CELL_HEIGHT as u32,
                    });
                }
                runtime.size = (cols, rows);
            }
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
                    snapshot
                        .tabs_for(workspace_id)
                        .nth(number.saturating_sub(1))
                        .map(|tab| tab.tab_id.clone())
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
            ("s", false) => self.open_herdr_settings(cx),
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
            (key, false) if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => {
                let number = (key.as_bytes()[0] - b'0') as usize;
                if number > 0 {
                    self.select_tab_number(number, cx);
                }
            }
            _ => {}
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
            self.context_menu = None;
            cx.notify();
            return true;
        }
        if self.prefix_pending {
            self.handle_prefix_key(event, window, cx);
            return true;
        }
        if key == "escape" {
            if self.context_menu.take().is_some()
                || self.node_manager_open
                || self.appearance_open
                || self.herdr_settings_open
            {
                self.node_manager_open = false;
                self.add_remote_open = false;
                self.appearance_open = false;
                self.herdr_settings_open = false;
                self.focus.focus(window, cx);
                cx.notify();
                return true;
            }
            if self.pending_close.take().is_some() {
                cx.notify();
                return true;
            }
        }
        if modifiers.platform && !modifiers.alt && !modifiers.control {
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
                    if let Some(target) = self.selected_tab_target() {
                        self.request_close(target, cx);
                    }
                    true
                }
                ("n", true) => {
                    self.create_workspace(cx);
                    true
                }
                (",", false) => {
                    self.open_herdr_settings(cx);
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
                (key, false) if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => {
                    let number = (key.as_bytes()[0] - b'0') as usize;
                    if number > 0 {
                        self.select_tab_number(number, cx);
                    }
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
        let Some(pane_id) = self.selection.pane_id.as_deref() else {
            return;
        };
        let Some(runtime) = self.panes.get(pane_id) else {
            return;
        };
        let key = &event.keystroke;
        if key.modifiers.platform && key.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let _ = runtime
                    .session
                    .send(TerminalCommand::Input(text.to_string()));
                cx.stop_propagation();
            }
            return;
        }
        if key.modifiers.platform {
            return;
        }
        let mut text = if key.modifiers.control && key.key.len() == 1 {
            let byte = key.key.as_bytes()[0].to_ascii_lowercase();
            if byte.is_ascii_lowercase() {
                String::from_utf8(vec![byte - b'a' + 1]).unwrap_or_default()
            } else {
                String::new()
            }
        } else if let Some(character) = &key.key_char {
            character.clone()
        } else {
            match key.key.as_str() {
                "enter" => "\r".into(),
                "tab" => "\t".into(),
                "backspace" => "\x7f".into(),
                "escape" => "\x1b".into(),
                "up" => "\x1b[A".into(),
                "down" => "\x1b[B".into(),
                "right" => "\x1b[C".into(),
                "left" => "\x1b[D".into(),
                "home" => "\x1b[H".into(),
                "end" => "\x1b[F".into(),
                "pageup" => "\x1b[5~".into(),
                "pagedown" => "\x1b[6~".into(),
                "delete" => "\x1b[3~".into(),
                _ => String::new(),
            }
        };
        if key.modifiers.alt && !text.is_empty() {
            text.insert(0, '\x1b');
        }
        if !text.is_empty() {
            let _ = runtime.session.send(TerminalCommand::Input(text));
            cx.stop_propagation();
        }
    }

    pub(super) fn invoke(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        let Some(connection) = &self.connection else {
            return;
        };
        let socket = connection.socket_path().to_owned();
        self.operation = Some(self.i18n.running_operation(method).into());
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    request_socket(&socket, method, params)?;
                    let result = request_socket(&socket, "session.snapshot", json!({}))?;
                    let snapshot = result.get("snapshot").cloned().ok_or_else(|| {
                        HerdrError::Protocol("snapshot result is missing `snapshot`".into())
                    })?;
                    Ok::<HierarchySnapshot, HerdrError>(serde_json::from_value(snapshot)?)
                })
                .await;
            this.update(cx, |this, cx| {
                this.operation = None;
                match result {
                    Ok(snapshot) => {
                        this.snapshot = Some(snapshot);
                        if let Some(snapshot) = &this.snapshot {
                            this.selection.reconcile(snapshot);
                        }
                        this.start_visible_terminals(cx);
                    }
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
