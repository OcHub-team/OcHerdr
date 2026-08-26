use super::*;
use crate::{LayoutPane, LayoutSplit};

fn rect(x: u16, y: u16, width: u16, height: u16) -> LayoutRect {
    LayoutRect {
        x,
        y,
        width,
        height,
    }
}

fn pane(id: &str, r: LayoutRect) -> LayoutPane {
    LayoutPane {
        pane_id: id.into(),
        focused: false,
        rect: r,
    }
}

fn split(id: &str, direction: SplitDirection, ratio: f32, r: LayoutRect) -> LayoutSplit {
    LayoutSplit {
        id: id.into(),
        direction,
        ratio,
        rect: r,
    }
}

fn layout(area: LayoutRect, panes: Vec<LayoutPane>, splits: Vec<LayoutSplit>) -> PaneLayout {
    PaneLayout {
        workspace_id: "w1".into(),
        tab_id: "t1".into(),
        zoomed: false,
        area,
        focused_pane_id: panes[0].pane_id.clone(),
        panes,
        splits,
    }
}

/// Build a `PaneLayout` the way Herdr would emit it for `tree`, with the
/// one-cell gap chrome applied when `gaps` is set.
fn layout_from_tree(tree: &LayoutTree, gaps: bool) -> PaneLayout {
    let raw = tree.pane_rects();
    let multi = raw.len() > 1;
    let panes = raw
        .iter()
        .map(|p| {
            let mut r = p.rect;
            if multi && gaps {
                let has_right = raw.iter().any(|o| {
                    o.rect.x == r.x + r.width
                        && o.rect.y < r.y + r.height
                        && o.rect.y + o.rect.height > r.y
                });
                let has_below = raw.iter().any(|o| {
                    o.rect.y == r.y + r.height
                        && o.rect.x < r.x + r.width
                        && o.rect.x + o.rect.width > r.x
                });
                if has_right {
                    r.width = r.width.saturating_sub(1);
                }
                if has_below {
                    r.height = r.height.saturating_sub(1);
                }
            }
            pane(&p.pane_id, r)
        })
        .collect();
    let splits = tree
        .splits()
        .into_iter()
        .enumerate()
        .map(|(idx, s)| {
            let path: String = if s.path.is_empty() {
                "root".into()
            } else {
                s.path.iter().map(|b| if *b { '1' } else { '0' }).collect()
            };
            split(&format!("split_{idx}_{path}"), s.direction, s.ratio, s.rect)
        })
        .collect();
    layout(tree.area, panes, splits)
}

fn leaf(id: &str) -> Box<LayoutNode> {
    Box::new(LayoutNode::Pane(id.into()))
}

fn node_split(
    direction: SplitDirection,
    ratio: f32,
    first: Box<LayoutNode>,
    second: Box<LayoutNode>,
) -> Box<LayoutNode> {
    Box::new(LayoutNode::Split {
        direction,
        ratio,
        first,
        second,
    })
}

fn rects(panes: &[PredictedPane]) -> Vec<(&str, LayoutRect)> {
    panes.iter().map(|p| (p.pane_id.as_str(), p.rect)).collect()
}

const AREA: LayoutRect = LayoutRect {
    x: 0,
    y: 0,
    width: 100,
    height: 50,
};

/// `[a | b]` at 0.5.
fn horizontal_tree() -> LayoutTree {
    LayoutTree {
        root: *node_split(SplitDirection::Right, 0.5, leaf("a"), leaf("b")),
        area: AREA,
    }
}

/// `[a / b]` at 0.5.
fn vertical_tree() -> LayoutTree {
    LayoutTree {
        root: *node_split(SplitDirection::Down, 0.5, leaf("a"), leaf("b")),
        area: AREA,
    }
}

/// `[a | [b / [c | d]]]` with ratios 0.3, 0.5, 0.5.
fn nested_tree() -> LayoutTree {
    LayoutTree {
        root: *node_split(
            SplitDirection::Right,
            0.3,
            leaf("a"),
            node_split(
                SplitDirection::Down,
                0.5,
                leaf("b"),
                node_split(SplitDirection::Right, 0.5, leaf("c"), leaf("d")),
            ),
        ),
        area: AREA,
    }
}

#[test]
fn split_rect_rounds_like_herdr() {
    let (a, b) = split_rect(rect(0, 0, 101, 50), SplitDirection::Right, 0.5);
    assert_eq!(a, rect(0, 0, 51, 50));
    assert_eq!(b, rect(51, 0, 50, 50));
    let (a, b) = split_rect(rect(10, 5, 80, 41), SplitDirection::Down, 0.3);
    assert_eq!(a, rect(10, 5, 80, 12));
    assert_eq!(b, rect(10, 17, 80, 29));
}

#[test]
fn valid_split_ratio_mirrors_herdr() {
    assert_eq!(valid_split_ratio(0.5), 0.5);
    assert_eq!(valid_split_ratio(0.0), SPLIT_RATIO_MIN);
    assert_eq!(valid_split_ratio(-3.0), SPLIT_RATIO_MIN);
    assert_eq!(valid_split_ratio(1.0), SPLIT_RATIO_MAX);
    assert_eq!(valid_split_ratio(f32::NAN), 0.5);
    assert_eq!(valid_split_ratio(f32::INFINITY), 0.5);
}

#[test]
fn rebuilds_horizontal_vertical_and_nested_trees_with_and_without_gaps() {
    for tree in [horizontal_tree(), vertical_tree(), nested_tree()] {
        for gaps in [false, true] {
            let layout = layout_from_tree(&tree, gaps);
            assert_eq!(rebuild_tree(&layout), Some(tree.clone()), "gaps={gaps}");
        }
    }
}

#[test]
fn rebuilds_a_single_pane_layout() {
    let layout = layout(AREA, vec![pane("a", AREA)], vec![]);
    assert_eq!(
        rebuild_tree(&layout),
        Some(LayoutTree {
            root: LayoutNode::Pane("a".into()),
            area: AREA
        })
    );
}

#[test]
fn rebuild_rejects_ambiguous_or_inconsistent_snapshots() {
    // Two panes, no splits.
    let l = layout(AREA, vec![pane("a", AREA), pane("b", AREA)], vec![]);
    assert_eq!(rebuild_tree(&l), None);

    // Split whose rect does not match the parent's computed child rect.
    let mut l = layout_from_tree(&nested_tree(), false);
    l.splits[1].rect.x += 1;
    assert_eq!(rebuild_tree(&l), None);

    // A leaf with no pane at its origin.
    let mut l = layout_from_tree(&horizontal_tree(), false);
    l.panes[1].rect.x += 1;
    assert_eq!(rebuild_tree(&l), None);

    // Extra pane the tree does not account for.
    let mut l = layout_from_tree(&horizontal_tree(), false);
    l.panes.push(pane("z", rect(0, 0, 10, 10)));
    assert_eq!(rebuild_tree(&l), None);

    // Orphan split path.
    let mut l = layout_from_tree(&horizontal_tree(), false);
    l.splits.push(split(
        "split_1_1",
        SplitDirection::Down,
        0.5,
        rect(50, 0, 50, 50),
    ));
    assert_eq!(rebuild_tree(&l), None);

    // Unparsable split id.
    let mut l = layout_from_tree(&horizontal_tree(), false);
    l.splits[0].id = "root".into();
    assert_eq!(rebuild_tree(&l), None);

    // Duplicate path.
    let mut l = layout_from_tree(&horizontal_tree(), false);
    let dup = l.splits[0].clone();
    l.splits.push(dup);
    assert_eq!(rebuild_tree(&l), None);
}

#[test]
fn relocating_right_in_a_horizontal_split_collapses_and_resplits() {
    // [a | b], drop a on the right of b: remove a → [b]; split b right
    // with a second → [b | a].
    let layout = layout_from_tree(&horizontal_tree(), true);
    let predicted = predict_relocation(&layout, "a", "b", DropEdge::Right, 0.5).unwrap();
    assert_eq!(
        rects(&predicted.panes),
        vec![("b", rect(0, 0, 50, 50)), ("a", rect(50, 0, 50, 50))]
    );
    assert_eq!(predicted.splits.len(), 1);
    assert_eq!(predicted.splits[0].path, Vec::<bool>::new());
    assert_eq!(predicted.splits[0].direction, SplitDirection::Right);
    assert_eq!(predicted.splits[0].ratio, 0.5);
}

#[test]
fn relocating_left_uses_one_minus_ratio_and_puts_source_first() {
    // [a | b], drop a on the LEFT of b with ratio 0.3: the target b keeps
    // 30% exactly as it would for a right drop. Herdr gets
    // split(b, a, r' = 1 - 0.3 = 0.7), the swap yields split(a, b, 0.7),
    // so a (first child) takes 70 columns and b the remaining 30.
    let layout = layout_from_tree(&horizontal_tree(), false);
    let predicted = predict_relocation(&layout, "a", "b", DropEdge::Left, 0.3).unwrap();
    assert_eq!(DropEdge::Left.request_ratio(0.3), 0.7);
    assert_eq!(
        rects(&predicted.panes),
        vec![("a", rect(0, 0, 70, 50)), ("b", rect(70, 0, 30, 50))]
    );
    assert_eq!(predicted.splits[0].ratio, 0.7);
}

#[test]
fn relocating_up_in_a_vertical_split_swaps_leaves() {
    // [a / b], drop b above a with 0.5 → [b / a].
    let layout = layout_from_tree(&vertical_tree(), true);
    let predicted = predict_relocation(&layout, "b", "a", DropEdge::Up, 0.5).unwrap();
    assert_eq!(
        rects(&predicted.panes),
        vec![("b", rect(0, 0, 100, 25)), ("a", rect(0, 25, 100, 25))]
    );
    assert_eq!(predicted.splits[0].direction, SplitDirection::Down);
}

#[test]
fn relocating_down_onto_a_horizontal_sibling_nests_a_vertical_split() {
    // [a | b], drop a below b → [b / a] over the full area.
    let layout = layout_from_tree(&horizontal_tree(), false);
    let predicted = predict_relocation(&layout, "a", "b", DropEdge::Down, 0.5).unwrap();
    assert_eq!(
        rects(&predicted.panes),
        vec![("b", rect(0, 0, 100, 25)), ("a", rect(0, 25, 100, 25))]
    );
}

#[test]
fn relocating_from_a_deep_subtree_collapses_only_that_split() {
    // [a | [b / [c | d]]]; move d to the right of a.
    // remove d → [a | [b / c]]; split a right with d → [[a | d] | [b / c]].
    let layout = layout_from_tree(&nested_tree(), true);
    let predicted = predict_relocation(&layout, "d", "a", DropEdge::Right, 0.5).unwrap();
    // Root 0.3 of 100 → a-side 30 wide; a|d at 0.5 → 15 / 15.
    // Right side 70 wide, b / c at 0.5 → 25 rows each.
    assert_eq!(
        rects(&predicted.panes),
        vec![
            ("a", rect(0, 0, 15, 50)),
            ("d", rect(15, 0, 15, 50)),
            ("b", rect(30, 0, 70, 25)),
            ("c", rect(30, 25, 70, 25)),
        ]
    );
    let paths: Vec<Vec<bool>> = predicted.splits.iter().map(|s| s.path.clone()).collect();
    assert_eq!(paths, vec![vec![], vec![false], vec![true]]);
}

#[test]
fn relocating_onto_the_adjacent_sibling_keeps_the_outer_split_ratio() {
    // [a | [b / [c | d]]]; move c above d: remove c → [a | [b / d]] (the
    // c|d split collapses, d takes its rect); split d down with c, then
    // swap → [a | [b / [c / d]]].
    let layout = layout_from_tree(&nested_tree(), false);
    let predicted = predict_relocation(&layout, "c", "d", DropEdge::Up, 0.5).unwrap();
    assert_eq!(
        rects(&predicted.panes),
        vec![
            ("a", rect(0, 0, 30, 50)),
            ("b", rect(30, 0, 70, 25)),
            ("c", rect(30, 25, 70, 13)),
            ("d", rect(30, 38, 70, 12)),
        ]
    );
    let directions: Vec<SplitDirection> = predicted.splits.iter().map(|s| s.direction).collect();
    assert_eq!(
        directions,
        vec![
            SplitDirection::Right,
            SplitDirection::Down,
            SplitDirection::Down
        ]
    );
}

#[test]
fn relocating_the_target_parent_sibling_takes_the_whole_area() {
    // [a | b]: move b left of a → remove b → [a]; split a right with b
    // → [a | b]; swap → [b | a].
    let layout = layout_from_tree(&horizontal_tree(), false);
    let predicted = predict_relocation(&layout, "b", "a", DropEdge::Left, 0.5).unwrap();
    assert_eq!(
        rects(&predicted.panes),
        vec![("b", rect(0, 0, 50, 50)), ("a", rect(50, 0, 50, 50))]
    );
}

#[test]
fn relocation_ratio_is_clamped_like_herdr() {
    let layout = layout_from_tree(&horizontal_tree(), false);
    let low = predict_relocation(&layout, "a", "b", DropEdge::Right, 0.0).unwrap();
    assert_eq!(low.splits[0].ratio, SPLIT_RATIO_MIN);
    assert_eq!(rects(&low.panes)[1], ("a", rect(10, 0, 90, 50)));

    let high = predict_relocation(&layout, "a", "b", DropEdge::Right, 5.0).unwrap();
    assert_eq!(high.splits[0].ratio, SPLIT_RATIO_MAX);

    let nan = predict_relocation(&layout, "a", "b", DropEdge::Right, f32::NAN).unwrap();
    assert_eq!(nan.splits[0].ratio, 0.5);

    // Left with ratio 0 → request 1.0 → clamped to 0.9: source keeps 90%.
    let left = predict_relocation(&layout, "a", "b", DropEdge::Left, 0.0).unwrap();
    assert_eq!(left.splits[0].ratio, SPLIT_RATIO_MAX);
    assert_eq!(rects(&left.panes)[0], ("a", rect(0, 0, 90, 50)));
    assert_eq!(DropEdge::Up.request_ratio(f32::NAN), 0.5);
}

#[test]
fn relocation_returns_none_for_single_pane_same_pane_or_unknown_panes() {
    let single = layout(AREA, vec![pane("a", AREA)], vec![]);
    assert_eq!(
        predict_relocation(&single, "a", "a", DropEdge::Right, 0.5),
        None
    );
    assert_eq!(
        predict_relocation(&single, "a", "b", DropEdge::Right, 0.5),
        None
    );

    let layout = layout_from_tree(&horizontal_tree(), false);
    assert_eq!(
        predict_relocation(&layout, "a", "a", DropEdge::Right, 0.5),
        None
    );
    assert_eq!(
        predict_relocation(&layout, "a", "zz", DropEdge::Right, 0.5),
        None
    );
    assert_eq!(
        predict_relocation(&layout, "zz", "a", DropEdge::Right, 0.5),
        None
    );

    // Unreconstructable → None even with valid ids.
    let mut broken = layout.clone();
    broken.splits.clear();
    assert_eq!(
        predict_relocation(&broken, "a", "b", DropEdge::Right, 0.5),
        None
    );
}

#[test]
fn tiny_panes_survive_rounding() {
    // 3-cell-wide area split at 0.5 → first gets round(1.5) = 2, second 1.
    let tree = LayoutTree {
        root: *node_split(SplitDirection::Right, 0.5, leaf("a"), leaf("b")),
        area: rect(0, 0, 3, 1),
    };
    let layout = layout_from_tree(&tree, false);
    assert_eq!(rebuild_tree(&layout), Some(tree));
    let predicted = predict_relocation(&layout, "a", "b", DropEdge::Right, 0.1).unwrap();
    // round(3 * 0.1) = 0 → b gets 0 columns, a gets 3.
    assert_eq!(
        rects(&predicted.panes),
        vec![("b", rect(0, 0, 0, 1)), ("a", rect(0, 0, 3, 1))]
    );
}

#[test]
fn swap_exchanges_only_the_two_rects() {
    let layout = layout_from_tree(&nested_tree(), true);
    let swapped = predict_swap(&layout, "a", "d").unwrap();
    let before = &layout.panes;
    assert_eq!(swapped.len(), before.len());
    for (p, q) in swapped.iter().zip(before) {
        assert_eq!(p.pane_id, q.pane_id);
    }
    assert_eq!(swapped[0].rect, before[3].rect);
    assert_eq!(swapped[3].rect, before[0].rect);
    assert_eq!(swapped[1].rect, before[1].rect);
    assert_eq!(swapped[2].rect, before[2].rect);
    assert_eq!(predict_swap(&layout, "a", "a"), None);
    assert_eq!(predict_swap(&layout, "a", "zz"), None);
}

#[test]
fn drop_zone_center_is_the_inner_44_percent() {
    let r = ZoneRect {
        x: 100.0,
        y: 200.0,
        width: 200.0,
        height: 100.0,
    };
    assert_eq!(drop_zone(r, 200.0, 250.0), Some(DropZone::Center));
    // Centre spans x ∈ [156, 244], y ∈ [228, 272].
    assert_eq!(drop_zone(r, 156.5, 250.0), Some(DropZone::Center));
    assert_eq!(drop_zone(r, 243.5, 250.0), Some(DropZone::Center));
    assert_eq!(drop_zone(r, 200.0, 228.5), Some(DropZone::Center));
    assert_eq!(drop_zone(r, 200.0, 271.5), Some(DropZone::Center));
    assert_eq!(drop_zone(r, 155.0, 250.0), Some(DropZone::Left));
    assert_eq!(drop_zone(r, 245.0, 250.0), Some(DropZone::Right));
    assert_eq!(drop_zone(r, 200.0, 227.0), Some(DropZone::Up));
    assert_eq!(drop_zone(r, 200.0, 273.0), Some(DropZone::Down));
}

#[test]
fn drop_zone_edges_and_corners_follow_the_diagonals() {
    let r = ZoneRect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 50.0,
    };
    assert_eq!(drop_zone(r, 1.0, 25.0), Some(DropZone::Left));
    assert_eq!(drop_zone(r, 99.0, 25.0), Some(DropZone::Right));
    assert_eq!(drop_zone(r, 50.0, 1.0), Some(DropZone::Up));
    assert_eq!(drop_zone(r, 50.0, 49.0), Some(DropZone::Down));
    // Near the top-left corner, slightly closer (proportionally) to the
    // top edge → Up; slightly closer to the left edge → Left.
    assert_eq!(drop_zone(r, 5.0, 2.0), Some(DropZone::Up));
    assert_eq!(drop_zone(r, 2.0, 2.0), Some(DropZone::Left));
    // Bottom-right corner.
    assert_eq!(drop_zone(r, 97.0, 49.5), Some(DropZone::Down));
    assert_eq!(drop_zone(r, 99.0, 49.0), Some(DropZone::Right));
    // Exactly on the corner: tie resolves Left / Right first.
    assert_eq!(drop_zone(r, 0.0, 0.0), Some(DropZone::Left));
    assert_eq!(drop_zone(r, 100.0, 50.0), Some(DropZone::Right));
}

#[test]
fn drop_zone_outside_or_degenerate_is_none() {
    let r = ZoneRect {
        x: 10.0,
        y: 10.0,
        width: 20.0,
        height: 20.0,
    };
    assert_eq!(drop_zone(r, 9.9, 15.0), None);
    assert_eq!(drop_zone(r, 30.1, 15.0), None);
    assert_eq!(drop_zone(r, 15.0, 9.0), None);
    assert_eq!(drop_zone(r, 15.0, 31.0), None);
    assert_eq!(drop_zone(r, f32::NAN, 15.0), None);
    let empty = ZoneRect { width: 0.0, ..r };
    assert_eq!(drop_zone(empty, 10.0, 15.0), None);
}

#[test]
fn drop_zone_maps_to_edges() {
    assert_eq!(DropZone::Center.edge(), None);
    assert_eq!(DropZone::Left.edge(), Some(DropEdge::Left));
    assert_eq!(DropZone::Down.edge(), Some(DropEdge::Down));
    assert_eq!(DropEdge::Left.split_direction(), SplitDirection::Right);
    assert_eq!(DropEdge::Down.split_direction(), SplitDirection::Down);
}

#[test]
fn fingerprint_ignores_ratios_and_rects_but_tracks_structure_and_focus() {
    let base = layout_from_tree(&nested_tree(), false);
    let fp = layout_fingerprint(&base);
    assert_eq!(fp, layout_fingerprint(&base.clone()));

    // Ratio / rect changes (divider drag, window resize) keep it.
    let mut resized = base.clone();
    resized.splits[0].ratio = 0.6;
    for p in &mut resized.panes {
        p.rect.width += 3;
    }
    resized.area.width += 3;
    assert_eq!(layout_fingerprint(&resized), fp);

    // Chrome (gaps) does not change it either.
    assert_eq!(
        layout_fingerprint(&layout_from_tree(&nested_tree(), true)),
        fp
    );

    // A swap changes which leaf sits where.
    let mut swapped = base.clone();
    swapped.panes.swap(0, 3);
    assert_ne!(layout_fingerprint(&swapped), fp);

    // A pane vanishing / appearing.
    let mut fewer = base.clone();
    fewer.panes.pop();
    assert_ne!(layout_fingerprint(&fewer), fp);

    // Split direction / path changes.
    let mut redirected = base.clone();
    redirected.splits[2].direction = SplitDirection::Down;
    assert_ne!(layout_fingerprint(&redirected), fp);

    // Focus and zoom.
    let mut focused = base.clone();
    focused.focused_pane_id = "d".into();
    assert_ne!(layout_fingerprint(&focused), fp);
    let mut zoomed = base.clone();
    zoomed.zoomed = true;
    assert_ne!(layout_fingerprint(&zoomed), fp);

    // Another tab with the same shape is a different fingerprint.
    let mut other_tab = base;
    other_tab.tab_id = "t2".into();
    assert_ne!(layout_fingerprint(&other_tab), fp);
}

fn pane_order(layout: &PredictedLayout) -> Vec<&str> {
    layout.panes.iter().map(|p| p.pane_id.as_str()).collect()
}

#[test]
fn relocation_steps_walk_removed_inserted_final_for_a_left_drop() {
    // [a | [b / [c | d]]]: drop `a` on the left edge of `d`.
    let base = layout_from_tree(&nested_tree(), false);
    let steps = predict_relocation_steps(&base, "a", "d", DropEdge::Left, 0.5).unwrap();
    // Step 1: `a` gone, the root collapses to `[b / [c | d]]`.
    assert_eq!(pane_order(&steps.removed), vec!["b", "c", "d"]);
    assert_eq!(steps.removed.splits.len(), 2);
    assert_eq!(steps.removed.panes[0].rect, rect(0, 0, 100, 25));
    // Step 2: `a` inserted as `d`'s *second* child.
    assert_eq!(pane_order(&steps.inserted), vec!["b", "c", "d", "a"]);
    // Step 3: swapped so `a` is first, and identical to predict_relocation.
    assert_eq!(pane_order(&steps.final_layout), vec!["b", "c", "a", "d"]);
    assert_eq!(
        steps.final_layout,
        predict_relocation(&base, "a", "d", DropEdge::Left, 0.5).unwrap()
    );
    // Same split shape between step 2 and 3: only the leaves trade.
    let shape = |layout: &PredictedLayout| {
        layout
            .splits
            .iter()
            .map(|s| (s.path.clone(), s.direction))
            .collect::<Vec<_>>()
    };
    assert_eq!(shape(&steps.inserted), shape(&steps.final_layout));
}

#[test]
fn relocation_steps_use_one_minus_ratio_on_left_and_up() {
    let base = layout_from_tree(&horizontal_tree(), false);
    let left = predict_relocation_steps(&base, "b", "a", DropEdge::Left, 0.3).unwrap();
    let right = predict_relocation_steps(&base, "b", "a", DropEdge::Right, 0.3).unwrap();
    // The split fed to the tree carries the request ratio.
    assert!((left.inserted.splits[0].ratio - 0.7).abs() < 1e-6);
    assert!((right.inserted.splits[0].ratio - 0.3).abs() < 1e-6);
    assert_eq!(DropEdge::Left.request_ratio(0.3), 0.7);
    assert_eq!(DropEdge::Up.request_ratio(0.3), 0.7);
    assert_eq!(DropEdge::Right.request_ratio(0.3), 0.3);
    assert_eq!(DropEdge::Down.request_ratio(0.3), 0.3);
    // Right/down: inserted == final, so the target keeps 30 cells.
    assert_eq!(right.inserted, right.final_layout);
    assert_eq!(right.final_layout.panes[0].rect, rect(0, 0, 30, 50));
    assert_eq!(right.final_layout.panes[1].rect, rect(30, 0, 70, 50));
    // Left: after the swap `b` is first with 70 cells, `a` keeps 30.
    assert_ne!(left.inserted, left.final_layout);
    assert_eq!(pane_order(&left.final_layout), vec!["b", "a"]);
    assert_eq!(left.final_layout.panes[0].rect, rect(0, 0, 70, 50));
    assert_eq!(left.final_layout.panes[1].rect, rect(70, 0, 30, 50));
}

#[test]
fn relocation_steps_reject_what_predict_relocation_rejects() {
    let base = layout_from_tree(&horizontal_tree(), false);
    assert!(predict_relocation_steps(&base, "a", "a", DropEdge::Left, 0.5).is_none());
    assert!(predict_relocation_steps(&base, "a", "zz", DropEdge::Left, 0.5).is_none());
}

/// `[a | [b / c]]` at 0.5 / 0.5.
fn three_pane_tree() -> LayoutTree {
    LayoutTree {
        root: *node_split(
            SplitDirection::Right,
            0.5,
            leaf("a"),
            node_split(SplitDirection::Down, 0.5, leaf("b"), leaf("c")),
        ),
        area: AREA,
    }
}

fn pane_area(layout: &PredictedLayout) -> u32 {
    layout
        .panes
        .iter()
        .map(|pane| u32::from(pane.rect.width) * u32::from(pane.rect.height))
        .sum()
}

#[test]
fn removing_a_pane_from_two_three_and_four_pane_trees_collapses_the_parent() {
    let two = layout_from_tree(&horizontal_tree(), false);
    let removed = predict_remove_pane(&two, "a").unwrap();
    assert_eq!(pane_order(&removed), vec!["b"]);
    assert!(removed.splits.is_empty());
    assert_eq!(removed.panes[0].rect, AREA);
    assert_eq!(
        pane_area(&removed),
        u32::from(AREA.width) * u32::from(AREA.height)
    );

    let three = layout_from_tree(&three_pane_tree(), false);
    let removed = predict_remove_pane(&three, "a").unwrap();
    assert_eq!(pane_order(&removed), vec!["b", "c"]);
    assert_eq!(removed.splits.len(), 1);
    assert_eq!(removed.splits[0].direction, SplitDirection::Down);
    assert_eq!(removed.panes[0].rect, rect(0, 0, 100, 25));
    assert_eq!(removed.panes[1].rect, rect(0, 25, 100, 25));
    assert_eq!(
        pane_area(&removed),
        u32::from(AREA.width) * u32::from(AREA.height)
    );

    let nested = layout_from_tree(&nested_tree(), false);
    let removed = predict_remove_pane(&nested, "a").unwrap();
    assert_eq!(pane_order(&removed), vec!["b", "c", "d"]);
    assert_eq!(removed.splits.len(), 2);
    assert_eq!(
        pane_area(&removed),
        u32::from(AREA.width) * u32::from(AREA.height)
    );
    let ids: Vec<&str> = removed
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect();
    assert!(!ids.contains(&"a"));
    assert_eq!(ids.len(), 3);
}

#[test]
fn removing_a_nested_leaf_keeps_the_remaining_set_and_area() {
    let three = layout_from_tree(&three_pane_tree(), false);
    let removed = predict_remove_pane(&three, "b").unwrap();
    assert_eq!(pane_order(&removed), vec!["a", "c"]);
    assert_eq!(removed.splits.len(), 1);
    assert_eq!(removed.splits[0].direction, SplitDirection::Right);
    assert_eq!(removed.panes[0].rect, rect(0, 0, 50, 50));
    assert_eq!(removed.panes[1].rect, rect(50, 0, 50, 50));
    assert_eq!(
        pane_area(&removed),
        u32::from(AREA.width) * u32::from(AREA.height)
    );
}

#[test]
fn predict_remove_pane_rejects_the_last_pane_and_unknown_ids() {
    let two = layout_from_tree(&horizontal_tree(), false);
    assert!(predict_remove_pane(&two, "zz").is_none());
    let single = layout(AREA, vec![pane("a", AREA)], vec![]);
    assert!(predict_remove_pane(&single, "a").is_none());
    let mut broken = two.clone();
    broken.splits.clear();
    assert!(predict_remove_pane(&broken, "a").is_none());
}

#[test]
fn inserting_to_the_right_of_a_single_pane_splits_one_to_one() {
    let single = layout(AREA, vec![pane("a", AREA)], vec![]);
    let predicted = predict_insert_pane(&single, "src", "a").unwrap();
    assert_eq!(pane_order(&predicted), vec!["a", "src"]);
    assert_eq!(predicted.splits.len(), 1);
    assert_eq!(predicted.splits[0].direction, SplitDirection::Right);
    assert!((predicted.splits[0].ratio - 0.5).abs() < 1e-6);
    assert_eq!(predicted.panes[0].rect, rect(0, 0, 50, 50));
    assert_eq!(predicted.panes[1].rect, rect(50, 0, 50, 50));
    assert_eq!(
        pane_area(&predicted),
        u32::from(AREA.width) * u32::from(AREA.height)
    );
}

#[test]
fn inserting_into_two_and_nested_layouts_keeps_the_set_and_area() {
    let two = layout_from_tree(&horizontal_tree(), false);
    let on_a = predict_insert_pane(&two, "src", "a").unwrap();
    assert_eq!(pane_order(&on_a), vec!["a", "src", "b"]);
    assert_eq!(
        pane_area(&on_a),
        u32::from(AREA.width) * u32::from(AREA.height)
    );
    let on_b = predict_insert_pane(&two, "src", "b").unwrap();
    assert_eq!(pane_order(&on_b), vec!["a", "b", "src"]);
    assert_eq!(
        pane_area(&on_b),
        u32::from(AREA.width) * u32::from(AREA.height)
    );

    let nested = layout_from_tree(&nested_tree(), false);
    let predicted = predict_insert_pane(&nested, "src", "d").unwrap();
    assert_eq!(pane_order(&predicted), vec!["a", "b", "c", "d", "src"]);
    assert!(
        !predicted
            .panes
            .iter()
            .any(|pane| pane.pane_id == "src" && pane.rect.width == 0 && pane.rect.height == 0)
    );
    assert_eq!(
        pane_area(&predicted),
        u32::from(AREA.width) * u32::from(AREA.height)
    );
}

#[test]
fn predict_insert_pane_rejects_illegal_inputs() {
    let two = layout_from_tree(&horizontal_tree(), false);
    assert!(predict_insert_pane(&two, "a", "a").is_none());
    assert!(predict_insert_pane(&two, "src", "zz").is_none());
    assert!(predict_insert_pane(&two, "b", "a").is_none());
    let mut broken = two.clone();
    broken.splits.clear();
    assert!(predict_insert_pane(&broken, "src", "a").is_none());
    let empty = PaneLayout {
        workspace_id: "w1".into(),
        tab_id: "t1".into(),
        zoomed: false,
        area: AREA,
        focused_pane_id: String::new(),
        panes: Vec::new(),
        splits: Vec::new(),
    };
    assert!(predict_insert_pane(&empty, "src", "a").is_none());
}
