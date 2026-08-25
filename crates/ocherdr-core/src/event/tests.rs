use std::collections::{HashMap, HashSet, VecDeque};

use super::*;
use crate::{AgentStatus, LayoutPane, LayoutRect, WorkspaceWorktreeInfo};

pub(super) fn workspace(id: &str, label: &str, focused: bool) -> WorkspaceInfo {
    WorkspaceInfo {
        workspace_id: id.into(),
        number: 1,
        label: label.into(),
        focused,
        pane_count: 1,
        tab_count: 1,
        active_tab_id: format!("{id}:t1"),
        agent_status: AgentStatus::Idle,
        tokens: HashMap::new(),
        worktree: None,
    }
}

pub(super) fn tab(id: &str, workspace_id: &str, label: &str, focused: bool) -> TabInfo {
    TabInfo {
        tab_id: id.into(),
        workspace_id: workspace_id.into(),
        number: 1,
        label: label.into(),
        focused,
        pane_count: 1,
        agent_status: AgentStatus::Idle,
    }
}

pub(super) fn pane(id: &str, workspace_id: &str, tab_id: &str, focused: bool) -> PaneInfo {
    PaneInfo {
        pane_id: id.into(),
        terminal_id: id.into(),
        workspace_id: workspace_id.into(),
        tab_id: tab_id.into(),
        focused,
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
        revision: 1,
    }
}

pub(super) fn layout(workspace_id: &str, tab_id: &str, pane_id: &str) -> PaneLayout {
    PaneLayout {
        workspace_id: workspace_id.into(),
        tab_id: tab_id.into(),
        zoomed: false,
        area: LayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        },
        focused_pane_id: pane_id.into(),
        panes: vec![LayoutPane {
            pane_id: pane_id.into(),
            focused: true,
            rect: LayoutRect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        }],
        splits: Vec::new(),
    }
}

pub(super) fn cascade_snapshot() -> HierarchySnapshot {
    HierarchySnapshot {
        focused_workspace_id: Some("w1".into()),
        focused_tab_id: Some("t1".into()),
        focused_pane_id: Some("p1".into()),
        workspaces: vec![workspace("w1", "one", true)],
        tabs: vec![
            tab("t1", "w1", "alpha", true),
            tab("t2", "w1", "beta", false),
        ],
        panes: vec![
            pane("p1", "w1", "t1", true),
            pane("p2", "w1", "t1", false),
            pane("p3", "w1", "t2", false),
        ],
        layouts: vec![layout("w1", "t1", "p1")],
        ..Default::default()
    }
}

pub(super) fn ids(values: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect()
}

#[path = "tests/layout.rs"]
mod layout;
#[path = "tests/pane.rs"]
mod pane;
#[path = "tests/workspace.rs"]
mod workspace;
#[path = "tests/worktree.rs"]
mod worktree;
