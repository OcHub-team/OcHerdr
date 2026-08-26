use super::*;

impl OcHerdrView {
    /// Prefix `m`: lift the selected pane. Arrows pick a neighbour, Tab
    /// cycles the zone, Enter commits, Esc cancels.
    pub(crate) fn enter_keyboard_pane_move(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::None)
            || !matches!(
                self.surface_drag,
                SurfaceDrag::Idle | SurfaceDrag::Text { .. }
            )
        {
            return;
        }
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let Some(pane) = snapshot.pane(&pane_id) else {
            return;
        };
        let (workspace_id, tab_id) = (pane.workspace_id.clone(), pane.tab_id.clone());
        if self.tab_relocation_locked(&tab_id) {
            return;
        }
        let Some(layout) = snapshot.layout_for(&tab_id) else {
            return;
        };
        if layout.zoomed || layout.panes.len() < 2 {
            return;
        }
        let fingerprint = layout_fingerprint(layout);
        self.end_text_drag();
        self.pane_keyboard_move = Some(KeyboardPaneMove {
            workspace_id,
            tab_id,
            pane_id,
            fingerprint,
            target: None,
            edge_drops: self.edge_drops_enabled(),
        });
        cx.notify();
    }

    pub(crate) fn cancel_keyboard_pane_move(&mut self) {
        self.pane_keyboard_move = None;
    }

    /// Snapshot changed: the mode survives only while the tab's structure is
    /// what it was on entry.
    pub(super) fn reconcile_keyboard_pane_move(&mut self, cx: &mut Context<Self>) {
        let Some(mode) = self.pane_keyboard_move.as_ref() else {
            return;
        };
        let survives = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.layout_for(&mode.tab_id))
            .is_some_and(|layout| layout_fingerprint(layout) == mode.fingerprint);
        if !survives {
            self.pane_keyboard_move = None;
            cx.notify();
        }
    }

    /// Returns `true` when the key belonged to the move mode.
    pub(crate) fn handle_keyboard_pane_move_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut mode) = self.pane_keyboard_move.take() else {
            return false;
        };
        let key = event.keystroke.key.as_str();
        let direction = match key {
            "left" => Some(DropEdge::Left),
            "right" => Some(DropEdge::Right),
            "up" => Some(DropEdge::Up),
            "down" => Some(DropEdge::Down),
            _ => None,
        };
        let handled = match (key, direction) {
            ("escape", _) => {
                cx.notify();
                return true;
            }
            ("enter", _) => {
                if let Some(hover) = mode.target.clone().filter(|_| mode.droppable()) {
                    let request = PaneDropRequest {
                        workspace_id: mode.workspace_id.clone(),
                        tab_id: mode.tab_id.clone(),
                        pane_id: mode.pane_id.clone(),
                        fingerprint: mode.fingerprint,
                        hover,
                        edge_drops: mode.edge_drops,
                    };
                    self.commit_pane_drop(&request, cx);
                }
                cx.notify();
                return true;
            }
            ("tab", _) => {
                if let Some(target) = mode.target.as_mut() {
                    target.zone = next_keyboard_zone(target.zone, mode.edge_drops);
                }
                true
            }
            (_, Some(direction)) => {
                let target = self.snapshot.as_ref().and_then(|snapshot| {
                    let layout = snapshot.layout_for(&mode.tab_id)?;
                    let target_pane_id = keyboard_neighbour(layout, &mode.pane_id, direction)?;
                    let target_rect = self
                        .terminal_surface_bounds
                        .and_then(|surface| pane_window_rect(layout, &target_pane_id, surface))
                        .unwrap_or((0., 0., 0., 0.));
                    Some(PaneDropHover {
                        target_pane_id,
                        zone: DropZone::Center,
                        target_rect,
                    })
                });
                if let Some(target) = target {
                    mode.target = Some(target);
                }
                true
            }
            _ => false,
        };
        self.pane_keyboard_move = Some(mode);
        if handled {
            cx.notify();
        }
        handled
    }

    /// Context-menu swap with the neighbouring pane (design §11).
    pub(crate) fn swap_pane_direction(
        &mut self,
        pane_id: String,
        direction: &'static str,
        cx: &mut Context<Self>,
    ) {
        if self
            .pane_tab_id(&pane_id)
            .is_some_and(|tab_id| self.tab_relocation_locked(&tab_id))
        {
            return;
        }
        self.invoke(
            "pane.swap",
            json!({ "pane_id": pane_id, "direction": direction }),
            cx,
        );
    }
}
