use super::*;

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
fn pane_agent_status_changed_refreshes_tab_and_workspace_attention_priority() {
    let mut snapshot = cascade_snapshot();
    snapshot.panes[0].agent = Some("grok".into());
    snapshot.panes[0].agent_status = AgentStatus::Working;
    snapshot.panes[1].agent_status = AgentStatus::Done;
    snapshot.panes[2].agent_status = AgentStatus::Idle;

    assert_eq!(
        snapshot.apply(&status_event("grok", AgentStatus::Working)),
        SnapshotUpdate::Applied
    );
    assert_eq!(snapshot.tabs[0].agent_status, AgentStatus::Done);
    assert_eq!(snapshot.tabs[1].agent_status, AgentStatus::Idle);
    assert_eq!(snapshot.workspaces[0].agent_status, AgentStatus::Done);

    assert_eq!(
        snapshot.apply(&status_event("grok", AgentStatus::Blocked)),
        SnapshotUpdate::Applied
    );
    assert_eq!(snapshot.tabs[0].agent_status, AgentStatus::Blocked);
    assert_eq!(snapshot.workspaces[0].agent_status, AgentStatus::Blocked);
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
