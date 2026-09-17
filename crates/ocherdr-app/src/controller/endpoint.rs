//! Herdr endpoint generation 1 integration.
//!
//! Servers at private protocol 22+ (Herdr 0.9.x) no longer offer the numbered
//! per-pane streams OcHerdr 0.8.x compatibility keeps. Instead one client-owned
//! shell connection per session carries the session snapshot, the composited
//! tab surface, semantic input, and an endpoint-scoped API channel. This module
//! adapts that single stream onto OcHerdr's per-pane model: `SurfaceDecoder`
//! slices each pane's `inner_rect` out of the composited frame and re-encodes
//! it as ANSI for the pane's own terminal mirror.

use super::*;

pub(crate) use ocherdr_herdr::endpoint::EndpointHandle;
use ocherdr_herdr::endpoint::{
    EndpointCommand, EndpointEvent, EndpointEventReceiver, EndpointSession,
};
use ocherdr_herdr::endpoint_v1 as ep;
pub(crate) use ocherdr_herdr::endpoint_v1::{
    ClientKeyCode, ClientKeyKind, ClientMouseButton, ClientMouseGeometry, ClientMouseKind,
    ClientMousePosition, ClientPaneInputEvent,
};
use ocherdr_herdr::surface_ansi::{PaneRenderUpdate, SurfaceDecoder};

/// First private protocol version that carries the endpoint shell protocol.
/// Herdr 0.9.0/0.9.1 both report private protocol 22. Protocol 21 only ever
/// shipped on pre-0.9 preview builds, so it is intentionally not bridged:
/// those servers keep using neither path and surface as unsupported.
pub(crate) const ENDPOINT_MIN_PROTOCOL: u32 = 22;

/// A live client-owned shell connection for one session.
pub(crate) struct EndpointRuntime {
    pub session: EndpointSession,
    pub decoder: SurfaceDecoder,
    pub listen: Option<Task<()>>,
    /// Cell metrics last reported to the server (physical pixels).
    pub cell_px: (u32, u32),
    /// Surface grid last reported (cells).
    pub surface: (u16, u16),
    /// The client view location last pushed (workspace, tab, pane).
    pub view: (Option<String>, Option<String>, Option<String>),
    /// `window_active` state last reported.
    pub focused: Option<bool>,
}

impl EndpointRuntime {
    fn new(session: EndpointSession) -> Self {
        Self {
            session,
            decoder: SurfaceDecoder::new(),
            listen: None,
            cell_px: (0, 0),
            surface: (0, 0),
            view: (None, None, None),
            focused: None,
        }
    }
}

impl OcHerdrView {
    /// True while a live session runs on the endpoint shell protocol.
    pub(crate) fn endpoint_active(&self) -> bool {
        self.session_panes
            .as_ref()
            .is_some_and(|session| session.endpoint.is_some())
    }

    /// True when the connected server can host an endpoint session, whether or
    /// not the connection has finished negotiating.
    pub(crate) fn endpoint_capable(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.protocol >= ENDPOINT_MIN_PROTOCOL)
    }

    /// Spawns the session-level endpoint connection when the server supports
    /// it. Safe to call on every `ensure_session_terminals`.
    pub(super) fn ensure_endpoint_session(&mut self, cx: &mut Context<Self>) {
        if !self.endpoint_capable() {
            return;
        }
        #[cfg(test)]
        if self.headless_terminals {
            return;
        }
        if self
            .session_panes
            .as_ref()
            .is_some_and(|session| session.endpoint.is_some())
        {
            return;
        }
        let Some(terminal_endpoint) = self
            .connection
            .as_ref()
            .map(SessionConnection::terminal_endpoint)
        else {
            return;
        };
        let (cols, rows, cell) = self.endpoint_surface_size();
        let Some(session) = self.session_panes.as_mut() else {
            return;
        };
        let (endpoint_session, events) =
            EndpointSession::spawn(terminal_endpoint, cols, rows, cell.0, cell.1);
        let mut runtime = EndpointRuntime::new(endpoint_session);
        runtime.cell_px = cell;
        runtime.surface = (cols, rows);
        let owner = session.owner.clone();
        let task = Self::listen_endpoint(owner.clone(), events, cx);
        runtime.listen = Some(task);
        session.endpoint = Some(runtime);
        // Claim the surface and hand the server our current view so the first
        // frame composites the tab OcHerdr is showing.
        self.endpoint_activate(cx);
    }

    /// Pushes surface-active, window focus, and the client's view location to
    /// the endpoint so the composited surface tracks OcHerdr's selection.
    pub(crate) fn endpoint_activate(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session_panes.as_mut() else {
            return;
        };
        let Some(endpoint) = session.endpoint.as_mut() else {
            return;
        };
        let _ = endpoint.session.set_surface_active(true);
        let focused = self.window_active;
        if endpoint.focused != Some(focused) {
            endpoint.focused = Some(focused);
            let _ = endpoint.session.send(EndpointCommand::Focus(focused));
        }
        self.endpoint_sync_view();
        self.endpoint_send_resize();
        self.endpoint_send_host_theme();
        cx.notify();
    }

    /// Mirrors OcHerdr's selection into the client's view location so the
    /// server composites the tab this window is actually showing.
    fn endpoint_sync_view(&mut self) {
        let view = (
            self.selection.workspace_id.clone(),
            self.selection.tab_id.clone(),
            self.selection.pane_id.clone(),
        );
        let Some(session) = self.session_panes.as_mut() else {
            return;
        };
        let Some(endpoint) = session.endpoint.as_mut() else {
            return;
        };
        if endpoint.view == view {
            return;
        }
        let previous = std::mem::replace(&mut endpoint.view, view.clone());
        let handle = endpoint.session.handle();
        if view.0 != previous.0
            && let Some(workspace_id) = &view.0
        {
            let _ = handle.request(
                "workspace.focus",
                serde_json::json!({ "workspace_id": workspace_id }),
            );
        }
        if view.1 != previous.1
            && let Some(tab_id) = &view.1
        {
            let _ = handle.request("tab.focus", serde_json::json!({ "tab_id": tab_id }));
        }
        if view.2 != previous.2
            && let Some(pane_id) = &view.2
        {
            let _ = handle.request("pane.focus", serde_json::json!({ "pane_id": pane_id }));
        }
    }

    /// The tab content area in cells plus the measured cell metrics. Falls
    /// back to a generous grid before the first pane reports its font's
    /// physical cell size; the server tiles panes inside whatever it gets.
    fn endpoint_surface_size(&self) -> (u16, u16, (u32, u32)) {
        let cell = self.endpoint_cell_px();
        let Some((_, _, w, h)) = self.terminal_surface_bounds else {
            return (240, 60, cell);
        };
        let scale = self.endpoint_scale_factor() as f32;
        let cols = ((w * scale) / cell.0.max(1) as f32) as u16;
        let rows = ((h * scale) / cell.1.max(1) as f32) as u16;
        (cols.max(2), rows.max(1), cell)
    }

    fn endpoint_scale_factor(&self) -> f64 {
        self.pane_viewports
            .values()
            .next()
            .map(|viewport| viewport.scale_factor)
            .unwrap_or(2.0)
    }

    /// Physical-pixel cell size from any live pane mirror; falls back to a
    /// plausible default before the first surface is measured.
    fn endpoint_cell_px(&self) -> (u32, u32) {
        if let Some(session) = self.session_panes.as_ref() {
            for runtime in session.panes.values() {
                let size = runtime.terminal.surface_size();
                if size.cell_width_px > 0 && size.cell_height_px > 0 {
                    return (size.cell_width_px, size.cell_height_px);
                }
            }
        }
        (9, 18)
    }

    /// Reports the total surface grid to the endpoint when it changed. The
    /// server tiles panes into it with its own dividers; OcHerdr sizes each
    /// pane's mirror from the returned `inner_rect` cells.
    pub(crate) fn endpoint_send_resize(&mut self) {
        let (cols, rows, cell) = self.endpoint_surface_size();
        let Some(session) = self.session_panes.as_mut() else {
            return;
        };
        let Some(endpoint) = session.endpoint.as_mut() else {
            return;
        };
        if endpoint.surface == (cols, rows) && endpoint.cell_px == cell {
            return;
        }
        endpoint.surface = (cols, rows);
        endpoint.cell_px = cell;
        let _ = endpoint.session.send(EndpointCommand::Resize {
            cols,
            rows,
            cell_width_px: cell.0,
            cell_height_px: cell.1,
            // OcHerdr sends pixel-accurate positions when a pane asks for them.
            pixel_mouse: true,
        });
    }

    /// The endpoint writer, when the session is in endpoint mode.
    pub(crate) fn endpoint_handle(&self) -> Option<EndpointHandle> {
        self.session_panes
            .as_ref()
            .and_then(|session| session.endpoint.as_ref())
            .map(|endpoint| endpoint.session.handle())
    }

    /// Sends semantic input for `pane_id` over the endpoint channel.
    pub(crate) fn endpoint_pane_input(&self, pane_id: &str, events: Vec<ClientPaneInputEvent>) {
        if let Some(handle) = self.endpoint_handle() {
            let _ = handle.pane_input(pane_id, events);
        }
    }

    /// Mirrors window activation into `ClientShellFocus`.
    pub(crate) fn endpoint_set_focused(&mut self, focused: bool) {
        if let Some(session) = self.session_panes.as_mut()
            && let Some(endpoint) = session.endpoint.as_mut()
            && endpoint.focused != Some(focused)
        {
            endpoint.focused = Some(focused);
            let _ = endpoint.session.send(EndpointCommand::Focus(focused));
        }
    }

    /// Enables or disables surface streaming (parked hosts stop pulling
    /// frames without dropping the connection).
    pub(crate) fn endpoint_set_surface_active(&mut self, active: bool) {
        if let Some(session) = self.session_panes.as_mut()
            && let Some(endpoint) = session.endpoint.as_ref()
        {
            let _ = endpoint.session.set_surface_active(active);
        }
    }

    /// Re-publishes the client's view location after selection changes.
    pub(crate) fn endpoint_selection_changed(&mut self) {
        self.endpoint_sync_view();
    }

    /// Publishes the host palette + appearance so server-side terminal
    /// chrome (pane borders, agent status) follows OcHerdr's theme.
    pub(crate) fn endpoint_send_host_theme(&mut self) {
        let Some(session) = self.session_panes.as_ref() else {
            return;
        };
        let Some(endpoint) = session.endpoint.as_ref() else {
            return;
        };
        let palette = current_terminal_palette(&self.appearance);
        let send = |update| {
            let _ = endpoint.session.send(EndpointCommand::HostTheme(update));
        };
        send(ep::ClientHostThemeUpdate::Appearance(if palette.dark {
            ep::ClientHostAppearance::Dark
        } else {
            ep::ClientHostAppearance::Light
        }));
        send(ep::ClientHostThemeUpdate::DefaultColor {
            kind: ep::ClientHostDefaultColorKind::Background,
            color: client_host_color(palette.background),
        });
        send(ep::ClientHostThemeUpdate::DefaultColor {
            kind: ep::ClientHostDefaultColorKind::Foreground,
            color: client_host_color(palette.foreground),
        });
        send(ep::ClientHostThemeUpdate::PaletteColors(
            palette
                .ansi
                .iter()
                .enumerate()
                .map(|(index, &color)| (index as u8, client_host_color(color)))
                .collect(),
        ));
    }

    fn listen_endpoint(
        owner: SessionKey,
        mut events: EndpointEventReceiver,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let Some(batch) = next_batch(&mut events).await else {
                    break;
                };
                let keep = this
                    .update(cx, |this, cx| this.apply_endpoint_batch(&owner, batch, cx))
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
    }

    /// Drains one batch of endpoint events onto the pane runtimes. Returns
    /// false when the connection is gone and the listener should stop.
    pub(crate) fn apply_endpoint_batch(
        &mut self,
        owner: &SessionKey,
        batch: Vec<std::result::Result<EndpointEvent, HerdrError>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let active = self.is_active_session(owner);
        let selected_pane = active.then(|| self.selection.pane_id.clone()).flatten();
        let mut error = None;
        let mut closed = false;
        let mut updates: Vec<PaneRenderUpdate> = Vec::new();
        let mut notifies = Vec::new();
        let mut clipboards = Vec::new();
        let mut report_all = Vec::new();
        let mut view_sync = false;
        {
            let Some(session) = self.session_for_owner_mut(owner) else {
                return false;
            };
            let Some(endpoint) = session.endpoint.as_mut() else {
                return false;
            };
            for item in batch {
                match item {
                    // The snapshot duplicates `session.snapshot` on the public
                    // socket, which stays authoritative for the hierarchy. Its
                    // arrival doubles as the cue to re-verify the pushed view
                    // after a refused request.
                    Ok(EndpointEvent::Welcome(_)) => {}
                    Ok(EndpointEvent::Snapshot(_)) => {
                        view_sync = true;
                    }
                    Ok(EndpointEvent::Surface(frame)) => {
                        updates.extend(endpoint.decoder.frame(*frame, false));
                    }
                    Ok(EndpointEvent::SurfacePatch(patch)) => {
                        if let Some(patched) = endpoint.decoder.patch(&patch) {
                            updates.extend(patched);
                        }
                    }
                    Ok(EndpointEvent::Response { data, .. }) => {
                        // A refused endpoint call (endpoint_busy,
                        // surface_inactive, stale_boot, or a stale target id)
                        // means the pushed view may never have been applied.
                        // Invalidate it so the next sync re-issues instead of
                        // suppressing it as already sent; the next snapshot
                        // or selection change converges it.
                        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data)
                            && value.get("error").is_some()
                        {
                            endpoint.view = (None, None, None);
                        }
                    }
                    Ok(EndpointEvent::Notify {
                        kind,
                        message,
                        body,
                    }) => {
                        notifies.push((kind, message, body));
                    }
                    // Agent attention state already arrives through the
                    // public-socket snapshot the hierarchy renders from.
                    Ok(EndpointEvent::SemanticNotification(_)) => {}
                    Ok(EndpointEvent::ShellError(message)) => {
                        error = Some((FailureKind::TerminalStream, message));
                    }
                    Ok(EndpointEvent::Clipboard(data)) => {
                        clipboards.push(data);
                    }
                    // The window title is OcHerdr-owned (session + profile),
                    // not delegated to pane OSC sequences.
                    Ok(EndpointEvent::WindowTitle(_)) => {}
                    // Session-level capture hint; per-pane reporting state
                    // arrives on each PaneRenderUpdate instead.
                    Ok(EndpointEvent::MouseCapture { .. }) => {}
                    Ok(EndpointEvent::KeyboardReportAll(enabled)) => {
                        report_all.push(enabled);
                    }
                    // No host bell or sound playback is wired into OcHerdr.
                    Ok(EndpointEvent::Bell(_count)) => {}
                    Ok(EndpointEvent::ReloadSoundConfig) => {}
                    Ok(EndpointEvent::Shutdown(reason)) => {
                        error = Some((
                            FailureKind::TerminalStream,
                            reason.unwrap_or_else(|| "endpoint server shutdown".into()),
                        ));
                        closed = true;
                    }
                    Err(stream_error) => {
                        closed = true;
                        error = Some((FailureKind::TerminalStream, stream_error.to_string()));
                    }
                }
            }
        }
        if active && view_sync && !closed {
            self.endpoint_sync_view();
        }
        for (kind, message, body) in notifies {
            self.post_herdr_notification(owner, "", kind, message, body, cx);
        }
        for data in clipboards {
            cx.write_to_clipboard(ClipboardItem::new_string(data));
        }
        if let Some(&enabled) = report_all.last()
            && let Some(pane_id) = &selected_pane
            && let Some(runtime) = self.pane_for_owner_mut(owner, pane_id)
        {
            runtime.terminal.set_kitty_keyboard_report_all(enabled);
        }
        if let Some((kind, detail)) = error {
            self.notify_failure(kind, detail, cx);
        }
        if closed {
            if let Some(session) = self.session_for_owner_mut(owner) {
                session.endpoint = None;
                for runtime in session.panes.values_mut() {
                    runtime.exit_seen = true;
                }
            }
            if active {
                self.resync_snapshot(self.event_epoch, cx);
            }
            cx.notify();
            return false;
        }
        let mut changed = false;
        for update in updates {
            changed |= self.apply_endpoint_pane_update(owner, update);
        }
        if let Err(runtime_error) = Terminal::tick_runtime() {
            self.notify_failure(FailureKind::TerminalRuntime, runtime_error.to_string(), cx);
        }
        let visible = if active {
            self.optimistic_visible_pane_ids()
        } else {
            HashSet::new()
        };
        let mut painted = false;
        let mut frame_error = None;
        if let Some(session) = self.session_for_owner_mut(owner) {
            for (pane_id, runtime) in session.panes.iter_mut() {
                match runtime.terminal.try_frame() {
                    Ok(Some(frame)) if frame.host_context == runtime.frame_context => {
                        runtime.frame = Some(frame);
                        painted |= visible.contains(pane_id);
                    }
                    Ok(_) => {}
                    Err(error) => frame_error = Some(error),
                }
            }
        }
        if let Some(frame_error) = frame_error {
            self.notify_failure(FailureKind::RenderTerminal, frame_error.to_string(), cx);
        }
        if changed || painted {
            self.render_cache.notify_terminal(cx);
        }
        true
    }

    /// Applies one pane's ANSI blit + geometry to its terminal mirror.
    fn apply_endpoint_pane_update(&mut self, owner: &SessionKey, update: PaneRenderUpdate) -> bool {
        let Some(runtime) = self.pane_for_owner_mut(owner, &update.pane_id) else {
            return false;
        };
        let size = (update.inner_rect.width, update.inner_rect.height);
        if runtime.size != size {
            runtime.size = size;
            let _ = runtime.terminal.set_grid_size(size.0.max(1), size.1.max(1));
        }
        runtime.endpoint_pixels = (update.pixel_width, update.pixel_height);
        let capture = (update.mouse_reporting, update.sgr_pixel_mouse);
        if runtime.mouse_capture != Some(capture) {
            runtime.mouse_capture = Some(capture);
            runtime.terminal.set_mouse_capture(capture.0, capture.1);
        }
        runtime.terminal.apply_frame(&update.ansi, update.full);
        true
    }
}

// ---------------------------------------------------------------------------
// Semantic input mapping: GPUI keystrokes → ClientPaneInputEvent
// ---------------------------------------------------------------------------

/// Palette colors are `0xRRGGBB`.
fn client_host_color(color: u32) -> ep::ClientHostColor {
    ep::ClientHostColor {
        r: ((color >> 16) & 0xff) as u8,
        g: ((color >> 8) & 0xff) as u8,
        b: (color & 0xff) as u8,
    }
}

const MOD_SHIFT: u8 = 1;
const MOD_CONTROL: u8 = 2;
const MOD_ALT: u8 = 4;
const MOD_SUPER: u8 = 8;

pub(crate) fn endpoint_modifiers(modifiers: ochub_ui::gpui::Modifiers) -> u8 {
    let mut bits = 0u8;
    if modifiers.shift {
        bits |= MOD_SHIFT;
    }
    if modifiers.control {
        bits |= MOD_CONTROL;
    }
    if modifiers.alt {
        bits |= MOD_ALT;
    }
    if modifiers.platform {
        bits |= MOD_SUPER;
    }
    bits
}

/// Maps a GPUI key name to the wire `ClientKeyCode`.
pub(crate) fn client_key_code(key: &str) -> Option<ClientKeyCode> {
    Some(match key.to_ascii_lowercase().as_str() {
        "backspace" => ClientKeyCode::Backspace,
        "enter" | "return" => ClientKeyCode::Enter,
        "left" => ClientKeyCode::Left,
        "right" => ClientKeyCode::Right,
        "up" => ClientKeyCode::Up,
        "down" => ClientKeyCode::Down,
        "home" => ClientKeyCode::Home,
        "end" => ClientKeyCode::End,
        "pageup" => ClientKeyCode::PageUp,
        "pagedown" => ClientKeyCode::PageDown,
        "tab" => ClientKeyCode::Tab,
        "backtab" => ClientKeyCode::BackTab,
        "delete" => ClientKeyCode::Delete,
        "insert" => ClientKeyCode::Insert,
        "escape" | "esc" => ClientKeyCode::Esc,
        "space" | " " => ClientKeyCode::Char(' '),
        name => {
            if let Some(number) = name
                .strip_prefix('f')
                .and_then(|digits| digits.parse::<u8>().ok())
                && (1..=35).contains(&number)
            {
                ClientKeyCode::F(number)
            } else {
                let mut chars = name.chars();
                match (chars.next(), chars.next()) {
                    (Some(ch), None) => ClientKeyCode::Char(ch),
                    _ => return None,
                }
            }
        }
    })
}

/// One key event translated for the endpoint channel. `generated_text` carries
/// the text the key produces when modifiers are text-only (none or shift),
/// matching Herdr's own `with_text_commit` rule; chord modifiers leave it
/// `None` so the server encodes the chord itself.
pub(crate) fn endpoint_key_event(
    key: &Keystroke,
    action: KeyAction,
) -> Option<ClientPaneInputEvent> {
    let code = client_key_code(&key.key)?;
    let modifiers = endpoint_modifiers(key.modifiers);
    let kind = match action {
        KeyAction::Press => ClientKeyKind::Press,
        KeyAction::Repeat => ClientKeyKind::Repeat,
        KeyAction::Release => ClientKeyKind::Release,
    };
    let text_only =
        matches!(code, ClientKeyCode::Char(_)) && (modifiers == 0 || modifiers == MOD_SHIFT);
    let generated_text = if text_only && kind != ClientKeyKind::Release {
        key.key_char
            .as_deref()
            .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
            .map(str::to_owned)
    } else {
        None
    };
    Some(ClientPaneInputEvent::Key {
        code,
        modifiers,
        kind,
        repeat_count: 1,
        shifted_codepoint: generated_text
            .as_deref()
            .filter(|_| modifiers == MOD_SHIFT)
            .and_then(|text| text.chars().next())
            .map(u32::from),
        generated_text,
        tracks_release: true,
        physical_key_id: None,
        windows_record: None,
    })
}

/// Pane-local cell position for a window-space point. `body_bounds` are
/// logical px; `size` is the mirror's cell grid.
pub(crate) fn endpoint_cell_position(
    point: (f32, f32),
    body_bounds: (f32, f32, f32, f32),
    size: (u16, u16),
) -> (u16, u16) {
    let cell_w = (body_bounds.2 / f32::from(size.0.max(1))).max(1.);
    let cell_h = (body_bounds.3 / f32::from(size.1.max(1))).max(1.);
    let column =
        ((point.0 - body_bounds.0) / cell_w).clamp(0., f32::from(size.0.saturating_sub(1))) as u16;
    let row =
        ((point.1 - body_bounds.1) / cell_h).clamp(0., f32::from(size.1.saturating_sub(1))) as u16;
    (column, row)
}

/// Pane-local physical-pixel position for `ClientMousePosition::Pixels`.
pub(crate) fn endpoint_pixel_position(
    point: (f32, f32),
    body_bounds: (f32, f32, f32, f32),
    scale_factor: f64,
) -> (u32, u32) {
    (
        ((point.0 - body_bounds.0).max(0.) * scale_factor as f32) as u32,
        ((point.1 - body_bounds.1).max(0.) * scale_factor as f32) as u32,
    )
}

pub(crate) fn endpoint_mouse_geometry(
    size: (u16, u16),
    pixel_size: (u32, u32),
) -> ClientMouseGeometry {
    ClientMouseGeometry {
        cols: size.0,
        rows: size.1,
        width_px: pixel_size.0,
        height_px: pixel_size.1,
    }
}

/// Builds a semantic mouse event for `point` (window coordinates, logical px).
pub(crate) fn endpoint_mouse_event(
    kind: ClientMouseKind,
    point: (f32, f32),
    runtime: &PaneRuntime,
    modifiers: u8,
    lines: u16,
    scale_factor: f64,
) -> ClientPaneInputEvent {
    let (column, row) = endpoint_cell_position(point, runtime.body_bounds, runtime.size);
    let position = if runtime
        .mouse_capture
        .is_some_and(|(_, sgr_pixels)| sgr_pixels)
    {
        let (x, y) = endpoint_pixel_position(point, runtime.body_bounds, scale_factor);
        ClientMousePosition::Pixels { x, y, column, row }
    } else {
        ClientMousePosition::Cell { column, row }
    };
    ClientPaneInputEvent::Mouse {
        kind,
        position,
        geometry: Some(endpoint_mouse_geometry(
            runtime.size,
            // The server's inner_rect pixels describe the geometry the
            // pixel-mouse coordinate space is defined over; fall back to the
            // local framebuffer size before the first frame arrives.
            if runtime.endpoint_pixels.0 > 0 {
                runtime.endpoint_pixels
            } else {
                runtime.pixel_size
            },
        )),
        modifiers,
        lines,
    }
}

pub(crate) fn endpoint_mouse_button(button: SurfaceMouseButton) -> ClientMouseButton {
    match button {
        SurfaceMouseButton::Left => ClientMouseButton::Left,
        SurfaceMouseButton::Right => ClientMouseButton::Right,
        SurfaceMouseButton::Middle => ClientMouseButton::Middle,
    }
}
