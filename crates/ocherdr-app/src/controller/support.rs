use super::*;

/// `pane.swap` answers `{ type: "pane_swap", swap: { changed, .. } }`. A
/// `changed: false` (same pane, cross-tab) means nothing moved, so the
/// prediction must not stay on screen. A result without the field is
/// treated as accepted: older shapes still emit `layout.updated`.
pub(super) fn pane_swap_changed(result: &Value) -> bool {
    let swap = result.get("swap").unwrap_or(result);
    swap.get("changed").and_then(Value::as_bool).unwrap_or(true)
}

/// Herdr wraps `PaneMoveResult` as `{ type: "pane_move", move_result }`.
pub(super) fn pane_move_result(result: &Value) -> &Value {
    result.get("move_result").unwrap_or(result)
}

pub(super) fn pane_move_changed(result: &Value) -> bool {
    pane_move_result(result)
        .get("changed")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Step 1 of the insert must report the created tab and the (possibly
/// re-aliased) pane id; anything else is treated as a failed park.
pub(super) fn parked_pane_from_response(result: &Value) -> Option<ParkedPane> {
    let result = pane_move_result(result);
    if !result
        .get("changed")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        return None;
    }
    let temp_tab_id = result
        .get("created_tab")?
        .get("tab_id")?
        .as_str()?
        .to_owned();
    let pane_id = result.get("pane")?.get("pane_id")?.as_str()?.to_owned();
    Some(ParkedPane {
        temp_tab_id,
        pane_id,
    })
}

pub(super) fn snapshot_pane_ids(snapshot: &HierarchySnapshot) -> HashSet<String> {
    snapshot.pane_ids()
}

pub(super) fn session_terminals_need_rebuild(
    old_tab: Option<&str>,
    old_selected: Option<&str>,
    old_panes: &HashSet<String>,
    selection: &Selection,
    snapshot: &HierarchySnapshot,
    closed_stream: bool,
) -> bool {
    old_tab != selection.tab_id.as_deref()
        || old_selected != selection.pane_id.as_deref()
        || *old_panes != snapshot_pane_ids(snapshot)
        || closed_stream
}

/// Stream and focus wanted for one snapshot pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PaneRuntimeTarget {
    pub(super) pane_id: String,
    pub(super) mode: TerminalMode,
    pub(super) focused: bool,
}

/// Visible panes use the control modes selected by the session lifecycle;
/// everything else observes. Focus remains separate so only the selected
/// local surface gets native Ghostty key handling.
pub(super) fn snapshot_runtime_targets(
    snapshot: &HierarchySnapshot,
    controls: &HashMap<String, TerminalMode>,
    visible_tab_id: Option<&str>,
    selected_pane: Option<&str>,
) -> Vec<PaneRuntimeTarget> {
    snapshot
        .panes
        .iter()
        .map(|pane| {
            let mode = (visible_tab_id == Some(pane.tab_id.as_str()))
                .then(|| controls.get(&pane.pane_id).copied())
                .flatten()
                .unwrap_or(TerminalMode::Observe);
            let focused = selected_pane == Some(pane.pane_id.as_str());
            PaneRuntimeTarget {
                pane_id: pane.pane_id.clone(),
                mode,
                focused,
            }
        })
        .collect()
}

/// Give every newly visible pane one non-takeover control attempt. Successful
/// control resizes the real PTY to the measured OcHerdr viewport. If another
/// client already owns the pane the server refuses the request, and this
/// attempted set prevents an observe/control reconnect loop; an explicit
/// interaction can still take over.
pub(super) fn prime_automatic_terminal_control(
    session: &mut SessionPanes,
    visible_pane_ids: &HashSet<String>,
    live_pane_ids: &HashSet<String>,
) {
    session
        .automatic_control_attempts
        .retain(|pane_id| live_pane_ids.contains(pane_id));
    for pane_id in visible_pane_ids {
        if session.automatic_control_attempts.insert(pane_id.clone()) {
            session
                .controls
                .entry(pane_id.clone())
                .or_insert(TerminalMode::Control);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaneResizePlan {
    /// Grid frozen by a pending relocation or split drag, or pixels unchanged.
    Skip,
    /// Refit the local grid; the endpoint surface resize reports the new
    /// canvas to Herdr, which retiles and answers with each pane's
    /// authoritative `inner_rect`.
    ResizeAndReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaneResizeSchedule {
    /// Geometry is frozen or already committed; invalidate any older timer.
    Cancel,
    /// The same final measurement already owns a timer.
    Keep,
    /// Replace the pending measurement and start a new settle timer.
    Replace,
}

pub(super) fn pane_grid_mountable(cols: u16, rows: u16) -> bool {
    cols >= 4 && rows >= 2
}

/// What a measured pane body means for the terminal: the grid follows the
/// body once it is authoritative. Herdr sizes the shared PTY from the
/// endpoint surface the canvas belongs to and answers with the pane's
/// `inner_rect` cells.
pub(super) fn pane_resize_plan(
    frozen: bool,
    current_pixels: (u32, u32),
    measured_pixels: (u32, u32),
) -> PaneResizePlan {
    if frozen || current_pixels == measured_pixels {
        return PaneResizePlan::Skip;
    }
    PaneResizePlan::ResizeAndReport
}

pub(super) fn pane_resize_schedule(
    frozen: bool,
    current_pixels: (u32, u32),
    pending: Option<PendingPaneResize>,
    measured_pixels: (u32, u32),
    scale_factor: f64,
) -> PaneResizeSchedule {
    if pane_resize_plan(frozen, current_pixels, measured_pixels) == PaneResizePlan::Skip {
        return PaneResizeSchedule::Cancel;
    }
    if pending.is_some_and(|pending| {
        pending.pixels == measured_pixels && pending.scale_factor == scale_factor
    }) {
        PaneResizeSchedule::Keep
    } else {
        PaneResizeSchedule::Replace
    }
}

pub(super) fn should_flush_session_pane(
    pane_tab_id: Option<&str>,
    visible_tab_id: Option<&str>,
    newly_spawned: bool,
) -> bool {
    newly_spawned || pane_tab_id == visible_tab_id
}

pub(super) fn cmd_w_close_target(
    snapshot: &HierarchySnapshot,
    tab_id: &str,
    pane_id: Option<&str>,
) -> Option<HierarchyTarget> {
    if snapshot.panes_for(tab_id).count() > 1 {
        let pane = snapshot.pane(pane_id?)?;
        return Some(HierarchyTarget::Pane {
            id: pane.pane_id.clone(),
            label: pane.display_name().to_owned(),
        });
    }
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id)?;
    Some(HierarchyTarget::Tab {
        id: tab.tab_id.clone(),
        label: tab.label.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionPanesPlan {
    Keep,
    Replace,
}

pub(super) fn session_panes_plan(
    current: Option<&SessionKey>,
    incoming: &SessionKey,
) -> SessionPanesPlan {
    match current {
        Some(owner) if owner == incoming => SessionPanesPlan::Keep,
        _ => SessionPanesPlan::Replace,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VisiblePanePlan {
    Keep,
    PromoteToControl,
    DemoteToObserve,
    Spawn,
}

pub(super) fn visible_pane_plan(
    existing: Option<TerminalMode>,
    existing_closed: bool,
    wanted: TerminalMode,
) -> VisiblePanePlan {
    if existing_closed {
        return VisiblePanePlan::Spawn;
    }
    match existing {
        None => VisiblePanePlan::Spawn,
        Some(current) if current == wanted => VisiblePanePlan::Keep,
        Some(TerminalMode::Observe) => VisiblePanePlan::PromoteToControl,
        Some(TerminalMode::Control | TerminalMode::ControlTakeover) => {
            VisiblePanePlan::DemoteToObserve
        }
    }
}

pub(super) fn snapshot_refresh_should_queue(refreshing: bool) -> bool {
    refreshing
}

pub(super) type InvokeResponseCallback = Box<
    dyn FnOnce(&mut OcHerdrView, std::result::Result<Value, HerdrError>, &mut Context<OcHerdrView>),
>;

// pane.rename emits nothing. pane.close can delete the parent tab and
// reshuffle focus / tab numbers without emitting tab.closed. pane.move and
// pane.swap both emit (`pane.moved` / `layout.updated`), so they stay out.
/// The tab a `tab.create`, `workspace.create`, `worktree.create` or
/// `worktree.open` result names: all four carry Herdr's `TabInfo` as `tab`.
pub(super) fn created_tab_id(result: &Value) -> Option<String> {
    result
        .get("tab")?
        .get("tab_id")?
        .as_str()
        .map(str::to_owned)
}

pub(super) fn command_needs_snapshot_resync(method: &str) -> bool {
    matches!(method, "pane.rename" | "pane.close")
}

pub(super) fn worktree_repo_params(workspace: &WorkspaceInfo) -> serde_json::Map<String, Value> {
    let mut params = serde_json::Map::new();
    match workspace.worktree.as_ref() {
        Some(worktree) if worktree.is_linked_worktree => {
            params.insert("cwd".into(), json!(worktree.repo_root));
        }
        _ => {
            params.insert("workspace_id".into(), json!(workspace.workspace_id));
        }
    }
    params
}

pub(super) fn worktree_create_params(
    workspace: &WorkspaceInfo,
    label: &str,
    branch: &str,
    base: &str,
    path: &str,
) -> Value {
    let mut params = worktree_repo_params(workspace);
    params.insert("focus".into(), json!(true));
    for (key, value) in [
        ("label", label),
        ("branch", branch),
        ("base", base),
        ("path", path),
    ] {
        let value = value.trim();
        if !value.is_empty() {
            params.insert(key.into(), json!(value));
        }
    }
    Value::Object(params)
}

pub(super) fn overlay_after_abandoning_worktree_list(overlay: Overlay) -> Overlay {
    if matches!(overlay, Overlay::WorktreeOpen(_)) {
        Overlay::None
    } else {
        overlay
    }
}

pub(super) fn agent_panel_pane(overlay: &Overlay) -> Option<&str> {
    match overlay {
        Overlay::AgentPanel { pane_id } => Some(pane_id.as_str()),
        _ => None,
    }
}

pub(super) fn agent_panel_target_missing(
    overlay: &Overlay,
    snapshot: Option<&HierarchySnapshot>,
) -> bool {
    let Some(pane_id) = agent_panel_pane(overlay) else {
        return false;
    };
    let Some(snapshot) = snapshot else {
        return true;
    };
    snapshot
        .pane(pane_id)
        .is_none_or(|pane| pane.display_agent.is_none() && pane.agent.is_none())
}

pub(super) fn agent_panel_refresh_needed(
    overlay: &Overlay,
    old_status: Option<AgentStatus>,
    snapshot: Option<&HierarchySnapshot>,
) -> bool {
    agent_panel_pane(overlay).is_some_and(|pane_id| {
        snapshot
            .and_then(|snapshot| snapshot.pane(pane_id))
            .map(|pane| pane.agent_status)
            != old_status
    })
}

pub(super) fn agent_prompt_text_to_send(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

pub(super) fn parse_agent_info_result(value: Value) -> Result<AgentInfo, String> {
    let agent = value
        .get("agent")
        .cloned()
        .ok_or_else(|| "API response is missing `agent`".to_owned())?;
    serde_json::from_value(agent).map_err(|error| format!("invalid `agent`: {error}"))
}

pub(super) fn parse_agent_read_result(value: &Value) -> Result<(String, bool), String> {
    let read = value
        .get("read")
        .ok_or_else(|| "API response is missing `read`".to_owned())?;
    let text = read
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "read is missing `text`".to_owned())?;
    let truncated = read
        .get("truncated")
        .and_then(Value::as_bool)
        .ok_or_else(|| "read is missing `truncated`".to_owned())?;
    Ok((text.to_owned(), truncated))
}

pub(super) fn snapshot_contains_workspace(
    snapshot: Option<&HierarchySnapshot>,
    workspace_id: &str,
) -> bool {
    snapshot.is_some_and(|snapshot| {
        snapshot
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == workspace_id)
    })
}

/// Workspace this picker was opened from. `worktree.open` still sends that id
/// when Herdr returned it, so a missing workspace makes the list unusable.
pub(super) fn worktree_open_target_id(overlay: &Overlay) -> Option<&str> {
    match overlay {
        Overlay::WorktreeOpen(WorktreeOpenState::Loading { workspace_id, .. }) => {
            Some(workspace_id.as_str())
        }
        Overlay::WorktreeOpen(WorktreeOpenState::Ready { source, .. }) => {
            source.source_workspace_id.as_deref()
        }
        _ => None,
    }
}

pub(super) fn worktree_open_target_is_missing(
    overlay: &Overlay,
    snapshot: Option<&HierarchySnapshot>,
) -> bool {
    worktree_open_target_id(overlay)
        .is_some_and(|workspace_id| !snapshot_contains_workspace(snapshot, workspace_id))
}

pub(super) fn worktree_list_applies(
    overlay: &Overlay,
    live_session: Option<&SessionKey>,
    fetched_workspace_id: &str,
    fetched_session: &SessionKey,
    snapshot: Option<&HierarchySnapshot>,
) -> bool {
    let Overlay::WorktreeOpen(WorktreeOpenState::Loading {
        owner,
        workspace_id,
    }) = overlay
    else {
        return false;
    };
    live_session == Some(fetched_session)
        && owner == fetched_session
        && workspace_id == fetched_workspace_id
        && snapshot_contains_workspace(snapshot, fetched_workspace_id)
}

pub(super) fn worktree_open_params(source: &WorktreeSourceInfo, path: &str) -> Value {
    let mut params = json!({ "path": path, "focus": true });
    if let Some(workspace_id) = source.source_workspace_id.as_deref() {
        params["workspace_id"] = json!(workspace_id);
    } else {
        params["cwd"] = json!(source.repo_root);
    }
    params
}

pub(super) fn worktree_remove_params(workspace_id: &str, force: bool) -> Value {
    if force {
        json!({ "workspace_id": workspace_id, "force": true })
    } else {
        json!({ "workspace_id": workspace_id })
    }
}

pub(super) fn dirty_worktree_remove_offer(
    method: &str,
    params: &Value,
    error: &HerdrError,
) -> Option<String> {
    if method != "worktree.remove" {
        return None;
    }
    if params.get("force").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let HerdrError::Api { code, .. } = error else {
        return None;
    };
    if code != "dirty_worktree_requires_force" {
        return None;
    }
    params
        .get("workspace_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(super) fn mouse_point(position: ochub_ui::gpui::Point<ochub_ui::gpui::Pixels>) -> (f32, f32) {
    (f32::from(position.x), f32::from(position.y))
}

pub(super) fn pointer_along_split(
    direction: SplitDirection,
    area: LayoutRect,
    surface: (f32, f32, f32, f32),
    mouse: (f32, f32),
) -> Option<f32> {
    let (sx, sy, sw, sh) = surface;
    if sw <= 0. || sh <= 0. || area.width == 0 || area.height == 0 {
        return None;
    }
    Some(match direction {
        SplitDirection::Right => f32::from(area.x) + (mouse.0 - sx) / sw * f32::from(area.width),
        SplitDirection::Down => f32::from(area.y) + (mouse.1 - sy) / sh * f32::from(area.height),
    })
}

pub(super) fn split_axis_line(split: &LayoutSplit) -> f32 {
    match split.direction {
        SplitDirection::Right => {
            f32::from(split.rect.x) + f32::from(split.rect.width) * split.ratio
        }
        SplitDirection::Down => {
            f32::from(split.rect.y) + f32::from(split.rect.height) * split.ratio
        }
    }
}

pub(super) fn split_drag_from_press(
    tab_id: String,
    split: &LayoutSplit,
    layout: &ocherdr_core::PaneLayout,
    surface: (f32, f32, f32, f32),
    mouse: (f32, f32),
) -> Option<SplitDrag> {
    let path = split.path()?;
    let size = match split.direction {
        SplitDirection::Right => split.rect.width,
        SplitDirection::Down => split.rect.height,
    };
    if size == 0 {
        return None;
    }
    let pointer = pointer_along_split(split.direction, layout.area, surface, mouse)?;
    Some(SplitDrag {
        workspace_id: layout.workspace_id.clone(),
        tab_id,
        path,
        layout: split_layout_fingerprint(layout),
        direction: split.direction,
        rect: split.rect,
        grab_offset: split_axis_line(split) - pointer,
        preview_ratio: split
            .ratio
            .clamp(ocherdr_core::SPLIT_RATIO_MIN, ocherdr_core::SPLIT_RATIO_MAX),
        start_ratio: split.ratio,
    })
}

pub(crate) fn split_layout_fingerprint(
    layout: &ocherdr_core::PaneLayout,
) -> SplitLayoutFingerprint {
    SplitLayoutFingerprint {
        zoomed: layout.zoomed,
        splits: layout
            .splits
            .iter()
            .filter_map(|split| Some((split.path()?, split.direction)))
            .collect(),
        panes: layout
            .panes
            .iter()
            .map(|pane| pane.pane_id.clone())
            .collect(),
    }
}

pub(super) fn split_drag_survives_layout(drag: &SplitDrag, snapshot: &HierarchySnapshot) -> bool {
    let Some(layout) = snapshot.layout_for(&drag.tab_id) else {
        return false;
    };
    if split_layout_fingerprint(layout) != drag.layout {
        return false;
    }
    // PaneCreated/PaneClosed update snapshot.panes before layout.updated.
    let mut live: Vec<&str> = snapshot
        .panes_for(&drag.tab_id)
        .map(|pane| pane.pane_id.as_str())
        .collect();
    let mut expected: Vec<&str> = drag.layout.panes.iter().map(String::as_str).collect();
    live.sort();
    expected.sort();
    live == expected
}

pub(super) fn split_drag_voided_by_pane(
    drag: &SplitDrag,
    workspace_id: Option<&str>,
    tab_id: Option<&str>,
) -> bool {
    match (workspace_id, tab_id) {
        (Some(workspace_id), Some(tab_id)) => {
            tab_id != drag.tab_id || workspace_id != drag.workspace_id
        }
        _ => true,
    }
}

/// The requests a released drag sends: the `pinned_ratios` output (dragged
/// split first) when the dragged split moved, nothing otherwise, since the
/// descendants are only retuned for that move.
pub(super) fn split_commit_ratios(
    ratios: Vec<(Vec<bool>, f32)>,
    start_ratio: f32,
) -> Vec<(Vec<bool>, f32)> {
    match ratios.first() {
        Some((_, dragged)) if (dragged - start_ratio).abs() > f32::EPSILON => ratios,
        _ => Vec::new(),
    }
}

pub(super) fn reconcile_split_drag_state(
    drag: SplitDrag,
    snapshot: Option<&HierarchySnapshot>,
) -> SurfaceDrag {
    if snapshot.is_some_and(|snapshot| split_drag_survives_layout(&drag, snapshot)) {
        SurfaceDrag::Split(drag)
    } else {
        SurfaceDrag::Idle
    }
}

pub(super) fn reorder_live_ids(
    list: &ReorderList,
    snapshot: &HierarchySnapshot,
) -> Option<Vec<String>> {
    match list {
        ReorderList::Workspaces => Some(
            snapshot
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace_id.clone())
                .collect(),
        ),
        ReorderList::Tabs { workspace_id } => {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| &workspace.workspace_id == workspace_id)?;
            Some(
                snapshot
                    .tabs_for(workspace_id)
                    .map(|tab| tab.tab_id.clone())
                    .collect(),
            )
        }
    }
}

pub(super) fn reconcile_reorder_drag_state(
    drag: ReorderDrag,
    snapshot: Option<&HierarchySnapshot>,
) -> SurfaceDrag {
    let Some(live) = snapshot.and_then(|snapshot| reorder_live_ids(&drag.list, snapshot)) else {
        return SurfaceDrag::Idle;
    };
    if live == drag.order {
        SurfaceDrag::Reorder(drag)
    } else {
        SurfaceDrag::Idle
    }
}

pub(super) fn apply_split_drag_pointer(
    mut drag: SplitDrag,
    snapshot: Option<&HierarchySnapshot>,
    surface: Option<(f32, f32, f32, f32)>,
    mouse: (f32, f32),
) -> SplitDrag {
    let Some(surface) = surface else {
        return drag;
    };
    let Some(area) = snapshot
        .and_then(|snapshot| snapshot.layout_for(&drag.tab_id))
        .map(|layout| layout.area)
    else {
        return drag;
    };
    let Some(pointer) = pointer_along_split(drag.direction, area, surface, mouse) else {
        return drag;
    };
    drag.preview_ratio =
        split_ratio_from_drag(drag.direction, drag.rect, pointer + drag.grab_offset);
    drag
}

pub(crate) fn gpui_key_modifiers(modifiers: ochub_ui::gpui::Modifiers) -> KeyModifiers {
    KeyModifiers {
        control: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
        platform: modifiers.platform,
    }
}

pub(super) fn point_in_rect(point: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    point.0 >= rect.0 && point.1 >= rect.1 && point.0 < rect.0 + rect.2 && point.1 < rect.1 + rect.3
}

pub(super) struct FittedSurface {
    origin: (f32, f32),
    fitted: (f32, f32),
    surface: (f32, f32),
}

pub(super) fn fitted_surface(
    body: (f32, f32, f32, f32),
    pixel_size: (u32, u32),
    scale_factor: f32,
) -> Option<FittedSurface> {
    let (bx, by, bw, bh) = body;
    if bw <= 0. || bh <= 0. || pixel_size.0 == 0 || pixel_size.1 == 0 {
        return None;
    }
    let image_w = pixel_size.0 as f32;
    let image_h = pixel_size.1 as f32;
    let image_ratio = image_w / image_h;
    let bounds_ratio = bw / bh;
    let (fitted_w, fitted_h) = if bounds_ratio > image_ratio {
        (image_w * (bh / image_h), bh)
    } else {
        (bw, image_h * (bw / image_w))
    };
    if fitted_w <= 0. || fitted_h <= 0. {
        return None;
    }
    let scale = scale_factor.max(1.);
    Some(FittedSurface {
        origin: (bx + (bw - fitted_w) / 2., by + (bh - fitted_h) / 2.),
        fitted: (fitted_w, fitted_h),
        surface: (image_w / scale, image_h / scale),
    })
}

/// Map a window-space click onto Ghostty view points, matching GPUI
/// `ObjectFit::Contain` (device pixels treated as `Pixels` 1:1).
pub(super) fn map_mouse_to_surface(
    mouse: (f32, f32),
    body: (f32, f32, f32, f32),
    pixel_size: (u32, u32),
    scale_factor: f32,
) -> Option<(f64, f64)> {
    let fitted = fitted_surface(body, pixel_size, scale_factor)?;
    Some((
        f64::from((mouse.0 - fitted.origin.0) / fitted.fitted.0 * fitted.surface.0),
        f64::from((mouse.1 - fitted.origin.1) / fitted.fitted.1 * fitted.surface.1),
    ))
}

pub(super) fn map_surface_rect_to_window(
    rect: (f64, f64, f64, f64),
    body: (f32, f32, f32, f32),
    pixel_size: (u32, u32),
    scale_factor: f32,
) -> Option<(f32, f32, f32, f32)> {
    let fitted = fitted_surface(body, pixel_size, scale_factor)?;
    if fitted.surface.0 <= 0. || fitted.surface.1 <= 0. {
        return None;
    }
    let left = fitted.origin.0 + (rect.0 as f32) / fitted.surface.0 * fitted.fitted.0;
    let top = fitted.origin.1 + (rect.1 as f32) / fitted.surface.1 * fitted.fitted.1;
    let width = (rect.2 as f32) / fitted.surface.0 * fitted.fitted.0;
    let height = (rect.3 as f32) / fitted.surface.1 * fitted.fitted.1;
    Some((left, top, width, height))
}

pub(super) fn copy_terminal_selection(runtime: &PaneRuntime, cx: &mut Context<OcHerdrView>) {
    if !runtime.terminal.has_selection() {
        return;
    }
    let Some(text) = runtime.terminal.read_selection() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

pub(super) fn flush_pane_surface(runtime: &mut PaneRuntime) {
    runtime.terminal.refresh();
    let _ = Terminal::tick_runtime();
    discard_terminal_input(runtime);
    if let Ok(Some(frame)) = runtime.terminal.try_frame()
        && frame.host_context == runtime.frame_context
    {
        runtime.frame = Some(frame);
    }
}

pub(super) fn logical_scroll_line_height(body_height: f32, rows: u16) -> f32 {
    if rows == 0 || !body_height.is_finite() || body_height <= 0. {
        16.
    } else {
        (body_height / f32::from(rows)).max(1.)
    }
}

pub(super) fn wheel_scroll_lines(delta: ScrollDelta, line_height: f32, leftover: &mut f32) -> i32 {
    match delta {
        ScrollDelta::Lines(delta) => {
            *leftover = 0.;
            delta.y.round() as i32
        }
        ScrollDelta::Pixels(delta) => {
            if !line_height.is_finite() || line_height <= 0. || !f32::from(delta.y).is_finite() {
                *leftover = 0.;
                return 0;
            }
            if !leftover.is_finite() {
                *leftover = 0.;
            }
            *leftover += f32::from(delta.y);
            let lines = (*leftover / line_height).trunc() as i32;
            *leftover -= lines as f32 * line_height;
            lines
        }
    }
}

pub(super) fn current_terminal_palette(appearance: &AppearanceSettings) -> TerminalPalette {
    let dark = theme::is_dark();
    let family = terminal_theme_family(appearance, dark);
    let overlay = crate::theme_ansi::overlay_for(family.as_ref());
    let terminal_theme = family
        .map(|family| if dark { family.dark } else { family.light })
        .unwrap_or_else(theme::current);
    let mut palette = terminal_palette_from_theme(terminal_theme, dark, overlay, appearance);
    palette.ansi = crate::theme_ansi::apply_overrides(palette.ansi, &appearance.palette);
    palette
}

pub(crate) fn terminal_overlay(
    appearance: &AppearanceSettings,
    dark: bool,
) -> crate::theme_ansi::ThemeAnsi {
    crate::theme_ansi::overlay_for(terminal_theme_family(appearance, dark).as_ref())
}

pub(super) fn terminal_theme_family(
    appearance: &AppearanceSettings,
    dark: bool,
) -> Option<theme::ThemeFamily> {
    let family_id = match appearance
        .terminal_theme
        .as_deref()
        .and_then(crate::config::values::ThemeRef::parse)
    {
        None => appearance.theme_family.clone(),
        Some(crate::config::values::ThemeRef::Name(id)) => id,
        Some(crate::config::values::ThemeRef::Pair {
            light,
            dark: dark_id,
        }) => {
            if dark {
                dark_id
            } else {
                light
            }
        }
    };
    theme::find_family(&family_id).or_else(|| theme::find_family(&appearance.theme_family))
}

pub(super) fn terminal_ansi(
    overlay: crate::theme_ansi::ThemeAnsi,
    theme: &theme::Theme,
    dark: bool,
) -> [u32; 16] {
    crate::theme_ansi::resolved_ansi(overlay, theme, dark)
}

pub(super) fn terminal_palette_from_theme(
    theme: ochub_ui::theme::Theme,
    dark: bool,
    overlay: crate::theme_ansi::ThemeAnsi,
    appearance: &AppearanceSettings,
) -> TerminalPalette {
    let font = &appearance.font;
    let colors = &appearance.colors;
    let variant = if dark { overlay.dark } else { overlay.light };
    let background = colors.background.unwrap_or(theme.bg.0);
    let foreground = colors.foreground.unwrap_or(theme.text.0);
    TerminalPalette {
        dark,
        background,
        background_opacity: crate::config::values::opacity_percent_u8(
            appearance.background_opacity,
        ),
        foreground,
        cursor: colors.cursor.unwrap_or(theme.accent.0),
        cursor_text: colors
            .cursor_text
            .or(variant.cursor_text.map(|color| color.0))
            .unwrap_or(background),
        selection: colors.selection.unwrap_or(theme.selection.0),
        selection_foreground: colors
            .selection_foreground
            .or(variant.selection_foreground.map(|color| color.0))
            .unwrap_or(foreground),
        ansi: terminal_ansi(overlay, &theme, dark),
        font_family: font.family.clone(),
        font_size: font.size.round().clamp(1.0, 255.0) as u8,
        font_features: font.features.clone(),
        thicken: font.thicken,
        thicken_strength: font.thicken_strength,
        cell_width: font
            .cell_width
            .map(crate::config::values::MetricModifier::to_config),
        cell_height: font
            .cell_height
            .map(crate::config::values::MetricModifier::to_config),
        padding_x: appearance.window_padding_x,
        padding_y: appearance.window_padding_y,
    }
}

pub(super) fn palette_config_values(palette: &[Option<u32>; 16]) -> Vec<String> {
    palette
        .iter()
        .enumerate()
        .filter_map(|(index, color)| {
            color.map(|value| format!("{index}={}", crate::config::values::Color(value).to_hex()))
        })
        .collect()
}

pub(super) fn visible_pane_ids(
    snapshot: Option<&HierarchySnapshot>,
    tab_id: Option<&str>,
) -> HashSet<String> {
    snapshot
        .zip(tab_id)
        .map(|(snapshot, tab_id)| {
            snapshot
                .panes_for(tab_id)
                .map(|pane| pane.pane_id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Endpoint panes share the session connection; mode flips are bookkeeping
/// only — the server already streams every visible pane.
pub(super) fn sync_pane_session(runtime: &mut PaneRuntime, wanted: TerminalMode, focused: bool) {
    if runtime.focused != focused {
        runtime.terminal.set_focus(focused);
        runtime.focused = focused;
    }
    runtime.mode = wanted;
}

/// Ghostty queues pty writes for input OcHerdr now sends as semantic endpoint
/// events; drain the queue so stale bytes never accumulate.
pub(super) fn discard_terminal_input(runtime: &PaneRuntime) {
    while runtime.terminal.try_input().is_some() {}
}

pub(super) fn merge_settings_persist(
    previous: SettingsPersist,
    next: SettingsPersist,
) -> SettingsPersist {
    SettingsPersist {
        config_error: next.config_error.or(previous.config_error),
        host: merge_host_follow_up(previous.host, next.host),
        rollback: previous.rollback.or(next.rollback),
        domains: PersistDomains {
            connections: previous.domains.connections || next.domains.connections,
            config: previous.domains.config || next.domains.config,
            ui_state: previous.domains.ui_state || next.domains.ui_state,
        },
    }
}

pub(super) fn merge_host_follow_up(
    previous: Option<HostPersistFollowUp>,
    next: Option<HostPersistFollowUp>,
) -> Option<HostPersistFollowUp> {
    match (previous, next) {
        // A queued save/connect already expresses the user's final intent for
        // the write it belongs to — neither a later trivial write nor a
        // background Stay follow-up (SSH import) may steal its slot.
        (
            Some(saved @ HostPersistFollowUp::Saved { then, .. }),
            Some(HostPersistFollowUp::Revertible { .. }),
        )
        | (
            Some(saved @ HostPersistFollowUp::Saved { then, .. }),
            Some(HostPersistFollowUp::Saved {
                then: HostSaveThen::Stay,
                ..
            }),
        ) if then != HostSaveThen::Stay => Some(saved),
        (previous, None) => previous,
        (_, Some(next)) => Some(next),
    }
}

/// A failed write rolls live state back only when nothing else is queued to
/// save the user's latest host intent. Otherwise the earliest snapshot moves
/// onto that queued request.
pub(super) fn persist_failure_rollback(
    pending: &mut Option<SettingsPersist>,
    failed_rollback: Option<HostRollback>,
) -> Option<HostRollback> {
    if let Some(queued) = pending.as_mut().filter(|queued| queued.host.is_some()) {
        queued.rollback = failed_rollback.or(queued.rollback.take());
        return None;
    }
    failed_rollback
}

/// Keep one waiting request. Start a write only when none is in flight.
pub(super) fn enqueue_settings_persist(
    pending: &mut Option<SettingsPersist>,
    in_flight: bool,
    request: SettingsPersist,
) -> Option<SettingsPersist> {
    *pending = Some(match pending.take() {
        Some(previous) => merge_settings_persist(previous, request),
        None => request,
    });
    if in_flight { None } else { pending.take() }
}

pub(super) fn overlay_confirm_or_cancel(event: &KeyDownEvent) -> Option<bool> {
    if event.is_held || event.keystroke.modifiers.modified() {
        return None;
    }
    match event.keystroke.key.as_str() {
        "enter" | "return" => Some(true),
        "escape" => Some(false),
        _ => None,
    }
}

pub(super) fn tab_index_from_keystroke(key: &str, key_char: Option<&str>) -> Option<usize> {
    ochub_ui::shortcuts::navigation_digit(key, key_char)
}

pub(super) fn tab_id_for_shortcut<'a>(
    tabs: impl Iterator<Item = &'a ocherdr_core::TabInfo>,
    number: usize,
) -> Option<String> {
    // Herdr's tab number is a stable identity, not a visual position. It can
    // have gaps after tabs are closed and does not change when tabs are moved.
    // Cmd+1…9 must therefore index the authoritative order used by the tab
    // bar, otherwise an old tab whose number happens to match the shortcut
    // can steal it from the tab displaying that shortcut.
    let tabs = tabs.collect::<Vec<_>>();
    if tabs.is_empty() {
        return None;
    }
    if number == 0 {
        return tabs.last().map(|tab| tab.tab_id.clone());
    }
    tabs.get(number.saturating_sub(1))
        .map(|tab| tab.tab_id.clone())
}
