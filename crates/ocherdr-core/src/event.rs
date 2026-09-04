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
    /// Herdr's `pane.moved`. Emitted after `tab.closed` / `workspace.closed`
    /// / `workspace.created` / `tab.created` for the same move and before the
    /// `layout.updated` events for the source and target tabs.
    PaneMoved {
        pane: PaneInfo,
        previous_pane_id: String,
        previous_workspace_id: String,
        previous_tab_id: String,
        #[serde(default)]
        created_workspace: Option<Box<WorkspaceInfo>>,
        #[serde(default)]
        created_tab: Option<Box<TabInfo>>,
        #[serde(default)]
        closed_workspace_id: Option<String>,
        #[serde(default)]
        closed_tab_id: Option<String>,
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

/// Borrowed view of a `pane.moved` payload for `apply_pane_moved`.
struct PaneMovedEvent<'a> {
    pane: &'a PaneInfo,
    previous_pane_id: &'a str,
    previous_workspace_id: &'a str,
    previous_tab_id: &'a str,
    created_workspace: Option<&'a WorkspaceInfo>,
    created_tab: Option<&'a TabInfo>,
    closed_workspace_id: Option<&'a str>,
    closed_tab_id: Option<&'a str>,
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
        let refresh_agent_aggregates = matches!(
            event,
            HerdrEvent::PaneCreated { .. }
                | HerdrEvent::PaneUpdated { .. }
                | HerdrEvent::PaneClosed { .. }
                | HerdrEvent::PaneExited { .. }
                | HerdrEvent::PaneAgentDetected { .. }
                | HerdrEvent::PaneAgentStatusChanged { .. }
        );
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
                self.drop_tab(tab_id);
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
                // Herdr's global focused pane is, by definition, the layout
                // focus of the active tab. A pane.move does not emit
                // pane.focused, so the source tab's layout.updated is the
                // only carrier of where its focus went.
                if self.focused_tab_id.as_deref() == Some(layout.tab_id.as_str()) {
                    self.focused_pane_id = Some(layout.focused_pane_id.clone());
                    for pane in &mut self.panes {
                        if pane.tab_id == layout.tab_id {
                            pane.focused = pane.pane_id == layout.focused_pane_id;
                        }
                    }
                }
            }
            HerdrEvent::PaneMoved {
                pane,
                previous_pane_id,
                previous_workspace_id,
                previous_tab_id,
                created_workspace,
                created_tab,
                closed_workspace_id,
                closed_tab_id,
            } => {
                let update = self.apply_pane_moved(PaneMovedEvent {
                    pane,
                    previous_pane_id,
                    previous_workspace_id,
                    previous_tab_id,
                    created_workspace: created_workspace.as_deref(),
                    created_tab: created_tab.as_deref(),
                    closed_workspace_id: closed_workspace_id.as_deref(),
                    closed_tab_id: closed_tab_id.as_deref(),
                });
                if update == SnapshotUpdate::Applied {
                    self.refresh_agent_aggregates();
                }
                return update;
            }
            HerdrEvent::Unknown => return SnapshotUpdate::Resync,
        }
        if refresh_agent_aggregates {
            self.refresh_agent_aggregates();
        }
        SnapshotUpdate::Applied
    }

    /// Keep the snapshot's tab/workspace summaries in lockstep with the live
    /// pane status stream. Herdr computes the same priority from terminal
    /// state; OcHerdr repeats it locally because pane status events do not
    /// include refreshed parent records.
    fn refresh_agent_aggregates(&mut self) {
        for tab in &mut self.tabs {
            tab.agent_status = aggregate_agent_status(
                self.panes
                    .iter()
                    .filter(|pane| pane.tab_id == tab.tab_id)
                    .map(|pane| pane.agent_status),
            );
        }
        for workspace in &mut self.workspaces {
            workspace.agent_status = aggregate_agent_status(
                self.panes
                    .iter()
                    .filter(|pane| pane.workspace_id == workspace.workspace_id)
                    .map(|pane| pane.agent_status),
            );
        }
    }

    /// Incremental `pane.moved` (design §6.1). Records only: the pane moves
    /// tab/workspace, created records are inserted, closed ones dropped,
    /// counts and focus flags follow. Layouts are left to the `layout.updated`
    /// events Herdr sends right after; nothing is derived here.
    fn apply_pane_moved(&mut self, event: PaneMovedEvent<'_>) -> SnapshotUpdate {
        let PaneMovedEvent {
            pane,
            previous_pane_id,
            previous_workspace_id,
            previous_tab_id,
            created_workspace,
            created_tab,
            closed_workspace_id,
            closed_tab_id,
        } = event;

        // Herdr emits tab.closed / workspace.closed before pane.moved, and
        // those handlers cascade over the tab's panes. A missing source pane
        // is only a contradiction when nothing in the event explains it.
        let existing_index = self
            .panes
            .iter()
            .position(|item| item.pane_id == previous_pane_id);
        match existing_index {
            Some(index) => {
                let existing = &self.panes[index];
                if existing.tab_id != previous_tab_id
                    || existing.workspace_id != previous_workspace_id
                {
                    return SnapshotUpdate::Resync;
                }
            }
            None => {
                let tab_cascade = closed_tab_id == Some(previous_tab_id)
                    && !self.tabs.iter().any(|tab| tab.tab_id == previous_tab_id);
                let workspace_cascade = closed_workspace_id == Some(previous_workspace_id)
                    && !self
                        .workspaces
                        .iter()
                        .any(|workspace| workspace.workspace_id == previous_workspace_id);
                if !tab_cascade && !workspace_cascade {
                    return SnapshotUpdate::Resync;
                }
            }
        }

        if let Some(workspace) = created_workspace {
            upsert_by_id(
                &mut self.workspaces,
                &workspace.workspace_id,
                |item| &item.workspace_id,
                workspace.clone(),
            );
        }
        if let Some(tab) = created_tab {
            upsert_by_id(
                &mut self.tabs,
                &tab.tab_id,
                |item| &item.tab_id,
                tab.clone(),
            );
        }
        let target_workspace_known = self
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == pane.workspace_id);
        let target_tab_known = self
            .tabs
            .iter()
            .any(|tab| tab.tab_id == pane.tab_id && tab.workspace_id == pane.workspace_id);
        if !target_workspace_known || !target_tab_known {
            return SnapshotUpdate::Resync;
        }

        // Move the record. Agent lifecycle fields stay owned by the
        // agent-status stream, as in PaneUpdated.
        let mut record = pane.clone();
        if let Some(index) = existing_index {
            let existing = self.panes.remove(index);
            record.agent_status = existing.agent_status;
            record.agent = existing.agent;
            record.title = existing.title;
            record.display_agent = existing.display_agent;
            record.state_labels = existing.state_labels;
        }
        let insert_at = self
            .panes
            .iter()
            .rposition(|item| item.tab_id == pane.tab_id)
            .map_or(self.panes.len(), |index| index + 1);
        self.panes.insert(insert_at, record);

        // Counts. Records Herdr created in this move already carry the
        // post-move numbers; only pre-existing records need adjusting.
        let target_tab_created = created_tab.is_some_and(|tab| tab.tab_id == pane.tab_id);
        let target_workspace_created =
            created_workspace.is_some_and(|workspace| workspace.workspace_id == pane.workspace_id);
        if existing_index.is_some()
            && let Some(source_tab) = self
                .tabs
                .iter_mut()
                .find(|tab| tab.tab_id == previous_tab_id)
        {
            source_tab.pane_count = source_tab.pane_count.saturating_sub(1);
        }
        if !target_tab_created
            && let Some(target_tab) = self.tabs.iter_mut().find(|tab| tab.tab_id == pane.tab_id)
        {
            target_tab.pane_count += 1;
        }
        // Workspace pane counts move only across workspaces — or when the
        // cascade already debited the source (the record was gone), in which
        // case the target must be credited even within one workspace.
        let cross_workspace = previous_workspace_id != pane.workspace_id;
        if cross_workspace
            && existing_index.is_some()
            && let Some(source) = self
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.workspace_id == previous_workspace_id)
        {
            source.pane_count = source.pane_count.saturating_sub(1);
        }
        if (cross_workspace || existing_index.is_none())
            && !target_workspace_created
            && let Some(target) = self
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.workspace_id == pane.workspace_id)
        {
            target.pane_count += 1;
        }
        if created_tab.is_some()
            && !target_workspace_created
            && let Some(target) = self
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.workspace_id == pane.workspace_id)
        {
            target.tab_count += 1;
        }

        // Drop what the move closed (no-ops when the cascade already did).
        if let Some(tab_id) = closed_tab_id {
            self.drop_tab(tab_id);
        }
        if let Some(workspace_id) = closed_workspace_id {
            self.drop_workspace(workspace_id);
        }

        // Focus. Herdr switches to the target tab without emitting focus
        // events; `pane.focused` is the global focus after the move.
        if pane.focused {
            self.focused_workspace_id = Some(pane.workspace_id.clone());
            self.focused_tab_id = Some(pane.tab_id.clone());
            self.focused_pane_id = Some(pane.pane_id.clone());
            for workspace in &mut self.workspaces {
                workspace.focused = workspace.workspace_id == pane.workspace_id;
                if workspace.focused {
                    workspace.active_tab_id.clone_from(&pane.tab_id);
                }
            }
            for tab in &mut self.tabs {
                tab.focused = tab.tab_id == pane.tab_id;
            }
            for item in &mut self.panes {
                item.focused = item.pane_id == pane.pane_id;
            }
        } else if self.focused_pane_id.as_deref() == Some(previous_pane_id)
            || self.focused_pane_id.as_deref() == Some(pane.pane_id.as_str())
        {
            // The focused pane left the active tab; the source tab's
            // layout.updated says who has focus there now.
            self.focused_pane_id = None;
        }
        self.forget_missing_focus();
        SnapshotUpdate::Applied
    }

    fn drop_tab(&mut self, tab_id: &str) {
        let Some(index) = self.tabs.iter().position(|tab| tab.tab_id == tab_id) else {
            return;
        };
        let tab = self.tabs.remove(index);
        let before = self.panes.len();
        self.panes.retain(|pane| pane.tab_id != tab_id);
        let removed_panes = before - self.panes.len();
        self.layouts.retain(|layout| layout.tab_id != tab_id);
        if let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.workspace_id == tab.workspace_id)
        {
            workspace.tab_count = workspace.tab_count.saturating_sub(1);
            workspace.pane_count = workspace.pane_count.saturating_sub(removed_panes);
        }
        self.forget_missing_focus();
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

fn aggregate_agent_status(statuses: impl Iterator<Item = AgentStatus>) -> AgentStatus {
    statuses
        .max_by_key(|status| match status {
            AgentStatus::Blocked => 4,
            AgentStatus::Done => 3,
            AgentStatus::Working => 2,
            AgentStatus::Idle => 1,
            AgentStatus::Unknown => 0,
        })
        .unwrap_or(AgentStatus::Unknown)
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
#[path = "event/tests.rs"]
mod tests;
