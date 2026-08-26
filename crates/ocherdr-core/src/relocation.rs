//! Predicted pane geometry for drag relocation (design §6.3).
//!
//! Pure functions over a [`PaneLayout`]. Nothing here talks to Herdr; the
//! caller renders these predictions while the real `pane.move` / `pane.swap`
//! round-trips are in flight and discards them once `layout.updated` lands.
//!
//! The tree operations mirror Herdr's `src/layout.rs` (`split_at`,
//! `remove_pane`, `swap_pane_ids`, `valid_split_ratio`, `split_rect`) so the
//! prediction lands where the authority will actually put the panes. Rects are
//! the raw split rects in terminal cells; Herdr's pane chrome (one-cell gaps)
//! is not applied.

use std::collections::{HashMap, HashSet};

use super::{LayoutRect, PaneLayout, SPLIT_RATIO_MAX, SPLIT_RATIO_MIN, SplitDirection};

/// Node of the binary split tree behind a [`PaneLayout`].
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutNode {
    Pane(String),
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

/// Binary split tree plus the area it is laid out into.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutTree {
    pub root: LayoutNode,
    pub area: LayoutRect,
}

/// A pane rect in the predicted layout, in tree order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictedPane {
    pub pane_id: String,
    pub rect: LayoutRect,
}

/// A split in the predicted layout, in the same order Herdr enumerates them
/// (pre-order). `path` matches `parse_split_path_id` (`false` = first child).
#[derive(Debug, Clone, PartialEq)]
pub struct PredictedSplit {
    pub path: Vec<bool>,
    pub direction: SplitDirection,
    pub ratio: f32,
    pub rect: LayoutRect,
}

/// Result of [`predict_relocation`].
#[derive(Debug, Clone, PartialEq)]
pub struct PredictedLayout {
    pub tree: LayoutTree,
    pub panes: Vec<PredictedPane>,
    pub splits: Vec<PredictedSplit>,
}

/// Edge of the target pane the source is dropped on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropEdge {
    Left,
    Right,
    Up,
    Down,
}

impl DropEdge {
    /// Direction of the split Herdr creates for this edge.
    pub fn split_direction(self) -> SplitDirection {
        match self {
            Self::Left | Self::Right => SplitDirection::Right,
            Self::Up | Self::Down => SplitDirection::Down,
        }
    }

    /// The ratio to send Herdr (and feed the tree) so that the *target* pane
    /// keeps `ratio` of its rect on every edge, exactly as a plain right/down
    /// `pane.move` would. Left/up drops are inserted as right/down then
    /// swapped, so the moved pane becomes the *first* child; Herdr's ratio
    /// is the first child's share, hence `1 - ratio` there (design §4.2).
    /// Clamped like Herdr's `valid_split_ratio` after the subtraction.
    pub fn request_ratio(self, ratio: f32) -> f32 {
        match self {
            Self::Left | Self::Up => valid_split_ratio(1.0 - ratio),
            Self::Right | Self::Down => valid_split_ratio(ratio),
        }
    }

    /// Whether the moved pane ends up as the first child.
    pub fn moved_pane_is_first(self) -> bool {
        matches!(self, Self::Left | Self::Up)
    }
}

/// Five-zone hit test over the target pane (design §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropZone {
    Center,
    Left,
    Right,
    Up,
    Down,
}

impl DropZone {
    pub fn edge(self) -> Option<DropEdge> {
        match self {
            Self::Center => None,
            Self::Left => Some(DropEdge::Left),
            Self::Right => Some(DropEdge::Right),
            Self::Up => Some(DropEdge::Up),
            Self::Down => Some(DropEdge::Down),
        }
    }
}

/// Rect in whatever unit the pointer is in (pixels for the GUI, cells for
/// tests). `drop_zone` only needs proportions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Fraction of each axis taken by the centre swap zone.
pub const DROP_ZONE_CENTER_FRACTION: f32 = 0.44;

/// Herdr's `valid_split_ratio`: clamp to `0.1..=0.9`, non-finite falls back
/// to `0.5`.
pub fn valid_split_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(SPLIT_RATIO_MIN, SPLIT_RATIO_MAX)
    } else {
        0.5
    }
}

/// Herdr's `split_rect`: the first child gets `round(size * ratio)` cells.
pub fn split_rect(
    area: LayoutRect,
    direction: SplitDirection,
    ratio: f32,
) -> (LayoutRect, LayoutRect) {
    match direction {
        SplitDirection::Right => {
            let first_w = (f32::from(area.width) * ratio).round() as u16;
            let second_w = area.width.saturating_sub(first_w);
            (
                LayoutRect {
                    x: area.x,
                    y: area.y,
                    width: first_w,
                    height: area.height,
                },
                LayoutRect {
                    x: area.x.saturating_add(first_w),
                    y: area.y,
                    width: second_w,
                    height: area.height,
                },
            )
        }
        SplitDirection::Down => {
            let first_h = (f32::from(area.height) * ratio).round() as u16;
            let second_h = area.height.saturating_sub(first_h);
            (
                LayoutRect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: first_h,
                },
                LayoutRect {
                    x: area.x,
                    y: area.y.saturating_add(first_h),
                    width: area.width,
                    height: second_h,
                },
            )
        }
    }
}

impl LayoutNode {
    pub fn pane_ids(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_ids(&mut out);
        out
    }

    fn collect_ids<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Self::Pane(id) => out.push(id),
            Self::Split { first, second, .. } => {
                first.collect_ids(out);
                second.collect_ids(out);
            }
        }
    }

    pub fn contains(&self, pane_id: &str) -> bool {
        match self {
            Self::Pane(id) => id == pane_id,
            Self::Split { first, second, .. } => {
                first.contains(pane_id) || second.contains(pane_id)
            }
        }
    }

    fn collect_panes(&self, area: LayoutRect, out: &mut Vec<PredictedPane>) {
        match self {
            Self::Pane(id) => out.push(PredictedPane {
                pane_id: id.clone(),
                rect: area,
            }),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_rect(area, *direction, *ratio);
                first.collect_panes(a, out);
                second.collect_panes(b, out);
            }
        }
    }

    fn collect_splits(
        &self,
        area: LayoutRect,
        path: &mut Vec<bool>,
        out: &mut Vec<PredictedSplit>,
    ) {
        if let Self::Split {
            direction,
            ratio,
            first,
            second,
        } = self
        {
            let (a, b) = split_rect(area, *direction, *ratio);
            out.push(PredictedSplit {
                path: path.clone(),
                direction: *direction,
                ratio: *ratio,
                rect: area,
            });
            path.push(false);
            first.collect_splits(a, path, out);
            path.pop();
            path.push(true);
            second.collect_splits(b, path, out);
            path.pop();
        }
    }
}

impl LayoutTree {
    /// Pane rects in tree order, mirroring Herdr's `TileLayout::panes`.
    pub fn pane_rects(&self) -> Vec<PredictedPane> {
        let mut out = Vec::new();
        self.root.collect_panes(self.area, &mut out);
        out
    }

    /// Splits in pre-order, mirroring Herdr's `TileLayout::splits`.
    pub fn splits(&self) -> Vec<PredictedSplit> {
        let mut out = Vec::new();
        self.root
            .collect_splits(self.area, &mut Vec::new(), &mut out);
        out
    }

    pub fn pane_count(&self) -> usize {
        self.root.pane_ids().len()
    }
}

/// Rebuild the binary split tree from a `layout.updated` snapshot.
///
/// Splits carry their path (`split_{n}_{01…}`), direction, ratio and raw
/// rect; leaves are matched to panes by rect origin, because Herdr's pane
/// chrome may shrink a pane's width/height by one cell but never moves it.
/// Returns `None` when anything is ambiguous or inconsistent: unparsable or
/// duplicate split paths, a split whose rect does not match what its parent
/// predicts, a leaf without exactly one pane, or leftover panes/splits.
pub fn rebuild_tree(layout: &PaneLayout) -> Option<LayoutTree> {
    if layout.splits.is_empty() {
        if layout.panes.len() != 1 {
            return None;
        }
        let pane = &layout.panes[0];
        if !rect_fits(pane.rect, layout.area) {
            return None;
        }
        return Some(LayoutTree {
            root: LayoutNode::Pane(pane.pane_id.clone()),
            area: layout.area,
        });
    }

    let mut splits = HashMap::with_capacity(layout.splits.len());
    for split in &layout.splits {
        let path = split.path()?;
        if splits.insert(path, split).is_some() {
            return None;
        }
    }
    let mut panes: HashMap<(u16, u16), Vec<&super::LayoutPane>> = HashMap::new();
    for pane in &layout.panes {
        panes
            .entry((pane.rect.x, pane.rect.y))
            .or_default()
            .push(pane);
    }

    let mut used_splits = 0usize;
    let mut used_panes = HashSet::new();
    let root = build_node(
        &splits,
        &panes,
        layout.area,
        &mut Vec::new(),
        &mut used_splits,
        &mut used_panes,
    )?;
    if used_splits != splits.len() || used_panes.len() != layout.panes.len() {
        return None;
    }
    Some(LayoutTree {
        root,
        area: layout.area,
    })
}

fn build_node<'a>(
    splits: &HashMap<Vec<bool>, &'a super::LayoutSplit>,
    panes: &HashMap<(u16, u16), Vec<&'a super::LayoutPane>>,
    area: LayoutRect,
    path: &mut Vec<bool>,
    used_splits: &mut usize,
    used_panes: &mut HashSet<&'a str>,
) -> Option<LayoutNode> {
    if let Some(split) = splits.get(path.as_slice()) {
        if split.rect != area || !split.ratio.is_finite() {
            return None;
        }
        *used_splits += 1;
        let (a, b) = split_rect(area, split.direction, split.ratio);
        path.push(false);
        let first = build_node(splits, panes, a, path, used_splits, used_panes);
        path.pop();
        let first = first?;
        path.push(true);
        let second = build_node(splits, panes, b, path, used_splits, used_panes);
        path.pop();
        return Some(LayoutNode::Split {
            direction: split.direction,
            ratio: split.ratio,
            first: Box::new(first),
            second: Box::new(second?),
        });
    }

    let candidates = panes.get(&(area.x, area.y))?;
    let mut matched = candidates
        .iter()
        .filter(|pane| rect_fits(pane.rect, area) && !used_panes.contains(pane.pane_id.as_str()));
    let pane = matched.next()?;
    if matched.next().is_some() {
        return None;
    }
    used_panes.insert(pane.pane_id.as_str());
    Some(LayoutNode::Pane(pane.pane_id.clone()))
}

fn rect_fits(inner: LayoutRect, outer: LayoutRect) -> bool {
    inner.x == outer.x
        && inner.y == outer.y
        && inner.width <= outer.width
        && inner.height <= outer.height
}

/// Herdr's `remove_pane`: the parent split collapses and the sibling subtree
/// takes the parent's rect.
pub fn remove_pane(node: LayoutNode, target: &str) -> Option<LayoutNode> {
    match node {
        LayoutNode::Pane(id) if id == target => None,
        LayoutNode::Pane(id) => Some(LayoutNode::Pane(id)),
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => match (remove_pane(*first, target), remove_pane(*second, target)) {
            (None, Some(s)) => Some(s),
            (Some(f), None) => Some(f),
            (Some(f), Some(s)) => Some(LayoutNode::Split {
                direction,
                ratio,
                first: Box::new(f),
                second: Box::new(s),
            }),
            (None, None) => None,
        },
    }
}

/// Herdr's `split_at`: `target` becomes the first child, `new_id` the second.
/// `ratio` must already be validated.
pub fn split_at(
    node: LayoutNode,
    target: &str,
    direction: SplitDirection,
    new_id: &str,
    ratio: f32,
) -> LayoutNode {
    match node {
        LayoutNode::Pane(id) if id == target => LayoutNode::Split {
            direction,
            ratio,
            first: Box::new(LayoutNode::Pane(id)),
            second: Box::new(LayoutNode::Pane(new_id.to_owned())),
        },
        LayoutNode::Pane(id) => LayoutNode::Pane(id),
        LayoutNode::Split {
            direction: d,
            ratio: r,
            first,
            second,
        } => LayoutNode::Split {
            direction: d,
            ratio: r,
            first: Box::new(split_at(*first, target, direction, new_id, ratio)),
            second: Box::new(split_at(*second, target, direction, new_id, ratio)),
        },
    }
}

/// Herdr's `swap_pane_ids`: exchange two leaf ids, keeping the split shape.
pub fn swap_pane_ids(node: &mut LayoutNode, first: &str, second: &str) {
    match node {
        LayoutNode::Pane(id) if id == first => *id = second.to_owned(),
        LayoutNode::Pane(id) if id == second => *id = first.to_owned(),
        LayoutNode::Pane(_) => {}
        LayoutNode::Split {
            first: a,
            second: b,
            ..
        } => {
            swap_pane_ids(a, first, second);
            swap_pane_ids(b, first, second);
        }
    }
}

/// Predict the layout after dropping `source` on `edge` of `target`.
/// `ratio` is the share the *target* keeps of its current rect (the same
/// meaning as the `ratio` of a right/down `pane.move`); the source gets the
/// rest.
///
/// Mirrors design §4.2: remove the source (its parent split collapses), then
/// `split_at` the target with the source as second child, then for left/up
/// swap the two leaves. The split ratio handed to the tree is
/// [`DropEdge::request_ratio`] — the same value the caller must send to Herdr.
///
/// Returns `None` when the tree cannot be rebuilt, the layout has fewer than
/// two panes, `source == target`, or either pane is missing.
pub fn predict_relocation(
    layout: &PaneLayout,
    source: &str,
    target: &str,
    edge: DropEdge,
    ratio: f32,
) -> Option<PredictedLayout> {
    if source == target {
        return None;
    }
    let tree = rebuild_tree(layout)?;
    if tree.pane_count() < 2 || !tree.root.contains(source) || !tree.root.contains(target) {
        return None;
    }
    let request_ratio = edge.request_ratio(ratio);
    let root = remove_pane(tree.root, source)?;
    let mut root = split_at(root, target, edge.split_direction(), source, request_ratio);
    if edge.moved_pane_is_first() {
        swap_pane_ids(&mut root, source, target);
    }
    let tree = LayoutTree {
        root,
        area: tree.area,
    };
    let panes = tree.pane_rects();
    let splits = tree.splits();
    Some(PredictedLayout {
        tree,
        panes,
        splits,
    })
}

/// Predict the layout after removing `pane_id`. The parent split collapses
/// and the sibling subtree fills the parent's rect, matching Herdr's
/// `remove_pane`.
///
/// Returns `None` when the tree cannot be rebuilt, the pane is missing, or
/// it is the last pane (nothing remains to lay out).
pub fn predict_remove_pane(layout: &PaneLayout, pane_id: &str) -> Option<PredictedLayout> {
    let tree = rebuild_tree(layout)?;
    if !tree.root.contains(pane_id) {
        return None;
    }
    let root = remove_pane(tree.root, pane_id)?;
    let tree = LayoutTree {
        root,
        area: tree.area,
    };
    let panes = tree.pane_rects();
    let splits = tree.splits();
    Some(PredictedLayout {
        tree,
        panes,
        splits,
    })
}

/// Predict the layout after inserting `source` to the right of `anchor` at
/// 1:1 (`split = right`, `ratio = 0.5`). The source is not already in this
/// tree: it is arriving from another tab.
///
/// Returns `None` when the tree cannot be rebuilt, `anchor` is missing,
/// `source == anchor`, or `source` is already a leaf of this layout.
pub fn predict_insert_pane(
    layout: &PaneLayout,
    source: &str,
    anchor: &str,
) -> Option<PredictedLayout> {
    if source == anchor || source.is_empty() || anchor.is_empty() {
        return None;
    }
    let tree = rebuild_tree(layout)?;
    if !tree.root.contains(anchor) || tree.root.contains(source) {
        return None;
    }
    let root = split_at(
        tree.root,
        anchor,
        SplitDirection::Right,
        source,
        valid_split_ratio(0.5),
    );
    let tree = LayoutTree {
        root,
        area: tree.area,
    };
    let panes = tree.pane_rects();
    let splits = tree.splits();
    Some(PredictedLayout {
        tree,
        panes,
        splits,
    })
}

/// Every layout the target tab passes through during the §4.2 orchestration,
/// so the client can tell an expected intermediate `layout.updated` from a
/// foreign change (design §7.3).
#[derive(Debug, Clone, PartialEq)]
pub struct RelocationSteps {
    /// After step 1 (`pane.move` to a new tab): the source is gone and its
    /// parent split collapsed.
    pub removed: PredictedLayout,
    /// After step 2 (`pane.move` back with `right`/`down`): the source is the
    /// target's second child. Equal to `final_layout` for right/down drops.
    pub inserted: PredictedLayout,
    /// After step 3 (`pane.swap`, left/up only): the source is the first
    /// child. This is what [`predict_relocation`] returns.
    pub final_layout: PredictedLayout,
}

/// [`predict_relocation`] plus the two intermediate layouts of the
/// orchestration. Same preconditions and `None` cases.
pub fn predict_relocation_steps(
    layout: &PaneLayout,
    source: &str,
    target: &str,
    edge: DropEdge,
    ratio: f32,
) -> Option<RelocationSteps> {
    if source == target {
        return None;
    }
    let tree = rebuild_tree(layout)?;
    if tree.pane_count() < 2 || !tree.root.contains(source) || !tree.root.contains(target) {
        return None;
    }
    let area = tree.area;
    let predicted = |root: LayoutNode| {
        let tree = LayoutTree { root, area };
        let panes = tree.pane_rects();
        let splits = tree.splits();
        PredictedLayout {
            tree,
            panes,
            splits,
        }
    };
    let request_ratio = edge.request_ratio(ratio);
    let removed_root = remove_pane(tree.root, source)?;
    let inserted_root = split_at(
        removed_root.clone(),
        target,
        edge.split_direction(),
        source,
        request_ratio,
    );
    let mut final_root = inserted_root.clone();
    if edge.moved_pane_is_first() {
        swap_pane_ids(&mut final_root, source, target);
    }
    Some(RelocationSteps {
        removed: predicted(removed_root),
        inserted: predicted(inserted_root),
        final_layout: predicted(final_root),
    })
}

/// Predict a centre drop: only the two panes' rects trade places (Herdr's
/// `pane.swap` keeps the split shape and ratios). Returns the full pane list
/// in the layout's order, or `None` when either pane is missing or `a == b`.
pub fn predict_swap(layout: &PaneLayout, a: &str, b: &str) -> Option<Vec<PredictedPane>> {
    if a == b {
        return None;
    }
    let rect_a = layout.panes.iter().find(|pane| pane.pane_id == a)?.rect;
    let rect_b = layout.panes.iter().find(|pane| pane.pane_id == b)?.rect;
    Some(
        layout
            .panes
            .iter()
            .map(|pane| PredictedPane {
                pane_id: pane.pane_id.clone(),
                rect: if pane.pane_id == a {
                    rect_b
                } else if pane.pane_id == b {
                    rect_a
                } else {
                    pane.rect
                },
            })
            .collect(),
    )
}

/// Five-zone hit test. The inner 44% × 44% is [`DropZone::Center`]; outside
/// it the zone is the nearest edge measured as a fraction of that axis, so
/// the boundaries are the rect's diagonals. Ties resolve left, right, up,
/// down. Points outside the rect (or a degenerate rect) yield `None`.
pub fn drop_zone(rect: ZoneRect, x: f32, y: f32) -> Option<DropZone> {
    if !(rect.width > 0.0 && rect.height > 0.0) {
        return None;
    }
    let right = rect.x + rect.width;
    let bottom = rect.y + rect.height;
    if !(x >= rect.x && x <= right && y >= rect.y && y <= bottom) {
        return None;
    }
    let fx = (x - rect.x) / rect.width;
    let fy = (y - rect.y) / rect.height;
    let half = DROP_ZONE_CENTER_FRACTION / 2.0;
    if (fx - 0.5).abs() <= half && (fy - 0.5).abs() <= half {
        return Some(DropZone::Center);
    }
    let candidates = [
        (fx, DropZone::Left),
        (1.0 - fx, DropZone::Right),
        (fy, DropZone::Up),
        (1.0 - fy, DropZone::Down),
    ];
    let mut best = candidates[0];
    for candidate in &candidates[1..] {
        if candidate.0 < best.0 {
            best = *candidate;
        }
    }
    Some(best.1)
}

/// Structural fingerprint of a tab layout for transaction invalidation
/// (design §7.3).
///
/// Hashes, in order: `tab_id`, `zoomed`, `focused_pane_id`, every pane's id
/// and `focused` flag in layout order, and every split's path and direction
/// in layout order. Split ratios and rects are deliberately excluded: a
/// divider drag or window resize keeps the plan valid (the caller re-derives
/// geometry from the authoritative layout), while a pane appearing,
/// disappearing, moving to another slot (swap), or a split changing shape
/// changes the fingerprint.
pub fn layout_fingerprint(layout: &PaneLayout) -> u64 {
    let mut hasher = Fnv1a::new();
    hasher.write_str(&layout.tab_id);
    hasher.write_u8(u8::from(layout.zoomed));
    hasher.write_str(&layout.focused_pane_id);
    hasher.write_usize(layout.panes.len());
    for pane in &layout.panes {
        hasher.write_str(&pane.pane_id);
        hasher.write_u8(u8::from(pane.focused));
    }
    hasher.write_usize(layout.splits.len());
    for split in &layout.splits {
        match split.path() {
            Some(path) => {
                hasher.write_usize(path.len());
                for step in path {
                    hasher.write_u8(u8::from(step));
                }
            }
            None => {
                // Unparsable id: hash the raw id so it still participates.
                hasher.write_u8(0xff);
                hasher.write_str(&split.id);
            }
        }
        hasher.write_u8(match split.direction {
            SplitDirection::Right => 1,
            SplitDirection::Down => 2,
        });
    }
    hasher.finish()
}

/// 64-bit FNV-1a. Deterministic across processes and Rust versions, unlike
/// `DefaultHasher`, so fingerprints can be logged and compared meaningfully.
struct Fnv1a(u64);

impl Fnv1a {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write_bytes(&[value]);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_bytes(&(value as u64).to_le_bytes());
    }

    /// Length-prefixed so `("ab","c")` and `("a","bc")` differ.
    fn write_str(&mut self, value: &str) {
        self.write_usize(value.len());
        self.write_bytes(value.as_bytes());
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
#[path = "relocation_tests.rs"]
mod tests;
