//! Accessibility facts derived from Herdr snapshot + GUI selection.
//!
//! Render applies these properties onto GPUI elements (`id` + `role` required).
//! Tests call the same mapping so chrome/terminal announcements cannot drift.

use std::collections::HashSet;

use ocherdr_core::{
    AgentStatus, HierarchySnapshot, PaneInfo, Selection, SessionSummary, TabInfo, WorkspaceInfo,
};
use ochub_ui::gpui::{Div, Role, Stateful, Toggled, prelude::*};

use super::EventStreamState;
use super::OcHerdrView;
use super::Overlay;
use super::i18n::{I18n, k};
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
pub struct SessionRow {
    pub index: usize,
    pub running: bool,
    pub a11y: ControlA11y,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceRow {
    pub number: usize,
    pub agent_status: AgentStatus,
    pub a11y: ControlA11y,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRow {
    pub pane_id: String,
    pub agent_status: AgentStatus,
    pub a11y: ControlA11y,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabRow {
    pub number: usize,
    pub a11y: ControlA11y,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListA11y<T> {
    pub id: &'static str,
    pub role: Role,
    pub name: String,
    pub items: Vec<T>,
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
    pub connections: ListA11y<SessionRow>,
    pub workspaces: ListA11y<WorkspaceRow>,
    pub agents: ListA11y<AgentRow>,
    pub tabs: ListA11y<TabRow>,
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
    pub node_manager_open: bool,
    pub prefix_pending: bool,
    pub operation: Option<&'a str>,
    pub event_stream: &'a EventStreamState,
    pub profile_label: &'a str,
}

pub fn chrome_a11y(input: ChromeA11yInput<'_>) -> ChromeA11y {
    let i18n = input.i18n;
    let connections = ListA11y {
        id: "connections-list",
        role: Role::List,
        name: i18n.text(k::TERMINAL_SESSIONS).to_owned(),
        items: connection_rows(input.sessions, input.session_index, i18n),
    };
    let (workspaces, agents, tabs) = if let Some(snapshot) = input.snapshot {
        let workspaces = workspace_rows(
            &snapshot.workspaces,
            input.selection.workspace_id.as_deref(),
        );
        let agents = agent_rows(&snapshot.panes, input.selection.pane_id.as_deref(), i18n);
        let tabs = if let Some(workspace_id) = input.selection.workspace_id.as_deref() {
            tab_rows(
                snapshot.tabs_for(workspace_id),
                input.selection.tab_id.as_deref(),
            )
        } else {
            Vec::new()
        };
        (workspaces, agents, tabs)
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };

    let status_text = status_value(&input);
    ChromeA11y {
        sidebar: RegionA11y {
            id: "sidebar",
            // Toolbar survives macOS AX; Navigation/Main/Status collapse to AXGroup and are dropped.
            role: Role::Toolbar,
            name: i18n.text(k::TERMINAL_SPACES).to_owned(),
        },
        main: RegionA11y {
            id: "main",
            role: Role::Toolbar,
            name: i18n.text(k::TERMINAL_REGION).to_owned(),
        },
        status: RegionA11y {
            id: "status-bar",
            role: Role::Toolbar,
            name: i18n.text(k::TERMINAL_STATUS_BAR).to_owned(),
        },
        connections,
        workspaces: ListA11y {
            id: "workspaces-list",
            role: Role::List,
            name: i18n.text(k::TERMINAL_WORKSPACES).to_owned(),
            items: workspaces,
        },
        agents: ListA11y {
            id: "agents-list",
            role: Role::List,
            name: i18n.text(k::TERMINAL_AGENTS).to_owned(),
            items: agents,
        },
        tabs: ListA11y {
            id: "tab-list",
            role: Role::TabList,
            name: i18n.text(k::TERMINAL_TABS).to_owned(),
            items: tabs,
        },
        toolbar: ToolbarA11y {
            new_tab: toolbar_button("new-tab", i18n.text(k::TERMINAL_NEW_TAB)),
            split_right: toolbar_button("split-right", i18n.text(k::TERMINAL_SPLIT_PANE_RIGHT)),
            split_down: toolbar_button("split-down", i18n.text(k::TERMINAL_SPLIT_PANE_DOWN)),
            zoom: toolbar_button("zoom-pane", i18n.text(k::TERMINAL_ZOOM_PANE)),
            close_pane: toolbar_button("close-pane", i18n.text(k::TERMINAL_CLOSE_PANE)),
            appearance: toolbar_toggle(
                "open-appearance",
                i18n.text(k::APPEARANCE_TITLE),
                input.appearance_open,
            ),
            herdr_settings: toolbar_button(
                "open-herdr-settings",
                i18n.text(k::TERMINAL_SETTINGS_IN_TERMINAL),
            ),
            remote: toolbar_toggle(
                "manage-nodes",
                i18n.text(k::HOSTS_TITLE),
                input.node_manager_open,
            ),
        },
        new_workspace: ControlA11y {
            id: "new-workspace".into(),
            role: Role::Button,
            name: i18n.text(k::TERMINAL_NEW_WORKSPACE).to_owned(),
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
        return i18n.text(k::TERMINAL_WAITING).to_owned();
    }
    match screen_text {
        Some(text) if !text.trim().is_empty() => text.to_owned(),
        _ => i18n.text(k::TERMINAL_EMPTY).to_owned(),
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

pub fn apply_list<T>(element: Stateful<Div>, list: &ListA11y<T>) -> Stateful<Div> {
    element.role(list.role).aria_label(list.name.clone())
}

pub fn apply_dialog(
    element: Div,
    id: &'static str,
    title: impl Into<ochub_ui::gpui::SharedString>,
) -> Stateful<Div> {
    element.id(id).role(Role::Dialog).aria_label(title)
}

fn connection_rows(
    sessions: &[SessionSummary],
    session_index: Option<usize>,
    i18n: I18n,
) -> Vec<SessionRow> {
    sessions
        .iter()
        .enumerate()
        .map(|(index, session)| {
            let (id, name) = if session.default {
                ("default".into(), i18n.text(k::COMMON_DEFAULT).to_owned())
            } else {
                (session.name.clone(), session.display_name().to_owned())
            };
            SessionRow {
                index,
                running: session.running,
                a11y: list_control(id, Role::Button, name, session_index == Some(index)),
            }
        })
        .collect()
}

fn workspace_rows(workspaces: &[WorkspaceInfo], selected_id: Option<&str>) -> Vec<WorkspaceRow> {
    workspaces
        .iter()
        .map(|workspace| WorkspaceRow {
            number: workspace.number,
            agent_status: workspace.agent_status,
            a11y: list_control(
                workspace.workspace_id.clone(),
                Role::Button,
                workspace.label.clone(),
                selected_id == Some(workspace.workspace_id.as_str()),
            ),
        })
        .collect()
}

fn agent_rows(panes: &[PaneInfo], selected_pane_id: Option<&str>, i18n: I18n) -> Vec<AgentRow> {
    let mut seen_agents = HashSet::new();
    let mut items = Vec::new();
    for pane in panes {
        let Some(agent_name) = pane.display_agent.as_deref().or(pane.agent.as_deref()) else {
            continue;
        };
        if !seen_agents.insert(agent_name) {
            continue;
        }
        items.push(AgentRow {
            pane_id: pane.pane_id.clone(),
            agent_status: pane.agent_status,
            a11y: list_control(
                agent_name.to_owned(),
                Role::Button,
                format!("{} · {}", agent_name, i18n.agent_status(pane.agent_status)),
                selected_pane_id == Some(pane.pane_id.as_str()),
            ),
        });
    }
    items
}

fn tab_rows<'a>(
    tabs: impl IntoIterator<Item = &'a TabInfo>,
    selected_tab_id: Option<&str>,
) -> Vec<TabRow> {
    let mut items: Vec<TabRow> = tabs
        .into_iter()
        .map(|tab| TabRow {
            number: tab.number,
            a11y: list_control(
                tab.tab_id.clone(),
                Role::Tab,
                tab.label.clone(),
                selected_tab_id == Some(tab.tab_id.as_str()),
            ),
        })
        .collect();
    items.sort_by_key(|row| row.number);
    items
}

// ListItem is dropped from the macOS AX tree; Button and Tab survive as AXButton/AXTab.
fn list_control(id: String, role: Role, name: String, selected: bool) -> ControlA11y {
    ControlA11y {
        id,
        role,
        name,
        selected: Some(selected),
        toggled: None,
        tab_stop: false,
    }
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
            i18n.text(k::TERMINAL_PREFIX),
            i18n.text(k::TERMINAL_PREFIX_HINT)
        )
    } else if let Some(operation) = input.operation {
        operation.to_owned()
    } else if matches!(input.event_stream, EventStreamState::Lost(_)) {
        event_stream_lost_copy(i18n)
    } else if let Some(snapshot) = input.snapshot {
        event_stream_status_copy(i18n, input.event_stream, snapshot)
    } else {
        i18n.text(k::TERMINAL_NO_SESSION).to_owned()
    }
}

pub(crate) fn event_stream_lost_copy(i18n: I18n) -> String {
    i18n.text(k::TERMINAL_LIVE_DISCONNECTED).to_owned()
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
            appearance_open: matches!(self.overlay, Overlay::Appearance),
            node_manager_open: self.overlay.host_center(),
            prefix_pending: self.prefix_pending,
            operation: self.operation.as_deref(),
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
            node_manager_open: true,
            prefix_pending: false,
            operation: None,
            event_stream,
            profile_label: "This Mac",
        }
    }

    #[test]
    fn workspace_rows_carry_a11y_from_the_same_workspace() {
        assert!(workspace_rows(&[], None).is_empty());
        assert!(workspace_rows(&[], Some("w1")).is_empty());

        let workspaces = vec![
            workspace("w1", 1, "schedule review", true, "t1"),
            workspace("w2", 2, "code", false, "t3"),
            workspace("w3", 3, "notes", false, "t4"),
        ];
        let rows = workspace_rows(&workspaces, Some("w2"));
        assert_eq!(rows.len(), workspaces.len());
        for (row, workspace) in rows.iter().zip(&workspaces) {
            assert_eq!(row.a11y.id, workspace.workspace_id);
            assert_eq!(row.a11y.name, workspace.label);
        }
        assert_eq!(
            rows.iter().map(|row| row.a11y.selected).collect::<Vec<_>>(),
            vec![Some(false), Some(true), Some(false)]
        );
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
        let default = &chrome.connections.items[0];
        assert_eq!(default.a11y.role, Role::Button);
        assert!(!default.a11y.tab_stop);
        assert_eq!(default.a11y.id, "default");
        assert_eq!(default.a11y.name, "Default");
        assert_eq!(default.a11y.selected, Some(true));
        let other = &chrome.connections.items[1];
        assert_eq!(other.a11y.id, "other");
        assert_eq!(other.a11y.name, "other");
        assert_eq!(other.a11y.selected, Some(false));

        assert_eq!(chrome.workspaces.role, Role::List);
        assert_eq!(chrome.workspaces.name, "WORKSPACES");
        assert_eq!(chrome.workspaces.items.len(), snapshot.workspaces.len());
        let current = &chrome.workspaces.items[0];
        assert_eq!(current.a11y.role, Role::Button);
        assert!(!current.a11y.tab_stop);
        assert_eq!(current.a11y.id, "w1");
        assert_eq!(current.a11y.name, "schedule review");
        assert_eq!(current.a11y.selected, Some(true));
        assert_eq!(chrome.workspaces.items[1].a11y.id, "w2");
        assert_eq!(chrome.workspaces.items[1].a11y.selected, Some(false));

        assert_eq!(chrome.tabs.role, Role::TabList);
        assert_eq!(chrome.tabs.name, "Tabs");
        assert_eq!(
            chrome.tabs.items.len(),
            2,
            "tabs from the selected workspace only"
        );
        let tab = &chrome.tabs.items[0];
        assert_eq!(tab.a11y.role, Role::Tab);
        assert_eq!(tab.a11y.id, "t1");
        assert_eq!(tab.a11y.name, "1");
        assert_eq!(tab.a11y.selected, Some(true));
        assert_eq!(chrome.tabs.items[1].a11y.id, "t2");
        assert_eq!(chrome.tabs.items[1].a11y.name, "logs");
        assert_eq!(chrome.tabs.items[1].a11y.selected, Some(false));

        let grok = &chrome.agents.items[0];
        assert_eq!(grok.a11y.role, Role::Button);
        assert!(!grok.a11y.tab_stop);
        assert_eq!(grok.a11y.id, "grok");
        assert_eq!(grok.a11y.name, "grok · idle");
        assert_eq!(grok.a11y.selected, Some(true));
        let codex = &chrome.agents.items[1];
        assert_eq!(codex.a11y.id, "codex");
        assert_eq!(codex.a11y.name, "codex · working");
        assert_eq!(codex.a11y.selected, Some(false));

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
        assert_eq!(
            chrome.toolbar.herdr_settings.name,
            "Open Herdr settings in Terminal"
        );
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
            .map(|row| &row.a11y)
            .chain(chrome.workspaces.items.iter().map(|row| &row.a11y))
            .chain(chrome.agents.items.iter().map(|row| &row.a11y))
            .chain(chrome.tabs.items.iter().map(|row| &row.a11y))
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
            node_manager_open: false,
            prefix_pending: false,
            operation: None,
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
        assert!(chrome.connections.items.is_empty());
        assert!(chrome.workspaces.items.is_empty());
        assert!(chrome.agents.items.is_empty());
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
