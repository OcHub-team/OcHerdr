use super::*;

pub(crate) struct PendingReorder {
    pub(crate) _request: Task<()>,
    /// Release-time projection kept on screen until Herdr publishes the
    /// authoritative order. Tabs and workspaces share this settle.
    pub(crate) display: Option<PendingListReorder>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingListReorder {
    pub(crate) list: ReorderList,
    pub(crate) order: Vec<String>,
    pub(crate) source_index: usize,
    pub(crate) hover: ReorderHover,
    /// Window-local origin of the drag ghost at mouse-up. The real row starts
    /// here and settles into the projected empty slot.
    pub(crate) released_origin: (f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReorderDrag {
    pub(crate) list: ReorderList,
    pub(crate) source_index: usize,
    /// Ids in list order at press. Membership or order change cancels.
    pub(crate) order: Vec<String>,
    /// The prior gap lets each new declarative animation start where the last
    /// one ended instead of replaying from the authoritative layout.
    pub(crate) previous_hover: ReorderHover,
    pub(crate) hover: ReorderHover,
    pub(crate) origin: (f32, f32),
    pub(crate) pointer: (f32, f32),
    /// Where inside the source row the pointer grabbed it. Measured at press,
    /// so the drag cannot exist before the row has been laid out.
    pub(crate) grab_offset: (f32, f32),
    /// Slot rect at press. Ghost size stays on this even if the live canvas
    /// span is rewritten by the squeeze animation.
    pub(crate) source_rect: (f32, f32, f32, f32),
    /// A workspace row currently under a dragged tab. Keeping this semantic
    /// target on the drag avoids deriving a cross-workspace drop from the tab
    /// strip's horizontal reorder coordinate.
    pub(crate) workspace_drop: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReorderList {
    Workspaces,
    Tabs { workspace_id: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabTransferStep {
    pub(crate) pane_id: String,
    pub(crate) anchor_pane_id: String,
    pub(crate) split: SplitDirection,
    pub(crate) ratio: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabTransferPlan {
    pub(crate) first_pane_id: String,
    pub(crate) steps: Vec<TabTransferStep>,
}

/// Turn a split tree into the `pane.move` sequence needed to recreate it in a
/// fresh tab. Herdr always inserts the moved pane as the second child, so each
/// split is created before either child subtree is expanded.
pub(crate) fn tab_transfer_plan(root: &LayoutNode) -> TabTransferPlan {
    fn first_pane(node: &LayoutNode) -> &str {
        match node {
            LayoutNode::Pane(id) => id,
            LayoutNode::Split { first, .. } => first_pane(first),
        }
    }

    fn append(node: &LayoutNode, steps: &mut Vec<TabTransferStep>) {
        let LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } = node
        else {
            return;
        };
        steps.push(TabTransferStep {
            pane_id: first_pane(second).to_owned(),
            anchor_pane_id: first_pane(first).to_owned(),
            split: *direction,
            ratio: *ratio,
        });
        append(first, steps);
        append(second, steps);
    }

    let first_pane_id = first_pane(root).to_owned();
    let mut steps = Vec::new();
    append(root, &mut steps);
    TabTransferPlan {
        first_pane_id,
        steps,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabTransferPhase {
    Moving,
    Failed,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingTabTransfer {
    pub(crate) operation_id: u64,
    pub(crate) source_tab_id: String,
    pub(crate) target_workspace_id: String,
    pub(crate) tab_label: String,
    pub(crate) focused_pane_id: String,
    pub(crate) plan: TabTransferPlan,
    pub(crate) target_tab_id: Option<String>,
    /// Herdr can re-alias a pane when it crosses a workspace boundary.
    pub(crate) pane_aliases: HashMap<String, String>,
    /// Stable runtime identity used to recover an applied move whose socket
    /// response was lost before OcHerdr learned the new public pane id.
    pub(crate) pane_terminal_ids: HashMap<String, String>,
    pub(crate) next_step: usize,
    pub(crate) request_in_flight: bool,
    pub(crate) phase: TabTransferPhase,
}

#[cfg(test)]
mod tab_transfer_tests {
    use super::*;

    fn pane(id: &str) -> Box<LayoutNode> {
        Box::new(LayoutNode::Pane(id.to_owned()))
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

    #[test]
    fn one_pane_tab_needs_only_the_initial_cross_workspace_move() {
        let plan = tab_transfer_plan(&LayoutNode::Pane("only".into()));
        assert_eq!(plan.first_pane_id, "only");
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn preorder_moves_rebuild_an_arbitrary_binary_layout_exactly() {
        let original = *split(
            SplitDirection::Right,
            0.62,
            split(
                SplitDirection::Down,
                0.35,
                pane("a"),
                split(SplitDirection::Right, 0.4, pane("b"), pane("c")),
            ),
            split(SplitDirection::Down, 0.7, pane("d"), pane("e")),
        );
        let plan = tab_transfer_plan(&original);
        assert_eq!(plan.first_pane_id, "a");
        assert_eq!(plan.steps.len(), 4);

        let mut rebuilt = LayoutNode::Pane(plan.first_pane_id);
        for step in plan.steps {
            rebuilt = ocherdr_core::split_at(
                rebuilt,
                &step.anchor_pane_id,
                step.split,
                &step.pane_id,
                step.ratio,
            );
        }
        assert_eq!(rebuilt, original);
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ReorderMetrics {
    pub(crate) workspaces: Vec<ReorderSpan>,
    pub(crate) tabs: Vec<ReorderSpan>,
}

#[derive(Clone, Debug)]
pub(crate) struct ReorderSpan {
    pub(crate) id: String,
    pub(crate) rect: (f32, f32, f32, f32),
}

pub(crate) fn reorder_past_slop(drag: &ReorderDrag) -> bool {
    (drag.pointer.0 - drag.origin.0).abs() > REORDER_SLOP_PX
        || (drag.pointer.1 - drag.origin.1).abs() > REORDER_SLOP_PX
}

pub(crate) fn reorder_display_positions(
    order: &[String],
    source_index: usize,
    hover: ReorderHover,
) -> Vec<usize> {
    let mut positions = (0..order.len()).collect::<Vec<_>>();
    let Some(insert_index) = reorder_insert_index(order.len(), source_index, hover) else {
        return positions;
    };
    let destination = if insert_index > source_index {
        insert_index - 1
    } else {
        insert_index
    };
    positions[source_index] = destination;
    if destination < source_index {
        for position in &mut positions[destination..source_index] {
            *position += 1;
        }
    } else {
        for position in &mut positions[source_index + 1..=destination] {
            *position -= 1;
        }
    }
    positions
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReorderAxis {
    Horizontal,
    Vertical,
}

pub(crate) fn reorder_axis(list: &ReorderList) -> ReorderAxis {
    match list {
        ReorderList::Workspaces => ReorderAxis::Vertical,
        ReorderList::Tabs { .. } => ReorderAxis::Horizontal,
    }
}

/// Pixel shift along the list axis for each original index. `spans` are
/// `(origin, extent)` in original order. Horizontal tabs and vertical
/// workspaces pass the same numbers through this function.
pub(crate) fn reorder_display_shifts(
    spans: &[(f32, f32)],
    positions: &[usize],
    gap: f32,
) -> Vec<f32> {
    let mut originals_by_position = vec![0; positions.len()];
    for (original, position) in positions.iter().copied().enumerate() {
        originals_by_position[position] = original;
    }
    let mut target = spans[0].0;
    let mut shifts = vec![0.; positions.len()];
    for original in originals_by_position {
        shifts[original] = target - spans[original].0;
        target += spans[original].1 + gap;
    }
    shifts
}

pub(crate) fn reorder_axis_offset(shift: f32, axis: ReorderAxis) -> (f32, f32) {
    match axis {
        ReorderAxis::Horizontal => (shift, 0.),
        ReorderAxis::Vertical => (0., shift),
    }
}

pub(crate) fn reorder_list_bounds(rects: &[(f32, f32, f32, f32)]) -> (f32, f32, f32, f32) {
    let mut min_x = rects[0].0;
    let mut min_y = rects[0].1;
    let mut max_x = rects[0].0 + rects[0].2;
    let mut max_y = rects[0].1 + rects[0].3;
    for rect in &rects[1..] {
        min_x = min_x.min(rect.0);
        min_y = min_y.min(rect.1);
        max_x = max_x.max(rect.0 + rect.2);
        max_y = max_y.max(rect.1 + rect.3);
    }
    (min_x, min_y, max_x - min_x, max_y - min_y)
}

/// Ghost origin so the dragged row stays on its list axis.
///
/// Tabs lock `top` to the strip and clamp `left`; workspaces lock `left` to
/// the sidebar and clamp `top`. A tab with a semantic workspace target uses
/// its free pointer origin in `reorder_ghost`, outside this same-list helper.
pub(crate) fn reorder_ghost_origin(
    pointer: (f32, f32),
    grab_offset: (f32, f32),
    list: (f32, f32, f32, f32),
    ghost_size: (f32, f32),
    axis: ReorderAxis,
) -> (f32, f32) {
    let free = (pointer.0 - grab_offset.0, pointer.1 - grab_offset.1);
    match axis {
        ReorderAxis::Horizontal => {
            let max_x = (list.0 + list.2 - ghost_size.0).max(list.0);
            (free.0.clamp(list.0, max_x), list.1)
        }
        ReorderAxis::Vertical => {
            let max_y = (list.1 + list.3 - ghost_size.1).max(list.1);
            (list.0, free.1.clamp(list.1, max_y))
        }
    }
}

pub(crate) struct ReorderSlotOffsets {
    pub(crate) previous: Vec<(f32, f32)>,
    pub(crate) current: Vec<(f32, f32)>,
}

pub(crate) fn reorder_slot_offsets(
    source_index: usize,
    motion: ReorderMotion,
    positions: &[usize],
    previous_positions: &[usize],
    rects: &[(f32, f32, f32, f32)],
    gap: f32,
    axis: ReorderAxis,
) -> ReorderSlotOffsets {
    let along = |rect: (f32, f32, f32, f32)| match axis {
        ReorderAxis::Horizontal => (rect.0, rect.2),
        ReorderAxis::Vertical => (rect.1, rect.3),
    };
    let spans = rects.iter().copied().map(along).collect::<Vec<_>>();
    let to_offsets = |positions: &[usize]| {
        reorder_display_shifts(&spans, positions, gap)
            .into_iter()
            .map(|shift| reorder_axis_offset(shift, axis))
            .collect::<Vec<_>>()
    };
    let mut previous = to_offsets(previous_positions);
    if let ReorderMotion::Settling { released_origin } = motion {
        previous[source_index] = (
            released_origin.0 - rects[source_index].0,
            released_origin.1 - rects[source_index].1,
        );
    }
    ReorderSlotOffsets {
        previous,
        current: to_offsets(positions),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ReorderMotion {
    Dragging,
    Settling { released_origin: (f32, f32) },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReorderProjection {
    pub(crate) source_id: String,
    pub(crate) source_index: usize,
    pub(crate) positions: Vec<usize>,
    pub(crate) previous_positions: Vec<usize>,
    pub(crate) motion: ReorderMotion,
}

/// Derive display positions without mutating the authoritative snapshot. The
/// same mapping drives both pointer-time squeezing and the request-time settle
/// for tabs and workspaces. A changed authoritative order always wins over a
/// prediction based on stale input, including an order published by another
/// client.
pub(crate) fn reorder_projection(
    list: &ReorderList,
    authoritative_order: &[String],
    drag: Option<&ReorderDrag>,
    pending: Option<&PendingListReorder>,
) -> Option<ReorderProjection> {
    let dragging = drag.and_then(|drag| {
        if drag.list != *list || !reorder_past_slop(drag) || drag.workspace_drop.is_some() {
            return None;
        }
        Some((
            drag.order.as_slice(),
            drag.source_index,
            drag.previous_hover,
            drag.hover,
            ReorderMotion::Dragging,
        ))
    });
    let pending = pending.and_then(|pending| {
        (pending.list == *list).then_some((
            pending.order.as_slice(),
            pending.source_index,
            pending.hover,
            pending.hover,
            ReorderMotion::Settling {
                released_origin: pending.released_origin,
            },
        ))
    });
    let (order, source_index, previous_hover, hover, motion) = dragging.or(pending)?;
    if order != authoritative_order {
        return None;
    }
    let source_id = order.get(source_index)?.clone();
    Some(ReorderProjection {
        source_id,
        source_index,
        positions: reorder_display_positions(order, source_index, hover),
        previous_positions: reorder_display_positions(order, source_index, previous_hover),
        motion,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SplitDrag {
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) path: Vec<bool>,
    /// Topology at press. Ratio-derived geometry is omitted so a nested
    /// ancestor `layout.updated` does not void the gesture.
    pub(crate) layout: SplitLayoutFingerprint,
    pub(crate) direction: SplitDirection,
    pub(crate) rect: LayoutRect,
    pub(crate) grab_offset: f32,
    pub(crate) preview_ratio: f32,
    pub(crate) start_ratio: f32,
}

/// A divider drag that has been released: the ratios sent to Herdr, kept
/// as the squeeze preview until the last `layout.updated` of the batch
/// lands, so the intermediate layouts (dragged split moved, nested ones
/// not yet retuned) never flash on screen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingSplitCommit {
    pub(crate) tab_id: String,
    /// Topology at release; any other change voids the preview.
    pub(crate) layout: SplitLayoutFingerprint,
    /// Dragged split first, then the retuned descendants (request order).
    pub(crate) ratios: Vec<(Vec<bool>, f32)>,
    /// Distinguishes a late response from a replaced commit.
    pub(crate) serial: u64,
    /// Requests still without a response.
    pub(crate) outstanding: usize,
    /// Split ratios of the tab's layout as last seen, and how many ratio
    /// changes landed since release: Herdr emits one `layout.updated` per
    /// request, so once every request is answered and as many changes have
    /// landed the batch is over even if Herdr kept other ratios.
    pub(crate) last_ratios: Vec<f32>,
    pub(crate) layouts_seen: usize,
}

impl PendingSplitCommit {
    /// Whether every ratio of the batch is what the authoritative layout
    /// shows (within the f32 → JSON → f32 round trip).
    pub(crate) fn landed(&self, layout: &PaneLayout) -> bool {
        self.ratios.iter().all(|(path, ratio)| {
            layout.splits.iter().any(|split| {
                split.path().as_deref() == Some(path) && (split.ratio - ratio).abs() < 1e-4
            })
        })
    }

    /// Count a layout whose ratios differ from the last one seen. Returns
    /// whether the batch is over: every ratio landed, or every request is
    /// answered and one layout per request has come in.
    pub(crate) fn observe(&mut self, layout: &PaneLayout) -> bool {
        let ratios = split_ratios_of(layout);
        if ratios != self.last_ratios {
            self.last_ratios = ratios;
            self.layouts_seen += 1;
        }
        self.landed(layout) || (self.outstanding == 0 && self.layouts_seen >= self.ratios.len())
    }
}

/// Split ratios in layout order, the part of a layout `layout_fingerprint`
/// leaves out.
pub(crate) fn split_ratios_of(layout: &PaneLayout) -> Vec<f32> {
    layout.splits.iter().map(|split| split.ratio).collect()
}

/// Split tree shape and which pane sits at each preorder leaf.
/// Paths and directions only: Herdr recomputes split/pane rects from ratios,
/// so including those would cancel a nested drag when an ancestor ratio changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SplitLayoutFingerprint {
    pub(crate) zoomed: bool,
    pub(crate) splits: Vec<(Vec<bool>, SplitDirection)>,
    pub(crate) panes: Vec<String>,
}
