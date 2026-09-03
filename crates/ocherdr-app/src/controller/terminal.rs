use super::*;

impl OcHerdrView {
    pub(crate) fn ensure_session_terminals(&mut self, cx: &mut Context<Self>) {
        let profile_id = self.current_profile().id().to_owned();
        let Some(session_name) = self.current_session().map(|session| session.name.clone()) else {
            self.stop_session_terminals();
            return;
        };
        if self.snapshot.is_none() {
            self.stop_session_terminals();
            return;
        }
        let Some(terminal_endpoint) = self
            .connection
            .as_ref()
            .map(SessionConnection::terminal_endpoint)
        else {
            self.stop_session_terminals();
            return;
        };
        let visible_tab_id = self.selection.tab_id.clone();
        let selected_pane_id = self.selection.pane_id.clone();
        let snapshot = self.snapshot.as_ref().expect("snapshot checked above");
        let terminal_protocol = snapshot.protocol;
        let live_pane_ids = snapshot_pane_ids(snapshot);
        let pane_tabs = snapshot
            .panes
            .iter()
            .map(|pane| (pane.pane_id.clone(), pane.tab_id.clone()))
            .collect::<HashMap<_, _>>();
        let incoming = SessionKey {
            profile_id,
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
        let optimistic_visible = self.optimistic_visible_pane_ids();
        self.pane_viewports
            .retain(|pane_id, _| live_pane_ids.contains(pane_id));
        let measured_viewports = self.pane_viewports.clone();
        let mut too_small = Vec::new();
        let mut mounted = 0usize;
        let mut mount_more = false;
        {
            let (controls, access_serial) = {
                let session = self
                    .session_panes
                    .as_mut()
                    .expect("live session adopted panes");
                session.access_serial = session.access_serial.wrapping_add(1);
                session
                    .controls
                    .retain(|pane_id, _| live_pane_ids.contains(pane_id));
                prime_automatic_terminal_control(session, &optimistic_visible, &live_pane_ids);
                (session.controls.clone(), session.access_serial)
            };
            #[cfg_attr(not(test), allow(unused_mut))]
            let mut wanted = snapshot_runtime_targets(
                snapshot,
                &controls,
                visible_tab_id.as_deref(),
                selected_pane_id.as_deref(),
            );
            wanted.retain(|target| optimistic_visible.contains(&target.pane_id));
            wanted.sort_by_key(|target| !target.focused);
            // A template rebuild temporarily moves panes through hidden tabs.
            // Keep the streams users are still looking at in their current
            // control modes instead of reconnecting them midway.
            for target in &mut wanted {
                if optimistic_visible.contains(&target.pane_id)
                    && let Some(mode) = controls.get(&target.pane_id)
                {
                    target.mode = *mode;
                }
            }
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

            // Visibility and control ownership are independent. Keep a hidden
            // pane's existing private stream intact so an ordinary tab switch
            // does not detach from Herdr, discard client-scoped resources, and
            // reconnect when the user returns. LRU eviction and real pane or
            // session removal still drop the stream normally.
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
                            runtime.last_visible_serial = access_serial;
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
                                terminal_endpoint.clone(),
                                terminal_protocol,
                                pane_id.clone(),
                            ) {
                                pending_listens.push((pane_id.clone(), frames));
                            }
                        }
                    }
                    VisiblePanePlan::Spawn => {
                        let Some(viewport) =
                            measured_viewports.get(pane_id).copied().filter(|viewport| {
                                viewport.mountable && viewport.pixels.0 > 0 && viewport.pixels.1 > 0
                            })
                        else {
                            continue;
                        };
                        if mounted >= PANE_MOUNT_BATCH_SIZE {
                            mount_more = true;
                            continue;
                        }
                        let cols = 80;
                        let rows = 24;
                        match Terminal::new(cols, rows, 10_000, &palette) {
                            Ok(terminal) => {
                                let frame_context = 1;
                                let resolved = terminal.resize_pixels(
                                    viewport.pixels.0,
                                    viewport.pixels.1,
                                    viewport.scale_factor,
                                    frame_context,
                                );
                                if !pane_grid_mountable(resolved.columns, resolved.rows) {
                                    too_small.push(pane_id.clone());
                                    continue;
                                }
                                let (session, frames) = TerminalSession::spawn(
                                    terminal_endpoint.clone(),
                                    terminal_protocol,
                                    pane_id.clone(),
                                    mode,
                                    resolved.columns,
                                    resolved.rows,
                                );
                                terminal.set_focus(target.focused);
                                panes.insert(
                                    pane_id.clone(),
                                    PaneRuntime {
                                        session,
                                        terminal,
                                        frame: None,
                                        mode,
                                        focused: target.focused,
                                        size: (resolved.columns, resolved.rows),
                                        pixel_size: viewport.pixels,
                                        viewport_ready: true,
                                        frame_context,
                                        color_scheme_dark,
                                        palette_signature: palette.signature(),
                                        listen: None,
                                        exit_seen: false,
                                        scroll_px: 0.,
                                        body_bounds: viewport.body_bounds,
                                        pending_resize: None,
                                        last_visible_serial: access_serial,
                                    },
                                );
                                mounted += 1;
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
                ) && !optimistic_visible.contains(pane_id)
                {
                    continue;
                }
                if let Some(runtime) = panes.get_mut(pane_id) {
                    flush_pane_surface(runtime);
                }
            }

            let retained_limit = PANE_RUNTIME_CACHE_LIMIT.max(optimistic_visible.len());
            if panes.len() > retained_limit {
                let mut hidden = panes
                    .iter()
                    .filter(|(pane_id, _)| !optimistic_visible.contains(*pane_id))
                    .map(|(pane_id, runtime)| (runtime.last_visible_serial, pane_id.clone()))
                    .collect::<Vec<_>>();
                hidden.sort_by_key(|(serial, _)| *serial);
                let remove = panes.len().saturating_sub(retained_limit);
                for (_, pane_id) in hidden.into_iter().take(remove) {
                    panes.remove(&pane_id);
                }
            }
        }
        for pane_id in too_small {
            if let Some(viewport) = self.pane_viewports.get_mut(&pane_id) {
                viewport.mountable = false;
            }
        }
        if let Some(error) = palette_error {
            self.notify_failure(FailureKind::ApplyPalette, error, cx);
        }
        let spawn_failed = spawn_error.is_some();
        if let Some(error) = spawn_error {
            self.notify_failure(FailureKind::SpawnTerminal, error, cx);
        }
        for (pane_id, frames) in pending_listens {
            let owner = self
                .session_panes
                .as_ref()
                .expect("pane listener requires a live session")
                .owner
                .clone();
            let task = Self::listen_pane(owner, pane_id.clone(), frames, cx);
            if let Some(runtime) = self.pane_mut(&pane_id) {
                runtime.listen = Some(task);
            }
        }
        if mount_more && !spawn_failed {
            self.schedule_pane_mount(cx);
        }
    }

    pub(super) fn stop_session_terminals(&mut self) {
        self.pane_resize_serial = self.pane_resize_serial.wrapping_add(1);
        self.session_panes = None;
        self.pane_viewports.clear();
        self.pane_mount_scheduled = false;
    }

    fn schedule_pane_mount(&mut self, cx: &mut Context<Self>) {
        if self.pane_mount_scheduled {
            return;
        }
        self.pane_mount_scheduled = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PANE_MOUNT_DELAY).await;
            this.update(cx, |this, cx| {
                this.pane_mount_scheduled = false;
                this.ensure_session_terminals(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A direct terminal interaction is an explicit request to take over this
    /// pane without releasing other controlled panes. The request is
    /// deliberately user-driven, never a reconnect retry, so clients cannot
    /// get into a takeover loop.
    pub(crate) fn take_terminal_control(&mut self, pane_id: String, cx: &mut Context<Self>) {
        if self
            .pane(&pane_id)
            .is_some_and(|runtime| runtime.mode.is_controlled())
        {
            return;
        }
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
        mode: TerminalMode,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.session_panes.as_mut() else {
            return false;
        };
        if !demote_terminal_control(session, pane_id) {
            return false;
        }
        // The first visible-pane control request is deliberately
        // non-takeover. An existing owner is normal: reconnect once as an
        // observer without presenting an error or retrying control.
        if mode == TerminalMode::Control && loss == TerminalControlLoss::Busy {
            return true;
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
        owner: SessionKey,
        pane_id: String,
        mut frames: TerminalEventReceiver,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let herdr = next_batch(&mut frames);
                let ghostty = poll_fn(|task_cx| {
                    this.update(cx, |this, _| {
                        let Some(runtime) = this.pane_for_owner_mut(&owner, &pane_id) else {
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
                            .update(cx, |this, cx| {
                                this.apply_herdr_frames(&owner, &pane_id, batch, cx)
                            })
                            .unwrap_or(false);
                        if !keep {
                            break;
                        }
                    }
                    Either::Right((frame, _)) => {
                        let keep = this
                            .update(cx, |this, cx| {
                                this.apply_ghostty_frame(&owner, &pane_id, frame, cx)
                            })
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
        owner: &SessionKey,
        pane_id: &str,
        batch: Option<Vec<std::result::Result<TerminalEvent, HerdrError>>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let active = self.is_active_session(owner);
        let composing = active.then(|| self.ime_marked.clone()).flatten();
        let selected_pane = active.then(|| self.selection.pane_id.clone()).flatten();
        let visible_pane_ids = if active {
            self.optimistic_visible_pane_ids()
        } else {
            HashSet::new()
        };
        let mut error = None;
        let mut hierarchy_changed = false;
        let mut control_loss = None;
        let mut changed = false;
        let keep = {
            let Some(runtime) = self.pane_for_owner_mut(owner, pane_id) else {
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
                            Ok(TerminalEvent::KittyKeyboardReportAll { enabled }) => {
                                runtime.terminal.set_kitty_keyboard_report_all(enabled)
                            }
                            Err(stream_error) => {
                                runtime.exit_seen = true;
                                control_loss = runtime
                                    .mode
                                    .is_controlled()
                                    .then(|| {
                                        terminal_control_loss(&stream_error)
                                            .map(|loss| (loss, runtime.mode))
                                    })
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
        if !active {
            if !keep || error.is_some() {
                if let Some(runtime) = self.parked_hosts.get_mut(&owner.profile_id) {
                    runtime.event_stream = EventStreamState::Lost(
                        HerdrError::TerminalClosed("terminal stream disconnected".into())
                            .to_string()
                            .into(),
                    );
                }
                cx.notify();
            }
            return keep;
        }
        if let Some((kind, detail)) = error {
            self.notify_failure(kind, detail, cx);
        }
        if let Some((loss, mode)) = control_loss
            && self.demote_lost_terminal_control(pane_id, loss, mode, cx)
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
        owner: &SessionKey,
        pane_id: &str,
        frame: Option<std::result::Result<RenderedFrame, ocherdr_terminal::TerminalError>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let active = self.is_active_session(owner);
        let visible_pane_ids = if active {
            self.optimistic_visible_pane_ids()
        } else {
            HashSet::new()
        };
        let mut error = None;
        let mut changed = false;
        let mut hierarchy_changed = false;
        let keep = {
            let Some(runtime) = self.pane_for_owner_mut(owner, pane_id) else {
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
        if !active {
            if !keep || error.is_some() {
                if let Some(runtime) = self.parked_hosts.get_mut(&owner.profile_id) {
                    runtime.event_stream = EventStreamState::Lost(
                        HerdrError::TerminalClosed("terminal renderer disconnected".into())
                            .to_string()
                            .into(),
                    );
                }
                cx.notify();
            }
            return keep;
        }
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

    /// Pane ids painted in the selected tab, including panes Herdr has
    /// temporarily parked elsewhere during an optimistic template rebuild.
    pub(crate) fn optimistic_visible_pane_ids(&self) -> HashSet<String> {
        let mut visible =
            visible_pane_ids(self.snapshot.as_ref(), self.selection.tab_id.as_deref());
        if let Some(tab_id) = self.selection.tab_id.as_deref()
            && !self.pane_template_commits.contains_key(tab_id)
            && let Some(layout) = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.layout_for(tab_id))
            && layout.zoomed
        {
            visible.retain(|pane_id| pane_id == &layout.focused_pane_id);
        }
        if let Some(tab_id) = self.selection.tab_id.as_deref()
            && let Some(pending) = self.pane_template_commits.get(tab_id)
        {
            visible.extend(pending.predicted_pane_ids().map(str::to_owned));
        }
        if let Some(tab_id) = self.selection.tab_id.as_deref()
            && let Some(pending) = self
                .pane_relocations
                .get(tab_id)
                .filter(|pending| pending.phase.locks_tab())
        {
            visible.extend(pending.plan.predicted_pane_ids().map(str::to_owned));
        }
        for pending in self.pane_detaches.values() {
            visible.insert(pending.source_pane_id.clone());
            visible.extend(pending.predicted_pane_ids().map(str::to_owned));
        }
        visible
    }

    /// Terminal grids stay put while free-form geometry is only a preview
    /// (design §5.4, §7.2). Template destinations are already exact, so they
    /// resize optimistically while Herdr rebuilds the same layout in back.
    pub(crate) fn pane_resize_frozen(&self, pane_id: &str) -> bool {
        if self
            .pane_template_commits
            .values()
            .any(|pending| pending.predicted_pane_ids().any(|id| id == pane_id))
        {
            return false;
        }
        if self
            .pane_detaches
            .values()
            .any(|pending| pending.predicted_pane_ids().any(|id| id == pane_id))
        {
            return false;
        }
        self.pane_tab_id(pane_id)
            .is_some_and(|tab_id| self.tab_resize_frozen(&tab_id))
            || self.pane_relocations.values().any(|pending| {
                pending.phase.locks_tab()
                    && pending.plan.predicted_pane_ids().any(|id| id == pane_id)
            })
    }

    pub(super) fn tab_resize_frozen(&self, tab_id: &str) -> bool {
        if self.pane_template_commits.contains_key(tab_id) {
            return false;
        }
        if self
            .pane_detaches
            .values()
            .any(|pending| pending.locks_tab(tab_id))
        {
            return false;
        }
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
        let previous = self.pane_viewports.get(pane_id).copied();
        let mountable = previous
            .filter(|viewport| viewport.pixels == (width_px, height_px))
            .is_none_or(|viewport| viewport.mountable);
        self.pane_viewports.insert(
            pane_id.to_owned(),
            MeasuredPaneViewport {
                body_bounds: body,
                pixels: (width_px, height_px),
                scale_factor,
                mountable,
            },
        );
        let palette = current_terminal_palette(&self.appearance);
        let mut palette_error = None;
        let frozen = self.pane_resize_frozen(pane_id);
        let next_serial = self.pane_resize_serial.wrapping_add(1);
        let mut scheduled = None;
        {
            let Some(runtime) = self.pane_mut(pane_id) else {
                self.schedule_pane_mount(cx);
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
        let visible = self.optimistic_visible_pane_ids().contains(pane_id);
        let frozen = self.pane_resize_frozen(pane_id);
        let mut collapse = false;
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
            if !pane_grid_mountable(resolved.columns, resolved.rows) {
                collapse = true;
            }
            let size = (resolved.columns, resolved.rows);
            // Herdr distinguishes the stream mode: a controller resize
            // changes the shared PTY, while an observer resize updates only
            // that observer's render viewport. The native protocol handles
            // both modes in place, so resizing never tears down the surface or
            // its private stream.
            runtime.size = size;
            runtime.pixel_size = pending.pixels;
            runtime.viewport_ready = pending.pixels.0 > 0 && pending.pixels.1 > 0;
            if collapse {
                // Keep only the shell/dividers. A later, larger measurement
                // makes the pane mountable again and creates a fresh stream.
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
            if !collapse {
                flush_pane_surface(runtime);
            }
        }
        if collapse {
            if let Some(viewport) = self.pane_viewports.get_mut(pane_id) {
                viewport.mountable = false;
            }
            if let Some(session) = self.session_panes.as_mut() {
                session.controls.remove(pane_id);
                session.panes.remove(pane_id);
            }
            cx.notify();
            return;
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
