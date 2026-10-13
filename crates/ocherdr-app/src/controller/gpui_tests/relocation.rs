use super::*;
#[cfg(target_os = "macos")]
use crate::Overlay;
#[cfg(target_os = "macos")]
use ocherdr_herdr::TerminalNotificationKind;
#[cfg(target_os = "macos")]
use ocherdr_terminal::SurfaceMouseButton;

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

/// What Herdr's hierarchy looks like after step 1 (`p-left` parked in
/// `t-tmp`): the temp tab exists, both tabs hold one pane each.
pub(super) fn park_state(snapshot: &mut HierarchySnapshot) {
    snapshot.tabs.push(temp_tab());
    if let Some(pane) = snapshot
        .panes
        .iter_mut()
        .find(|pane| pane.pane_id == "p-left")
    {
        pane.tab_id = "t-tmp".into();
        pane.focused = false;
    }
    snapshot.layouts = vec![
        single_pane_layout("t-a", "p-right"),
        single_pane_layout("t-tmp", "p-left"),
    ];
}

/// After step 2 (`p-left` back beside `p-right` as the second child): the
/// temporary tab is gone and `t-a` holds both panes.
pub(super) fn insert_state(snapshot: &mut HierarchySnapshot) {
    snapshot.tabs.retain(|tab| tab.tab_id != "t-tmp");
    if let Some(pane) = snapshot
        .panes
        .iter_mut()
        .find(|pane| pane.pane_id == "p-left")
    {
        pane.tab_id = "t-a".into();
        pane.focused = true;
    }
    snapshot.layouts = vec![two_pane_layout("p-right", "p-left")];
}

/// After the corrective swap: `p-left` leads the tab.
pub(super) fn swap_state(snapshot: &mut HierarchySnapshot) {
    snapshot.layouts = vec![two_pane_layout("p-left", "p-right")];
}

fn connect_edge_view(
    cx: &mut TestAppContext,
    script: PaneMoveScript,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::serving_with_script(two_pane_snapshot(), script);
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
fn a_right_drop_issues_two_pane_moves_back_to_back_and_settles(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
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

    // Step 1 state: the temp tab is real in the snapshot but hidden.
    push_state(&fake, &view, cx, park_state, |snapshot| {
        snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp")
    });
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

    // Step 2 state: the final layout settles the plan.
    push_state(&fake, &view, cx, insert_state, |snapshot| {
        snapshot.tabs.iter().all(|tab| tab.tab_id != "t-tmp")
            && snapshot
                .layout_for("t-a")
                .is_some_and(|layout| layout.panes.len() == 2)
    });
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

/// The snapshot push and the response ride different channels: the push for
/// the temporary tab can be applied while the plan is still `Parking`, before
/// the step-1 response names the tab. The strip must hide it at every step
/// of the executor, not only once the response has been applied.
#[gpui::test]
fn the_temporary_tab_is_hidden_before_the_park_response_names_it(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    apply_snapshot_before_response(&fake, &view, park_state, cx);
    view.read_with(cx, |this, _| {
        assert_eq!(
            this.pane_relocations.get("t-a").map(|p| &p.phase),
            Some(&crate::RelocationPhase::Parking)
        );
        assert!(
            this.snapshot
                .as_ref()
                .unwrap()
                .tabs
                .iter()
                .any(|tab| tab.tab_id == "t-tmp")
        );
        assert!(this.hidden_tab_ids().contains("t-tmp"));
        assert!(!visible_tab_ids(this).iter().any(|id| id == "t-tmp"));
    });
    cx.run_until_parked();
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

    push_state(&fake, &view, cx, insert_state, |snapshot| {
        snapshot.tabs.iter().all(|tab| tab.tab_id != "t-tmp")
            && snapshot
                .layout_for("t-a")
                .is_some_and(|layout| layout.panes.len() == 2)
    });
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
    push_state(&fake, &view, cx, park_state, |snapshot| {
        snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp")
    });
    push_state(&fake, &view, cx, insert_state, |snapshot| {
        snapshot.tabs.iter().all(|tab| tab.tab_id != "t-tmp")
            && snapshot
                .layout_for("t-a")
                .is_some_and(|layout| layout.panes.len() == 2)
    });
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
    push_state(&fake, &view, cx, swap_state, |snapshot| {
        snapshot
            .layout_for("t-a")
            .is_some_and(|layout| layout.panes[0].pane_id == "p-left")
    });
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
    push_state(&fake, &view, cx, park_state, |snapshot| {
        snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp")
    });
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
    push_state(
        &fake,
        &view,
        cx,
        |snapshot| {
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
            snapshot.layouts = vec![layout];
        },
        |snapshot| {
            snapshot
                .layout_for("t-a")
                .is_some_and(|layout| layout.panes.len() == 3)
        },
    );
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

    // Flag on, but no connection: the capability is off.
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| {
        this.headless_terminals = true;
        this.pane_edge_relocation = true;
        this.snapshot = Some(two_pane_snapshot());
        this.selection = Selection {
            connection_id: "local".into(),
            session_name: Some("alpha".into()),
            workspace_id: Some("w".into()),
            tab_id: Some("t-a".into()),
            pane_id: Some("p-left".into()),
        };
        this.terminal_surface_bounds = Some(SURFACE);
        assert!(!this.pane_move_supported());
        assert!(!this.edge_drops_enabled());
    });
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.pane_relocations.is_empty());
        assert!(this.pane_drag_return.is_some(), "the preview returned home");
    });
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

fn pane_test_point(
    view: &Entity<OcHerdrView>,
    cx: &VisualTestContext,
) -> gpui::Point<gpui::Pixels> {
    view.read_with(cx, |this, _| {
        let (x, y, width, height) = this.pane("p-left").expect("left pane").body_bounds;
        gpui::point(gpui::px(x + width * 0.4), gpui::px(y + height * 0.4))
    })
}

/// Wait until the endpoint records a mouse event of `kind` for `pane_id`.
/// The input lane runs on the endpoint worker thread, so pump the executor
/// and yield real time while it catches up.
#[cfg(target_os = "macos")]
fn wait_for_mouse_input(
    fake: &FakeHerdr,
    pane_id: &str,
    kind: ep::ClientMouseKind,
    cx: &mut VisualTestContext,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let seen = fake.pane_inputs(pane_id).iter().any(|event| {
            matches!(event, ep::ClientPaneInputEvent::Mouse { kind: reported, .. } if *reported == kind)
        });
        if seen {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "mouse {kind:?} never reached {pane_id}: {:?}",
            fake.pane_inputs(pane_id)
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }
}

/// Mouse-capture state arrives on the endpoint's surface frame. A click must
/// reach the endpoint as soon as it happens; relying on a later pointer or
/// key event to flush left a click's release (and a quick click entirely)
/// stranded until something unrelated happened in the pane.
#[gpui::test]
#[cfg(target_os = "macos")]
fn mouse_click_is_forwarded_without_a_follow_up_ui_event(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);
    push_surface_frame(
        &view,
        surface_frame(vec![
            surface_pane("p-left", (0, 0, 40, 30), true, true, false),
            surface_pane("p-right", (40, 0, 40, 30), false, false, false),
        ]),
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(
            this.pane("p-left")
                .expect("agent terminal")
                .terminal
                .mouse_captured()
        );
    });

    let point = pane_test_point(&view, cx);
    let left_down = gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: point,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    };
    let left_up = gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position: point,
        modifiers: Default::default(),
        click_count: 1,
    };
    view.update_in(cx, |this, window, cx| {
        this.pane_mouse_down("p-left".into(), &left_down, window, cx);
        this.pane_mouse_up(&left_up, window, cx);
    });

    // Deliberately no extra pointer traffic: the click itself must forward.
    let has_report = |kind: ep::ClientMouseKind| {
        fake.pane_inputs("p-left").iter().any(|event| {
            matches!(event, ep::ClientPaneInputEvent::Mouse { kind: reported, .. } if *reported == kind)
        })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(has_report(ep::ClientMouseKind::Down(ep::ClientMouseButton::Left))
        && has_report(ep::ClientMouseKind::Up(ep::ClientMouseButton::Left)))
    {
        assert!(
            Instant::now() < deadline,
            "listener never forwarded the click: {:?}",
            fake.pane_inputs("p-left")
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(20));
    }
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn auto_mouse_mode_uses_herdr_authority_instead_of_guessing_from_agent(cx: &mut TestAppContext) {
    let mut snapshot = two_pane_snapshot();
    let agent = snapshot
        .panes
        .iter_mut()
        .find(|pane| pane.pane_id == "p-left")
        .expect("left pane");
    agent.agent = Some("codex".into());
    agent.display_agent = Some("Codex".into());
    let fake = FakeHerdr::serving(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);

    view.read_with(cx, |this, _| {
        let runtime = this.pane("p-left").expect("agent terminal");
        assert!(runtime.mouse_capture.is_none());
        assert!(
            !runtime.terminal.mouse_captured(),
            "an agent label is not evidence that its terminal currently captures the mouse"
        );
    });

    let point = pane_test_point(&view, cx);
    let mut left_down = gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: point,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    };
    let mut left_up = gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position: point,
        modifiers: Default::default(),
        click_count: 1,
    };
    let drag_point = point + gpui::point(gpui::px(20.), gpui::px(10.));
    let drag_move = gpui::MouseMoveEvent {
        position: drag_point,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: Default::default(),
    };
    let drag_up = gpui::MouseUpEvent {
        position: drag_point,
        ..left_up
    };
    view.update_in(cx, |this, window, cx| {
        this.pane_mouse_down("p-left".into(), &left_down, window, cx);
        assert!(matches!(
            this.surface_drag,
            crate::SurfaceDrag::Text {
                captured: false,
                ..
            }
        ));
        this.pane_mouse_move(&drag_move, window, cx);
        this.pane_mouse_up(&drag_up, window, cx);
    });
    assert!(
        fake.pane_inputs("p-left").is_empty(),
        "unknown mouse state must remain a local text selection"
    );

    // The surface frame carries the pane's authoritative capture flag.
    push_surface_frame(
        &view,
        surface_frame(vec![
            surface_pane("p-left", (0, 0, 40, 30), true, true, false),
            surface_pane("p-right", (40, 0, 40, 30), false, false, false),
        ]),
        cx,
    );
    view.read_with(cx, |this, _| {
        let runtime = this.pane("p-left").expect("agent terminal");
        assert_eq!(runtime.mouse_capture, Some((true, false)));
        assert!(runtime.terminal.mouse_captured());
    });

    // Applying the first surface permits normal layout and can move the pane
    // away from the synthetic bounds used to create its native terminal.
    left_down.position = pane_test_point(&view, cx);
    left_up.position = left_down.position;

    view.update_in(cx, |this, window, cx| {
        this.pane_mouse_down("p-left".into(), &left_down, window, cx);
        assert!(matches!(
            this.surface_drag,
            crate::SurfaceDrag::Text { captured: true, .. }
        ));
        this.pane_mouse_up(&left_up, window, cx);
    });
    wait_for_mouse_input(
        &fake,
        "p-left",
        ep::ClientMouseKind::Down(ep::ClientMouseButton::Left),
        cx,
    );
    wait_for_mouse_input(
        &fake,
        "p-left",
        ep::ClientMouseKind::Up(ep::ClientMouseButton::Left),
        cx,
    );

    // A mouse-aware TUI also gets hover motion without a button held.
    let hover = gpui::MouseMoveEvent {
        position: left_down.position + gpui::point(gpui::px(10.), gpui::px(5.)),
        pressed_button: None,
        modifiers: Default::default(),
    };
    view.update_in(cx, |this, window, cx| {
        this.pane_surface_mouse_move("p-left", &hover, window, cx);
    });
    wait_for_mouse_input(&fake, "p-left", ep::ClientMouseKind::Moved, cx);

    let right_down = gpui::MouseDownEvent {
        button: gpui::MouseButton::Right,
        ..left_down
    };
    let right_up = gpui::MouseUpEvent {
        button: gpui::MouseButton::Right,
        ..left_up
    };
    view.update_in(cx, |this, window, cx| {
        assert!(this.pane_aux_mouse_down(
            "p-left".into(),
            SurfaceMouseButton::Right,
            &right_down,
            window,
            cx,
        ));
        assert!(matches!(this.overlay, Overlay::None));
        // GPUI can deliver the out-of-pane release through the pane below the
        // pointer first. That pane must not consume a right click which began
        // on p-left.
        this.pane_aux_mouse_up("p-right", SurfaceMouseButton::Right, &right_up, window, cx);
        assert!(matches!(
            this.aux_mouse_drag.as_ref(),
            Some(crate::AuxMouseDrag { pane_id, .. }) if pane_id == "p-left"
        ));
        this.pane_aux_mouse_up("p-left", SurfaceMouseButton::Right, &right_up, window, cx);
    });
    wait_for_mouse_input(
        &fake,
        "p-left",
        ep::ClientMouseKind::Down(ep::ClientMouseButton::Right),
        cx,
    );
    wait_for_mouse_input(
        &fake,
        "p-left",
        ep::ClientMouseKind::Up(ep::ClientMouseButton::Right),
        cx,
    );

    let before_shift = fake.pane_inputs("p-left").len();
    let shift_down = gpui::MouseDownEvent {
        modifiers: gpui::Modifiers {
            shift: true,
            ..Default::default()
        },
        ..left_down
    };
    let shift_up = gpui::MouseUpEvent { ..left_up };
    view.update_in(cx, |this, window, cx| {
        assert!(
            matches!(this.surface_drag, crate::SurfaceDrag::Idle),
            "previous click did not finish: {:?}",
            this.surface_drag
        );
        this.pane_mouse_down("p-left".into(), &shift_down, window, cx);
        assert!(
            matches!(
                this.surface_drag,
                crate::SurfaceDrag::Text {
                    captured: false,
                    ..
                }
            ),
            "Shift click did not start selection: {:?}",
            this.surface_drag
        );
        // Shift may be released before the mouse button. The press stays a
        // local selection; it must not turn the release into a TUI report.
        this.pane_mouse_up(&shift_up, window, cx);
    });
    assert_eq!(
        fake.pane_inputs("p-left").len(),
        before_shift,
        "Shift bypasses mouse capture and remains local text selection"
    );

    // Capture off again: the pane goes back to selection-first.
    push_surface_frame(
        &view,
        surface_frame(vec![
            surface_pane("p-left", (0, 0, 40, 30), true, false, false),
            surface_pane("p-right", (40, 0, 40, 30), false, false, false),
        ]),
        cx,
    );
    view.update_in(cx, |this, window, cx| {
        assert_eq!(
            this.pane("p-left").unwrap().mouse_capture,
            Some((false, false))
        );
        assert!(!this.pane_aux_mouse_down(
            "p-left".into(),
            SurfaceMouseButton::Right,
            &right_down,
            window,
            cx,
        ));
    });
}

#[gpui::test]
fn tab_switch_retains_the_hidden_native_surface_and_stream(cx: &mut TestAppContext) {
    let mut snapshot = pane_move_capable_snapshot();
    snapshot.version = "0.9.4".into();
    snapshot.protocol = 22;
    snapshot.tabs.truncate(2);
    snapshot.tabs[0].pane_count = 1;
    snapshot.tabs[1].pane_count = 1;
    snapshot.workspaces[0].tab_count = 2;
    snapshot.workspaces[0].pane_count = 2;
    let pane_a = split_pane("p-a", true);
    let mut pane_b = split_pane("p-b", false);
    pane_b.tab_id = "t-b".into();
    snapshot.focused_pane_id = Some("p-a".into());
    snapshot.panes = vec![pane_a, pane_b];
    snapshot.layouts = vec![
        single_pane_layout("t-a", "p-a"),
        single_pane_layout("t-b", "p-b"),
    ];

    let fake = FakeHerdr::serving(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    assert_eq!(
        fake.endpoint_connections(),
        1,
        "one endpoint connection carries every pane"
    );
    view.update_in(cx, |this, window, cx| {
        this.sync_measured_pane_body(
            "p-a",
            gpui::Bounds {
                origin: gpui::point(gpui::px(0.), gpui::px(0.)),
                size: gpui::size(gpui::px(500.), gpui::px(320.)),
            },
            window,
            cx,
        );
        this.pane_mount_scheduled = false;
        this.ensure_session_terminals(cx);
    });
    push_surface_frame(
        &view,
        surface_frame(vec![surface_pane(
            "p-a",
            (0, 0, 50, 32),
            true,
            false,
            false,
        )]),
        cx,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !view.read_with(cx, |this, _| {
        this.pane("p-a")
            .is_some_and(|runtime| runtime.frame.is_some())
    }) {
        assert!(
            Instant::now() < deadline,
            "native renderer never delivered its first frame"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }
    view.read_with(cx, |this, _| {
        assert!(
            this.pane("p-a")
                .is_some_and(|runtime| runtime.frame.is_some()),
            "first tab painted"
        );
    });
    let original_context = view.read_with(cx, |this, _| this.pane("p-a").unwrap().frame_context);

    view.update(cx, |this, cx| {
        this.selection.tab_id = Some("t-b".into());
        this.selection.pane_id = Some("p-b".into());
        this.ensure_session_terminals(cx);
        let hidden = this.pane("p-a").expect("hidden surface stays cached");
        assert!(hidden.frame.is_some());
        assert_eq!(hidden.frame_context, original_context);
    });
    view.update_in(cx, |this, window, cx| {
        this.sync_measured_pane_body(
            "p-b",
            gpui::Bounds {
                origin: gpui::point(gpui::px(0.), gpui::px(0.)),
                size: gpui::size(gpui::px(500.), gpui::px(320.)),
            },
            window,
            cx,
        );
        this.pane_mount_scheduled = false;
        this.ensure_session_terminals(cx);
    });
    push_surface_frame(
        &view,
        surface_frame(vec![surface_pane(
            "p-b",
            (0, 0, 50, 32),
            true,
            false,
            false,
        )]),
        cx,
    );

    assert_eq!(
        fake.endpoint_connections(),
        1,
        "hiding a pane must not replace the session's endpoint connection"
    );

    view.update(cx, |this, cx| {
        this.selection.tab_id = Some("t-a".into());
        this.selection.pane_id = Some("p-a".into());
        this.ensure_session_terminals(cx);
        let restored = this
            .pane("p-a")
            .expect("cached surface restores immediately");
        assert!(restored.frame.is_some());
        assert_eq!(restored.frame_context, original_context);
    });
    assert_eq!(
        fake.endpoint_connections(),
        1,
        "switching away and back reuses the original endpoint connection"
    );
}

#[gpui::test]
fn host_switch_parks_terminal_stream_until_explicit_disconnect(cx: &mut TestAppContext) {
    let fake_a = FakeHerdr::serving(two_pane_snapshot());
    let fake_b = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake_a, cx);
    measure_two_terminal_bodies(&view, cx);
    assert_eq!(fake_a.endpoint_connections(), 1, "host A endpoint attached");

    view.update(cx, |this, cx| {
        let local_owner = this.current_session_key().expect("local session owner");
        let local_background_pane = this
            .snapshot
            .as_mut()
            .and_then(|snapshot| {
                snapshot
                    .panes
                    .iter_mut()
                    .find(|pane| pane.pane_id == "p-right")
            })
            .expect("local background pane");
        local_background_pane.agent = Some("codex".into());
        local_background_pane.display_agent = Some("Codex".into());
        local_background_pane.agent_status = AgentStatus::Working;
        let profile_b = ConnectionProfile::Ssh {
            id: "remote-test".into(),
            label: "Remote test".into(),
            destination: "unused".into(),
            port: None,
            identity_file: None,
            proxy_jump: None,
            herdr_path: "herdr".into(),
        };
        let session_b = SessionSummary {
            name: "beta".into(),
            running: true,
            socket_path: fake_b.socket_path(),
            session_dir: fake_b._dir.path().join("beta"),
            default: false,
        };
        let connection_b = SessionConnection::connect(&ConnectionProfile::default(), &session_b)
            .expect("connect host B fake");
        let snapshot_b = two_pane_snapshot();
        let profile_b_index = this.profiles.len();
        this.profiles.push(profile_b);
        this.parked_hosts.insert(
            "remote-test".into(),
            crate::ParkedHostRuntime {
                sessions: vec![session_b],
                session_index: Some(0),
                connection: connection_b,
                event_stream: EventStreamState::Idle,
                snapshot: Some(snapshot_b),
                selection: Selection {
                    connection_id: "remote-test".into(),
                    session_name: Some("beta".into()),
                    workspace_id: Some("w".into()),
                    tab_id: Some("t-left".into()),
                    pane_id: Some("p-left".into()),
                },
                session_panes: None,
                pane_viewports: HashMap::new(),
                snapshot_refreshing: false,
                snapshot_refresh_pending: false,
            },
        );

        this.select_profile(profile_b_index, cx);
        assert!(this.parked_hosts.contains_key("local"));
        assert!(
            this.parked_hosts["local"]
                .session_panes
                .as_ref()
                .is_some_and(|session| session.endpoint.is_some()),
            "switching machines must retain the established endpoint stream"
        );
        assert_eq!(fake_a.endpoint_disconnects(), 0);
        assert!(this.apply_endpoint_batch(
            &local_owner,
            vec![Ok(EndpointEvent::SemanticNotification(
                ep::SemanticNotification {
                    kind: ep::SemanticNotificationKind::Finished,
                    title: String::new(),
                    body: None,
                    sound: None,
                    agent: Some("Codex".into()),
                    workspace_id: Some("w".into()),
                    tab_id: Some("t-a".into()),
                    pane_id: Some("p-right".into()),
                    position: None,
                },
            ))],
            cx,
        ));

        this.select_profile(0, cx);
        assert_eq!(this.current_profile().id(), "local");
        assert_eq!(
            fake_a.endpoint_connections(),
            1,
            "the parked endpoint connection is reused"
        );
        assert_eq!(fake_a.endpoint_disconnects(), 0);

        this.disconnect_host("local", cx);
    });
    assert_eq!(
        cx.shown_system_notifications().len(),
        1,
        "an agent on the parked machine still notifies locally"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while fake_a.endpoint_disconnects() == 0 {
        assert!(
            Instant::now() < deadline,
            "explicit disconnect never detached host A"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fake_a.endpoint_disconnects(), 1);
}

#[gpui::test]
fn host_switcher_red_x_disconnects_without_switching_hosts(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, cx| this.toggle_host_switcher(cx));
    cx.run_until_parked();

    let disconnect = cx
        .debug_bounds("disconnect-host-local")
        .expect("connected host row exposes its explicit disconnect button")
        .center();
    cx.simulate_click(disconnect, gpui::Modifiers::default());
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert_eq!(this.current_profile().id(), "local");
        assert!(this.connection.is_none());
        assert_eq!(
            this.host_connection_state("local"),
            crate::HostConnectionState::Disconnected
        );
    });
}

#[gpui::test]
fn notification_detail_copy_button_wins_over_the_terminal_behind_it(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(gpui::size(gpui::px(700.), gpui::px(500.)));
    view.update(cx, |this, cx| {
        this.notifications.update(cx, |host, cx| {
            let notification = host.notify(
                ochub_ui::notifications::NotificationRequest::new(
                    ochub_ui::notifications::NotificationLevel::Error,
                    "Copy title",
                )
                .message("Copy detail")
                .timeout(Duration::from_secs(60)),
                cx,
            );
            host.show_details(notification, cx);
        });
    });
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
    cx.run_until_parked();

    let copy = cx
        .debug_bounds("notification-copy")
        .expect("notification detail copy button");
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
        "the notification center must intercept the click instead of starting a terminal selection"
    );
}

/// Real Ghostty surfaces (not `headless_terminals`): a key press takes
/// control, then the endpoint carries it to that pane as a semantic input
/// event — and only that pane.
#[gpui::test]
#[cfg(target_os = "macos")]
fn a_key_press_reaches_only_the_selected_pane_through_the_endpoint(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);
    let shift_enter = ep::ClientPaneInputEvent::Key {
        code: ep::ClientKeyCode::Enter,
        modifiers: 1,
        kind: ep::ClientKeyKind::Press,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: true,
        physical_key_id: None,
        windows_record: None,
    };
    view.update_in(cx, |this, window, cx| {
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-left"));
        assert!(
            this.pane("p-left").is_some(),
            "the visible tab spawns a Ghostty surface"
        );
        assert!(this.pane("p-right").is_some());
        let mut shift_enter_event = key("enter", false);
        shift_enter_event.keystroke.modifiers.shift = true;
        shift_enter_event.keystroke.key_char = Some("\n".into());
        this.send_key(&shift_enter_event, window, cx);
    });
    // The endpoint worker thread writes the input lane; poll until it lands.
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.pane_inputs("p-left").is_empty() {
        assert!(
            Instant::now() < deadline,
            "the key never reached the endpoint"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fake.pane_inputs("p-left"), vec![shift_enter.clone()]);
    assert!(fake.pane_inputs("p-right").is_empty());

    // PixPin and Finder expose an image as a local file path. Keep the file
    // alive while both local pass-through and remote protocol bridging exercise it.
    let clipboard_dir = tempfile::TempDir::new().expect("clipboard directory");
    let clipboard_image = clipboard_dir.path().join("PixPin capture.png");
    std::fs::write(&clipboard_image, b"\x89PNG\r\n\x1a\nremote").expect("clipboard image");

    // On a local profile the file-backed image must still reach the agent as
    // the original Cmd+V chord.
    let cmd_v = ep::ClientPaneInputEvent::Key {
        code: ep::ClientKeyCode::Char('v'),
        modifiers: 8,
        kind: ep::ClientKeyKind::Press,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: true,
        physical_key_id: None,
        windows_record: None,
    };
    view.update_in(cx, |this, window, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem {
            entries: vec![gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                vec![clipboard_image.clone()].into(),
            ))],
        });
        let mut cmd_v_event = key("v", false);
        cmd_v_event.keystroke.modifiers.platform = true;
        this.send_key(&cmd_v_event, window, cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.pane_inputs("p-left").len() < 2 {
        assert!(
            Instant::now() < deadline,
            "image-only Cmd+V never passed through"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fake.pane_inputs("p-left"),
        vec![shift_enter.clone(), cmd_v.clone()]
    );

    // The same file-backed Cmd+V on an SSH profile reads the local file in the
    // background, then sends its bytes over the endpoint's clipboard bridge.
    view.update_in(cx, |this, window, cx| {
        this.profiles[0] = ConnectionProfile::Ssh {
            // Keep the fixture's existing endpoint connection while making
            // routing remote. Production profile ids are unique; this test id
            // intentionally matches Local's synthetic `local` owner.
            id: "local".into(),
            label: "Remote".into(),
            destination: "unused.example".into(),
            port: None,
            identity_file: None,
            proxy_jump: None,
            herdr_path: "herdr".into(),
        };
        cx.write_to_clipboard(gpui::ClipboardItem {
            entries: vec![gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                vec![clipboard_image.clone()].into(),
            ))],
        });
        assert!(matches!(
            this.current_profile(),
            ConnectionProfile::Ssh { .. }
        ));
        let mut cmd_v_event = key("v", false);
        cmd_v_event.keystroke.modifiers.platform = true;
        this.send_key(&cmd_v_event, window, cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.clipboard_images().is_empty() {
        assert!(
            Instant::now() < deadline,
            "remote ClipboardImage never reached the endpoint"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }

    // Herdr users also paste with Ctrl+V. It must take the identical native
    // bridge path instead of being encoded as a literal control character.
    view.update_in(cx, |this, window, cx| {
        this.send_key(&key("v", true), window, cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.clipboard_images().len() < 2 {
        assert!(
            Instant::now() < deadline,
            "Ctrl+V ClipboardImage never reached the endpoint"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fake.pane_inputs("p-left"),
        vec![shift_enter, cmd_v],
        "remote image paste must not leak Cmd+V or Ctrl+V as keys"
    );
    assert_eq!(
        fake.clipboard_images(),
        vec![
            (
                "p-left".into(),
                "png".into(),
                b"\x89PNG\r\n\x1a\nremote".to_vec()
            ),
            (
                "p-left".into(),
                "png".into(),
                b"\x89PNG\r\n\x1a\nremote".to_vec()
            ),
        ]
    );
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn herdr_system_toast_reaches_the_os_notification_center(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);

    view.update(cx, |this, cx| {
        let owner = this.current_session_key().expect("connected session");
        assert!(this.apply_endpoint_batch(
            &owner,
            vec![Ok(EndpointEvent::Notify {
                kind: TerminalNotificationKind::SystemToast,
                message: "Codex finished".into(),
                body: Some("workspace 1".into()),
            })],
            cx,
        ));
    });

    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].tag.starts_with("ocherdr-herdr-"));
    assert_eq!(shown[0].title, "Codex finished");
    assert_eq!(shown[0].body, "workspace 1");
    assert!(shown[0].actions.is_empty());
    cx.simulate_system_notification_response(gpui::SystemNotificationResponse {
        tag: shown[0].tag.clone(),
        action_id: None,
    });
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert_eq!(
            this.selection.pane_id.as_deref(),
            Some("p-left"),
            "an endpoint toast carries no pane target, so clicking it changes nothing"
        );
    });
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn background_agent_completion_reaches_the_os_notification_center(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);

    let finished = || {
        EndpointEvent::SemanticNotification(ep::SemanticNotification {
            kind: ep::SemanticNotificationKind::Finished,
            title: String::new(),
            body: None,
            sound: None,
            agent: Some("Codex".into()),
            workspace_id: Some("w".into()),
            tab_id: Some("t-a".into()),
            pane_id: Some("p-right".into()),
            position: None,
        })
    };
    view.update(cx, |this, cx| {
        this.i18n = I18n::new(Language::English);
        let pane = this
            .snapshot
            .as_mut()
            .and_then(|snapshot| {
                snapshot
                    .panes
                    .iter_mut()
                    .find(|pane| pane.pane_id == "p-right")
            })
            .expect("background pane");
        pane.agent = Some("codex".into());
        pane.display_agent = Some("Codex".into());
        pane.agent_status = AgentStatus::Working;
        let owner = this.current_session_key().expect("connected session");
        assert!(this.apply_endpoint_batch(&owner, vec![Ok(finished())], cx));
    });

    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].title, "Codex finished");
    assert!(shown[0].body.starts_with("Local · "));

    view.update(cx, |this, cx| {
        // The semantic channel has no event log to dedupe against; the
        // selected pane's own completion stays quiet only while it is visible.
        this.selection.pane_id = Some("p-right".into());
        this.window_active = true;
        let owner = this.current_session_key().expect("connected session");
        assert!(this.apply_endpoint_batch(&owner, vec![Ok(finished())], cx));
    });
    assert_eq!(
        cx.shown_system_notifications().len(),
        1,
        "the visible selected pane's completion must not notify"
    );
}

#[gpui::test]
fn clicking_an_agent_notification_jumps_to_its_pane(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);

    view.update(cx, |this, cx| {
        let pane = this
            .snapshot
            .as_mut()
            .and_then(|snapshot| {
                snapshot
                    .panes
                    .iter_mut()
                    .find(|pane| pane.pane_id == "p-right")
            })
            .expect("background pane");
        pane.tab_id = "t-b".into();
        pane.agent = Some("codex".into());
        pane.display_agent = Some("Codex".into());
        pane.agent_status = AgentStatus::Working;
        let owner = this.current_session_key().expect("connected session");
        assert!(this.apply_endpoint_batch(
            &owner,
            vec![Ok(EndpointEvent::SemanticNotification(
                ep::SemanticNotification {
                    kind: ep::SemanticNotificationKind::Finished,
                    title: String::new(),
                    body: None,
                    sound: None,
                    agent: Some("Codex".into()),
                    workspace_id: Some("w".into()),
                    tab_id: Some("t-b".into()),
                    pane_id: Some("p-right".into()),
                    position: None,
                },
            ))],
            cx,
        ));
    });
    let notification = cx
        .shown_system_notifications()
        .into_iter()
        .next()
        .expect("agent notification");
    assert!(notification.tag.starts_with("ocherdr-agent-"));
    view.update(cx, |this, _| {
        this.snapshot
            .as_mut()
            .and_then(|snapshot| {
                snapshot
                    .panes
                    .iter_mut()
                    .find(|pane| pane.pane_id == "p-right")
            })
            .expect("notified pane still exists")
            .pane_id = "p-right-realiased".into();
    });

    cx.simulate_system_notification_response(gpui::SystemNotificationResponse {
        tag: notification.tag,
        action_id: None,
    });
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert_eq!(this.selection.workspace_id.as_deref(), Some("w"));
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-b"));
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-right-realiased"));
    });
    let focus = fake.requests_for("pane.focus");
    assert_eq!(focus.len(), 1);
    assert_eq!(focus[0]["params"]["pane_id"], json!("p-right-realiased"));
}

#[gpui::test]
fn clicking_a_parked_hosts_agent_notification_switches_host_and_pane(cx: &mut TestAppContext) {
    let fake_a = FakeHerdr::serving(two_pane_snapshot());
    let fake_b = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake_a, cx);

    view.update(cx, |this, cx| {
        let local_owner = this.current_session_key().expect("local session owner");
        let pane = this
            .snapshot
            .as_mut()
            .and_then(|snapshot| {
                snapshot
                    .panes
                    .iter_mut()
                    .find(|pane| pane.pane_id == "p-right")
            })
            .expect("local background pane");
        pane.agent = Some("codex".into());
        pane.display_agent = Some("Codex".into());
        pane.agent_status = AgentStatus::Working;

        let profile_b = ConnectionProfile::Ssh {
            id: "remote-click-test".into(),
            label: "Remote click test".into(),
            destination: "unused".into(),
            port: None,
            identity_file: None,
            proxy_jump: None,
            herdr_path: "herdr".into(),
        };
        let session_b = SessionSummary {
            name: "beta".into(),
            running: true,
            socket_path: fake_b.socket_path(),
            session_dir: fake_b._dir.path().join("beta"),
            default: false,
        };
        let connection_b = SessionConnection::connect(&ConnectionProfile::default(), &session_b)
            .expect("connect host B fake");
        let snapshot_b = two_pane_snapshot();
        let profile_b_index = this.profiles.len();
        this.profiles.push(profile_b);
        this.parked_hosts.insert(
            "remote-click-test".into(),
            crate::ParkedHostRuntime {
                sessions: vec![session_b],
                session_index: Some(0),
                connection: connection_b,
                event_stream: EventStreamState::Idle,
                snapshot: Some(snapshot_b),
                selection: Selection {
                    connection_id: "remote-click-test".into(),
                    session_name: Some("beta".into()),
                    workspace_id: Some("w".into()),
                    tab_id: Some("t-a".into()),
                    pane_id: Some("p-left".into()),
                },
                session_panes: None,
                pane_viewports: HashMap::new(),
                snapshot_refreshing: false,
                snapshot_refresh_pending: false,
            },
        );
        this.select_profile(profile_b_index, cx);
        assert!(this.apply_endpoint_batch(
            &local_owner,
            vec![Ok(EndpointEvent::SemanticNotification(
                ep::SemanticNotification {
                    kind: ep::SemanticNotificationKind::Finished,
                    title: String::new(),
                    body: None,
                    sound: None,
                    agent: Some("Codex".into()),
                    workspace_id: Some("w".into()),
                    tab_id: Some("t-a".into()),
                    pane_id: Some("p-right".into()),
                    position: None,
                },
            ))],
            cx,
        ));
    });
    let notification = cx
        .shown_system_notifications()
        .into_iter()
        .next()
        .expect("parked host notification");

    cx.simulate_system_notification_response(gpui::SystemNotificationResponse {
        tag: notification.tag,
        action_id: None,
    });
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert_eq!(this.current_profile().id(), "local");
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-right"));
    });
    let focus = fake_a.requests_for("pane.focus");
    assert_eq!(focus.len(), 1);
    assert_eq!(focus[0]["params"]["pane_id"], json!("p-right"));
}

#[gpui::test]
fn wheel_interaction_reuses_automatic_controls_without_changing_focus(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);

    view.update(cx, |this, cx| {
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-left"));
        assert_eq!(this.pane("p-left").unwrap().mode, TerminalMode::Control);
        assert_eq!(this.pane("p-right").unwrap().mode, TerminalMode::Control);
        let wheel = gpui::ScrollWheelEvent {
            position: gpui::point(gpui::px(0.), gpui::px(0.)),
            delta: gpui::ScrollDelta::Lines(gpui::point(0., 3.)),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        };

        this.scroll_pane("p-left", &wheel, cx);
        assert_eq!(this.pane("p-left").unwrap().mode, TerminalMode::Control);
        assert_eq!(this.pane("p-right").unwrap().mode, TerminalMode::Control);

        this.scroll_pane("p-right", &wheel, cx);
        assert_eq!(this.pane("p-left").unwrap().mode, TerminalMode::Control);
        assert_eq!(this.pane("p-right").unwrap().mode, TerminalMode::Control);
        assert_eq!(
            this.selection.pane_id.as_deref(),
            Some("p-left"),
            "wheel promotion must not move keyboard focus"
        );
        let controls = &this.session_panes.as_ref().unwrap().controls;
        assert_eq!(controls.len(), 2);
        assert_eq!(controls.get("p-left"), Some(&TerminalMode::Control));
        assert_eq!(controls.get("p-right"), Some(&TerminalMode::Control));
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
    push_state(&fake, &view, cx, park_state, |snapshot| {
        snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp")
    });
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
