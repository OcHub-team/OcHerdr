//! Divider dragging that moves one divider only (tmux / VS Code behaviour).
//!
//! When the ratio of a split changes, Herdr rescales both of its children,
//! so every nested divider of the same direction slides along. To keep
//! them where they are, the client retunes the ratios of those nested
//! splits so their divider positions in cells stay put, and sends one
//! `layout.set_split_ratio` per retuned split right after the dragged one.
//!
//! Everything is computed in whole cells with Herdr's `split_rect` rounding,
//! so the squeeze preview drawn from these ratios equals the layout Herdr
//! settles on.

use super::relocation::{LayoutNode, LayoutTree, split_rect, valid_split_ratio};
use super::{LayoutRect, SplitDirection};

/// Ratios to apply (and send) when the split at `path` is dragged to
/// `new_ratio`: the dragged split first, then every same-direction
/// descendant whose ratio must change so its divider stays on the same
/// cell, in pre-order. Splits of the other direction, ancestors and the
/// rest of the tree are untouched.
///
/// The dragged ratio is clamped like Herdr (`0.1..=0.9`) and limited so
/// that neither pane touching the divider drops below one cell on the drag
/// axis. A pinned divider whose ratio would leave the clamp is clamped
/// instead, so it moves the minimum Herdr allows.
///
/// Empty when `path` does not name a split.
pub fn pinned_ratios(tree: &LayoutTree, path: &[bool], new_ratio: f32) -> Vec<(Vec<bool>, f32)> {
    let Some((node, rect)) = locate(&tree.root, tree.area, path) else {
        return Vec::new();
    };
    let LayoutNode::Split {
        direction,
        ratio,
        first,
        second,
    } = node
    else {
        return Vec::new();
    };
    let direction = *direction;
    let old_ratio = *ratio;
    let (old_first, old_second) = split_rect(rect, direction, old_ratio);
    let total = axis_size(rect, direction);
    if total == 0 {
        return Vec::new();
    }

    // Limit the dragged ratio so the panes touching the divider keep at
    // least one cell: the first child's size may shrink by at most (adjacent
    // pane − 1) and grow by at most (adjacent pane in the second child − 1).
    let mut dragged = valid_split_ratio(new_ratio);
    let first_size = axis_size(old_first, direction);
    let adjacent_first = edge_pane_size(first, old_first, direction, true);
    let adjacent_second = edge_pane_size(second, old_second, direction, false);
    let min_first = first_size.saturating_sub(adjacent_first.saturating_sub(1));
    let max_first = first_size.saturating_add(adjacent_second.saturating_sub(1));
    let wanted_first = (f32::from(total) * dragged).round() as u16;
    if wanted_first < min_first {
        dragged = valid_split_ratio(f32::from(min_first) / f32::from(total));
    } else if wanted_first > max_first {
        dragged = valid_split_ratio(f32::from(max_first) / f32::from(total));
    }

    let (new_first, new_second) = split_rect(rect, direction, dragged);
    let mut out = vec![(path.to_vec(), dragged)];
    let mut child_path = path.to_vec();
    child_path.push(false);
    pin_subtree(
        first,
        old_first,
        new_first,
        direction,
        &mut child_path,
        &mut out,
    );
    child_path.pop();
    child_path.push(true);
    pin_subtree(
        second,
        old_second,
        new_second,
        direction,
        &mut child_path,
        &mut out,
    );
    out
}

/// Apply a list of `(path, ratio)` overrides to a tree, in place.
pub fn apply_ratios(node: &mut LayoutNode, ratios: &[(Vec<bool>, f32)]) {
    for (path, ratio) in ratios {
        set_ratio_at(node, path, *ratio);
    }
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

fn locate<'a>(
    node: &'a LayoutNode,
    rect: LayoutRect,
    path: &[bool],
) -> Option<(&'a LayoutNode, LayoutRect)> {
    match path.split_first() {
        None => Some((node, rect)),
        Some((step, rest)) => match node {
            LayoutNode::Pane(_) => None,
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_rect(rect, *direction, *ratio);
                if *step {
                    locate(second, b, rest)
                } else {
                    locate(first, a, rest)
                }
            }
        },
    }
}

fn axis_size(rect: LayoutRect, direction: SplitDirection) -> u16 {
    match direction {
        SplitDirection::Right => rect.width,
        SplitDirection::Down => rect.height,
    }
}

fn axis_origin(rect: LayoutRect, direction: SplitDirection) -> u16 {
    match direction {
        SplitDirection::Right => rect.x,
        SplitDirection::Down => rect.y,
    }
}

/// Size along `direction` of the pane touching the subtree's trailing
/// (`trailing = true`) or leading edge on that axis. Same-direction splits
/// pick the child on that edge; other-direction splits could expose two
/// panes, the smaller one bounds the drag.
fn edge_pane_size(
    node: &LayoutNode,
    rect: LayoutRect,
    direction: SplitDirection,
    trailing: bool,
) -> u16 {
    match node {
        LayoutNode::Pane(_) => axis_size(rect, direction),
        LayoutNode::Split {
            direction: d,
            ratio,
            first,
            second,
        } => {
            let (a, b) = split_rect(rect, *d, *ratio);
            if *d == direction {
                if trailing {
                    edge_pane_size(second, b, direction, trailing)
                } else {
                    edge_pane_size(first, a, direction, trailing)
                }
            } else {
                edge_pane_size(first, a, direction, trailing)
                    .min(edge_pane_size(second, b, direction, trailing))
            }
        }
    }
}

/// Walk a subtree whose rect moved from `old` to `new` along `direction`,
/// retuning same-direction splits so their dividers stay on the same cell.
fn pin_subtree(
    node: &LayoutNode,
    old: LayoutRect,
    new: LayoutRect,
    direction: SplitDirection,
    path: &mut Vec<bool>,
    out: &mut Vec<(Vec<bool>, f32)>,
) {
    let LayoutNode::Split {
        direction: d,
        ratio,
        first,
        second,
    } = node
    else {
        return;
    };
    let (old_first, old_second) = split_rect(old, *d, *ratio);
    let mut new_ratio = *ratio;
    if *d == direction && old != new {
        let divider =
            axis_origin(old_first, direction).saturating_add(axis_size(old_first, direction));
        let size = axis_size(new, direction);
        if size > 0 {
            let offset = divider
                .saturating_sub(axis_origin(new, direction))
                .min(size);
            new_ratio = valid_split_ratio(f32::from(offset) / f32::from(size));
            if (new_ratio - *ratio).abs() > f32::EPSILON {
                out.push((path.clone(), new_ratio));
            }
        }
    }
    let (new_first, new_second) = split_rect(new, *d, new_ratio);
    path.push(false);
    pin_subtree(first, old_first, new_first, direction, path, out);
    path.pop();
    path.push(true);
    pin_subtree(second, old_second, new_second, direction, path, out);
    path.pop();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SPLIT_RATIO_MAX, SPLIT_RATIO_MIN};

    const AREA: LayoutRect = LayoutRect {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    };

    fn leaf(id: &str) -> Box<LayoutNode> {
        Box::new(LayoutNode::Pane(id.into()))
    }

    fn split(
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

    fn tree(root: Box<LayoutNode>) -> LayoutTree {
        #![allow(clippy::boxed_local)]
        LayoutTree {
            root: *root,
            area: AREA,
        }
    }

    /// Divider cell of every split, keyed by path: the far edge of the
    /// first child on the split axis.
    fn divider_cells(tree: &LayoutTree) -> Vec<(Vec<bool>, u16)> {
        tree.splits()
            .iter()
            .map(|s| {
                let (first, _) = split_rect(s.rect, s.direction, s.ratio);
                let cell = match s.direction {
                    SplitDirection::Right => first.x + first.width,
                    SplitDirection::Down => first.y + first.height,
                };
                (s.path.clone(), cell)
            })
            .collect()
    }

    fn retuned(tree: &LayoutTree, ratios: &[(Vec<bool>, f32)]) -> LayoutTree {
        let mut out = tree.clone();
        apply_ratios(&mut out.root, ratios);
        out
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    /// `right[ right[p1, p2] | p3 ]`, all 0.5: dividers at 25 and 50.
    fn nested_left() -> LayoutTree {
        tree(split(
            SplitDirection::Right,
            0.5,
            split(SplitDirection::Right, 0.5, leaf("p1"), leaf("p2")),
            leaf("p3"),
        ))
    }

    /// `right[ p1 | right[p2, p3] ]`, all 0.5: dividers at 50 and 75.
    fn nested_right() -> LayoutTree {
        tree(split(
            SplitDirection::Right,
            0.5,
            leaf("p1"),
            split(SplitDirection::Right, 0.5, leaf("p2"), leaf("p3")),
        ))
    }

    #[test]
    fn dragging_the_outer_divider_keeps_a_nested_divider_on_the_shrinking_side() {
        let tree = nested_left();
        // Outer divider 50 → 60: the left subtree grows to 60 cells, its
        // divider must stay on cell 25, so 25/60.
        let ratios = pinned_ratios(&tree, &[], 0.6);
        assert_eq!(ratios.len(), 2);
        assert_eq!(ratios[0].0, Vec::<bool>::new());
        assert!(close(ratios[0].1, 0.6));
        assert_eq!(ratios[1].0, vec![false]);
        assert!(close(ratios[1].1, 25. / 60.), "{}", ratios[1].1);
        let after = retuned(&tree, &ratios);
        assert_eq!(divider_cells(&after), vec![(vec![], 60), (vec![false], 25)]);

        // Outer divider 50 → 40: the left subtree shrinks, p2 absorbs it.
        let ratios = pinned_ratios(&tree, &[], 0.4);
        assert!(close(ratios[1].1, 25. / 40.));
        let after = retuned(&tree, &ratios);
        assert_eq!(divider_cells(&after), vec![(vec![], 40), (vec![false], 25)]);
    }

    #[test]
    fn dragging_the_outer_divider_keeps_a_nested_divider_on_the_growing_side() {
        let tree = nested_right();
        // Outer divider 50 → 30: the right subtree spans 30..100, its
        // divider stays on 75: (75 − 30) / 70.
        let ratios = pinned_ratios(&tree, &[], 0.3);
        assert_eq!(ratios.len(), 2);
        assert_eq!(ratios[1].0, vec![true]);
        assert!(close(ratios[1].1, 45. / 70.));
        let after = retuned(&tree, &ratios);
        assert_eq!(divider_cells(&after), vec![(vec![], 30), (vec![true], 75)]);
    }

    #[test]
    fn dragging_the_inner_divider_touches_nothing_else() {
        let tree = nested_left();
        let ratios = pinned_ratios(&tree, &[false], 0.8);
        assert_eq!(ratios, vec![(vec![false], 0.8)]);
    }

    #[test]
    fn splits_of_the_other_direction_are_untouched_but_their_descendants_are_pinned() {
        // right[ down[ right[a, b], c ] | d ]
        let tree = tree(split(
            SplitDirection::Right,
            0.5,
            split(
                SplitDirection::Down,
                0.4,
                split(SplitDirection::Right, 0.5, leaf("a"), leaf("b")),
                leaf("c"),
            ),
            leaf("d"),
        ));
        let before = divider_cells(&tree);
        let ratios = pinned_ratios(&tree, &[], 0.7);
        assert_eq!(ratios.len(), 2, "{ratios:?}");
        assert_eq!(ratios[1].0, vec![false, false]);
        let after = divider_cells(&retuned(&tree, &ratios));
        assert_eq!(after[0], (vec![], 70));
        assert_eq!(
            after[1], before[1],
            "the down split keeps its ratio and cell"
        );
        assert_eq!(after[2], before[2], "a|b stays on its cell");
        assert!(close(
            tree.splits()[1].ratio,
            retuned(&tree, &ratios).splits()[1].ratio
        ));
    }

    #[test]
    fn a_pinned_divider_that_leaves_the_clamp_moves_the_minimum() {
        // Outer 50 → 90 (the max): left subtree 0..90, inner divider at 25
        // would need 25/90 ≈ 0.28, fine. Shrink instead: 50 → 27 gives the
        // inner divider 25/27 > 0.9, clamped to 0.9.
        let tree = nested_left();
        let ratios = pinned_ratios(&tree, &[], 0.27);
        assert_eq!(ratios.len(), 2);
        assert_eq!(ratios[1].1, SPLIT_RATIO_MAX);
        let after = retuned(&tree, &ratios);
        assert_eq!(divider_cells(&after), vec![(vec![], 27), (vec![false], 24)]);

        // Growing side: the right subtree's divider is pinned at 75; the
        // outer divider dragged right stops at 74 (p2 keeps one cell) and
        // 1/26 is below the clamp, so the inner divider moves to the minimum.
        let tree = nested_right();
        let ratios = pinned_ratios(&tree, &[], 0.9);
        assert!(close(ratios[0].1, 0.74), "{ratios:?}");
        assert_eq!(ratios[1].1, SPLIT_RATIO_MIN);
    }

    #[test]
    fn the_dragged_ratio_is_clamped_and_limited_by_the_adjacent_panes() {
        let flat = tree(split(SplitDirection::Right, 0.5, leaf("a"), leaf("b")));
        assert_eq!(pinned_ratios(&flat, &[], 5.0)[0].1, SPLIT_RATIO_MAX);
        assert_eq!(pinned_ratios(&flat, &[], -1.0)[0].1, SPLIT_RATIO_MIN);
        // Nested: the pane next to the divider bounds the drag. p1|p2 at
        // 0.5 leaves p2 25 cells, so the outer divider stops at 26.
        let ratios = pinned_ratios(&nested_left(), &[], -1.0);
        assert!(close(ratios[0].1, 26. / 100.), "{ratios:?}");
        // A 3-cell pane next to the divider stops the drag two cells short.
        let small = LayoutTree {
            root: LayoutNode::Split {
                direction: SplitDirection::Right,
                ratio: 0.5,
                first: Box::new(LayoutNode::Split {
                    direction: SplitDirection::Right,
                    ratio: 0.94,
                    first: leaf("p1"),
                    second: leaf("p2"),
                }),
                second: leaf("p3"),
            },
            area: AREA,
        };
        let (first, _) = split_rect(AREA, SplitDirection::Right, 0.5);
        let (_, p2) = split_rect(first, SplitDirection::Right, 0.94);
        assert_eq!(p2.width, 3);
        let ratios = pinned_ratios(&small, &[], 0.1);
        assert!(close(ratios[0].1, 48. / 100.), "{ratios:?}");
    }

    #[test]
    fn not_a_split_yields_nothing() {
        let tree = nested_left();
        assert!(pinned_ratios(&tree, &[false, false], 0.5).is_empty());
        assert!(pinned_ratios(&tree, &[true, true], 0.5).is_empty());
        assert!(pinned_ratios(&tree, &[true], 0.5).is_empty());
    }

    #[test]
    fn applying_all_ratios_keeps_every_divider_but_the_dragged_one() {
        // down[ right[ right[a, b] | down[c, d] ] / right[e, right[f, g]] ]
        let tree = tree(split(
            SplitDirection::Down,
            0.5,
            split(
                SplitDirection::Right,
                0.6,
                split(SplitDirection::Right, 0.3, leaf("a"), leaf("b")),
                split(SplitDirection::Down, 0.5, leaf("c"), leaf("d")),
            ),
            split(
                SplitDirection::Right,
                0.5,
                leaf("e"),
                split(SplitDirection::Right, 0.5, leaf("f"), leaf("g")),
            ),
        ));
        for (path, ratio) in [
            (vec![], 0.7_f32),
            (vec![], 0.3),
            (vec![false], 0.35),
            (vec![false], 0.85),
            (vec![true], 0.2),
            (vec![true], 0.8),
            (vec![true, true], 0.1),
        ] {
            let before = divider_cells(&tree);
            let ratios = pinned_ratios(&tree, &path, ratio);
            let after = divider_cells(&retuned(&tree, &ratios));
            assert_eq!(before.len(), after.len());
            for ((p, cell_before), (_, cell_after)) in before.iter().zip(&after) {
                if *p == path {
                    continue;
                }
                // A divider Herdr's clamp refuses to keep moves the minimum.
                let clamped = ratios
                    .iter()
                    .any(|(rp, r)| rp == p && (*r == SPLIT_RATIO_MIN || *r == SPLIT_RATIO_MAX));
                if clamped {
                    continue;
                }
                assert_eq!(
                    cell_before, cell_after,
                    "{path:?}={ratio}: divider {p:?} moved ({ratios:?})"
                );
            }
        }
    }
}
