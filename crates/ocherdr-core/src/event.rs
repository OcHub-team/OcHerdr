//! Typed Herdr event payloads and incremental snapshot updates.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Deserialize;

use super::{
    AgentStatus, HierarchySnapshot, PaneInfo, PaneLayout, TabInfo, WorkspaceInfo, WorktreeInfo,
};

/// Outcome of applying one Herdr event to a [`HierarchySnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotUpdate {
    /// The event fully determined the change and is already reflected.
    Applied,
    /// The payload is not enough to update safely; pull a fresh snapshot.
    Resync,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HerdrEvent {
    WorkspaceCreated {
        workspace: WorkspaceInfo,
    },
    WorkspaceUpdated {
        workspace: WorkspaceInfo,
    },
    WorkspaceMetadataUpdated {
        workspace: WorkspaceInfo,
    },
    WorkspaceRenamed {
        workspace_id: String,
        label: String,
    },
    WorkspaceClosed {
        workspace_id: String,
    },
    WorkspaceFocused {
        workspace_id: String,
    },
    WorkspaceMoved {
        workspace_id: String,
        insert_index: usize,
        workspaces: Vec<WorkspaceInfo>,
    },
    WorkspaceReordered {
        workspace_ids: Vec<String>,
        workspaces: Vec<WorkspaceInfo>,
    },
    WorktreeCreated {
        workspace: WorkspaceInfo,
        worktree: WorktreeInfo,
    },
    WorktreeOpened {
        workspace: WorkspaceInfo,
        worktree: WorktreeInfo,
        already_open: bool,
    },
    WorktreeRemoved {
        workspace_id: String,
        #[serde(default)]
        workspace: Option<WorkspaceInfo>,
        worktree: WorktreeInfo,
        forced: bool,
    },
    TabCreated {
        tab: TabInfo,
    },
    TabClosed {
        workspace_id: String,
        tab_id: String,
    },
    TabRenamed {
        workspace_id: String,
        tab_id: String,
        label: String,
    },
    TabFocused {
        workspace_id: String,
        tab_id: String,
    },
    TabMoved {
        workspace_id: String,
        tab_id: String,
        insert_index: usize,
        tabs: Vec<TabInfo>,
    },
    PaneCreated {
        pane: PaneInfo,
    },
    PaneUpdated {
        pane: PaneInfo,
    },
    PaneClosed {
        workspace_id: String,
        pane_id: String,
    },
    PaneFocused {
        workspace_id: String,
        pane_id: String,
    },
    PaneExited {
        workspace_id: String,
        pane_id: String,
    },
    PaneMoved {
        pane: PaneInfo,
        previous_pane_id: String,
        previous_workspace_id: String,
        previous_tab_id: String,
    },
    PaneAgentDetected {
        workspace_id: String,
        pane_id: String,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        released: bool,
        #[serde(default)]
        final_status: Option<AgentStatus>,
    },
    PaneAgentStatusChanged {
        pane_id: String,
        workspace_id: String,
        agent_status: AgentStatus,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        display_agent: Option<String>,
        #[serde(default)]
        state_labels: HashMap<String, String>,
    },
    LayoutUpdated {
        layout: PaneLayout,
    },
    #[serde(other)]
    Unknown,
}

/// True when the live per-pane agent-status subscription no longer covers
/// the snapshot's pane set. Compare the parsed ids; do not enumerate the
/// operations that might have changed them.
pub fn agent_status_stream_should_rebuild(
    subscribed: &HashSet<String>,
    snapshot: &HierarchySnapshot,
) -> bool {
    *subscribed != snapshot.pane_ids()
}

/// After a failed subscribe, restore `previous` when this attempt is still
/// the current target so the next `agent_status_stream_should_rebuild`
/// check retries.
pub fn event_panes_after_failed_subscribe(
    current: &HashSet<String>,
    attempted: &HashSet<String>,
    previous: &HashSet<String>,
) -> HashSet<String> {
    if current == attempted {
        previous.clone()
    } else {
        current.clone()
    }
}

/// After the live per-pane stream dies, forget the subscribed pane set so
/// the next `agent_status_stream_should_rebuild` check reconnects. Empty,
/// so an empty snapshot does not rebuild in a loop. Does not drop an
/// in-flight subscribe→snapshot handoff; those events still belong on
/// the snapshot that was requested at subscribe.
pub fn agent_status_panes_after_stream_closed() -> HashSet<String> {
    HashSet::new()
}

/// Newest-preserving cap for the subscribe→snapshot handoff buffer.
pub const AGENT_STATUS_HANDOFF_LIMIT: usize = 128;

/// Status events held until the post-subscribe snapshot is installed.
#[derive(Debug)]
pub struct AgentStatusHandoff<T> {
    pending: VecDeque<T>,
    resync_after: bool,
}

impl<T> AgentStatusHandoff<T> {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            resync_after: false,
        }
    }

    pub fn push(&mut self, incoming: impl IntoIterator<Item = T>, limit: usize) -> bool {
        agent_status_handoff_push(&mut self.pending, incoming, limit)
    }

    /// A payload the live path would resync on. Replay first, then resync.
    pub fn note_payload_error(&mut self) {
        self.resync_after = true;
    }

    /// Drain the buffer for replay (or flush on snapshot failure) and
    /// whether a follow-up resync is still required.
    pub fn into_release(self) -> (Vec<T>, bool) {
        (self.pending.into_iter().collect(), self.resync_after)
    }
}

impl<T> Default for AgentStatusHandoff<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Push `incoming` onto `pending`, dropping the oldest when over `limit`.
/// Returns whether any oldest events were discarded to stay in bound.
pub fn agent_status_handoff_push<T>(
    pending: &mut VecDeque<T>,
    incoming: impl IntoIterator<Item = T>,
    limit: usize,
) -> bool {
    let mut overflowed = false;
    for item in incoming {
        if pending.len() >= limit {
            pending.pop_front();
            overflowed = true;
        }
        pending.push_back(item);
    }
    overflowed
}

/// Take the buffered events so they can be applied after the snapshot
/// installs, or flushed onto the existing snapshot if that fetch fails.
pub fn agent_status_handoff_take<T>(pending: &mut VecDeque<T>) -> Vec<T> {
    pending.drain(..).collect()
}

impl HierarchySnapshot {
    pub fn apply(&mut self, event: &HerdrEvent) -> SnapshotUpdate {
        match event {
            HerdrEvent::WorkspaceCreated { workspace } => {
                upsert_by_id(
                    &mut self.workspaces,
                    &workspace.workspace_id,
                    |item| &item.workspace_id,
                    workspace.clone(),
                );
            }
            HerdrEvent::WorkspaceUpdated { workspace }
            | HerdrEvent::WorkspaceMetadataUpdated { workspace } => {
                if replace_by_id(
                    &mut self.workspaces,
                    &workspace.workspace_id,
                    |item| &item.workspace_id,
                    workspace.clone(),
                )
                .is_some()
                {
                    return SnapshotUpdate::Resync;
                }
            }
            HerdrEvent::WorkspaceRenamed {
                workspace_id,
                label,
            } => {
                let Some(workspace) = self
                    .workspaces
                    .iter_mut()
                    .find(|workspace| &workspace.workspace_id == workspace_id)
                else {
                    return SnapshotUpdate::Resync;
                };
                workspace.label.clone_from(label);
            }
            HerdrEvent::WorkspaceClosed { workspace_id } => {
                self.drop_workspace(workspace_id);
            }
            HerdrEvent::WorktreeCreated { workspace, .. }
            | HerdrEvent::WorktreeOpened { workspace, .. } => {
                let Some(existing) = self
                    .workspaces
                    .iter_mut()
                    .find(|existing| existing.workspace_id == workspace.workspace_id)
                else {
                    return SnapshotUpdate::Resync;
                };
                existing.worktree.clone_from(&workspace.worktree);
            }
            HerdrEvent::WorktreeRemoved {
                workspace_id,
                worktree,
                ..
            } => {
                let Some(existing) = self
                    .workspaces
                    .iter()
                    .find(|workspace| &workspace.workspace_id == workspace_id)
                else {
                    return SnapshotUpdate::Resync;
                };
                // Herdr keeps the workspace when it already belongs to another
                // checkout by the time git remove finishes.
                if linked_checkout_matches(existing, worktree) {
                    self.drop_workspace(workspace_id);
                }
            }
            HerdrEvent::WorkspaceFocused { workspace_id } => {
                self.focused_workspace_id = Some(workspace_id.clone());
                for workspace in &mut self.workspaces {
                    workspace.focused = &workspace.workspace_id == workspace_id;
                }
            }
            HerdrEvent::WorkspaceMoved { workspaces, .. }
            | HerdrEvent::WorkspaceReordered { workspaces, .. } => {
                self.workspaces.clone_from(workspaces);
            }
            HerdrEvent::TabCreated { tab } => {
                upsert_by_id(
                    &mut self.tabs,
                    &tab.tab_id,
                    |item| &item.tab_id,
                    tab.clone(),
                );
            }
            HerdrEvent::TabClosed {
                workspace_id,
                tab_id,
            } => {
                let tab_was_focused = self.focused_tab_id.as_deref() == Some(tab_id.as_str());
                self.tabs.retain(|tab| &tab.tab_id != tab_id);
                self.panes.retain(|pane| &pane.tab_id != tab_id);
                self.layouts.retain(|layout| &layout.tab_id != tab_id);
                self.forget_missing_focus();
                if self.tab_close_needs_resync(workspace_id, tab_was_focused) {
                    return SnapshotUpdate::Resync;
                }
            }
            HerdrEvent::TabRenamed { tab_id, label, .. } => {
                let Some(tab) = self.tabs.iter_mut().find(|tab| &tab.tab_id == tab_id) else {
                    return SnapshotUpdate::Resync;
                };
                tab.label.clone_from(label);
            }
            HerdrEvent::TabFocused { tab_id, .. } => {
                self.focused_tab_id = Some(tab_id.clone());
                for tab in &mut self.tabs {
                    tab.focused = &tab.tab_id == tab_id;
                }
            }
            HerdrEvent::TabMoved {
                workspace_id, tabs, ..
            } => {
                self.tabs.retain(|tab| &tab.workspace_id != workspace_id);
                self.tabs.extend(tabs.iter().cloned());
            }
            HerdrEvent::PaneCreated { pane } => {
                upsert_by_id(
                    &mut self.panes,
                    &pane.pane_id,
                    |item| &item.pane_id,
                    pane.clone(),
                );
            }
            HerdrEvent::PaneUpdated { pane } => {
                let Some(existing) = self
                    .panes
                    .iter_mut()
                    .find(|item| item.pane_id == pane.pane_id)
                else {
                    return SnapshotUpdate::Resync;
                };
                // Agent lifecycle fields are owned by PaneAgentStatusChanged /
                // PaneAgentDetected (and the initial snapshot / PaneCreated).
                // pane.updated's last event can carry a stale Working.
                let agent_status = existing.agent_status;
                let agent = existing.agent.clone();
                let title = existing.title.clone();
                let display_agent = existing.display_agent.clone();
                let state_labels = existing.state_labels.clone();
                *existing = pane.clone();
                existing.agent_status = agent_status;
                existing.agent = agent;
                existing.title = title;
                existing.display_agent = display_agent;
                existing.state_labels = state_labels;
            }
            HerdrEvent::PaneClosed {
                pane_id,
                workspace_id,
            }
            | HerdrEvent::PaneExited {
                pane_id,
                workspace_id,
            } => {
                let tab_id = self
                    .panes
                    .iter()
                    .find(|pane| &pane.pane_id == pane_id)
                    .map(|pane| pane.tab_id.clone());
                self.panes.retain(|pane| &pane.pane_id != pane_id);
                // Herdr deletes the tab when its last pane closes, but only emits
                // pane.closed / pane.exited — not tab.closed.
                if let Some(tab_id) = tab_id
                    && !self.panes.iter().any(|pane| pane.tab_id == tab_id)
                {
                    let tab_was_focused = self.focused_tab_id.as_deref() == Some(tab_id.as_str());
                    self.tabs.retain(|tab| tab.tab_id != tab_id);
                    self.layouts.retain(|layout| layout.tab_id != tab_id);
                    self.forget_missing_focus();
                    if self.tab_close_needs_resync(workspace_id, tab_was_focused) {
                        return SnapshotUpdate::Resync;
                    }
                } else {
                    self.forget_missing_focus();
                }
            }
            HerdrEvent::PaneFocused { pane_id, .. } => {
                self.focused_pane_id = Some(pane_id.clone());
                for pane in &mut self.panes {
                    pane.focused = &pane.pane_id == pane_id;
                }
            }
            HerdrEvent::PaneAgentDetected {
                pane_id,
                agent,
                released,
                final_status,
                ..
            } => {
                let Some(pane) = self.panes.iter_mut().find(|pane| &pane.pane_id == pane_id) else {
                    return SnapshotUpdate::Resync;
                };
                if *released {
                    // Payload still names the old agent; identity is gone.
                    pane.agent = None;
                    pane.display_agent = None;
                    pane.title = None;
                    pane.state_labels.clear();
                    if let Some(status) = final_status {
                        pane.agent_status = *status;
                    }
                } else {
                    pane.agent.clone_from(agent);
                }
            }
            HerdrEvent::PaneAgentStatusChanged {
                pane_id,
                agent_status,
                agent,
                title,
                display_agent,
                state_labels,
                ..
            } => {
                let Some(pane) = self.panes.iter_mut().find(|pane| &pane.pane_id == pane_id) else {
                    return SnapshotUpdate::Resync;
                };
                // Partial defense only. The session subscription and the
                // per-pane status subscription are independent sockets;
                // Herdr does not merge them into one ordered stream. A name
                // mismatch means the two have diverged; resync rather than
                // guess.
                //
                // This cannot tell two grok instances apart on the same pane.
                // Same-kind restart plus cross-subscription reordering can
                // still apply a stale status. A complete fix needs Herdr to
                // expose a globally sequenced aggregate subscription and a
                // snapshot barrier.
                if agent.as_deref() != pane.agent.as_deref() {
                    return SnapshotUpdate::Resync;
                }
                pane.agent_status = *agent_status;
                pane.agent.clone_from(agent);
                pane.title.clone_from(title);
                pane.display_agent.clone_from(display_agent);
                pane.state_labels.clone_from(state_labels);
            }
            HerdrEvent::LayoutUpdated { layout } => {
                if let Some(existing) = self.layouts.iter_mut().find(|existing| {
                    existing.workspace_id == layout.workspace_id && existing.tab_id == layout.tab_id
                }) {
                    existing.clone_from(layout);
                } else {
                    self.layouts.push(layout.clone());
                }
            }
            HerdrEvent::PaneMoved { .. } | HerdrEvent::Unknown => return SnapshotUpdate::Resync,
        }
        SnapshotUpdate::Applied
    }

    fn drop_workspace(&mut self, workspace_id: &str) {
        self.workspaces
            .retain(|workspace| workspace.workspace_id != workspace_id);
        self.tabs.retain(|tab| tab.workspace_id != workspace_id);
        self.panes.retain(|pane| pane.workspace_id != workspace_id);
        self.layouts
            .retain(|layout| layout.workspace_id != workspace_id);
        self.forget_missing_focus();
    }

    fn forget_missing_focus(&mut self) {
        if self.focused_workspace_id.as_ref().is_some_and(|id| {
            !self
                .workspaces
                .iter()
                .any(|workspace| &workspace.workspace_id == id)
        }) {
            self.focused_workspace_id = None;
        }
        if self
            .focused_tab_id
            .as_ref()
            .is_some_and(|id| !self.tabs.iter().any(|tab| &tab.tab_id == id))
        {
            self.focused_tab_id = None;
        }
        if self
            .focused_pane_id
            .as_ref()
            .is_some_and(|id| !self.panes.iter().any(|pane| &pane.pane_id == id))
        {
            self.focused_pane_id = None;
        }
    }

    fn tab_close_needs_resync(&self, workspace_id: &str, tab_was_focused: bool) -> bool {
        tab_was_focused || !self.tabs.iter().any(|tab| tab.workspace_id == workspace_id)
    }
}

fn replace_by_id<T>(
    items: &mut [T],
    id: &str,
    item_id: impl Fn(&T) -> &str,
    replacement: T,
) -> Option<T> {
    let Some(existing) = items.iter_mut().find(|item| item_id(item) == id) else {
        return Some(replacement);
    };
    *existing = replacement;
    None
}

fn upsert_by_id<T>(items: &mut Vec<T>, id: &str, item_id: impl Fn(&T) -> &str, item: T) {
    if let Some(item) = replace_by_id(items, id, item_id, item) {
        items.push(item);
    }
}

fn linked_checkout_matches(workspace: &WorkspaceInfo, removed: &WorktreeInfo) -> bool {
    workspace
        .worktree
        .as_ref()
        .is_some_and(|info| info.is_linked_worktree && info.checkout_path == removed.path)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet, VecDeque};

    use super::*;
    use crate::{AgentStatus, LayoutPane, LayoutRect, WorkspaceWorktreeInfo};

    fn workspace(id: &str, label: &str, focused: bool) -> WorkspaceInfo {
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

    fn tab(id: &str, workspace_id: &str, label: &str, focused: bool) -> TabInfo {
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

    fn pane(id: &str, workspace_id: &str, tab_id: &str, focused: bool) -> PaneInfo {
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

    fn layout(workspace_id: &str, tab_id: &str, pane_id: &str) -> PaneLayout {
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

    fn cascade_snapshot() -> HierarchySnapshot {
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

    fn ids(values: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
        values
            .into_iter()
            .map(|value| value.as_ref().to_owned())
            .collect()
    }

    #[test]
    fn workspace_created_upserts_the_workspace_by_id() {
        let mut snapshot = HierarchySnapshot::default();
        let created = workspace("w2", "two", false);
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceCreated {
                workspace: created.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.workspaces, vec![created.clone()]);

        let mut updated = created;
        updated.label = "two-prime".into();
        updated.pane_count = 3;
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceCreated {
                workspace: updated.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.workspaces, vec![updated]);
    }

    #[test]
    fn workspace_updated_replaces_the_workspace_by_id() {
        let mut snapshot = HierarchySnapshot {
            workspaces: vec![workspace("w1", "old", true)],
            ..Default::default()
        };
        let mut updated = workspace("w1", "new", true);
        updated.pane_count = 4;
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceUpdated {
                workspace: updated.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.workspaces, vec![updated]);
    }

    #[test]
    fn workspace_metadata_updated_replaces_the_workspace_by_id() {
        let mut snapshot = HierarchySnapshot {
            workspaces: vec![workspace("w1", "core", true)],
            ..Default::default()
        };
        let mut updated = workspace("w1", "core", true);
        updated.agent_status = AgentStatus::Working;
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceMetadataUpdated {
                workspace: updated.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.workspaces[0].agent_status, AgentStatus::Working);
    }

    #[test]
    fn workspace_renamed_updates_the_label() {
        let mut snapshot = HierarchySnapshot {
            workspaces: vec![workspace("w1", "old", true)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceRenamed {
                workspace_id: "w1".into(),
                label: "new".into(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.workspaces[0].label, "new");
    }

    #[test]
    fn workspace_closed_removes_the_workspace_and_cascades_tabs_panes_and_layouts() {
        let mut snapshot = cascade_snapshot();
        snapshot.workspaces.push(workspace("w2", "two", false));
        snapshot.tabs.push(tab("t9", "w2", "other", false));
        snapshot.panes.push(pane("p9", "w2", "t9", false));
        snapshot.layouts.push(layout("w2", "t9", "p9"));
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceClosed {
                workspace_id: "w1".into()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot
                .workspaces
                .iter()
                .map(|item| item.workspace_id.as_str())
                .collect::<Vec<_>>(),
            ["w2"]
        );
        assert_eq!(
            snapshot
                .tabs
                .iter()
                .map(|item| item.tab_id.as_str())
                .collect::<Vec<_>>(),
            ["t9"]
        );
        assert_eq!(
            snapshot
                .panes
                .iter()
                .map(|item| item.pane_id.as_str())
                .collect::<Vec<_>>(),
            ["p9"]
        );
        assert_eq!(
            snapshot
                .layouts
                .iter()
                .map(|item| item.tab_id.as_str())
                .collect::<Vec<_>>(),
            ["t9"]
        );
        assert_eq!(snapshot.focused_workspace_id, None);
        assert_eq!(snapshot.focused_tab_id, None);
        assert_eq!(snapshot.focused_pane_id, None);
    }

    #[test]
    fn workspace_focused_updates_focus_flags() {
        let mut snapshot = HierarchySnapshot {
            focused_workspace_id: Some("w1".into()),
            workspaces: vec![workspace("w1", "one", true), workspace("w2", "two", false)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceFocused {
                workspace_id: "w2".into()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.focused_workspace_id.as_deref(), Some("w2"));
        assert!(!snapshot.workspaces[0].focused);
        assert!(snapshot.workspaces[1].focused);
    }

    #[test]
    fn workspace_moved_replaces_the_workspace_list() {
        let mut snapshot = HierarchySnapshot {
            workspaces: vec![workspace("w1", "one", true), workspace("w2", "two", false)],
            ..Default::default()
        };
        let reordered = vec![workspace("w2", "two", false), workspace("w1", "one", true)];
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceMoved {
                workspace_id: "w2".into(),
                insert_index: 0,
                workspaces: reordered.clone(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot
                .workspaces
                .iter()
                .map(|item| item.workspace_id.as_str())
                .collect::<Vec<_>>(),
            ["w2", "w1"]
        );
    }

    #[test]
    fn workspace_reordered_replaces_the_workspace_list_in_payload_order() {
        let mut snapshot = HierarchySnapshot {
            workspaces: vec![
                workspace("w1", "one", true),
                workspace("w2", "two", false),
                workspace("w3", "three", false),
            ],
            ..Default::default()
        };
        let workspaces = vec![
            workspace("w3", "three", false),
            workspace("w1", "one", true),
            workspace("w2", "two", false),
        ];
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceReordered {
                workspace_ids: ids(["w3", "w1", "w2"]),
                workspaces,
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot
                .workspaces
                .iter()
                .map(|item| item.workspace_id.as_str())
                .collect::<Vec<_>>(),
            ["w3", "w1", "w2"]
        );
    }

    #[test]
    fn tab_created_upserts_the_tab_by_id() {
        let mut snapshot = HierarchySnapshot::default();
        let created = tab("t1", "w1", "shell", true);
        assert_eq!(
            snapshot.apply(&HerdrEvent::TabCreated {
                tab: created.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.tabs, vec![created.clone()]);

        let mut updated = created;
        updated.label = "renamed".into();
        updated.focused = false;
        assert_eq!(
            snapshot.apply(&HerdrEvent::TabCreated {
                tab: updated.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.tabs, vec![updated]);
    }

    #[test]
    fn tab_closed_removes_the_tab_and_its_panes_without_touching_sibling_tabs() {
        let mut snapshot = cascade_snapshot();
        snapshot.layouts.push(layout("w1", "t2", "p3"));
        assert_eq!(
            snapshot.apply(&HerdrEvent::TabClosed {
                workspace_id: "w1".into(),
                tab_id: "t2".into(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot
                .tabs
                .iter()
                .map(|item| item.tab_id.as_str())
                .collect::<Vec<_>>(),
            ["t1"]
        );
        assert_eq!(
            snapshot
                .panes
                .iter()
                .map(|item| item.pane_id.as_str())
                .collect::<Vec<_>>(),
            ["p1", "p2"]
        );
        assert_eq!(
            snapshot
                .layouts
                .iter()
                .map(|item| item.tab_id.as_str())
                .collect::<Vec<_>>(),
            ["t1"]
        );
        assert_eq!(snapshot.workspaces[0].workspace_id, "w1");
        assert_eq!(snapshot.focused_tab_id.as_deref(), Some("t1"));
        assert_eq!(snapshot.focused_pane_id.as_deref(), Some("p1"));
        assert_eq!(snapshot.focused_workspace_id.as_deref(), Some("w1"));
    }

    #[test]
    fn tab_renamed_updates_the_label() {
        let mut snapshot = HierarchySnapshot {
            tabs: vec![tab("t1", "w1", "old", true)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::TabRenamed {
                workspace_id: "w1".into(),
                tab_id: "t1".into(),
                label: "new".into(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.tabs[0].label, "new");
    }

    #[test]
    fn tab_focused_updates_focus_flags() {
        let mut snapshot = HierarchySnapshot {
            focused_tab_id: Some("t1".into()),
            tabs: vec![
                tab("t1", "w1", "alpha", true),
                tab("t2", "w1", "beta", false),
            ],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::TabFocused {
                workspace_id: "w1".into(),
                tab_id: "t2".into(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.focused_tab_id.as_deref(), Some("t2"));
        assert!(!snapshot.tabs[0].focused);
        assert!(snapshot.tabs[1].focused);
    }

    #[test]
    fn tab_moved_replaces_that_workspace_tabs_in_payload_order() {
        let mut snapshot = HierarchySnapshot {
            tabs: vec![
                tab("t1", "w1", "alpha", true),
                tab("t2", "w1", "beta", false),
                tab("t9", "w2", "other", false),
            ],
            ..Default::default()
        };
        let mut moved = vec![
            tab("t2", "w1", "beta", false),
            tab("t1", "w1", "alpha", true),
        ];
        moved[0].number = 1;
        moved[1].number = 2;
        assert_eq!(
            snapshot.apply(&HerdrEvent::TabMoved {
                workspace_id: "w1".into(),
                tab_id: "t2".into(),
                insert_index: 0,
                tabs: moved,
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot
                .tabs
                .iter()
                .map(|item| item.tab_id.as_str())
                .collect::<Vec<_>>(),
            ["t9", "t2", "t1"]
        );
    }

    #[test]
    fn pane_created_upserts_the_pane_by_id() {
        let mut snapshot = HierarchySnapshot::default();
        let created = pane("p2", "w1", "t1", false);
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneCreated {
                pane: created.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes, vec![created.clone()]);

        let mut updated = created;
        updated.revision = 4;
        updated.focused = true;
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneCreated {
                pane: updated.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes, vec![updated]);
    }

    #[test]
    fn pane_updated_does_not_overwrite_agent_status_but_still_updates_other_fields() {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        snapshot.panes[0].agent_status = AgentStatus::Done;
        snapshot.panes[0].agent = Some("grok".into());
        snapshot.panes[0].display_agent = Some("grok".into());
        snapshot.panes[0].title = Some("old-title".into());
        snapshot.panes[0].terminal_title = Some("old-term".into());
        snapshot.panes[0].cwd = Some("/old".into());
        snapshot.panes[0].revision = 1;
        snapshot.panes[0]
            .state_labels
            .insert("model".into(), "grok".into());

        let mut updated = snapshot.panes[0].clone();
        updated.agent_status = AgentStatus::Working;
        updated.agent = Some("claude".into());
        updated.display_agent = Some("claude".into());
        updated.title = Some("new-title".into());
        updated.terminal_title = Some("new-term".into());
        updated.cwd = Some("/new".into());
        updated.revision = 9;
        updated.state_labels.insert("model".into(), "claude".into());

        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneUpdated { pane: updated }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes[0].agent_status, AgentStatus::Done);
        assert_eq!(snapshot.panes[0].agent.as_deref(), Some("grok"));
        assert_eq!(snapshot.panes[0].display_agent.as_deref(), Some("grok"));
        assert_eq!(
            snapshot.panes[0]
                .state_labels
                .get("model")
                .map(String::as_str),
            Some("grok")
        );
        assert_eq!(snapshot.panes[0].title.as_deref(), Some("old-title"));
        assert_eq!(
            snapshot.panes[0].terminal_title.as_deref(),
            Some("new-term")
        );
        assert_eq!(snapshot.panes[0].cwd.as_deref(), Some("/new"));
        assert_eq!(snapshot.panes[0].revision, 9);
    }

    #[test]
    fn pane_closed_removes_the_pane() {
        let mut snapshot = HierarchySnapshot {
            focused_pane_id: Some("p1".into()),
            panes: vec![pane("p1", "w1", "t1", true), pane("p2", "w1", "t1", false)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneClosed {
                workspace_id: "w1".into(),
                pane_id: "p1".into(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot
                .panes
                .iter()
                .map(|item| item.pane_id.as_str())
                .collect::<Vec<_>>(),
            ["p2"]
        );
        assert_eq!(snapshot.focused_pane_id, None);
    }

    #[test]
    fn closing_the_only_pane_in_a_tab_does_not_leave_an_empty_tab() {
        let mut snapshot = cascade_snapshot();
        snapshot.layouts.push(layout("w1", "t2", "p3"));
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneClosed {
                workspace_id: "w1".into(),
                pane_id: "p3".into(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot
                .tabs
                .iter()
                .map(|item| item.tab_id.as_str())
                .collect::<Vec<_>>(),
            ["t1"]
        );
        assert_eq!(
            snapshot
                .panes
                .iter()
                .map(|item| item.pane_id.as_str())
                .collect::<Vec<_>>(),
            ["p1", "p2"]
        );
        assert_eq!(
            snapshot
                .layouts
                .iter()
                .map(|item| item.tab_id.as_str())
                .collect::<Vec<_>>(),
            ["t1"]
        );
    }

    #[test]
    fn pane_exited_removes_the_pane() {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true), pane("p2", "w1", "t1", false)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneExited {
                workspace_id: "w1".into(),
                pane_id: "p1".into(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot
                .panes
                .iter()
                .map(|item| item.pane_id.as_str())
                .collect::<Vec<_>>(),
            ["p2"]
        );
    }

    #[test]
    fn exiting_the_last_pane_of_a_workspace_resyncs_instead_of_guessing_whether_to_delete_it() {
        let mut snapshot = HierarchySnapshot {
            focused_workspace_id: Some("w2".into()),
            focused_tab_id: Some("t9".into()),
            focused_pane_id: Some("p9".into()),
            workspaces: vec![workspace("w1", "one", false), workspace("w2", "two", true)],
            tabs: vec![
                tab("t1", "w1", "alpha", false),
                tab("t9", "w2", "other", true),
            ],
            panes: vec![pane("p1", "w1", "t1", false), pane("p9", "w2", "t9", true)],
            layouts: vec![layout("w1", "t1", "p1"), layout("w2", "t9", "p9")],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneExited {
                workspace_id: "w1".into(),
                pane_id: "p1".into(),
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot
                .workspaces
                .iter()
                .map(|item| item.workspace_id.as_str())
                .collect::<Vec<_>>(),
            ["w1", "w2"]
        );
        assert_eq!(
            snapshot
                .tabs
                .iter()
                .map(|item| item.tab_id.as_str())
                .collect::<Vec<_>>(),
            ["t9"]
        );
    }

    #[test]
    fn pane_focused_updates_focus_flags() {
        let mut snapshot = HierarchySnapshot {
            focused_pane_id: Some("p1".into()),
            panes: vec![pane("p1", "w1", "t1", true), pane("p2", "w1", "t1", false)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneFocused {
                workspace_id: "w1".into(),
                pane_id: "p2".into(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.focused_pane_id.as_deref(), Some("p2"));
        assert!(!snapshot.panes[0].focused);
        assert!(snapshot.panes[1].focused);
    }

    #[test]
    fn pane_agent_detected_updates_the_agent() {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneAgentDetected {
                workspace_id: "w1".into(),
                pane_id: "p1".into(),
                agent: Some("claude".into()),
                released: false,
                final_status: None,
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes[0].agent.as_deref(), Some("claude"));
    }

    #[test]
    fn pane_agent_detected_release_clears_presentation_and_applies_final_status() {
        let event: HerdrEvent = serde_json::from_str(
            r#"{"type":"pane_agent_detected","agent":"t21-rel","final_status":"unknown","pane_id":"wC:p6","released":true,"workspace_id":"wC"}"#,
        )
        .unwrap();
        let HerdrEvent::PaneAgentDetected {
            released,
            final_status,
            agent,
            ..
        } = event
        else {
            panic!("expected pane_agent_detected");
        };
        assert!(released);
        assert_eq!(final_status, Some(AgentStatus::Unknown));
        assert_eq!(agent.as_deref(), Some("t21-rel"));

        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        snapshot.panes[0].agent = Some("t21-rel".into());
        snapshot.panes[0].display_agent = Some("t21-rel".into());
        snapshot.panes[0].title = Some("still working".into());
        snapshot.panes[0].agent_status = AgentStatus::Idle;
        snapshot.panes[0]
            .state_labels
            .insert("model".into(), "t21".into());
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneAgentDetected {
                workspace_id: "w1".into(),
                pane_id: "p1".into(),
                agent: Some("t21-rel".into()),
                released: true,
                final_status: Some(AgentStatus::Unknown),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes[0].agent, None);
        assert_eq!(snapshot.panes[0].display_agent, None);
        assert_eq!(snapshot.panes[0].title, None);
        assert!(snapshot.panes[0].state_labels.is_empty());
        assert_eq!(snapshot.panes[0].agent_status, AgentStatus::Unknown);
    }

    fn released_pane(agent: &str, status: AgentStatus) -> HierarchySnapshot {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        snapshot.panes[0].agent = Some(agent.into());
        snapshot.panes[0].display_agent = Some(agent.into());
        snapshot.panes[0].title = Some("still working".into());
        snapshot.panes[0].agent_status = AgentStatus::Idle;
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneAgentDetected {
                workspace_id: "w1".into(),
                pane_id: "p1".into(),
                agent: Some(agent.into()),
                released: true,
                final_status: Some(status),
            }),
            SnapshotUpdate::Applied
        );
        snapshot
    }

    fn status_event(agent: &str, status: AgentStatus) -> HerdrEvent {
        HerdrEvent::PaneAgentStatusChanged {
            pane_id: "p1".into(),
            workspace_id: "w1".into(),
            agent_status: status,
            agent: Some(agent.into()),
            title: Some("still working".into()),
            display_agent: Some(agent.into()),
            state_labels: HashMap::new(),
        }
    }

    #[test]
    fn status_after_release_does_not_restore_the_old_agent_name() {
        let mut snapshot = released_pane("grok", AgentStatus::Unknown);
        assert_eq!(
            snapshot.apply(&status_event("grok", AgentStatus::Unknown)),
            SnapshotUpdate::Resync
        );
        assert_eq!(snapshot.panes[0].agent, None);
        assert_eq!(snapshot.panes[0].display_agent, None);
        assert_eq!(snapshot.panes[0].title, None);
        assert_eq!(snapshot.panes[0].agent_status, AgentStatus::Unknown);
    }

    #[test]
    fn status_for_a_different_agent_resyncs_instead_of_applying() {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        snapshot.panes[0].agent = Some("grok".into());
        snapshot.panes[0].agent_status = AgentStatus::Idle;
        assert_eq!(
            snapshot.apply(&status_event("claude", AgentStatus::Working)),
            SnapshotUpdate::Resync
        );
        assert_eq!(snapshot.panes[0].agent.as_deref(), Some("grok"));
        assert_eq!(snapshot.panes[0].agent_status, AgentStatus::Idle);
    }

    #[test]
    fn same_kind_agent_restarting_on_the_same_pane_takes_the_new_generation() {
        let mut snapshot = released_pane("grok", AgentStatus::Unknown);
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneAgentDetected {
                workspace_id: "w1".into(),
                pane_id: "p1".into(),
                agent: Some("grok".into()),
                released: false,
                final_status: None,
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot.apply(&status_event("grok", AgentStatus::Working)),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes[0].agent.as_deref(), Some("grok"));
        assert_eq!(snapshot.panes[0].agent_status, AgentStatus::Working);
        assert_eq!(snapshot.panes[0].display_agent.as_deref(), Some("grok"));
    }

    #[test]
    fn pane_agent_status_changed_moves_working_to_done_and_updates_presentation() {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true), pane("p2", "w1", "t1", false)],
            ..Default::default()
        };
        snapshot.panes[0].agent_status = AgentStatus::Working;
        snapshot.panes[0].agent = Some("grok".into());
        snapshot.panes[1].agent_status = AgentStatus::Working;
        snapshot.panes[1].display_agent = Some("sibling".into());
        snapshot.panes[1].title = Some("keep-me".into());
        let mut state_labels = HashMap::new();
        state_labels.insert("model".into(), "grok".into());
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneAgentStatusChanged {
                pane_id: "p1".into(),
                workspace_id: "w1".into(),
                agent_status: AgentStatus::Done,
                agent: Some("grok".into()),
                title: Some("finished".into()),
                display_agent: Some("grok".into()),
                state_labels: state_labels.clone(),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes[0].agent_status, AgentStatus::Done);
        assert_eq!(snapshot.panes[0].agent.as_deref(), Some("grok"));
        assert_eq!(snapshot.panes[0].title.as_deref(), Some("finished"));
        assert_eq!(snapshot.panes[0].display_agent.as_deref(), Some("grok"));
        assert_eq!(snapshot.panes[0].state_labels, state_labels);
        assert_eq!(snapshot.panes[1].agent_status, AgentStatus::Working);
        assert_eq!(snapshot.panes[1].display_agent.as_deref(), Some("sibling"));
        assert_eq!(snapshot.panes[1].title.as_deref(), Some("keep-me"));
    }

    #[test]
    fn pane_agent_status_changed_resyncs_when_the_pane_is_missing() {
        let mut snapshot = HierarchySnapshot::default();
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneAgentStatusChanged {
                pane_id: "p1".into(),
                workspace_id: "w1".into(),
                agent_status: AgentStatus::Done,
                agent: None,
                title: None,
                display_agent: None,
                state_labels: HashMap::new(),
            }),
            SnapshotUpdate::Resync
        );
    }

    #[test]
    fn agent_status_stream_rebuilds_only_when_the_pane_set_changes() {
        let snapshot = cascade_snapshot();
        let same = snapshot.pane_ids();
        assert!(!agent_status_stream_should_rebuild(&same, &snapshot));
        let mut extra = same.clone();
        extra.insert("p-new".into());
        assert!(agent_status_stream_should_rebuild(&extra, &snapshot));
        assert!(agent_status_stream_should_rebuild(
            &HashSet::new(),
            &snapshot
        ));
        let empty = HierarchySnapshot::default();
        assert!(!agent_status_stream_should_rebuild(&HashSet::new(), &empty));
    }

    #[test]
    fn a_failed_subscribe_rolls_back_so_the_next_ensure_still_rebuilds() {
        let previous: HashSet<String> = ["p1".into()].into_iter().collect();
        let attempted: HashSet<String> = ["p1".into(), "p2".into()].into_iter().collect();
        let rolled_back = event_panes_after_failed_subscribe(&attempted, &attempted, &previous);
        assert_eq!(rolled_back, previous);
        let mut snapshot = cascade_snapshot();
        snapshot.panes.push(pane("p-new", "w1", "t1", false));
        assert!(agent_status_stream_should_rebuild(&rolled_back, &snapshot));

        let superseded: HashSet<String> = ["p1".into(), "p2".into(), "p3".into()]
            .into_iter()
            .collect();
        assert_eq!(
            event_panes_after_failed_subscribe(&superseded, &attempted, &previous),
            superseded
        );
    }

    #[test]
    fn a_dead_agent_status_stream_forgets_its_panes_so_the_next_ensure_rebuilds() {
        let snapshot = cascade_snapshot();
        let live = snapshot.pane_ids();
        assert!(!agent_status_stream_should_rebuild(&live, &snapshot));
        let forgotten = agent_status_panes_after_stream_closed();
        assert!(agent_status_stream_should_rebuild(&forgotten, &snapshot));
        let empty = HierarchySnapshot::default();
        assert!(!agent_status_stream_should_rebuild(&forgotten, &empty));
    }

    #[test]
    fn handoff_replays_buffered_status_after_the_snapshot_is_installed() {
        let mut pending = VecDeque::new();
        assert!(!agent_status_handoff_push(
            &mut pending,
            [status_event("grok", AgentStatus::Done)],
            AGENT_STATUS_HANDOFF_LIMIT,
        ));

        let mut installed = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        installed.panes[0].agent = Some("grok".into());
        installed.panes[0].agent_status = AgentStatus::Working;
        for event in agent_status_handoff_take(&mut pending) {
            assert_eq!(installed.apply(&event), SnapshotUpdate::Applied);
        }
        assert_eq!(installed.panes[0].agent_status, AgentStatus::Done);
        assert!(pending.is_empty());
    }

    #[test]
    fn handoff_buffer_keeps_the_newest_events_when_over_limit() {
        let mut pending = VecDeque::new();
        assert!(agent_status_handoff_push(
            &mut pending,
            [
                status_event("grok", AgentStatus::Working),
                status_event("grok", AgentStatus::Idle),
                status_event("grok", AgentStatus::Done),
            ],
            2,
        ));
        let replayed = agent_status_handoff_take(&mut pending);
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0], status_event("grok", AgentStatus::Idle));
        assert_eq!(replayed[1], status_event("grok", AgentStatus::Done));
    }

    #[test]
    fn handoff_snapshot_failure_still_applies_the_buffered_events() {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        snapshot.panes[0].agent = Some("grok".into());
        snapshot.panes[0].agent_status = AgentStatus::Working;
        let mut pending = VecDeque::new();
        agent_status_handoff_push(
            &mut pending,
            [status_event("grok", AgentStatus::Done)],
            AGENT_STATUS_HANDOFF_LIMIT,
        );
        for event in agent_status_handoff_take(&mut pending) {
            assert_eq!(snapshot.apply(&event), SnapshotUpdate::Applied);
        }
        assert_eq!(snapshot.panes[0].agent_status, AgentStatus::Done);
        assert!(pending.is_empty());
    }

    #[test]
    fn handoff_payload_error_requests_resync_after_release() {
        let mut handoff = AgentStatusHandoff::new();
        handoff.push(
            [status_event("grok", AgentStatus::Done)],
            AGENT_STATUS_HANDOFF_LIMIT,
        );
        handoff.note_payload_error();
        let (events, resync_after) = handoff.into_release();
        assert_eq!(events, vec![status_event("grok", AgentStatus::Done)]);
        assert!(resync_after);
    }

    #[test]
    fn in_place_updates_resync_when_the_target_is_missing() {
        let mut snapshot = HierarchySnapshot::default();
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceUpdated {
                workspace: workspace("w1", "one", true)
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceMetadataUpdated {
                workspace: workspace("w1", "one", true)
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorkspaceRenamed {
                workspace_id: "w1".into(),
                label: "new".into(),
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot.apply(&HerdrEvent::TabRenamed {
                workspace_id: "w1".into(),
                tab_id: "t1".into(),
                label: "new".into(),
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneUpdated {
                pane: pane("p1", "w1", "t1", true)
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneAgentDetected {
                workspace_id: "w1".into(),
                pane_id: "p1".into(),
                agent: Some("claude".into()),
                released: false,
                final_status: None,
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneAgentStatusChanged {
                pane_id: "p1".into(),
                workspace_id: "w1".into(),
                agent_status: AgentStatus::Done,
                agent: None,
                title: None,
                display_agent: None,
                state_labels: HashMap::new(),
            }),
            SnapshotUpdate::Resync
        );
        assert!(snapshot.workspaces.is_empty());
        assert!(snapshot.tabs.is_empty());
        assert!(snapshot.panes.is_empty());
    }

    #[test]
    fn layout_updated_replaces_or_inserts_the_layout_for_that_tab() {
        let mut snapshot = HierarchySnapshot::default();
        let first = layout("w1", "t1", "p1");
        assert_eq!(
            snapshot.apply(&HerdrEvent::LayoutUpdated {
                layout: first.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.layouts, vec![first]);
        let mut second = layout("w1", "t1", "p1");
        second.zoomed = true;
        second.area.width = 120;
        assert_eq!(
            snapshot.apply(&HerdrEvent::LayoutUpdated {
                layout: second.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.layouts, vec![second]);
    }

    #[test]
    fn pane_moved_requests_a_resync() {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneMoved {
                pane: pane("p1", "w2", "t2", true),
                previous_pane_id: "p1".into(),
                previous_workspace_id: "w1".into(),
                previous_tab_id: "t1".into(),
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(snapshot.panes[0].workspace_id, "w1");
    }

    #[test]
    fn unknown_events_request_a_resync() {
        let mut snapshot = HierarchySnapshot::default();
        assert_eq!(snapshot.apply(&HerdrEvent::Unknown), SnapshotUpdate::Resync);
    }

    #[test]
    fn unknown_event_types_deserialize_as_unknown() {
        let event: HerdrEvent =
            serde_json::from_str(r#"{"type":"some_future_event","whatever":1}"#).unwrap();
        assert_eq!(event, HerdrEvent::Unknown);
    }

    #[test]
    fn captured_herdr_payloads_deserialize() {
        let pane_updated = serde_json::json!({
            "event": "pane_updated",
            "data": {
                "type": "pane_updated",
                "pane": {
                    "pane_id": "w6:p3",
                    "terminal_id": "term-6-3",
                    "workspace_id": "w6",
                    "tab_id": "w6:t1",
                    "focused": true,
                    "cwd": "/Users/sleepstars/code",
                    "terminal_title": "claude",
                    "terminal_title_stripped": "claude",
                    "agent": "claude",
                    "display_agent": "claude",
                    "agent_status": "working",
                    "revision": 1482,
                    "scroll": {
                        "offset_from_bottom": 0,
                        "max_offset_from_bottom": 240,
                        "viewport_rows": 38
                    }
                }
            }
        });
        let layout_updated = serde_json::json!({
            "event": "layout_updated",
            "data": {
                "type": "layout_updated",
                "layout": {
                    "workspace_id": "w6",
                    "tab_id": "w6:t1",
                    "zoomed": false,
                    "area": { "x": 0, "y": 0, "width": 120, "height": 40 },
                    "focused_pane_id": "w6:p3",
                    "panes": [{
                        "pane_id": "w6:p3",
                        "focused": true,
                        "rect": { "x": 0, "y": 0, "width": 120, "height": 40 }
                    }],
                    "splits": []
                }
            }
        });
        let workspace_closed = serde_json::json!({
            "event": "workspace_closed",
            "data": {
                "type": "workspace_closed",
                "workspace_id": "w8",
                "workspace": {
                    "workspace_id": "w8",
                    "number": 3,
                    "label": "notes",
                    "focused": false,
                    "pane_count": 1,
                    "tab_count": 1,
                    "active_tab_id": "w8:t1",
                    "agent_status": "idle"
                }
            }
        });

        let HerdrEvent::PaneUpdated { pane } =
            serde_json::from_value(pane_updated["data"].clone()).unwrap()
        else {
            panic!("expected pane_updated");
        };
        assert_eq!(pane.pane_id, "w6:p3");
        assert_eq!(pane.agent_status, AgentStatus::Working);
        assert_eq!(pane.terminal_title.as_deref(), Some("claude"));
        assert_eq!(pane.revision, 1482);

        let HerdrEvent::LayoutUpdated { layout } =
            serde_json::from_value(layout_updated["data"].clone()).unwrap()
        else {
            panic!("expected layout_updated");
        };
        assert_eq!(layout.tab_id, "w6:t1");
        assert_eq!(layout.focused_pane_id, "w6:p3");
        assert_eq!(layout.area.width, 120);

        let HerdrEvent::WorkspaceClosed { workspace_id } =
            serde_json::from_value(workspace_closed["data"].clone()).unwrap()
        else {
            panic!("expected workspace_closed");
        };
        assert_eq!(workspace_id, "w8");
    }

    fn linked_worktree_info() -> WorkspaceWorktreeInfo {
        WorkspaceWorktreeInfo {
            repo_key: "/repo/.git".into(),
            repo_name: "repo".into(),
            repo_root: "/repo".into(),
            checkout_path: "/worktrees/repo/feature".into(),
            is_linked_worktree: true,
        }
    }

    fn git_worktree(open_workspace_id: Option<&str>) -> WorktreeInfo {
        WorktreeInfo {
            path: "/worktrees/repo/feature".into(),
            branch: Some("worktree/feature".into()),
            is_bare: false,
            is_detached: false,
            is_prunable: false,
            is_linked_worktree: true,
            open_workspace_id: open_workspace_id.map(str::to_owned),
            label: "repo".into(),
        }
    }

    fn workspace_with_worktree(id: &str, label: &str) -> WorkspaceInfo {
        let mut workspace = workspace(id, label, true);
        workspace.worktree = Some(linked_worktree_info());
        workspace
    }

    #[test]
    fn worktree_created_and_opened_set_workspace_worktree_from_the_event() {
        let mut snapshot = HierarchySnapshot {
            workspaces: vec![workspace("w1", "one", true)],
            ..Default::default()
        };
        let incoming = workspace_with_worktree("w1", "one");
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorktreeCreated {
                workspace: incoming.clone(),
                worktree: git_worktree(Some("w1")),
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot.workspaces[0].worktree.as_ref(),
            Some(&linked_worktree_info())
        );

        snapshot.workspaces[0].worktree = None;
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorktreeOpened {
                workspace: incoming,
                worktree: git_worktree(Some("w1")),
                already_open: true,
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(
            snapshot.workspaces[0].worktree.as_ref(),
            Some(&linked_worktree_info())
        );
    }

    #[test]
    fn worktree_removed_drops_the_workspace() {
        let mut snapshot = HierarchySnapshot {
            workspaces: vec![workspace_with_worktree("w1", "one")],
            tabs: vec![tab("w1:t1", "w1", "1", true)],
            panes: vec![pane("w1:p1", "w1", "w1:t1", true)],
            focused_workspace_id: Some("w1".into()),
            focused_tab_id: Some("w1:t1".into()),
            focused_pane_id: Some("w1:p1".into()),
            ..Default::default()
        };
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorktreeRemoved {
                workspace_id: "w1".into(),
                workspace: Some(workspace_with_worktree("w1", "one")),
                worktree: git_worktree(None),
                forced: false,
            }),
            SnapshotUpdate::Applied
        );
        assert!(snapshot.workspaces.is_empty());
        assert!(snapshot.tabs.is_empty());
        assert!(snapshot.panes.is_empty());
        assert_eq!(snapshot.focused_workspace_id, None);
    }

    #[test]
    fn worktree_removed_keeps_a_workspace_that_moved_to_another_checkout() {
        let mut current = workspace_with_worktree("w1", "one");
        current.worktree.as_mut().unwrap().checkout_path = "/repo/other".into();
        let mut snapshot = HierarchySnapshot {
            workspaces: vec![current],
            tabs: vec![tab("w1:t1", "w1", "1", true)],
            panes: vec![pane("w1:p1", "w1", "w1:t1", true)],
            focused_workspace_id: Some("w1".into()),
            ..Default::default()
        };
        let mut removed = git_worktree(None);
        removed.path = "/repo/herdr-issue".into();
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorktreeRemoved {
                workspace_id: "w1".into(),
                workspace: Some(workspace_with_worktree("w1", "one")),
                worktree: removed,
                forced: true,
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.workspaces.len(), 1);
        assert_eq!(
            snapshot.workspaces[0]
                .worktree
                .as_ref()
                .map(|info| info.checkout_path.as_str()),
            Some("/repo/other")
        );
        assert_eq!(snapshot.tabs.len(), 1);
        assert_eq!(snapshot.panes.len(), 1);
        assert_eq!(snapshot.focused_workspace_id.as_deref(), Some("w1"));
    }

    #[test]
    fn worktree_events_resync_when_the_workspace_is_missing() {
        let mut snapshot = HierarchySnapshot::default();
        let incoming = workspace_with_worktree("w1", "one");
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorktreeCreated {
                workspace: incoming.clone(),
                worktree: git_worktree(Some("w1")),
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorktreeOpened {
                workspace: incoming.clone(),
                worktree: git_worktree(Some("w1")),
                already_open: false,
            }),
            SnapshotUpdate::Resync
        );
        assert_eq!(
            snapshot.apply(&HerdrEvent::WorktreeRemoved {
                workspace_id: "w1".into(),
                workspace: Some(incoming),
                worktree: git_worktree(None),
                forced: false,
            }),
            SnapshotUpdate::Resync
        );
        assert!(snapshot.workspaces.is_empty());
    }

    #[test]
    fn captured_worktree_payloads_deserialize() {
        let created: HerdrEvent = serde_json::from_value(serde_json::json!({
            "type": "worktree_created",
            "workspace": {
                "workspace_id": "wN",
                "number": 3,
                "label": "t16-probe",
                "focused": false,
                "pane_count": 1,
                "tab_count": 1,
                "active_tab_id": "wN:t1",
                "agent_status": "unknown",
                "worktree": {
                    "repo_key": "/repo/.git",
                    "repo_name": "repo",
                    "repo_root": "/repo",
                    "checkout_path": "/worktrees/repo/t16-probe",
                    "is_linked_worktree": true
                }
            },
            "worktree": {
                "path": "/worktrees/repo/t16-probe",
                "branch": "worktree/t16-probe",
                "is_bare": false,
                "is_detached": false,
                "is_prunable": false,
                "is_linked_worktree": true,
                "open_workspace_id": "wN",
                "label": "repo"
            }
        }))
        .unwrap();
        let HerdrEvent::WorktreeCreated {
            workspace,
            worktree,
        } = created
        else {
            panic!("expected worktree_created");
        };
        assert_eq!(workspace.workspace_id, "wN");
        assert_eq!(
            workspace
                .worktree
                .as_ref()
                .map(|info| info.checkout_path.as_str()),
            Some("/worktrees/repo/t16-probe")
        );
        assert_eq!(worktree.branch.as_deref(), Some("worktree/t16-probe"));

        let opened: HerdrEvent = serde_json::from_value(serde_json::json!({
            "type": "worktree_opened",
            "already_open": true,
            "workspace": {
                "workspace_id": "wN",
                "number": 3,
                "label": "t16-probe",
                "focused": false,
                "pane_count": 1,
                "tab_count": 1,
                "active_tab_id": "wN:t1",
                "agent_status": "unknown",
                "worktree": {
                    "repo_key": "/repo/.git",
                    "repo_name": "repo",
                    "repo_root": "/repo",
                    "checkout_path": "/worktrees/repo/t16-probe",
                    "is_linked_worktree": true
                }
            },
            "worktree": {
                "path": "/worktrees/repo/t16-probe",
                "branch": "worktree/t16-probe",
                "is_bare": false,
                "is_detached": false,
                "is_prunable": false,
                "is_linked_worktree": true,
                "open_workspace_id": "wN",
                "label": "repo"
            }
        }))
        .unwrap();
        let HerdrEvent::WorktreeOpened { already_open, .. } = opened else {
            panic!("expected worktree_opened");
        };
        assert!(already_open);

        let removed: HerdrEvent = serde_json::from_value(serde_json::json!({
            "type": "worktree_removed",
            "forced": false,
            "workspace_id": "wN",
            "workspace": {
                "workspace_id": "wN",
                "number": 3,
                "label": "t16-probe",
                "focused": false,
                "pane_count": 1,
                "tab_count": 1,
                "active_tab_id": "wN:t1",
                "agent_status": "unknown",
                "worktree": {
                    "repo_key": "/repo/.git",
                    "repo_name": "repo",
                    "repo_root": "/repo",
                    "checkout_path": "/worktrees/repo/t16-probe",
                    "is_linked_worktree": true
                }
            },
            "worktree": {
                "path": "/worktrees/repo/t16-probe",
                "branch": "worktree/t16-probe",
                "is_bare": false,
                "is_detached": false,
                "is_prunable": false,
                "is_linked_worktree": true,
                "label": "repo"
            }
        }))
        .unwrap();
        let HerdrEvent::WorktreeRemoved {
            workspace_id,
            forced,
            worktree,
            ..
        } = removed
        else {
            panic!("expected worktree_removed");
        };
        assert_eq!(workspace_id, "wN");
        assert!(!forced);
        assert_eq!(worktree.open_workspace_id, None);
    }
}
