use super::*;

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // PaneDrag carries hover + tab-bar drop state.
pub(crate) enum SurfaceDrag {
    Idle,
    Text { pane_id: String, captured: bool },
    Split(SplitDrag),
    Reorder(ReorderDrag),
    Pane(PaneDrag),
}

/// A pane grabbed by its title-bar handle (design §5).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneDrag {
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
    /// `layout_fingerprint` of the tab at press. Any structural change to the
    /// tab while dragging cancels the gesture.
    pub(crate) fingerprint: u64,
    pub(crate) origin: (f32, f32),
    pub(crate) pointer: (f32, f32),
    /// Where inside the source pane rect the pointer grabbed it.
    pub(crate) grab_offset: (f32, f32),
    /// Source pane rect in window coordinates at press.
    pub(crate) source_rect: (f32, f32, f32, f32),
    pub(crate) hover: Option<PaneDropHover>,
    /// Cell of the layout palette currently under the pointer. Template
    /// cells take precedence over the pane-local five-zone target.
    pub(crate) template_hover: Option<PaneTemplateHover>,
    /// Tab-bar drop published by the painted `+` / trailing strip (or, in
    /// phase 2, an existing tab pill). Tab targets outrank pane-local and
    /// template hits and are never reverse-engineered from the terminal.
    pub(crate) tab_target: Option<PaneTabDropTarget>,
    /// Local-only layout shown while hovering a droppable zone. Herdr is not
    /// contacted until release. `intent == None` is the animated return to
    /// the authoritative layout after leaving a valid zone.
    pub(crate) layout_preview: Option<PaneDragLayoutPreview>,
    /// Whether the four edge zones accept drops: the `pane-edge-relocation`
    /// flag and the connection's `pane.move` capability, read at press.
    pub(crate) edge_drops: bool,
    /// The layout palette needs `pane.move`, independently of the optional
    /// four-edge drop setting.
    pub(crate) layout_templates: bool,
    /// Tab-bar drops need `pane.move` and a non-zoomed tab; a single-pane
    /// tab may still use them.
    pub(crate) tab_bar_drops: bool,
    pub(crate) pressed_at: Instant,
}

/// Semantic tab-bar drop published by a painted element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneTabDropTarget {
    NewTab,
    Existing {
        tab_id: String,
        target_pane_id: String,
    },
}

pub(crate) type PaneFractions = (f32, f32, f32, f32);

/// Stable identity of the draft layout under the pointer. Keeping this
/// separate from the moving render geometry prevents feedback loops where a
/// squeezed pane moves its own drop zone out from under the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneDragIntent {
    Pane {
        target_pane_id: String,
        zone: DropZone,
    },
    Template(PaneTemplatePlacement),
    Tab(PaneTabDropTarget),
}

/// Transition between two local draft layouts during a pane drag.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneDragLayoutPreview {
    pub(crate) intent: Option<PaneDragIntent>,
    pub(crate) from: Vec<(String, PaneFractions)>,
    pub(crate) to: Vec<(String, PaneFractions)>,
    pub(crate) started: Instant,
}

impl PaneDragLayoutPreview {
    pub(crate) fn display_fractions(
        &self,
        pane_id: &str,
        now: Instant,
        reduce_motion: bool,
    ) -> Option<PaneFractions> {
        let from = self
            .from
            .iter()
            .find(|(id, _)| id == pane_id)
            .map(|(_, rect)| *rect)?;
        let to = self
            .to
            .iter()
            .find(|(id, _)| id == pane_id)
            .map(|(_, rect)| *rect)?;
        let progress = ochub_ui::anim::linear_progress(
            self.started,
            PANE_DRAG_LAYOUT_ANIMATION,
            now,
            reduce_motion,
        );
        Some(lerp_rect(
            from,
            to,
            ochub_ui::anim::ease_out_quint(progress),
        ))
    }

    pub(crate) fn is_animating(&self, now: Instant, reduce_motion: bool) -> bool {
        !reduce_motion && now.saturating_duration_since(self.started) < PANE_DRAG_LAYOUT_ANIMATION
    }

    pub(crate) fn target_fractions(&self, pane_id: &str) -> Option<PaneFractions> {
        self.to
            .iter()
            .find(|(id, _)| id == pane_id)
            .map(|(_, rect)| *rect)
    }
}

/// Keyboard equivalent of the pane drag (design §11). Entered with the
/// prefix key `m`; arrows choose a neighbouring pane, Tab cycles the zone,
/// Enter commits through the same plan machinery, Esc cancels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KeyboardPaneMove {
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
    pub(crate) fingerprint: u64,
    /// Chosen target pane and zone; `None` until the first arrow.
    pub(crate) target: Option<PaneDropHover>,
    pub(crate) edge_drops: bool,
}

impl KeyboardPaneMove {
    pub(crate) fn droppable(&self) -> bool {
        self.target
            .as_ref()
            .is_some_and(|hover| hover.droppable(self.edge_drops))
    }
}

/// What survives a disconnect of a relocation that had already parked its
/// pane in a temporary tab.
#[derive(Clone)]
pub(crate) struct ParkedRecovery {
    pub(crate) plan: RelocationPlan,
    pub(crate) temp_tab_id: String,
    pub(crate) moved_pane_id: String,
}

/// The pane and zone under the pointer during a pane drag.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneDropHover {
    pub(crate) target_pane_id: String,
    pub(crate) zone: DropZone,
    /// Window rect of the target pane, for the highlight.
    pub(crate) target_rect: (f32, f32, f32, f32),
}

impl PaneDropHover {
    pub(crate) fn droppable(&self, edge_drops: bool) -> bool {
        match self.zone {
            DropZone::Center => true,
            DropZone::Left | DropZone::Right | DropZone::Up | DropZone::Down => edge_drops,
        }
    }
}

/// A drop to commit, from the mouse gesture or the keyboard mode.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneDropRequest {
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_id: String,
    /// `layout_fingerprint` of the tab when the gesture started.
    pub(crate) fingerprint: u64,
    pub(crate) hover: PaneDropHover,
    pub(crate) edge_drops: bool,
}

/// Neighbour of `source` in `direction` for the keyboard move mode: the
/// pane sharing the longest edge on that side, else the nearest pane whose
/// centre lies on that side. `None` when nothing is there.
pub(crate) fn keyboard_neighbour(
    layout: &PaneLayout,
    source: &str,
    direction: DropEdge,
) -> Option<String> {
    let source_rect = layout
        .panes
        .iter()
        .find(|pane| pane.pane_id == source)?
        .rect;
    let centre = |rect: LayoutRect| {
        (
            f32::from(rect.x) + f32::from(rect.width) / 2.,
            f32::from(rect.y) + f32::from(rect.height) / 2.,
        )
    };
    let (sx, sy) = centre(source_rect);
    let overlap = |a0: u16, a1: u16, b0: u16, b1: u16| -> i32 {
        i32::from(a1.min(b1)) - i32::from(a0.max(b0))
    };
    let mut best: Option<(i32, f32, &str)> = None;
    for pane in layout.panes.iter().filter(|pane| pane.pane_id != source) {
        let r = pane.rect;
        let (cx, cy) = centre(r);
        let (on_side, shared) = match direction {
            DropEdge::Left => (
                cx < sx,
                overlap(
                    source_rect.y,
                    source_rect.y + source_rect.height,
                    r.y,
                    r.y + r.height,
                ),
            ),
            DropEdge::Right => (
                cx > sx,
                overlap(
                    source_rect.y,
                    source_rect.y + source_rect.height,
                    r.y,
                    r.y + r.height,
                ),
            ),
            DropEdge::Up => (
                cy < sy,
                overlap(
                    source_rect.x,
                    source_rect.x + source_rect.width,
                    r.x,
                    r.x + r.width,
                ),
            ),
            DropEdge::Down => (
                cy > sy,
                overlap(
                    source_rect.x,
                    source_rect.x + source_rect.width,
                    r.x,
                    r.x + r.width,
                ),
            ),
        };
        if !on_side {
            continue;
        }
        let distance = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
        let candidate = (shared.max(0), -distance, pane.pane_id.as_str());
        if best.is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1)) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, id)| id.to_owned())
}

/// Tab cycles the drop zone in the keyboard move mode.
pub(crate) fn next_keyboard_zone(zone: DropZone, edge_drops: bool) -> DropZone {
    if !edge_drops {
        return DropZone::Center;
    }
    match zone {
        DropZone::Center => DropZone::Left,
        DropZone::Left => DropZone::Right,
        DropZone::Right => DropZone::Up,
        DropZone::Up => DropZone::Down,
        DropZone::Down => DropZone::Center,
    }
}

/// Where a released-but-not-dropped preview flies back from (design §10:
/// "invalid drop / cancel → preview returns to the source rect, 120 ms").
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneDragReturn {
    pub(crate) pane_id: String,
    pub(crate) tab_id: String,
    pub(crate) from: (f32, f32, f32, f32),
    pub(crate) to: (f32, f32, f32, f32),
    /// Shell positions at cancellation. They ease back together with the
    /// floating pane instead of snapping to the authoritative layout.
    pub(crate) layout_from: Vec<(String, PaneFractions)>,
    pub(crate) started: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RelocationIntent {
    Swap,
    /// Design §4.2: park in a new tab, move back beside the target, and for
    /// left/up swap the two leaves. `ratio` is the target's share.
    Insert {
        edge: DropEdge,
        ratio: f32,
    },
}

impl RelocationIntent {
    /// Whether the orchestration needs the third `pane.swap` request.
    pub(crate) fn corrects_order(self) -> bool {
        matches!(self, Self::Insert { edge, .. } if edge.moved_pane_is_first())
    }
}

/// The shapes the target tab passes through during an insert (design
/// §7.3): each authoritative `layout.updated` is classified against these.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InsertShapes {
    /// After step 1: the source removed, its parent split collapsed.
    pub(crate) removed: SplitLayoutFingerprint,
    /// After step 2: the source as the target's second child.
    pub(crate) inserted: SplitLayoutFingerprint,
    /// After step 3 (or step 2 for right/down): the prediction.
    pub(crate) final_shape: SplitLayoutFingerprint,
}

impl InsertShapes {
    pub(crate) fn from_steps(steps: &ocherdr_core::RelocationSteps) -> Self {
        Self {
            removed: predicted_shape(&steps.removed),
            inserted: predicted_shape(&steps.inserted),
            final_shape: predicted_shape(&steps.final_layout),
        }
    }
}

/// Where an observed layout of the target tab sits in the transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutShape {
    /// Still the release-time layout (the event has not arrived).
    Release,
    Removed,
    Inserted,
    Final,
    /// None of the expected shapes: someone else changed the tab.
    Foreign,
}

/// Immutable plan built at release (design §7.1). `predicted_rects` only
/// drives rendering and motion; Herdr's layout stays authoritative.
#[derive(Clone)]
pub(crate) struct RelocationPlan {
    pub(crate) operation_id: u64,
    pub(crate) source_pane_id: String,
    pub(crate) source_tab_id: String,
    pub(crate) target_pane_id: String,
    pub(crate) target_tab_id: String,
    pub(crate) intent: RelocationIntent,
    /// `layout_fingerprint` of the target tab at release.
    pub(crate) fingerprint: u64,
    /// Split topology at release. The authoritative `layout.updated` must
    /// keep the same shape and pane set (only leaves swap) to settle.
    pub(crate) topology: SplitLayoutFingerprint,
    /// Area the predicted rects are expressed in.
    pub(crate) area: LayoutRect,
    pub(crate) predicted_rects: Vec<PredictedPane>,
    pub(crate) visual_snapshot: Option<RenderedFrame>,
    /// Workspace of the source tab: `pane.move`'s `new_tab` destination.
    pub(crate) workspace_id: String,
    /// Tabs of that workspace at release. Step 1 creates one more; events
    /// travel on their own socket, so `tab.created` can land before the
    /// step-1 response names it and `tab.closed` after the step-2 response.
    /// A tab outside this set that holds nothing but the source pane is the
    /// temporary tab whatever the phase knows (see `unlisted_temp_tabs`).
    pub(crate) known_tab_ids: HashSet<String>,
    /// Intermediate shapes of an insert; `None` for a swap.
    pub(crate) insert_shapes: Option<InsertShapes>,
}

/// Design §7.2. Phases before `Settling` keep the tab locked and render the
/// plan's predicted rects; `Parked` shows the authoritative snapshot plus the
/// recovery notice.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RelocationPhase {
    /// `pane.swap` sent. Needs both the response and a matching
    /// `layout.updated` before the correction runs.
    Swapping { responded: bool, layout_seen: bool },
    /// Step 1 (`pane.move` to a new tab) in flight.
    Parking,
    /// Step 2 (`pane.move` back beside the target) in flight or answered.
    /// `temp_tab_id` is hidden from the tab strip while this phase lasts.
    Inserting {
        temp_tab_id: String,
        moved_pane_id: String,
        responded: bool,
        layout_seen: bool,
    },
    /// Step 3 (`pane.swap`, left/up) in flight or answered.
    CorrectingOrder { responded: bool, layout_seen: bool },
    /// Step 2 failed: the pane sits in `temp_tab_id`. No prediction, no
    /// lock; the inline notice offers retry / go to tab.
    Parked {
        temp_tab_id: String,
        moved_pane_id: String,
    },
    /// Shells and borders move from the predicted rects to the authoritative
    /// ones (design §10: 120–180 ms, `ease_out_quint`).
    Settling {
        started: Instant,
        from: Vec<(String, (f32, f32, f32, f32))>,
    },
}

impl RelocationPhase {
    /// Predicted rects are on screen and the tab is locked.
    pub(crate) fn locks_tab(&self) -> bool {
        !matches!(self, Self::Parked { .. })
    }

    /// The temporary tab this phase keeps out of the tab strip.
    pub(crate) fn hidden_tab_id(&self) -> Option<&str> {
        match self {
            Self::Inserting { temp_tab_id, .. } => Some(temp_tab_id),
            _ => None,
        }
    }

    pub(crate) fn parked_tab_id(&self) -> Option<&str> {
        match self {
            Self::Parked { temp_tab_id, .. } => Some(temp_tab_id),
            _ => None,
        }
    }
}

/// Pane the first `pane.move` response reports, read back for step 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParkedPane {
    pub(crate) temp_tab_id: String,
    pub(crate) pane_id: String,
}

/// What reaches the insert state machine (design §7.2).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RelocationSignal {
    /// Step 1 answered.
    Parked(Option<ParkedPane>),
    /// Step 2 answered (`true` = accepted and changed).
    Inserted(bool),
    /// Step 3 answered.
    Reordered(bool),
    /// The target tab's authoritative layout changed.
    Layout(LayoutShape),
    /// User pressed "Retry" on the parked notice.
    Retry,
}

/// Side effect the controller performs after a transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RelocationAction {
    None,
    /// Issue step 2 with the parked pane's ids.
    SendInsert,
    /// Issue step 3.
    SendSwap,
    /// Response and matching layout both in: run the settle correction.
    Settle,
    /// Drop the plan; the authoritative snapshot is what is on screen.
    Revert,
    /// Step 2 failed: show the parked notice, unhide the temp tab.
    Park,
    /// Step 3 failed: the layout is legal but mirrored. Unlock, one notice.
    Misordered,
}

/// Pure transition of the insert phases. `corrects_order` is whether the
/// plan needs step 3 (left/up). Returns the next phase (`None` = plan
/// dropped) and the action to take.
pub(crate) fn advance_insert_phase(
    phase: RelocationPhase,
    signal: RelocationSignal,
    corrects_order: bool,
) -> (Option<RelocationPhase>, RelocationAction) {
    use RelocationAction as A;
    use RelocationPhase as P;
    use RelocationSignal as S;
    match (phase, signal) {
        (P::Parking, S::Parked(Some(parked))) => (
            Some(P::Inserting {
                temp_tab_id: parked.temp_tab_id,
                moved_pane_id: parked.pane_id,
                responded: false,
                layout_seen: false,
            }),
            A::SendInsert,
        ),
        (P::Parking, S::Parked(None)) => (None, A::Revert),
        // `Final` can equal the release shape (two-pane tab, left drop), so
        // it is benign here too: step 1 has not even answered yet.
        (
            P::Parking,
            S::Layout(LayoutShape::Release | LayoutShape::Removed | LayoutShape::Final),
        ) => (Some(P::Parking), A::None),
        (P::Parking, S::Layout(_)) => (None, A::Revert),
        (
            P::Inserting {
                temp_tab_id,
                moved_pane_id,
                layout_seen,
                ..
            },
            S::Inserted(true),
        ) => {
            if corrects_order {
                (
                    Some(P::CorrectingOrder {
                        responded: false,
                        layout_seen: false,
                    }),
                    A::SendSwap,
                )
            } else if layout_seen {
                (
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded: true,
                        layout_seen: true,
                    }),
                    A::Settle,
                )
            } else {
                (
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded: true,
                        layout_seen: false,
                    }),
                    A::None,
                )
            }
        }
        (
            P::Inserting {
                temp_tab_id,
                moved_pane_id,
                ..
            },
            S::Inserted(false),
        ) => (
            Some(P::Parked {
                temp_tab_id,
                moved_pane_id,
            }),
            A::Park,
        ),
        (
            P::Inserting {
                temp_tab_id,
                moved_pane_id,
                responded,
                layout_seen,
            },
            S::Layout(shape),
        ) => {
            let landed = match shape {
                LayoutShape::Inserted => true,
                LayoutShape::Final => !corrects_order,
                _ => false,
            };
            match shape {
                LayoutShape::Release | LayoutShape::Removed => (
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded,
                        layout_seen,
                    }),
                    A::None,
                ),
                LayoutShape::Inserted | LayoutShape::Final if landed && corrects_order => (
                    // Step 2 landed for a left/up plan: still waiting for the
                    // response before the swap goes out.
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded,
                        layout_seen,
                    }),
                    A::None,
                ),
                LayoutShape::Inserted | LayoutShape::Final if landed => (
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded,
                        layout_seen: true,
                    }),
                    if responded { A::Settle } else { A::None },
                ),
                _ => (None, A::Revert),
            }
        }
        (P::CorrectingOrder { layout_seen, .. }, S::Reordered(true)) => (
            Some(P::CorrectingOrder {
                responded: true,
                layout_seen,
            }),
            if layout_seen { A::Settle } else { A::None },
        ),
        (P::CorrectingOrder { .. }, S::Reordered(false)) => (None, A::Misordered),
        (P::CorrectingOrder { responded, .. }, S::Layout(LayoutShape::Final)) => (
            Some(P::CorrectingOrder {
                responded,
                layout_seen: true,
            }),
            if responded { A::Settle } else { A::None },
        ),
        // Events ride a different socket than responses, so an earlier
        // step's layout can land after a later step answered: every
        // expected intermediate shape is benign here.
        (
            P::CorrectingOrder {
                responded,
                layout_seen,
            },
            S::Layout(LayoutShape::Release | LayoutShape::Removed | LayoutShape::Inserted),
        ) => (
            Some(P::CorrectingOrder {
                responded,
                layout_seen,
            }),
            A::None,
        ),
        (P::CorrectingOrder { .. }, S::Layout(LayoutShape::Foreign)) => (None, A::Revert),
        (
            P::Parked {
                temp_tab_id,
                moved_pane_id,
            },
            S::Retry,
        ) => (
            Some(P::Inserting {
                temp_tab_id,
                moved_pane_id,
                responded: false,
                layout_seen: false,
            }),
            A::SendInsert,
        ),
        // Parked shows the authoritative snapshot: layout changes are fine.
        (phase @ P::Parked { .. }, S::Layout(_)) => (Some(phase), A::None),
        // Stale or out-of-order signals never move the machine.
        (phase, _) => (Some(phase), A::None),
    }
}

/// Pane order and split shape of a predicted layout, for exact comparison
/// with an authoritative `layout.updated`.
pub(crate) fn predicted_shape(layout: &PredictedLayout) -> SplitLayoutFingerprint {
    SplitLayoutFingerprint {
        zoomed: false,
        splits: layout
            .splits
            .iter()
            .map(|split| (split.path.clone(), split.direction))
            .collect(),
        panes: layout
            .panes
            .iter()
            .map(|pane| pane.pane_id.clone())
            .collect(),
    }
}

/// Classify the target tab's authoritative layout against an insert plan.
pub(crate) fn classify_insert_layout(layout: &PaneLayout, plan: &RelocationPlan) -> LayoutShape {
    let Some(shapes) = plan.insert_shapes.as_ref() else {
        return LayoutShape::Foreign;
    };
    // Shapes first: in a two-pane tab the final layout of a left drop has
    // the release-time shape, and a foreign change that happens to produce
    // the expected shape is harmless.
    let shape = controller::split_layout_fingerprint(layout);
    if shape == shapes.final_shape {
        LayoutShape::Final
    } else if shape == shapes.inserted {
        LayoutShape::Inserted
    } else if shape == shapes.removed {
        LayoutShape::Removed
    } else if layout_fingerprint(layout) == plan.fingerprint {
        LayoutShape::Release
    } else {
        LayoutShape::Foreign
    }
}

#[derive(Clone)]
pub(crate) struct PendingPaneRelocation {
    pub(crate) plan: RelocationPlan,
    pub(crate) phase: RelocationPhase,
}

pub(crate) fn pane_drag_past_slop(drag: &PaneDrag) -> bool {
    (drag.pointer.0 - drag.origin.0).abs() > PANE_DRAG_SLOP_PX
        || (drag.pointer.1 - drag.origin.1).abs() > PANE_DRAG_SLOP_PX
}

/// Pane rect in window coordinates from its layout fractions.
pub(crate) fn pane_window_rect(
    layout: &PaneLayout,
    pane_id: &str,
    surface: (f32, f32, f32, f32),
) -> Option<(f32, f32, f32, f32)> {
    let pane = layout.panes.iter().find(|pane| pane.pane_id == pane_id)?;
    let (fx, fy, fw, fh) = layout_rect_fractions(layout.area, pane.rect)?;
    Some(fractions_to_window(surface, (fx, fy, fw, fh)))
}

pub(crate) fn fractions_to_window(
    surface: (f32, f32, f32, f32),
    fractions: (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    (
        surface.0 + fractions.0 * surface.2,
        surface.1 + fractions.1 * surface.3,
        fractions.2 * surface.2,
        fractions.3 * surface.3,
    )
}

pub(crate) fn layout_rect_fractions(
    area: LayoutRect,
    rect: LayoutRect,
) -> Option<(f32, f32, f32, f32)> {
    let area_w = f32::from(area.width);
    let area_h = f32::from(area.height);
    if area_w == 0. || area_h == 0. {
        return None;
    }
    Some((
        (f32::from(rect.x) - f32::from(area.x)) / area_w,
        (f32::from(rect.y) - f32::from(area.y)) / area_h,
        f32::from(rect.width) / area_w,
        f32::from(rect.height) / area_h,
    ))
}

/// Five-zone hit test over every other pane of the tab (design §5.3).
/// The source pane itself is never a target.
pub(crate) fn pane_drop_hover(
    layout: &PaneLayout,
    source_pane_id: &str,
    surface: (f32, f32, f32, f32),
    pointer: (f32, f32),
) -> Option<PaneDropHover> {
    layout
        .panes
        .iter()
        .filter(|pane| pane.pane_id != source_pane_id)
        .find_map(|pane| {
            let rect = pane_window_rect(layout, &pane.pane_id, surface)?;
            let zone = drop_zone(
                ZoneRect {
                    x: rect.0,
                    y: rect.1,
                    width: rect.2,
                    height: rect.3,
                },
                pointer.0,
                pointer.1,
            )?;
            Some(PaneDropHover {
                target_pane_id: pane.pane_id.clone(),
                zone,
                target_rect: rect,
            })
        })
}

pub(crate) fn authoritative_pane_fractions(layout: &PaneLayout) -> Vec<(String, PaneFractions)> {
    layout
        .panes
        .iter()
        .filter_map(|pane| {
            Some((
                pane.pane_id.clone(),
                layout_rect_fractions(layout.area, pane.rect)?,
            ))
        })
        .collect()
}

pub(crate) fn predicted_pane_fractions(
    area: LayoutRect,
    panes: impl IntoIterator<Item = PredictedPane>,
) -> Vec<(String, PaneFractions)> {
    panes
        .into_iter()
        .filter_map(|pane| Some((pane.pane_id, layout_rect_fractions(area, pane.rect)?)))
        .collect()
}

/// Build the draft geometry for one stable hover intent. The source pane is
/// still part of this geometry so its shell can become the dashed drop slot;
/// the floating preview remains independent and follows the pointer.
pub(crate) fn pane_drag_target_fractions(
    layout: &PaneLayout,
    source_pane_id: &str,
    intent: &PaneDragIntent,
) -> Option<Vec<(String, PaneFractions)>> {
    let panes = match intent {
        PaneDragIntent::Pane {
            target_pane_id,
            zone,
        } => match zone.edge() {
            None => predict_swap(layout, source_pane_id, target_pane_id)?,
            Some(edge) => {
                predict_relocation_steps(
                    layout,
                    source_pane_id,
                    target_pane_id,
                    edge,
                    PANE_EDGE_DROP_RATIO,
                )?
                .final_layout
                .panes
            }
        },
        PaneDragIntent::Template(placement) => {
            pane_template_predicted_layout(layout, source_pane_id, *placement)?.panes
        }
        PaneDragIntent::Tab(_) => match predict_remove_pane(layout, source_pane_id) {
            Some(predicted) => predicted.panes,
            None if layout.panes.len() == 1 && layout.panes[0].pane_id == source_pane_id => {
                Vec::new()
            }
            None => return None,
        },
    };
    Some(predicted_pane_fractions(layout.area, panes))
}

/// Transition to a new local draft layout, or back to the authoritative one
/// when the pointer leaves a droppable zone. Repeated pointer moves inside
/// the same zone keep the original transition instead of restarting it.
pub(crate) fn update_pane_drag_layout_preview(
    layout: &PaneLayout,
    source_pane_id: &str,
    next_intent: Option<PaneDragIntent>,
    current: Option<&PaneDragLayoutPreview>,
    now: Instant,
    reduce_motion: bool,
) -> Option<PaneDragLayoutPreview> {
    if current.is_some_and(|preview| preview.intent == next_intent) {
        return current.cloned();
    }
    if current.is_none() && next_intent.is_none() {
        return None;
    }

    let authoritative = authoritative_pane_fractions(layout);
    let from = authoritative
        .iter()
        .map(|(pane_id, rect)| {
            let displayed = current
                .and_then(|preview| preview.display_fractions(pane_id, now, reduce_motion))
                .unwrap_or(*rect);
            (pane_id.clone(), displayed)
        })
        .collect();
    let to = match next_intent.as_ref() {
        Some(intent) => pane_drag_target_fractions(layout, source_pane_id, intent)?,
        None => authoritative,
    };
    Some(PaneDragLayoutPreview {
        intent: next_intent,
        from,
        to,
        started: now,
    })
}

pub(crate) fn pane_drag_preview_intent(
    hover: Option<&PaneDropHover>,
    template_hover: Option<&PaneTemplateHover>,
    tab_target: Option<&PaneTabDropTarget>,
    edge_drops: bool,
) -> Option<PaneDragIntent> {
    tab_target
        .cloned()
        .map(PaneDragIntent::Tab)
        .or_else(|| template_hover.map(|hover| PaneDragIntent::Template(hover.placement)))
        .or_else(|| {
            hover
                .filter(|hover| hover.droppable(edge_drops))
                .map(|hover| PaneDragIntent::Pane {
                    target_pane_id: hover.target_pane_id.clone(),
                    zone: hover.zone,
                })
        })
}

/// Top-left of the floating preview: the pointer keeps its grab offset, and
/// the 1.015 scale grows the card around its centre.
pub(crate) fn pane_drag_preview_rect(drag: &PaneDrag) -> (f32, f32, f32, f32) {
    let (w, h) = (drag.source_rect.2, drag.source_rect.3);
    let scaled_w = w * PANE_DRAG_PREVIEW_SCALE;
    let scaled_h = h * PANE_DRAG_PREVIEW_SCALE;
    (
        drag.pointer.0 - drag.grab_offset.0 - (scaled_w - w) / 2.,
        drag.pointer.1 - drag.grab_offset.1 - (scaled_h - h) / 2.,
        scaled_w,
        scaled_h,
    )
}

/// Pane and split geometry of a tab as surface fractions, with the split at
/// `path` drawn at `ratio` instead of its authoritative value (design §5.4).
/// Rects are laid out exactly as Herdr will lay them out for that ratio
/// (`split_rect`: whole cells, first child rounded), so the frame the
/// preview shows at release is the frame the authoritative `layout.updated`
/// brings back; a continuous preview sat up to half a cell away from it and
/// jumped on release.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SqueezedLayout {
    pub(crate) panes: Vec<(String, (f32, f32, f32, f32))>,
    pub(crate) splits: Vec<SqueezedSplit>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SqueezedSplit {
    pub(crate) path: Vec<bool>,
    pub(crate) rect: (f32, f32, f32, f32),
    /// Divider position along the split axis, as a surface fraction.
    pub(crate) line: f32,
}

impl SqueezedLayout {
    pub(crate) fn pane(&self, pane_id: &str) -> Option<(f32, f32, f32, f32)> {
        self.panes
            .iter()
            .find(|(id, _)| id == pane_id)
            .map(|(_, rect)| *rect)
    }

    pub(crate) fn split(&self, path: &[bool]) -> Option<((f32, f32, f32, f32), f32)> {
        self.splits
            .iter()
            .find(|split| split.path == path)
            .map(|split| (split.rect, split.line))
    }
}

/// The tab's geometry with the given split ratios applied (the dragged
/// split plus the descendants `pinned_ratios` retunes), in whole cells like
/// Herdr, as surface fractions. Ratios are clamped the way Herdr clamps them.
pub(crate) fn squeezed_layout(
    layout: &PaneLayout,
    ratios: &[(Vec<bool>, f32)],
) -> Option<SqueezedLayout> {
    let mut tree = rebuild_tree(layout)?;
    if layout.area.width == 0 || layout.area.height == 0 {
        return None;
    }
    let clamped: Vec<(Vec<bool>, f32)> = ratios
        .iter()
        .map(|(path, ratio)| (path.clone(), valid_split_ratio(*ratio)))
        .collect();
    ocherdr_core::apply_ratios(&mut tree.root, &clamped);
    let mut out = SqueezedLayout {
        panes: Vec::new(),
        splits: Vec::new(),
    };
    squeeze_node(
        &tree.root,
        layout.area,
        layout.area,
        &mut Vec::new(),
        &mut out,
    );
    Some(out)
}

/// The ratios a divider drag applies: the dragged split at `ratio` and every
/// same-direction descendant retuned so its divider stays on its cell.
pub(crate) fn split_drag_ratios(
    layout: &PaneLayout,
    path: &[bool],
    ratio: f32,
) -> Vec<(Vec<bool>, f32)> {
    rebuild_tree(layout)
        .map(|tree| ocherdr_core::pinned_ratios(&tree, path, ratio))
        .unwrap_or_default()
}

pub(crate) fn squeeze_node(
    node: &LayoutNode,
    rect: LayoutRect,
    area: LayoutRect,
    current: &mut Vec<bool>,
    out: &mut SqueezedLayout,
) {
    // `area` is non-empty (checked by the caller), so the fractions exist.
    let fractions = |rect| layout_rect_fractions(area, rect).unwrap_or((0., 0., 0., 0.));
    match node {
        LayoutNode::Pane(pane_id) => out.panes.push((pane_id.clone(), fractions(rect))),
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let (first_rect, second_rect) = split_rect(rect, *direction, *ratio);
            let (fx, fy, fw, fh) = fractions(first_rect);
            let line = match direction {
                SplitDirection::Right => fx + fw,
                SplitDirection::Down => fy + fh,
            };
            out.splits.push(SqueezedSplit {
                path: current.clone(),
                rect: fractions(rect),
                line,
            });
            current.push(false);
            squeeze_node(first, first_rect, area, current, out);
            current.pop();
            current.push(true);
            squeeze_node(second, second_rect, area, current, out);
            current.pop();
        }
    }
}

pub(crate) fn lerp_rect(
    from: (f32, f32, f32, f32),
    to: (f32, f32, f32, f32),
    t: f32,
) -> (f32, f32, f32, f32) {
    (
        from.0 + (to.0 - from.0) * t,
        from.1 + (to.1 - from.1) * t,
        from.2 + (to.2 - from.2) * t,
        from.3 + (to.3 - from.3) * t,
    )
}

/// Whether the authoritative layout is the one the plan predicted: same
/// split shape, same pane set, and the fingerprint has moved on from the
/// release-time value (so a ratio-only `layout.updated` is not mistaken for
/// the swap landing).
pub(crate) fn layout_settles_plan(layout: &PaneLayout, plan: &RelocationPlan) -> bool {
    if layout_fingerprint(layout) == plan.fingerprint {
        return false;
    }
    if layout.zoomed != plan.topology.zoomed {
        return false;
    }
    let splits: Vec<(Vec<bool>, SplitDirection)> = layout
        .splits
        .iter()
        .filter_map(|split| Some((split.path()?, split.direction)))
        .collect();
    if splits != plan.topology.splits {
        return false;
    }
    let mut live: Vec<&str> = layout
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect();
    let mut expected: Vec<&str> = plan.topology.panes.iter().map(String::as_str).collect();
    live.sort_unstable();
    expected.sort_unstable();
    live == expected
}

/// Whether the tab still looks like it did when the plan was made, i.e. the
/// authoritative event has not arrived yet.
pub(crate) fn layout_still_matches_plan(layout: &PaneLayout, plan: &RelocationPlan) -> bool {
    layout_fingerprint(layout) == plan.fingerprint
}

impl RelocationPlan {
    /// Temporary tabs of an insert as the event stream reports them: tabs of
    /// the plan's workspace that did not exist at release and hold nothing
    /// but the source pane. Hidden from the tab strip and tab navigation
    /// alongside the id the step-1 response names (design §7.2), so the
    /// tab never flashes for the frames between the event and the response.
    pub(crate) fn unlisted_temp_tabs<'a>(
        &'a self,
        snapshot: &'a HierarchySnapshot,
    ) -> impl Iterator<Item = &'a str> + 'a {
        let inserting = matches!(self.intent, RelocationIntent::Insert { .. });
        snapshot
            .tabs_for(&self.workspace_id)
            .filter(move |tab| inserting && !self.known_tab_ids.contains(&tab.tab_id))
            .filter(|tab| {
                snapshot
                    .panes_for(&tab.tab_id)
                    .all(|pane| pane.pane_id == self.source_pane_id)
            })
            .map(|tab| tab.tab_id.as_str())
    }

    /// Both intents are same-tab only: the drop model never offers a pane
    /// of another tab as a target.
    pub(crate) fn is_supported(&self) -> bool {
        self.source_tab_id == self.target_tab_id
            && match self.intent {
                RelocationIntent::Swap => true,
                RelocationIntent::Insert { .. } => self.insert_shapes.is_some(),
            }
    }

    /// Pane ids the prediction lays out, in tree order.
    pub(crate) fn predicted_pane_ids(&self) -> impl Iterator<Item = &str> {
        self.predicted_rects
            .iter()
            .map(|pane| pane.pane_id.as_str())
    }

    /// Frame to show for the source pane while the plan is pending, when the
    /// runtime has none of its own yet.
    pub(crate) fn frame_for(&self, pane_id: &str) -> Option<RenderedFrame> {
        (pane_id == self.source_pane_id)
            .then(|| self.visual_snapshot.clone())
            .flatten()
    }

    pub(crate) fn predicted_fractions(&self) -> Vec<(String, (f32, f32, f32, f32))> {
        self.predicted_rects
            .iter()
            .filter_map(|pane| {
                Some((
                    pane.pane_id.clone(),
                    layout_rect_fractions(self.area, pane.rect)?,
                ))
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) struct SettlingSeed {
    pub(crate) plan: RelocationPlan,
    pub(crate) from: Vec<(String, (f32, f32, f32, f32))>,
}

#[cfg(test)]
impl SettlingSeed {
    pub(crate) fn into_settling(self, started: Instant) -> PendingPaneRelocation {
        PendingPaneRelocation {
            plan: self.plan,
            phase: RelocationPhase::Settling {
                started,
                from: self.from,
            },
        }
    }
}

impl PendingPaneRelocation {
    /// Pane fractions to render for this tab right now, or `None` when the
    /// pane is not part of the plan.
    pub(crate) fn display_fractions(
        &self,
        pane_id: &str,
        layout: Option<&PaneLayout>,
        now: Instant,
        reduce_motion: bool,
    ) -> Option<(f32, f32, f32, f32)> {
        match &self.phase {
            RelocationPhase::Swapping { .. }
            | RelocationPhase::Parking
            | RelocationPhase::Inserting { .. }
            | RelocationPhase::CorrectingOrder { .. } => self
                .plan
                .predicted_fractions()
                .into_iter()
                .find(|(id, _)| id == pane_id)
                .map(|(_, rect)| rect),
            RelocationPhase::Parked { .. } => None,
            RelocationPhase::Settling { started, from } => {
                let from = from.iter().find(|(id, _)| id == pane_id).map(|(_, r)| *r)?;
                let to = layout.and_then(|layout| {
                    let pane = layout.panes.iter().find(|pane| pane.pane_id == pane_id)?;
                    layout_rect_fractions(layout.area, pane.rect)
                })?;
                let progress = ochub_ui::anim::linear_progress(
                    *started,
                    PANE_SETTLE_ANIMATION,
                    now,
                    reduce_motion,
                );
                Some(lerp_rect(
                    from,
                    to,
                    ochub_ui::anim::ease_out_quint(progress),
                ))
            }
        }
    }

    pub(crate) fn is_settled(&self, now: Instant, reduce_motion: bool) -> bool {
        match &self.phase {
            RelocationPhase::Settling { started, .. } => {
                reduce_motion || now.saturating_duration_since(*started) >= PANE_SETTLE_ANIMATION
            }
            _ => false,
        }
    }
}
