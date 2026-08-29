use super::*;

fn fake_remote_clipboard_image_upload(
    profile: ConnectionProfile,
    extension: String,
    bytes: Vec<u8>,
) -> std::result::Result<String, ocherdr_herdr::HerdrError> {
    assert!(matches!(profile, ConnectionProfile::Ssh { .. }));
    assert_eq!(extension, "png");
    assert_eq!(bytes, b"\x89PNG\r\n\x1a\nremote");
    Ok("/tmp/ocherdr-clipboard-images-501/clipboard-123-456-1/image.png".into())
}

pub(super) fn temp_tab() -> TabInfo {
    TabInfo {
        tab_id: "t-tmp".into(),
        workspace_id: "w".into(),
        number: 9,
        label: "tmp".into(),
        focused: false,
        pane_count: 1,
        agent_status: AgentStatus::Idle,
    }
}

pub(super) fn parked_pane_json(pane_id: &str) -> Value {
    let mut pane = split_pane(pane_id, false);
    pane.tab_id = "t-tmp".into();
    serde_json::to_value(pane).expect("pane json")
}

pub(super) fn single_pane_layout(tab_id: &str, pane_id: &str) -> ocherdr_core::PaneLayout {
    ocherdr_core::PaneLayout {
        workspace_id: "w".into(),
        tab_id: tab_id.into(),
        zoomed: false,
        area: layout_rect(0, 0, 120, 40),
        focused_pane_id: pane_id.into(),
        panes: vec![ocherdr_core::LayoutPane {
            pane_id: pane_id.into(),
            focused: true,
            rect: layout_rect(0, 0, 120, 40),
        }],
        splits: Vec::new(),
    }
}

pub(super) fn layout_event(layout: ocherdr_core::PaneLayout) -> Value {
    json!({
        "event": "layout_updated",
        "data": { "type": "layout_updated", "layout": layout }
    })
}

/// What Herdr broadcasts for step 1 (`p-left` parked in `t-tmp`):
/// `tab.created → pane.moved → layout.updated(t-a) → layout.updated(t-tmp)`.
fn park_events() -> Vec<Value> {
    vec![
        json!({ "event": "tab_created", "data": { "type": "tab_created", "tab": temp_tab() } }),
        json!({
            "event": "pane_moved",
            "data": {
                "type": "pane_moved",
                "pane": parked_pane_json("p-left"),
                "previous_pane_id": "p-left",
                "previous_workspace_id": "w",
                "previous_tab_id": "t-a",
                "created_tab": temp_tab(),
            }
        }),
        layout_event(single_pane_layout("t-a", "p-right")),
        layout_event(single_pane_layout("t-tmp", "p-left")),
    ]
}

/// What Herdr broadcasts for step 2 (`p-left` back beside `p-right` as the
/// second child): `tab.closed → pane.moved → layout.updated(t-a)`.
fn insert_events() -> Vec<Value> {
    let mut back = split_pane("p-left", true);
    back.tab_id = "t-a".into();
    vec![
        json!({
            "event": "tab_closed",
            "data": { "type": "tab_closed", "tab_id": "t-tmp", "workspace_id": "w" }
        }),
        json!({
            "event": "pane_moved",
            "data": {
                "type": "pane_moved",
                "pane": back,
                "previous_pane_id": "p-left",
                "previous_workspace_id": "w",
                "previous_tab_id": "t-tmp",
                "closed_tab_id": "t-tmp",
            }
        }),
        layout_event(two_pane_layout("p-right", "p-left")),
    ]
}

pub(super) fn send_events(fake: &FakeHerdr, events: Vec<Value>, cx: &mut VisualTestContext) {
    for event in events {
        fake.send_event(event);
    }
    thread::sleep(Duration::from_millis(30));
    cx.run_until_parked();
}

fn connect_edge_view(
    cx: &mut TestAppContext,
    script: PaneMoveScript,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events_and_script(two_pane_snapshot(), script);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| {
        this.headless_terminals = true;
        this.pane_edge_relocation = true;
    });
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| {
        this.terminal_surface_bounds = Some(SURFACE);
        assert!(this.edge_drops_enabled(), "flag + capability");
    });
    (fake, view, cx)
}

/// Right edge of `p-right`.
const RIGHT_EDGE: (f32, f32) = (SURFACE.0 + SURFACE.2 - 6., SURFACE.1 + 200.);
/// Left edge of `p-right`.
const LEFT_EDGE: (f32, f32) = (SURFACE.0 + 300. + 6., SURFACE.1 + 200.);

fn assert_park_request(request: &Value) {
    assert_eq!(request["method"], json!("pane.move"));
    assert_eq!(request["params"]["pane_id"], json!("p-left"));
    assert_eq!(
        request["params"]["destination"],
        json!({ "type": "new_tab", "workspace_id": "w" })
    );
    assert_eq!(request["params"]["focus"], json!(false));
}

fn assert_insert_request(request: &Value) {
    assert_eq!(request["method"], json!("pane.move"));
    assert_eq!(
        request["params"]["pane_id"],
        json!("p-left"),
        "pane id comes from the step-1 response"
    );
    assert_eq!(request["params"]["destination"]["type"], json!("tab"));
    assert_eq!(request["params"]["destination"]["tab_id"], json!("t-a"));
    assert_eq!(
        request["params"]["destination"]["target_pane_id"],
        json!("p-right")
    );
    assert_eq!(request["params"]["destination"]["split"], json!("right"));
    let ratio = request["params"]["destination"]["ratio"]
        .as_f64()
        .expect("ratio");
    assert!((ratio - 0.5).abs() < 1e-6, "{ratio}");
    assert_eq!(request["params"]["focus"], json!(true));
}

#[gpui::test]
fn a_right_drop_issues_two_pane_moves_back_to_back_and_settles_without_a_resync(
    cx: &mut TestAppContext,
) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    let snapshots_before = fake.requests_for("session.snapshot").len();
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("plan pending");
        assert_eq!(pending.phase, crate::RelocationPhase::Parking);
        assert!(matches!(
            pending.plan.intent,
            crate::RelocationIntent::Insert {
                edge: ocherdr_core::DropEdge::Right,
                ..
            }
        ));
        assert!(this.tab_relocation_locked("t-a"));
        let predicted = this
            .displayed_pane_fractions(
                this.snapshot.as_ref().and_then(|s| s.layout_for("t-a")),
                "p-left",
                Instant::now(),
                false,
            )
            .expect("predicted rect");
        assert!(
            (predicted.0 - 0.5).abs() < 1e-6,
            "the source is drawn on the right at once: {predicted:?}"
        );
    });
    cx.run_until_parked();

    // Both requests went out before any event was injected, in order, the
    // second built from the first response.
    let methods = fake.request_methods();
    let moves: Vec<usize> = methods
        .iter()
        .enumerate()
        .filter(|(_, m)| *m == "pane.move")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(moves.len(), 2, "{methods:?}");
    assert_eq!(moves[1], moves[0] + 1, "back to back: {methods:?}");
    let requests = fake.requests_for("pane.move");
    assert_park_request(&requests[0]);
    assert_insert_request(&requests[1]);
    assert!(
        fake.requests_for("pane.swap").is_empty(),
        "right needs no swap"
    );
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("still pending");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::Inserting {
                temp_tab_id: "t-tmp".into(),
                moved_pane_id: "p-left".into(),
                responded: true,
                layout_seen: false,
            }
        );
        assert!(this.hidden_tab_ids().contains("t-tmp"));
    });

    // Step 1 events: the temp tab is real in the snapshot but hidden.
    send_events(&fake, park_events(), cx);
    view.read_with(cx, |this, _| {
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp"));
        assert_eq!(
            snapshot.pane("p-left").map(|p| p.tab_id.as_str()),
            Some("t-tmp")
        );
        let tabs: Vec<String> = this
            .chrome_a11y()
            .tabs
            .items
            .iter()
            .map(|row| row.a11y.id.clone())
            .collect();
        assert!(!tabs.contains(&"t-tmp".to_owned()), "hidden: {tabs:?}");
        let rendered: Vec<String> = this
            .rendered_panes_for_tab(snapshot, "t-a")
            .into_iter()
            .map(|pane| pane.pane_id)
            .collect();
        assert_eq!(rendered, vec!["p-right", "p-left"], "plan pane set");
        assert!(this.tab_relocation_locked("t-a"));
        assert!(this.pane_resize_frozen("p-left"), "frozen while parked");
        assert_eq!(
            this.selection.pane_id.as_deref(),
            Some("p-left"),
            "selection pinned to the moved pane"
        );
        let pending = this.pane_relocations.get("t-a").expect("still pending");
        assert!(matches!(
            pending.phase,
            crate::RelocationPhase::Inserting {
                layout_seen: false,
                ..
            }
        ));
    });

    // Step 2 events: the final layout settles the plan.
    send_events(&fake, insert_events(), cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a");
        assert!(
            pending.is_none()
                || matches!(
                    pending.map(|p| &p.phase),
                    Some(crate::RelocationPhase::Settling { .. })
                ),
            "response + matching layout → Settling: {:?}",
            pending.map(|p| &p.phase)
        );
        assert!(
            !this
                .snapshot
                .as_ref()
                .unwrap()
                .tabs
                .iter()
                .any(|tab| tab.tab_id == "t-tmp")
        );
    });
    thread::sleep(Duration::from_millis(220));
    view.update(cx, |this, _| {
        this.expire_pane_motion(Instant::now(), false);
        assert!(!this.tab_relocation_locked("t-a"));
        let layout = this
            .snapshot
            .as_ref()
            .and_then(|s| s.layout_for("t-a"))
            .expect("layout");
        assert_eq!(layout.panes[0].pane_id, "p-right");
        assert_eq!(layout.panes[1].pane_id, "p-left");
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-left"));
    });
    assert_eq!(
        fake.requests_for("session.snapshot").len(),
        snapshots_before,
        "the incremental pane.moved apply never resyncs"
    );
    assert_eq!(fake.requests_for("pane.move").len(), 2);
}

fn visible_tab_ids(view: &crate::OcHerdrView) -> Vec<String> {
    view.chrome_a11y()
        .tabs
        .items
        .iter()
        .map(|row| row.a11y.id.clone())
        .collect()
}

/// Events and responses ride different sockets: `tab.created` for the
/// temporary tab can be applied while the plan is still `Parking`, before
/// the step-1 response names the tab. The strip must hide it at every step
/// of the executor, not only once the response has been applied.
#[gpui::test]
fn the_temporary_tab_is_hidden_before_the_park_response_names_it(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        events_before_park_response: Mutex::new(park_events()),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_edge_view(cx, script);
    drag_left_pane_to(&view, RIGHT_EDGE, cx);

    // Step the executor one task at a time. The fake answers step 1 only
    // after the four step-1 events are on the wire, so the event task and
    // the response continuation are both runnable after the request.
    let mut parking_with_temp_tab = false;
    let mut ticks = 0;
    loop {
        let ran = cx.executor().tick();
        view.read_with(cx, |this, _| {
            assert!(
                !visible_tab_ids(this).iter().any(|id| id == "t-tmp"),
                "tick {ticks}: the temp tab reached the strip"
            );
            let temp_tab_known = this
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp"));
            if temp_tab_known
                && this.pane_relocations.get("t-a").map(|p| &p.phase)
                    == Some(&crate::RelocationPhase::Parking)
            {
                parking_with_temp_tab = true;
                assert!(this.hidden_tab_ids().contains("t-tmp"));
            }
        });
        ticks += 1;
        if !ran {
            if fake.requests_for("pane.move").len() >= 2 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
            assert!(ticks < 2_000, "step 2 never went out");
        }
    }
    assert!(
        parking_with_temp_tab,
        "the race was exercised: the temp tab was in the snapshot while Parking"
    );
    view.update(cx, |this, cx| {
        assert!(matches!(
            this.pane_relocations.get("t-a").map(|p| &p.phase),
            Some(crate::RelocationPhase::Inserting { temp_tab_id, .. }) if temp_tab_id == "t-tmp"
        ));
        assert_eq!(visible_tab_ids(this), vec!["t-a", "t-b", "t-c"]);
        this.cycle_tab(-1, cx);
        assert_eq!(
            this.selection.tab_id.as_deref(),
            Some("t-c"),
            "tab navigation wraps past the hidden tab"
        );
        this.select_tab_number(1, cx);
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });

    send_events(&fake, insert_events(), cx);
    thread::sleep(Duration::from_millis(220));
    view.update(cx, |this, _| {
        this.expire_pane_motion(Instant::now(), false);
        assert!(!this.tab_relocation_locked("t-a"));
        assert_eq!(visible_tab_ids(this), vec!["t-a", "t-b", "t-c"]);
        assert!(this.hidden_tab_ids().is_empty());
    });
}

#[gpui::test]
fn a_left_drop_adds_a_pane_swap_after_the_second_move(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    drag_left_pane_to(&view, LEFT_EDGE, cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("plan pending");
        assert!(matches!(
            pending.plan.intent,
            crate::RelocationIntent::Insert {
                edge: ocherdr_core::DropEdge::Left,
                ..
            }
        ));
        let predicted = this
            .displayed_pane_fractions(
                this.snapshot.as_ref().and_then(|s| s.layout_for("t-a")),
                "p-left",
                Instant::now(),
                false,
            )
            .expect("predicted rect");
        assert!(
            predicted.0.abs() < 1e-6 && (predicted.2 - 0.5).abs() < 1e-6,
            "left of the target: {predicted:?}"
        );
    });
    cx.run_until_parked();
    let methods = fake.request_methods();
    let tail: Vec<&str> = methods
        .iter()
        .filter(|m| m.starts_with("pane."))
        .map(String::as_str)
        .collect();
    assert_eq!(
        tail,
        vec!["pane.move", "pane.move", "pane.swap"],
        "strictly serial, no event in between: {methods:?}"
    );
    let moves = fake.requests_for("pane.move");
    assert_park_request(&moves[0]);
    assert_insert_request(&moves[1]);
    let swaps = fake.requests_for("pane.swap");
    assert_eq!(swaps[0]["params"]["source_pane_id"], json!("p-left"));
    assert_eq!(swaps[0]["params"]["target_pane_id"], json!("p-right"));
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("still pending");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::CorrectingOrder {
                responded: true,
                layout_seen: false,
            }
        );
    });
    send_events(&fake, park_events(), cx);
    send_events(&fake, insert_events(), cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("still pending");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::CorrectingOrder {
                responded: true,
                layout_seen: false,
            },
            "the step-2 layout (source second) is not the landing"
        );
    });
    // The swap's layout: source first.
    send_events(
        &fake,
        vec![layout_event(two_pane_layout("p-left", "p-right"))],
        cx,
    );
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a");
        assert!(
            pending.is_none()
                || matches!(
                    pending.map(|p| &p.phase),
                    Some(crate::RelocationPhase::Settling { .. })
                ),
            "{:?}",
            pending.map(|p| &p.phase)
        );
    });
    assert_eq!(fake.requests_for("pane.move").len(), 2);
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
}

#[gpui::test]
fn a_failed_second_move_parks_the_pane_and_retry_reissues_it(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        insert_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_edge_view(cx, script);
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 2);
    send_events(&fake, park_events(), cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("parked plan");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::Parked {
                temp_tab_id: "t-tmp".into(),
                moved_pane_id: "p-left".into(),
            }
        );
        assert!(this.parked_relocation("t-a").is_some(), "inline notice");
        assert!(!this.tab_relocation_locked("t-a"), "no prediction, no lock");
        assert!(
            this.hidden_tab_ids().is_empty(),
            "the temp tab is shown while parked"
        );
        assert!(
            this.displayed_pane_fractions(
                this.snapshot.as_ref().and_then(|s| s.layout_for("t-a")),
                "p-right",
                Instant::now(),
                false,
            )
            .is_some_and(|rect| (rect.2 - 1.).abs() < 1e-6),
            "authoritative single-pane layout on screen"
        );
    });
    view.update(cx, |this, cx| this.retry_parked_relocation("t-a", cx));
    cx.run_until_parked();
    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 3, "retry re-issues step 2 only");
    assert_insert_request(&moves[2]);
    assert!(fake.requests_for("pane.swap").is_empty());
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("inserting again");
        assert!(matches!(
            pending.phase,
            crate::RelocationPhase::Inserting {
                responded: true,
                ..
            }
        ));
        assert!(this.tab_relocation_locked("t-a"));
    });
}

#[gpui::test]
fn a_failed_first_move_reverts_without_touching_the_selection(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        park_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_edge_view(cx, script);
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    view.read_with(cx, |this, _| assert!(this.tab_relocation_locked("t-a")));
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    view.read_with(cx, |this, _| {
        assert!(this.pane_relocations.is_empty(), "reverted");
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-left"));
    });
}

#[gpui::test]
fn a_foreign_layout_during_an_insert_reverts_to_authority(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 2);
    // Someone split the tab before our move landed.
    let mut layout = two_pane_layout("p-left", "p-right");
    layout.panes.push(ocherdr_core::LayoutPane {
        pane_id: "p-new".into(),
        focused: false,
        rect: layout_rect(60, 20, 60, 20),
    });
    layout.panes[1].rect = layout_rect(60, 0, 60, 20);
    layout.splits.push(ocherdr_core::LayoutSplit {
        id: "split_1_1".into(),
        direction: ocherdr_core::SplitDirection::Down,
        ratio: 0.5,
        rect: layout_rect(60, 0, 60, 40),
    });
    send_events(&fake, vec![layout_event(layout)], cx);
    view.read_with(cx, |this, _| {
        assert!(
            this.pane_relocations.is_empty(),
            "fingerprint mismatch aborts"
        );
        assert!(!this.tab_relocation_locked("t-a"));
    });
}

#[gpui::test]
fn edge_zones_need_both_the_flag_and_the_capability(cx: &mut TestAppContext) {
    // Flag off (default) on a capable Herdr.
    let (fake, view, cx) = connect_two_pane_view(cx);
    view.read_with(cx, |this, _| {
        assert!(this.pane_move_supported());
        assert!(!this.edge_drops_enabled());
    });
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.pane_relocations.is_empty()));
    assert!(fake.requests_for("pane.move").is_empty());
    drop(fake);

    // Flag on, Herdr too old for `pane.move`.
    let mut snapshot = two_pane_snapshot();
    snapshot.version = "0.6.0".into();
    snapshot.protocol = 13;
    let fake = FakeHerdr::snapshot_with_live_events(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| {
        this.headless_terminals = true;
        this.pane_edge_relocation = true;
    });
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| this.terminal_surface_bounds = Some(SURFACE));
    view.read_with(cx, |this, _| {
        assert!(!this.pane_move_supported());
        assert!(!this.edge_drops_enabled());
    });
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.pane_relocations.is_empty());
        assert!(this.pane_drag_return.is_some(), "the preview returned home");
    });
    assert!(fake.requests_for("pane.move").is_empty());
}

fn key(name: &str, control: bool) -> gpui::KeyDownEvent {
    gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control,
                ..Default::default()
            },
            key: name.into(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    }
}

fn measure_two_terminal_bodies(view: &Entity<OcHerdrView>, cx: &mut VisualTestContext) {
    view.update_in(cx, |this, window, cx| {
        for (pane_id, x) in [("p-left", 0.), ("p-right", 400.)] {
            this.sync_measured_pane_body(
                pane_id,
                gpui::Bounds {
                    origin: gpui::point(gpui::px(x), gpui::px(0.)),
                    size: gpui::size(gpui::px(400.), gpui::px(300.)),
                },
                window,
                cx,
            );
        }
        // The production scheduler spreads one mount over each wall-clock
        // turn. GPUI's parked test executor does not advance that timer, so
        // drive the same two batches explicitly after supplying geometry.
        for _ in 0..2 {
            this.pane_mount_scheduled = false;
            this.ensure_session_terminals(cx);
        }
    });
    cx.run_until_parked();
}

#[gpui::test]
fn notification_copy_button_wins_over_the_terminal_behind_it(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_with_live_events(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(gpui::size(gpui::px(700.), gpui::px(500.)));
    view.update(cx, |this, cx| {
        this.notifications.update(cx, |host, cx| {
            host.notify(
                ochub_ui::notifications::NotificationRequest::new(
                    ochub_ui::notifications::NotificationLevel::Error,
                    "Copy title",
                )
                .message("Copy detail")
                .timeout(Duration::from_secs(60)),
                cx,
            );
            cx.notify();
        });
    });
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
    cx.run_until_parked();

    let copy = cx
        .debug_bounds("notification-copy-1")
        .expect("notification copy button");
    let copy_center = copy.center();
    let pointer = (f32::from(copy_center.x), f32::from(copy_center.y));
    view.read_with(cx, |this, _| {
        assert!(
            ["p-left", "p-right"].into_iter().any(|pane_id| {
                this.pane(pane_id).is_some_and(|runtime| {
                    let (x, y, width, height) = runtime.body_bounds;
                    pointer.0 >= x
                        && pointer.0 <= x + width
                        && pointer.1 >= y
                        && pointer.1 <= y + height
                })
            }),
            "the fixture must put a live terminal below the notification button"
        );
    });
    cx.simulate_click(copy_center, gpui::Modifiers::default());

    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("Copy title\nCopy detail".into()),
        "the toast must intercept the click instead of starting a terminal selection"
    );
}

/// Real Ghostty surfaces (not `headless_terminals`): a key press takes control,
/// then libghostty encodes it and sends its bytes to that pane's stream.
#[gpui::test]
fn a_key_press_reaches_only_the_selected_panes_stream_through_ghostty(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_with_live_events(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);
    view.update_in(cx, |this, window, cx| {
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-left"));
        assert!(
            this.pane("p-left").is_some(),
            "the visible tab spawns a Ghostty surface"
        );
        assert!(this.pane("p-right").is_some());
        let mut shift_enter = key("enter", false);
        shift_enter.keystroke.modifiers.shift = true;
        shift_enter.keystroke.key_char = Some("\n".into());
        this.send_key(&shift_enter, window, cx);
    });
    // Ghostty writes to the pty from its IO thread; production drains the
    // queue on every frame and event poll, the test pumps it by hand.
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.terminal_inputs("p-left").is_empty() {
        assert!(Instant::now() < deadline, "Ghostty never wrote the key");
        view.update(cx, |this, _| this.pump_terminal_input());
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(20));
    }
    // Ghostty's legacy encoding of Shift+Enter, base64 as Herdr receives it.
    assert_eq!(
        fake.terminal_inputs("p-left"),
        vec!["G1syNzsyOzEzfg==".to_owned()]
    );
    assert!(fake.terminal_inputs("p-right").is_empty());

    // PixPin and Finder expose an image as a local file path. Keep the file
    // alive while both local pass-through and remote upload exercise it.
    let clipboard_dir = tempfile::TempDir::new().expect("clipboard directory");
    let clipboard_image = clipboard_dir.path().join("PixPin capture.png");
    std::fs::write(&clipboard_image, b"\x89PNG\r\n\x1a\nremote").expect("clipboard image");

    // On a local profile the file-backed image must still reach the agent as
    // the original Cmd+V through its Kitty keyboard protocol.
    view.update_in(cx, |this, window, cx| {
        let runtime = this.pane_mut("p-left").expect("selected terminal");
        runtime.terminal.apply_frame(b"\x1b[>1u", false);
        cx.write_to_clipboard(gpui::ClipboardItem {
            entries: vec![gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                vec![clipboard_image.clone()].into(),
            ))],
        });
        let mut cmd_v = key("v", false);
        cmd_v.keystroke.modifiers.platform = true;
        this.send_key(&cmd_v, window, cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.terminal_inputs("p-left").len() < 2 {
        assert!(
            Instant::now() < deadline,
            "Ghostty never forwarded image-only Cmd+V"
        );
        view.update(cx, |this, _| this.pump_terminal_input());
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fake.terminal_inputs("p-left"),
        vec!["G1syNzsyOzEzfg==".to_owned(), "G1sxMTg7OXU=".to_owned()]
    );

    // The same file-backed Cmd+V on an SSH profile reads the local file in the
    // background, uploads outside Herdr, then pastes the returned remote path.
    view.update_in(cx, |this, window, cx| {
        this.profiles[0] = ConnectionProfile::Ssh {
            // Keep the fixture's existing per-pane child stream while making
            // routing remote. Production profile ids are unique; this test id
            // intentionally matches Local's synthetic `local` owner.
            id: "local".into(),
            label: "Remote".into(),
            destination: "unused.example".into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        };
        this.remote_clipboard_image_upload = fake_remote_clipboard_image_upload;
        cx.write_to_clipboard(gpui::ClipboardItem {
            entries: vec![gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                vec![clipboard_image.clone()].into(),
            ))],
        });
        assert!(matches!(
            this.current_profile(),
            ConnectionProfile::Ssh { .. }
        ));
        let mut cmd_v = key("v", false);
        cmd_v.keystroke.modifiers.platform = true;
        this.send_key(&cmd_v, window, cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.terminal_inputs("p-left").len() < 3 {
        assert!(
            Instant::now() < deadline,
            "uploaded remote image path never reached the pane stream"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(20));
    }

    // Herdr users also paste with Ctrl+V. It must take the identical remote
    // upload path instead of being encoded as a literal control character.
    view.update_in(cx, |this, window, cx| {
        this.send_key(&key("v", true), window, cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.terminal_inputs("p-left").len() < 4 {
        assert!(
            Instant::now() < deadline,
            "Ctrl+V remote image path never reached the pane stream"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fake.terminal_inputs("p-left"),
        vec![
            "G1syNzsyOzEzfg==".to_owned(),
            "G1sxMTg7OXU=".to_owned(),
            "L3RtcC9vY2hlcmRyLWNsaXBib2FyZC1pbWFnZXMtNTAxL2NsaXBib2FyZC0xMjMtNDU2LTEvaW1hZ2UucG5n"
                .to_owned(),
            "L3RtcC9vY2hlcmRyLWNsaXBib2FyZC1pbWFnZXMtNTAxL2NsaXBib2FyZC0xMjMtNDU2LTEvaW1hZ2UucG5n"
                .to_owned(),
        ],
        "remote image paste must not leak Cmd+V or Ctrl+V into the PTY"
    );
}

#[gpui::test]
fn wheel_interaction_controls_each_hovered_pane_without_changing_focus(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_with_live_events(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);

    view.update(cx, |this, cx| {
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-left"));
        assert_eq!(this.pane("p-left").unwrap().mode, TerminalMode::Observe);
        assert_eq!(this.pane("p-right").unwrap().mode, TerminalMode::Observe);
        let wheel = gpui::ScrollWheelEvent {
            position: gpui::point(gpui::px(0.), gpui::px(0.)),
            delta: gpui::ScrollDelta::Lines(gpui::point(0., 3.)),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        };

        this.scroll_pane("p-left", &wheel, cx);
        assert_eq!(
            this.pane("p-left").unwrap().mode,
            TerminalMode::ControlTakeover
        );
        assert_eq!(this.pane("p-right").unwrap().mode, TerminalMode::Observe);

        this.scroll_pane("p-right", &wheel, cx);
        assert_eq!(
            this.pane("p-left").unwrap().mode,
            TerminalMode::ControlTakeover
        );
        assert_eq!(
            this.pane("p-right").unwrap().mode,
            TerminalMode::ControlTakeover
        );
        assert_eq!(
            this.selection.pane_id.as_deref(),
            Some("p-left"),
            "wheel promotion must not move keyboard focus"
        );
        let controls = &this.session_panes.as_ref().unwrap().controls;
        assert_eq!(controls.len(), 2);
        assert_eq!(controls.get("p-left"), Some(&TerminalMode::ControlTakeover));
        assert_eq!(
            controls.get("p-right"),
            Some(&TerminalMode::ControlTakeover)
        );
    });
}

#[gpui::test]
fn keyboard_move_mode_commits_through_the_same_plan_machinery(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    view.update_in(cx, |this, window, cx| {
        assert!(this.handle_app_shortcut(&key("b", true), window, cx));
        assert!(this.prefix_pending);
        assert!(this.handle_app_shortcut(&key("m", false), window, cx));
        let mode = this.pane_keyboard_move.as_ref().expect("move mode");
        assert_eq!(mode.pane_id, "p-left");
        assert!(mode.target.is_none());
        // Right arrow picks the neighbour, centre zone.
        assert!(this.handle_app_shortcut(&key("right", false), window, cx));
        let mode = this.pane_keyboard_move.as_ref().expect("move mode");
        let target = mode.target.as_ref().expect("target");
        assert_eq!(target.target_pane_id, "p-right");
        assert_eq!(target.zone, ocherdr_core::DropZone::Center);
        // Tab cycles to the left edge (flag + capability are on).
        assert!(this.handle_app_shortcut(&key("tab", false), window, cx));
        let zone = this
            .pane_keyboard_move
            .as_ref()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .zone;
        assert_eq!(zone, ocherdr_core::DropZone::Left);
        // Esc cancels without a request.
        assert!(this.handle_app_shortcut(&key("escape", false), window, cx));
        assert!(this.pane_keyboard_move.is_none());
        assert!(this.pane_relocations.is_empty());
        // Again, and confirm a left insert.
        assert!(this.handle_app_shortcut(&key("b", true), window, cx));
        assert!(this.handle_app_shortcut(&key("m", false), window, cx));
        assert!(this.handle_app_shortcut(&key("right", false), window, cx));
        assert!(this.handle_app_shortcut(&key("tab", false), window, cx));
        assert!(this.handle_app_shortcut(&key("enter", false), window, cx));
        assert!(this.pane_keyboard_move.is_none());
        let pending = this
            .pane_relocations
            .get("t-a")
            .expect("plan from the keyboard");
        assert!(matches!(
            pending.plan.intent,
            crate::RelocationIntent::Insert {
                edge: ocherdr_core::DropEdge::Left,
                ..
            }
        ));
    });
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 2);
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
}

#[gpui::test]
fn a_disconnect_while_parked_restores_the_notice_from_the_reconnect_snapshot(
    cx: &mut TestAppContext,
) {
    let script = PaneMoveScript {
        insert_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_edge_view(cx, script);
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    send_events(&fake, park_events(), cx);
    view.update(cx, |this, cx| {
        assert!(this.parked_relocation("t-a").is_some());
        // The connection drops: local state is abandoned, the parked pane
        // is remembered.
        this.abort_pane_relocations_for_disconnect();
        assert!(this.pane_relocations.is_empty());
        assert!(this.parked_recovery.is_some());
        // Reconnect snapshot still shows the pane in the temporary tab.
        this.restore_parked_recovery(cx);
        let pending = this.parked_relocation("t-a").expect("parked notice back");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::Parked {
                temp_tab_id: "t-tmp".into(),
                moved_pane_id: "p-left".into(),
            }
        );
        assert!(this.parked_recovery.is_none());
        // A snapshot where the pane already left the temp tab offers nothing.
        this.abort_pane_relocations_for_disconnect();
        this.snapshot = Some(two_pane_snapshot());
        this.restore_parked_recovery(cx);
        assert!(this.pane_relocations.is_empty());
    });
}
