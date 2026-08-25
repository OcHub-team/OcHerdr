use super::*;

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
