use super::*;

fn prepare_two_pane_view(
    cx: &mut TestAppContext,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    (fake, view, cx)
}

fn connect_one_pane_view(
    cx: &mut TestAppContext,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let mut snapshot = pane_move_capable_snapshot();
    snapshot.focused_pane_id = Some("p-left".into());
    snapshot.panes = vec![split_pane("p-left", true)];
    snapshot.layouts = vec![single_pane_layout("t-a", "p-left")];
    let fake = FakeHerdr::snapshot_with_live_events(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| {
        this.terminal_surface_bounds = Some(SURFACE);
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    (fake, view, cx)
}

fn connect_two_pane_scripted(
    cx: &mut TestAppContext,
    script: PaneMoveScript,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events_and_script(two_pane_snapshot(), script);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| {
        this.terminal_surface_bounds = Some(SURFACE);
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    (fake, view, cx)
}

fn lift_left_pane(view: &Entity<OcHerdrView>, cx: &mut VisualTestContext) {
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    view.update(cx, |this, cx| {
        assert!(this.begin_pane_drag("p-left".into(), grab));
        assert!(this.update_pane_drag((grab.0 + 40., grab.1 + 30.), cx));
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(drag.tab_bar_drops);
    });
    cx.run_until_parked();
}

fn drop_zone_center(cx: &mut VisualTestContext) -> gpui::Point<gpui::Pixels> {
    let bounds = cx
        .debug_bounds("pane-tab-drop-new-tab")
        .or_else(|| cx.debug_bounds("tab-strip-space"))
        .expect("new-tab drop zone");
    bounds.center()
}

fn hover_new_tab_drop(
    view: &Entity<OcHerdrView>,
    cx: &mut VisualTestContext,
) -> gpui::Point<gpui::Pixels> {
    lift_left_pane(view, cx);
    let point = drop_zone_center(cx);
    cx.simulate_mouse_move(
        point,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert_eq!(
            drag.tab_target,
            Some(crate::PaneTabDropTarget::NewTab),
            "the painted + / trailing strip must publish NewTab"
        );
        assert!(drag.hover.is_none());
        assert!(drag.template_hover.is_none());
        assert!(matches!(
            drag.layout_preview
                .as_ref()
                .and_then(|preview| preview.intent.as_ref()),
            Some(crate::PaneDragIntent::Tab(crate::PaneTabDropTarget::NewTab))
        ));
    });
    point
}

fn assert_new_tab_move(request: &Value) {
    assert_eq!(request["method"], json!("pane.move"));
    assert_eq!(request["params"]["pane_id"], json!("p-left"));
    assert_eq!(
        request["params"]["destination"],
        json!({ "type": "new_tab", "workspace_id": "w" })
    );
    assert_eq!(request["params"]["focus"], json!(true));
}

fn focused_temp_tab() -> TabInfo {
    let mut tab = temp_tab();
    tab.focused = true;
    tab
}

fn pane_on_temp_tab(pane_id: &str, focused: bool) -> Value {
    let mut pane = split_pane(pane_id, focused);
    pane.tab_id = "t-tmp".into();
    serde_json::to_value(pane).expect("pane json")
}

fn new_tab_detach_events() -> Vec<Value> {
    let created = focused_temp_tab();
    vec![
        json!({ "event": "tab_created", "data": { "type": "tab_created", "tab": created.clone() } }),
        json!({
            "event": "pane_moved",
            "data": {
                "type": "pane_moved",
                "pane": pane_on_temp_tab("p-left", true),
                "previous_pane_id": "p-left",
                "previous_workspace_id": "w",
                "previous_tab_id": "t-a",
                "created_tab": created,
            }
        }),
        layout_event(single_pane_layout("t-a", "p-right")),
        layout_event(single_pane_layout("t-tmp", "p-left")),
    ]
}

fn last_pane_detach_events() -> Vec<Value> {
    let created = focused_temp_tab();
    vec![
        json!({ "event": "tab_created", "data": { "type": "tab_created", "tab": created.clone() } }),
        json!({
            "event": "pane_moved",
            "data": {
                "type": "pane_moved",
                "pane": pane_on_temp_tab("p-left", true),
                "previous_pane_id": "p-left",
                "previous_workspace_id": "w",
                "previous_tab_id": "t-a",
                "created_tab": created,
            }
        }),
        json!({
            "event": "tab_closed",
            "data": { "type": "tab_closed", "tab_id": "t-a", "workspace_id": "w" }
        }),
        layout_event(single_pane_layout("t-tmp", "p-left")),
    ]
}

#[gpui::test]
fn dragging_over_the_plus_and_trailing_strip_hits_new_tab(cx: &mut TestAppContext) {
    let (fake, view, cx) = prepare_two_pane_view(cx);
    hover_new_tab_drop(&view, cx);
    assert!(
        cx.debug_bounds("pane-tab-drop-new-tab").is_some(),
        "the painted target is locatable"
    );
    assert!(
        cx.debug_bounds("pane-tab-drop-new-tab-hint").is_some(),
        "hint text is shown while hovered"
    );
    assert!(fake.requests_for("pane.move").is_empty());
}

#[gpui::test]
fn releasing_on_the_new_tab_target_sends_one_focused_pane_move(cx: &mut TestAppContext) {
    let (fake, view, cx) = prepare_two_pane_view(cx);
    let point = hover_new_tab_drop(&view, cx);
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    view.update_in(cx, |this, window, cx| {
        assert!(
            !this.finish_pane_drag((f32::from(point.x), f32::from(point.y)), window, cx),
            "a second release must not resubmit"
        );
    });
    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 1, "exactly one pane.move: {moves:?}");
    assert_new_tab_move(&moves[0]);
    assert!(fake.requests_for("tab.close").is_empty());
    view.read_with(cx, |this, _| {
        assert!(this.pane_detaches.contains_key("t-a"));
        assert!(this.tab_relocation_locked("t-a"));
        assert!(!this.hidden_tab_ids().contains("t-tmp"));
    });
}

#[gpui::test]
fn a_new_tab_detach_resizes_remaining_panes_immediately(cx: &mut TestAppContext) {
    let (fake, view, cx) = prepare_two_pane_view(cx);
    let point = hover_new_tab_drop(&view, cx);
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    view.read_with(cx, |this, _| {
        assert!(this.pane_detaches.contains_key("t-a"));
        assert!(
            this.tab_relocation_locked("t-a"),
            "structure stays locked while the move is in flight"
        );
        assert!(
            !this.tab_resize_frozen("t-a"),
            "the predicted remaining layout is already exact"
        );
        assert!(!this.pane_resize_frozen("p-right"));
        let visible = this.optimistic_visible_pane_ids();
        assert!(
            visible.contains("p-left"),
            "the moved pane must keep producing frames: {visible:?}"
        );
        assert!(visible.contains("p-right"));
    });
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 1);
}

#[gpui::test]
fn leaving_the_new_tab_target_sends_nothing(cx: &mut TestAppContext) {
    let (fake, view, cx) = prepare_two_pane_view(cx);
    hover_new_tab_drop(&view, cx);
    let terminal = gpui::point(
        gpui::px(SURFACE.0 + SURFACE.2 / 2.),
        gpui::px(SURFACE.1 + SURFACE.3 / 2.),
    );
    cx.simulate_mouse_move(
        terminal,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(drag.tab_target.is_none());
    });
    view.update_in(cx, |this, window, cx| {
        assert!(this.finish_pane_drag(
            (SURFACE.0 + SURFACE.2 / 2., SURFACE.1 + SURFACE.3 / 2.),
            window,
            cx
        ));
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.move").is_empty());
    assert!(fake.requests_for("tab.close").is_empty());
}

#[gpui::test]
fn escape_cancels_a_new_tab_hover_without_a_request(cx: &mut TestAppContext) {
    let (fake, view, cx) = prepare_two_pane_view(cx);
    hover_new_tab_drop(&view, cx);
    view.update_in(cx, |this, window, cx| {
        let escape = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: Default::default(),
                key: "escape".into(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(this.handle_app_shortcut(&escape, window, cx));
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(this.pane_drag_return.is_some());
        assert!(this.pane_detaches.is_empty());
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.move").is_empty());
}

#[gpui::test]
fn a_single_pane_tab_can_hit_the_tab_bar_but_not_pane_local_drops(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_one_pane_view(cx);
    lift_left_pane(&view, cx);
    view.update(cx, |this, cx| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(drag.tab_bar_drops);
        assert!(!drag.layout_templates);
        let centre = (SURFACE.0 + SURFACE.2 / 2., SURFACE.1 + SURFACE.3 / 2.);
        assert!(this.update_pane_drag(centre, cx));
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(
            drag.hover.is_none(),
            "no pane-local target in a one-pane tab"
        );
        assert!(drag.template_hover.is_none());
        assert!(drag.tab_target.is_none());
    });
    let point = drop_zone_center(cx);
    cx.simulate_mouse_move(
        point,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert_eq!(drag.tab_target, Some(crate::PaneTabDropTarget::NewTab));
    });
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 1);
    assert_new_tab_move(&moves[0]);
    assert!(fake.requests_for("tab.close").is_empty());
}

#[gpui::test]
fn zoomed_or_locked_or_unsupported_tabs_do_not_open_tab_bar_drops(cx: &mut TestAppContext) {
    let (_fake, view, cx) = prepare_two_pane_view(cx);
    view.update(cx, |this, _| {
        this.snapshot
            .as_mut()
            .and_then(|snapshot| snapshot.layouts.first_mut())
            .expect("layout")
            .zoomed = true;
        assert!(
            !this.begin_pane_drag("p-left".into(), (SURFACE.0 + 12., SURFACE.1 + 12.)),
            "zoomed layouts refuse the drag"
        );
        this.snapshot
            .as_mut()
            .and_then(|snapshot| snapshot.layouts.first_mut())
            .expect("layout")
            .zoomed = false;
    });

    let mut snapshot = three_tab_snapshot();
    snapshot.focused_pane_id = Some("p-left".into());
    snapshot.panes = vec![split_pane("p-left", true), split_pane("p-right", false)];
    snapshot.layouts = vec![two_pane_layout("p-left", "p-right")];
    let fake = FakeHerdr::snapshot_with_live_events(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, cx| {
        this.terminal_surface_bounds = Some(SURFACE);
        assert!(!this.pane_move_supported());
        assert!(this.begin_pane_drag("p-left".into(), (SURFACE.0 + 12., SURFACE.1 + 12.)));
        assert!(this.update_pane_drag((SURFACE.0 + 52., SURFACE.1 + 42.), cx));
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(!drag.tab_bar_drops);
        assert!(!drag.layout_templates);
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("pane-tab-drop-new-tab").is_none());
}

#[gpui::test]
fn a_failed_or_disconnected_new_tab_drop_clears_the_lock(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        park_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_two_pane_scripted(cx, script);
    let point = hover_new_tab_drop(&view, cx);
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    view.read_with(cx, |this, _| {
        assert!(
            this.pane_detaches.is_empty(),
            "a refused move must not keep the optimistic lock"
        );
        assert!(!this.tab_relocation_locked("t-a"));
    });

    let (fake, view, cx) = prepare_two_pane_view(cx);
    let point = hover_new_tab_drop(&view, cx);
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    view.update(cx, |this, cx| {
        assert!(!this.pane_detaches.is_empty());
        this.apply_event_batch(None, cx);
        assert!(this.pane_detaches.is_empty());
        assert!(!this.tab_relocation_locked("t-a"));
    });
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 1);
}

#[gpui::test]
fn new_tab_events_before_the_response_still_converge(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        events_before_park_response: Mutex::new(new_tab_detach_events()),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_two_pane_scripted(cx, script);
    let point = hover_new_tab_drop(&view, cx);
    view.update_in(cx, |this, window, cx| {
        assert!(this.finish_pane_drag((f32::from(point.x), f32::from(point.y)), window, cx));
        assert!(
            this.pane_detaches.contains_key("t-a"),
            "the detach is pending before any event or response"
        );
        assert!(!this.pane_detaches.get("t-a").unwrap().responded);
    });

    // The fake broadcasts live events, then waits 60ms before writing the
    // pane.move response. Pump until those events are applied while the
    // request is still unanswered.
    let mut events_while_pending = false;
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let _ = cx.executor().tick();
        view.read_with(cx, |this, _| {
            let temp_tab_known = this
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp"));
            if temp_tab_known {
                let pending = this
                    .pane_detaches
                    .get("t-a")
                    .expect("events must not drop the pending detach before the response");
                assert!(!pending.responded, "events beat the response: {pending:?}");
                assert!(!this.hidden_tab_ids().contains("t-tmp"));
                events_while_pending = true;
            }
        });
        if events_while_pending {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        events_while_pending,
        "the race was exercised: events landed while the detach was still pending"
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.pane_detaches.is_empty(),
            "response converges the detach"
        );
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp"));
        let tabs: Vec<String> = this
            .chrome_a11y()
            .tabs
            .items
            .iter()
            .map(|row| row.a11y.id.clone())
            .collect();
        assert!(
            tabs.contains(&"t-tmp".to_owned()),
            "the new tab stays visible: {tabs:?}"
        );
        assert!(!this.hidden_tab_ids().contains("t-tmp"));
    });
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    assert!(fake.requests_for("tab.close").is_empty());
}

#[gpui::test]
fn new_tab_events_settle_without_hiding_the_created_tab(cx: &mut TestAppContext) {
    let (fake, view, cx) = prepare_two_pane_view(cx);
    let point = hover_new_tab_drop(&view, cx);
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    send_events(&fake, new_tab_detach_events(), cx);
    view.read_with(cx, |this, _| {
        assert!(
            this.pane_detaches.is_empty(),
            "response + matching events settle"
        );
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp"));
        let tabs: Vec<String> = this
            .chrome_a11y()
            .tabs
            .items
            .iter()
            .map(|row| row.a11y.id.clone())
            .collect();
        assert!(
            tabs.contains(&"t-tmp".to_owned()),
            "the new tab stays visible: {tabs:?}"
        );
        assert!(!this.hidden_tab_ids().contains("t-tmp"));
    });
    assert!(fake.requests_for("tab.close").is_empty());
    assert_eq!(fake.requests_for("pane.move").len(), 1);
}

#[gpui::test]
fn moving_the_last_pane_does_not_send_tab_close(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_one_pane_view(cx);
    let point = hover_new_tab_drop(&view, cx);
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    assert_new_tab_move(&fake.requests_for("pane.move")[0]);
    assert!(fake.requests_for("tab.close").is_empty());
    send_events(&fake, last_pane_detach_events(), cx);
    view.read_with(cx, |this, _| {
        assert!(this.pane_detaches.is_empty());
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(snapshot.tabs.iter().all(|tab| tab.tab_id != "t-a"));
        assert!(snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp"));
        assert!(!this.hidden_tab_ids().contains("t-tmp"));
    });
    assert!(fake.requests_for("tab.close").is_empty());
}

#[gpui::test]
fn clicking_the_plus_still_creates_a_tab(cx: &mut TestAppContext) {
    let (fake, view, cx) = prepare_two_pane_view(cx);
    view.read_with(cx, |this, _| {
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
    });
    let plus = cx.debug_bounds("new-tab").expect("new-tab button");
    cx.simulate_click(plus.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(fake.requests_for("tab.create").len(), 1);
    assert!(fake.requests_for("pane.move").is_empty());
}

fn pane_on_tab(pane_id: &str, tab_id: &str, focused: bool) -> PaneInfo {
    let mut pane = split_pane(pane_id, focused);
    pane.tab_id = tab_id.into();
    pane
}

fn layout_on_tab(tab_id: &str, left: &str, right: &str) -> ocherdr_core::PaneLayout {
    let mut layout = two_pane_layout(left, right);
    layout.tab_id = tab_id.into();
    layout
}

fn two_tab_merge_snapshot() -> HierarchySnapshot {
    let mut snapshot = pane_move_capable_snapshot();
    snapshot.focused_pane_id = Some("p-left".into());
    snapshot.focused_tab_id = Some("t-a".into());
    snapshot.panes = vec![
        pane_on_tab("p-left", "t-a", true),
        pane_on_tab("p-b", "t-b", false),
    ];
    snapshot.layouts = vec![
        single_pane_layout("t-a", "p-left"),
        single_pane_layout("t-b", "p-b"),
    ];
    snapshot
}

fn connect_merge_view(
    cx: &mut TestAppContext,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    connect_merge_view_scripted(cx, PaneMoveScript::default())
}

fn connect_merge_view_scripted(
    cx: &mut TestAppContext,
    script: PaneMoveScript,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events_and_script(two_tab_merge_snapshot(), script);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| {
        this.terminal_surface_bounds = Some(SURFACE);
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    (fake, view, cx)
}

fn hover_existing_drop(
    view: &Entity<OcHerdrView>,
    cx: &mut VisualTestContext,
    tab_id: &'static str,
) -> gpui::Point<gpui::Pixels> {
    lift_left_pane(view, cx);
    let selector: &'static str = match tab_id {
        "t-a" => "tab-t-a",
        "t-b" => "tab-t-b",
        "t-c" => "tab-t-c",
        other => panic!("unknown tab {other}"),
    };
    let bounds = cx.debug_bounds(selector).expect("existing tab pill");
    let point = bounds.center();
    cx.simulate_mouse_move(
        point,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    point
}

fn assert_existing_tab_move(request: &Value, tab_id: &str, target_pane_id: &str) {
    assert_eq!(request["method"], json!("pane.move"));
    assert_eq!(request["params"]["pane_id"], json!("p-left"));
    assert_eq!(request["params"]["destination"]["type"], json!("tab"));
    assert_eq!(request["params"]["destination"]["tab_id"], json!(tab_id));
    assert_eq!(
        request["params"]["destination"]["target_pane_id"],
        json!(target_pane_id)
    );
    assert_eq!(request["params"]["destination"]["split"], json!("right"));
    let ratio = request["params"]["destination"]["ratio"]
        .as_f64()
        .expect("ratio");
    assert!((ratio - 0.5).abs() < 1e-6, "{ratio}");
    assert_eq!(request["params"]["focus"], json!(true));
}

fn merge_into_b_events() -> Vec<Value> {
    let mut moved = pane_on_tab("p-left", "t-b", true);
    moved.tab_id = "t-b".into();
    vec![
        json!({
            "event": "pane_moved",
            "data": {
                "type": "pane_moved",
                "pane": moved,
                "previous_pane_id": "p-left",
                "previous_workspace_id": "w",
                "previous_tab_id": "t-a",
            }
        }),
        json!({
            "event": "tab_closed",
            "data": { "type": "tab_closed", "tab_id": "t-a", "workspace_id": "w" }
        }),
        layout_event(layout_on_tab("t-b", "p-b", "p-left")),
    ]
}

#[gpui::test]
fn dragging_over_an_existing_tab_pill_hits_and_highlights(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    hover_existing_drop(&view, cx, "t-b");
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert_eq!(
            drag.tab_target,
            Some(crate::PaneTabDropTarget::Existing {
                tab_id: "t-b".into(),
                target_pane_id: "p-b".into(),
            })
        );
        assert!(drag.hover.is_none());
        assert!(drag.template_hover.is_none());
    });
    assert!(
        cx.debug_bounds("pane-tab-drop-existing-t-b").is_some(),
        "the painted pill highlight is locatable"
    );
    assert!(fake.requests_for("pane.move").is_empty());
}

#[gpui::test]
fn the_source_tab_pill_is_not_an_existing_drop_target(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    let point = hover_existing_drop(&view, cx, "t-a");
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(
            drag.tab_target.is_none(),
            "the source pill is not droppable"
        );
    });
    assert!(cx.debug_bounds("pane-tab-drop-existing-t-a").is_none());
    view.update_in(cx, |this, window, cx| {
        assert!(this.finish_pane_drag((f32::from(point.x), f32::from(point.y)), window, cx));
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.move").is_empty());
}

#[gpui::test]
fn releasing_on_an_existing_tab_sends_one_right_split_move(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    let point = hover_existing_drop(&view, cx, "t-b");
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    view.update_in(cx, |this, window, cx| {
        assert!(
            !this.finish_pane_drag((f32::from(point.x), f32::from(point.y)), window, cx),
            "a second release must not resubmit"
        );
    });
    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 1, "exactly one pane.move: {moves:?}");
    assert_existing_tab_move(&moves[0], "t-b", "p-b");
    assert!(fake.requests_for("tab.close").is_empty());
    view.read_with(cx, |this, _| {
        assert!(this.pane_detaches.contains_key("t-a"));
        assert!(this.tab_relocation_locked("t-a"));
        assert!(this.tab_relocation_locked("t-b"));
        assert!(!this.tab_resize_frozen("t-a"));
        assert!(!this.tab_resize_frozen("t-b"));
        assert!(!this.pane_resize_frozen("p-b"));
        let visible = this.optimistic_visible_pane_ids();
        assert!(visible.contains("p-left"));
        assert!(visible.contains("p-b"));
    });
}

#[gpui::test]
fn leaving_or_escaping_an_existing_tab_target_sends_nothing(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    hover_existing_drop(&view, cx, "t-b");
    let terminal = gpui::point(
        gpui::px(SURFACE.0 + SURFACE.2 / 2.),
        gpui::px(SURFACE.1 + SURFACE.3 / 2.),
    );
    cx.simulate_mouse_move(
        terminal,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(drag.tab_target.is_none());
    });
    view.update_in(cx, |this, window, cx| {
        assert!(this.finish_pane_drag(
            (SURFACE.0 + SURFACE.2 / 2., SURFACE.1 + SURFACE.3 / 2.),
            window,
            cx
        ));
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.move").is_empty());
}

#[gpui::test]
fn escape_cancels_an_existing_tab_hover_without_a_request(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    hover_existing_drop(&view, cx, "t-b");
    view.update_in(cx, |this, window, cx| {
        let escape = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: Default::default(),
                key: "escape".into(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(this.handle_app_shortcut(&escape, window, cx));
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(this.pane_detaches.is_empty());
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.move").is_empty());
}

#[gpui::test]
fn existing_tab_events_settle_after_the_response(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    let point = hover_existing_drop(&view, cx, "t-b");
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    send_events(&fake, merge_into_b_events(), cx);
    view.read_with(cx, |this, _| {
        assert!(this.pane_detaches.is_empty());
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(snapshot.tabs.iter().all(|tab| tab.tab_id != "t-a"));
        assert!(snapshot.tabs.iter().any(|tab| tab.tab_id == "t-b"));
        assert_eq!(
            snapshot.pane("p-left").map(|pane| pane.tab_id.as_str()),
            Some("t-b")
        );
    });
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    assert!(fake.requests_for("tab.close").is_empty());
}

#[gpui::test]
fn existing_tab_events_before_the_response_still_converge(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        events_before_insert_response: Mutex::new(merge_into_b_events()),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_merge_view_scripted(cx, script);
    let point = hover_existing_drop(&view, cx, "t-b");
    view.update_in(cx, |this, window, cx| {
        assert!(this.finish_pane_drag((f32::from(point.x), f32::from(point.y)), window, cx));
        assert!(this.pane_detaches.contains_key("t-a"));
        assert!(!this.pane_detaches.get("t-a").unwrap().responded);
    });
    let mut events_while_pending = false;
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let _ = cx.executor().tick();
        view.read_with(cx, |this, _| {
            let moved = this.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .pane("p-left")
                    .is_some_and(|pane| pane.tab_id == "t-b")
            });
            if moved {
                let pending = this
                    .pane_detaches
                    .get("t-a")
                    .expect("events must not drop the pending transfer before the response");
                assert!(!pending.responded);
                events_while_pending = true;
            }
        });
        if events_while_pending {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        events_while_pending,
        "the race was exercised: events landed while the transfer was still pending"
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.pane_detaches.is_empty());
        assert!(this.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .pane("p-left")
                .is_some_and(|pane| pane.tab_id == "t-b")
        }));
    });
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    assert!(fake.requests_for("tab.close").is_empty());
}

#[gpui::test]
fn merging_the_last_pane_does_not_send_tab_close(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    let point = hover_existing_drop(&view, cx, "t-b");
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    send_events(&fake, merge_into_b_events(), cx);
    view.read_with(cx, |this, _| {
        assert!(this.pane_detaches.is_empty());
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(snapshot.tabs.iter().all(|tab| tab.tab_id != "t-a"));
    });
    assert!(fake.requests_for("tab.close").is_empty());
}

#[gpui::test]
fn zoomed_missing_or_locked_tabs_are_not_existing_drop_targets(cx: &mut TestAppContext) {
    let (_fake, view, cx) = connect_merge_view(cx);
    view.update(cx, |this, _| {
        this.snapshot
            .as_mut()
            .and_then(|snapshot| {
                snapshot
                    .layouts
                    .iter_mut()
                    .find(|layout| layout.tab_id == "t-b")
            })
            .expect("t-b layout")
            .zoomed = true;
    });
    hover_existing_drop(&view, cx, "t-b");
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(drag.tab_target.is_none(), "zoomed target is not droppable");
    });
    view.update(cx, |this, _| this.cancel_pane_drag());

    hover_existing_drop(&view, cx, "t-c");
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(
            drag.tab_target.is_none(),
            "a tab without a layout is not droppable"
        );
    });
    view.update(cx, |this, _| this.cancel_pane_drag());

    view.update(cx, |this, _| {
        this.snapshot
            .as_mut()
            .and_then(|snapshot| {
                snapshot
                    .layouts
                    .iter_mut()
                    .find(|layout| layout.tab_id == "t-b")
            })
            .expect("t-b layout")
            .zoomed = false;
        this.split_commit = Some(crate::PendingSplitCommit {
            tab_id: "t-b".into(),
            layout: crate::SplitLayoutFingerprint {
                zoomed: false,
                splits: Vec::new(),
                panes: vec!["p-b".into()],
            },
            ratios: Vec::new(),
            serial: 1,
            outstanding: 1,
            last_ratios: Vec::new(),
            layouts_seen: 0,
        });
    });
    hover_existing_drop(&view, cx, "t-b");
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(
            drag.tab_target.is_none(),
            "a structurally locked tab is not droppable"
        );
        assert!(this.tab_relocation_locked("t-b"));
    });
}

#[gpui::test]
fn a_cross_workspace_existing_target_is_rejected(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    view.update(cx, |this, cx| {
        this.snapshot
            .as_mut()
            .and_then(|snapshot| snapshot.tabs.iter_mut().find(|tab| tab.tab_id == "t-b"))
            .expect("t-b")
            .workspace_id = "w-other".into();
        assert!(
            this.existing_tab_drop_target("t-b", "t-a", "w").is_none(),
            "hover must not publish a cross-workspace pill"
        );
        let fingerprint = ocherdr_core::layout_fingerprint(
            this.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.layout_for("t-a"))
                .expect("source layout"),
        );
        assert!(
            !this.commit_pane_tab_drop(
                "w",
                "t-a",
                "p-left",
                fingerprint,
                crate::PaneTabDropTarget::Existing {
                    tab_id: "t-b".into(),
                    target_pane_id: "p-b".into(),
                },
                cx,
            ),
            "release must re-check snapshot.tabs workspace, not trust hover"
        );
        assert!(this.pane_detaches.is_empty());
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.move").is_empty());
}

#[gpui::test]
fn a_vanished_existing_target_drops_the_pending_transfer(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_merge_view(cx);
    let point = hover_existing_drop(&view, cx, "t-b");
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    view.update(cx, |this, cx| {
        assert!(this.pane_detaches.contains_key("t-a"));
        assert!(this.tab_relocation_locked("t-a"));
        assert!(this.tab_relocation_locked("t-b"));
        let snapshot = this.snapshot.as_mut().expect("snapshot");
        snapshot.tabs.retain(|tab| tab.tab_id != "t-b");
        snapshot.layouts.retain(|layout| layout.tab_id != "t-b");
        snapshot.panes.retain(|pane| pane.tab_id != "t-b");
        this.reconcile_pane_detaches(cx);
        assert!(
            this.pane_detaches.is_empty(),
            "a missing Existing target is foreign, not a hang"
        );
        assert!(!this.tab_relocation_locked("t-a"));
        assert!(!this.tab_relocation_locked("t-b"));
    });
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    assert!(fake.requests_for("tab.close").is_empty());
}

fn two_pane_source_merge_snapshot() -> HierarchySnapshot {
    let mut snapshot = pane_move_capable_snapshot();
    snapshot.focused_pane_id = Some("p-left".into());
    snapshot.focused_tab_id = Some("t-a".into());
    snapshot.panes = vec![
        pane_on_tab("p-left", "t-a", true),
        pane_on_tab("p-right", "t-a", false),
        pane_on_tab("p-b", "t-b", false),
    ];
    snapshot.layouts = vec![
        two_pane_layout("p-left", "p-right"),
        single_pane_layout("t-b", "p-b"),
    ];
    snapshot
}

fn connect_two_pane_merge_view(
    cx: &mut TestAppContext,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events_and_script(
        two_pane_source_merge_snapshot(),
        PaneMoveScript::default(),
    );
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| {
        this.terminal_surface_bounds = Some(SURFACE);
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    (fake, view, cx)
}

fn multi_pane_merge_into_b_events() -> Vec<Value> {
    vec![
        json!({
            "event": "pane_moved",
            "data": {
                "type": "pane_moved",
                "pane": pane_on_tab("p-left", "t-b", true),
                "previous_pane_id": "p-left",
                "previous_workspace_id": "w",
                "previous_tab_id": "t-a",
            }
        }),
        layout_event(single_pane_layout("t-a", "p-right")),
        layout_event(layout_on_tab("t-b", "p-b", "p-left")),
    ]
}

#[gpui::test]
fn merging_from_a_multi_pane_tab_keeps_the_source_tab(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_merge_view(cx);
    let point = hover_existing_drop(&view, cx, "t-b");
    cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 1, "exactly one pane.move: {moves:?}");
    assert_existing_tab_move(&moves[0], "t-b", "p-b");
    send_events(&fake, multi_pane_merge_into_b_events(), cx);
    view.read_with(cx, |this, _| {
        assert!(this.pane_detaches.is_empty(), "the transfer must settle");
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(
            snapshot.tabs.iter().any(|tab| tab.tab_id == "t-a"),
            "the source tab stays: {:?}",
            snapshot
                .tabs
                .iter()
                .map(|tab| &tab.tab_id)
                .collect::<Vec<_>>()
        );
        assert!(snapshot.tabs.iter().any(|tab| tab.tab_id == "t-b"));
        let source = snapshot.layout_for("t-a").expect("collapsed source layout");
        assert_eq!(source.panes.len(), 1);
        assert_eq!(source.panes[0].pane_id, "p-right");
        assert!(source.splits.is_empty());
        let target = snapshot.layout_for("t-b").expect("merged target layout");
        let ids: Vec<&str> = target
            .panes
            .iter()
            .map(|pane| pane.pane_id.as_str())
            .collect();
        assert_eq!(ids, vec!["p-b", "p-left"]);
        assert_eq!(
            snapshot.pane("p-left").map(|pane| pane.tab_id.as_str()),
            Some("t-b")
        );
        assert_eq!(
            snapshot.pane("p-right").map(|pane| pane.tab_id.as_str()),
            Some("t-a")
        );
    });
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    assert!(fake.requests_for("tab.close").is_empty());
}
