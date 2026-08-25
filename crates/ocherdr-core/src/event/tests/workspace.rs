use super::*;

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
