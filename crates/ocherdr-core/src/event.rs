//! Typed Herdr event payloads and incremental snapshot updates.

use serde::Deserialize;

use super::{HierarchySnapshot, PaneInfo, PaneLayout, TabInfo, WorkspaceInfo, WorktreeInfo};

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
    },
    LayoutUpdated {
        layout: PaneLayout,
    },
    #[serde(other)]
    Unknown,
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
                if replace_by_id(
                    &mut self.panes,
                    &pane.pane_id,
                    |item| &item.pane_id,
                    pane.clone(),
                )
                .is_some()
                {
                    return SnapshotUpdate::Resync;
                }
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
            HerdrEvent::PaneAgentDetected { pane_id, agent, .. } => {
                let Some(pane) = self.panes.iter_mut().find(|pane| &pane.pane_id == pane_id) else {
                    return SnapshotUpdate::Resync;
                };
                pane.agent.clone_from(agent);
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
    use std::collections::HashMap;

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
    fn pane_updated_replaces_the_pane_by_id() {
        let mut snapshot = HierarchySnapshot {
            panes: vec![pane("p1", "w1", "t1", true)],
            ..Default::default()
        };
        let mut updated = pane("p1", "w1", "t1", true);
        updated.revision = 9;
        updated.agent_status = AgentStatus::Working;
        updated.terminal_title = Some("claude".into());
        assert_eq!(
            snapshot.apply(&HerdrEvent::PaneUpdated {
                pane: updated.clone()
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes, vec![updated]);
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
            }),
            SnapshotUpdate::Applied
        );
        assert_eq!(snapshot.panes[0].agent.as_deref(), Some("claude"));
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
