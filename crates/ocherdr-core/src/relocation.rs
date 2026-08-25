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
mod tests {
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
        let directions: Vec<SplitDirection> =
            predicted.splits.iter().map(|s| s.direction).collect();
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
}
