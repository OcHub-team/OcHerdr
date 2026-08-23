//! Product-neutral state for the OcHerdr desktop client.
//!
//! Herdr remains authoritative. This crate only keeps the latest public API
//! snapshot and the user's current GUI selection.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod event;
pub use event::{
    AGENT_STATUS_HANDOFF_LIMIT, AgentStatusHandoff, HerdrEvent, SnapshotUpdate,
    agent_status_handoff_push, agent_status_handoff_take, agent_status_panes_after_stream_closed,
    agent_status_stream_should_rebuild, event_panes_after_failed_subscribe,
};

pub const MINIMUM_HERDR_VERSION: &str = "0.8.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionProfile {
    Local {
        #[serde(default = "default_herdr_path")]
        herdr_path: String,
    },
    Ssh {
        id: String,
        label: String,
        destination: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_file: Option<PathBuf>,
        #[serde(default = "default_herdr_path")]
        herdr_path: String,
    },
}

fn default_herdr_path() -> String {
    "herdr".into()
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self::Local {
            herdr_path: default_herdr_path(),
        }
    }
}

impl ConnectionProfile {
    pub fn id(&self) -> &str {
        match self {
            Self::Local { .. } => "local",
            Self::Ssh { id, .. } => id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Local { .. } => "Local",
            Self::Ssh { label, .. } => label,
        }
    }

    pub fn herdr_path(&self) -> &str {
        match self {
            Self::Local { herdr_path } | Self::Ssh { herdr_path, .. } => herdr_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub name: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub socket_path: PathBuf,
    #[serde(default)]
    pub session_dir: PathBuf,
}

impl SessionSummary {
    pub fn display_name(&self) -> &str {
        if self.default { "Default" } else { &self.name }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[default]
    Unknown,
}

impl AgentStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub tab_count: usize,
    pub active_tab_id: String,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceWorktreeInfo {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub checkout_path: String,
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    pub tab_id: String,
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    #[serde(default)]
    pub agent_status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub focused: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub terminal_title: Option<String>,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
    #[serde(default)]
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub state_labels: HashMap<String, String>,
    #[serde(default)]
    pub tokens: HashMap<String, String>,
    #[serde(default)]
    pub revision: u64,
}

impl PaneInfo {
    pub fn display_name(&self) -> &str {
        self.label
            .as_deref()
            .or(self.display_agent.as_deref())
            .or(self.terminal_title_stripped.as_deref())
            .or(self.title.as_deref())
            .unwrap_or(&self.pane_id)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutPane {
    pub pane_id: String,
    pub focused: bool,
    pub rect: LayoutRect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutSplit {
    pub id: String,
    pub direction: SplitDirection,
    pub ratio: f32,
    pub rect: LayoutRect,
}

impl LayoutSplit {
    pub fn path(&self) -> Option<Vec<bool>> {
        parse_split_path_id(&self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Right,
    Down,
}

/// Herdr's `Layout::set_ratio_at` clamps to this range, so a pane cannot be
/// dragged to zero cells. Preview uses the same bounds so the indicator lands
/// where the authority will actually put the split.
pub const SPLIT_RATIO_MIN: f32 = 0.1;
pub const SPLIT_RATIO_MAX: f32 = 0.9;

/// Split ids in `layout.updated` are `split_{index}_root` or `split_{index}_{01…}`,
/// encoding the `layout.set_split_ratio` path (`false` = first child).
pub fn parse_split_path_id(id: &str) -> Option<Vec<bool>> {
    let rest = id.strip_prefix("split_")?;
    let (index, path) = rest.split_once('_')?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if path == "root" {
        return Some(Vec::new());
    }
    if path.is_empty() {
        return None;
    }
    path.chars()
        .map(|c| match c {
            '0' => Some(false),
            '1' => Some(true),
            _ => None,
        })
        .collect()
}

/// Map a pointer in the same coordinate space as `rect` to a clamped split ratio.
pub fn split_ratio_from_drag(direction: SplitDirection, rect: LayoutRect, pointer: f32) -> f32 {
    let (origin, size) = match direction {
        SplitDirection::Right => (rect.x, rect.width),
        SplitDirection::Down => (rect.y, rect.height),
    };
    ((pointer - f32::from(origin)) / f32::from(size)).clamp(SPLIT_RATIO_MIN, SPLIT_RATIO_MAX)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneLayout {
    pub workspace_id: String,
    pub tab_id: String,
    pub zoomed: bool,
    pub area: LayoutRect,
    pub focused_pane_id: String,
    pub panes: Vec<LayoutPane>,
    pub splits: Vec<LayoutSplit>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HierarchySnapshot {
    pub version: String,
    pub protocol: u32,
    #[serde(default)]
    pub focused_workspace_id: Option<String>,
    #[serde(default)]
    pub focused_tab_id: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
    #[serde(default)]
    pub tabs: Vec<TabInfo>,
    #[serde(default)]
    pub panes: Vec<PaneInfo>,
    #[serde(default, rename = "layouts")]
    pub layouts: Vec<PaneLayout>,
    #[serde(default)]
    pub agents: Vec<serde_json::Value>,
}

impl HierarchySnapshot {
    pub fn tabs_for<'a>(&'a self, workspace_id: &'a str) -> impl Iterator<Item = &'a TabInfo> {
        self.tabs
            .iter()
            .filter(move |tab| tab.workspace_id == workspace_id)
    }

    pub fn panes_for<'a>(&'a self, tab_id: &'a str) -> impl Iterator<Item = &'a PaneInfo> {
        self.panes.iter().filter(move |pane| pane.tab_id == tab_id)
    }

    pub fn layout_for(&self, tab_id: &str) -> Option<&PaneLayout> {
        self.layouts.iter().find(|layout| layout.tab_id == tab_id)
    }

    pub fn pane(&self, pane_id: &str) -> Option<&PaneInfo> {
        self.panes.iter().find(|pane| pane.pane_id == pane_id)
    }

    pub fn pane_ids(&self) -> HashSet<String> {
        self.panes.iter().map(|pane| pane.pane_id.clone()).collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub connection_id: String,
    pub session_name: Option<String>,
    pub workspace_id: Option<String>,
    pub tab_id: Option<String>,
    pub pane_id: Option<String>,
}

impl Selection {
    pub fn reconcile(&mut self, snapshot: &HierarchySnapshot) {
        if self.workspace_id.as_ref().is_none_or(|id| {
            !snapshot
                .workspaces
                .iter()
                .any(|item| &item.workspace_id == id)
        }) {
            self.workspace_id = snapshot.focused_workspace_id.clone().or_else(|| {
                snapshot
                    .workspaces
                    .first()
                    .map(|item| item.workspace_id.clone())
            });
        }
        if self.tab_id.as_ref().is_none_or(|id| {
            !snapshot.tabs.iter().any(|item| {
                &item.tab_id == id && Some(&item.workspace_id) == self.workspace_id.as_ref()
            })
        }) {
            self.tab_id = snapshot
                .focused_tab_id
                .clone()
                .filter(|id| snapshot.tabs.iter().any(|item| &item.tab_id == id))
                .or_else(|| {
                    self.workspace_id.as_deref().and_then(|workspace_id| {
                        snapshot
                            .tabs_for(workspace_id)
                            .next()
                            .map(|item| item.tab_id.clone())
                    })
                });
        }
        if self.pane_id.as_ref().is_none_or(|id| {
            !snapshot
                .panes
                .iter()
                .any(|item| &item.pane_id == id && Some(&item.tab_id) == self.tab_id.as_ref())
        }) {
            self.pane_id = snapshot
                .focused_pane_id
                .clone()
                .filter(|id| snapshot.panes.iter().any(|item| &item.pane_id == id))
                .or_else(|| {
                    self.tab_id.as_deref().and_then(|tab_id| {
                        snapshot
                            .panes_for(tab_id)
                            .next()
                            .map(|item| item.pane_id.clone())
                    })
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_snapshot_fields_are_ignored() {
        let snapshot: HierarchySnapshot = serde_json::from_value(serde_json::json!({
            "version": "0.8.1",
            "protocol": 7,
            "focused_workspace_id": "w1",
            "focused_tab_id": "w1:t1",
            "focused_pane_id": "w1:p1",
            "workspaces": [{
                "workspace_id": "w1", "number": 1, "label": "core", "focused": true,
                "pane_count": 1, "tab_count": 1, "active_tab_id": "w1:t1",
                "agent_status": "working", "future_field": true
            }],
            "tabs": [{
                "tab_id": "w1:t1", "workspace_id": "w1", "number": 1,
                "label": "shell", "focused": true, "pane_count": 1,
                "agent_status": "working"
            }],
            "panes": [{
                "pane_id": "w1:p1", "terminal_id": "term-1", "workspace_id": "w1",
                "tab_id": "w1:t1", "focused": true, "agent_status": "working",
                "revision": 3
            }],
            "layouts": [], "agents": [], "unknown": "ignored"
        }))
        .unwrap();
        assert_eq!(snapshot.panes[0].display_name(), "w1:p1");
        assert_eq!(snapshot.workspaces[0].agent_status, AgentStatus::Working);
    }

    #[test]
    fn selection_follows_authoritative_focus() {
        let mut selection = Selection::default();
        let snapshot = HierarchySnapshot {
            focused_workspace_id: Some("w1".into()),
            focused_tab_id: Some("t1".into()),
            focused_pane_id: Some("p1".into()),
            workspaces: vec![WorkspaceInfo {
                workspace_id: "w1".into(),
                number: 1,
                label: "one".into(),
                focused: true,
                pane_count: 1,
                tab_count: 1,
                active_tab_id: "t1".into(),
                agent_status: AgentStatus::Idle,
                tokens: HashMap::new(),
                worktree: None,
            }],
            tabs: vec![TabInfo {
                tab_id: "t1".into(),
                workspace_id: "w1".into(),
                number: 1,
                label: "tab".into(),
                focused: true,
                pane_count: 1,
                agent_status: AgentStatus::Idle,
            }],
            panes: vec![PaneInfo {
                pane_id: "p1".into(),
                terminal_id: "term".into(),
                workspace_id: "w1".into(),
                tab_id: "t1".into(),
                focused: true,
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
            }],
            ..Default::default()
        };
        selection.reconcile(&snapshot);
        assert_eq!(selection.pane_id.as_deref(), Some("p1"));
    }

    fn layout_rect(x: u16, y: u16, width: u16, height: u16) -> LayoutRect {
        LayoutRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn dragging_a_right_split_handle_to_the_center_yields_half() {
        let rect = layout_rect(0, 0, 100, 50);
        assert_eq!(split_ratio_from_drag(SplitDirection::Right, rect, 50.), 0.5);
    }

    #[test]
    fn dragging_a_down_split_handle_to_the_center_yields_half() {
        let rect = layout_rect(0, 0, 100, 50);
        assert_eq!(split_ratio_from_drag(SplitDirection::Down, rect, 25.), 0.5);
    }

    #[test]
    fn dragging_a_split_handle_to_an_edge_clamps_away_from_zero_and_one() {
        let rect = layout_rect(0, 0, 100, 50);
        assert_eq!(
            split_ratio_from_drag(SplitDirection::Right, rect, 0.),
            SPLIT_RATIO_MIN
        );
        assert_eq!(
            split_ratio_from_drag(SplitDirection::Right, rect, 100.),
            SPLIT_RATIO_MAX
        );
        assert_eq!(
            split_ratio_from_drag(SplitDirection::Down, rect, -20.),
            SPLIT_RATIO_MIN
        );
        assert_eq!(
            split_ratio_from_drag(SplitDirection::Down, rect, 80.),
            SPLIT_RATIO_MAX
        );
    }

    #[test]
    fn dragging_a_split_handle_measures_ratio_from_the_split_rect_origin() {
        let rect = layout_rect(20, 10, 80, 40);
        assert_eq!(split_ratio_from_drag(SplitDirection::Right, rect, 60.), 0.5);
        assert_eq!(split_ratio_from_drag(SplitDirection::Down, rect, 30.), 0.5);
    }

    #[test]
    fn herdr_split_ids_decode_to_the_set_split_ratio_path() {
        assert_eq!(parse_split_path_id("split_0_root"), Some(vec![]));
        assert_eq!(parse_split_path_id("split_1_0"), Some(vec![false]));
        assert_eq!(parse_split_path_id("split_2_01"), Some(vec![false, true]));
        assert_eq!(parse_split_path_id("pane-1"), None);
    }
}
