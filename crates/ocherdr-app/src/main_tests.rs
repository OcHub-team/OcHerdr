use super::*;

fn item_order() -> Vec<String> {
    ["a", "b", "c", "d"].map(str::to_owned).to_vec()
}

fn tabs_list() -> ReorderList {
    ReorderList::Tabs {
        workspace_id: "w".into(),
    }
}

fn pending_list_reorder(list: ReorderList, order: &[String]) -> PendingListReorder {
    PendingListReorder {
        list,
        order: order.to_vec(),
        source_index: 1,
        hover: ReorderHover::Item {
            index: 2,
            trailing: true,
        },
        released_origin: (640., 18.),
    }
}

fn two_pane_layout() -> PaneLayout {
    let rect = |x, y, width, height| LayoutRect {
        x,
        y,
        width,
        height,
    };
    PaneLayout {
        workspace_id: "w".into(),
        tab_id: "t".into(),
        zoomed: false,
        area: rect(0, 0, 100, 50),
        focused_pane_id: "a".into(),
        panes: vec![
            ocherdr_core::LayoutPane {
                pane_id: "a".into(),
                focused: true,
                rect: rect(0, 0, 50, 50),
            },
            ocherdr_core::LayoutPane {
                pane_id: "b".into(),
                focused: false,
                rect: rect(50, 0, 50, 50),
            },
        ],
        splits: vec![LayoutSplit {
            id: "split_0_root".into(),
            direction: SplitDirection::Right,
            ratio: 0.5,
            rect: rect(0, 0, 100, 50),
        }],
    }
}

const PANE_SURFACE: (f32, f32, f32, f32) = (10., 20., 400., 200.);

fn pane_drag_at(pointer: (f32, f32)) -> PaneDrag {
    let layout = two_pane_layout();
    let source_rect = pane_window_rect(&layout, "a", PANE_SURFACE).unwrap();
    PaneDrag {
        workspace_id: "w".into(),
        tab_id: "t".into(),
        pane_id: "a".into(),
        fingerprint: layout_fingerprint(&layout),
        origin: pointer,
        pointer,
        grab_offset: (pointer.0 - source_rect.0, pointer.1 - source_rect.1),
        source_rect,
        hover: None,
        template_hover: None,
        layout_preview: None,
        edge_drops: false,
        layout_templates: true,
        pressed_at: Instant::now(),
    }
}

fn close(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    (a.0 - b.0).abs() < 1e-6
        && (a.1 - b.1).abs() < 1e-6
        && (a.2 - b.2).abs() < 1e-6
        && (a.3 - b.3).abs() < 1e-6
}

#[test]
fn a_squeezed_layout_follows_the_preview_ratio_in_whole_cells() {
    let layout = two_pane_layout();
    let squeezed = squeezed_layout(&layout, &[(vec![], 0.7)]).unwrap();
    assert!(close(squeezed.pane("a").unwrap(), (0., 0., 0.7, 1.)));
    assert!(close(squeezed.pane("b").unwrap(), (0.7, 0., 0.3, 1.)));
    let (rect, line) = squeezed.split(&[]).unwrap();
    assert!(close(rect, (0., 0., 1., 1.)));
    assert!((line - 0.7).abs() < 1e-6);
    // A path that is not in the tree leaves everything authoritative.
    let untouched = squeezed_layout(&layout, &[(vec![true], 0.7)]).unwrap();
    assert!(close(untouched.pane("a").unwrap(), (0., 0., 0.5, 1.)));
    // Cells, like Herdr: 0.333 of 100 columns is 33 columns, and the
    // divider sits on that column, not at 33.3.
    let fine = squeezed_layout(&layout, &[(vec![], 0.333)]).unwrap();
    assert!((fine.pane("a").unwrap().2 - 0.33).abs() < 1e-6);
    assert!((fine.split(&[]).unwrap().1 - 0.33).abs() < 1e-6);
    // Out-of-range ratios are clamped the way Herdr clamps them.
    let clamped = squeezed_layout(&layout, &[(vec![], 0.01)]).unwrap();
    assert!((clamped.pane("a").unwrap().2 - 0.1).abs() < 1e-6);
}

/// The squeeze preview and the settled render must agree: for any ratio
/// the preview's rects equal the rects the normal renderer produces for
/// the authoritative layout Herdr returns for that ratio. An odd-sized
/// nested `Down` split is the case that jumped half a cell on release.
#[test]
fn the_squeeze_preview_matches_the_settled_layout_for_the_same_ratio() {
    let rect = |x, y, width, height| LayoutRect {
        x,
        y,
        width,
        height,
    };
    let area = rect(0, 0, 101, 41);
    // a | (b / c)
    let tree = LayoutNode::Split {
        direction: SplitDirection::Right,
        ratio: 0.5,
        first: Box::new(LayoutNode::Pane("a".into())),
        second: Box::new(LayoutNode::Split {
            direction: SplitDirection::Down,
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane("b".into())),
            second: Box::new(LayoutNode::Pane("c".into())),
        }),
    };
    let settled = |tree: &LayoutNode| -> PaneLayout {
        let predicted = ocherdr_core::LayoutTree {
            root: tree.clone(),
            area,
        };
        PaneLayout {
            workspace_id: "w".into(),
            tab_id: "t".into(),
            zoomed: false,
            area,
            focused_pane_id: "a".into(),
            panes: predicted
                .pane_rects()
                .iter()
                .map(|pane| ocherdr_core::LayoutPane {
                    pane_id: pane.pane_id.clone(),
                    focused: pane.pane_id == "a",
                    rect: pane.rect,
                })
                .collect(),
            splits: predicted
                .splits()
                .iter()
                .enumerate()
                .map(|(index, split)| LayoutSplit {
                    id: split_path_id(index, &split.path),
                    direction: split.direction,
                    ratio: split.ratio,
                    rect: split.rect,
                })
                .collect(),
        }
    };
    let before = settled(&tree);
    for (path, ratio) in [(vec![true], 0.5_f32), (vec![true], 0.37), (vec![], 0.61)] {
        let squeezed = squeezed_layout(&before, &[(path.clone(), ratio)]).unwrap();
        let mut retuned = tree.clone();
        set_ratio_at(&mut retuned, &path, ratio);
        let after = settled(&retuned);
        for pane in &after.panes {
            let rendered = layout_rect_fractions(after.area, pane.rect).unwrap();
            let preview = squeezed.pane(&pane.pane_id).unwrap();
            assert!(
                close(preview, rendered),
                "{} at {path:?}={ratio}: preview {preview:?} vs settled {rendered:?}",
                pane.pane_id
            );
        }
        for split in &after.splits {
            let split_path = split.path().unwrap();
            let (first, _) = split_rect(split.rect, split.direction, split.ratio);
            let (fx, fy, fw, fh) = layout_rect_fractions(after.area, first).unwrap();
            let edge = match split.direction {
                SplitDirection::Right => fx + fw,
                SplitDirection::Down => fy + fh,
            };
            let (_, line) = squeezed.split(&split_path).unwrap();
            assert!(
                (line - edge).abs() < 1e-6,
                "divider {split_path:?}: preview {line} vs pane edge {edge}"
            );
        }
    }
}

fn split_path_id(index: usize, path: &[bool]) -> String {
    if path.is_empty() {
        return format!("split_{index}_root");
    }
    let steps: String = path.iter().map(|s| if *s { '1' } else { '0' }).collect();
    format!("split_{index}_{steps}")
}

fn set_ratio_at(node: &mut LayoutNode, path: &[bool], new_ratio: f32) {
    if let LayoutNode::Split {
        ratio,
        first,
        second,
        ..
    } = node
    {
        match path.split_first() {
            None => *ratio = new_ratio,
            Some((true, rest)) => set_ratio_at(second, rest, new_ratio),
            Some((false, rest)) => set_ratio_at(first, rest, new_ratio),
        }
    }
}

#[test]
fn a_pane_drag_starts_only_past_six_pixels() {
    let mut drag = pane_drag_at((30., 40.));
    drag.pointer = (36., 40.);
    assert!(!pane_drag_past_slop(&drag), "6 px is still a click");
    drag.pointer = (36.5, 40.);
    assert!(pane_drag_past_slop(&drag));
    drag.pointer = (30., 33.);
    assert!(pane_drag_past_slop(&drag), "either axis counts");
}

#[test]
fn the_preview_keeps_the_grab_offset_and_grows_around_its_centre() {
    let mut drag = pane_drag_at((30., 40.));
    assert_eq!(drag.source_rect, (10., 20., 200., 200.));
    assert_eq!(drag.grab_offset, (20., 20.));
    drag.pointer = (130., 90.);
    let (x, y, w, h) = pane_drag_preview_rect(&drag);
    assert!((w - 203.).abs() < 1e-3 && (h - 203.).abs() < 1e-3);
    assert!((x - (110. - 1.5)).abs() < 1e-3, "{x}");
    assert!((y - (70. - 1.5)).abs() < 1e-3, "{y}");
}

#[test]
fn drop_hover_uses_the_core_five_zones_and_never_targets_the_source() {
    let layout = two_pane_layout();
    // Centre of pane b.
    let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (310., 120.)).unwrap();
    assert_eq!(hover.target_pane_id, "b");
    assert_eq!(hover.zone, DropZone::Center);
    assert!(hover.droppable(false));
    // Right edge of pane b.
    let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (405., 120.)).unwrap();
    assert_eq!(hover.zone, DropZone::Right);
    assert!(!hover.droppable(false), "edges wait for phase 3");
    assert!(hover.droppable(true));
    // Top edge of pane b.
    let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (310., 24.)).unwrap();
    assert_eq!(hover.zone, DropZone::Up);
    // Over the source pane itself: nothing.
    assert!(pane_drop_hover(&layout, "a", PANE_SURFACE, (100., 120.)).is_none());
    // Outside the surface: nothing.
    assert!(pane_drop_hover(&layout, "a", PANE_SURFACE, (500., 120.)).is_none());
}

#[test]
fn a_droppable_hover_builds_and_animates_the_local_draft_layout() {
    let layout = two_pane_layout();
    let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (310., 120.)).unwrap();
    let started = Instant::now();
    let preview = update_pane_drag_layout_preview(
        &layout,
        "a",
        pane_drag_preview_intent(Some(&hover), None, false),
        None,
        started,
        false,
    )
    .expect("centre swap preview");
    assert_eq!(
        preview.intent,
        Some(PaneDragIntent::Pane {
            target_pane_id: "b".into(),
            zone: DropZone::Center,
        })
    );
    assert!(close(
        preview.display_fractions("a", started, false).unwrap(),
        (0., 0., 0.5, 1.)
    ));
    assert!(close(
        preview
            .display_fractions("a", started + PANE_DRAG_LAYOUT_ANIMATION, false)
            .unwrap(),
        (0.5, 0., 0.5, 1.)
    ));
    assert!(close(
        preview.target_fractions("b").unwrap(),
        (0., 0., 0.5, 1.)
    ));

    let repeated = update_pane_drag_layout_preview(
        &layout,
        "a",
        pane_drag_preview_intent(Some(&hover), None, false),
        Some(&preview),
        started + Duration::from_millis(20),
        false,
    )
    .unwrap();
    assert_eq!(
        repeated.started, started,
        "mouse moves do not restart motion"
    );
}

#[test]
fn leaving_a_drop_zone_eases_every_shell_back_to_authority() {
    let layout = two_pane_layout();
    let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (310., 120.)).unwrap();
    let started = Instant::now();
    let active = update_pane_drag_layout_preview(
        &layout,
        "a",
        pane_drag_preview_intent(Some(&hover), None, false),
        None,
        started,
        false,
    )
    .unwrap();
    let settled = started + PANE_DRAG_LAYOUT_ANIMATION;
    let returning =
        update_pane_drag_layout_preview(&layout, "a", None, Some(&active), settled, false).unwrap();
    assert_eq!(returning.intent, None);
    assert!(close(
        returning.display_fractions("a", settled, false).unwrap(),
        (0.5, 0., 0.5, 1.)
    ));
    assert!(close(
        returning
            .display_fractions("a", settled + PANE_DRAG_LAYOUT_ANIMATION, false)
            .unwrap(),
        (0., 0., 0.5, 1.)
    ));
}

#[test]
fn a_disabled_edge_zone_never_creates_an_uncommittable_preview() {
    let layout = two_pane_layout();
    let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (405., 120.)).unwrap();
    assert_eq!(hover.zone, DropZone::Right);
    assert!(
        update_pane_drag_layout_preview(
            &layout,
            "a",
            pane_drag_preview_intent(Some(&hover), None, false),
            None,
            Instant::now(),
            false,
        )
        .is_none()
    );
    assert!(
        update_pane_drag_layout_preview(
            &layout,
            "a",
            pane_drag_preview_intent(Some(&hover), None, true),
            None,
            Instant::now(),
            false,
        )
        .is_some()
    );
}

fn swap_plan(layout: &PaneLayout) -> RelocationPlan {
    RelocationPlan {
        operation_id: 1,
        source_pane_id: "a".into(),
        source_tab_id: "t".into(),
        target_pane_id: "b".into(),
        target_tab_id: "t".into(),
        intent: RelocationIntent::Swap,
        fingerprint: layout_fingerprint(layout),
        topology: SplitLayoutFingerprint {
            zoomed: layout.zoomed,
            splits: layout
                .splits
                .iter()
                .filter_map(|split| Some((split.path()?, split.direction)))
                .collect(),
            panes: layout.panes.iter().map(|p| p.pane_id.clone()).collect(),
        },
        area: layout.area,
        predicted_rects: predict_swap(layout, "a", "b").unwrap(),
        visual_snapshot: None,
        workspace_id: "w".into(),
        known_tab_ids: HashSet::from(["t".to_owned()]),
        insert_shapes: None,
    }
}

#[test]
fn a_plan_settles_on_the_swapped_layout_and_is_invalidated_by_anything_else() {
    let layout = two_pane_layout();
    let plan = swap_plan(&layout);
    assert!(layout_still_matches_plan(&layout, &plan));
    assert!(!layout_settles_plan(&layout, &plan), "nothing landed yet");

    let mut swapped = layout.clone();
    swapped.panes.swap(0, 1);
    swapped.focused_pane_id = "a".into();
    assert!(!layout_still_matches_plan(&swapped, &plan));
    assert!(layout_settles_plan(&swapped, &plan));

    let mut ratio_only = layout.clone();
    ratio_only.splits[0].ratio = 0.7;
    assert!(
        layout_still_matches_plan(&ratio_only, &plan),
        "a divider move keeps the plan waiting"
    );

    let mut extra_pane = swapped.clone();
    extra_pane.panes.push(ocherdr_core::LayoutPane {
        pane_id: "c".into(),
        focused: false,
        rect: LayoutRect::default(),
    });
    assert!(!layout_settles_plan(&extra_pane, &plan));
    assert!(!layout_still_matches_plan(&extra_pane, &plan));

    let mut zoomed = swapped.clone();
    zoomed.zoomed = true;
    assert!(!layout_settles_plan(&zoomed, &plan));

    let predicted = plan.predicted_fractions();
    assert_eq!(predicted[0].0, "a");
    assert!(
        (predicted[0].1.0 - 0.5).abs() < 1e-6,
        "a is predicted on the right"
    );
}

#[test]
fn settling_moves_from_the_prediction_to_authority_and_lands_at_once_under_reduce_motion() {
    let layout = two_pane_layout();
    let plan = swap_plan(&layout);
    let mut swapped = layout.clone();
    swapped.panes.swap(0, 1);
    // Authority put the split at 0.6 while we predicted 0.5.
    swapped.panes[0].rect.width = 60;
    swapped.panes[1].rect.x = 60;
    swapped.panes[1].rect.width = 40;
    let started = Instant::now();
    let pending = SettlingSeed {
        from: plan.predicted_fractions(),
        plan,
    }
    .into_settling(started);
    let at_start = pending
        .display_fractions("a", Some(&swapped), started, false)
        .unwrap();
    assert!((at_start.0 - 0.5).abs() < 1e-6, "starts on the prediction");
    let at_end = pending
        .display_fractions("a", Some(&swapped), started + PANE_SETTLE_ANIMATION, false)
        .unwrap();
    assert!((at_end.0 - 0.6).abs() < 1e-6, "ends on authority");
    assert!(pending.is_settled(started + PANE_SETTLE_ANIMATION, false));
    assert!(!pending.is_settled(started, false));
    let reduced = pending
        .display_fractions("a", Some(&swapped), started, true)
        .unwrap();
    assert!(
        (reduced.0 - 0.6).abs() < 1e-6,
        "reduce motion lands immediately"
    );
    assert!(pending.is_settled(started, true));
}

#[test]
fn display_positions_stay_put_before_crossing_a_midpoint() {
    assert_eq!(
        reorder_display_positions(
            &item_order(),
            1,
            ReorderHover::Item {
                index: 1,
                trailing: false,
            },
        ),
        [0, 1, 2, 3]
    );
}

#[test]
fn display_positions_move_the_crossed_right_neighbor_into_the_hole() {
    assert_eq!(
        reorder_display_positions(
            &item_order(),
            1,
            ReorderHover::Item {
                index: 2,
                trailing: true,
            },
        ),
        [0, 2, 1, 3]
    );
}

#[test]
fn display_positions_move_the_crossed_left_neighbor_into_the_hole() {
    assert_eq!(
        reorder_display_positions(
            &item_order(),
            2,
            ReorderHover::Item {
                index: 1,
                trailing: false,
            },
        ),
        [0, 2, 1, 3]
    );
}

#[test]
fn display_positions_stay_in_bounds_at_both_ends() {
    for (source_index, hover) in [
        (
            2,
            ReorderHover::Item {
                index: 0,
                trailing: false,
            },
        ),
        (0, ReorderHover::AfterLast),
    ] {
        let positions = reorder_display_positions(&item_order(), source_index, hover);
        assert_eq!(positions.len(), 4);
        assert!(positions.into_iter().all(|position| position < 4));
    }
}

#[test]
fn display_shifts_use_the_same_numbers_on_either_axis() {
    let positions = reorder_display_positions(
        &item_order(),
        1,
        ReorderHover::Item {
            index: 2,
            trailing: true,
        },
    );
    let spans = [(0., 10.), (14., 10.), (28., 10.), (42., 10.)];
    let shifts = reorder_display_shifts(&spans, &positions, 4.);
    assert_eq!(shifts, [0., 14., -14., 0.]);
    assert_eq!(
        shifts
            .iter()
            .map(|&shift| reorder_axis_offset(shift, ReorderAxis::Horizontal))
            .collect::<Vec<_>>(),
        [(0., 0.), (14., 0.), (-14., 0.), (0., 0.)]
    );
    assert_eq!(
        shifts
            .iter()
            .map(|&shift| reorder_axis_offset(shift, ReorderAxis::Vertical))
            .collect::<Vec<_>>(),
        [(0., 0.), (0., 14.), (0., -14.), (0., 0.)]
    );
}

#[test]
fn a_pointer_below_the_tab_strip_does_not_take_the_ghost_with_it() {
    let strip = (260., 9., 400., 28.);
    let size = (120., 28.);
    let grab = (40., 14.);
    let pointer = (400., 200.);
    let origin = reorder_ghost_origin(pointer, grab, strip, size, ReorderAxis::Horizontal);
    assert_eq!(origin, (360., 9.));
    assert_ne!(
        pointer.1 - grab.1,
        origin.1,
        "unclamped follow would drop the ghost into the terminal"
    );
}

#[test]
fn a_pointer_beside_the_sidebar_does_not_take_the_ghost_with_it() {
    let list = (8., 120., 236., 120.);
    let size = (236., 30.);
    let grab = (20., 10.);
    let pointer = (600., 180.);
    let origin = reorder_ghost_origin(pointer, grab, list, size, ReorderAxis::Vertical);
    assert_eq!(origin.0, 8.);
    assert_eq!(origin.1, 170.);
}

#[test]
fn tab_preview_origin_centers_under_the_tab() {
    let tab = (320., 10., TAB_PILL_WIDTH, TAB_PILL_HEIGHT);
    let (x, y) = tab_preview_origin(tab, 800.);
    assert_eq!(x, 240.);
    assert_eq!(x + TAB_PREVIEW_WIDTH / 2., tab.0 + tab.2 / 2.);
    assert_eq!(y, tab.1 + tab.3 + TAB_PREVIEW_GAP);
}

#[test]
fn tab_preview_origin_clamps_to_the_left_margin() {
    let tab = (0., 10., TAB_PILL_WIDTH, TAB_PILL_HEIGHT);
    let unclamped = tab.0 + tab.2 / 2. - TAB_PREVIEW_WIDTH / 2.;
    let (x, y) = tab_preview_origin(tab, 800.);
    assert!(unclamped < TAB_PREVIEW_MARGIN);
    assert_eq!(x, TAB_PREVIEW_MARGIN);
    assert_eq!(y, tab.1 + tab.3 + TAB_PREVIEW_GAP);
}

#[test]
fn tab_preview_origin_clamps_to_the_right_margin() {
    let window_width = 800.;
    let tab = (
        window_width - TAB_PILL_WIDTH,
        10.,
        TAB_PILL_WIDTH,
        TAB_PILL_HEIGHT,
    );
    let unclamped = tab.0 + tab.2 / 2. - TAB_PREVIEW_WIDTH / 2.;
    let max_x = window_width - TAB_PREVIEW_WIDTH - TAB_PREVIEW_MARGIN;
    let (x, y) = tab_preview_origin(tab, window_width);
    assert!(unclamped > max_x);
    assert_eq!(x, max_x);
    assert_eq!(y, tab.1 + tab.3 + TAB_PREVIEW_GAP);
}

#[test]
fn ghost_origin_clamps_the_free_axis_to_the_list() {
    let strip = (260., 9., 400., 28.);
    let size = (120., 28.);
    let grab = (40., 14.);
    assert_eq!(
        reorder_ghost_origin((0., 12.), grab, strip, size, ReorderAxis::Horizontal),
        (260., 9.)
    );
    assert_eq!(
        reorder_ghost_origin((900., 12.), grab, strip, size, ReorderAxis::Horizontal),
        (540., 9.)
    );
    let list = (8., 120., 236., 120.);
    let size = (236., 30.);
    assert_eq!(
        reorder_ghost_origin((20., 0.), grab, list, size, ReorderAxis::Vertical),
        (8., 120.)
    );
    assert_eq!(
        reorder_ghost_origin((20., 800.), grab, list, size, ReorderAxis::Vertical),
        (8., 210.)
    );
}

#[test]
fn an_in_flight_tab_move_keeps_the_predicted_display_order() {
    let order = item_order();
    let pending = pending_list_reorder(tabs_list(), &order);
    let projection = reorder_projection(&tabs_list(), &order, None, Some(&pending))
        .expect("the pending request must keep its release-time projection");

    assert_eq!(projection.positions, [0, 2, 1, 3]);
    assert_eq!(projection.previous_positions, projection.positions);
    assert_eq!(
        projection.motion,
        ReorderMotion::Settling {
            released_origin: (640., 18.)
        }
    );
}

#[test]
fn an_in_flight_workspace_move_uses_the_same_projection() {
    let order = item_order();
    let pending = pending_list_reorder(ReorderList::Workspaces, &order);
    let projection = reorder_projection(&ReorderList::Workspaces, &order, None, Some(&pending))
        .expect("workspaces settle with the same mapping as tabs");
    assert_eq!(projection.positions, [0, 2, 1, 3]);
}

#[test]
fn a_different_authoritative_order_overrides_the_pending_prediction() {
    let original = item_order();
    let pending = pending_list_reorder(tabs_list(), &original);
    let authoritative = ["c", "a", "b", "d"].map(str::to_owned).to_vec();

    assert!(
        reorder_projection(&tabs_list(), &authoritative, None, Some(&pending)).is_none(),
        "a prediction based on stale order must never mask the published order"
    );
}

#[test]
fn legacy_connection_settings_keep_host_fields_without_appearance() {
    let settings: Settings = serde_json::from_str(
        r#"{"connections":[],"appearance":{"theme_family":"ember"},"language":"english"}"#,
    )
    .unwrap();

    assert!(settings.connections.is_empty());
    assert!(settings.host_metadata.is_empty());
    assert!(settings.host_groups.is_empty());
    assert!(settings.host_health.is_empty());
    let value = serde_json::to_value(&settings).unwrap();
    assert!(value.get("appearance").is_none());
    assert!(value.get("language").is_none());
}

#[test]
fn legacy_recent_ssh_ids_migrate_to_stable_alias_ids() {
    let profiles = vec![
        ConnectionProfile::default(),
        ConnectionProfile::Ssh {
            id: "ssh-config:build-box".into(),
            label: "build-box".into(),
            destination: "build-box".into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        },
    ];
    assert_eq!(
        normalize_recent_host_id("ssh-7-build-box", &profiles).as_deref(),
        Some("ssh-config:build-box")
    );
}

#[test]
fn host_tags_are_trimmed_and_deduplicated_case_insensitively() {
    assert_eq!(
        parse_host_tags(" production, gpu,Production, , arm64 "),
        ["production", "gpu", "arm64"]
    );
}

#[test]
fn remote_search_matches_labels_endpoints_and_sources() {
    let profile = ConnectionProfile::Ssh {
        id: "ssh-0-build".into(),
        label: "Build box".into(),
        destination: "builder@example.net".into(),
        port: Some(2222),
        identity_file: None,
        herdr_path: "herdr".into(),
    };

    let i18n = I18n::new(Language::English);
    assert!(profile_matches_search(&profile, "build", i18n));
    assert!(profile_matches_search(&profile, "2222", i18n));
    assert!(profile_matches_search(&profile, "ssh config", i18n));
    assert!(!profile_matches_search(&profile, "production", i18n));
}

#[test]
fn recent_hosts_move_to_the_front_and_stay_bounded() {
    let mut recents = vec!["a".into(), "b".into(), "c".into()];
    remember_recent(&mut recents, "b");
    assert_eq!(recents, ["b", "a", "c"]);
    for index in 0..10 {
        remember_recent(&mut recents, &format!("h{index}"));
    }
    assert_eq!(recents.len(), 8);
    assert_eq!(recents[0], "h9");
}

#[test]
fn switching_hosts_confirms_only_when_leaving_a_live_session() {
    assert!(!switch_requires_confirm(0, 0, true));
    assert!(!switch_requires_confirm(1, 2, false));
    assert!(switch_requires_confirm(1, 2, true));
}

fn ssh_host(id: &str, label: &str) -> ConnectionProfile {
    ConnectionProfile::Ssh {
        id: id.into(),
        label: label.into(),
        destination: label.into(),
        port: None,
        identity_file: None,
        herdr_path: "herdr".into(),
    }
}

#[test]
fn a_host_confirmation_follows_the_host_id_after_the_list_is_reordered() {
    let alpha = ssh_host("manual-1", "alpha");
    let beta = ssh_host("manual-2", "beta");
    let gamma = ssh_host("manual-3", "gamma");
    let overlay = Overlay::ConfirmSwitchProfile {
        id: beta.id().to_owned(),
        from_hosts: false,
    };

    let original = [alpha.clone(), beta.clone(), gamma.clone()];
    assert_eq!(confirmed_host_index(&overlay, &original), Some(1));

    let reordered = [gamma.clone(), alpha.clone(), beta.clone()];
    let index = confirmed_host_index(&overlay, &reordered).expect("host still exists");
    assert_eq!(reordered[index].id(), "manual-2");
    assert_ne!(
        reordered[1].id(),
        "manual-2",
        "the old index now points at a different host"
    );

    let remaining = [gamma, alpha];
    assert_eq!(
        confirmed_host_index(
            &Overlay::ConfirmRemoveProfile(beta.id().to_owned()),
            &remaining
        ),
        None
    );
}

#[test]
fn cell_metric_presets_write_ghostty_percent_values() {
    assert_eq!(CellWidthChoice::Tight.metric().unwrap().to_config(), "-10%");
    assert_eq!(CellWidthChoice::Normal.metric(), None);
    assert_eq!(CellWidthChoice::Wide.metric().unwrap().to_config(), "10%");
    assert_eq!(
        CellHeightChoice::Compact.metric().unwrap().to_config(),
        "-8%"
    );
    assert_eq!(CellHeightChoice::Normal.metric(), None);
    assert_eq!(
        CellHeightChoice::Relaxed.metric().unwrap().to_config(),
        "12%"
    );
    assert_eq!(CellHeightChoice::Loose.metric().unwrap().to_config(), "20%");
}

#[test]
fn a_missing_theme_family_warns_and_stays_in_settings() {
    let requested = "vanished-theme";
    assert!(theme::find_family(requested).is_none());
    assert!(theme::find_family(theme::DEFAULT_THEME_FAMILY).is_some());

    let english = I18n::new(Language::English);
    let notice = missing_theme_notice(requested, english).expect("missing theme must warn");
    assert_eq!(
        notice.level,
        ochub_ui::notifications::NotificationLevel::Warning
    );
    assert_eq!(notice.title, "Theme not found");
    assert_eq!(
        notice.message,
        "The theme vanished-theme in your settings does not exist. Using the default theme."
    );
    assert!(missing_theme_notice(theme::DEFAULT_THEME_FAMILY, english).is_none());

    let chinese = I18n::new(Language::SimplifiedChinese);
    let zh = missing_theme_notice(requested, chinese).expect("missing theme must warn in zh");
    assert_eq!(zh.title, "找不到主题");
    assert_eq!(
        zh.message,
        "配置里的主题 vanished-theme 不存在，已使用默认主题。"
    );

    let appearance = AppearanceSettings {
        theme_family: requested.into(),
        ..AppearanceSettings::default()
    };
    assert_eq!(appearance.theme_family, requested);
}

#[test]
fn keys_go_to_the_terminal_only_when_no_overlay_is_open() {
    let target = HierarchyTarget::Pane {
        id: "p".into(),
        label: "p".into(),
    };
    let overlays = [
        Overlay::None,
        Overlay::NodeManager,
        Overlay::RemoteForm(RemoteForm::Create),
        Overlay::RemoteForm(RemoteForm::Edit(0)),
        Overlay::Appearance,
        Overlay::HostSwitcher,
        Overlay::ContextMenu(HierarchyContextMenu {
            target: target.clone(),
            x: 0.,
            y: 0.,
            agent_details: false,
        }),
        Overlay::Rename(target.clone()),
        Overlay::ConfirmClose(target),
        Overlay::ConfirmRemoveWorktree {
            workspace_id: "w1".into(),
            label: "feature".into(),
            prompt: RemoveWorktreePrompt::Safe,
        },
        Overlay::WorktreeCreate {
            workspace_id: "w1".into(),
            advanced: false,
        },
        Overlay::WorktreeOpen(WorktreeOpenState::Loading {
            owner: SessionKey {
                profile_id: "local".into(),
                session_name: "default".into(),
            },
            workspace_id: "w1".into(),
        }),
        Overlay::ConfirmRemoveProfile("manual-1".into()),
        Overlay::ConfirmSwitchProfile {
            id: "local".into(),
            from_hosts: false,
        },
        Overlay::ConfirmSwitchProfile {
            id: "manual-1".into(),
            from_hosts: true,
        },
        Overlay::ConfirmBulkRemove,
    ];
    for overlay in overlays {
        assert_eq!(
            key_goes_to_terminal(&overlay),
            matches!(overlay, Overlay::None),
            "{overlay:?}"
        );
    }
}

#[test]
fn saved_hosts_hide_the_matching_ssh_config_entry() {
    let profiles = vec![
        ConnectionProfile::default(),
        ConnectionProfile::Ssh {
            id: "manual-1".into(),
            label: "Build".into(),
            destination: "build".into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        },
        ConnectionProfile::Ssh {
            id: "ssh-0-build".into(),
            label: "build".into(),
            destination: "build".into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        },
    ];
    assert!(ssh_config_covered_by_saved(&profiles, "build"));
    assert!(!ssh_config_covered_by_saved(&profiles, "prod"));
    assert_eq!(connection_source(&profiles[0]), ConnectionSource::ThisMac);
    assert_eq!(connection_source(&profiles[1]), ConnectionSource::Saved);
    assert_eq!(connection_source(&profiles[2]), ConnectionSource::SshConfig);
}

fn sample_visible_hosts() -> (Vec<ConnectionProfile>, HashMap<String, HostMetadata>) {
    let profiles = vec![
        ConnectionProfile::default(),
        ConnectionProfile::Ssh {
            id: "manual-1".into(),
            label: "Alpha".into(),
            destination: "alpha.example".into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        },
        ConnectionProfile::Ssh {
            id: "manual-2".into(),
            label: "Beta".into(),
            destination: "beta.example".into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        },
    ];
    let mut metadata = HashMap::new();
    metadata.insert(
        "manual-1".into(),
        HostMetadata {
            favorite: true,
            ..HostMetadata::default()
        },
    );
    (profiles, metadata)
}

fn indices_for(filter: HostFilter) -> (Vec<ConnectionProfile>, Vec<usize>) {
    let (profiles, metadata) = sample_visible_hosts();
    let recent_ids = Vec::<String>::new();
    let orphaned = HashSet::new();
    let health = HashMap::new();
    let indexes = visible_host_indices(
        &HostCatalog {
            profiles: &profiles,
            metadata: &metadata,
            recent_ids: &recent_ids,
            orphaned: &orphaned,
            health: &health,
        },
        &filter,
        "",
        0,
        I18n::new(Language::English),
    );
    (profiles, indexes)
}

#[test]
fn changing_the_host_filter_changes_the_visible_index_set() {
    let (_, all) = indices_for(HostFilter::All);
    let (_, favorites) = indices_for(HostFilter::Favorites);
    assert_ne!(all, favorites);
    assert!(all.contains(&1) && all.contains(&2));
    assert_eq!(favorites, vec![1]);
}

#[test]
fn visible_host_indices_are_always_in_range_of_the_profile_list() {
    let (profiles, all) = indices_for(HostFilter::All);
    let (_, favorites) = indices_for(HostFilter::Favorites);
    for index in all.iter().chain(&favorites) {
        assert!(*index < profiles.len());
    }
}

#[test]
fn a_filter_that_matches_nothing_returns_no_indices() {
    let (_, indexes) = indices_for(HostFilter::Tag("no-such-tag".into()));
    assert!(indexes.is_empty());
}

// ---- Insert transaction (design §7.2) ----

fn insert_plan(layout: &PaneLayout, edge: DropEdge) -> RelocationPlan {
    let steps = predict_relocation_steps(layout, "a", "b", edge, 0.5).unwrap();
    RelocationPlan {
        operation_id: 2,
        source_pane_id: "a".into(),
        source_tab_id: "t".into(),
        target_pane_id: "b".into(),
        target_tab_id: "t".into(),
        intent: RelocationIntent::Insert { edge, ratio: 0.5 },
        fingerprint: layout_fingerprint(layout),
        topology: controller::split_layout_fingerprint(layout),
        area: layout.area,
        predicted_rects: steps.final_layout.panes.clone(),
        visual_snapshot: None,
        workspace_id: "w".into(),
        known_tab_ids: HashSet::from(["t".to_owned()]),
        insert_shapes: Some(InsertShapes::from_steps(&steps)),
    }
}

fn parked() -> ParkedPane {
    ParkedPane {
        temp_tab_id: "t-tmp".into(),
        pane_id: "a".into(),
    }
}

fn inserting(responded: bool, layout_seen: bool) -> RelocationPhase {
    RelocationPhase::Inserting {
        temp_tab_id: "t-tmp".into(),
        moved_pane_id: "a".into(),
        responded,
        layout_seen,
    }
}

#[test]
fn a_right_drop_walks_parking_inserting_settle() {
    use RelocationAction as A;
    use RelocationPhase as P;
    use RelocationSignal as S;
    let (phase, action) = advance_insert_phase(P::Parking, S::Parked(Some(parked())), false);
    assert_eq!(phase, Some(inserting(false, false)));
    assert_eq!(
        action,
        A::SendInsert,
        "step 2 goes out inside the step-1 callback"
    );
    // The removed layout lands: benign.
    let (phase, action) =
        advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Removed), false);
    assert_eq!(phase, Some(inserting(false, false)));
    assert_eq!(action, A::None);
    // Response before the final layout.
    let (phase, action) = advance_insert_phase(phase.unwrap(), S::Inserted(true), false);
    assert_eq!(phase, Some(inserting(true, false)));
    assert_eq!(action, A::None);
    let (phase, action) =
        advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Final), false);
    assert_eq!(phase, Some(inserting(true, true)));
    assert_eq!(action, A::Settle);
    // The other order: layout first, then the response settles.
    let (phase, action) = advance_insert_phase(
        inserting(false, false),
        S::Layout(LayoutShape::Inserted),
        false,
    );
    assert_eq!(phase, Some(inserting(false, true)));
    assert_eq!(action, A::None);
    let (_, action) = advance_insert_phase(phase.unwrap(), S::Inserted(true), false);
    assert_eq!(action, A::Settle);
}

#[test]
fn a_left_drop_adds_the_order_correction_before_settling() {
    use RelocationAction as A;
    use RelocationPhase as P;
    use RelocationSignal as S;
    let (phase, action) = advance_insert_phase(inserting(false, false), S::Inserted(true), true);
    assert_eq!(
        phase,
        Some(P::CorrectingOrder {
            responded: false,
            layout_seen: false
        })
    );
    assert_eq!(action, A::SendSwap);
    // The step-2 layout (source second) is an intermediate, not a landing;
    // so is a late step-1 layout arriving after the step-2 response.
    let (phase, action) =
        advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Removed), true);
    assert_eq!(action, A::None);
    let (phase, action) =
        advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Inserted), true);
    assert_eq!(action, A::None);
    let (phase, action) = advance_insert_phase(phase.unwrap(), S::Reordered(true), true);
    assert_eq!(
        phase,
        Some(P::CorrectingOrder {
            responded: true,
            layout_seen: false
        })
    );
    assert_eq!(action, A::None);
    let (_, action) = advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Final), true);
    assert_eq!(action, A::Settle);
    // While still Inserting, the step-2 layout of a left drop is not a
    // landing either.
    let (phase, action) = advance_insert_phase(
        inserting(false, false),
        S::Layout(LayoutShape::Inserted),
        true,
    );
    assert_eq!(phase, Some(inserting(false, false)));
    assert_eq!(action, A::None);
}

#[test]
fn every_failure_branch_lands_where_the_design_says() {
    use RelocationAction as A;
    use RelocationPhase as P;
    use RelocationSignal as S;
    // Step 1 fails: revert, nothing else.
    assert_eq!(
        advance_insert_phase(P::Parking, S::Parked(None), false),
        (None, A::Revert)
    );
    // Step 2 fails: Parked with the temp tab, then Retry re-issues step 2.
    let (phase, action) = advance_insert_phase(inserting(false, false), S::Inserted(false), true);
    let parked_phase = P::Parked {
        temp_tab_id: "t-tmp".into(),
        moved_pane_id: "a".into(),
    };
    assert_eq!(phase, Some(parked_phase.clone()));
    assert_eq!(action, A::Park);
    let (phase, action) = advance_insert_phase(parked_phase.clone(), S::Retry, true);
    assert_eq!(phase, Some(inserting(false, false)));
    assert_eq!(action, A::SendInsert);
    // Parked shows authority: layouts do not disturb it.
    assert_eq!(
        advance_insert_phase(parked_phase.clone(), S::Layout(LayoutShape::Foreign), true),
        (Some(parked_phase), A::None)
    );
    // Step 3 fails: Misordered, plan dropped, layout kept.
    assert_eq!(
        advance_insert_phase(
            P::CorrectingOrder {
                responded: false,
                layout_seen: false
            },
            S::Reordered(false),
            true
        ),
        (None, A::Misordered)
    );
    // A foreign layout at any in-flight phase aborts to authority.
    assert_eq!(
        advance_insert_phase(P::Parking, S::Layout(LayoutShape::Foreign), false),
        (None, A::Revert)
    );
    assert_eq!(
        advance_insert_phase(
            inserting(true, false),
            S::Layout(LayoutShape::Foreign),
            false
        ),
        (None, A::Revert)
    );
    assert_eq!(
        advance_insert_phase(
            P::CorrectingOrder {
                responded: true,
                layout_seen: false
            },
            S::Layout(LayoutShape::Foreign),
            true
        ),
        (None, A::Revert)
    );
    // Out-of-order signals are ignored.
    assert_eq!(
        advance_insert_phase(P::Parking, S::Inserted(true), false),
        (Some(P::Parking), A::None)
    );
    assert_eq!(
        advance_insert_phase(P::Parking, S::Reordered(true), false),
        (Some(P::Parking), A::None)
    );
}

#[test]
fn insert_layouts_are_classified_against_the_predicted_shapes() {
    let layout = two_pane_layout();
    // `a` onto the right edge of `b`: final = [b | a].
    let plan = insert_plan(&layout, DropEdge::Right);
    assert_eq!(
        classify_insert_layout(&layout, &plan),
        LayoutShape::Release,
        "unchanged layout"
    );
    let mut removed = layout.clone();
    removed.panes.remove(0);
    removed.panes[0].rect = layout.area;
    removed.splits.clear();
    assert_eq!(
        classify_insert_layout(&removed, &plan),
        LayoutShape::Removed
    );
    let mut final_layout = layout.clone();
    final_layout.panes.swap(0, 1);
    final_layout.panes[0].rect = LayoutRect {
        x: 0,
        y: 0,
        width: 50,
        height: 50,
    };
    final_layout.panes[1].rect = LayoutRect {
        x: 50,
        y: 0,
        width: 50,
        height: 50,
    };
    assert_eq!(
        classify_insert_layout(&final_layout, &plan),
        LayoutShape::Final,
        "right/down: inserted == final"
    );
    let mut foreign = layout.clone();
    foreign.panes.push(ocherdr_core::LayoutPane {
        pane_id: "c".into(),
        focused: false,
        rect: layout.area,
    });
    assert_eq!(
        classify_insert_layout(&foreign, &plan),
        LayoutShape::Foreign
    );

    // `a` onto the left edge of `b`: step 2 gives [b | a], the swap
    // gives [a | b], which is the release shape again.
    let plan = insert_plan(&layout, DropEdge::Left);
    assert_eq!(
        classify_insert_layout(&final_layout, &plan),
        LayoutShape::Inserted
    );
    assert_eq!(classify_insert_layout(&layout, &plan), LayoutShape::Final);
    assert!(plan.intent.corrects_order());
    assert!(plan.is_supported());
    let swap = swap_plan(&layout);
    assert!(!swap.intent.corrects_order());
}

#[test]
fn a_pending_insert_renders_the_prediction_until_parked() {
    let layout = two_pane_layout();
    let plan = insert_plan(&layout, DropEdge::Right);
    let now = Instant::now();
    for phase in [
        RelocationPhase::Parking,
        inserting(false, false),
        RelocationPhase::CorrectingOrder {
            responded: false,
            layout_seen: false,
        },
    ] {
        let pending = PendingPaneRelocation {
            plan: plan.clone(),
            phase,
        };
        let rect = pending
            .display_fractions("a", Some(&layout), now, false)
            .expect("predicted");
        assert!(
            (rect.0 - 0.5).abs() < 1e-6,
            "`a` is drawn on the right: {rect:?}"
        );
        assert!(pending.phase.locks_tab());
        assert!(!pending.is_settled(now, true));
    }
    let parked = PendingPaneRelocation {
        plan,
        phase: RelocationPhase::Parked {
            temp_tab_id: "t-tmp".into(),
            moved_pane_id: "a".into(),
        },
    };
    assert!(
        parked
            .display_fractions("a", Some(&layout), now, false)
            .is_none()
    );
    assert!(!parked.phase.locks_tab());
    assert_eq!(parked.phase.parked_tab_id(), Some("t-tmp"));
    assert_eq!(inserting(false, false).hidden_tab_id(), Some("t-tmp"));
    assert_eq!(parked.phase.hidden_tab_id(), None, "parked tabs are shown");
}

/// The event stream can name the temporary tab before (or keep it after)
/// the responses do: an unknown tab of the workspace holding only the
/// source pane is the temporary tab; anything else stays visible.
#[test]
fn an_unlisted_tab_holding_only_the_source_pane_is_the_temp_tab() {
    let layout = two_pane_layout();
    let plan = insert_plan(&layout, DropEdge::Right);
    let pane = |pane_id: &str, tab_id: &str| {
        json!({
            "pane_id": pane_id,
            "terminal_id": pane_id,
            "workspace_id": "w",
            "tab_id": tab_id,
            "focused": false,
        })
    };
    let tab = |tab_id: &str, number: usize| {
        json!({
            "tab_id": tab_id,
            "workspace_id": "w",
            "number": number,
            "label": tab_id,
            "focused": false,
            "pane_count": 1,
        })
    };
    let snapshot: HierarchySnapshot = serde_json::from_value(json!({
        "version": "0.9.0",
        "protocol": 14,
        "tabs": [tab("t", 1), tab("t-tmp", 2), tab("t-other", 3), tab("t-empty", 4)],
        "panes": [
            pane("b", "t"),
            pane("a", "t-tmp"),
            pane("c", "t-other"),
        ],
    }))
    .unwrap();
    let hidden: Vec<&str> = plan.unlisted_temp_tabs(&snapshot).collect();
    assert_eq!(
        hidden,
        vec!["t-tmp", "t-empty"],
        "the tab with the source pane and a pane-less newcomer are hidden; \
             a foreign tab and the known tab are not"
    );
    let swap = swap_plan(&layout);
    assert_eq!(
        swap.unlisted_temp_tabs(&snapshot).count(),
        0,
        "a swap creates no tab and hides nothing"
    );
}

#[test]
fn keyboard_move_picks_the_neighbour_and_cycles_zones() {
    let layout = two_pane_layout();
    assert_eq!(
        keyboard_neighbour(&layout, "a", DropEdge::Right).as_deref(),
        Some("b")
    );
    assert_eq!(keyboard_neighbour(&layout, "a", DropEdge::Left), None);
    assert_eq!(keyboard_neighbour(&layout, "a", DropEdge::Up), None);
    assert_eq!(
        keyboard_neighbour(&layout, "b", DropEdge::Left).as_deref(),
        Some("a")
    );
    assert_eq!(
        next_keyboard_zone(DropZone::Center, false),
        DropZone::Center
    );
    assert_eq!(next_keyboard_zone(DropZone::Center, true), DropZone::Left);
    assert_eq!(next_keyboard_zone(DropZone::Down, true), DropZone::Center);
    let mode = KeyboardPaneMove {
        workspace_id: "w".into(),
        tab_id: "t".into(),
        pane_id: "a".into(),
        fingerprint: 0,
        target: Some(PaneDropHover {
            target_pane_id: "b".into(),
            zone: DropZone::Left,
            target_rect: (0., 0., 0., 0.),
        }),
        edge_drops: false,
    };
    assert!(!mode.droppable(), "edges need the flag");
    let mode = KeyboardPaneMove {
        edge_drops: true,
        ..mode
    };
    assert!(mode.droppable());
}
