use super::*;

#[gpui::test]
fn a_two_pane_template_parks_then_rebuilds_the_tab(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    let geometry = crate::pane_template_palette_geometry(SURFACE, 2).unwrap();
    let slot = geometry.cards[0].slots[1];
    let drop = (slot.0 + slot.2 / 2., slot.1 + slot.3 / 2.);
    view.update(cx, |this, cx| {
        assert!(this.begin_pane_drag("p-left".into(), (SURFACE.0 + 12., SURFACE.1 + 12.)));
        assert!(this.update_pane_drag(drop, cx));
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert_eq!(
            drag.template_hover.as_ref().map(|hover| hover.placement),
            Some(crate::PaneTemplatePlacement {
                template: crate::PaneLayoutTemplate::TwoColumns,
                slot: 1,
            })
        );
        assert!(matches!(
            drag.layout_preview
                .as_ref()
                .and_then(|preview| preview.intent.as_ref()),
            Some(crate::PaneDragIntent::Template(_))
        ));
    });
    assert!(
        fake.requests_for("pane.move").is_empty(),
        "hover is local-only"
    );
    view.update_in(cx, |this, window, cx| {
        let surface = this.terminal_surface_bounds.unwrap();
        let geometry = crate::pane_template_palette_geometry(surface, 2).unwrap();
        let slot = geometry.cards[0].slots[1];
        let release = (slot.0 + slot.2 / 2., slot.1 + slot.3 / 2.);
        assert!(this.update_pane_drag(release, cx));
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag before release");
        };
        let layout = this.snapshot.as_ref().unwrap().layout_for("t-a").unwrap();
        assert!(this.pane_move_supported());
        assert!(!this.tab_relocation_locked("t-a"));
        assert_eq!(drag.fingerprint, ocherdr_core::layout_fingerprint(layout));
        assert!(
            crate::pane_template_predicted_layout(
                layout,
                &drag.pane_id,
                drag.template_hover.as_ref().unwrap().placement,
            )
            .is_some()
        );
        assert!(this.finish_pane_drag(release, window, cx));
        assert!(
            this.tab_relocation_locked("t-a"),
            "commits={:?}, drag={:?}, return={}",
            this.pane_template_commits.keys().collect::<Vec<_>>(),
            this.surface_drag,
            this.pane_drag_return.is_some(),
        );
    });
    cx.run_until_parked();

    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 2, "one park and one insertion: {moves:?}");
    assert_eq!(moves[0]["params"]["destination"]["type"], json!("new_tab"));
    assert_eq!(moves[1]["params"]["destination"]["type"], json!("tab"));
    assert_eq!(
        moves[1]["params"]["destination"]["target_pane_id"],
        json!("p-right")
    );
    assert_eq!(moves[1]["params"]["destination"]["split"], json!("right"));
    view.read_with(cx, |this, _| {
        assert_eq!(
            this.pane_template_commits
                .get("t-a")
                .map(|pending| &pending.phase),
            Some(&crate::PaneTemplateCommitPhase::AwaitingLayout)
        );
    });

    fake.send_event(swapped_layout_event());
    thread::sleep(Duration::from_millis(20));
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.pane_template_commits.is_empty());
        assert!(!this.tab_relocation_locked("t-a"));
    });
}

#[gpui::test]
fn a_centre_drop_sends_one_pane_swap_and_settles_on_the_matching_layout(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    // Centre of the right pane: x = 100 + 600 * 0.75, y = 50 + 200.
    drag_left_pane_to(&view, (550., 250.), cx);
    view.read_with(cx, |this, _| {
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(
            this.tab_relocation_locked("t-a"),
            "the plan locks the tab while the swap is in flight"
        );
        let pending = this.pane_relocations.get("t-a").expect("plan pending");
        assert_eq!(pending.plan.source_pane_id, "p-left");
        assert_eq!(pending.plan.target_pane_id, "p-right");
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
            "the source pane is drawn on the right immediately: {predicted:?}"
        );
    });
    cx.run_until_parked();

    let swaps = fake.requests_for("pane.swap");
    assert_eq!(swaps.len(), 1, "exactly one pane.swap: {swaps:?}");
    assert_eq!(swaps[0]["params"]["source_pane_id"], json!("p-left"));
    assert_eq!(swaps[0]["params"]["target_pane_id"], json!("p-right"));
    view.read_with(cx, |this, _| {
        let pending = this
            .pane_relocations
            .get("t-a")
            .expect("still waiting for layout");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::Swapping {
                responded: true,
                layout_seen: false
            }
        );
    });

    fake.send_event(swapped_layout_event());
    thread::sleep(Duration::from_millis(20));
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a");
        assert!(
            pending.is_none()
                || matches!(
                    pending.map(|p| &p.phase),
                    Some(crate::RelocationPhase::Settling { .. })
                ),
            "response + matching layout.updated moves the plan into Settling"
        );
    });
    thread::sleep(Duration::from_millis(220));
    view.update(cx, |this, _| {
        this.expire_pane_motion(Instant::now(), false);
        assert!(
            !this.tab_relocation_locked("t-a"),
            "the settle animation releases the tab"
        );
        let layout = this
            .snapshot
            .as_ref()
            .and_then(|s| s.layout_for("t-a"))
            .expect("layout");
        assert_eq!(
            layout.panes[0].pane_id, "p-right",
            "authoritative layout on screen"
        );
    });
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
}

#[gpui::test]
fn a_centre_hover_squeezes_the_local_layout_before_release(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    view.update(cx, |this, cx| {
        assert!(this.begin_pane_drag("p-left".into(), grab));
        assert!(this.update_pane_drag((550., 250.), cx));
    });
    view.read_with(cx, |this, _| {
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("pane drag");
        };
        assert!(drag.layout_preview.is_some(), "hover owns a local draft");
        assert_eq!(
            drag.layout_preview
                .as_ref()
                .and_then(|preview| preview.intent.as_ref())
                .and_then(|intent| match intent {
                    crate::PaneDragIntent::Pane {
                        target_pane_id,
                        zone,
                    } => Some((&**target_pane_id, *zone)),
                    crate::PaneDragIntent::Template(_) => None,
                }),
            Some(("p-right", ocherdr_core::DropZone::Center))
        );
        let layout = this
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.layout_for("t-a"));
        let source = this
            .displayed_pane_fractions(layout, "p-left", Instant::now(), true)
            .unwrap();
        let target = this
            .displayed_pane_fractions(layout, "p-right", Instant::now(), true)
            .unwrap();
        assert!((source.0 - 0.5).abs() < 1e-6, "source slot moved right");
        assert!(
            target.0.abs() < 1e-6,
            "target was squeezed into source slot"
        );
        assert!(this.pane_resize_frozen("p-left"));
        assert!(this.pane_resize_frozen("p-right"));
    });
    assert!(fake.requests_for("pane.swap").is_empty());
    assert!(
        fake.requests_for("pane.move").is_empty(),
        "hover is local-only"
    );
    view.update(cx, |this, _| this.cancel_pane_drag());
}

#[gpui::test]
fn the_first_authoritative_frame_after_drag_reports_a_terminal_thaw(cx: &mut TestAppContext) {
    let (_fake, view, cx) = connect_two_pane_view(cx);
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    view.update(cx, |this, cx| {
        assert!(this.begin_pane_drag("p-left".into(), grab));
        assert!(this.update_pane_drag((550., 250.), cx));
        assert!(!this.tab_resize_just_thawed("t-a"));
        assert!(this.pane_resize_frozen_tabs.contains("t-a"));
        this.cancel_pane_drag();
        assert!(!this.tab_resize_just_thawed("t-a"));
        let finished = this
            .pane_drag_return
            .as_ref()
            .map(|flight| flight.started + crate::PANE_DRAG_RETURN_ANIMATION)
            .expect("return flight");
        assert!(this.expire_pane_motion(finished, false));
        assert!(
            this.tab_resize_just_thawed("t-a"),
            "the render must actively resize and flush cached pane bodies"
        );
        assert!(!this.tab_resize_just_thawed("t-a"), "thaw fires once");
    });
}

#[gpui::test]
fn an_invalid_drop_sends_nothing_and_returns_home(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    // Well outside every pane.
    drag_left_pane_to(&view, (SURFACE.0 + SURFACE.2 + 80., 20.), cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(
            this.pane_relocations.is_empty(),
            "no plan without a drop zone"
        );
        assert_eq!(
            this.pane_drag_return.as_ref().map(|flight| flight.to),
            Some((SURFACE.0, SURFACE.1, SURFACE.2 / 2., SURFACE.3)),
            "the preview flies back to the source slot"
        );
    });
    assert!(fake.requests_for("pane.swap").is_empty());
}

#[gpui::test]
fn an_edge_drop_is_not_droppable_with_the_flag_off(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    // Far right edge of the right pane: zone Right, computed but not droppable.
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    let edge = (SURFACE.0 + SURFACE.2 - 6., SURFACE.1 + 200.);
    view.update_in(cx, |this, window, cx| {
        assert!(this.begin_pane_drag("p-left".into(), grab));
        this.update_pane_drag(edge, cx);
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("dragging");
        };
        let hover = drag.hover.as_ref().expect("hover over the right pane");
        assert_eq!(hover.target_pane_id, "p-right");
        assert_eq!(hover.zone, ocherdr_core::DropZone::Right);
        assert!(!hover.droppable(drag.edge_drops));
        assert!(
            drag.layout_preview.is_none(),
            "a disabled edge cannot preview a layout that release cannot commit"
        );
        this.finish_pane_drag(edge, window, cx);
        assert!(this.pane_relocations.is_empty());
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.swap").is_empty());
}

#[gpui::test]
fn escape_cancels_a_pane_drag_without_a_request(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    view.update_in(cx, |this, window, cx| {
        assert!(this.begin_pane_drag("p-left".into(), grab));
        this.update_pane_drag((550., 250.), cx);
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Pane(_)));
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
        assert!(
            this.pane_drag_return.is_some(),
            "the lifted preview returns"
        );
        assert_eq!(
            this.pane_drag_return
                .as_ref()
                .map(|flight| flight.layout_from.len()),
            Some(2),
            "both pane shells return with the floating preview"
        );
        // Releasing afterwards is a no-op: nothing is dragging any more.
        assert!(!this.finish_pane_drag((550., 250.), window, cx));
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.swap").is_empty());
    view.read_with(cx, |this, _| assert!(this.pane_relocations.is_empty()));
}

/// Esc is handled by the root `on_key_down`, which GPUI dispatches along
/// the focused element's ancestry; with nothing focused only the window's
/// root node (the view wrapper, above our root div) receives the key. The
/// handle press stops propagation, so it cannot rely on the surface's
/// focus-on-click: it focuses the surface itself, and Esc then reaches the
/// drag whether or not anything was focused before the grab.
#[gpui::test]
fn escape_reaches_the_drag_through_window_dispatch_regardless_of_focus(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    let press = gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: gpui::point(gpui::px(grab.0), gpui::px(grab.1)),
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    };
    for surface_focused_before in [false, true] {
        view.update_in(cx, |this, window, cx| {
            if surface_focused_before {
                this.focus.focus(window, cx);
            } else {
                window.blur();
            }
            assert_eq!(this.focus.is_focused(window), surface_focused_before);
            this.press_pane_handle("p-left".into(), &press, window, cx);
            assert!(
                this.focus.is_focused(window),
                "the grab focuses the surface"
            );
            assert!(this.update_pane_drag((550., 250.), cx));
            assert!(matches!(this.surface_drag, crate::SurfaceDrag::Pane(_)));
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        view.update(cx, |this, _| {
            assert!(
                matches!(this.surface_drag, crate::SurfaceDrag::Idle),
                "Esc cancels with surface_focused_before={surface_focused_before}"
            );
            assert!(this.pane_drag_return.is_some(), "the preview flies back");
            this.pane_drag_return = None;
        });
    }
    assert!(fake.requests_for("pane.swap").is_empty());
    assert!(fake.requests_for("pane.move").is_empty());
}

#[gpui::test]
fn a_layout_that_does_not_match_the_plan_reverts_to_authority(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    drag_left_pane_to(&view, (550., 250.), cx);
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.swap").len(), 1);

    // Someone else split the tab before our swap landed: the pane set no
    // longer matches the plan.
    let mut layout = two_pane_layout("p-right", "p-left");
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
    fake.send_event(json!({
        "event": "layout_updated",
        "data": { "type": "layout_updated", "layout": layout }
    }));
    thread::sleep(Duration::from_millis(20));
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.pane_relocations.is_empty(),
            "a mismatching layout.updated drops the prediction"
        );
        let rect = this
            .displayed_pane_fractions(
                this.snapshot.as_ref().and_then(|s| s.layout_for("t-a")),
                "p-left",
                Instant::now(),
                false,
            )
            .expect("authoritative rect");
        assert!(
            (rect.3 - 0.5).abs() < 1e-6,
            "authoritative geometry wins: {rect:?}"
        );
    });
}

#[gpui::test]
fn a_rejected_swap_drops_the_plan_and_unlocks_the_tab(cx: &mut TestAppContext) {
    let mut snapshot = two_pane_snapshot();
    snapshot.panes[1].pane_id = "reject".into();
    snapshot.layouts = vec![two_pane_layout("p-left", "reject")];
    let fake = FakeHerdr::snapshot_with_live_events(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| this.terminal_surface_bounds = Some(SURFACE));
    drag_left_pane_to(&view, (550., 250.), cx);
    view.read_with(cx, |this, _| assert!(this.tab_relocation_locked("t-a")));
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
    view.read_with(cx, |this, _| {
        assert!(
            !this.tab_relocation_locked("t-a"),
            "the error response releases the lock"
        );
    });
}

#[gpui::test]
fn a_locked_tab_refuses_a_second_drag_and_pane_close(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    drag_left_pane_to(&view, (550., 250.), cx);
    view.update(cx, |this, cx| {
        assert!(
            !this.begin_pane_drag("p-right".into(), (SURFACE.0 + 400., SURFACE.1 + 12.)),
            "one plan per tab"
        );
        this.request_close(
            crate::HierarchyTarget::Pane {
                id: "p-right".into(),
                label: "P-RIGHT".into(),
            },
            cx,
        );
        assert!(
            matches!(this.overlay, crate::Overlay::None),
            "pane close is refused while the plan is pending"
        );
        assert!(this.pane_resize_frozen("p-right"), "grids are frozen");
    });
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
}

// ---- Split drag: squeeze preview (design §5.4) ----

#[gpui::test]
fn a_split_drag_squeezes_the_preview_and_sends_one_set_split_ratio(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    // The divider sits at x = SURFACE.0 + 300 (ratio 0.5 of 600 px); the
    // pointer lands at 0.7.
    let press = (SURFACE.0 + 300., SURFACE.1 + 100.);
    let release = (SURFACE.0 + 420., SURFACE.1 + 100.);
    view.update(cx, |this, cx| {
        let snapshot = this.snapshot.clone().expect("snapshot");
        let layout = snapshot.layout_for("t-a").expect("layout");
        let split = layout.splits[0].clone();
        let drag = super::split_drag_from_press("t-a".into(), &split, layout, SURFACE, press)
            .expect("split drag");
        this.surface_drag = crate::SurfaceDrag::Split(drag);
        assert!(this.update_split_drag(release, cx));
        assert!(
            this.pane_resize_frozen("p-left") && this.pane_resize_frozen("p-right"),
            "terminal surfaces are not resized while the divider is dragged"
        );
        let left = this
            .displayed_pane_fractions(Some(layout), "p-left", Instant::now(), false)
            .expect("left rect");
        let right = this
            .displayed_pane_fractions(Some(layout), "p-right", Instant::now(), false)
            .expect("right rect");
        assert!((left.2 - 0.7).abs() < 1e-3, "left shell squeezes: {left:?}");
        assert!(
            (right.0 - 0.7).abs() < 1e-3,
            "right shell follows: {right:?}"
        );
        assert!((right.2 - 0.3).abs() < 1e-3, "{right:?}");
        assert!(this.finish_split_drag(release, cx));
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(
            this.pane_resize_frozen("p-left"),
            "release keeps the freeze until the ratio lands"
        );
        assert!(this.tab_relocation_locked("t-a"), "the batch locks the tab");
        let kept = this
            .displayed_pane_fractions(Some(layout), "p-left", Instant::now(), false)
            .expect("preview rect");
        assert!(
            (kept.2 - 0.7).abs() < 1e-3,
            "the preview stays until layout.updated: {kept:?}"
        );
    });
    cx.run_until_parked();
    let requests = fake.requests_for("layout.set_split_ratio");
    assert_eq!(requests.len(), 1, "one request on release: {requests:?}");
    let ratio = requests[0]["params"]["ratio"].as_f64().expect("ratio");
    assert!((ratio - 0.7).abs() < 1e-3, "{ratio}");
    view.read_with(cx, |this, _| {
        assert!(
            this.pane_resize_frozen("p-left"),
            "the ok response alone does not end the preview"
        );
    });
    let mut landed = two_pane_layout("p-left", "p-right");
    landed.splits[0].ratio = ratio as f32;
    landed.panes[0].rect = layout_rect(0, 0, 84, 40);
    landed.panes[1].rect = layout_rect(84, 0, 36, 40);
    send_events(&fake, vec![layout_event(landed)], cx);
    view.read_with(cx, |this, _| {
        assert!(this.split_commit.is_none(), "the matching layout settles");
        assert!(!this.pane_resize_frozen("p-left"), "settling unfreezes");
        assert!(!this.tab_relocation_locked("t-a"));
    });
}

/// `t-a` holds `right[ right[p1, p2] | p3 ]` over a 120×40 area with the
/// given ratios; rects follow Herdr's rounding.
fn nested_three_pane_layout(outer: f32, inner: f32) -> ocherdr_core::PaneLayout {
    let area = layout_rect(0, 0, 120, 40);
    let (left, right) = ocherdr_core::split_rect(area, ocherdr_core::SplitDirection::Right, outer);
    let (p1, p2) = ocherdr_core::split_rect(left, ocherdr_core::SplitDirection::Right, inner);
    ocherdr_core::PaneLayout {
        workspace_id: "w".into(),
        tab_id: "t-a".into(),
        zoomed: false,
        area,
        focused_pane_id: "p1".into(),
        panes: vec![
            ocherdr_core::LayoutPane {
                pane_id: "p1".into(),
                focused: true,
                rect: p1,
            },
            ocherdr_core::LayoutPane {
                pane_id: "p2".into(),
                focused: false,
                rect: p2,
            },
            ocherdr_core::LayoutPane {
                pane_id: "p3".into(),
                focused: false,
                rect: right,
            },
        ],
        splits: vec![
            ocherdr_core::LayoutSplit {
                id: "split_0_root".into(),
                direction: ocherdr_core::SplitDirection::Right,
                ratio: outer,
                rect: area,
            },
            ocherdr_core::LayoutSplit {
                id: "split_1_0".into(),
                direction: ocherdr_core::SplitDirection::Right,
                ratio: inner,
                rect: left,
            },
        ],
    }
}

#[gpui::test]
fn dragging_the_outer_divider_keeps_the_inner_one_pinned_and_sends_both_ratios(
    cx: &mut TestAppContext,
) {
    let mut snapshot = pane_move_capable_snapshot();
    snapshot.focused_pane_id = Some("p1".into());
    snapshot.panes = vec![
        split_pane("p1", true),
        split_pane("p2", false),
        split_pane("p3", false),
    ];
    snapshot.layouts = vec![nested_three_pane_layout(0.5, 0.5)];
    let fake = FakeHerdr::snapshot_with_live_events(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| this.terminal_surface_bounds = Some(SURFACE));
    // Outer divider at x = 300 px (cell 60) dragged to 0.7 (cell 84); the
    // inner divider sits on cell 30 and must stay there: 30 / 84.
    let press = (SURFACE.0 + 300., SURFACE.1 + 100.);
    let release = (SURFACE.0 + 420., SURFACE.1 + 100.);
    let inner_expected = 30. / 84.;
    view.update(cx, |this, cx| {
        let snapshot = this.snapshot.clone().expect("snapshot");
        let layout = snapshot.layout_for("t-a").expect("layout");
        let outer = layout.splits[0].clone();
        let drag = super::split_drag_from_press("t-a".into(), &outer, layout, SURFACE, press)
            .expect("split drag");
        this.surface_drag = crate::SurfaceDrag::Split(drag);
        assert!(this.update_split_drag(release, cx));
        let squeezed = this.squeezed_tab_layout(layout).expect("squeezed preview");
        let (_, outer_line) = squeezed.split(&[]).expect("outer divider");
        let (_, inner_line) = squeezed.split(&[false]).expect("inner divider");
        assert!((outer_line - 0.7).abs() < 1e-6, "{outer_line}");
        assert!(
            (inner_line - 0.25).abs() < 1e-6,
            "the inner divider keeps its x: {inner_line}"
        );
        let p2 = squeezed.pane("p2").expect("p2");
        assert!(
            (p2.0 - 0.25).abs() < 1e-6 && (p2.2 - 0.45).abs() < 1e-6,
            "{p2:?}"
        );
        let p1 = squeezed.pane("p1").expect("p1");
        assert!((p1.2 - 0.25).abs() < 1e-6, "p1 is untouched: {p1:?}");
        assert!(this.finish_split_drag(release, cx));
        let kept = this
            .squeezed_tab_layout(layout)
            .expect("preview kept after release");
        assert!((kept.split(&[false]).expect("inner").1 - 0.25).abs() < 1e-6);
    });
    cx.run_until_parked();
    let requests = fake.requests_for("layout.set_split_ratio");
    assert_eq!(requests.len(), 2, "outer then inner: {requests:?}");
    assert_eq!(requests[0]["params"]["tab_id"], json!("t-a"));
    assert_eq!(requests[0]["params"]["path"], json!([]));
    let outer_ratio = requests[0]["params"]["ratio"].as_f64().expect("ratio");
    assert!((outer_ratio - 0.7).abs() < 1e-3, "{outer_ratio}");
    assert_eq!(requests[1]["params"]["tab_id"], json!("t-a"));
    assert_eq!(requests[1]["params"]["path"], json!([false]));
    let inner_ratio = requests[1]["params"]["ratio"].as_f64().expect("ratio");
    assert!((inner_ratio - inner_expected).abs() < 1e-4, "{inner_ratio}");

    // Herdr answers with one layout per request: the first (outer moved,
    // inner still 0.5) must not flash; the second settles the batch.
    send_events(
        &fake,
        vec![layout_event(nested_three_pane_layout(
            outer_ratio as f32,
            0.5,
        ))],
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(
            this.split_commit.is_some(),
            "intermediate layout keeps the preview"
        );
        let layout = this
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.layout_for("t-a"))
            .expect("layout");
        let squeezed = this.squeezed_tab_layout(layout).expect("preview");
        assert!((squeezed.split(&[false]).expect("inner").1 - 0.25).abs() < 1e-6);
        assert!(this.pane_resize_frozen("p2"));
    });
    send_events(
        &fake,
        vec![layout_event(nested_three_pane_layout(
            outer_ratio as f32,
            inner_ratio as f32,
        ))],
        cx,
    );
    view.read_with(cx, |this, _| {
        assert!(this.split_commit.is_none(), "the last layout settles");
        assert!(!this.pane_resize_frozen("p2"));
        let layout = this
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.layout_for("t-a"))
            .expect("layout");
        assert_eq!(layout.panes[1].rect, layout_rect(30, 0, 54, 40));
    });
}

// ---- Edge relocation (design §4.2, §7, phase 3) ----
