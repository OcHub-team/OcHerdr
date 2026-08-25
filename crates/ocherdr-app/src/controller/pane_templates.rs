use super::*;

impl OcHerdrView {
    pub(super) fn commit_pane_template(
        &mut self,
        workspace_id: &str,
        tab_id: &str,
        source_pane_id: &str,
        fingerprint: u64,
        placement: PaneTemplatePlacement,
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
        if layout.zoomed
            || layout_fingerprint(layout) != fingerprint
            || !(2..=4).contains(&layout.panes.len())
        {
            return false;
        }
        let Some(predicted) = pane_template_predicted_layout(layout, source_pane_id, placement)
        else {
            return false;
        };
        let Some((_anchor, steps)) = pane_template_construction(&predicted.tree.root) else {
            return false;
        };
        if steps.is_empty() {
            return false;
        }
        self.pane_relocation_serial = self.pane_relocation_serial.wrapping_add(1);
        let operation_id = self.pane_relocation_serial;
        let pending = PendingPaneTemplateCommit {
            operation_id,
            workspace_id: workspace_id.to_owned(),
            tab_id: tab_id.to_owned(),
            source_pane_id: source_pane_id.to_owned(),
            expected_root: predicted.tree.root,
            predicted_rects: predicted.panes,
            steps,
            parked: HashMap::new(),
            known_tab_ids: snapshot
                .tabs_for(workspace_id)
                .map(|tab| tab.tab_id.clone())
                .collect(),
            phase: PaneTemplateCommitPhase::Parking(0),
        };
        self.pane_template_commits
            .insert(tab_id.to_owned(), pending);
        self.send_template_park(tab_id, operation_id, cx);
        true
    }

    fn send_template_park(&mut self, tab_id: &str, operation_id: u64, cx: &mut Context<Self>) {
        let Some(pending) = self.pane_template_commits.get(tab_id) else {
            return;
        };
        let PaneTemplateCommitPhase::Parking(index) = pending.phase else {
            return;
        };
        let Some(step) = pending.steps.get(index) else {
            return;
        };
        let pane_id = step.pane_id.clone();
        let workspace_id = pending.workspace_id.clone();
        let tab_id = tab_id.to_owned();
        self.invoke_with_response(
            "pane.move",
            json!({
                "pane_id": pane_id,
                "destination": {
                    "type": "new_tab",
                    "workspace_id": workspace_id,
                },
                "focus": false,
            }),
            move |this, result, cx| {
                let parked = result
                    .ok()
                    .and_then(|value| parked_pane_from_response(&value));
                this.template_park_responded(&tab_id, operation_id, index, parked, cx);
            },
            cx,
        );
    }

    fn template_park_responded(
        &mut self,
        tab_id: &str,
        operation_id: u64,
        index: usize,
        parked: Option<ParkedPane>,
        cx: &mut Context<Self>,
    ) {
        let Some(parked) = parked else {
            self.pane_template_commits.remove(tab_id);
            cx.notify();
            return;
        };
        let Some(pending) = self.pane_template_commits.get_mut(tab_id) else {
            return;
        };
        if pending.operation_id != operation_id
            || pending.phase != PaneTemplateCommitPhase::Parking(index)
        {
            return;
        }
        let pane_id = pending.steps[index].pane_id.clone();
        pending.parked.insert(pane_id, parked);
        if index + 1 < pending.steps.len() {
            pending.phase = PaneTemplateCommitPhase::Parking(index + 1);
            self.send_template_park(tab_id, operation_id, cx);
        } else {
            pending.phase = PaneTemplateCommitPhase::Inserting(0);
            self.send_template_insert(tab_id, operation_id, cx);
        }
        cx.notify();
    }

    fn send_template_insert(&mut self, tab_id: &str, operation_id: u64, cx: &mut Context<Self>) {
        let Some(pending) = self.pane_template_commits.get(tab_id) else {
            return;
        };
        let PaneTemplateCommitPhase::Inserting(index) = pending.phase else {
            return;
        };
        let Some(step) = pending.steps.get(index).cloned() else {
            return;
        };
        let Some(parked) = pending.parked.get(&step.pane_id) else {
            return;
        };
        let moved_pane_id = parked.pane_id.clone();
        let target_tab_id = pending.tab_id.clone();
        let focus = step.pane_id == pending.source_pane_id;
        let tab_id = tab_id.to_owned();
        self.invoke_with_response(
            "pane.move",
            json!({
                "pane_id": moved_pane_id,
                "destination": {
                    "type": "tab",
                    "tab_id": target_tab_id,
                    "target_pane_id": step.target_pane_id,
                    "split": step.direction,
                    "ratio": step.ratio,
                },
                "focus": focus,
            }),
            move |this, result, cx| {
                let accepted = result.is_ok_and(|value| pane_move_changed(&value));
                this.template_insert_responded(&tab_id, operation_id, index, accepted, cx);
            },
            cx,
        );
    }

    fn template_insert_responded(
        &mut self,
        tab_id: &str,
        operation_id: u64,
        index: usize,
        accepted: bool,
        cx: &mut Context<Self>,
    ) {
        if !accepted {
            self.pane_template_commits.remove(tab_id);
            cx.notify();
            return;
        }
        let Some(pending) = self.pane_template_commits.get_mut(tab_id) else {
            return;
        };
        if pending.operation_id != operation_id
            || pending.phase != PaneTemplateCommitPhase::Inserting(index)
        {
            return;
        }
        if index + 1 < pending.steps.len() {
            pending.phase = PaneTemplateCommitPhase::Inserting(index + 1);
            self.send_template_insert(tab_id, operation_id, cx);
        } else {
            pending.phase = PaneTemplateCommitPhase::AwaitingLayout;
            self.reconcile_pane_template_commits(cx);
        }
        cx.notify();
    }

    pub(super) fn reconcile_pane_template_commits(&mut self, cx: &mut Context<Self>) {
        let finished = self
            .pane_template_commits
            .iter()
            .filter(|(_, pending)| pending.phase == PaneTemplateCommitPhase::AwaitingLayout)
            .filter_map(|(tab_id, pending)| {
                let layout = self.snapshot.as_ref()?.layout_for(tab_id)?;
                let tree = rebuild_tree(layout)?;
                pane_template_tree_matches(&pending.expected_root, &tree.root)
                    .then(|| tab_id.clone())
            })
            .collect::<Vec<_>>();
        for tab_id in finished {
            let source = self
                .pane_template_commits
                .remove(&tab_id)
                .map(|pending| pending.source_pane_id);
            if self.selection.tab_id.as_deref() == Some(&tab_id) {
                self.selection.pane_id = source;
                self.ensure_session_terminals(cx);
            }
            cx.notify();
        }
    }
}
