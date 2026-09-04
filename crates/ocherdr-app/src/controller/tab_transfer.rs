use super::*;

impl OcHerdrView {
    pub(super) fn start_tab_transfer(
        &mut self,
        source_tab_id: String,
        source_workspace_id: String,
        target_workspace_id: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if source_workspace_id == target_workspace_id
            || self.pending_tab_transfer.is_some()
            || !self.pane_move_supported()
            || self.tab_relocation_locked(&source_tab_id)
        {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        if !snapshot
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == target_workspace_id)
        {
            return false;
        }
        let Some(tab) = snapshot.tabs.iter().find(|tab| tab.tab_id == source_tab_id) else {
            return false;
        };
        let Some(layout) = snapshot.layout_for(&source_tab_id) else {
            return false;
        };
        if layout.zoomed {
            self.notify_failure(
                FailureKind::MoveTab,
                self.i18n.text(k::TERMINAL_TAB_TRANSFER_ZOOMED),
                cx,
            );
            return false;
        }
        let Some(tree) = rebuild_tree(layout) else {
            self.notify_failure(
                FailureKind::MoveTab,
                self.i18n.text(k::TERMINAL_TAB_TRANSFER_INVALID_LAYOUT),
                cx,
            );
            return false;
        };
        let plan = tab_transfer_plan(&tree.root);
        let pane_terminal_ids = snapshot
            .panes_for(&source_tab_id)
            .map(|pane| (pane.pane_id.clone(), pane.terminal_id.clone()))
            .collect();
        let focused_pane_id = if tree.root.contains(&layout.focused_pane_id) {
            layout.focused_pane_id.clone()
        } else {
            plan.first_pane_id.clone()
        };
        self.tab_transfer_serial = self.tab_transfer_serial.wrapping_add(1);
        let operation_id = self.tab_transfer_serial;
        self.pending_tab_transfer = Some(PendingTabTransfer {
            operation_id,
            source_tab_id,
            target_workspace_id,
            tab_label: tab.label.clone(),
            focused_pane_id,
            plan,
            target_tab_id: None,
            pane_aliases: HashMap::new(),
            pane_terminal_ids,
            next_step: 0,
            request_in_flight: false,
            phase: TabTransferPhase::Moving,
        });
        self.send_next_tab_transfer_request(operation_id, cx);
        true
    }

    fn send_next_tab_transfer_request(&mut self, operation_id: u64, cx: &mut Context<Self>) {
        let Some(transfer) = self.pending_tab_transfer.as_ref() else {
            return;
        };
        if transfer.operation_id != operation_id
            || transfer.request_in_flight
            || transfer.phase != TabTransferPhase::Moving
        {
            return;
        }
        if !self.pane_move_supported() {
            self.fail_tab_transfer(operation_id, None, cx);
            return;
        }

        if transfer.target_tab_id.is_none() {
            if let Some((pane_id, tab_id)) = self.snapshot.as_ref().and_then(|snapshot| {
                let terminal_id = transfer
                    .pane_terminal_ids
                    .get(&transfer.plan.first_pane_id)?;
                snapshot
                    .panes
                    .iter()
                    .find(|pane| {
                        pane.terminal_id == *terminal_id
                            && pane.workspace_id == transfer.target_workspace_id
                    })
                    .map(|pane| (pane.pane_id.clone(), pane.tab_id.clone()))
            }) {
                let first_pane_id = transfer.plan.first_pane_id.clone();
                let transfer = self.pending_tab_transfer.as_mut().expect("checked above");
                transfer.pane_aliases.insert(first_pane_id, pane_id);
                transfer.target_tab_id = Some(tab_id);
                self.send_next_tab_transfer_request(operation_id, cx);
                return;
            }
            let pane_id = transfer.plan.first_pane_id.clone();
            let workspace_id = transfer.target_workspace_id.clone();
            let label = transfer.tab_label.clone();
            if let Some(transfer) = self.pending_tab_transfer.as_mut() {
                transfer.request_in_flight = true;
            }
            self.invoke_with_response(
                "pane.move",
                json!({
                    "pane_id": pane_id,
                    "destination": {
                        "type": "new_tab",
                        "workspace_id": workspace_id,
                        "label": label,
                    },
                    "focus": false,
                }),
                move |this, result, cx| {
                    this.on_tab_transfer_created(operation_id, result, cx);
                },
                cx,
            );
            return;
        }

        let Some(step) = transfer.plan.steps.get(transfer.next_step).cloned() else {
            self.finish_tab_transfer(operation_id, cx);
            return;
        };
        let target_tab_id = transfer.target_tab_id.clone().unwrap_or_default();
        if let Some(pane_id) = self.snapshot.as_ref().and_then(|snapshot| {
            let terminal_id = transfer.pane_terminal_ids.get(&step.pane_id)?;
            snapshot
                .panes_for(&target_tab_id)
                .find(|pane| pane.terminal_id == *terminal_id)
                .map(|pane| pane.pane_id.clone())
        }) {
            let transfer = self.pending_tab_transfer.as_mut().expect("checked above");
            transfer.pane_aliases.insert(step.pane_id, pane_id);
            transfer.next_step += 1;
            self.send_next_tab_transfer_request(operation_id, cx);
            return;
        }
        let pane_id = transfer
            .pane_aliases
            .get(&step.pane_id)
            .cloned()
            .unwrap_or_else(|| step.pane_id.clone());
        let anchor_pane_id = transfer
            .pane_aliases
            .get(&step.anchor_pane_id)
            .cloned()
            .unwrap_or_else(|| step.anchor_pane_id.clone());
        if let Some(transfer) = self.pending_tab_transfer.as_mut() {
            transfer.request_in_flight = true;
        }
        self.invoke_with_response(
            "pane.move",
            json!({
                "pane_id": pane_id,
                "destination": {
                    "type": "tab",
                    "tab_id": target_tab_id,
                    "target_pane_id": anchor_pane_id,
                    "split": step.split,
                    "ratio": step.ratio,
                },
                "focus": false,
            }),
            move |this, result, cx| {
                this.on_tab_transfer_inserted(operation_id, step, result, cx);
            },
            cx,
        );
    }

    fn on_tab_transfer_created(
        &mut self,
        operation_id: u64,
        result: std::result::Result<Value, HerdrError>,
        cx: &mut Context<Self>,
    ) {
        let Ok(result) = result else {
            self.fail_tab_transfer(operation_id, None, cx);
            return;
        };
        let Some(parked) = parked_pane_from_response(&result) else {
            self.fail_tab_transfer(
                operation_id,
                Some(self.i18n.text(k::TERMINAL_TAB_TRANSFER_INVALID_RESPONSE)),
                cx,
            );
            return;
        };
        let Some(transfer) = self
            .pending_tab_transfer
            .as_mut()
            .filter(|transfer| transfer.operation_id == operation_id)
        else {
            return;
        };
        transfer.request_in_flight = false;
        transfer
            .pane_aliases
            .insert(transfer.plan.first_pane_id.clone(), parked.pane_id);
        transfer.target_tab_id = Some(parked.temp_tab_id);
        self.send_next_tab_transfer_request(operation_id, cx);
    }

    fn on_tab_transfer_inserted(
        &mut self,
        operation_id: u64,
        step: TabTransferStep,
        result: std::result::Result<Value, HerdrError>,
        cx: &mut Context<Self>,
    ) {
        let Ok(result) = result else {
            self.fail_tab_transfer(operation_id, None, cx);
            return;
        };
        let result = pane_move_result(&result);
        let expected_tab_id = self
            .pending_tab_transfer
            .as_ref()
            .filter(|transfer| transfer.operation_id == operation_id)
            .and_then(|transfer| transfer.target_tab_id.as_deref());
        let pane = result.get("pane");
        let changed = result
            .get("changed")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let already_in_target = !changed
            && pane
                .and_then(|pane| pane.get("tab_id"))
                .and_then(Value::as_str)
                == expected_tab_id;
        let moved_pane_id = (changed || already_in_target)
            .then(|| pane?.get("pane_id")?.as_str())
            .flatten()
            .map(str::to_owned);
        let Some(moved_pane_id) = moved_pane_id else {
            self.fail_tab_transfer(
                operation_id,
                Some(self.i18n.text(k::TERMINAL_TAB_TRANSFER_INVALID_RESPONSE)),
                cx,
            );
            return;
        };
        let Some(transfer) = self
            .pending_tab_transfer
            .as_mut()
            .filter(|transfer| transfer.operation_id == operation_id)
        else {
            return;
        };
        transfer.request_in_flight = false;
        transfer.pane_aliases.insert(step.pane_id, moved_pane_id);
        transfer.next_step += 1;
        self.send_next_tab_transfer_request(operation_id, cx);
    }

    fn finish_tab_transfer(&mut self, operation_id: u64, cx: &mut Context<Self>) {
        let Some(transfer) = self
            .pending_tab_transfer
            .as_ref()
            .filter(|transfer| transfer.operation_id == operation_id)
        else {
            return;
        };
        let Some(target_tab_id) = transfer.target_tab_id.clone() else {
            self.fail_tab_transfer(operation_id, None, cx);
            return;
        };
        let focused_pane_id = transfer
            .pane_aliases
            .get(&transfer.focused_pane_id)
            .cloned()
            .unwrap_or_else(|| transfer.focused_pane_id.clone());
        self.pending_tab_transfer = None;
        self.pending_created_tab = Some(target_tab_id);
        self.settle_pending_created_tab(cx);
        self.invoke("pane.focus", json!({ "pane_id": focused_pane_id }), cx);
        cx.notify();
    }

    fn fail_tab_transfer(
        &mut self,
        operation_id: u64,
        detail: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(transfer) = self
            .pending_tab_transfer
            .as_mut()
            .filter(|transfer| transfer.operation_id == operation_id)
        else {
            return;
        };
        transfer.request_in_flight = false;
        transfer.phase = TabTransferPhase::Failed;
        if let Some(detail) = detail {
            self.notify_failure(FailureKind::MoveTab, detail, cx);
        }
        cx.notify();
    }

    pub(crate) fn retry_tab_transfer(&mut self, cx: &mut Context<Self>) {
        let Some(transfer) = self.pending_tab_transfer.as_mut() else {
            return;
        };
        if transfer.phase != TabTransferPhase::Failed || transfer.request_in_flight {
            return;
        }
        transfer.phase = TabTransferPhase::Moving;
        let operation_id = transfer.operation_id;
        self.send_next_tab_transfer_request(operation_id, cx);
    }

    pub(crate) fn go_to_tab_transfer_target(&mut self, cx: &mut Context<Self>) {
        let Some(tab_id) = self
            .pending_tab_transfer
            .as_ref()
            .and_then(|transfer| transfer.target_tab_id.clone())
        else {
            return;
        };
        self.pending_created_tab = Some(tab_id);
        self.settle_pending_created_tab(cx);
    }

    pub(super) fn abort_tab_transfer_for_disconnect(&mut self, cx: &mut Context<Self>) {
        let Some(transfer) = self.pending_tab_transfer.as_mut() else {
            return;
        };
        transfer.request_in_flight = false;
        transfer.phase = TabTransferPhase::Failed;
        cx.notify();
    }
}
