use super::*;

fn transfer_snapshot() -> HierarchySnapshot {
    let mut snapshot = two_pane_snapshot();
    snapshot.workspaces[0].tab_count = 2;
    snapshot.tabs.retain(|tab| tab.tab_id != "t-b");
    snapshot.workspaces.push(WorkspaceInfo {
        workspace_id: "w-target".into(),
        number: 2,
        label: "target workspace".into(),
        focused: false,
        pane_count: 0,
        tab_count: 1,
        active_tab_id: "t-b".into(),
        agent_status: AgentStatus::Idle,
        tokens: Default::default(),
        worktree: None,
    });
    snapshot.tabs.push(TabInfo {
        tab_id: "t-b".into(),
        workspace_id: "w-target".into(),
        number: 1,
        label: "existing target".into(),
        focused: true,
        pane_count: 0,
        agent_status: AgentStatus::Idle,
    });
    snapshot
}

fn single_source_tab_transfer_snapshot() -> HierarchySnapshot {
    let mut snapshot = transfer_snapshot();
    snapshot.tabs.retain(|tab| tab.tab_id != "t-c");
    snapshot.workspaces[0].tab_count = 1;
    snapshot.focused_pane_id = Some("p-left".into());
    snapshot.panes = vec![split_pane("p-left", true)];
    snapshot.layouts = vec![single_pane_layout("t-a", "p-left")];
    snapshot
}

fn connect_transfer_view(
    cx: &mut TestAppContext,
    script: PaneMoveScript,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events_and_script(transfer_snapshot(), script);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    (fake, view, cx)
}

fn connect_snapshot(
    cx: &mut TestAppContext,
    snapshot: HierarchySnapshot,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    (fake, view, cx)
}

fn drag_tab_to_target_workspace(cx: &mut VisualTestContext) {
    let tab = cx.debug_bounds("tab-t-a").expect("source tab pill");
    let target = cx
        .debug_bounds("workspace-w-target")
        .expect("target workspace row");
    cx.simulate_mouse_down(
        tab.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_move(
        target.center(),
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tab-workspace-drop-w-target").is_some(),
        "the workspace row paints the semantic tab drop target"
    );
    cx.simulate_mouse_up(
        target.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
}

#[gpui::test]
fn dragging_a_tab_to_a_workspace_rebuilds_its_layout_with_existing_panes(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_transfer_view(cx, PaneMoveScript::default());
    drag_tab_to_target_workspace(cx);
    cx.run_until_parked();

    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 2, "two panes require two moves: {moves:?}");
    assert_eq!(moves[0]["params"]["pane_id"], json!("p-left"));
    assert_eq!(
        moves[0]["params"]["destination"],
        json!({
            "type": "new_tab",
            "workspace_id": "w-target",
            "label": "alpha",
        })
    );
    assert_eq!(moves[0]["params"]["focus"], json!(false));
    assert_eq!(moves[1]["params"]["pane_id"], json!("p-right"));
    assert_eq!(
        moves[1]["params"]["destination"],
        json!({
            "type": "tab",
            "tab_id": "t-tmp",
            "target_pane_id": "p-left",
            "split": "right",
            "ratio": 0.5,
        })
    );
    let focus = fake.requests_for("pane.focus");
    assert_eq!(focus.len(), 1);
    assert_eq!(focus[0]["params"]["pane_id"], json!("p-left"));
    view.read_with(cx, |this, _| {
        assert!(this.pending_tab_transfer.is_none());
    });
}

#[gpui::test]
fn the_only_tab_in_a_workspace_can_still_be_dragged_to_another_workspace(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_snapshot(cx, single_source_tab_transfer_snapshot());
    drag_tab_to_target_workspace(cx);
    cx.run_until_parked();

    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0]["params"]["pane_id"], json!("p-left"));
    assert_eq!(
        moves[0]["params"]["destination"]["workspace_id"],
        json!("w-target")
    );
    view.read_with(cx, |this, _| {
        assert!(this.pending_tab_transfer.is_none());
    });
}

#[gpui::test]
fn a_failed_layout_step_is_resumable_without_repeating_completed_moves(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        insert_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_transfer_view(cx, script);
    drag_tab_to_target_workspace(cx);
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        let transfer = this.pending_tab_transfer.as_ref().expect("paused transfer");
        assert_eq!(transfer.phase, crate::TabTransferPhase::Failed);
        assert_eq!(transfer.next_step, 0);
        assert_eq!(transfer.target_tab_id.as_deref(), Some("t-tmp"));
    });
    view.update(cx, |_this, cx| cx.notify());
    cx.run_until_parked();
    assert!(cx.debug_bounds("tab-transfer-paused").is_some());
    assert_eq!(fake.requests_for("pane.move").len(), 2);

    let retry = cx.debug_bounds("tab-transfer-retry").expect("retry action");
    cx.simulate_click(retry.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 3, "only the failed insert is retried");
    assert_eq!(
        moves
            .iter()
            .filter(|request| request["params"]["destination"]["type"] == json!("new_tab"))
            .count(),
        1,
        "the completed first move must not repeat"
    );
    view.read_with(cx, |this, _| {
        assert!(this.pending_tab_transfer.is_none());
    });
}

#[gpui::test]
fn retry_skips_a_move_that_the_live_snapshot_shows_was_already_applied(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        insert_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_transfer_view(cx, script);
    drag_tab_to_target_workspace(cx);
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 2);

    // Models the ambiguous network case: Herdr applied the insert and emitted
    // its event, but the command socket delivered an error instead of the
    // response. Stable terminal identity proves the pane is already there.
    view.update(cx, |this, cx| {
        let snapshot = this.snapshot.as_mut().expect("snapshot");
        let pane = snapshot
            .panes
            .iter_mut()
            .find(|pane| pane.pane_id == "p-right")
            .expect("second pane");
        pane.workspace_id = "w-target".into();
        pane.tab_id = "t-tmp".into();
        this.retry_tab_transfer(cx);
    });
    cx.run_until_parked();

    assert_eq!(
        fake.requests_for("pane.move").len(),
        2,
        "an already-applied insert is not sent twice"
    );
    assert_eq!(fake.requests_for("pane.focus").len(), 1);
    view.read_with(cx, |this, _| {
        assert!(this.pending_tab_transfer.is_none());
    });
}
