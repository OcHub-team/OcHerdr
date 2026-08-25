use super::*;

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

/// `[first | second]` at 0.5 over the 80x24 test area.
fn split_layout(
    workspace_id: &str,
    tab_id: &str,
    first: &str,
    second: &str,
    focused: &str,
) -> PaneLayout {
    let rect = |x, width| LayoutRect {
        x,
        y: 0,
        width,
        height: 24,
    };
    PaneLayout {
        workspace_id: workspace_id.into(),
        tab_id: tab_id.into(),
        zoomed: false,
        area: rect(0, 80),
        focused_pane_id: focused.into(),
        panes: vec![
            LayoutPane {
                pane_id: first.into(),
                focused: first == focused,
                rect: rect(0, 40),
            },
            LayoutPane {
                pane_id: second.into(),
                focused: second == focused,
                rect: rect(40, 40),
            },
        ],
        splits: vec![crate::LayoutSplit {
            id: "split_0_root".into(),
            direction: crate::SplitDirection::Right,
            ratio: 0.5,
            rect: rect(0, 80),
        }],
    }
}

fn pane_moved(
    pane: PaneInfo,
    previous_tab_id: &str,
    created_tab: Option<TabInfo>,
    closed_tab_id: Option<&str>,
) -> HerdrEvent {
    HerdrEvent::PaneMoved {
        previous_pane_id: pane.pane_id.clone(),
        previous_workspace_id: pane.workspace_id.clone(),
        previous_tab_id: previous_tab_id.into(),
        pane,
        created_workspace: None,
        created_tab: created_tab.map(Box::new),
        closed_workspace_id: None,
        closed_tab_id: closed_tab_id.map(str::to_owned),
    }
}

/// What `session.snapshot` returns for: w1 with tab t1 = `[p1 | p2]`,
/// p1 focused.
fn one_tab_two_panes() -> HierarchySnapshot {
    let mut w1 = workspace("w1", "one", true);
    w1.pane_count = 2;
    w1.active_tab_id = "t1".into();
    let mut t1 = tab("t1", "w1", "alpha", true);
    t1.pane_count = 2;
    HierarchySnapshot {
        focused_workspace_id: Some("w1".into()),
        focused_tab_id: Some("t1".into()),
        focused_pane_id: Some("p1".into()),
        workspaces: vec![w1],
        tabs: vec![t1],
        panes: vec![pane("p1", "w1", "t1", true), pane("p2", "w1", "t1", false)],
        layouts: vec![split_layout("w1", "t1", "p1", "p2", "p1")],
        ..Default::default()
    }
}

/// What `session.snapshot` returns after step 1 of design §4.2 moved p2
/// out to its own tab t2 with `focus: false`.
fn two_tabs_one_pane_each() -> HierarchySnapshot {
    let mut w1 = workspace("w1", "one", true);
    w1.pane_count = 2;
    w1.tab_count = 2;
    w1.active_tab_id = "t1".into();
    let mut t2 = tab("t2", "w1", "beta", false);
    t2.number = 2;
    HierarchySnapshot {
        focused_workspace_id: Some("w1".into()),
        focused_tab_id: Some("t1".into()),
        focused_pane_id: Some("p1".into()),
        workspaces: vec![w1],
        tabs: vec![tab("t1", "w1", "alpha", true), t2],
        panes: vec![pane("p1", "w1", "t1", true), pane("p2", "w1", "t2", false)],
        layouts: vec![layout("w1", "t1", "p1"), layout("w1", "t2", "p2")],
        ..Default::default()
    }
}

#[test]
fn moving_a_pane_to_a_new_tab_applies_without_resync() {
    // Herdr order for pane.move { destination: new_tab, focus: false }:
    // tab.created → pane.moved → layout.updated(source) → layout.updated(target).
    let mut snapshot = one_tab_two_panes();
    let expected = two_tabs_one_pane_each();
    let created = expected.tabs[1].clone();
    let events = [
        HerdrEvent::TabCreated {
            tab: created.clone(),
        },
        pane_moved(pane("p2", "w1", "t2", false), "t1", Some(created), None),
        HerdrEvent::LayoutUpdated {
            layout: layout("w1", "t1", "p1"),
        },
        HerdrEvent::LayoutUpdated {
            layout: layout("w1", "t2", "p2"),
        },
    ];
    for event in &events {
        assert_eq!(snapshot.apply(event), SnapshotUpdate::Applied, "{event:?}");
    }
    assert_eq!(snapshot, expected);
}

#[test]
fn moving_the_last_pane_out_of_a_tab_applies_without_resync() {
    // Step 2 of design §4.2: pane.move { destination: tab t1, target p1,
    // split right, focus: true }. Herdr emits tab.closed(t2) first; the
    // spec's shorter sequence without it must land on the same state.
    for with_tab_closed in [true, false] {
        let mut snapshot = two_tabs_one_pane_each();
        let mut events = Vec::new();
        if with_tab_closed {
            events.push(HerdrEvent::TabClosed {
                workspace_id: "w1".into(),
                tab_id: "t2".into(),
            });
        }
        events.push(pane_moved(
            pane("p2", "w1", "t1", true),
            "t2",
            None,
            Some("t2"),
        ));
        events.push(HerdrEvent::LayoutUpdated {
            layout: split_layout("w1", "t1", "p1", "p2", "p2"),
        });
        for event in &events {
            assert_eq!(
                snapshot.apply(event),
                SnapshotUpdate::Applied,
                "with_tab_closed={with_tab_closed}: {event:?}"
            );
        }

        let mut expected = one_tab_two_panes();
        expected.focused_pane_id = Some("p2".into());
        expected.panes[0].focused = false;
        expected.panes[1].focused = true;
        expected.layouts = vec![split_layout("w1", "t1", "p1", "p2", "p2")];
        assert_eq!(snapshot, expected, "with_tab_closed={with_tab_closed}");
    }
}

#[test]
fn moving_the_focused_pane_away_without_focus_takes_focus_from_the_source_layout() {
    // pane.moved carries focused:false and no pane.focused follows; the
    // source tab's layout.updated names the pane Herdr focused instead.
    let mut snapshot = one_tab_two_panes();
    let mut created = tab("t2", "w1", "beta", false);
    created.number = 2;
    let events = [
        HerdrEvent::TabCreated {
            tab: created.clone(),
        },
        pane_moved(pane("p1", "w1", "t2", false), "t1", Some(created), None),
    ];
    for event in &events {
        assert_eq!(snapshot.apply(event), SnapshotUpdate::Applied);
    }
    assert_eq!(snapshot.focused_pane_id, None);
    assert_eq!(
        snapshot.apply(&HerdrEvent::LayoutUpdated {
            layout: layout("w1", "t1", "p2"),
        }),
        SnapshotUpdate::Applied
    );
    assert_eq!(snapshot.focused_pane_id.as_deref(), Some("p2"));
    assert_eq!(snapshot.focused_tab_id.as_deref(), Some("t1"));
    let focused: Vec<(&str, bool)> = snapshot
        .panes
        .iter()
        .map(|pane| (pane.pane_id.as_str(), pane.focused))
        .collect();
    assert_eq!(focused, [("p2", true), ("p1", false)]);
}

#[test]
fn pane_moved_keeps_agent_lifecycle_fields_from_the_live_record() {
    let mut snapshot = two_tabs_one_pane_each();
    snapshot.panes[1].agent = Some("claude".into());
    snapshot.panes[1].agent_status = AgentStatus::Done;
    snapshot.panes[1].title = Some("done".into());
    let mut moved = pane("p2", "w1", "t1", true);
    moved.agent = Some("claude".into());
    moved.agent_status = AgentStatus::Working;
    moved.title = Some("stale".into());
    moved.cwd = Some("/new".into());
    assert_eq!(
        snapshot.apply(&pane_moved(moved, "t2", None, Some("t2"))),
        SnapshotUpdate::Applied
    );
    let record = snapshot.pane("p2").unwrap();
    assert_eq!(record.tab_id, "t1");
    assert_eq!(record.agent_status, AgentStatus::Done);
    assert_eq!(record.title.as_deref(), Some("done"));
    assert_eq!(record.cwd.as_deref(), Some("/new"));
}

#[test]
fn pane_moved_across_workspaces_applies_created_and_closed_records() {
    // Last pane of w1 moved to a new workspace w3: workspace.closed(w1)
    // → workspace.created(w3) → tab.created(t3) → pane.moved →
    // layout.updated(t3). (Herdr also emits tab.closed(t1) first, which
    // the TabClosed handler already resyncs on for a workspace's last
    // tab; this exercises the pane.moved cascade rule on its own.)
    let mut w1 = workspace("w1", "one", false);
    w1.active_tab_id = "t1".into();
    let mut w2_focused = workspace("w2", "two", true);
    w2_focused.active_tab_id = "t9".into();
    let mut snapshot = HierarchySnapshot {
        focused_workspace_id: Some("w2".into()),
        focused_tab_id: Some("t9".into()),
        focused_pane_id: Some("p9".into()),
        workspaces: vec![w1, w2_focused],
        tabs: vec![
            tab("t1", "w1", "alpha", false),
            tab("t9", "w2", "other", true),
        ],
        panes: vec![pane("p1", "w1", "t1", false), pane("p9", "w2", "t9", true)],
        layouts: vec![layout("w1", "t1", "p1"), layout("w2", "t9", "p9")],
        ..Default::default()
    };
    let mut w3 = workspace("w3", "three", true);
    w3.number = 2;
    w3.active_tab_id = "t3".into();
    let t3 = tab("t3", "w3", "alpha", true);
    let moved = pane("w3:p1", "w3", "t3", true);
    let events = [
        HerdrEvent::WorkspaceClosed {
            workspace_id: "w1".into(),
        },
        HerdrEvent::WorkspaceCreated {
            workspace: w3.clone(),
        },
        HerdrEvent::TabCreated { tab: t3.clone() },
        HerdrEvent::PaneMoved {
            pane: moved.clone(),
            previous_pane_id: "p1".into(),
            previous_workspace_id: "w1".into(),
            previous_tab_id: "t1".into(),
            created_workspace: Some(Box::new(w3.clone())),
            created_tab: Some(Box::new(t3.clone())),
            closed_workspace_id: Some("w1".into()),
            closed_tab_id: Some("t1".into()),
        },
        HerdrEvent::LayoutUpdated {
            layout: layout("w3", "t3", "w3:p1"),
        },
    ];
    for event in &events {
        assert_eq!(snapshot.apply(event), SnapshotUpdate::Applied, "{event:?}");
    }
    let mut w2 = workspace("w2", "two", false);
    w2.active_tab_id = "t9".into();
    let expected = HierarchySnapshot {
        focused_workspace_id: Some("w3".into()),
        focused_tab_id: Some("t3".into()),
        focused_pane_id: Some("w3:p1".into()),
        workspaces: vec![w2, w3],
        tabs: vec![tab("t9", "w2", "other", false), t3],
        panes: vec![pane("p9", "w2", "t9", false), moved],
        layouts: vec![layout("w2", "t9", "p9"), layout("w3", "t3", "w3:p1")],
        ..Default::default()
    };
    assert_eq!(snapshot, expected);
}

#[test]
fn pane_moved_resyncs_when_the_event_contradicts_the_snapshot() {
    // Target tab unknown.
    let mut snapshot = one_tab_two_panes();
    assert_eq!(
        snapshot.apply(&pane_moved(pane("p2", "w1", "t7", true), "t1", None, None)),
        SnapshotUpdate::Resync
    );
    assert_eq!(snapshot.pane("p2").unwrap().tab_id, "t1");

    // Target workspace unknown.
    let mut snapshot = one_tab_two_panes();
    assert_eq!(
        snapshot.apply(&pane_moved(pane("p2", "w9", "t1", true), "t1", None, None)),
        SnapshotUpdate::Resync
    );

    // Source pane unknown and nothing in the event explains it.
    let mut snapshot = one_tab_two_panes();
    assert_eq!(
        snapshot.apply(&pane_moved(pane("p8", "w1", "t1", true), "t1", None, None)),
        SnapshotUpdate::Resync
    );
    assert_eq!(snapshot.panes.len(), 2);

    // Source pane unknown, closed_tab_id names a tab we still have.
    let mut snapshot = one_tab_two_panes();
    assert_eq!(
        snapshot.apply(&pane_moved(
            pane("p8", "w1", "t1", true),
            "t1",
            None,
            Some("t1")
        )),
        SnapshotUpdate::Resync
    );

    // Source pane recorded in a different tab than the event claims.
    let mut snapshot = two_tabs_one_pane_each();
    assert_eq!(
        snapshot.apply(&pane_moved(pane("p2", "w1", "t1", true), "t1", None, None)),
        SnapshotUpdate::Resync
    );
}

#[test]
fn pane_moved_payloads_deserialize_with_and_without_optional_fields() {
    let full: HerdrEvent = serde_json::from_value(serde_json::json!({
        "type": "pane_moved",
        "previous_pane_id": "w1:p2",
        "previous_workspace_id": "w1",
        "previous_tab_id": "w1:t1",
        "pane": {
            "pane_id": "w1:p2", "terminal_id": "term-2", "workspace_id": "w1",
            "tab_id": "w1:t2", "focused": false, "revision": 3
        },
        "created_tab": {
            "tab_id": "w1:t2", "workspace_id": "w1", "number": 2,
            "label": "shell", "focused": false, "pane_count": 1
        },
        "closed_tab_id": "w1:t0"
    }))
    .unwrap();
    let HerdrEvent::PaneMoved {
        created_tab,
        closed_tab_id,
        created_workspace,
        closed_workspace_id,
        ..
    } = full
    else {
        panic!("expected pane_moved");
    };
    assert_eq!(created_tab.map(|tab| tab.tab_id).as_deref(), Some("w1:t2"));
    assert_eq!(closed_tab_id.as_deref(), Some("w1:t0"));
    assert_eq!(created_workspace, None);
    assert_eq!(closed_workspace_id, None);

    let minimal: HerdrEvent = serde_json::from_value(serde_json::json!({
        "type": "pane_moved",
        "previous_pane_id": "w1:p2",
        "previous_workspace_id": "w1",
        "previous_tab_id": "w1:t1",
        "pane": {
            "pane_id": "w1:p2", "terminal_id": "term-2", "workspace_id": "w1",
            "tab_id": "w1:t2", "focused": false
        }
    }))
    .unwrap();
    assert!(matches!(
        minimal,
        HerdrEvent::PaneMoved {
            created_tab: None,
            closed_tab_id: None,
            ..
        }
    ));
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
