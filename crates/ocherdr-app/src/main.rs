use std::borrow::Cow;
use std::collections::HashMap;
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
    BadgeTone, ButtonSize, ButtonTone, badge, button, empty_state, field, icon_button_tone,
    modal_body, modal_card, modal_footer, modal_header, modal_overlay, spinner, status_dot,
};
use ochub_ui::gpui::{
    App, AppContext, AssetSource, Bounds, Context, Entity, FocusHandle, FontWeight, IntoElement,
    KeyDownEvent, Render, SharedString, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    size,
};
use ochub_ui::icons::{IconName, icon};
use ochub_ui::text_input::TextInput;
use ochub_ui::{assets, theme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const SIDEBAR_WIDTH: f32 = 292.;
const HEADER_HEIGHT: f32 = 58.;
const TAB_BAR_HEIGHT: f32 = 38.;
const PANE_HEADER_HEIGHT: f32 = 26.;
const CELL_WIDTH: f32 = 8.4;
const CELL_HEIGHT: f32 = 17.;

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

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    connections: Vec<ConnectionProfile>,
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
    add_remote_open: bool,
    pending_close_pane: Option<String>,
    remote_label: Entity<TextInput>,
    remote_destination: Entity<TextInput>,
    remote_port: Entity<TextInput>,
}

impl OcHerdrView {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut profiles = vec![ConnectionProfile::default()];
        profiles.extend(load_settings().connections);
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
            add_remote_open: false,
            pending_close_pane: None,
            remote_label: cx.new(|cx| TextInput::new(cx, "Production")),
            remote_destination: cx.new(|cx| TextInput::new(cx, "user@example.com or SSH alias")),
            remote_port: cx.new(|cx| TextInput::new(cx, "22 (optional)")),
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
        let profile = ConnectionProfile::Ssh {
            id: format!("manual-{}", self.profiles.len()),
            label: if label.is_empty() {
                destination.clone()
            } else {
                label
            },
            destination,
            port,
            identity_file: None,
            herdr_path: "herdr".into(),
        };
        self.profiles.push(profile);
        if let Err(error) = save_settings(&self.profiles) {
            self.profiles.pop();
            self.error = Some(error.into());
            return;
        }
        self.profile_index = self.profiles.len() - 1;
        self.add_remote_open = false;
        self.remote_label
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_destination
            .update(cx, |input, cx| input.set_content("", cx));
        self.remote_port
            .update(cx, |input, cx| input.set_content("", cx));
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
        if self.selection.pane_id.as_deref() != Some(&pane_id) {
            self.selection.pane_id = Some(pane_id);
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
            (f32::from(viewport.height) - HEADER_HEIGHT - TAB_BAR_HEIGHT).max(180.);
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
        let profile_rows = self
            .profiles
            .iter()
            .enumerate()
            .map(|(index, profile)| {
                let selected = index == self.profile_index;
                let icon_name = if matches!(profile, ConnectionProfile::Local { .. }) {
                    IconName::Desktop
                } else {
                    IconName::Cloud
                };
                div()
                    .id(("profile", index))
                    .role(ochub_ui::gpui::Role::Button)
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(34.))
                    .px_3()
                    .rounded_md()
                    .bg(if selected {
                        theme::sidebar_selected()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _window, cx| this.select_profile(index, cx)),
                    )
                    .child(icon(
                        icon_name,
                        if selected {
                            theme::accent()
                        } else {
                            theme::muted()
                        },
                        14.,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(profile.label().to_owned()),
                    )
                    .child(status_dot(if selected && self.error.is_none() {
                        theme::green()
                    } else {
                        theme::muted()
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

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
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(32.))
                    .pl(px(18.))
                    .pr_3()
                    .rounded_md()
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
                    .child(if running {
                        badge(BadgeTone::Success, "LIVE")
                    } else {
                        badge(BadgeTone::Neutral, "START")
                    })
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let mut hierarchy = Vec::new();
        if let Some(snapshot) = &self.snapshot {
            for workspace in &snapshot.workspaces {
                let workspace_id = workspace.workspace_id.clone();
                let selected = self.selection.workspace_id.as_deref() == Some(&workspace_id);
                hierarchy.push(
                    tree_row(
                        ("workspace", workspace.number),
                        &workspace.label,
                        18.,
                        IconName::Folder,
                        selected,
                        status_color(workspace.agent_status),
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.select_workspace(workspace_id.clone(), cx)
                    }))
                    .into_any_element(),
                );
                for tab in snapshot.tabs_for(&workspace.workspace_id) {
                    let tab_id = tab.tab_id.clone();
                    let selected = self.selection.tab_id.as_deref() == Some(&tab_id);
                    hierarchy.push(
                        tree_row(
                            ("tab", tab.number),
                            &tab.label,
                            34.,
                            IconName::Layers,
                            selected,
                            status_color(tab.agent_status),
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_tab(tab_id.clone(), cx)
                        }))
                        .into_any_element(),
                    );
                    for pane in snapshot.panes_for(&tab.tab_id) {
                        let pane_id = pane.pane_id.clone();
                        let selected = self.selection.pane_id.as_deref() == Some(&pane_id);
                        hierarchy.push(
                            tree_row(
                                ochub_ui::gpui::ElementId::Name(
                                    format!("tree-pane-{}", pane.pane_id).into(),
                                ),
                                pane.display_name(),
                                50.,
                                IconName::Terminal,
                                selected,
                                status_color(pane.agent_status),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_pane(pane_id.clone(), window, cx)
                            }))
                            .into_any_element(),
                        );
                    }
                }
            }
        }

        div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme::sidebar_background())
            .border_r_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(58.))
                    .px_4()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(28.))
                            .h(px(28.))
                            .rounded_md()
                            .bg(theme::text())
                            .child(icon(IconName::Terminal, theme::content_background(), 15.)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .child("OcHerdr"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child("HERDR DESKTOP"),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button_tone(
                            "add-remote",
                            "Add SSH",
                            IconName::Add,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.open_add_remote(cx))),
                    )
                    .child(
                        icon_button_tone(
                            "refresh-connections",
                            "Refresh",
                            IconName::Refresh,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            let preferred =
                                this.current_session().map(|session| session.name.clone());
                            this.reload(preferred, cx);
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
                    .child(section_label("CONNECTIONS"))
                    .children(profile_rows)
                    .child(section_label("SESSIONS"))
                    .children(session_rows)
                    .child(section_label("WORKSPACE TREE"))
                    .children(hierarchy),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(34.))
                    .px_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .text_color(theme::muted())
                    .child(if let Some(operation) = &self.operation {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(spinner(theme::muted(), 12.))
                            .child(operation.clone())
                    } else {
                        div().child("PUBLIC API · GHOSTTY VT")
                    }),
            )
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                self.selection.workspace_id.as_deref().and_then(|id| {
                    snapshot
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.workspace_id == id)
                })
            })
            .map(|workspace| workspace.label.clone())
            .unwrap_or_else(|| "No workspace selected".into());
        let subtitle = self
            .current_session()
            .map(|session| {
                format!(
                    "{}  /  {}",
                    self.current_profile().label(),
                    session.display_name()
                )
            })
            .unwrap_or_else(|| self.current_profile().label().into());
        let workspace_id = self.selection.workspace_id.clone();
        let pane_id_right = self.selection.pane_id.clone();
        let pane_id_down = self.selection.pane_id.clone();
        let pane_id_zoom = self.selection.pane_id.clone();
        let pane_id_close = self.selection.pane_id.clone();
        div()
            .flex().items_center().h(px(HEADER_HEIGHT)).px_4().gap_3().border_b_1().border_color(theme::border())
            .child(div().flex().flex_col().flex_1().min_w_0().child(div().truncate().text_base().font_weight(FontWeight::SEMIBOLD).child(title)).child(div().truncate().text_xs().text_color(theme::muted()).child(subtitle)))
            .child(icon_button_tone("new-workspace", "Workspace", IconName::Add, ButtonTone::Neutral, ButtonSize::Sm).on_click(cx.listener(|this, _, _window, cx| this.invoke("workspace.create", json!({ "focus": true, "env": {} }), cx))))
            .child(icon_button_tone("new-tab", "Tab", IconName::Layers, ButtonTone::Neutral, ButtonSize::Sm).on_click(cx.listener(move |this, _, _window, cx| {
                if let Some(workspace_id) = workspace_id.clone() { this.invoke("tab.create", json!({ "workspace_id": workspace_id, "focus": true, "env": {} }), cx) }
            })))
            .child(icon_button_tone("split-right", "Split", IconName::Blocks, ButtonTone::Primary, ButtonSize::Sm).on_click(cx.listener(move |this, _, _window, cx| {
                if let Some(pane_id) = pane_id_right.clone() { this.invoke("pane.split", json!({ "target_pane_id": pane_id, "direction": SplitDirection::Right, "focus": true, "right_click": "herdr", "env": {} }), cx) }
            })))
            .child(icon_button_tone("split-down", "Down", IconName::Blocks, ButtonTone::Neutral, ButtonSize::Sm).on_click(cx.listener(move |this, _, _window, cx| {
                if let Some(pane_id) = pane_id_down.clone() { this.invoke("pane.split", json!({ "target_pane_id": pane_id, "direction": SplitDirection::Down, "focus": true, "right_click": "herdr", "env": {} }), cx) }
            })))
            .child(icon_button_tone("zoom-pane", "Zoom", IconName::Eye, ButtonTone::Ghost, ButtonSize::Sm).on_click(cx.listener(move |this, _, _window, cx| {
                if let Some(pane_id) = pane_id_zoom.clone() { this.invoke("pane.zoom", json!({ "pane_id": pane_id, "mode": "toggle" }), cx) }
            })))
            .child(icon_button_tone("close-pane", "Close", IconName::Close, ButtonTone::Danger, ButtonSize::Sm).on_click(cx.listener(move |this, _, _window, cx| {
                if let Some(pane_id) = pane_id_close.clone() { this.request_close_pane(pane_id, cx) }
            })))
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
                        .flex()
                        .items_center()
                        .h_full()
                        .px_3()
                        .gap_2()
                        .border_b_2()
                        .border_color(if selected {
                            theme::accent()
                        } else {
                            theme::surface().alpha(0.)
                        })
                        .text_xs()
                        .text_color(if selected {
                            theme::text()
                        } else {
                            theme::muted()
                        })
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .hover(|style| style.bg(theme::surface_hover()))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_tab(tab_id.clone(), cx)
                        }))
                        .child(status_dot(status_color(tab.agent_status)))
                        .child(tab.label.clone())
                        .into_any_element(),
                );
            }
        }
        div()
            .flex()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .children(tabs)
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
                    "Start Herdr locally or choose an SSH host in the sidebar.",
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
        let height = (f32::from(viewport.height) - HEADER_HEIGHT - TAB_BAR_HEIGHT).max(180.);
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
            .bg(theme::c(0x111416))
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
            .child(self.render_header(cx))
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
                    .rounded_md()
                    .border_1()
                    .border_color(theme::red())
                    .bg(theme::error_surface())
                    .text_xs()
                    .text_color(theme::red())
                    .child(error.clone()),
            );
        }
        let mut root = div()
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h_full()
            .bg(theme::window_base_background())
            .child(self.render_sidebar(cx))
            .child(main);
        if self.add_remote_open {
            root = root.child(self.render_add_remote(cx));
        } else if self.pending_close_pane.is_some() {
            root = root.child(self.render_close_pane(cx));
        }
        root
    }
}

impl OcHerdrView {
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
        modal_overlay(
            modal_card()
                .child(modal_header("Add SSH connection"))
                .child(
                    modal_body()
                        .child(field(
                            "Label",
                            false,
                            Some("A short name shown in the connection tree.".into()),
                            self.remote_label.clone(),
                        ))
                        .child(field(
                            "Destination",
                            true,
                            Some(
                                "Uses system OpenSSH and may be an alias from ~/.ssh/config."
                                    .into(),
                            ),
                            self.remote_destination.clone(),
                        ))
                        .child(field(
                            "Port",
                            false,
                            Some("Leave empty to use SSH config or the default port.".into()),
                            self.remote_port.clone(),
                        )),
                )
                .child(modal_footer(vec![cancel, connect])),
        )
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
    }
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

fn save_settings(profiles: &[ConnectionProfile]) -> std::result::Result<(), String> {
    let connections = profiles
        .iter()
        .filter(|profile| profile.id().starts_with("manual-"))
        .cloned()
        .collect();
    let settings = Settings { connections };
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
        .flex()
        .items_center()
        .gap_2()
        .h(px(30.))
        .pl(px(indent))
        .pr_2()
        .rounded_md()
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
    let pane_id = pane.pane_id.clone();
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
            theme::c(0x353a3e)
        })
        .bg(theme::c(0x111416))
        .cursor_text()
        .child(
            div()
                .flex()
                .items_center()
                .h(px(PANE_HEADER_HEIGHT))
                .px_2()
                .gap_2()
                .border_b_1()
                .border_color(theme::c(0x2c3033))
                .bg(if selected {
                    theme::c(0x20292a)
                } else {
                    theme::c(0x191c1e)
                })
                .text_xs()
                .text_color(theme::c(0xc9d1d4))
                .child(status_dot(status_color(pane.agent_status)))
                .child(div().truncate().flex_1().child(pane_name))
                .child(div().text_color(theme::c(0x717b80)).child(pane_id)),
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
                .text_color(theme::c(0xd7dedf))
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
            theme::install_family(
                &theme::ochub_family(),
                theme::ThemeMode::System,
                cx.window_appearance(),
            );
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1260.), px(820.)),
                        cx,
                    ))),
                    window_min_size: Some(size(px(860.), px(560.))),
                    window_background: theme::window_background_appearance(),
                    ..Default::default()
                },
                |_window, cx| cx.new(OcHerdrView::new),
            )
            .expect("open OcHerdr window");
            cx.activate(true);
        });
}
