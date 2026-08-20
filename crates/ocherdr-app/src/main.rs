use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use gpui_platform::application;
use ocherdr_core::{
    AgentStatus, ConnectionProfile, HierarchySnapshot, PaneInfo, Selection, SessionSummary,
    SplitDirection,
};
use ocherdr_herdr::{
    EventSubscription, HerdrError, SessionConnection, TerminalCommand, TerminalMode,
    TerminalSession, attach_command, discover_sessions, open_system_terminal, request_socket,
    ssh_host_aliases,
};
use ocherdr_terminal::Terminal;
use ochub_ui::components::{
    ButtonSize, ButtonTone, button, empty_state, field, icon_button_tone, icon_only_button_tone,
    modal_body, modal_card, modal_footer, modal_header, modal_overlay, segmented, spinner,
    status_dot,
};
use ochub_ui::gpui::{
    App, AppContext, AssetSource, Bounds, Context, Entity, FocusHandle, FontWeight, IntoElement,
    KeyDownEvent, Render, SharedString, TitlebarOptions, Window, WindowAppearance, WindowBounds,
    WindowOptions, div, point, prelude::*, px, size,
};
use ochub_ui::icons::{IconName, icon};
use ochub_ui::text_input::{TextInput, TextInputEvent};
use ochub_ui::{assets, theme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const SIDEBAR_WIDTH: f32 = 252.;
const HEADER_HEIGHT: f32 = 46.;
const STATUS_BAR_HEIGHT: f32 = 28.;
const PANE_HEADER_HEIGHT: f32 = 26.;
const CELL_WIDTH: f32 = 8.4;
const CELL_HEIGHT: f32 = 17.;
const CORNER_MODAL: f32 = 16.;
const CORNER_PANEL: f32 = 12.;
const CORNER_CONTROL: f32 = 8.;
const CORNER_COMPACT: f32 = 6.;

struct OcHerdrAssets;

impl AssetSource for OcHerdrAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(assets::load(path))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(assets::list(path))
    }
}

struct LoadedSession {
    sessions: Vec<SessionSummary>,
    selected: Option<usize>,
    connection: Option<SessionConnection>,
    events: Option<EventSubscription>,
    snapshot: Option<HierarchySnapshot>,
}

struct PaneRuntime {
    session: TerminalSession,
    terminal: Terminal,
    text: SharedString,
    mode: TerminalMode,
    size: (u16, u16),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AppearanceMode {
    System,
    Light,
    #[default]
    Dark,
}

impl AppearanceMode {
    fn theme_mode(self) -> theme::ThemeMode {
        match self {
            Self::System => theme::ThemeMode::System,
            Self::Light => theme::ThemeMode::Light,
            Self::Dark => theme::ThemeMode::Dark,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::System => 0,
            Self::Light => 1,
            Self::Dark => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BackdropMode {
    Opaque,
    Transparent,
    #[default]
    Blurred,
}

impl BackdropMode {
    fn theme_effect(self) -> theme::ThemeWindowBackground {
        match self {
            Self::Opaque => theme::ThemeWindowBackground::Opaque,
            Self::Transparent => theme::ThemeWindowBackground::Transparent,
            Self::Blurred => theme::ThemeWindowBackground::Blurred,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Opaque => 0,
            Self::Transparent => 1,
            Self::Blurred => 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AppearanceSettings {
    #[serde(default = "default_theme_family")]
    theme_family: String,
    #[serde(default)]
    mode: AppearanceMode,
    #[serde(default)]
    backdrop: BackdropMode,
    #[serde(default = "default_background_opacity")]
    background_opacity: u8,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme_family: default_theme_family(),
            mode: AppearanceMode::Dark,
            backdrop: BackdropMode::Blurred,
            background_opacity: default_background_opacity(),
        }
    }
}

fn default_theme_family() -> String {
    theme::DEFAULT_THEME_FAMILY.to_owned()
}

const fn default_background_opacity() -> u8 {
    92
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    connections: Vec<ConnectionProfile>,
    #[serde(default)]
    appearance: AppearanceSettings,
}

struct OcHerdrView {
    profiles: Vec<ConnectionProfile>,
    profile_index: usize,
    sessions: Vec<SessionSummary>,
    session_index: Option<usize>,
    connection: Option<SessionConnection>,
    events: Option<EventSubscription>,
    snapshot: Option<HierarchySnapshot>,
    selection: Selection,
    operation: Option<SharedString>,
    error: Option<SharedString>,
    focus: FocusHandle,
    load_epoch: u64,
    event_epoch: u64,
    snapshot_refreshing: bool,
    terminal_epoch: u64,
    panes: HashMap<String, PaneRuntime>,
    node_manager_open: bool,
    add_remote_open: bool,
    appearance_open: bool,
    managed_profile_index: usize,
    pending_remove_profile: Option<usize>,
    pending_close_pane: Option<String>,
    remote_label: Entity<TextInput>,
    remote_destination: Entity<TextInput>,
    remote_port: Entity<TextInput>,
    remote_identity_file: Entity<TextInput>,
    remote_herdr_path: Entity<TextInput>,
    remote_search: Entity<TextInput>,
    appearance: AppearanceSettings,
}

impl OcHerdrView {
    fn new(settings: Settings, cx: &mut Context<Self>) -> Self {
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
        let remote_search =
            cx.new(|cx| TextInput::new(cx, "Search hosts").search_field().compact());
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
            managed_profile_index: 0,
            pending_remove_profile: None,
            pending_close_pane: None,
            remote_label: cx.new(|cx| TextInput::new(cx, "Production")),
            remote_destination: cx.new(|cx| TextInput::new(cx, "user@example.com or SSH alias")),
            remote_port: cx.new(|cx| TextInput::new(cx, "22 (optional)")),
            remote_identity_file: cx.new(|cx| TextInput::new(cx, "~/.ssh/id_ed25519 (optional)")),
            remote_herdr_path: cx.new(|cx| TextInput::new(cx, "herdr").with_content("herdr")),
            remote_search,
            appearance: settings.appearance,
        };
        view.reload(None, cx);
        view
    }

    fn current_profile(&self) -> ConnectionProfile {
        self.profiles[self.profile_index].clone()
    }

    fn current_session(&self) -> Option<&SessionSummary> {
        self.session_index
            .and_then(|index| self.sessions.get(index))
    }

    fn reload(&mut self, preferred_session: Option<String>, cx: &mut Context<Self>) {
        self.load_epoch = self.load_epoch.wrapping_add(1);
        self.event_epoch = self.event_epoch.wrapping_add(1);
        self.events = None;
        self.snapshot_refreshing = false;
        let epoch = self.load_epoch;
        let profile = self.current_profile();
        self.error = None;
        self.operation = Some("Discovering Herdr sessions…".into());
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

    fn select_profile(&mut self, index: usize, cx: &mut Context<Self>) {
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

    fn schedule_event_poll(&self, epoch: u64, cx: &mut Context<Self>) {
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

    fn refresh_snapshot_from_event(&mut self, epoch: u64, cx: &mut Context<Self>) {
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

    fn open_add_remote(&mut self, cx: &mut Context<Self>) {
        self.add_remote_open = true;
        self.error = None;
        cx.notify();
    }

    fn open_node_manager(&mut self, cx: &mut Context<Self>) {
        self.node_manager_open = true;
        self.appearance_open = false;
        self.managed_profile_index = self.profile_index;
        self.error = None;
        cx.notify();
    }

    fn close_node_manager(&mut self, cx: &mut Context<Self>) {
        self.node_manager_open = false;
        self.add_remote_open = false;
        self.pending_remove_profile = None;
        cx.notify();
    }

    fn open_appearance(&mut self, cx: &mut Context<Self>) {
        self.appearance_open = true;
        self.node_manager_open = false;
        cx.notify();
    }

    fn close_appearance(&mut self, cx: &mut Context<Self>) {
        self.appearance_open = false;
        cx.notify();
    }

    fn select_managed_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.profiles.len() {
            self.managed_profile_index = index;
            self.add_remote_open = false;
            cx.notify();
        }
    }

    fn ensure_managed_profile_visible(&mut self, cx: &mut Context<Self>) {
        let query = self.remote_search.read(cx).content().trim().to_lowercase();
        if self
            .profiles
            .get(self.managed_profile_index)
            .is_some_and(|profile| profile_matches_search(profile, &query))
        {
            cx.notify();
            return;
        }
        if let Some(index) = self
            .profiles
            .iter()
            .position(|profile| profile_matches_search(profile, &query))
        {
            self.managed_profile_index = index;
        }
        cx.notify();
    }

    fn choose_node(&mut self, index: usize, cx: &mut Context<Self>) {
        self.node_manager_open = false;
        if index == self.profile_index {
            cx.notify();
        } else {
            self.select_profile(index, cx);
        }
    }

    fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.appearance.background_opacity = self.appearance.background_opacity.clamp(40, 100);
        self.appearance.theme_family = install_appearance(&self.appearance, window.appearance());
        theme::apply_window_background(window);
        if let Err(error) = save_settings(&self.profiles, &self.appearance) {
            self.error = Some(error.into());
        }
        cx.refresh_windows();
        cx.notify();
    }

    fn set_theme_family(&mut self, family_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.appearance.theme_family = family_id;
        self.apply_appearance(window, cx);
    }

    fn set_appearance_mode(
        &mut self,
        mode: AppearanceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.mode = mode;
        self.apply_appearance(window, cx);
    }

    fn set_backdrop_mode(
        &mut self,
        backdrop: BackdropMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.backdrop = backdrop;
        self.apply_appearance(window, cx);
    }

    fn set_background_opacity(&mut self, opacity: u8, window: &mut Window, cx: &mut Context<Self>) {
        self.appearance.background_opacity = opacity;
        self.apply_appearance(window, cx);
    }

    fn request_remove_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if self
            .profiles
            .get(index)
            .is_some_and(|profile| profile.id().starts_with("manual-"))
        {
            self.pending_remove_profile = Some(index);
            cx.notify();
        }
    }

    fn cancel_remove_node(&mut self, cx: &mut Context<Self>) {
        self.pending_remove_profile = None;
        cx.notify();
    }

    fn confirm_remove_node(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.pending_remove_profile.take() else {
            return;
        };
        if index == 0 || index >= self.profiles.len() {
            return;
        }
        let removed = self.profiles.remove(index);
        if let Err(error) = save_settings(&self.profiles, &self.appearance) {
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

    fn close_add_remote(&mut self, cx: &mut Context<Self>) {
        self.add_remote_open = false;
        cx.notify();
    }

    fn request_close_pane(&mut self, pane_id: String, cx: &mut Context<Self>) {
        self.pending_close_pane = Some(pane_id);
        cx.notify();
    }

    fn cancel_close_pane(&mut self, cx: &mut Context<Self>) {
        self.pending_close_pane = None;
        cx.notify();
    }

    fn confirm_close_pane(&mut self, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.pending_close_pane.take() {
            self.invoke("pane.close", json!({ "pane_id": pane_id }), cx);
        }
    }

    fn save_remote(&mut self, cx: &mut Context<Self>) {
        let destination = self.remote_destination.read(cx).content().trim().to_owned();
        if destination.is_empty() {
            self.error = Some("SSH destination is required.".into());
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
                    self.error = Some("SSH port must be a number from 1 to 65535.".into());
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
        if let Err(error) = save_settings(&self.profiles, &self.appearance) {
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

    fn select_session(&mut self, index: usize, cx: &mut Context<Self>) {
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

    fn select_workspace(&mut self, workspace_id: String, cx: &mut Context<Self>) {
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

    fn select_tab(&mut self, tab_id: String, cx: &mut Context<Self>) {
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

    fn select_pane(&mut self, pane_id: String, window: &mut Window, cx: &mut Context<Self>) {
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

    fn start_visible_terminals(&mut self, cx: &mut Context<Self>) {
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
                        text: "Waiting for terminal frame…".into(),
                        mode,
                        size: (cols, rows),
                    },
                );
            }
        }
        self.schedule_terminal_poll(epoch, cx);
    }

    fn schedule_terminal_poll(&self, epoch: u64, cx: &mut Context<Self>) {
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

    fn resize_visible_terminals(&mut self, window: &Window) {
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

    fn send_key(&mut self, event: &KeyDownEvent, cx: &mut App) {
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

    fn invoke(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        let Some(connection) = &self.connection else {
            return;
        };
        let socket = connection.socket_path().to_owned();
        self.operation = Some(format!("Running {method}…").into());
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

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let session_rows = self
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let selected = self.session_index == Some(index);
                let running = session.running;
                div()
                    .id(("session", index))
                    .role(ochub_ui::gpui::Role::Button)
                    .aria_label(session.display_name().to_owned())
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(32.))
                    .px_3()
                    .rounded(px(CORNER_COMPACT))
                    .bg(if selected {
                        theme::sidebar_selected()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _window, cx| this.select_session(index, cx)),
                    )
                    .child(status_dot(if running {
                        theme::green()
                    } else {
                        theme::muted()
                    }))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .child(session.display_name().to_owned()),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let mut hierarchy = Vec::new();
        let mut agent_rows = Vec::new();
        let mut seen_agents = HashSet::new();
        if let Some(snapshot) = &self.snapshot {
            for workspace in &snapshot.workspaces {
                let workspace_id = workspace.workspace_id.clone();
                let selected = self.selection.workspace_id.as_deref() == Some(&workspace_id);
                hierarchy.push(
                    tree_row(
                        ("workspace", workspace.number),
                        &workspace.label,
                        12.,
                        IconName::Folder,
                        selected,
                        status_color(workspace.agent_status),
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.select_workspace(workspace_id.clone(), cx)
                    }))
                    .into_any_element(),
                );
            }
            for pane in &snapshot.panes {
                let Some(agent_name) = pane.display_agent.as_deref().or(pane.agent.as_deref())
                else {
                    continue;
                };
                if !seen_agents.insert(agent_name.to_owned()) {
                    continue;
                }
                let pane_id = pane.pane_id.clone();
                let status = pane.agent_status;
                agent_rows.push(
                    div()
                        .id(ochub_ui::gpui::ElementId::Name(
                            format!("agent-{pane_id}").into(),
                        ))
                        .role(ochub_ui::gpui::Role::Button)
                        .aria_label(agent_name.to_owned())
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(30.))
                        .px_3()
                        .rounded(px(CORNER_COMPACT))
                        .hover(|style| style.bg(theme::surface_hover()))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_pane(pane_id.clone(), window, cx)
                        }))
                        .child(status_dot(status_color(status)))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_sm()
                                .child(agent_name.to_owned()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(status.label()),
                        )
                        .into_any_element(),
                );
            }
        }

        div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme::sidebar_background())
            .text_color(theme::sidebar_text())
            .border_r_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(HEADER_HEIGHT))
                    .pl(px(78.))
                    .pr_4()
                    .gap_2()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Spaces"),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_only_button_tone(
                            "new-workspace",
                            "New workspace",
                            IconName::Add,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.invoke("workspace.create", json!({ "focus": true, "env": {} }), cx)
                        })),
                    ),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .px_2()
                    .pb_3()
                    .child(section_label("SESSIONS"))
                    .children(session_rows)
                    .child(section_label("WORKSPACES"))
                    .children(hierarchy),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .max_h(px(220.))
                    .px_2()
                    .pb_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_2()
                            .pt_3()
                            .pb_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::muted())
                            .child("AGENTS")
                            .child("STATUS"),
                    )
                    .child(
                        div()
                            .id("agent-scroll")
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .overflow_scroll()
                            .children(agent_rows),
                    ),
            )
    }

    fn render_tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = Vec::new();
        if let (Some(snapshot), Some(workspace_id)) =
            (&self.snapshot, self.selection.workspace_id.as_deref())
        {
            for tab in snapshot.tabs_for(workspace_id) {
                let tab_id = tab.tab_id.clone();
                let selected = self.selection.tab_id.as_deref() == Some(&tab_id);
                tabs.push(
                    div()
                        .id(("main-tab", tab.number))
                        .role(ochub_ui::gpui::Role::Button)
                        .flex()
                        .items_center()
                        .h_full()
                        .min_w(px(108.))
                        .max_w(px(180.))
                        .px_3()
                        .gap_2()
                        .border_b_2()
                        .border_color(if selected {
                            theme::accent()
                        } else {
                            theme::surface().alpha(0.)
                        })
                        .bg(if selected {
                            theme::selection()
                        } else {
                            theme::surface().alpha(0.)
                        })
                        .text_sm()
                        .text_color(if selected {
                            theme::text()
                        } else {
                            theme::muted()
                        })
                        .hover(|style| style.bg(theme::surface_hover()))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_tab(tab_id.clone(), cx)
                        }))
                        .child(icon(IconName::Terminal, theme::muted(), 13.))
                        .child(div().truncate().child(tab.label.clone()))
                        .into_any_element(),
                );
            }
        }
        let workspace_id = self.selection.workspace_id.clone();
        let pane_id_right = self.selection.pane_id.clone();
        let pane_id_down = self.selection.pane_id.clone();
        let pane_id_zoom = self.selection.pane_id.clone();
        let pane_id_close = self.selection.pane_id.clone();
        let node_manager_open = self.node_manager_open;
        div()
            .flex()
            .items_center()
            .h(px(HEADER_HEIGHT))
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h_full()
                    .min_w_0()
                    .overflow_hidden()
                    .children(tabs),
            )
            .child(
                icon_only_button_tone(
                    "new-tab",
                    "New tab",
                    IconName::Add,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(workspace_id) = workspace_id.clone() {
                        this.invoke(
                            "tab.create",
                            json!({ "workspace_id": workspace_id, "focus": true, "env": {} }),
                            cx,
                        )
                    }
                })),
            )
            .child(div().flex_1())
            .child(div().flex().items_center().gap_1().px_2()
            .child(
                icon_only_button_tone(
                    "split-right",
                    "Split pane right",
                    IconName::Blocks,
                    ButtonTone::Primary,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_right.clone() {
                        this.invoke(
                            "pane.split",
                            json!({ "target_pane_id": pane_id, "direction": SplitDirection::Right, "focus": true, "right_click": "herdr", "env": {} }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                icon_only_button_tone(
                    "split-down",
                    "Split pane down",
                    IconName::ChevronDown,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_down.clone() {
                        this.invoke(
                            "pane.split",
                            json!({ "target_pane_id": pane_id, "direction": SplitDirection::Down, "focus": true, "right_click": "herdr", "env": {} }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                icon_only_button_tone(
                    "zoom-pane",
                    "Zoom pane",
                    IconName::Eye,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_zoom.clone() {
                        this.invoke(
                            "pane.zoom",
                            json!({ "pane_id": pane_id, "mode": "toggle" }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                icon_only_button_tone(
                    "close-pane",
                    "Close pane",
                    IconName::Close,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_close.clone() {
                        this.request_close_pane(pane_id, cx)
                    }
                })),
            )
            )
            .child(div().h(px(22.)).w(px(1.)).bg(theme::border()))
            .child(
                icon_only_button_tone(
                    "open-appearance",
                    "Appearance",
                    IconName::Palette,
                    if self.appearance_open {
                        ButtonTone::Primary
                    } else {
                        ButtonTone::Ghost
                    },
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.open_appearance(cx))),
            )
            .child(icon_button_tone(
                "manage-nodes",
                "Remote",
                IconName::Settings,
                if node_manager_open { ButtonTone::Primary } else { ButtonTone::Neutral },
                ButtonSize::Sm,
            ).mr_3().on_click(cx.listener(|this, _, _window, cx| this.open_node_manager(cx))))
    }

    fn render_status_bar(&self) -> impl IntoElement {
        let profile = self.current_profile();
        let profile_icon = if matches!(profile, ConnectionProfile::Local { .. }) {
            IconName::Desktop
        } else {
            IconName::Cloud
        };
        let profile_label = profile.label().to_owned();
        let status = if let Some(operation) = &self.operation {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(spinner(theme::muted(), 11.))
                .child(operation.clone())
                .into_any_element()
        } else if self.error.is_some() {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(theme::red()))
                .child("Connection unavailable")
                .into_any_element()
        } else if let Some(snapshot) = &self.snapshot {
            let subscription = if self.events.is_some() {
                "subscription active"
            } else {
                "snapshot"
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(theme::green()))
                .child(format!(
                    "Herdr {} · protocol {} · connected · {} · {} workspace{}",
                    snapshot.version,
                    snapshot.protocol,
                    subscription,
                    snapshot.workspaces.len(),
                    if snapshot.workspaces.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                ))
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(theme::muted()))
                .child("No Herdr session")
                .into_any_element()
        };
        div()
            .flex()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .text_xs()
            .text_color(theme::muted())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(px(SIDEBAR_WIDTH))
                    .h_full()
                    .px_3()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(icon(profile_icon, theme::muted(), 13.))
                    .child(div().truncate().child(profile_label)),
            )
            .child(div().flex().items_center().min_w_0().px_3().child(status))
    }

    fn render_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        self.resize_visible_terminals(window);
        let Some(snapshot) = self.snapshot.clone() else {
            let cta = button(
                "retry-empty",
                "Refresh",
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, _window, cx| this.reload(None, cx)))
            .into_any_element();
            return div()
                .flex()
                .items_center()
                .justify_center()
                .flex_1()
                .bg(theme::content_background())
                .child(empty_state(
                    IconName::Terminal,
                    "No running Herdr session",
                    "Start Herdr locally or open Remote in the top-right.",
                    Some(cta),
                ))
                .into_any_element();
        };
        let Some(tab_id) = self.selection.tab_id.as_deref() else {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(empty_state(
                    IconName::Layers,
                    "This session has no tabs",
                    "Create a workspace to open the first terminal.",
                    None,
                ))
                .into_any_element();
        };
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - SIDEBAR_WIDTH).max(320.);
        let height = (f32::from(viewport.height) - HEADER_HEIGHT - STATUS_BAR_HEIGHT).max(180.);
        let layout = snapshot.layout_for(tab_id).cloned();
        let panes = snapshot.panes_for(tab_id).cloned().collect::<Vec<_>>();
        let mut elements = Vec::new();
        for pane in panes {
            let geometry = layout
                .as_ref()
                .and_then(|layout| {
                    layout
                        .panes
                        .iter()
                        .find(|item| item.pane_id == pane.pane_id)
                        .map(|item| {
                            let area = layout.area;
                            let left = (item.rect.x.saturating_sub(area.x)) as f32
                                / area.width.max(1) as f32
                                * width;
                            let top = (item.rect.y.saturating_sub(area.y)) as f32
                                / area.height.max(1) as f32
                                * height;
                            let pane_width =
                                item.rect.width as f32 / area.width.max(1) as f32 * width;
                            let pane_height =
                                item.rect.height as f32 / area.height.max(1) as f32 * height;
                            (left, top, pane_width, pane_height)
                        })
                })
                .unwrap_or((0., 0., width, height));
            let selected = self.selection.pane_id.as_deref() == Some(&pane.pane_id);
            let pane_id = pane.pane_id.clone();
            let text = self
                .panes
                .get(&pane.pane_id)
                .map(|runtime| runtime.text.clone())
                .unwrap_or_else(|| "Connecting…".into());
            elements.push(
                render_pane(pane, text, geometry, selected)
                    .on_click(cx.listener(
                        move |this, _event: &ochub_ui::gpui::ClickEvent, window, cx| {
                            this.select_pane(pane_id.clone(), window, cx);
                        },
                    ))
                    .into_any_element(),
            );
        }
        div()
            .id("terminal-surface")
            .relative()
            .focusable()
            .tab_stop(true)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _window, cx| this.send_key(event, cx)))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(theme::content_background())
            .children(elements)
            .into_any_element()
    }
}

impl Render for OcHerdrView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut main = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme::content_background())
            .child(self.render_tab_bar(cx))
            .child(self.render_terminal(window, cx));
        if let Some(error) = &self.error {
            main = main.child(
                div()
                    .absolute()
                    .right_4()
                    .bottom_4()
                    .max_w(px(480.))
                    .px_3()
                    .py_2()
                    .rounded(px(CORNER_CONTROL))
                    .border_1()
                    .border_color(theme::red())
                    .bg(theme::error_surface())
                    .text_xs()
                    .text_color(theme::red())
                    .child(error.clone()),
            );
        }
        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(self.render_sidebar(cx))
            .child(main);
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(theme::window_base_background())
            .child(body)
            .child(self.render_status_bar());
        if self.node_manager_open {
            root = root.child(self.render_node_manager(cx));
        }
        if self.appearance_open {
            root = root.child(self.render_appearance(cx));
        }
        if self.pending_remove_profile.is_some() {
            root = root.child(self.render_remove_node(cx));
        } else if self.pending_close_pane.is_some() {
            root = root.child(self.render_close_pane(cx));
        }
        root
    }
}

impl OcHerdrView {
    fn render_node_manager(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.remote_search.read(cx).content().trim().to_lowercase();
        let mut rows = Vec::new();
        for source in [
            ConnectionSource::Current,
            ConnectionSource::Saved,
            ConnectionSource::SshConfig,
        ] {
            let matches = self
                .profiles
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, profile)| {
                    connection_source(profile) == source && profile_matches_search(profile, &query)
                })
                .collect::<Vec<_>>();
            if matches.is_empty() {
                continue;
            }
            rows.push(remote_group_label(source.label(), matches.len()).into_any_element());
            for (index, profile) in matches {
                let selected = index == self.managed_profile_index;
                let active = index == self.profile_index;
                let node_icon = if matches!(profile, ConnectionProfile::Local { .. }) {
                    IconName::Desktop
                } else {
                    IconName::Globe
                };
                rows.push(
                    div()
                        .id(("managed-node", index))
                        .role(ochub_ui::gpui::Role::Button)
                        .aria_label(format!(
                            "{} · {}",
                            profile.label(),
                            profile_endpoint(&profile)
                        ))
                        .flex()
                        .items_center()
                        .gap_3()
                        .min_h(px(54.))
                        .mx_2()
                        .px_3()
                        .py_2()
                        .rounded(px(CORNER_CONTROL))
                        .bg(if selected {
                            theme::selection()
                        } else {
                            theme::surface().alpha(0.)
                        })
                        .hover(|style| style.bg(theme::surface_hover()))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_managed_profile(index, cx)
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(30.))
                                .rounded(px(CORNER_CONTROL))
                                .bg(if selected {
                                    theme::accent_soft()
                                } else {
                                    theme::inset()
                                })
                                .child(icon(
                                    node_icon,
                                    if selected {
                                        theme::accent()
                                    } else {
                                        theme::muted()
                                    },
                                    15.,
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme::text())
                                        .child(profile.label().to_owned()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme::muted())
                                        .child(profile_endpoint(&profile)),
                                ),
                        )
                        .when(active, |row| {
                            row.child(status_dot(if self.error.is_some() {
                                theme::red()
                            } else if self.operation.is_some() {
                                theme::yellow()
                            } else {
                                theme::green()
                            }))
                        })
                        .into_any_element(),
                );
            }
        }
        if rows.is_empty() {
            rows.push(
                empty_state(
                    IconName::Search,
                    "No matching hosts",
                    "Try a host name, SSH alias, or address.",
                    None,
                )
                .into_any_element(),
            );
        }
        let detail = if self.add_remote_open {
            self.render_add_remote(cx).into_any_element()
        } else {
            self.render_remote_detail(cx).into_any_element()
        };
        let card = modal_card()
            .w(px(840.))
            .h(px(580.))
            .rounded(px(CORNER_MODAL))
            .child(
                modal_header("Remote connections").child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            icon_button_tone(
                                "add-managed-node",
                                "New SSH",
                                IconName::Add,
                                if self.add_remote_open {
                                    ButtonTone::Primary
                                } else {
                                    ButtonTone::Neutral
                                },
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(|this, _, _window, cx| this.open_add_remote(cx))),
                        )
                        .child(
                            icon_only_button_tone(
                                "close-node-manager",
                                "Close",
                                IconName::Close,
                                ButtonTone::Ghost,
                                ButtonSize::Sm,
                            )
                            .on_click(
                                cx.listener(|this, _, _window, cx| this.close_node_manager(cx)),
                            ),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(310.))
                            .flex_none()
                            .min_h_0()
                            .border_r_1()
                            .border_color(theme::border())
                            .bg(theme::sidebar_background())
                            .child(div().p_3().child(self.remote_search.clone()))
                            .child(
                                div()
                                    .id("managed-node-scroll")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_scroll()
                                    .pb_3()
                                    .children(rows),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .bg(theme::content_background())
                            .child(detail),
                    ),
            );
        modal_overlay(card).top_0().left_0()
    }

    fn render_remote_detail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(profile) = self.profiles.get(self.managed_profile_index).cloned() else {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .child("Select a connection");
        };
        let index = self.managed_profile_index;
        let active = index == self.profile_index;
        let saved = connection_source(&profile) == ConnectionSource::Saved;
        let source = connection_source(&profile).description();
        let (identity, herdr_path) = match &profile {
            ConnectionProfile::Local { herdr_path } => {
                ("System default".to_owned(), herdr_path.clone())
            }
            ConnectionProfile::Ssh {
                identity_file,
                herdr_path,
                ..
            } => (
                identity_file
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "SSH config or agent".into()),
                herdr_path.clone(),
            ),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .px_6()
                    .pt_6()
                    .pb_5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(44.))
                            .rounded(px(CORNER_PANEL))
                            .bg(theme::accent_soft())
                            .child(icon(
                                if matches!(profile, ConnectionProfile::Local { .. }) {
                                    IconName::Desktop
                                } else {
                                    IconName::Globe
                                },
                                theme::accent(),
                                20.,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(profile.label().to_owned()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .text_color(theme::muted())
                                    .child(profile_endpoint(&profile)),
                            ),
                    )
                    .when(active, |header| {
                        header.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded(px(CORNER_COMPACT))
                                .bg(theme::green_soft())
                                .text_xs()
                                .text_color(theme::green())
                                .child(status_dot(theme::green()))
                                .child("Connected"),
                        )
                    }),
            )
            .child(
                div()
                    .mx_6()
                    .rounded(px(CORNER_PANEL))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .child(remote_detail_row("Source", source.to_owned(), true))
                    .child(remote_detail_row("Identity", identity, true))
                    .child(remote_detail_row("Herdr command", herdr_path, false)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_6()
                    .py_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(theme::muted())
                            .child("Uses OpenSSH config, keys, agent, and known_hosts."),
                    )
                    .when(saved, |footer| {
                        footer.child(
                            icon_only_button_tone(
                                "remove-managed-node-detail",
                                "Remove saved host",
                                IconName::Trash,
                                ButtonTone::Ghost,
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(
                                move |this, _, _window, cx| this.request_remove_node(index, cx),
                            )),
                        )
                    })
                    .child(
                        button(
                            "connect-managed-node",
                            if active { "Reconnect" } else { "Connect" },
                            ButtonTone::Primary,
                            ButtonSize::Md,
                        )
                        .on_click(
                            cx.listener(move |this, _, _window, cx| this.choose_node(index, cx)),
                        ),
                    ),
            )
    }

    fn render_add_remote(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let cancel = button(
            "cancel-add-remote",
            "Cancel",
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.close_add_remote(cx)))
        .into_any_element();
        let connect = button(
            "save-add-remote",
            "Save & connect",
            ButtonTone::Primary,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.save_remote(cx)))
        .into_any_element();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_6()
                    .py_5()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(36.))
                            .rounded(px(CORNER_CONTROL))
                            .bg(theme::accent_soft())
                            .child(icon(IconName::Globe, theme::accent(), 17.)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("New SSH connection"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child("Save a host, then discover its Herdr sessions."),
                            ),
                    ),
            )
            .child(
                div()
                    .id("new-ssh-form-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .gap_3()
                    .px_6()
                    .py_5()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child(field(
                                "Label",
                                false,
                                Some("Name shown in OcHerdr.".into()),
                                self.remote_label.clone(),
                            )))
                            .child(div().w(px(132.)).flex_none().child(field(
                                "Port",
                                false,
                                Some("Uses SSH config when empty.".into()),
                                self.remote_port.clone(),
                            ))),
                    )
                    .child(field(
                        "Destination",
                        true,
                        Some("SSH alias or user@host from ~/.ssh/config.".into()),
                        self.remote_destination.clone(),
                    ))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child(field(
                                "Identity file",
                                false,
                                Some("Optional key path; SSH agent still works.".into()),
                                self.remote_identity_file.clone(),
                            )))
                            .child(div().w(px(150.)).flex_none().child(field(
                                "Herdr command",
                                false,
                                Some("Remote command or path.".into()),
                                self.remote_herdr_path.clone(),
                            ))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .px_6()
                    .py_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(cancel)
                    .child(connect),
            )
    }

    fn render_appearance(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut family_rows = Vec::new();
        for record in &theme::load_registry().themes {
            let family = record.family.clone();
            let family_id = family.id.clone();
            let selected = family_id == self.appearance.theme_family;
            let palette = if theme::is_dark() {
                family.dark
            } else {
                family.light
            };
            family_rows.push(
                div()
                    .id(ochub_ui::gpui::ElementId::Name(
                        format!("appearance-family-{family_id}").into(),
                    ))
                    .role(ochub_ui::gpui::Role::Button)
                    .aria_label(format!("Use {} theme", family.name))
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_h(px(52.))
                    .px_3()
                    .py_2()
                    .rounded(px(CORNER_CONTROL))
                    .border_1()
                    .border_color(if selected {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(if selected {
                        theme::selection()
                    } else {
                        theme::surface()
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme_family(family_id.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .p_1()
                            .rounded(px(CORNER_CONTROL))
                            .bg(palette.bg.rgba())
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(CORNER_COMPACT))
                                    .bg(palette.accent_fill.rgba()),
                            )
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(CORNER_COMPACT))
                                    .bg(palette.surface.rgba()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .gap(px(2.))
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(family.name),
                            )
                            .child(div().truncate().text_xs().text_color(theme::muted()).child(
                                if family.description.is_empty() {
                                    "Light and dark variants".to_owned()
                                } else {
                                    family.description
                                },
                            )),
                    )
                    .when(selected, |row| row.child(status_dot(theme::accent())))
                    .into_any_element(),
            );
        }
        let mode_listener = cx.listener(|this, index: &usize, window, cx| {
            let mode = match *index {
                1 => AppearanceMode::Light,
                2 => AppearanceMode::Dark,
                _ => AppearanceMode::System,
            };
            this.set_appearance_mode(mode, window, cx);
        });
        let mode = segmented(
            "appearance-mode-control",
            &["System", "Light", "Dark"],
            self.appearance.mode.index(),
            move |index, window, cx| mode_listener(&index, window, cx),
        );
        let backdrop_listener = cx.listener(|this, index: &usize, window, cx| {
            let backdrop = match *index {
                1 => BackdropMode::Transparent,
                2 => BackdropMode::Blurred,
                _ => BackdropMode::Opaque,
            };
            this.set_backdrop_mode(backdrop, window, cx);
        });
        let backdrop = segmented(
            "appearance-backdrop-control",
            &["Opaque", "Clear", "Blur"],
            self.appearance.backdrop.index(),
            move |index, window, cx| backdrop_listener(&index, window, cx),
        );
        let opacity_values = [100_u8, 92, 84, 72];
        let opacity_index = opacity_values
            .iter()
            .position(|value| *value == self.appearance.background_opacity)
            .unwrap_or(1);
        let opacity_listener = cx.listener(|this, index: &usize, window, cx| {
            let opacity = [100_u8, 92, 84, 72].get(*index).copied().unwrap_or(92);
            this.set_background_opacity(opacity, window, cx);
        });
        let opacity = segmented(
            "appearance-opacity-control",
            &["100%", "92%", "84%", "72%"],
            opacity_index,
            move |index, window, cx| opacity_listener(&index, window, cx),
        );
        let done = button(
            "close-appearance-footer",
            "Done",
            ButtonTone::Primary,
            ButtonSize::Md,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.close_appearance(cx)))
        .into_any_element();
        let card = modal_card()
            .w(px(680.))
            .h(px(560.))
            .rounded(px(CORNER_MODAL))
            .child(
                modal_header("Appearance").child(
                    icon_only_button_tone(
                        "close-appearance",
                        "Close",
                        IconName::Close,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| this.close_appearance(cx))),
                ),
            )
            .child(
                div()
                    .id("appearance-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .gap_5()
                    .px_5()
                    .py_5()
                    .child(appearance_section(
                        "Theme",
                        "Choose a color family. Each family includes light and dark variants.",
                        div().grid().grid_cols(2).gap_2().children(family_rows),
                    ))
                    .child(appearance_setting_row(
                        "Appearance",
                        "Follow macOS or pin a variant.",
                        mode,
                    ))
                    .child(appearance_setting_row(
                        "Window background",
                        "Clear keeps true transparency; Blur uses the native macOS backdrop.",
                        backdrop,
                    ))
                    .child(appearance_setting_row(
                        "Background opacity",
                        "Applied to terminal and shell surfaces when transparency is enabled.",
                        opacity,
                    )),
            )
            .child(modal_footer(vec![done]));
        modal_overlay(card).top_0().left_0()
    }

    fn render_remove_node(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let node_name = self
            .pending_remove_profile
            .and_then(|index| self.profiles.get(index))
            .map(ConnectionProfile::label)
            .unwrap_or("this node")
            .to_owned();
        let cancel = button(
            "cancel-remove-node",
            "Cancel",
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_remove_node(cx)))
        .into_any_element();
        let remove = button(
            "confirm-remove-node",
            "Remove node",
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_remove_node(cx)))
        .into_any_element();
        modal_overlay(
            modal_card()
                .child(modal_header("Remove SSH node?"))
                .child(
                    modal_body()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text())
                                .child(format!("Remove {node_name} from OcHerdr?")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(
                                    "This only removes the saved node profile. SSH keys and ~/.ssh/config are not changed.",
                                ),
                        ),
                )
                .child(modal_footer(vec![cancel, remove])),
        )
        .top_0()
        .left_0()
    }

    fn render_close_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let cancel = button(
            "cancel-close-pane",
            "Cancel",
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.cancel_close_pane(cx)))
        .into_any_element();
        let close = button(
            "confirm-close-pane",
            "Close pane",
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.confirm_close_pane(cx)))
        .into_any_element();
        modal_overlay(
            modal_card()
                .child(modal_header("Close terminal pane?"))
                .child(
                    modal_body()
                        .child(
                            div().text_sm().text_color(theme::text()).child(
                                "The process running in this Herdr pane will be terminated.",
                            ),
                        )
                        .child(div().text_xs().text_color(theme::muted()).child(
                            "If this is the final pane, Herdr may also close its tab or workspace.",
                        )),
                )
                .child(modal_footer(vec![cancel, close])),
        )
        .top_0()
        .left_0()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectionSource {
    Current,
    Saved,
    SshConfig,
}

impl ConnectionSource {
    fn label(self) -> &'static str {
        match self {
            Self::Current => "CURRENT",
            Self::Saved => "SAVED",
            Self::SshConfig => "SSH CONFIG",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Current => "This Mac",
            Self::Saved => "Saved in OcHerdr",
            Self::SshConfig => "Imported from ~/.ssh/config",
        }
    }
}

fn connection_source(profile: &ConnectionProfile) -> ConnectionSource {
    if matches!(profile, ConnectionProfile::Local { .. }) {
        ConnectionSource::Current
    } else if profile.id().starts_with("manual-") {
        ConnectionSource::Saved
    } else {
        ConnectionSource::SshConfig
    }
}

fn profile_endpoint(profile: &ConnectionProfile) -> String {
    match profile {
        ConnectionProfile::Local { .. } => "localhost".into(),
        ConnectionProfile::Ssh {
            destination, port, ..
        } => port.map_or_else(
            || destination.clone(),
            |port| format!("{destination}:{port}"),
        ),
    }
}

fn profile_matches_search(profile: &ConnectionProfile, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    profile.label().to_lowercase().contains(query)
        || profile_endpoint(profile).to_lowercase().contains(query)
        || connection_source(profile)
            .label()
            .to_lowercase()
            .contains(query)
        || connection_source(profile)
            .description()
            .to_lowercase()
            .contains(query)
}

fn remote_group_label(label: &'static str, count: usize) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_4()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::muted())
        .child(label)
        .child(count.to_string())
}

fn remote_detail_row(label: &'static str, value: String, separated: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .min_h(px(46.))
        .px_4()
        .when(separated, |row| {
            row.border_b_1().border_color(theme::border())
        })
        .child(
            div()
                .w(px(118.))
                .flex_none()
                .text_xs()
                .text_color(theme::muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(theme::text())
                .child(value),
        )
}

fn appearance_section(
    title: &'static str,
    hint: &'static str,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text())
                        .child(title),
                )
                .child(div().text_xs().text_color(theme::muted()).child(hint)),
        )
        .child(content)
}

fn appearance_setting_row(
    label: &'static str,
    hint: &'static str,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_5()
        .min_h(px(66.))
        .px_4()
        .py_3()
        .rounded(px(CORNER_PANEL))
        .border_1()
        .border_color(theme::border())
        .bg(theme::surface())
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(label),
                )
                .child(div().text_xs().text_color(theme::muted()).child(hint)),
        )
        .child(div().w(px(250.)).flex_none().child(control))
}

fn install_appearance(
    appearance: &AppearanceSettings,
    window_appearance: WindowAppearance,
) -> String {
    let mut family =
        theme::find_family(&appearance.theme_family).unwrap_or_else(theme::ochub_family);
    let effect = appearance.backdrop.theme_effect();
    let content_opacity = if appearance.backdrop == BackdropMode::Opaque {
        100
    } else {
        appearance.background_opacity.clamp(40, 100)
    };
    let sidebar_opacity = if appearance.backdrop == BackdropMode::Opaque {
        100
    } else {
        content_opacity.saturating_add(6).min(100)
    };
    for palette in [&mut family.light, &mut family.dark] {
        palette.effects.window_background = effect;
        palette.effects.content_opacity = content_opacity;
        palette.effects.sidebar_opacity = sidebar_opacity;
    }
    let installed_id = family.id.clone();
    theme::install_family(&family, appearance.mode.theme_mode(), window_appearance);
    installed_id
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("OcHerdr/connections.json"))
}

fn load_settings() -> Settings {
    settings_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_settings(
    profiles: &[ConnectionProfile],
    appearance: &AppearanceSettings,
) -> std::result::Result<(), String> {
    let connections = profiles
        .iter()
        .filter(|profile| profile.id().starts_with("manual-"))
        .cloned()
        .collect();
    let settings = Settings {
        connections,
        appearance: appearance.clone(),
    };
    let path =
        settings_path().ok_or_else(|| "Application Support directory is unavailable".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Settings path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn section_label(label: &'static str) -> impl IntoElement {
    div()
        .px_2()
        .pt_4()
        .pb_1()
        .text_color(theme::muted())
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

fn tree_row(
    id: impl Into<ochub_ui::gpui::ElementId>,
    label: &str,
    indent: f32,
    icon_name: IconName,
    selected: bool,
    color: ochub_ui::gpui::Rgba,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    div()
        .id(id)
        .role(ochub_ui::gpui::Role::Button)
        .aria_label(label.to_owned())
        .flex()
        .items_center()
        .gap_2()
        .h(px(30.))
        .pl(px(indent))
        .pr_2()
        .rounded(px(CORNER_COMPACT))
        .bg(if selected {
            theme::sidebar_selected()
        } else {
            theme::surface().alpha(0.)
        })
        .hover(|style| style.bg(theme::surface_hover()))
        .cursor_pointer()
        .child(icon(
            icon_name,
            if selected {
                theme::accent()
            } else {
                theme::muted()
            },
            13.,
        ))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_xs()
                .text_color(theme::sidebar_text())
                .child(label.to_owned()),
        )
        .child(status_dot(color))
}

fn render_pane(
    pane: PaneInfo,
    text: SharedString,
    geometry: (f32, f32, f32, f32),
    selected: bool,
) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
    let (left, top, width, height) = geometry;
    let pane_name = pane.display_name().to_owned();
    div()
        .id(ochub_ui::gpui::ElementId::Name(
            format!("terminal-pane-{}", pane.pane_id).into(),
        ))
        .absolute()
        .left(px(left + 2.))
        .top(px(top + 2.))
        .w(px((width - 4.).max(40.)))
        .h(px((height - 4.).max(40.)))
        .overflow_hidden()
        .border_1()
        .border_color(if selected {
            theme::accent()
        } else {
            theme::border_strong()
        })
        .bg(theme::content_background())
        .cursor_text()
        .child(
            div()
                .flex()
                .items_center()
                .h(px(PANE_HEADER_HEIGHT))
                .px_2()
                .gap_2()
                .border_b_1()
                .border_color(theme::border())
                .bg(if selected {
                    theme::selection()
                } else {
                    theme::panel()
                })
                .text_xs()
                .text_color(theme::subtext())
                .child(status_dot(status_color(pane.agent_status)))
                .child(div().truncate().flex_1().child(pane_name)),
        )
        .child(
            div()
                .w_full()
                .h(px((height - PANE_HEADER_HEIGHT - 4.).max(20.)))
                .overflow_hidden()
                .px_2()
                .py_1()
                .font_family("Menlo")
                .text_size(px(12.5))
                .line_height(px(CELL_HEIGHT))
                .text_color(theme::text())
                .child(text),
        )
}

fn status_color(status: AgentStatus) -> ochub_ui::gpui::Rgba {
    match status {
        AgentStatus::Working => theme::teal(),
        AgentStatus::Blocked => theme::yellow(),
        AgentStatus::Done => theme::green(),
        AgentStatus::Idle => theme::muted(),
        AgentStatus::Unknown => theme::border_strong(),
    }
}

fn main() {
    application()
        .with_assets(OcHerdrAssets)
        .run(|cx: &mut App| {
            ochub_ui::install(cx);
            let mut settings = load_settings();
            settings.appearance.theme_family =
                install_appearance(&settings.appearance, cx.window_appearance());
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1260.), px(820.)),
                        cx,
                    ))),
                    window_min_size: Some(size(px(860.), px(560.))),
                    window_background: theme::window_background_appearance(),
                    titlebar: Some(TitlebarOptions {
                        title: Some(SharedString::new_static("OcHerdr")),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(18.), px(18.))),
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    window.set_window_title("OcHerdr");
                    window
                        .observe_window_appearance(|window, cx| {
                            let settings = load_settings();
                            if settings.appearance.mode == AppearanceMode::System {
                                install_appearance(&settings.appearance, window.appearance());
                                theme::apply_window_background(window);
                                cx.refresh_windows();
                            }
                        })
                        .detach();
                    cx.new(|cx| OcHerdrView::new(settings, cx))
                },
            )
            .expect("open OcHerdr window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_connection_settings_receive_the_default_appearance() {
        let settings: Settings = serde_json::from_str(r#"{"connections":[]}"#).unwrap();

        assert_eq!(
            settings.appearance.theme_family,
            theme::DEFAULT_THEME_FAMILY
        );
        assert_eq!(settings.appearance.mode, AppearanceMode::Dark);
        assert_eq!(settings.appearance.backdrop, BackdropMode::Blurred);
        assert_eq!(settings.appearance.background_opacity, 92);
    }

    #[test]
    fn remote_search_matches_labels_endpoints_and_sources() {
        let profile = ConnectionProfile::Ssh {
            id: "ssh-0-build".into(),
            label: "Build box".into(),
            destination: "builder@example.net".into(),
            port: Some(2222),
            identity_file: None,
            herdr_path: "herdr".into(),
        };

        assert!(profile_matches_search(&profile, "build"));
        assert!(profile_matches_search(&profile, "2222"));
        assert!(profile_matches_search(&profile, "ssh config"));
        assert!(!profile_matches_search(&profile, "production"));
    }
}
