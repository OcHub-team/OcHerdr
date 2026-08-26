use super::*;

impl OcHerdrView {
    pub(crate) fn update_pane_drag_over_tab_target(
        &mut self,
        mouse: (f32, f32),
        target: PaneTabDropTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        self.update_pane_drag_with_hits(mouse, None, Some(target), cx)
    }

    pub(crate) fn clear_pane_tab_drop_target(
        &mut self,
        mouse: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        let SurfaceDrag::Pane(drag) = &self.surface_drag else {
            return false;
        };
        if drag.tab_target.is_none() {
            return false;
        }
        self.update_pane_drag_with_hits(mouse, None, None, cx)
    }

    /// Painted tab pill hit: same-workspace non-source tabs publish Existing
    /// when they are droppable. The source pill and undroppable tabs clear.
    pub(crate) fn update_pane_drag_over_tab_pill(
        &mut self,
        mouse: (f32, f32),
        tab_id: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let SurfaceDrag::Pane(drag) = &self.surface_drag else {
            return false;
        };
        if !drag.tab_bar_drops || !pane_drag_past_slop(drag) {
            return false;
        }
        let source_tab = drag.tab_id.clone();
        let workspace_id = drag.workspace_id.clone();
        if tab_id == source_tab {
            self.clear_pane_tab_drop_target(mouse, cx);
            return true;
        }
        match self.existing_tab_drop_target(&tab_id, &source_tab, &workspace_id) {
            Some(target) => self.update_pane_drag_over_tab_target(mouse, target, cx),
            None => {
                self.clear_pane_tab_drop_target(mouse, cx);
                true
            }
        }
    }

    pub(crate) fn existing_tab_drop_target(
        &self,
        tab_id: &str,
        source_tab_id: &str,
        workspace_id: &str,
    ) -> Option<PaneTabDropTarget> {
        if tab_id == source_tab_id || !self.pane_move_supported() {
            return None;
        }
        let snapshot = self.snapshot.as_ref()?;
        let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id)?;
        if tab.workspace_id != workspace_id {
            return None;
        }
        if self.tab_relocation_locked(tab_id) {
            return None;
        }
        let layout = snapshot.layout_for(tab_id)?;
        if layout.zoomed || layout.panes.is_empty() {
            return None;
        }
        let anchor = layout
            .panes
            .iter()
            .find(|pane| pane.pane_id == layout.focused_pane_id)
            .or_else(|| layout.panes.first())?
            .pane_id
            .clone();
        Some(PaneTabDropTarget::Existing {
            tab_id: tab_id.to_owned(),
            target_pane_id: anchor,
        })
    }

    pub(crate) fn pane_detach_for_tab(&self, tab_id: &str) -> Option<&PendingPaneDetach> {
        self.pane_detaches
            .values()
            .find(|pending| pending.locks_tab(tab_id))
    }

    pub(super) fn commit_pane_tab_drop(
        &mut self,
        workspace_id: &str,
        tab_id: &str,
        pane_id: &str,
        fingerprint: u64,
        destination: PaneTabDropTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.pane_move_supported() || self.tab_relocation_locked(tab_id) {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let Some(layout) = snapshot.layout_for(tab_id) else {
            return false;
        };
        if layout.zoomed || layout_fingerprint(layout) != fingerprint {
            return false;
        }
        if snapshot.pane(pane_id).is_none() {
            return false;
        }
        let predicted = predict_remove_pane(layout, pane_id);
        if predicted.is_none() && layout.panes.len() != 1 {
            return false;
        }
        let source = transfer_tab_state(
            tab_id,
            layout,
            predicted
                .as_ref()
                .map(|layout| layout.panes.clone())
                .unwrap_or_default(),
            predicted.as_ref().map(predicted_shape),
        );
        let (target, params) = match &destination {
            PaneTabDropTarget::NewTab => (
                None,
                json!({
                    "pane_id": pane_id,
                    "destination": {
                        "type": "new_tab",
                        "workspace_id": workspace_id,
                    },
                    "focus": true,
                }),
            ),
            PaneTabDropTarget::Existing {
                tab_id: target_tab_id,
                target_pane_id,
            } => {
                if self.tab_relocation_locked(target_tab_id) {
                    return false;
                }
                let Some(target_tab) = snapshot
                    .tabs
                    .iter()
                    .find(|tab| tab.tab_id == *target_tab_id)
                else {
                    return false;
                };
                if target_tab.workspace_id != workspace_id {
                    return false;
                }
                let Some(target_layout) = snapshot.layout_for(target_tab_id) else {
                    return false;
                };
                if target_layout.zoomed || target_layout.panes.is_empty() {
                    return false;
                }
                if !target_layout
                    .panes
                    .iter()
                    .any(|pane| pane.pane_id == *target_pane_id)
                {
                    return false;
                }
                let Some(inserted) = predict_insert_pane(target_layout, pane_id, target_pane_id)
                else {
                    return false;
                };
                let target = transfer_tab_state(
                    target_tab_id,
                    target_layout,
                    inserted.panes.clone(),
                    Some(predicted_shape(&inserted)),
                );
                (
                    Some(target),
                    json!({
                        "pane_id": pane_id,
                        "destination": {
                            "type": "tab",
                            "tab_id": target_tab_id,
                            "target_pane_id": target_pane_id,
                            "split": SplitDirection::Right,
                            "ratio": 0.5,
                        },
                        "focus": true,
                    }),
                )
            }
        };
        self.pane_relocation_serial = self.pane_relocation_serial.wrapping_add(1);
        let operation_id = self.pane_relocation_serial;
        let source_tab_id = source.tab_id.clone();
        self.pane_detaches.insert(
            source_tab_id.clone(),
            PendingPaneDetach {
                operation_id,
                workspace_id: workspace_id.to_owned(),
                source_pane_id: pane_id.to_owned(),
                destination,
                source,
                target,
                known_tab_ids: snapshot
                    .tabs_for(workspace_id)
                    .map(|tab| tab.tab_id.clone())
                    .collect(),
                responded: false,
                accepted: false,
                created_tab_id: None,
            },
        );
        self.invoke_with_response(
            "pane.move",
            params,
            move |this, result, cx| {
                this.on_pane_detach_response(&source_tab_id, operation_id, result, cx);
            },
            cx,
        );
        true
    }

    pub(super) fn on_pane_detach_response(
        &mut self,
        tab_id: &str,
        operation_id: u64,
        result: std::result::Result<Value, HerdrError>,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pane_detaches.get_mut(tab_id) else {
            return;
        };
        if pending.operation_id != operation_id {
            return;
        }
        if pending.responded {
            return;
        }
        pending.responded = true;
        let accepted = result.as_ref().ok().is_some_and(pane_move_changed);
        pending.accepted = accepted;
        if accepted && let Some(parked) = result.as_ref().ok().and_then(parked_pane_from_response) {
            pending.created_tab_id = Some(parked.temp_tab_id);
        }
        if !accepted {
            self.pane_detaches.remove(tab_id);
            cx.notify();
            return;
        }
        self.reconcile_pane_detaches(cx);
    }

    pub(super) fn reconcile_pane_detaches(&mut self, cx: &mut Context<Self>) {
        let tab_ids: Vec<String> = self.pane_detaches.keys().cloned().collect();
        for tab_id in tab_ids {
            let (drop_pending, created) =
                match (self.pane_detaches.get(&tab_id), self.snapshot.as_ref()) {
                    (None, _) => continue,
                    (_, None) => (true, None),
                    (Some(pending), Some(snapshot)) => (
                        detach_is_foreign(pending, snapshot),
                        pending.created_tab_from(snapshot),
                    ),
                };
            if drop_pending {
                self.pane_detaches.remove(&tab_id);
                cx.notify();
                continue;
            }
            if let Some(created) = created
                && let Some(pending) = self.pane_detaches.get_mut(&tab_id)
            {
                pending.created_tab_id = Some(created);
            }
            if self.pane_detach_ready(&tab_id) {
                self.pane_detaches.remove(&tab_id);
                self.ensure_session_terminals(cx);
                cx.notify();
            }
        }
    }

    fn pane_detach_ready(&self, tab_id: &str) -> bool {
        let Some(pending) = self.pane_detaches.get(tab_id) else {
            return false;
        };
        if !pending.responded || !pending.accepted {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let source_gone = snapshot
            .tabs
            .iter()
            .all(|tab| tab.tab_id != pending.source.tab_id);
        let source_matches = snapshot
            .layout_for(&pending.source.tab_id)
            .is_some_and(|layout| {
                split_layout_fingerprint(layout) == pending.source.predicted_topology
            });
        if pending.source.predicted_rects.is_empty() {
            if !source_gone {
                return false;
            }
        } else if !source_matches && !source_gone {
            return false;
        }
        match &pending.destination {
            PaneTabDropTarget::NewTab => snapshot
                .pane(&pending.source_pane_id)
                .is_some_and(|pane| !pending.known_tab_ids.contains(&pane.tab_id)),
            PaneTabDropTarget::Existing {
                tab_id: target_id, ..
            } => {
                let Some(target) = pending.target.as_ref() else {
                    return false;
                };
                let target_matches = snapshot.layout_for(target_id).is_some_and(|layout| {
                    split_layout_fingerprint(layout) == target.predicted_topology
                });
                target_matches
                    && snapshot
                        .pane(&pending.source_pane_id)
                        .is_some_and(|pane| pane.tab_id == *target_id)
            }
        }
    }
}

fn transfer_tab_state(
    tab_id: &str,
    layout: &PaneLayout,
    predicted_rects: Vec<PredictedPane>,
    predicted_topology: Option<SplitLayoutFingerprint>,
) -> PaneTransferTabState {
    PaneTransferTabState {
        tab_id: tab_id.to_owned(),
        fingerprint: layout_fingerprint(layout),
        topology: split_layout_fingerprint(layout),
        area: layout.area,
        predicted_topology: predicted_topology.unwrap_or(SplitLayoutFingerprint {
            zoomed: false,
            splits: Vec::new(),
            panes: Vec::new(),
        }),
        predicted_rects,
    }
}

fn layout_is_release_or_predicted(layout: &PaneLayout, state: &PaneTransferTabState) -> bool {
    let shape = split_layout_fingerprint(layout);
    shape == state.topology || shape == state.predicted_topology
}

fn detach_is_foreign(pending: &PendingPaneDetach, snapshot: &HierarchySnapshot) -> bool {
    let source_tab_gone = snapshot
        .tabs
        .iter()
        .all(|tab| tab.tab_id != pending.source.tab_id);
    let source_foreign = match snapshot.layout_for(&pending.source.tab_id) {
        Some(layout) => !layout_is_release_or_predicted(layout, &pending.source),
        None => !source_tab_gone,
    };
    if source_foreign {
        return true;
    }
    let Some(target) = pending.target.as_ref() else {
        return false;
    };
    match snapshot.layout_for(&target.tab_id) {
        Some(layout) => !layout_is_release_or_predicted(layout, target),
        // An Existing target had a layout at release. Its tab or layout
        // vanishing mid-flight is a foreign change, not a last-pane collapse.
        None => true,
    }
}
