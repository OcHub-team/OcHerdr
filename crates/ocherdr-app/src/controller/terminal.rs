use super::*;

impl OcHerdrView {
    pub(crate) fn ensure_session_terminals(&mut self, cx: &mut Context<Self>) {
        let Some(session_name) = self.current_session().map(|session| session.name.clone()) else {
            self.stop_session_terminals();
            return;
        };
        if self.snapshot.is_none() {
            self.stop_session_terminals();
            return;
        }
        let profile = self.current_profile();
        let visible_tab_id = self.selection.tab_id.clone();
        let selected_pane_id = self.selection.pane_id.clone();
        let snapshot = self.snapshot.as_ref().expect("snapshot checked above");
        let live_pane_ids = snapshot_pane_ids(snapshot);
        let pane_tabs = snapshot
            .panes
            .iter()
            .map(|pane| (pane.pane_id.clone(), pane.tab_id.clone()))
            .collect::<HashMap<_, _>>();
        let incoming = SessionKey {
            profile_id: profile.id().to_owned(),
            session_name: session_name.clone(),
        };
        if session_panes_plan(
            self.session_panes.as_ref().map(|session| &session.owner),
            &incoming,
        ) == SessionPanesPlan::Replace
        {
            self.pane_resize_serial = self.pane_resize_serial.wrapping_add(1);
            self.session_panes = Some(SessionPanes::new(incoming));
        }
        let palette = current_terminal_palette(&self.appearance);
        let color_scheme_dark = palette.dark;
        let mut palette_error = None;
        let mut spawn_error = None;
        let mut spawned = HashSet::new();
        let mut pending_listens = Vec::new();
        {
            let visible_pane_ids = visible_pane_ids(Some(snapshot), visible_tab_id.as_deref());
            let controls = {
                let session = self
                    .session_panes
                    .as_mut()
                    .expect("live session adopted panes");
                session
                    .controls
                    .retain(|pane_id, _| visible_pane_ids.contains(pane_id));
                session.controls.clone()
            };
            #[cfg_attr(not(test), allow(unused_mut))]
            let mut wanted = snapshot_runtime_targets(
                snapshot,
                &controls,
                visible_tab_id.as_deref(),
                selected_pane_id.as_deref(),
            );
            #[cfg(test)]
            if self.headless_terminals {
                wanted.clear();
            }
            let panes = &mut self
                .session_panes
                .as_mut()
                .expect("live session adopted panes")
                .panes;
            panes.retain(|pane_id, _| live_pane_ids.contains(pane_id));
            for target in &wanted {
                let pane_id = &target.pane_id;
                let mode = target.mode;
                match visible_pane_plan(
                    panes.get(pane_id).map(|runtime| runtime.mode),
                    panes
                        .get(pane_id)
                        .is_some_and(|runtime| runtime.session.is_closed() || runtime.exit_seen),
                    mode,
                ) {
                    VisiblePanePlan::Keep
                    | VisiblePanePlan::PromoteToControl
                    | VisiblePanePlan::DemoteToObserve => {
                        if let Some(runtime) = panes.get_mut(pane_id) {
                            if runtime.palette_signature != palette.signature() {
                                if let Err(error) = runtime.terminal.apply_palette(&palette) {
                                    palette_error = Some(error);
                                }
                                runtime.color_scheme_dark = palette.dark;
                                runtime.palette_signature = palette.signature();
                            }
                            if let Some(frames) = sync_pane_session(
                                runtime,
                                mode,
                                target.focused,
                                profile.clone(),
                                session_name.clone(),
                                pane_id.clone(),
                            ) {
                                pending_listens.push((pane_id.clone(), frames));
                            }
                        }
                    }
                    VisiblePanePlan::Spawn => {
                        let cols = 80;
                        let rows = 24;
                        let (session, frames) = TerminalSession::spawn(
                            profile.clone(),
                            session_name.clone(),
                            pane_id.clone(),
                            mode,
                            cols,
                            rows,
                        );
                        match Terminal::new(cols, rows, 10_000, &palette) {
                            Ok(terminal) => {
                                terminal.set_focus(target.focused);
                                panes.insert(
                                    pane_id.clone(),
                                    PaneRuntime {
                                        session,
                                        terminal,
                                        frame: None,
                                        mode,
                                        focused: target.focused,
                                        size: (cols, rows),
                                        pixel_size: (0, 0),
                                        viewport_ready: false,
                                        frame_context: 0,
                                        color_scheme_dark,
                                        palette_signature: palette.signature(),
                                        listen: None,
                                        exit_seen: false,
                                        scroll_px: 0.,
                                        body_bounds: (0., 0., 0., 0.),
                                        pending_resize: None,
                                    },
                                );
                                spawned.insert(pane_id.clone());
                                pending_listens.push((pane_id.clone(), frames));
                            }
                            Err(error) => spawn_error = Some(error.to_string()),
                        }
                    }
                }
            }
            for target in &wanted {
                let pane_id = &target.pane_id;
                let pane_tab = pane_tabs.get(pane_id).map(String::as_str);
                if !should_flush_session_pane(
                    pane_tab,
                    visible_tab_id.as_deref(),
                    spawned.contains(pane_id),
                ) {
                    continue;
                }
                if let Some(runtime) = panes.get_mut(pane_id) {
                    flush_pane_surface(runtime);
                }
            }
        }
        if let Some(error) = palette_error {
            self.notify_failure(FailureKind::ApplyPalette, error, cx);
        }
        if let Some(error) = spawn_error {
            self.notify_failure(FailureKind::SpawnTerminal, error, cx);
        }
        for (pane_id, frames) in pending_listens {
            let task = Self::listen_pane(pane_id.clone(), frames, cx);
            if let Some(runtime) = self.pane_mut(&pane_id) {
                runtime.listen = Some(task);
            }
        }
    }

    pub(super) fn stop_session_terminals(&mut self) {
        self.pane_resize_serial = self.pane_resize_serial.wrapping_add(1);
        self.session_panes = None;
    }

    /// A direct terminal interaction is an explicit request to take over this
    /// pane without releasing other controlled panes. The request is
    /// deliberately user-driven, never a reconnect retry, so clients cannot
    /// get into a takeover loop.
    pub(crate) fn take_terminal_control(&mut self, pane_id: String, cx: &mut Context<Self>) {
        let Some(session) = self.session_panes.as_mut() else {
            return;
        };
        if session.controls.get(&pane_id) == Some(&TerminalMode::ControlTakeover) {
            return;
        }
        session
            .controls
            .insert(pane_id, TerminalMode::ControlTakeover);
        self.ensure_session_terminals(cx);
        cx.notify();
    }

    pub(super) fn demote_lost_terminal_control(
        &mut self,
        pane_id: &str,
        loss: TerminalControlLoss,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.session_panes.as_mut() else {
            return false;
        };
        if !demote_terminal_control(session, pane_id) {
            return false;
        }
        let (kind, detail) = match loss {
            TerminalControlLoss::Busy => (
                FailureKind::TerminalControlBusy,
                self.i18n.text(k::NOTIFY_DETAIL_TERMINAL_CONTROL_BUSY),
            ),
            TerminalControlLoss::TakenOver => (
                FailureKind::TerminalControlTakenOver,
                self.i18n.text(k::NOTIFY_DETAIL_TERMINAL_CONTROL_TAKEN_OVER),
            ),
        };
        self.notify_failure(kind, detail, cx);
        true
    }

    pub(super) fn listen_pane(
        pane_id: String,
        mut frames: UnboundedReceiver<std::result::Result<TerminalEvent, HerdrError>>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let herdr = next_batch(&mut frames);
                let ghostty = poll_fn(|task_cx| {
                    this.update(cx, |this, _| {
                        let Some(runtime) = this.pane_mut(&pane_id) else {
                            return Poll::Ready(None);
                        };
                        runtime.terminal.poll_frame(task_cx)
                    })
                    .unwrap_or(Poll::Ready(None))
                });
                pin_mut!(herdr, ghostty);
                match future::select(herdr, ghostty).await {
                    Either::Left((batch, _)) => {
                        let keep = this
                            .update(cx, |this, cx| this.apply_herdr_frames(&pane_id, batch, cx))
                            .unwrap_or(false);
                        if !keep {
                            break;
                        }
                    }
                    Either::Right((frame, _)) => {
                        let keep = this
                            .update(cx, |this, cx| this.apply_ghostty_frame(&pane_id, frame, cx))
                            .unwrap_or(false);
                        if !keep {
                            break;
                        }
                    }
                }
            }
        })
    }

    pub(super) fn apply_herdr_frames(
        &mut self,
        pane_id: &str,
        batch: Option<Vec<std::result::Result<TerminalEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let composing = self.ime_marked.clone();
        let selected_pane = self.selection.pane_id.clone();
        let visible_pane_ids =
            visible_pane_ids(self.snapshot.as_ref(), self.selection.tab_id.as_deref());
        let mut error = None;
        let mut hierarchy_changed = false;
        let mut control_loss = None;
        let mut changed = false;
        let keep = {
            let Some(runtime) = self.pane_mut(pane_id) else {
                return false;
            };
            match batch {
                None => {
                    runtime.exit_seen = true;
                    hierarchy_changed = true;
                    false
                }
                Some(items) => {
                    let mut closed = false;
                    for item in items {
                        match item {
                            Ok(TerminalEvent::Frame(frame)) => {
                                let frame_size = (frame.width, frame.height);
                                if !incoming_frame_should_apply(
                                    runtime.viewport_ready,
                                    runtime.size,
                                    frame_size,
                                ) {
                                    // A stream reconnected at its bootstrap size can deliver a
                                    // full frame before Herdr applies this pane's measured
                                    // viewport. Before the initial measurement, or while waiting
                                    // for a new viewport-sized frame, keep the last compatible
                                    // frame instead of painting a stale grid into the upper-left
                                    // corner.
                                    continue;
                                }
                                runtime.terminal.apply_frame(&frame.bytes, frame.full);
                                if selected_pane.as_deref() == Some(pane_id)
                                    && let Some(preedit) = composing.as_deref()
                                {
                                    runtime.terminal.set_preedit(Some(preedit));
                                }
                            }
                            Ok(TerminalEvent::MouseCapture {
                                enabled,
                                sgr_pixels,
                            }) => runtime.terminal.set_mouse_capture(enabled, sgr_pixels),
                            Err(stream_error) => {
                                runtime.exit_seen = true;
                                control_loss = runtime
                                    .mode
                                    .is_controlled()
                                    .then(|| terminal_control_loss(&stream_error))
                                    .flatten();
                                hierarchy_changed = control_loss.is_none();
                                closed = true;
                                if !is_expected_terminal_exit(&stream_error)
                                    && control_loss.is_none()
                                {
                                    error = Some((
                                        FailureKind::TerminalStream,
                                        stream_error.to_string(),
                                    ));
                                }
                                break;
                            }
                        }
                    }
                    if let Err(runtime_error) = Terminal::tick_runtime() {
                        error = Some((FailureKind::TerminalRuntime, runtime_error.to_string()));
                    }
                    if forward_terminal_input(runtime).is_err() {
                        runtime.exit_seen = true;
                        hierarchy_changed = true;
                        closed = true;
                    }
                    match runtime.terminal.try_frame() {
                        Ok(Some(frame)) if frame.host_context == runtime.frame_context => {
                            runtime.frame = Some(frame);
                            if visible_pane_ids.contains(pane_id) {
                                changed = true;
                            }
                        }
                        Ok(Some(_)) | Ok(None) => {}
                        Err(frame_error) => {
                            error = Some((FailureKind::RenderTerminal, frame_error.to_string()))
                        }
                    }
                    !closed
                }
            }
        };
        if let Some((kind, detail)) = error {
            self.notify_failure(kind, detail, cx);
        }
        if let Some(loss) = control_loss
            && self.demote_lost_terminal_control(pane_id, loss, cx)
        {
            self.ensure_session_terminals(cx);
            cx.notify();
        }
        if hierarchy_changed {
            self.resync_snapshot(self.event_epoch, cx);
        }
        if changed {
            cx.notify();
        }
        keep
    }

    pub(super) fn apply_ghostty_frame(
        &mut self,
        pane_id: &str,
        frame: Option<std::result::Result<RenderedFrame, ocherdr_terminal::TerminalError>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let visible_pane_ids =
            visible_pane_ids(self.snapshot.as_ref(), self.selection.tab_id.as_deref());
        let mut error = None;
        let mut changed = false;
        let mut hierarchy_changed = false;
        let keep = {
            let Some(runtime) = self.pane_mut(pane_id) else {
                return false;
            };
            let Some(frame) = frame else {
                return false;
            };
            match frame {
                Ok(frame) if frame.host_context == runtime.frame_context => {
                    runtime.frame = Some(frame);
                    changed = visible_pane_ids.contains(pane_id);
                }
                Ok(_) => {}
                Err(frame_error) => {
                    error = Some((FailureKind::RenderTerminal, frame_error.to_string()))
                }
            }
            if forward_terminal_input(runtime).is_err() {
                runtime.exit_seen = true;
                hierarchy_changed = true;
                false
            } else {
                true
            }
        };
        if let Some((kind, detail)) = error {
            self.notify_failure(kind, detail, cx);
        }
        if hierarchy_changed {
            self.resync_snapshot(self.event_epoch, cx);
        }
        if changed {
            cx.notify();
        }
        keep
    }

    /// Terminal grids stay put while the tab's geometry is only a preview
    /// (design §5.4, §7.2): a pending relocation plan. They resize once,
    /// when the authoritative layout is on screen.
    pub(crate) fn pane_resize_frozen(&self, pane_id: &str) -> bool {
        self.pane_tab_id(pane_id)
            .is_some_and(|tab_id| self.tab_resize_frozen(&tab_id))
            || self.pane_relocations.values().any(|pending| {
                pending.phase.locks_tab()
                    && pending.plan.predicted_pane_ids().any(|id| id == pane_id)
            })
            || self
                .pane_template_commits
                .values()
                .any(|pending| pending.predicted_pane_ids().any(|id| id == pane_id))
    }

    pub(super) fn tab_resize_frozen(&self, tab_id: &str) -> bool {
        self.tab_relocation_locked(tab_id)
            || self.tab_split_dragging(tab_id)
            || matches!(
                &self.surface_drag,
                SurfaceDrag::Pane(drag)
                    if drag.tab_id == tab_id && drag.layout_preview.is_some()
            )
            || self
                .pane_drag_return
                .as_ref()
                .is_some_and(|flight| flight.tab_id == tab_id)
    }

    /// Called once per visible render. Returns true exactly on the first frame
    /// after a previously observed geometry freeze ends.
    pub(crate) fn tab_resize_just_thawed(&mut self, tab_id: &str) -> bool {
        if self.tab_resize_frozen(tab_id) {
            self.pane_resize_frozen_tabs.insert(tab_id.to_owned());
            false
        } else {
            self.pane_resize_frozen_tabs.remove(tab_id)
        }
    }

    /// A divider of this tab is being dragged, or its release batch of
    /// `layout.set_split_ratio` is still landing: geometry is the squeeze
    /// preview (design §5.4) until the authoritative layout carries every
    /// ratio of the batch.
    pub(crate) fn tab_split_dragging(&self, tab_id: &str) -> bool {
        matches!(&self.surface_drag, SurfaceDrag::Split(drag) if drag.tab_id == tab_id)
            || self
                .split_commit
                .as_ref()
                .is_some_and(|commit| commit.tab_id == tab_id)
    }

    /// The tab's geometry squeezed to the split drag's preview ratios (the
    /// dragged split plus the nested dividers pinned to their cells), or
    /// `None` when no divider of this tab is being dragged or committed.
    pub(crate) fn squeezed_tab_layout(&self, layout: &PaneLayout) -> Option<SqueezedLayout> {
        if let SurfaceDrag::Split(drag) = &self.surface_drag
            && drag.tab_id == layout.tab_id
        {
            let ratios = split_drag_ratios(layout, &drag.path, drag.preview_ratio);
            return squeezed_layout(layout, &ratios);
        }
        let commit = self.split_commit.as_ref()?;
        if commit.tab_id != layout.tab_id {
            return None;
        }
        squeezed_layout(layout, &commit.ratios)
    }

    pub(crate) fn sync_measured_pane_body(
        &mut self,
        pane_id: &str,
        bounds: Bounds<ochub_ui::gpui::Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let body = (
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        );
        let scale = window.scale_factor();
        let width_px = (body.2 * scale).round() as u32;
        let height_px = (body.3 * scale).round() as u32;
        let scale_factor = f64::from(scale);
        let palette = current_terminal_palette(&self.appearance);
        let mut palette_error = None;
        let frozen = self.pane_resize_frozen(pane_id);
        let next_serial = self.pane_resize_serial.wrapping_add(1);
        let mut scheduled = None;
        {
            let Some(runtime) = self.pane_mut(pane_id) else {
                return;
            };
            runtime.body_bounds = body;
            if runtime.palette_signature != palette.signature() {
                if let Err(error) = runtime.terminal.apply_palette(&palette) {
                    palette_error = Some(error);
                }
                runtime.color_scheme_dark = palette.dark;
                runtime.palette_signature = palette.signature();
            }
            match pane_resize_schedule(
                frozen,
                runtime.pixel_size,
                runtime.pending_resize,
                (width_px, height_px),
                scale_factor,
            ) {
                PaneResizeSchedule::Cancel => {
                    // Invalidate a timer queued by an older measurement. A
                    // thaw will schedule the cached final bounds again.
                    runtime.pending_resize = None;
                }
                PaneResizeSchedule::Keep => {}
                PaneResizeSchedule::Replace => {
                    let pending = PendingPaneResize {
                        serial: next_serial,
                        pixels: (width_px, height_px),
                        scale_factor,
                    };
                    runtime.pending_resize = Some(pending);
                    scheduled = Some(pending);
                }
            }
        }
        if let Some(error) = palette_error {
            self.notify_failure(FailureKind::ApplyPalette, error, cx);
        }
        let Some(pending) = scheduled else {
            return;
        };
        self.pane_resize_serial = pending.serial;
        let pane_id = pane_id.to_owned();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(PANE_RESIZE_SETTLE_DELAY)
                .await;
            this.update(cx, |this, cx| {
                this.commit_pending_pane_resize(&pane_id, pending.serial, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn commit_pending_pane_resize(
        &mut self,
        pane_id: &str,
        serial: u64,
        cx: &mut Context<Self>,
    ) {
        let visible = self.pane_tab_id(pane_id).as_deref() == self.selection.tab_id.as_deref();
        let frozen = self.pane_resize_frozen(pane_id);
        let observer_reconnect = self
            .current_session()
            .map(|session| (self.current_profile(), session.name.clone()));
        let mut replacement_frames = None;
        {
            let Some(runtime) = self.pane_mut(pane_id) else {
                return;
            };
            let Some(pending) = runtime
                .pending_resize
                .filter(|pending| pending.serial == serial)
            else {
                return;
            };
            runtime.pending_resize = None;
            if !visible || frozen || runtime.pixel_size == pending.pixels {
                return;
            }
            runtime.frame_context = runtime.frame_context.wrapping_add(1);
            let resolved = runtime.terminal.resize_pixels(
                pending.pixels.0,
                pending.pixels.1,
                pending.scale_factor,
                runtime.frame_context,
            );
            let size = (resolved.columns, resolved.rows);
            let restart_observer = observer_session_needs_measured_restart(
                runtime.viewport_ready,
                runtime.mode,
                runtime.size,
                size,
            );
            // Herdr distinguishes the stream mode: a controller resize
            // changes the shared PTY, while an observer resize updates only
            // that observer's render viewport. Bootstrap observers on older
            // servers are still replaced, but only after geometry settles.
            runtime.size = size;
            runtime.pixel_size = pending.pixels;
            runtime.viewport_ready = pending.pixels.0 > 0 && pending.pixels.1 > 0;
            if restart_observer && let Some((profile, session_name)) = observer_reconnect.as_ref() {
                let (session, frames) = TerminalSession::spawn(
                    profile.clone(),
                    session_name.clone(),
                    pane_id.to_owned(),
                    TerminalMode::Observe,
                    resolved.columns,
                    resolved.rows,
                );
                runtime.listen = None;
                runtime.session = session;
                replacement_frames = Some(frames);
            } else {
                let _ = runtime.session.send(TerminalCommand::Resize {
                    cols: resolved.columns,
                    rows: resolved.rows,
                    cell_width_px: resolved.cell_width_px,
                    cell_height_px: resolved.cell_height_px,
                });
            }
            // Resizing swaps the framebuffer context. Produce and collect the
            // first matching frame now instead of waiting for a pointer or key
            // event to take the same refresh path.
            flush_pane_surface(runtime);
        }
        if let Some(frames) = replacement_frames {
            let task = Self::listen_pane(pane_id.to_owned(), frames, cx);
            if let Some(runtime) = self.pane_mut(pane_id) {
                runtime.listen = Some(task);
            }
        }
        cx.notify();
    }

    /// A preview often has exactly the same final bounds as authority. GPUI
    /// then has no layout change with which to invoke the pane canvas measure
    /// callback after the freeze ends. Reapply the cached body explicitly and
    /// flush Ghostty so the first authoritative frame paints without a click.
    pub(crate) fn refresh_thawed_pane_bodies(
        &mut self,
        pane_ids: &[String],
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let cached = pane_ids
            .iter()
            .filter_map(|pane_id| {
                let body = self.pane(pane_id)?.body_bounds;
                (body.2 > 0. && body.3 > 0.).then(|| (pane_id.clone(), body))
            })
            .collect::<Vec<_>>();
        for (pane_id, body) in cached {
            self.sync_measured_pane_body(
                &pane_id,
                Bounds {
                    origin: point(px(body.0), px(body.1)),
                    size: size(px(body.2), px(body.3)),
                },
                window,
                cx,
            );
            if let Some(runtime) = self.pane_mut(&pane_id) {
                flush_pane_surface(runtime);
            }
        }
        cx.notify();
    }
}
