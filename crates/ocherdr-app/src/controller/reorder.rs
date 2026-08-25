use super::*;

impl OcHerdrView {
    pub(crate) fn press_workspace_row(
        &mut self,
        workspace_id: String,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let order = snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.clone())
            .collect::<Vec<_>>();
        let Some(source_index) = order.iter().position(|id| id == &workspace_id) else {
            return;
        };
        if order.len() < 2 {
            self.select_workspace(workspace_id, cx);
            return;
        }
        self.begin_reorder(ReorderList::Workspaces, source_index, order, event, cx);
    }

    pub(crate) fn press_tab_pill(
        &mut self,
        tab_id: String,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(workspace_id) = snapshot
            .tabs
            .iter()
            .find(|tab| tab.tab_id == tab_id)
            .map(|tab| tab.workspace_id.clone())
        else {
            return;
        };
        let order = snapshot
            .tabs_for(&workspace_id)
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        let Some(source_index) = order.iter().position(|id| id == &tab_id) else {
            return;
        };
        if order.len() < 2 {
            self.select_tab(tab_id, cx);
            return;
        }
        self.begin_reorder(
            ReorderList::Tabs { workspace_id },
            source_index,
            order,
            event,
            cx,
        );
    }

    pub(super) fn begin_reorder(
        &mut self,
        list: ReorderList,
        source_index: usize,
        order: Vec<String>,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let source_id = order[source_index].clone();
        // Herdr owns the order. While it is publishing a move, a new drag would
        // compute its index from a list that is about to be replaced.
        if self.pending_reorder.is_some() {
            self.select_reorder_source(&list, source_id, cx);
            return;
        }
        // A drag needs the row it grabbed. Without a measured rect there is no
        // grab offset and no hover, and inventing one puts the ghost somewhere
        // the pointer never was.
        let Some(rect) = self.span_for(&list, &source_id) else {
            self.select_reorder_source(&list, source_id, cx);
            return;
        };
        let pointer = mouse_point(event.position);
        let grab_offset = (pointer.0 - rect.0, pointer.1 - rect.1);
        self.end_text_drag();
        self.cancel_split_drag();
        let hover = ReorderHover::Item {
            index: source_index,
            trailing: false,
        };
        self.surface_drag = SurfaceDrag::Reorder(ReorderDrag {
            list,
            source_index,
            order,
            previous_hover: hover,
            hover,
            origin: pointer,
            pointer,
            grab_offset,
            source_rect: rect,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn move_selected_workspace(&mut self, delta: isize, cx: &mut Context<Self>) {
        // Same reason as `begin_reorder`: the index would come from a list
        // Herdr is about to replace.
        if self.pending_reorder.is_some() {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let ids = snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.clone())
            .collect::<Vec<_>>();
        let Some(source) = self
            .selection
            .workspace_id
            .as_ref()
            .and_then(|id| ids.iter().position(|candidate| candidate == id))
        else {
            return;
        };
        let hover = if delta < 0 {
            if source == 0 {
                return;
            }
            ReorderHover::Item {
                index: source - 1,
                trailing: false,
            }
        } else {
            let next = source + 1;
            if next >= ids.len() {
                return;
            }
            ReorderHover::Item {
                index: next,
                trailing: true,
            }
        };
        let Some(insert_index) = reorder_insert_index(ids.len(), source, hover) else {
            return;
        };
        let source_id = ids[source].clone();
        self.submit_reorder(&ReorderList::Workspaces, source_id, insert_index, None, cx);
    }

    /// The only path that asks Herdr to change an order. Holding the request in
    /// `pending_reorder` is what stops a second reorder from being computed
    /// against the list this one is replacing.
    pub(super) fn submit_reorder(
        &mut self,
        list: &ReorderList,
        id: String,
        insert_index: usize,
        settling: Option<PendingListReorder>,
        cx: &mut Context<Self>,
    ) {
        let (method, params) = match list {
            ReorderList::Workspaces => (
                "workspace.move",
                json!({ "workspace_id": id, "insert_index": insert_index }),
            ),
            ReorderList::Tabs { .. } => (
                "tab.move",
                json!({ "tab_id": id, "insert_index": insert_index }),
            ),
        };
        if let Some(request) = self.spawn_invoke(method, params, cx) {
            self.pending_reorder = Some(PendingReorder {
                _request: request,
                display: settling,
            });
        }
    }

    pub(super) fn cancel_reorder_drag(&mut self) {
        if matches!(self.surface_drag, SurfaceDrag::Reorder(_)) {
            self.surface_drag = SurfaceDrag::Idle;
        }
    }

    pub(super) fn take_reorder_drag(&mut self) -> Option<ReorderDrag> {
        match std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle) {
            SurfaceDrag::Reorder(drag) => Some(drag),
            other => {
                self.surface_drag = other;
                None
            }
        }
    }

    pub(super) fn reconcile_reorder_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.take_reorder_drag() else {
            return;
        };
        if let SurfaceDrag::Reorder(drag) =
            reconcile_reorder_drag_state(drag, self.snapshot.as_ref())
        {
            self.surface_drag = SurfaceDrag::Reorder(drag);
        } else {
            cx.notify();
        }
    }

    pub(super) fn update_reorder_drag(
        &mut self,
        mouse: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut drag) = self.take_reorder_drag() else {
            return false;
        };
        drag.pointer = mouse;
        // Rows left the layout mid-drag. Keeping the last hover would aim the
        // drop at a position that is no longer on screen.
        let Some(hover) = self.reorder_hover_for(&drag) else {
            cx.notify();
            return true;
        };
        if drag.hover != hover {
            drag.previous_hover = drag.hover;
            drag.hover = hover;
        }
        self.surface_drag = SurfaceDrag::Reorder(drag);
        cx.notify();
        true
    }

    pub(super) fn finish_reorder_drag(
        &mut self,
        mouse: (f32, f32),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut drag) = self.take_reorder_drag() else {
            return false;
        };
        drag.pointer = mouse;
        let source_id = drag.order[drag.source_index].clone();
        let list = drag.list.clone();
        let Some(hover) = self.reorder_hover_for(&drag) else {
            self.select_reorder_source(&list, source_id, cx);
            return true;
        };
        drag.hover = hover;
        if reorder_past_slop(&drag) {
            let SurfaceDrag::Reorder(drag) =
                reconcile_reorder_drag_state(drag, self.snapshot.as_ref())
            else {
                self.select_reorder_source(&list, source_id, cx);
                return true;
            };
            if let Some(insert_index) =
                reorder_insert_index(drag.order.len(), drag.source_index, drag.hover)
            {
                let settling = self.pending_display_for(&drag);
                self.submit_reorder(&list, source_id.clone(), insert_index, settling, cx);
            }
        }
        self.select_reorder_source(&list, source_id, cx);
        true
    }

    pub(super) fn select_reorder_source(
        &mut self,
        list: &ReorderList,
        source_id: String,
        cx: &mut Context<Self>,
    ) {
        match list {
            ReorderList::Workspaces => self.select_workspace(source_id, cx),
            ReorderList::Tabs { .. } => self.select_tab(source_id, cx),
        }
    }

    pub(super) fn reorder_hover_for(&self, drag: &ReorderDrag) -> Option<ReorderHover> {
        let spans = self.spans_along_axis(&drag.list, &drag.order)?;
        let pointer = match drag.list {
            ReorderList::Workspaces => drag.pointer.1,
            ReorderList::Tabs { .. } => drag.pointer.0,
        };
        Some(reorder_hover_along_axis(&spans, pointer))
    }

    pub(super) fn pending_display_for(&self, drag: &ReorderDrag) -> Option<PendingListReorder> {
        let rects = drag
            .order
            .iter()
            .map(|id| self.span_for(&drag.list, id))
            .collect::<Option<Vec<_>>>()?;
        Some(PendingListReorder {
            list: drag.list.clone(),
            order: drag.order.clone(),
            source_index: drag.source_index,
            hover: drag.hover,
            released_origin: reorder_ghost_origin(
                drag.pointer,
                drag.grab_offset,
                reorder_list_bounds(&rects),
                (drag.source_rect.2, drag.source_rect.3),
                reorder_axis(&drag.list),
            ),
        })
    }

    pub(super) fn spans_along_axis(
        &self,
        list: &ReorderList,
        order: &[String],
    ) -> Option<Vec<(f32, f32)>> {
        let mut spans = Vec::with_capacity(order.len());
        for id in order {
            let rect = self.span_for(list, id)?;
            spans.push(match list {
                ReorderList::Workspaces => (rect.1, rect.3),
                ReorderList::Tabs { .. } => (rect.0, rect.2),
            });
        }
        Some(spans)
    }

    pub(super) fn span_for(&self, list: &ReorderList, id: &str) -> Option<(f32, f32, f32, f32)> {
        let spans = match list {
            ReorderList::Workspaces => &self.reorder_metrics.workspaces,
            ReorderList::Tabs { .. } => &self.reorder_metrics.tabs,
        };
        spans
            .iter()
            .find(|span| span.id == id)
            .map(|span| span.rect)
    }

    pub(crate) fn note_reorder_span(&mut self, tabs: bool, id: String, rect: (f32, f32, f32, f32)) {
        let spans = if tabs {
            &mut self.reorder_metrics.tabs
        } else {
            &mut self.reorder_metrics.workspaces
        };
        if let Some(existing) = spans.iter_mut().find(|span| span.id == id) {
            existing.rect = rect;
        } else {
            spans.push(ReorderSpan { id, rect });
        }
    }
}
