use super::*;

impl OcHerdrView {
    pub(crate) fn begin_split_drag(
        &mut self,
        tab_id: String,
        split: LayoutSplit,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(surface) = self.terminal_surface_bounds else {
            cx.stop_propagation();
            return;
        };
        // A pending relocation or split batch owns this tab's geometry
        // until it settles.
        if self.tab_relocation_locked(&tab_id) {
            cx.stop_propagation();
            return;
        }
        let Some(drag) = self.snapshot.as_ref().and_then(|snapshot| {
            let layout = snapshot.layout_for(&tab_id)?;
            split_drag_from_press(tab_id, &split, layout, surface, mouse_point(event.position))
        }) else {
            cx.stop_propagation();
            return;
        };
        self.end_text_drag();
        self.cancel_reorder_drag();
        self.cancel_pane_drag();
        self.surface_drag = SurfaceDrag::Split(drag);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn end_text_drag_unless_pane(&mut self, pane_id: &str) {
        let Some((previous, captured)) = self.take_text_drag() else {
            return;
        };
        if previous == pane_id {
            self.surface_drag = SurfaceDrag::Text {
                pane_id: previous,
                captured,
            };
            return;
        }
        self.finish_text_drag_on(&previous);
    }

    pub(super) fn end_text_drag(&mut self) {
        if let Some((previous, _)) = self.take_text_drag() {
            self.finish_text_drag_on(&previous);
        }
    }

    pub(super) fn take_text_drag(&mut self) -> Option<(String, bool)> {
        match std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle) {
            SurfaceDrag::Text { pane_id, captured } => Some((pane_id, captured)),
            other => {
                self.surface_drag = other;
                None
            }
        }
    }

    pub(super) fn finish_text_drag_on(&mut self, pane_id: &str) {
        if let Some(runtime) = self.pane_mut(pane_id) {
            runtime
                .terminal
                .end_text_selection(None, KeyModifiers::default());
        }
    }

    /// Navigation and stream death void the gesture outright. Snapshot
    /// mutations go through `reconcile_split_drag` so a ratio-only
    /// `layout.updated` (including our own submit) does not self-cancel.
    pub(super) fn cancel_split_drag(&mut self) {
        if matches!(self.surface_drag, SurfaceDrag::Split(_)) {
            self.surface_drag = SurfaceDrag::Idle;
        }
    }

    pub(super) fn take_split_drag(&mut self) -> Option<SplitDrag> {
        match std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle) {
            SurfaceDrag::Split(drag) => Some(drag),
            other => {
                self.surface_drag = other;
                None
            }
        }
    }

    pub(super) fn reconcile_split_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.take_split_drag() else {
            return;
        };
        if let SurfaceDrag::Split(drag) = reconcile_split_drag_state(drag, self.snapshot.as_ref()) {
            self.surface_drag = SurfaceDrag::Split(drag);
        } else {
            cx.notify();
        }
    }

    pub(super) fn update_split_drag(&mut self, mouse: (f32, f32), cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.take_split_drag() else {
            return false;
        };
        let previous = drag.preview_ratio;
        let drag = apply_split_drag_pointer(
            drag,
            self.snapshot.as_ref(),
            self.terminal_surface_bounds,
            mouse,
        );
        if (drag.preview_ratio - previous).abs() > f32::EPSILON {
            cx.notify();
        }
        self.surface_drag = SurfaceDrag::Split(drag);
        true
    }

    pub(super) fn finish_split_drag(&mut self, mouse: (f32, f32), cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.take_split_drag() else {
            return false;
        };
        let drag = apply_split_drag_pointer(
            drag,
            self.snapshot.as_ref(),
            self.terminal_surface_bounds,
            mouse,
        );
        cx.notify();
        let SurfaceDrag::Split(drag) = reconcile_split_drag_state(drag, self.snapshot.as_ref())
        else {
            return true;
        };
        let ratios = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.layout_for(&drag.tab_id))
            .map(|layout| split_drag_ratios(layout, &drag.path, drag.preview_ratio))
            .unwrap_or_default();
        let ratios = split_commit_ratios(ratios, drag.start_ratio);
        if ratios.is_empty() {
            return true;
        }
        let last_ratios = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.layout_for(&drag.tab_id))
            .map(split_ratios_of)
            .unwrap_or_default();
        self.pane_relocation_serial = self.pane_relocation_serial.wrapping_add(1);
        let serial = self.pane_relocation_serial;
        self.split_commit = Some(PendingSplitCommit {
            tab_id: drag.tab_id.clone(),
            layout: drag.layout.clone(),
            ratios: ratios.clone(),
            serial,
            outstanding: ratios.len(),
            last_ratios,
            layouts_seen: 0,
        });
        // The dragged split first, then the pinned descendants, back to back:
        // Herdr applies them in order and emits one `layout.updated` each.
        for (path, ratio) in ratios {
            self.invoke_with_response(
                "layout.set_split_ratio",
                json!({
                    "tab_id": drag.tab_id,
                    "path": path,
                    "ratio": ratio,
                }),
                move |this, result, cx| this.split_commit_responded(serial, result.is_ok(), cx),
                cx,
            );
        }
        true
    }

    /// One `layout.set_split_ratio` of the release batch answered. An error
    /// drops the preview (the authoritative layout is whatever Herdr kept);
    /// otherwise the commit settles once the layout shows the ratios.
    pub(super) fn split_commit_responded(&mut self, serial: u64, ok: bool, cx: &mut Context<Self>) {
        let Some(commit) = self.split_commit.as_mut() else {
            return;
        };
        if commit.serial != serial {
            return;
        }
        if !ok {
            self.split_commit = None;
            cx.notify();
            return;
        }
        commit.outstanding = commit.outstanding.saturating_sub(1);
        self.reconcile_split_commit(cx);
    }

    /// Drop the release batch's preview once the authoritative layout
    /// carries it (or one layout per answered request came in), or when the
    /// tab's split shape changed under it.
    pub(super) fn reconcile_split_commit(&mut self, cx: &mut Context<Self>) {
        let Some(commit) = self.split_commit.as_mut() else {
            return;
        };
        let keep = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.layout_for(&commit.tab_id))
            .is_some_and(|layout| {
                split_layout_fingerprint(layout) == commit.layout && !commit.observe(layout)
            });
        if !keep {
            self.split_commit = None;
            cx.notify();
        }
    }

    // ---- Pane drag: handle press, hover, release, cancel (design §5, §7) ----

    /// The tab is locked while a relocation plan is pending: no split drag,
    /// no second pane drag, no pane close, frozen terminal resizes.
    pub(crate) fn tab_relocation_locked(&self, tab_id: &str) -> bool {
        self.pane_relocations
            .get(tab_id)
            .is_some_and(|pending| pending.phase.locks_tab())
            || self
                .pane_detaches
                .values()
                .any(|pending| pending.locks_tab(tab_id))
            || self
                .split_commit
                .as_ref()
                .is_some_and(|commit| commit.tab_id == tab_id)
            || self.pane_template_commits.contains_key(tab_id)
    }

    /// Whether the four edge zones accept drops on this connection: the
    /// `pane-edge-relocation` flag and the `pane.move` capability (design
    /// §8, §13 step 3).
    pub(crate) fn edge_drops_enabled(&self) -> bool {
        self.pane_edge_relocation && self.pane_move_supported()
    }

    /// Temporary tabs of in-flight inserts, kept out of the tab strip and
    /// tab navigation (design §7.2): the id the step-1 response named, plus
    /// any tab the event stream added for the plan before or after that
    /// response (`RelocationPlan::unlisted_temp_tabs`). A parked plan shows
    /// its tab on purpose.
    pub(crate) fn hidden_tab_ids(&self) -> HashSet<String> {
        let mut hidden: HashSet<String> = self
            .pane_relocations
            .values()
            .filter_map(|pending| pending.phase.hidden_tab_id().map(str::to_owned))
            .collect();
        if let Some(snapshot) = self.snapshot.as_ref() {
            for pending in self
                .pane_relocations
                .values()
                .filter(|pending| pending.phase.locks_tab())
            {
                hidden.extend(pending.plan.unlisted_temp_tabs(snapshot).map(str::to_owned));
            }
            for pending in self.pane_template_commits.values() {
                hidden.extend(pending.hidden_tab_ids(snapshot));
            }
        }
        hidden
    }

    /// Panes to draw in a tab. While an insert plan is pending the source
    /// pane's record lives in the temporary tab, so the plan's pane set is
    /// used instead of the snapshot's grouping.
    pub(crate) fn rendered_panes_for_tab(
        &self,
        snapshot: &HierarchySnapshot,
        tab_id: &str,
    ) -> Vec<PaneInfo> {
        if let Some(pending) = self.pane_template_commits.get(tab_id) {
            return pending
                .predicted_pane_ids()
                .filter_map(|pane_id| snapshot.pane(pane_id).cloned())
                .collect();
        }
        if let Some(pending) = self.pane_detach_for_tab(tab_id) {
            let ids = pending
                .tab_state(tab_id)
                .map(|state| {
                    state
                        .predicted_pane_ids()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return ids
                .into_iter()
                .filter_map(|pane_id| snapshot.pane(&pane_id).cloned())
                .collect();
        }
        if let Some(pending) = self.pane_relocations.get(tab_id)
            && pending.phase.locks_tab()
        {
            return pending
                .plan
                .predicted_pane_ids()
                .filter_map(|pane_id| snapshot.pane(pane_id).cloned())
                .collect();
        }
        if let Some(layout) = snapshot.layout_for(tab_id)
            && layout.zoomed
        {
            return snapshot
                .pane(&layout.focused_pane_id)
                .cloned()
                .into_iter()
                .collect();
        }
        snapshot.panes_for(tab_id).cloned().collect()
    }

    /// A pending insert keeps the source pane selected in its original tab
    /// even while Herdr has it in the temporary tab (the record's `tab_id`
    /// changes twice before the plan settles).
    pub(super) fn pin_relocation_selection(&mut self) {
        let Some(tab_id) = self.selection.tab_id.clone() else {
            return;
        };
        if let Some(pending) = self.pane_template_commits.get(&tab_id) {
            let source = pending.source_pane_id.clone();
            if self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.pane(&source).is_some())
            {
                self.selection.pane_id = Some(source);
            }
            return;
        }
        let Some(pending) = self.pane_relocations.get(&tab_id) else {
            return;
        };
        if !matches!(pending.plan.intent, RelocationIntent::Insert { .. })
            || !pending.phase.locks_tab()
        {
            return;
        }
        let source = pending.plan.source_pane_id.clone();
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.pane(&source).is_some())
        {
            self.selection.pane_id = Some(source);
        }
    }

    /// Disconnect: nothing in flight can be cancelled. Remember a pane that
    /// was already parked so the reconnect snapshot can offer recovery.
    pub(super) fn abort_pane_relocations_for_disconnect(&mut self) {
        let parked = self
            .pane_relocations
            .values()
            .find_map(|pending| match &pending.phase {
                RelocationPhase::Inserting {
                    temp_tab_id,
                    moved_pane_id,
                    ..
                }
                | RelocationPhase::Parked {
                    temp_tab_id,
                    moved_pane_id,
                } => Some(ParkedRecovery {
                    plan: pending.plan.clone(),
                    temp_tab_id: temp_tab_id.clone(),
                    moved_pane_id: moved_pane_id.clone(),
                }),
                _ => None,
            });
        self.pane_relocations.clear();
        self.pane_detaches.clear();
        self.pane_template_commits.clear();
        if parked.is_some() {
            self.parked_recovery = parked;
        }
    }

    /// Reconnect: if the temporary tab still holds the parked pane, show
    /// the Parked notice again (design §7.3).
    pub(super) fn restore_parked_recovery(&mut self, cx: &mut Context<Self>) {
        let Some(recovery) = self.parked_recovery.take() else {
            return;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let still_parked = snapshot
            .pane(&recovery.moved_pane_id)
            .is_some_and(|pane| pane.tab_id == recovery.temp_tab_id)
            && snapshot.layout_for(&recovery.plan.source_tab_id).is_some();
        if !still_parked
            || self
                .pane_relocations
                .contains_key(&recovery.plan.source_tab_id)
        {
            return;
        }
        self.pane_relocations.insert(
            recovery.plan.source_tab_id.clone(),
            PendingPaneRelocation {
                plan: recovery.plan,
                phase: RelocationPhase::Parked {
                    temp_tab_id: recovery.temp_tab_id,
                    moved_pane_id: recovery.moved_pane_id,
                },
            },
        );
        cx.notify();
    }

    /// Top-left of the measured terminal surface in window pixels; zero
    /// before the first layout pass.
    pub(crate) fn surface_origin(&self) -> (f32, f32) {
        self.terminal_surface_bounds
            .map(|surface| (surface.0, surface.1))
            .unwrap_or((0., 0.))
    }

    pub(super) fn pane_tab_id(&self, pane_id: &str) -> Option<String> {
        self.snapshot
            .as_ref()?
            .pane(pane_id)
            .map(|pane| pane.tab_id.clone())
    }

    /// Pane drag handle pressed. The gesture only becomes a drag once the
    /// pointer travels more than `PANE_DRAG_SLOP_PX`; until then the release
    /// is a plain click that selects the pane.
    /// Mouse-down on a pane's drag handle. Stopping propagation keeps the
    /// pane's own mouse-down (text selection) and the surface's focus-on-
    /// click out of the gesture, so the surface is focused here explicitly:
    /// Esc during the drag is handled by the root `on_key_down`, which GPUI
    /// only dispatches along the focused element's ancestry, and with
    /// nothing focused the window's root node alone receives the key.
    pub(crate) fn press_pane_handle(
        &mut self,
        pane_id: String,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus.is_focused(window) {
            self.focus.focus(window, cx);
        }
        if self.begin_pane_drag(pane_id, mouse_point(event.position)) {
            cx.notify();
        }
        cx.stop_propagation();
    }

    /// Returns `true` when a drag was armed. Refuses while any other gesture
    /// is active, while the tab has a pending plan, when the layout is zoomed,
    /// or when the pane rect cannot be measured yet. A single-pane tab can
    /// start a drag if tab-bar drops are available; pane-local targets still
    /// require two or more panes.
    pub(crate) fn begin_pane_drag(&mut self, pane_id: String, pointer: (f32, f32)) -> bool {
        if !matches!(self.overlay, Overlay::None) {
            return false;
        }
        if !matches!(
            self.surface_drag,
            SurfaceDrag::Idle | SurfaceDrag::Text { .. }
        ) {
            return false;
        }
        let Some(surface) = self.terminal_surface_bounds else {
            return false;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let Some(pane) = snapshot.pane(&pane_id) else {
            return false;
        };
        let (workspace_id, tab_id) = (pane.workspace_id.clone(), pane.tab_id.clone());
        if self.tab_relocation_locked(&tab_id) {
            return false;
        }
        let Some(layout) = snapshot.layout_for(&tab_id) else {
            return false;
        };
        if layout.zoomed {
            return false;
        }
        let tab_bar_drops = self.pane_move_supported();
        if layout.panes.len() < 2 && !tab_bar_drops {
            return false;
        }
        let Some(source_rect) = pane_window_rect(layout, &pane_id, surface) else {
            return false;
        };
        let fingerprint = layout_fingerprint(layout);
        let pane_count = layout.panes.len();
        // Capture before anything dims or re-lays out the slot: the source
        // body and the floating preview draw this when the runtime has no
        // frame of its own on a given render.
        self.pane_drag_snapshot = self
            .pane(&pane_id)
            .and_then(|runtime| runtime.frame.clone());
        self.end_text_drag();
        self.pane_drag_return = None;
        self.surface_drag = SurfaceDrag::Pane(PaneDrag {
            workspace_id,
            tab_id,
            pane_id,
            fingerprint,
            origin: pointer,
            pointer,
            grab_offset: (pointer.0 - source_rect.0, pointer.1 - source_rect.1),
            source_rect,
            hover: None,
            template_hover: None,
            tab_target: None,
            layout_preview: None,
            edge_drops: self.edge_drops_enabled(),
            layout_templates: self.pane_move_supported() && pane_count >= 2,
            tab_bar_drops,
            pressed_at: Instant::now(),
        });
        true
    }

    pub(super) fn take_pane_drag(&mut self) -> Option<PaneDrag> {
        match std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle) {
            SurfaceDrag::Pane(drag) => Some(drag),
            other => {
                self.surface_drag = other;
                None
            }
        }
    }

    /// Esc, navigation, disconnect, or a structural layout change. The preview
    /// flies back to its slot when it was already lifted.
    pub(crate) fn cancel_pane_drag(&mut self) {
        if let Some(drag) = self.take_pane_drag()
            && pane_drag_past_slop(&drag)
        {
            let started = Instant::now();
            let layout_from = drag
                .layout_preview
                .as_ref()
                .map(|preview| {
                    preview
                        .from
                        .iter()
                        .filter_map(|(pane_id, _)| {
                            Some((
                                pane_id.clone(),
                                preview.display_fractions(pane_id, started, false)?,
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let from = pane_drag_preview_rect(&drag);
            self.pane_drag_return = Some(PaneDragReturn {
                pane_id: drag.pane_id.clone(),
                tab_id: drag.tab_id,
                from,
                to: drag.source_rect,
                layout_from,
                started,
            });
        }
    }

    /// Snapshot changed: keep the drag only while the tab's structure is
    /// exactly what it was at press.
    pub(super) fn reconcile_pane_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.take_pane_drag() else {
            return;
        };
        let survives = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.layout_for(&drag.tab_id))
            .is_some_and(|layout| layout_fingerprint(layout) == drag.fingerprint);
        if survives {
            self.surface_drag = SurfaceDrag::Pane(drag);
        } else {
            self.surface_drag = SurfaceDrag::Pane(drag);
            self.cancel_pane_drag();
            cx.notify();
        }
    }

    pub(crate) fn update_pane_drag(&mut self, mouse: (f32, f32), cx: &mut Context<Self>) -> bool {
        self.update_pane_drag_with_hits(mouse, None, None, cx)
    }

    pub(crate) fn update_pane_drag_over_template(
        &mut self,
        mouse: (f32, f32),
        template_hover: PaneTemplateHover,
        cx: &mut Context<Self>,
    ) -> bool {
        self.update_pane_drag_with_hits(mouse, Some(template_hover), None, cx)
    }

    pub(super) fn update_pane_drag_with_hits(
        &mut self,
        mouse: (f32, f32),
        painted_template_hover: Option<PaneTemplateHover>,
        painted_tab_target: Option<PaneTabDropTarget>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut drag) = self.take_pane_drag() else {
            return false;
        };
        drag.pointer = mouse;
        let tab_target = if pane_drag_past_slop(&drag) && drag.tab_bar_drops {
            painted_tab_target
        } else {
            None
        };
        let template_hover =
            if pane_drag_past_slop(&drag) && drag.layout_templates && tab_target.is_none() {
                painted_template_hover.or_else(|| self.pane_template_hover_for(&drag))
            } else {
                None
            };
        let mut hover =
            if pane_drag_past_slop(&drag) && tab_target.is_none() && template_hover.is_none() {
                self.pane_hover_for(&drag)
            } else {
                None
            };
        if pane_drag_past_slop(&drag)
            && let Some(layout) = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.layout_for(&drag.tab_id))
        {
            let now = Instant::now();
            let intent = pane_drag_preview_intent(
                hover.as_ref(),
                template_hover.as_ref(),
                tab_target.as_ref(),
                drag.edge_drops,
            );
            drag.layout_preview = update_pane_drag_layout_preview(
                layout,
                &drag.pane_id,
                intent,
                drag.layout_preview.as_ref(),
                now,
                cx.reduce_motion(),
            );
            // The drop highlight follows the reserved source slot. Hit
            // testing on the next pointer event still uses the immutable
            // authoritative pane rects in `pane_hover_for`.
            if let Some(slot) = drag
                .layout_preview
                .as_ref()
                .filter(|preview| preview.intent.is_some())
                .and_then(|preview| preview.target_fractions(&drag.pane_id))
                && let Some(surface) = self.terminal_surface_bounds
                && let Some(hover) = hover.as_mut()
            {
                hover.target_rect = fractions_to_window(surface, slot);
            }
        }
        drag.hover = hover;
        drag.template_hover = template_hover;
        drag.tab_target = tab_target;
        self.surface_drag = SurfaceDrag::Pane(drag);
        cx.notify();
        true
    }

    pub(super) fn pane_hover_for(&self, drag: &PaneDrag) -> Option<PaneDropHover> {
        let surface = self.terminal_surface_bounds?;
        let layout = self.snapshot.as_ref()?.layout_for(&drag.tab_id)?;
        pane_drop_hover(layout, &drag.pane_id, surface, drag.pointer)
    }

    pub(super) fn pane_template_hover_for(&self, drag: &PaneDrag) -> Option<PaneTemplateHover> {
        let surface = self.terminal_surface_bounds?;
        let layout = self.snapshot.as_ref()?.layout_for(&drag.tab_id)?;
        pane_template_hover(surface, layout.panes.len(), drag.pointer)
    }

    /// Release. A click selects; a lifted preview over the centre zone
    /// commits a swap; anything else returns home without a request.
    pub(crate) fn finish_pane_drag(
        &mut self,
        mouse: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut drag) = self.take_pane_drag() else {
            return false;
        };
        // A template cell resolves its hit against the geometry that was
        // actually painted. Keep that semantic hit for the matching mouse-up
        // instead of recalculating it from a terminal surface that may already
        // have published bounds for the next frame.
        let pointer_unchanged =
            (drag.pointer.0 - mouse.0).abs() <= 0.5 && (drag.pointer.1 - mouse.1).abs() <= 0.5;
        let painted_template_hover = pointer_unchanged
            .then(|| drag.template_hover.clone())
            .flatten();
        let painted_tab_target = pointer_unchanged.then(|| drag.tab_target.clone()).flatten();
        drag.pointer = mouse;
        cx.notify();
        if !pane_drag_past_slop(&drag) {
            self.select_pane(drag.pane_id, window, cx);
            return true;
        }
        drag.tab_target = if drag.tab_bar_drops {
            painted_tab_target
        } else {
            None
        };
        if let Some(destination) = drag.tab_target.clone() {
            let committed = self.commit_pane_tab_drop(
                &drag.workspace_id,
                &drag.tab_id,
                &drag.pane_id,
                drag.fingerprint,
                destination,
                cx,
            );
            if !committed {
                self.surface_drag = SurfaceDrag::Pane(drag);
                self.cancel_pane_drag();
            }
            return true;
        }
        drag.template_hover = if drag.layout_templates {
            painted_template_hover.or_else(|| self.pane_template_hover_for(&drag))
        } else {
            None
        };
        drag.hover = if drag.template_hover.is_none() {
            self.pane_hover_for(&drag)
        } else {
            None
        };
        if let Some(template) = drag.template_hover.as_ref() {
            let committed = self.commit_pane_template(
                &drag.workspace_id,
                &drag.tab_id,
                &drag.pane_id,
                drag.fingerprint,
                template.placement,
                cx,
            );
            if !committed {
                self.surface_drag = SurfaceDrag::Pane(drag);
                self.cancel_pane_drag();
            }
            return true;
        }
        let droppable = drag
            .hover
            .as_ref()
            .is_some_and(|hover| hover.droppable(drag.edge_drops));
        let committed = droppable
            && self.commit_pane_drop(
                &PaneDropRequest {
                    workspace_id: drag.workspace_id.clone(),
                    tab_id: drag.tab_id.clone(),
                    pane_id: drag.pane_id.clone(),
                    fingerprint: drag.fingerprint,
                    hover: drag.hover.clone().expect("droppable implies hover"),
                    edge_drops: drag.edge_drops,
                },
                cx,
            );
        if !committed {
            self.surface_drag = SurfaceDrag::Pane(drag);
            self.cancel_pane_drag();
        }
        true
    }

    /// Build the `RelocationPlan`, render it, and send the first request:
    /// exactly one `pane.swap` for the centre zone, or step 1 of the §4.2
    /// orchestration for an edge. Returns `false` when the plan cannot be
    /// built (the layout moved, the tab is locked, the zone is not
    /// droppable), in which case nothing is sent.
    pub(super) fn commit_pane_drop(
        &mut self,
        request: &PaneDropRequest,
        cx: &mut Context<Self>,
    ) -> bool {
        let hover = &request.hover;
        if !hover.droppable(request.edge_drops) {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let Some(layout) = snapshot.layout_for(&request.tab_id) else {
            return false;
        };
        if layout_fingerprint(layout) != request.fingerprint
            || self.tab_relocation_locked(&request.tab_id)
        {
            return false;
        }
        let (intent, predicted_rects, insert_shapes) = match hover.zone.edge() {
            None => (
                RelocationIntent::Swap,
                predict_swap(layout, &request.pane_id, &hover.target_pane_id),
                None,
            ),
            Some(edge) => {
                if !self.edge_drops_enabled() {
                    return false;
                }
                let steps = predict_relocation_steps(
                    layout,
                    &request.pane_id,
                    &hover.target_pane_id,
                    edge,
                    PANE_EDGE_DROP_RATIO,
                );
                (
                    RelocationIntent::Insert {
                        edge,
                        ratio: PANE_EDGE_DROP_RATIO,
                    },
                    steps.as_ref().map(|steps| steps.final_layout.panes.clone()),
                    steps.as_ref().map(InsertShapes::from_steps),
                )
            }
        };
        let Some(predicted_rects) = predicted_rects else {
            return false;
        };
        self.pane_relocation_serial = self.pane_relocation_serial.wrapping_add(1);
        let plan = RelocationPlan {
            operation_id: self.pane_relocation_serial,
            source_pane_id: request.pane_id.clone(),
            source_tab_id: request.tab_id.clone(),
            target_pane_id: hover.target_pane_id.clone(),
            target_tab_id: request.tab_id.clone(),
            intent,
            fingerprint: request.fingerprint,
            topology: split_layout_fingerprint(layout),
            area: layout.area,
            predicted_rects,
            visual_snapshot: self
                .pane(&request.pane_id)
                .and_then(|runtime| runtime.frame.clone())
                .or_else(|| self.pane_drag_snapshot.clone()),
            workspace_id: request.workspace_id.clone(),
            known_tab_ids: snapshot
                .tabs_for(&request.workspace_id)
                .map(|tab| tab.tab_id.clone())
                .collect(),
            insert_shapes,
        };
        if !plan.is_supported() {
            return false;
        }
        let operation_id = plan.operation_id;
        let tab_id = plan.source_tab_id.clone();
        match intent {
            RelocationIntent::Swap => {
                let params = json!({
                    "source_pane_id": plan.source_pane_id,
                    "target_pane_id": plan.target_pane_id,
                });
                self.pane_relocations.insert(
                    tab_id.clone(),
                    PendingPaneRelocation {
                        plan,
                        phase: RelocationPhase::Swapping {
                            responded: false,
                            layout_seen: false,
                        },
                    },
                );
                self.invoke_with_response(
                    "pane.swap",
                    params,
                    move |this, result, cx| {
                        this.on_pane_swap_response(&tab_id, operation_id, result, cx);
                    },
                    cx,
                );
            }
            RelocationIntent::Insert { .. } => {
                self.pane_relocations.insert(
                    tab_id.clone(),
                    PendingPaneRelocation {
                        plan,
                        phase: RelocationPhase::Parking,
                    },
                );
                self.send_park_request(&tab_id, operation_id, cx);
            }
        }
        true
    }

    // ---- Insert orchestration (design §4.2, §7.2) ----
    //
    // Three strictly serial requests. Each next one is issued inside the
    // previous response callback; nothing waits on events or snapshots.

    /// Step 1: `pane.move { destination: new_tab }` with `focus: false`.
    pub(super) fn send_park_request(
        &mut self,
        tab_id: &str,
        operation_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pane_relocations.get(tab_id) else {
            return;
        };
        let params = json!({
            "pane_id": pending.plan.source_pane_id,
            "destination": {
                "type": "new_tab",
                "workspace_id": pending.plan.workspace_id,
            },
            "focus": false,
        });
        let tab_id = tab_id.to_owned();
        self.invoke_with_response(
            "pane.move",
            params,
            move |this, result, cx| {
                let parked = result
                    .ok()
                    .and_then(|value| parked_pane_from_response(&value));
                this.apply_relocation_signal(
                    &tab_id,
                    operation_id,
                    RelocationSignal::Parked(parked),
                    cx,
                );
            },
            cx,
        );
    }

    /// Step 2: `pane.move { destination: tab }` back beside the target with
    /// the ids the step-1 response reported, `focus: true`.
    pub(super) fn send_insert_request(
        &mut self,
        tab_id: &str,
        operation_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pane_relocations.get(tab_id) else {
            return;
        };
        let RelocationPhase::Inserting { moved_pane_id, .. } = &pending.phase else {
            return;
        };
        let RelocationIntent::Insert { edge, ratio } = pending.plan.intent else {
            return;
        };
        let params = json!({
            "pane_id": moved_pane_id,
            "destination": {
                "type": "tab",
                "tab_id": pending.plan.target_tab_id,
                "target_pane_id": pending.plan.target_pane_id,
                "split": edge.split_direction(),
                "ratio": edge.request_ratio(ratio),
            },
            "focus": true,
        });
        let tab_id = tab_id.to_owned();
        self.invoke_with_response(
            "pane.move",
            params,
            move |this, result, cx| {
                let accepted = result.is_ok_and(|value| pane_move_changed(&value));
                this.apply_relocation_signal(
                    &tab_id,
                    operation_id,
                    RelocationSignal::Inserted(accepted),
                    cx,
                );
            },
            cx,
        );
    }

    /// Step 3 (left/up): `pane.swap` so the moved pane becomes the first
    /// child.
    pub(super) fn send_order_swap_request(
        &mut self,
        tab_id: &str,
        operation_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pane_relocations.get(tab_id) else {
            return;
        };
        let params = json!({
            "source_pane_id": pending.plan.source_pane_id,
            "target_pane_id": pending.plan.target_pane_id,
        });
        let tab_id = tab_id.to_owned();
        self.invoke_with_response(
            "pane.swap",
            params,
            move |this, result, cx| {
                let accepted = result.is_ok_and(|value| pane_swap_changed(&value));
                this.apply_relocation_signal(
                    &tab_id,
                    operation_id,
                    RelocationSignal::Reordered(accepted),
                    cx,
                );
            },
            cx,
        );
    }

    /// Feed one signal to the insert state machine of the plan on `tab_id`
    /// and carry out the resulting action. `operation_id` guards against a
    /// late response for a replaced plan (`None` for layout signals).
    pub(super) fn apply_relocation_signal(
        &mut self,
        tab_id: &str,
        operation_id: u64,
        signal: RelocationSignal,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pane_relocations.get_mut(tab_id) else {
            return;
        };
        if pending.plan.operation_id != operation_id
            || !matches!(pending.plan.intent, RelocationIntent::Insert { .. })
        {
            return;
        }
        let corrects_order = pending.plan.intent.corrects_order();
        let phase = std::mem::replace(&mut pending.phase, RelocationPhase::Parking);
        let (next, action) = advance_insert_phase(phase, signal, corrects_order);
        match next {
            Some(phase) => pending.phase = phase,
            None => {
                self.pane_relocations.remove(tab_id);
            }
        }
        match action {
            RelocationAction::None => {}
            RelocationAction::SendInsert => self.send_insert_request(tab_id, operation_id, cx),
            RelocationAction::SendSwap => self.send_order_swap_request(tab_id, operation_id, cx),
            RelocationAction::Settle => self.settle_pane_relocation(tab_id, cx),
            RelocationAction::Revert => {}
            RelocationAction::Park => {
                // The failure toast came from the invoke path; the inline
                // notice with Retry / Go to tab is rendered from the phase.
            }
            RelocationAction::Misordered => {
                self.notify_failure(
                    FailureKind::PaneMisordered,
                    self.i18n.text(k::NOTIFY_DETAIL_PANE_MISORDERED),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// "Retry" on the parked notice: re-issue step 2 with the original plan.
    pub(crate) fn retry_parked_relocation(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(operation_id) = self
            .pane_relocations
            .get(tab_id)
            .filter(|pending| matches!(pending.phase, RelocationPhase::Parked { .. }))
            .map(|pending| pending.plan.operation_id)
        else {
            return;
        };
        self.apply_relocation_signal(tab_id, operation_id, RelocationSignal::Retry, cx);
    }

    /// "Go to tab" on the parked notice: drop the plan and focus the
    /// temporary tab holding the pane.
    pub(crate) fn go_to_parked_tab(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(temp_tab_id) = self
            .pane_relocations
            .get(tab_id)
            .and_then(|pending| pending.phase.parked_tab_id().map(str::to_owned))
        else {
            return;
        };
        self.pane_relocations.remove(tab_id);
        self.select_tab(temp_tab_id, cx);
    }

    /// The parked plan (if any) of this tab, for the inline notice.
    pub(crate) fn parked_relocation(&self, tab_id: &str) -> Option<&PendingPaneRelocation> {
        self.pane_relocations
            .get(tab_id)
            .filter(|pending| matches!(pending.phase, RelocationPhase::Parked { .. }))
    }

    pub(super) fn on_pane_swap_response(
        &mut self,
        tab_id: &str,
        operation_id: u64,
        result: std::result::Result<Value, HerdrError>,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pane_relocations.get_mut(tab_id) else {
            return;
        };
        if pending.plan.operation_id != operation_id
            || !matches!(pending.phase, RelocationPhase::Swapping { .. })
        {
            return;
        }
        let accepted = match &result {
            Ok(value) => pane_swap_changed(value),
            Err(_) => false,
        };
        if !accepted {
            // Failure notice already posted by the invoke path. Drop the
            // prediction: the authoritative snapshot is what is on screen.
            self.pane_relocations.remove(tab_id);
            cx.notify();
            return;
        }
        if let RelocationPhase::Swapping { responded, .. } = &mut pending.phase {
            *responded = true;
        }
        self.settle_pane_relocation_if_ready(tab_id, cx);
    }

    /// Snapshot changed: for each pending plan decide whether the
    /// authoritative layout is the swap landing (settle), still the old one
    /// (keep waiting), or something else (revert).
    pub(super) fn reconcile_pane_relocations(&mut self, cx: &mut Context<Self>) {
        self.reconcile_pane_template_commits(cx);
        self.reconcile_pane_detaches(cx);
        let tab_ids: Vec<String> = self.pane_relocations.keys().cloned().collect();
        for tab_id in tab_ids {
            let Some(pending) = self.pane_relocations.get_mut(&tab_id) else {
                continue;
            };
            let layout = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.layout_for(&tab_id));
            let is_insert = matches!(pending.plan.intent, RelocationIntent::Insert { .. });
            match (&mut pending.phase, layout) {
                (_, None) => {
                    self.pane_relocations.remove(&tab_id);
                    cx.notify();
                }
                (
                    RelocationPhase::Parked {
                        temp_tab_id,
                        moved_pane_id,
                    },
                    Some(_),
                ) => {
                    // The notice is only meaningful while the pane really
                    // sits in the temporary tab.
                    let still_parked = self.snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot
                            .pane(moved_pane_id)
                            .is_some_and(|pane| &pane.tab_id == temp_tab_id)
                    });
                    if !still_parked {
                        self.pane_relocations.remove(&tab_id);
                        cx.notify();
                    }
                }
                (RelocationPhase::Swapping { layout_seen, .. }, Some(layout)) => {
                    if layout_settles_plan(layout, &pending.plan) {
                        *layout_seen = true;
                        self.settle_pane_relocation_if_ready(&tab_id, cx);
                    } else if !layout_still_matches_plan(layout, &pending.plan) {
                        self.pane_relocations.remove(&tab_id);
                        cx.notify();
                    }
                }
                (RelocationPhase::Settling { .. }, Some(layout)) => {
                    let settled = if is_insert {
                        classify_insert_layout(layout, &pending.plan) == LayoutShape::Final
                    } else {
                        layout_settles_plan(layout, &pending.plan)
                    };
                    if !settled {
                        self.pane_relocations.remove(&tab_id);
                        cx.notify();
                    }
                }
                (
                    RelocationPhase::Parking
                    | RelocationPhase::Inserting { .. }
                    | RelocationPhase::CorrectingOrder { .. },
                    Some(layout),
                ) => {
                    let shape = classify_insert_layout(layout, &pending.plan);
                    let operation_id = pending.plan.operation_id;
                    self.apply_relocation_signal(
                        &tab_id,
                        operation_id,
                        RelocationSignal::Layout(shape),
                        cx,
                    );
                }
            }
        }
    }

    pub(super) fn settle_pane_relocation_if_ready(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(pending) = self.pane_relocations.get_mut(tab_id) else {
            return;
        };
        let RelocationPhase::Swapping {
            responded: true,
            layout_seen: true,
        } = pending.phase
        else {
            return;
        };
        self.settle_pane_relocation(tab_id, cx);
    }

    /// Response and matching layout are both in: run the correction from
    /// the predicted rects to the authoritative ones, or land at once under
    /// reduce-motion. Selection returns to the moved pane.
    pub(super) fn settle_pane_relocation(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let reduce_motion = cx.reduce_motion();
        let Some(pending) = self.pane_relocations.get_mut(tab_id) else {
            return;
        };
        let source = pending.plan.source_pane_id.clone();
        if reduce_motion {
            self.pane_relocations.remove(tab_id);
        } else {
            pending.phase = RelocationPhase::Settling {
                started: Instant::now(),
                from: pending.plan.predicted_fractions(),
            };
        }
        if self.selection.tab_id.as_deref() == Some(tab_id)
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.pane(&source).is_some())
        {
            self.selection.pane_id = Some(source);
            self.ensure_session_terminals(cx);
        }
        cx.notify();
    }

    /// Render-time cleanup: drop settled plans and finished return flights.
    pub(crate) fn expire_pane_motion(&mut self, now: Instant, reduce_motion: bool) -> bool {
        let before = self.pane_relocations.len() + usize::from(self.pane_drag_return.is_some());
        self.pane_relocations
            .retain(|_, pending| !pending.is_settled(now, reduce_motion));
        if self.pane_drag_return.as_ref().is_some_and(|flight| {
            reduce_motion
                || now.saturating_duration_since(flight.started) >= PANE_DRAG_RETURN_ANIMATION
        }) {
            self.pane_drag_return = None;
        }
        if self.pane_drag_snapshot.is_some()
            && !matches!(self.surface_drag, SurfaceDrag::Pane(_))
            && self.pane_drag_return.is_none()
            && self.pane_relocations.is_empty()
            && self.pane_detaches.is_empty()
            && self.pane_template_commits.is_empty()
        {
            self.pane_drag_snapshot = None;
        }
        before != self.pane_relocations.len() + usize::from(self.pane_drag_return.is_some())
    }

    /// Fractions to draw a pane at: the pending plan's prediction (or its
    /// settling correction) wins over the authoritative layout.
    pub(crate) fn displayed_pane_fractions(
        &self,
        layout: Option<&PaneLayout>,
        pane_id: &str,
        now: Instant,
        reduce_motion: bool,
    ) -> Option<(f32, f32, f32, f32)> {
        let layout = layout?;
        if layout.zoomed {
            return (layout.focused_pane_id == pane_id).then_some((0., 0., 1., 1.));
        }
        if let Some(pending) = self.pane_template_commits.get(&layout.tab_id)
            && let Some(rect) = pending
                .predicted_fractions(layout.area)
                .into_iter()
                .find(|(id, _)| id == pane_id)
                .map(|(_, rect)| rect)
        {
            return Some(rect);
        }
        if let Some(pending) = self.pane_detach_for_tab(&layout.tab_id)
            && let Some(rect) = pending
                .tab_state(&layout.tab_id)
                .map(|state| state.predicted_fractions())
                .unwrap_or_default()
                .into_iter()
                .find(|(id, _)| id == pane_id)
                .map(|(_, rect)| rect)
        {
            return Some(rect);
        }
        if let Some(pending) = self.pane_relocations.get(&layout.tab_id)
            && let Some(rect) = pending.display_fractions(pane_id, Some(layout), now, reduce_motion)
        {
            return Some(rect);
        }
        if let SurfaceDrag::Pane(drag) = &self.surface_drag
            && drag.tab_id == layout.tab_id
            && let Some(rect) = drag
                .layout_preview
                .as_ref()
                .and_then(|preview| preview.display_fractions(pane_id, now, reduce_motion))
        {
            return Some(rect);
        }
        if let Some(flight) = self
            .pane_drag_return
            .as_ref()
            .filter(|flight| flight.tab_id == layout.tab_id)
            && let Some(from) = flight
                .layout_from
                .iter()
                .find(|(id, _)| id == pane_id)
                .map(|(_, rect)| *rect)
            && let Some(to) = layout
                .panes
                .iter()
                .find(|pane| pane.pane_id == pane_id)
                .and_then(|pane| layout_rect_fractions(layout.area, pane.rect))
        {
            let progress = ochub_ui::anim::linear_progress(
                flight.started,
                PANE_DRAG_RETURN_ANIMATION,
                now,
                reduce_motion,
            );
            return Some(lerp_rect(
                from,
                to,
                ochub_ui::anim::ease_out_quint(progress),
            ));
        }
        if let Some(squeezed) = self.squeezed_tab_layout(layout)
            && let Some(rect) = squeezed.pane(pane_id)
        {
            return Some(rect);
        }
        squeezed_layout(layout, &[])
            .and_then(|resolved| resolved.pane(pane_id))
            .or_else(|| {
                let pane = layout.panes.iter().find(|pane| pane.pane_id == pane_id)?;
                layout_rect_fractions(layout.area, pane.rect)
            })
    }
}
