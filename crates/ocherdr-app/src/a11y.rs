//! Accessibility facts derived from Herdr snapshot + GUI selection.
//!
//! Render applies these properties onto GPUI elements (`id` + `role` required).
//! Tests call the same mapping so chrome/terminal announcements cannot drift.

use ocherdr_core::{HierarchySnapshot, PaneInfo, Selection, SessionSummary};
use ochub_ui::gpui::{Div, Role, Stateful, Toggled, prelude::*};

use super::EventStreamState;
use super::OcHerdrView;
use super::i18n::I18n;
use super::profile_display_label;

#[derive(Clone, Debug, PartialEq)]
pub struct RegionA11y {
    pub id: &'static str,
    pub role: Role,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControlA11y {
    pub id: String,
    pub role: Role,
    pub name: String,
    pub selected: Option<bool>,
    pub toggled: Option<bool>,
    /// Explicit tab-stop, independent of GPUI's Role::Button default.
    pub tab_stop: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListA11y {
    pub id: &'static str,
    pub role: Role,
    pub name: String,
    pub items: Vec<ControlA11y>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolbarA11y {
    pub new_tab: ControlA11y,
    pub split_right: ControlA11y,
    pub split_down: ControlA11y,
    pub zoom: ControlA11y,
    pub close_pane: ControlA11y,
    pub appearance: ControlA11y,
    pub herdr_settings: ControlA11y,
    pub remote: ControlA11y,
}

impl ToolbarA11y {
    #[cfg(test)]
    pub fn actions(&self) -> [&ControlA11y; 8] {
        [
            &self.new_tab,
            &self.split_right,
            &self.split_down,
            &self.zoom,
            &self.close_pane,
            &self.appearance,
            &self.herdr_settings,
            &self.remote,
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChromeA11y {
    pub sidebar: RegionA11y,
    pub main: RegionA11y,
    pub status: RegionA11y,
    pub connections: ListA11y,
    pub workspaces: ListA11y,
    pub agents: ListA11y,
    pub tabs: ListA11y,
    pub toolbar: ToolbarA11y,
    pub new_workspace: ControlA11y,
    pub status_profile: ControlA11y,
    pub status_message: ControlA11y,
    pub status_value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaneA11y {
    pub id: String,
    pub role: Role,
    pub name: String,
    pub selected: bool,
    pub value: String,
}

pub struct ChromeA11yInput<'a> {
    pub sessions: &'a [SessionSummary],
    pub session_index: Option<usize>,
    pub snapshot: Option<&'a HierarchySnapshot>,
    pub selection: &'a Selection,
    pub i18n: I18n,
    pub appearance_open: bool,
    pub herdr_settings_open: bool,
    pub node_manager_open: bool,
    pub prefix_pending: bool,
    pub operation: Option<&'a str>,
    pub has_error: bool,
    pub event_stream: &'a EventStreamState,
    pub profile_label: &'a str,
}

pub fn chrome_a11y(input: ChromeA11yInput<'_>) -> ChromeA11y {
    let i18n = input.i18n;
    let connections = ListA11y {
        id: "connections-list",
        role: Role::List,
        name: i18n.text("SESSIONS").to_owned(),
        items: input
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let name = if session.default {
                    i18n.text("Default").to_owned()
                } else {
                    session.display_name().to_owned()
                };
                ControlA11y {
                    id: if session.default {
                        "default".into()
                    } else {
                        session.name.clone()
                    },
                    // Button maps to AXButton on macOS; ListItem is dropped from the AX tree.
                    role: Role::Button,
                    name,
                    selected: Some(input.session_index == Some(index)),
                    toggled: None,
                    tab_stop: false,
                }
            })
            .collect(),
    };

    let mut workspaces = Vec::new();
    let mut tabs = Vec::new();
    let mut agents = Vec::new();
    if let Some(snapshot) = input.snapshot {
        for workspace in &snapshot.workspaces {
            workspaces.push(ControlA11y {
                id: workspace.workspace_id.clone(),
                role: Role::Button,
                name: workspace.label.clone(),
                selected: Some(
                    input.selection.workspace_id.as_deref()
                        == Some(workspace.workspace_id.as_str()),
                ),
                toggled: None,
                tab_stop: false,
            });
        }
        if let Some(workspace_id) = input.selection.workspace_id.as_deref() {
            for tab in snapshot.tabs_for(workspace_id) {
                tabs.push(ControlA11y {
                    id: tab.tab_id.clone(),
                    role: Role::Tab,
                    name: tab.label.clone(),
                    selected: Some(input.selection.tab_id.as_deref() == Some(tab.tab_id.as_str())),
                    toggled: None,
                    tab_stop: false,
                });
            }
        }
        let mut seen_agents = std::collections::HashSet::new();
        for pane in &snapshot.panes {
            let Some(agent_name) = pane.display_agent.as_deref().or(pane.agent.as_deref()) else {
                continue;
            };
            if !seen_agents.insert(agent_name.to_owned()) {
                continue;
            }
            agents.push(ControlA11y {
                id: agent_name.to_owned(),
                role: Role::Button,
                name: format!("{} · {}", agent_name, i18n.text(pane.agent_status.label())),
                selected: Some(input.selection.pane_id.as_deref() == Some(pane.pane_id.as_str())),
                toggled: None,
                tab_stop: false,
            });
        }
    }

    let status_text = status_value(&input);
    ChromeA11y {
        sidebar: RegionA11y {
            id: "sidebar",
            // Toolbar survives macOS AX; Navigation/Main/Status collapse to AXGroup and are dropped.
            role: Role::Toolbar,
            name: i18n.text("Spaces").to_owned(),
        },
        main: RegionA11y {
            id: "main",
            role: Role::Toolbar,
            name: i18n.text("Terminal").to_owned(),
        },
        status: RegionA11y {
            id: "status-bar",
            role: Role::Toolbar,
            name: i18n.text("Status bar").to_owned(),
        },
        connections,
        workspaces: ListA11y {
            id: "workspaces-list",
            role: Role::List,
            name: i18n.text("WORKSPACES").to_owned(),
            items: workspaces,
        },
        agents: ListA11y {
            id: "agents-list",
            role: Role::List,
            name: i18n.text("AGENTS").to_owned(),
            items: agents,
        },
        tabs: ListA11y {
            id: "tab-list",
            role: Role::TabList,
            name: i18n.text("Tabs").to_owned(),
            items: tabs,
        },
        toolbar: ToolbarA11y {
            new_tab: toolbar_button("new-tab", i18n.text("New tab")),
            split_right: toolbar_button("split-right", i18n.text("Split pane right")),
            split_down: toolbar_button("split-down", i18n.text("Split pane down")),
            zoom: toolbar_button("zoom-pane", i18n.text("Zoom pane")),
            close_pane: toolbar_button("close-pane", i18n.text("Close pane")),
            appearance: toolbar_toggle(
                "open-appearance",
                i18n.text("Appearance"),
                input.appearance_open,
            ),
            herdr_settings: toolbar_toggle(
                "open-herdr-settings",
                i18n.text("Herdr settings"),
                input.herdr_settings_open,
            ),
            remote: toolbar_toggle("manage-nodes", i18n.text("Hosts"), input.node_manager_open),
        },
        new_workspace: ControlA11y {
            id: "new-workspace".into(),
            role: Role::Button,
            name: i18n.text("New workspace").to_owned(),
            selected: None,
            toggled: None,
            tab_stop: true,
        },
        status_profile: silent_button("status-profile", input.profile_label.to_owned()),
        status_message: silent_button("status-message", status_text.clone()),
        status_value: status_text,
    }
}

pub fn pane_a11y(
    pane: &PaneInfo,
    selected: bool,
    screen_text: Option<&str>,
    waiting: bool,
    i18n: I18n,
) -> PaneA11y {
    PaneA11y {
        id: pane.pane_id.clone(),
        role: Role::Terminal,
        name: pane.display_name().to_owned(),
        selected,
        value: terminal_a11y_value(screen_text, waiting, i18n),
    }
}

pub fn terminal_a11y_value(screen_text: Option<&str>, waiting: bool, i18n: I18n) -> String {
    if waiting {
        return i18n.text("Waiting for terminal frame…").to_owned();
    }
    match screen_text {
        Some(text) if !text.trim().is_empty() => text.to_owned(),
        _ => i18n.text("Empty terminal").to_owned(),
    }
}

pub fn apply_control(element: Stateful<Div>, control: &ControlA11y) -> Stateful<Div> {
    let mut element = element
        .role(control.role)
        .aria_label(control.name.clone())
        .tab_stop(control.tab_stop);
    if let Some(selected) = control.selected {
        element = element.aria_selected(selected);
    }
    if let Some(toggled) = control.toggled {
        element = element.aria_toggled(if toggled {
            Toggled::True
        } else {
            Toggled::False
        });
    }
    element
}

pub fn apply_region(element: Stateful<Div>, region: &RegionA11y) -> Stateful<Div> {
    element
        .role(region.role)
        .aria_label(region.name.clone())
        .tab_stop(false)
}

pub fn apply_list(element: Stateful<Div>, list: &ListA11y) -> Stateful<Div> {
    element.role(list.role).aria_label(list.name.clone())
}

pub fn apply_dialog(
    element: Div,
    id: &'static str,
    title: impl Into<ochub_ui::gpui::SharedString>,
) -> Stateful<Div> {
    element.id(id).role(Role::Dialog).aria_label(title)
}

fn silent_button(id: &str, name: String) -> ControlA11y {
    ControlA11y {
        id: id.into(),
        role: Role::Button,
        name,
        selected: None,
        toggled: None,
        tab_stop: false,
    }
}

fn toolbar_button(id: &str, name: &str) -> ControlA11y {
    ControlA11y {
        id: id.into(),
        role: Role::Button,
        name: name.to_owned(),
        selected: None,
        toggled: None,
        tab_stop: true,
    }
}

fn toolbar_toggle(id: &str, name: &str, on: bool) -> ControlA11y {
    ControlA11y {
        id: id.into(),
        role: Role::Button,
        name: name.to_owned(),
        selected: Some(on),
        toggled: Some(on),
        tab_stop: true,
    }
}

fn status_value(input: &ChromeA11yInput<'_>) -> String {
    let i18n = input.i18n;
    if input.prefix_pending {
        format!(
            "{} · {}",
            i18n.text("PREFIX"),
            i18n.text("C new tab · ⇧N new workspace · S settings · 1–9 switch tab")
        )
    } else if let Some(operation) = input.operation {
        operation.to_owned()
    } else if input.has_error {
        i18n.text("Connection unavailable").to_owned()
    } else if matches!(input.event_stream, EventStreamState::Lost(_)) {
        event_stream_lost_copy(i18n)
    } else if let Some(snapshot) = input.snapshot {
        event_stream_status_copy(i18n, input.event_stream, snapshot)
    } else {
        i18n.text("No Herdr session").to_owned()
    }
}

pub(crate) fn event_stream_lost_copy(i18n: I18n) -> String {
    i18n.text("Live updates disconnected — click to reconnect")
        .to_owned()
}

pub(crate) fn event_stream_status_copy(
    i18n: I18n,
    stream: &EventStreamState,
    snapshot: &HierarchySnapshot,
) -> String {
    match stream {
        EventStreamState::Lost(_) => event_stream_lost_copy(i18n),
        EventStreamState::Live => i18n.herdr_status(
            &snapshot.version,
            snapshot.protocol,
            snapshot.workspaces.len(),
        ),
        EventStreamState::Idle => i18n.herdr_snapshot_status(
            &snapshot.version,
            snapshot.protocol,
            snapshot.workspaces.len(),
        ),
    }
}

impl OcHerdrView {
    pub(super) fn chrome_a11y(&self) -> ChromeA11y {
        let profile = self.current_profile();
        let profile_label = profile_display_label(&profile, self.i18n);
        chrome_a11y(ChromeA11yInput {
            sessions: &self.sessions,
            session_index: self.session_index,
            snapshot: self.snapshot.as_ref(),
            selection: &self.selection,
            i18n: self.i18n,
            appearance_open: self.appearance_open,
            herdr_settings_open: self.herdr_settings_open,
            node_manager_open: self.node_manager_open,
            prefix_pending: self.prefix_pending,
            operation: self.operation.as_deref(),
            has_error: self.error.is_some(),
            event_stream: &self.event_stream,
            profile_label: &profile_label,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use ocherdr_core::{AgentStatus, PaneInfo, TabInfo, WorkspaceInfo};

    use super::*;
    use crate::i18n::Language;

    fn live_event_stream() -> EventStreamState {
        EventStreamState::Live
    }

    fn sample_snapshot() -> HierarchySnapshot {
        HierarchySnapshot {
            version: "0.8.2".into(),
            protocol: 20,
            focused_workspace_id: Some("w1".into()),
            focused_tab_id: Some("t1".into()),
            focused_pane_id: Some("p1".into()),
            workspaces: vec![
                workspace("w1", 1, "schedule review", true, "t1"),
                workspace("w2", 2, "code", false, "t3"),
            ],
            tabs: vec![
                tab("t1", "w1", 1, "1", true),
                tab("t2", "w1", 2, "logs", false),
                tab("t3", "w2", 1, "other", false),
            ],
            panes: vec![
                pane("p1", "t1", "w1", "grok", AgentStatus::Idle, true),
                pane("p2", "t1", "w1", "codex", AgentStatus::Working, false),
            ],
            layouts: Vec::new(),
            agents: Vec::new(),
        }
    }

    fn workspace(
        id: &str,
        number: usize,
        label: &str,
        focused: bool,
        active_tab_id: &str,
    ) -> WorkspaceInfo {
        WorkspaceInfo {
            workspace_id: id.into(),
            number,
            label: label.into(),
            focused,
            pane_count: 2,
            tab_count: 2,
            active_tab_id: active_tab_id.into(),
            agent_status: AgentStatus::Idle,
            tokens: HashMap::new(),
            worktree: None,
        }
    }

    fn tab(id: &str, workspace_id: &str, number: usize, label: &str, focused: bool) -> TabInfo {
        TabInfo {
            tab_id: id.into(),
            workspace_id: workspace_id.into(),
            number,
            label: label.into(),
            focused,
            pane_count: 2,
            agent_status: AgentStatus::Idle,
        }
    }

    fn pane(
        id: &str,
        tab_id: &str,
        workspace_id: &str,
        agent: &str,
        agent_status: AgentStatus,
        focused: bool,
    ) -> PaneInfo {
        PaneInfo {
            pane_id: id.into(),
            terminal_id: format!("term-{id}"),
            workspace_id: workspace_id.into(),
            tab_id: tab_id.into(),
            focused,
            cwd: None,
            foreground_cwd: None,
            label: None,
            agent: Some(agent.into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: Some(agent.into()),
            agent_status,
            state_labels: HashMap::new(),
            tokens: HashMap::new(),
            revision: 1,
        }
    }

    fn sample_sessions() -> Vec<SessionSummary> {
        vec![
            SessionSummary {
                name: "default".into(),
                default: true,
                running: true,
                socket_path: PathBuf::new(),
                session_dir: PathBuf::new(),
            },
            SessionSummary {
                name: "other".into(),
                default: false,
                running: false,
                socket_path: PathBuf::new(),
                session_dir: PathBuf::new(),
            },
        ]
    }

    fn sample_selection() -> Selection {
        Selection {
            connection_id: "local".into(),
            session_name: Some("default".into()),
            workspace_id: Some("w1".into()),
            tab_id: Some("t1".into()),
            pane_id: Some("p1".into()),
        }
    }

    fn sample_input<'a>(
        sessions: &'a [SessionSummary],
        snapshot: &'a HierarchySnapshot,
        selection: &'a Selection,
        event_stream: &'a EventStreamState,
    ) -> ChromeA11yInput<'a> {
        ChromeA11yInput {
            sessions,
            session_index: Some(0),
            snapshot: Some(snapshot),
            selection,
            i18n: I18n::new(Language::English),
            appearance_open: false,
            herdr_settings_open: false,
            node_manager_open: true,
            prefix_pending: false,
            operation: None,
            has_error: false,
            event_stream,
            profile_label: "This Mac",
        }
    }

    fn item<'a>(list: &'a ListA11y, id: &str) -> &'a ControlA11y {
        list.items
            .iter()
            .find(|item| item.id == id)
            .unwrap_or_else(|| panic!("missing control {id}"))
    }

    #[test]
    fn live_idle_and_lost_event_streams_map_to_distinct_status_copy() {
        let snapshot = sample_snapshot();
        let live = live_event_stream();
        let lost = EventStreamState::Lost("event worker stopped".into());
        let english = I18n::new(Language::English);
        let chinese = I18n::new(Language::SimplifiedChinese);

        assert_eq!(
            event_stream_status_copy(english, &live, &snapshot),
            "Herdr 0.8.2 · protocol 20 · connected · subscription active · 2 workspaces"
        );
        assert_eq!(
            event_stream_status_copy(english, &EventStreamState::Idle, &snapshot),
            "Herdr 0.8.2 · protocol 20 · connected · snapshot · 2 workspaces"
        );
        assert_eq!(
            event_stream_status_copy(english, &lost, &snapshot),
            "Live updates disconnected — click to reconnect"
        );
        assert_eq!(
            event_stream_status_copy(chinese, &live, &snapshot),
            "Herdr 0.8.2 · 协议 20 · 已连接 · 实时订阅 · 2 个工作区"
        );
        assert_eq!(
            event_stream_status_copy(chinese, &EventStreamState::Idle, &snapshot),
            "Herdr 0.8.2 · 协议 20 · 已连接 · 状态快照 · 2 个工作区"
        );
        assert_eq!(
            event_stream_status_copy(chinese, &lost, &snapshot),
            "实时更新已断开 · 点击重新连接"
        );
    }

    #[test]
    fn chrome_a11y_announces_a_lost_event_stream() {
        let sessions = sample_sessions();
        let snapshot = sample_snapshot();
        let selection = sample_selection();
        let stream = EventStreamState::Lost("event worker stopped".into());
        let chrome = chrome_a11y(sample_input(&sessions, &snapshot, &selection, &stream));
        assert_eq!(
            chrome.status_value,
            "Live updates disconnected — click to reconnect"
        );
        assert_eq!(chrome.status_message.name, chrome.status_value);
        assert_eq!(chrome.status_message.role, Role::Button);
    }

    #[test]
    fn chrome_a11y_names_roles_selected_state_and_regions() {
        let sessions = sample_sessions();
        let snapshot = sample_snapshot();
        let selection = sample_selection();
        let stream = live_event_stream();
        let chrome = chrome_a11y(sample_input(&sessions, &snapshot, &selection, &stream));

        assert_eq!(chrome.sidebar.role, Role::Toolbar);
        assert_eq!(chrome.sidebar.name, "Spaces");
        assert_eq!(chrome.main.role, Role::Toolbar);
        assert_eq!(chrome.main.name, "Terminal");
        assert_eq!(chrome.status.role, Role::Toolbar);
        assert_eq!(chrome.status.name, "Status bar");
        assert!(chrome.status_value.contains("Herdr 0.8.2"));
        assert!(chrome.status_value.contains("protocol 20"));
        assert!(chrome.status_value.contains("2 workspaces"));
        assert_eq!(chrome.status_profile.role, Role::Button);
        assert!(!chrome.status_profile.tab_stop);
        assert_eq!(chrome.status_profile.name, "This Mac");
        assert_eq!(chrome.status_message.role, Role::Button);
        assert!(!chrome.status_message.tab_stop);
        assert_eq!(chrome.status_message.name, chrome.status_value);

        assert_eq!(chrome.connections.role, Role::List);
        assert_eq!(chrome.connections.name, "SESSIONS");
        let default = item(&chrome.connections, "default");
        assert_eq!(default.role, Role::Button);
        assert!(!default.tab_stop);
        assert_eq!(default.name, "Default");
        assert_eq!(default.selected, Some(true));
        let other = item(&chrome.connections, "other");
        assert_eq!(other.name, "other");
        assert_eq!(other.selected, Some(false));

        assert_eq!(chrome.workspaces.role, Role::List);
        assert_eq!(chrome.workspaces.name, "WORKSPACES");
        let current = item(&chrome.workspaces, "w1");
        assert_eq!(current.role, Role::Button);
        assert!(!current.tab_stop);
        assert_eq!(current.name, "schedule review");
        assert_eq!(current.selected, Some(true));
        assert_eq!(item(&chrome.workspaces, "w2").selected, Some(false));

        assert_eq!(chrome.tabs.role, Role::TabList);
        assert_eq!(chrome.tabs.name, "Tabs");
        assert_eq!(
            chrome.tabs.items.len(),
            2,
            "tabs from the selected workspace only"
        );
        let tab = item(&chrome.tabs, "t1");
        assert_eq!(tab.role, Role::Tab);
        assert_eq!(tab.name, "1");
        assert_eq!(tab.selected, Some(true));
        assert_eq!(item(&chrome.tabs, "t2").name, "logs");
        assert_eq!(item(&chrome.tabs, "t2").selected, Some(false));

        let grok = item(&chrome.agents, "grok");
        assert_eq!(grok.role, Role::Button);
        assert!(!grok.tab_stop);
        assert_eq!(grok.name, "grok · idle");
        assert_eq!(grok.selected, Some(true));
        let codex = item(&chrome.agents, "codex");
        assert_eq!(codex.name, "codex · working");
        assert_eq!(codex.selected, Some(false));

        for action in chrome.toolbar.actions() {
            assert_eq!(action.role, Role::Button);
            assert!(!action.name.is_empty(), "{}", action.id);
        }
        assert_eq!(chrome.toolbar.new_tab.name, "New tab");
        assert_eq!(chrome.toolbar.split_right.name, "Split pane right");
        assert_eq!(chrome.toolbar.split_down.name, "Split pane down");
        assert_eq!(chrome.toolbar.zoom.name, "Zoom pane");
        assert_eq!(chrome.toolbar.close_pane.name, "Close pane");
        assert_eq!(chrome.toolbar.appearance.name, "Appearance");
        assert_eq!(chrome.toolbar.herdr_settings.name, "Herdr settings");
        assert_eq!(chrome.toolbar.remote.name, "Hosts");
        assert_eq!(chrome.toolbar.remote.selected, Some(true));
        assert_eq!(chrome.toolbar.remote.toggled, Some(true));
        assert_eq!(chrome.toolbar.appearance.selected, Some(false));

        assert_eq!(chrome.new_workspace.role, Role::Button);
        assert_eq!(chrome.new_workspace.name, "New workspace");

        for control in chrome
            .connections
            .items
            .iter()
            .chain(&chrome.workspaces.items)
            .chain(&chrome.agents.items)
            .chain(&chrome.tabs.items)
        {
            assert_ne!(control.role, Role::GenericContainer);
            assert!(
                !control.tab_stop,
                "{} must not be a keyboard tab stop",
                control.id
            );
            assert!(!control.name.is_empty(), "{}", control.id);
        }
    }

    #[test]
    fn chrome_regions_exist_without_a_session_snapshot() {
        let i18n = I18n::new(Language::English);
        let selection = Selection::default();
        let chrome = chrome_a11y(ChromeA11yInput {
            sessions: &[],
            session_index: None,
            snapshot: None,
            selection: &selection,
            i18n,
            appearance_open: false,
            herdr_settings_open: false,
            node_manager_open: false,
            prefix_pending: false,
            operation: None,
            has_error: false,
            event_stream: &EventStreamState::Idle,
            profile_label: "This Mac",
        });
        assert_eq!(chrome.sidebar.role, Role::Toolbar);
        assert_eq!(chrome.sidebar.name, "Spaces");
        assert_eq!(chrome.main.role, Role::Toolbar);
        assert_eq!(chrome.main.name, "Terminal");
        assert_eq!(chrome.status.role, Role::Toolbar);
        assert_eq!(chrome.status.name, "Status bar");
        assert_eq!(chrome.status_value, "No Herdr session");
        assert_eq!(chrome.status_profile.name, "This Mac");
        assert_eq!(chrome.status_message.name, "No Herdr session");
        assert!(chrome.workspaces.items.is_empty());
        assert!(chrome.tabs.items.is_empty());
    }

    #[test]
    fn terminal_a11y_value_uses_screen_text_or_named_empty_or_waiting() {
        let english = I18n::new(Language::English);
        assert_eq!(
            terminal_a11y_value(Some("~/code\nls"), false, english),
            "~/code\nls"
        );
        assert_eq!(
            terminal_a11y_value(None, true, english),
            "Waiting for terminal frame…"
        );
        assert_eq!(
            terminal_a11y_value(Some("   \n"), false, english),
            "Empty terminal"
        );
        assert_eq!(terminal_a11y_value(None, false, english), "Empty terminal");
        assert_eq!(
            terminal_a11y_value(Some("still there"), true, english),
            "Waiting for terminal frame…"
        );

        let chinese = I18n::new(Language::SimplifiedChinese);
        assert_eq!(
            terminal_a11y_value(None, true, chinese),
            "正在等待终端画面…"
        );
        assert_eq!(terminal_a11y_value(None, false, chinese), "空终端");
    }

    #[test]
    fn pane_a11y_publishes_name_selected_and_screen_value() {
        let i18n = I18n::new(Language::English);
        let snapshot = sample_snapshot();
        let grok = pane_a11y(
            &snapshot.panes[0],
            true,
            Some("hello from grok"),
            false,
            i18n,
        );
        assert_eq!(grok.role, Role::Terminal);
        assert_eq!(grok.name, "grok");
        assert!(grok.selected);
        assert_eq!(grok.value, "hello from grok");

        let waiting = pane_a11y(&snapshot.panes[1], false, None, true, i18n);
        assert_eq!(waiting.name, "codex");
        assert!(!waiting.selected);
        assert_eq!(waiting.value, "Waiting for terminal frame…");
    }
}
