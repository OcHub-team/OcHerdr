use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use gpui_platform::application;
use ocherdr_core::{
    AgentInfo, AgentNameError, AgentStatus, AgentStatusHandoff, ConnectionProfile, DropEdge,
    DropZone, HerdrEvent, HierarchySnapshot, LayoutNode, LayoutRect, LayoutSplit, PaneInfo,
    PaneLayout, PredictedLayout, PredictedPane, ReorderHover, Selection, SessionSummary,
    SnapshotUpdate, SplitDirection, WorktreeInfo, WorktreeSourceInfo, ZoneRect, drop_zone,
    layout_fingerprint, predict_relocation_steps, predict_swap, rebuild_tree, reorder_insert_index,
    split_ratio_from_drag, split_rect, valid_split_ratio,
};
use ocherdr_herdr::{
    EventSubscription, HerdrError, HostHealthStatus, SessionConnection, TerminalCommand,
    TerminalMode, TerminalSession, attach_command, discover_sessions, open_system_terminal,
    request_socket,
};
use ocherdr_terminal::{KeyModifiers, RenderedFrame, Terminal, TerminalPalette};
use ochub_ui::anim::Transition;
use ochub_ui::components::{
    ButtonSize, ButtonTone, busy_button, button, context_menu, context_menu_item, disabled_button,
    empty_state, field, field_with_error, icon_button_tone, icon_only_button_tone, modal_body,
    modal_card, modal_footer, modal_header, modal_overlay, spinner, status_dot,
};
use ochub_ui::gpui::{
    Animation, AnimationExt, App, AppContext, AssetSource, Bounds, ClipboardItem, Context,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString,
    Task, TextOverflow, TextRun, TitlebarOptions, UTF16Selection, WeakEntity, Window,
    WindowAppearance, WindowBounds, WindowOptions, canvas, div, ease_out_quint, linear_color_stop,
    linear_gradient, point, prelude::*, px, relative, size, surface,
};
use ochub_ui::icons::{IconName, icon};
use ochub_ui::notifications::NotificationHost;
use ochub_ui::text_input::TextInput;
use ochub_ui::{assets, theme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod a11y;
mod config;
mod controller;
use controller::HerdrCapabilities;
mod fonts;
mod host_center;
mod i18n;
mod ime;
mod notify;
mod theme_ansi;
mod ui;

use host_center::{HostCenter, HostCenterEvent, HostRollback, HostSaveThen};
use i18n::{I18n, Language, k};
use notify::{FailureKind, FailureNotice, notification_for};

const SIDEBAR_WIDTH: f32 = 252.;
const HEADER_HEIGHT: f32 = 46.;
const TAB_PILL_HEIGHT: f32 = 28.;
/// Gutter left of the first tab; `pl_3` in the strip's own units.
const TAB_STRIP_LEAD_INSET: f32 = 12.;
const STATUS_BAR_HEIGHT: f32 = 28.;
const PANE_HEADER_HEIGHT: f32 = 26.;
const SPLIT_HANDLE_HIT_PX: f32 = 10.;
const SPLIT_HANDLE_VISUAL_PX: f32 = 4.;
const REORDER_SLOP_PX: f32 = 4.;
const TAB_REORDER_GAP_PX: f32 = 4.;
const REORDER_ANIMATION: Duration = Duration::from_millis(180);
const TAB_CLOSE_ANIMATION: Duration = Duration::from_millis(150);
/// Fade of the ⌘N tab hints when Command is pressed or released.
const TAB_SHORTCUT_ANIMATION: Duration = Duration::from_millis(100);
const TAB_PILL_WIDTH: f32 = 160.;
const TAB_TITLE_ACTION_WELL: f32 = 32.;
const TAB_TITLE_FADE_WIDTH: f32 = 16.;
const TAB_TITLE_FONT_SIZE: f32 = 14.;
const TAB_PREVIEW_DELAY: Duration = Duration::from_millis(900);
const TAB_PREVIEW_HIDE_DELAY: Duration = Duration::from_millis(200);
const TAB_PREVIEW_ANIMATION: Duration = Duration::from_millis(140);
const TAB_PREVIEW_WIDTH: f32 = 320.;
const TAB_PREVIEW_HEIGHT: f32 = 180.;
const TAB_PREVIEW_GAP: f32 = 6.;
const TAB_PREVIEW_MARGIN: f32 = 8.;
// Pane drag (design §5, §10).
const PANE_DRAG_HANDLE_WIDTH: f32 = 20.;
const PANE_DRAG_HANDLE_HEIGHT: f32 = 24.;
const PANE_DRAG_SLOP_PX: f32 = 6.;
const PANE_DRAG_PREVIEW_OPACITY: f32 = 0.92;
const PANE_DRAG_INVALID_OPACITY: f32 = 0.55;
const PANE_DRAG_PREVIEW_SCALE: f32 = 1.015;
const PANE_DRAG_SOURCE_OPACITY: f32 = 0.22;
const PANE_DRAG_LIFT_ANIMATION: Duration = Duration::from_millis(120);
const PANE_DROP_ZONE_ANIMATION: Duration = Duration::from_millis(100);
const PANE_DRAG_RETURN_ANIMATION: Duration = Duration::from_millis(120);
const PANE_SETTLE_ANIMATION: Duration = Duration::from_millis(160);
/// Share of the target pane it keeps on an edge drop (design §5.3: 0.5 in
/// the first version; presets come later).
const PANE_EDGE_DROP_RATIO: f32 = 0.5;
// macOS-style corner hierarchy: compact controls stay tight while sheets and
// panels step up evenly instead of using exaggerated capsule radii.
const CORNER_MODAL: f32 = 14.;
const CORNER_PANEL: f32 = 10.;
const CORNER_CONTROL: f32 = 7.;
const CORNER_COMPACT: f32 = 5.;

struct OcHerdrAssets;

impl AssetSource for OcHerdrAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(assets::load(path))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(assets::list(path))
    }
}

#[derive(Clone)]
struct TabPreviewPane {
    fractions: (f32, f32, f32, f32),
    frame: Option<RenderedFrame>,
}

struct TabPreviewCard {
    tab_id: String,
    title: SharedString,
    panes: Vec<TabPreviewPane>,
    waiting: SharedString,
}

impl TabPreviewCard {
    fn into_element(self) -> impl IntoElement {
        let waiting = self.waiting.clone();
        let pane_tab_id = self.tab_id.clone();
        let panes = self
            .panes
            .iter()
            .enumerate()
            .map(|(index, pane)| {
                let (x, y, width, height) = pane.fractions;
                let waiting = waiting.clone();
                let pane_debug_id = format!("tab-preview-pane-{pane_tab_id}-{index}");
                div()
                    .debug_selector(move || pane_debug_id.clone())
                    .absolute()
                    .left(relative(x))
                    .top(relative(y))
                    .w(relative(width))
                    .h(relative(height))
                    .p(px(2.))
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme::border_strong())
                            .bg(theme::current().bg.rgba())
                            .when_some(pane.frame.clone(), |pane, frame| {
                                pane.child(
                                    surface(frame.pixel_buffer)
                                        .with_frame_lifetime(frame.lifetime)
                                        .object_fit(ObjectFit::Contain)
                                        .size_full(),
                                )
                            })
                            .when(pane.frame.is_none(), |pane| {
                                pane.child(
                                    div().text_xs().text_color(theme::muted()).child(waiting),
                                )
                            }),
                    )
            })
            .collect::<Vec<_>>();
        let has_panes = !panes.is_empty();
        let tab_id = self.tab_id.clone();
        let title_id = self.tab_id.clone();
        div()
            .id((ElementId::from("tab-preview-card"), tab_id.clone()))
            .role(ochub_ui::gpui::Role::Tooltip)
            .aria_label(self.title.clone())
            .debug_selector(move || format!("tab-preview-{tab_id}"))
            .w(px(TAB_PREVIEW_WIDTH))
            .overflow_hidden()
            .rounded(px(CORNER_PANEL))
            .border_1()
            .border_color(theme::border())
            .bg(theme::overlay())
            .shadow(theme::shadow_popover())
            .child(
                div()
                    .debug_selector(move || format!("tab-preview-title-{title_id}"))
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .whitespace_normal()
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(TAB_PREVIEW_HEIGHT))
                    .overflow_hidden()
                    .bg(theme::current().bg.rgba())
                    .children(panes)
                    .when(!has_panes, |preview| {
                        preview.flex().items_center().justify_center().child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(self.waiting.clone()),
                        )
                    }),
            )
            .with_animation(
                (ElementId::from("tab-preview-enter"), self.tab_id.clone()),
                Animation::new(TAB_PREVIEW_ANIMATION).with_easing(ease_out_quint()),
                |card, delta| card.opacity(delta),
            )
    }
}

/// Window-local origin of the preview card. X is centered on the tab and
/// clamped so the card stays inside the window; Y is always below the tab
/// (never flipped above, even for an edge tab).
fn tab_preview_origin(tab_rect: (f32, f32, f32, f32), window_width: f32) -> (f32, f32) {
    let (x, y, width, height) = tab_rect;
    let centered = x + width / 2. - TAB_PREVIEW_WIDTH / 2.;
    let min_x = TAB_PREVIEW_MARGIN;
    let max_x = window_width - TAB_PREVIEW_WIDTH - TAB_PREVIEW_MARGIN;
    let origin_x = if min_x <= max_x {
        centered.clamp(min_x, max_x)
    } else {
        (window_width - TAB_PREVIEW_WIDTH).max(0.) / 2.
    };
    (origin_x, y + height + TAB_PREVIEW_GAP)
}

struct LoadedSession {
    sessions: Vec<SessionSummary>,
    selected: Option<usize>,
    connection: Option<SessionConnection>,
    events: LoadedEvents,
    snapshot: Option<HierarchySnapshot>,
}

enum LoadedEvents {
    Idle,
    Live(EventSubscription),
    Lost(SharedString),
}

impl LoadedEvents {
    fn from_subscribe(result: std::result::Result<EventSubscription, HerdrError>) -> Self {
        match result {
            Ok(events) => Self::Live(events),
            Err(error) => Self::Lost(error.to_string().into()),
        }
    }
}

pub(crate) enum EventStreamState {
    /// No live session, or the selected session is not running.
    Idle,
    Live,
    /// Subscribe failed, or a live stream later died. The snapshot is not live.
    Lost(SharedString),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionKey {
    profile_id: String,
    session_name: String,
}

struct SessionPanes {
    /// Dropping a mismatched owner drops every pane runtime and its listen task.
    owner: SessionKey,
    panes: HashMap<String, PaneRuntime>,
}

impl SessionPanes {
    fn new(owner: SessionKey) -> Self {
        Self {
            owner,
            panes: HashMap::new(),
        }
    }
}

struct PaneRuntime {
    /// Control (`--takeover`) for every pane of the visible tab, so each PTY
    /// follows the grid OcHerdr renders for it; observe for panes of hidden
    /// tabs, whose size Herdr keeps on its own. Snapshot panes stay alive
    /// across tabs so hidden terminals keep their Ghostty surface, last
    /// Metal frame, and stream.
    session: TerminalSession,
    terminal: Terminal,
    frame: Option<RenderedFrame>,
    mode: TerminalMode,
    /// The selected pane: the only one that receives keyboard, IME, and
    /// mouse input and reports terminal focus. Independent of `mode`, since
    /// every visible pane holds a control stream.
    focused: bool,
    size: (u16, u16),
    pixel_size: (u32, u32),
    frame_context: u64,
    color_scheme_dark: bool,
    palette_signature: u64,
    /// Dropping this cancels the pane's await loop.
    listen: Option<Task<()>>,
    /// The Herdr stream ended; keep the last frame until the snapshot drops this pane.
    exit_seen: bool,
    /// Leftover pixel delta from trackpad wheel events, in the pane's Y axis.
    scroll_px: f32,
    /// Terminal body in window coordinates: `(x, y, width, height)`.
    body_bounds: (f32, f32, f32, f32),
}

#[derive(Clone, Debug)]
enum HierarchyTarget {
    Workspace { id: String, label: String },
    Tab { id: String, label: String },
    Pane { id: String, label: String },
}

impl HierarchyTarget {
    fn label(&self) -> &str {
        match self {
            Self::Workspace { label, .. } | Self::Tab { label, .. } | Self::Pane { label, .. } => {
                label
            }
        }
    }

    fn kind_key(&self) -> i18n::Key {
        match self {
            Self::Workspace { .. } => k::COMMON_KIND_WORKSPACE,
            Self::Tab { .. } => k::COMMON_KIND_TAB,
            Self::Pane { .. } => k::COMMON_KIND_PANE,
        }
    }
}

#[derive(Clone, Debug)]
struct HierarchyContextMenu {
    target: HierarchyTarget,
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AppearanceMode {
    System,
    Light,
    #[default]
    Dark,
}

impl AppearanceMode {
    fn theme_mode(self) -> theme::ThemeMode {
        match self {
            Self::System => theme::ThemeMode::System,
            Self::Light => theme::ThemeMode::Light,
            Self::Dark => theme::ThemeMode::Dark,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::System => 0,
            Self::Light => 1,
            Self::Dark => 2,
        }
    }

    fn as_config(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn from_config(value: &str) -> Option<Self> {
        match value.trim() {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BackdropMode {
    Opaque,
    Transparent,
    #[default]
    Blurred,
}

impl BackdropMode {
    fn theme_effect(self) -> theme::ThemeWindowBackground {
        match self {
            Self::Opaque => theme::ThemeWindowBackground::Opaque,
            Self::Transparent => theme::ThemeWindowBackground::Transparent,
            Self::Blurred => theme::ThemeWindowBackground::Blurred,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Opaque => 0,
            Self::Transparent => 1,
            Self::Blurred => 2,
        }
    }

    fn as_config(self) -> &'static str {
        match self {
            Self::Opaque => "opaque",
            Self::Transparent => "transparent",
            Self::Blurred => "blurred",
        }
    }

    fn from_config(value: &str) -> Option<Self> {
        match value.trim() {
            "opaque" => Some(Self::Opaque),
            "transparent" => Some(Self::Transparent),
            "blurred" => Some(Self::Blurred),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct AppearanceSettings {
    theme_family: String,
    terminal_theme: Option<String>,
    mode: AppearanceMode,
    backdrop: BackdropMode,
    background_opacity: f64,
    window_padding_x: u32,
    window_padding_y: u32,
    palette: [Option<u32>; 16],
    font: TerminalFontSettings,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme_family: default_theme_family(),
            terminal_theme: None,
            mode: AppearanceMode::Dark,
            backdrop: BackdropMode::Blurred,
            background_opacity: 1.0,
            window_padding_x: 0,
            window_padding_y: 0,
            palette: [None; 16],
            font: TerminalFontSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct TerminalFontSettings {
    family: String,
    size: f32,
    features: Vec<String>,
    thicken: bool,
    thicken_strength: u8,
    cell_width: Option<config::values::MetricModifier>,
    cell_height: Option<config::values::MetricModifier>,
}

impl Default for TerminalFontSettings {
    fn default() -> Self {
        Self {
            family: String::new(),
            size: 13.0,
            features: Vec::new(),
            thicken: false,
            thicken_strength: 255,
            cell_width: None,
            cell_height: None,
        }
    }
}

fn default_theme_family() -> String {
    theme::DEFAULT_THEME_FAMILY.to_owned()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CellWidthChoice {
    Tight,
    #[default]
    Normal,
    Wide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CellHeightChoice {
    Compact,
    #[default]
    Normal,
    Relaxed,
    Loose,
}

impl CellWidthChoice {
    const ALL: [Self; 3] = [Self::Tight, Self::Normal, Self::Wide];

    fn value(self) -> i8 {
        match self {
            Self::Tight => -10,
            Self::Normal => 0,
            Self::Wide => 10,
        }
    }

    fn metric(self) -> Option<config::values::MetricModifier> {
        match self {
            Self::Normal => None,
            other => Some(config::values::MetricModifier::Percent(f64::from(
                other.value(),
            ))),
        }
    }

    fn matching(metric: Option<config::values::MetricModifier>) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|choice| choice.metric() == metric)
    }
}

impl CellHeightChoice {
    const ALL: [Self; 4] = [Self::Compact, Self::Normal, Self::Relaxed, Self::Loose];

    fn value(self) -> i8 {
        match self {
            Self::Compact => -8,
            Self::Normal => 0,
            Self::Relaxed => 12,
            Self::Loose => 20,
        }
    }

    fn metric(self) -> Option<config::values::MetricModifier> {
        match self {
            Self::Normal => None,
            other => Some(config::values::MetricModifier::Percent(f64::from(
                other.value(),
            ))),
        }
    }

    fn matching(metric: Option<config::values::MetricModifier>) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|choice| choice.metric() == metric)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    connections: Vec<ConnectionProfile>,
    #[serde(default)]
    recent_connection_ids: Vec<String>,
    #[serde(default)]
    host_metadata: HashMap<String, HostMetadata>,
    #[serde(default)]
    host_groups: Vec<String>,
    #[serde(default)]
    host_health: HashMap<String, CachedHostHealth>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct HostMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(default)]
    favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port_override: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_file_override: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    herdr_path_override: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CachedHostHealth {
    status: HostHealthStatus,
    checked_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    herdr_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_count: Option<usize>,
    #[serde(default)]
    latency_ms: u64,
}

#[derive(Clone, Debug)]
enum HostHealthView {
    Checking {
        /// Last completed result, restored if the probe is cancelled.
        previous: Option<Box<HostHealthView>>,
    },
    Checked {
        cached: CachedHostHealth,
        detail: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum HostFilter {
    #[default]
    All,
    Favorites,
    Recent,
    Attention,
    Source(ConnectionSource),
    Group(String),
    Tag(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HostListRevision {
    filter: HostFilter,
    query: String,
    bulk: bool,
    indexes: Vec<usize>,
}

struct OcHerdrView {
    profiles: Vec<ConnectionProfile>,
    profile_index: usize,
    sessions: Vec<SessionSummary>,
    session_index: Option<usize>,
    connection: Option<SessionConnection>,
    /// What the connected Herdr can do, derived from the last full snapshot.
    herdr_capabilities: HerdrCapabilities,
    event_stream: EventStreamState,
    /// Dropping this cancels the session-wide event await loop.
    event_listen: Option<Task<()>>,
    /// Dropping this cancels the per-pane agent-status await loop.
    agent_status_listen: Option<Task<()>>,
    /// In-flight agent-status subscribe that will replace `agent_status_listen`.
    agent_status_rebuild: Option<Task<()>>,
    /// Pane ids the current (or in-flight) agent-status subscribe was built for.
    agent_status_panes: HashSet<String>,
    /// Status events held until the post-subscribe snapshot is installed.
    agent_status_handoff: Option<AgentStatusHandoff<HerdrEvent>>,
    snapshot: Option<HierarchySnapshot>,
    selection: Selection,
    operation: Option<SharedString>,
    notifications: Entity<NotificationHost>,
    focus: FocusHandle,
    /// Focus for the confirm dialogs, so Enter/Esc reach them and nothing
    /// leaks to the terminal underneath while one is open.
    dialog_focus: FocusHandle,
    /// Focus move requested by a context that has no `Window`; the next
    /// render performs it.
    pending_focus: Option<PendingFocus>,
    load_epoch: u64,
    /// Invalidates in-flight snapshot refreshes when the live session is replaced.
    event_epoch: u64,
    snapshot_refreshing: bool,
    snapshot_refresh_pending: bool,
    session_panes: Option<SessionPanes>,
    overlay: Overlay,
    open_select: Option<SharedString>,
    appearance_scroll: ScrollHandle,
    appearance_ui: ui::AppearanceUi,
    tab_scroll: ScrollHandle,
    hovered_tab_id: Option<String>,
    /// Tab this client asked Herdr to create and has not switched to yet:
    /// the response can land before `tab.created` / `pane.created` are
    /// applied, so the switch waits until the tab and a pane exist locally.
    pending_created_tab: Option<String>,
    /// Dropping this cancels a pending show or hide.
    tab_preview_task: Option<Task<()>>,
    /// Tab whose preview is currently painted, after `TAB_PREVIEW_DELAY`.
    tab_preview_id: Option<String>,
    /// Target the in-flight `tab_preview_task` is trying to realize.
    tab_preview_goal: Option<String>,
    /// Mouse is over the preview overlay, so leaving the tab must not hide it.
    tab_preview_hovered: bool,
    tab_close_reveals: HashMap<String, Transition>,
    /// Command is down: the tab strip shows its ⌘N hints.
    command_held: bool,
    /// Opacity of the ⌘N hints, eased toward `command_held`.
    shortcut_reveal: Transition,
    prefix_pending: bool,
    surface_drag: SurfaceDrag,
    /// A released divider drag whose `layout.set_split_ratio` batch is still
    /// landing: the squeeze preview stays on and the tab stays locked until
    /// the authoritative layout carries every ratio of the batch.
    split_commit: Option<PendingSplitCommit>,
    /// The dragged pane's last rendered frame, captured at press before the
    /// source slot is dimmed: the slot body, the floating preview, and the
    /// relocation plan fall back to it on a render where the runtime has no
    /// frame. Dropped once no drag, return flight, or plan needs it.
    pane_drag_snapshot: Option<RenderedFrame>,
    pending_reorder: Option<PendingReorder>,
    /// Optimistic pane relocations keyed by tab. While one is set the tab is
    /// locked: no split drag, no pane drag, no pane close, frozen resizes.
    pane_relocations: HashMap<String, PendingPaneRelocation>,
    /// A cancelled or invalid pane drag flying back to its slot.
    pane_drag_return: Option<PaneDragReturn>,
    /// Operation ids for `RelocationPlan`, so a late response cannot settle a
    /// plan that was replaced.
    pane_relocation_serial: u64,
    /// `pane-edge-relocation` config key: the experimental four-edge drop
    /// (design §13 step 3). Combined with `pane_move_supported()` at press.
    pane_edge_relocation: bool,
    /// Keyboard "move pane" mode (design §11): the selected pane is lifted
    /// and arrows pick the drop target until Enter or Esc.
    pane_keyboard_move: Option<KeyboardPaneMove>,
    /// A pane that was parked in a temporary tab when the connection
    /// dropped. Restored as a `Parked` plan if the reconnect snapshot still
    /// shows the tab holding it (design §7.3).
    parked_recovery: Option<ParkedRecovery>,
    /// Tests that only exercise layout and gestures must not spin up
    /// GhosttyKit: its runtime is single-instance and main-thread bound, so
    /// `Terminal::new` from parallel test threads segfaults and from a lone
    /// test thread hangs in `ghostty_app_update_config`.
    #[cfg(test)]
    headless_terminals: bool,
    reorder_metrics: ReorderMetrics,
    terminal_surface_bounds: Option<(f32, f32, f32, f32)>,
    ime_marked: Option<String>,
    rename_input: Entity<TextInput>,
    worktree_label_input: Entity<TextInput>,
    worktree_branch_input: Entity<TextInput>,
    worktree_base_input: Entity<TextInput>,
    worktree_path_input: Entity<TextInput>,
    agent_name_input: Entity<TextInput>,
    agent_prompt_input: Entity<TextInput>,
    agent_output_scroll: ScrollHandle,
    agent_name: AgentNameState,
    agent_output: AgentOutputState,
    agent_prompts: HashMap<String, AgentPromptPhase>,
    agent_name_error: Option<AgentNameError>,
    /// Mutation tasks stay owned after their panel closes so their real-world
    /// result can still be reported.
    agent_keys: HashMap<String, Task<()>>,
    agent_renames: HashMap<String, Task<()>>,
    /// Dropping this cancels an in-flight `worktree.list`.
    worktree_list_task: Option<Task<()>>,
    appearance: AppearanceSettings,
    config: config::ConfigDocument,
    i18n: I18n,
    host_center: Entity<HostCenter>,
    pending_persist: Option<SettingsPersist>,
    /// Waiting for the current settings write to finish so the next can start.
    persist_task: Option<Task<()>>,
}

/// One waiting settings write. Payload is assembled from live state when the
/// write actually starts; rollback is the last known-good host catalog.
#[derive(Clone, Debug)]
struct SettingsPersist {
    error: Option<FailureKind>,
    host: Option<HostPersistFollowUp>,
    rollback: Option<HostRollback>,
}

#[derive(Clone, Copy, Debug)]
enum HostPersistFollowUp {
    Revertible { error: FailureKind },
    Saved { index: usize, then: HostSaveThen },
}

#[derive(Clone, Debug)]
enum Overlay {
    None,
    NodeManager,
    RemoteForm(RemoteForm),
    Appearance,
    HostSwitcher,
    ContextMenu(HierarchyContextMenu),
    Rename(HierarchyTarget),
    ConfirmClose(HierarchyTarget),
    ConfirmRemoveWorktree {
        workspace_id: String,
        label: String,
        prompt: RemoveWorktreePrompt,
    },
    WorktreeCreate {
        workspace_id: String,
        advanced: bool,
    },
    WorktreeOpen(WorktreeOpenState),
    ConfirmRemoveProfile(String),
    ConfirmSwitchProfile {
        id: String,
        from_hosts: bool,
    },
    ConfirmBulkRemove,
    AgentPanel {
        pane_id: String,
    },
}

enum AgentNameState {
    Idle,
    Loading { _task: Task<()> },
    Ready,
    Failed(String),
}

enum AgentOutputState {
    Idle,
    Loading { _task: Task<()> },
    Ready { text: String, truncated: bool },
    Failed { message: String },
}

enum AgentPromptPhase {
    Sending { _task: Task<()> },
    Sent,
    Blocked { message: String },
    Failed { message: String },
}

#[derive(Clone, Debug)]
enum RemoveWorktreePrompt {
    Safe,
    Force { error: String },
}

#[derive(Clone, Debug)]
enum WorktreeOpenState {
    Loading {
        owner: SessionKey,
        workspace_id: String,
    },
    Ready {
        source: WorktreeSourceInfo,
        worktrees: Vec<WorktreeInfo>,
    },
    Failed {
        error: String,
    },
}

/// Where focus goes on the next render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingFocus {
    /// The confirm dialog that just opened.
    Dialog,
    /// The terminal surface, after a dialog closed.
    Surface,
}

impl Overlay {
    /// The confirm dialogs: one destructive/primary action, one Cancel, no
    /// text input of their own. They share focus handling and key hints.
    fn is_confirm_dialog(&self) -> bool {
        matches!(
            self,
            Self::ConfirmClose(_)
                | Self::ConfirmRemoveWorktree { .. }
                | Self::ConfirmRemoveProfile(_)
                | Self::ConfirmSwitchProfile { .. }
                | Self::ConfirmBulkRemove
        )
    }

    fn host_center(&self) -> bool {
        matches!(
            self,
            Self::NodeManager
                | Self::RemoteForm(_)
                | Self::ConfirmRemoveProfile(_)
                | Self::ConfirmBulkRemove
                | Self::ConfirmSwitchProfile {
                    from_hosts: true,
                    ..
                }
        )
    }
}

fn key_goes_to_terminal(overlay: &Overlay) -> bool {
    matches!(overlay, Overlay::None)
}

#[derive(Clone, Debug, PartialEq)]
enum SurfaceDrag {
    Idle,
    Text { pane_id: String, captured: bool },
    Split(SplitDrag),
    Reorder(ReorderDrag),
    Pane(PaneDrag),
}

/// A pane grabbed by its title-bar handle (design §5).
#[derive(Clone, Debug, PartialEq)]
struct PaneDrag {
    workspace_id: String,
    tab_id: String,
    pane_id: String,
    /// `layout_fingerprint` of the tab at press. Any structural change to the
    /// tab while dragging cancels the gesture.
    fingerprint: u64,
    origin: (f32, f32),
    pointer: (f32, f32),
    /// Where inside the source pane rect the pointer grabbed it.
    grab_offset: (f32, f32),
    /// Source pane rect in window coordinates at press.
    source_rect: (f32, f32, f32, f32),
    hover: Option<PaneDropHover>,
    /// Whether the four edge zones accept drops: the `pane-edge-relocation`
    /// flag and the connection's `pane.move` capability, read at press.
    edge_drops: bool,
    pressed_at: Instant,
}

/// Keyboard equivalent of the pane drag (design §11). Entered with the
/// prefix key `m`; arrows choose a neighbouring pane, Tab cycles the zone,
/// Enter commits through the same plan machinery, Esc cancels.
#[derive(Clone, Debug, PartialEq)]
struct KeyboardPaneMove {
    workspace_id: String,
    tab_id: String,
    pane_id: String,
    fingerprint: u64,
    /// Chosen target pane and zone; `None` until the first arrow.
    target: Option<PaneDropHover>,
    edge_drops: bool,
}

impl KeyboardPaneMove {
    fn droppable(&self) -> bool {
        self.target
            .as_ref()
            .is_some_and(|hover| hover.droppable(self.edge_drops))
    }
}

/// What survives a disconnect of a relocation that had already parked its
/// pane in a temporary tab.
#[derive(Clone)]
struct ParkedRecovery {
    plan: RelocationPlan,
    temp_tab_id: String,
    moved_pane_id: String,
}

/// The pane and zone under the pointer during a pane drag.
#[derive(Clone, Debug, PartialEq)]
struct PaneDropHover {
    target_pane_id: String,
    zone: DropZone,
    /// Window rect of the target pane, for the highlight.
    target_rect: (f32, f32, f32, f32),
}

impl PaneDropHover {
    fn droppable(&self, edge_drops: bool) -> bool {
        match self.zone {
            DropZone::Center => true,
            DropZone::Left | DropZone::Right | DropZone::Up | DropZone::Down => edge_drops,
        }
    }
}

/// A drop to commit, from the mouse gesture or the keyboard mode.
#[derive(Clone, Debug, PartialEq)]
struct PaneDropRequest {
    workspace_id: String,
    tab_id: String,
    pane_id: String,
    /// `layout_fingerprint` of the tab when the gesture started.
    fingerprint: u64,
    hover: PaneDropHover,
    edge_drops: bool,
}

/// Neighbour of `source` in `direction` for the keyboard move mode: the
/// pane sharing the longest edge on that side, else the nearest pane whose
/// centre lies on that side. `None` when nothing is there.
fn keyboard_neighbour(layout: &PaneLayout, source: &str, direction: DropEdge) -> Option<String> {
    let source_rect = layout
        .panes
        .iter()
        .find(|pane| pane.pane_id == source)?
        .rect;
    let centre = |rect: LayoutRect| {
        (
            f32::from(rect.x) + f32::from(rect.width) / 2.,
            f32::from(rect.y) + f32::from(rect.height) / 2.,
        )
    };
    let (sx, sy) = centre(source_rect);
    let overlap = |a0: u16, a1: u16, b0: u16, b1: u16| -> i32 {
        i32::from(a1.min(b1)) - i32::from(a0.max(b0))
    };
    let mut best: Option<(i32, f32, &str)> = None;
    for pane in layout.panes.iter().filter(|pane| pane.pane_id != source) {
        let r = pane.rect;
        let (cx, cy) = centre(r);
        let (on_side, shared) = match direction {
            DropEdge::Left => (
                cx < sx,
                overlap(
                    source_rect.y,
                    source_rect.y + source_rect.height,
                    r.y,
                    r.y + r.height,
                ),
            ),
            DropEdge::Right => (
                cx > sx,
                overlap(
                    source_rect.y,
                    source_rect.y + source_rect.height,
                    r.y,
                    r.y + r.height,
                ),
            ),
            DropEdge::Up => (
                cy < sy,
                overlap(
                    source_rect.x,
                    source_rect.x + source_rect.width,
                    r.x,
                    r.x + r.width,
                ),
            ),
            DropEdge::Down => (
                cy > sy,
                overlap(
                    source_rect.x,
                    source_rect.x + source_rect.width,
                    r.x,
                    r.x + r.width,
                ),
            ),
        };
        if !on_side {
            continue;
        }
        let distance = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
        let candidate = (shared.max(0), -distance, pane.pane_id.as_str());
        if best.is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1)) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, id)| id.to_owned())
}

/// Tab cycles the drop zone in the keyboard move mode.
fn next_keyboard_zone(zone: DropZone, edge_drops: bool) -> DropZone {
    if !edge_drops {
        return DropZone::Center;
    }
    match zone {
        DropZone::Center => DropZone::Left,
        DropZone::Left => DropZone::Right,
        DropZone::Right => DropZone::Up,
        DropZone::Up => DropZone::Down,
        DropZone::Down => DropZone::Center,
    }
}

/// Where a released-but-not-dropped preview flies back from (design §10:
/// "invalid drop / cancel → preview returns to the source rect, 120 ms").
#[derive(Clone, Debug, PartialEq)]
struct PaneDragReturn {
    pane_id: String,
    from: (f32, f32, f32, f32),
    to: (f32, f32, f32, f32),
    started: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RelocationIntent {
    Swap,
    /// Design §4.2: park in a new tab, move back beside the target, and for
    /// left/up swap the two leaves. `ratio` is the target's share.
    Insert {
        edge: DropEdge,
        ratio: f32,
    },
}

impl RelocationIntent {
    /// Whether the orchestration needs the third `pane.swap` request.
    fn corrects_order(self) -> bool {
        matches!(self, Self::Insert { edge, .. } if edge.moved_pane_is_first())
    }
}

/// The shapes the target tab passes through during an insert (design
/// §7.3): each authoritative `layout.updated` is classified against these.
#[derive(Clone, Debug, PartialEq)]
struct InsertShapes {
    /// After step 1: the source removed, its parent split collapsed.
    removed: SplitLayoutFingerprint,
    /// After step 2: the source as the target's second child.
    inserted: SplitLayoutFingerprint,
    /// After step 3 (or step 2 for right/down): the prediction.
    final_shape: SplitLayoutFingerprint,
}

impl InsertShapes {
    fn from_steps(steps: &ocherdr_core::RelocationSteps) -> Self {
        Self {
            removed: predicted_shape(&steps.removed),
            inserted: predicted_shape(&steps.inserted),
            final_shape: predicted_shape(&steps.final_layout),
        }
    }
}

/// Where an observed layout of the target tab sits in the transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutShape {
    /// Still the release-time layout (the event has not arrived).
    Release,
    Removed,
    Inserted,
    Final,
    /// None of the expected shapes: someone else changed the tab.
    Foreign,
}

/// Immutable plan built at release (design §7.1). `predicted_rects` only
/// drives rendering and motion; Herdr's layout stays authoritative.
#[derive(Clone)]
struct RelocationPlan {
    operation_id: u64,
    source_pane_id: String,
    source_tab_id: String,
    target_pane_id: String,
    target_tab_id: String,
    intent: RelocationIntent,
    /// `layout_fingerprint` of the target tab at release.
    fingerprint: u64,
    /// Split topology at release. The authoritative `layout.updated` must
    /// keep the same shape and pane set (only leaves swap) to settle.
    topology: SplitLayoutFingerprint,
    /// Area the predicted rects are expressed in.
    area: LayoutRect,
    predicted_rects: Vec<PredictedPane>,
    visual_snapshot: Option<RenderedFrame>,
    /// Workspace of the source tab: `pane.move`'s `new_tab` destination.
    workspace_id: String,
    /// Tabs of that workspace at release. Step 1 creates one more; events
    /// travel on their own socket, so `tab.created` can land before the
    /// step-1 response names it and `tab.closed` after the step-2 response.
    /// A tab outside this set that holds nothing but the source pane is the
    /// temporary tab whatever the phase knows (see `unlisted_temp_tabs`).
    known_tab_ids: HashSet<String>,
    /// Intermediate shapes of an insert; `None` for a swap.
    insert_shapes: Option<InsertShapes>,
}

/// Design §7.2. Phases before `Settling` keep the tab locked and render the
/// plan's predicted rects; `Parked` shows the authoritative snapshot plus the
/// recovery notice.
#[derive(Clone, Debug, PartialEq)]
enum RelocationPhase {
    /// `pane.swap` sent. Needs both the response and a matching
    /// `layout.updated` before the correction runs.
    Swapping { responded: bool, layout_seen: bool },
    /// Step 1 (`pane.move` to a new tab) in flight.
    Parking,
    /// Step 2 (`pane.move` back beside the target) in flight or answered.
    /// `temp_tab_id` is hidden from the tab strip while this phase lasts.
    Inserting {
        temp_tab_id: String,
        moved_pane_id: String,
        responded: bool,
        layout_seen: bool,
    },
    /// Step 3 (`pane.swap`, left/up) in flight or answered.
    CorrectingOrder { responded: bool, layout_seen: bool },
    /// Step 2 failed: the pane sits in `temp_tab_id`. No prediction, no
    /// lock; the inline notice offers retry / go to tab.
    Parked {
        temp_tab_id: String,
        moved_pane_id: String,
    },
    /// Shells and borders move from the predicted rects to the authoritative
    /// ones (design §10: 120–180 ms, `ease_out_quint`).
    Settling {
        started: Instant,
        from: Vec<(String, (f32, f32, f32, f32))>,
    },
}

impl RelocationPhase {
    /// Predicted rects are on screen and the tab is locked.
    fn locks_tab(&self) -> bool {
        !matches!(self, Self::Parked { .. })
    }

    /// The temporary tab this phase keeps out of the tab strip.
    fn hidden_tab_id(&self) -> Option<&str> {
        match self {
            Self::Inserting { temp_tab_id, .. } => Some(temp_tab_id),
            _ => None,
        }
    }

    fn parked_tab_id(&self) -> Option<&str> {
        match self {
            Self::Parked { temp_tab_id, .. } => Some(temp_tab_id),
            _ => None,
        }
    }
}

/// Pane the first `pane.move` response reports, read back for step 2.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ParkedPane {
    temp_tab_id: String,
    pane_id: String,
}

/// What reaches the insert state machine (design §7.2).
#[derive(Clone, Debug, PartialEq)]
enum RelocationSignal {
    /// Step 1 answered.
    Parked(Option<ParkedPane>),
    /// Step 2 answered (`true` = accepted and changed).
    Inserted(bool),
    /// Step 3 answered.
    Reordered(bool),
    /// The target tab's authoritative layout changed.
    Layout(LayoutShape),
    /// User pressed "Retry" on the parked notice.
    Retry,
}

/// Side effect the controller performs after a transition.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RelocationAction {
    None,
    /// Issue step 2 with the parked pane's ids.
    SendInsert,
    /// Issue step 3.
    SendSwap,
    /// Response and matching layout both in: run the settle correction.
    Settle,
    /// Drop the plan; the authoritative snapshot is what is on screen.
    Revert,
    /// Step 2 failed: show the parked notice, unhide the temp tab.
    Park,
    /// Step 3 failed: the layout is legal but mirrored. Unlock, one notice.
    Misordered,
}

/// Pure transition of the insert phases. `corrects_order` is whether the
/// plan needs step 3 (left/up). Returns the next phase (`None` = plan
/// dropped) and the action to take.
fn advance_insert_phase(
    phase: RelocationPhase,
    signal: RelocationSignal,
    corrects_order: bool,
) -> (Option<RelocationPhase>, RelocationAction) {
    use RelocationAction as A;
    use RelocationPhase as P;
    use RelocationSignal as S;
    match (phase, signal) {
        (P::Parking, S::Parked(Some(parked))) => (
            Some(P::Inserting {
                temp_tab_id: parked.temp_tab_id,
                moved_pane_id: parked.pane_id,
                responded: false,
                layout_seen: false,
            }),
            A::SendInsert,
        ),
        (P::Parking, S::Parked(None)) => (None, A::Revert),
        // `Final` can equal the release shape (two-pane tab, left drop), so
        // it is benign here too: step 1 has not even answered yet.
        (
            P::Parking,
            S::Layout(LayoutShape::Release | LayoutShape::Removed | LayoutShape::Final),
        ) => (Some(P::Parking), A::None),
        (P::Parking, S::Layout(_)) => (None, A::Revert),
        (
            P::Inserting {
                temp_tab_id,
                moved_pane_id,
                layout_seen,
                ..
            },
            S::Inserted(true),
        ) => {
            if corrects_order {
                (
                    Some(P::CorrectingOrder {
                        responded: false,
                        layout_seen: false,
                    }),
                    A::SendSwap,
                )
            } else if layout_seen {
                (
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded: true,
                        layout_seen: true,
                    }),
                    A::Settle,
                )
            } else {
                (
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded: true,
                        layout_seen: false,
                    }),
                    A::None,
                )
            }
        }
        (
            P::Inserting {
                temp_tab_id,
                moved_pane_id,
                ..
            },
            S::Inserted(false),
        ) => (
            Some(P::Parked {
                temp_tab_id,
                moved_pane_id,
            }),
            A::Park,
        ),
        (
            P::Inserting {
                temp_tab_id,
                moved_pane_id,
                responded,
                layout_seen,
            },
            S::Layout(shape),
        ) => {
            let landed = match shape {
                LayoutShape::Inserted => true,
                LayoutShape::Final => !corrects_order,
                _ => false,
            };
            match shape {
                LayoutShape::Release | LayoutShape::Removed => (
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded,
                        layout_seen,
                    }),
                    A::None,
                ),
                LayoutShape::Inserted | LayoutShape::Final if landed && corrects_order => (
                    // Step 2 landed for a left/up plan: still waiting for the
                    // response before the swap goes out.
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded,
                        layout_seen,
                    }),
                    A::None,
                ),
                LayoutShape::Inserted | LayoutShape::Final if landed => (
                    Some(P::Inserting {
                        temp_tab_id,
                        moved_pane_id,
                        responded,
                        layout_seen: true,
                    }),
                    if responded { A::Settle } else { A::None },
                ),
                _ => (None, A::Revert),
            }
        }
        (P::CorrectingOrder { layout_seen, .. }, S::Reordered(true)) => (
            Some(P::CorrectingOrder {
                responded: true,
                layout_seen,
            }),
            if layout_seen { A::Settle } else { A::None },
        ),
        (P::CorrectingOrder { .. }, S::Reordered(false)) => (None, A::Misordered),
        (P::CorrectingOrder { responded, .. }, S::Layout(LayoutShape::Final)) => (
            Some(P::CorrectingOrder {
                responded,
                layout_seen: true,
            }),
            if responded { A::Settle } else { A::None },
        ),
        // Events ride a different socket than responses, so an earlier
        // step's layout can land after a later step answered: every
        // expected intermediate shape is benign here.
        (
            P::CorrectingOrder {
                responded,
                layout_seen,
            },
            S::Layout(LayoutShape::Release | LayoutShape::Removed | LayoutShape::Inserted),
        ) => (
            Some(P::CorrectingOrder {
                responded,
                layout_seen,
            }),
            A::None,
        ),
        (P::CorrectingOrder { .. }, S::Layout(LayoutShape::Foreign)) => (None, A::Revert),
        (
            P::Parked {
                temp_tab_id,
                moved_pane_id,
            },
            S::Retry,
        ) => (
            Some(P::Inserting {
                temp_tab_id,
                moved_pane_id,
                responded: false,
                layout_seen: false,
            }),
            A::SendInsert,
        ),
        // Parked shows the authoritative snapshot: layout changes are fine.
        (phase @ P::Parked { .. }, S::Layout(_)) => (Some(phase), A::None),
        // Stale or out-of-order signals never move the machine.
        (phase, _) => (Some(phase), A::None),
    }
}

/// Pane order and split shape of a predicted layout, for exact comparison
/// with an authoritative `layout.updated`.
fn predicted_shape(layout: &PredictedLayout) -> SplitLayoutFingerprint {
    SplitLayoutFingerprint {
        zoomed: false,
        splits: layout
            .splits
            .iter()
            .map(|split| (split.path.clone(), split.direction))
            .collect(),
        panes: layout
            .panes
            .iter()
            .map(|pane| pane.pane_id.clone())
            .collect(),
    }
}

/// Classify the target tab's authoritative layout against an insert plan.
fn classify_insert_layout(layout: &PaneLayout, plan: &RelocationPlan) -> LayoutShape {
    let Some(shapes) = plan.insert_shapes.as_ref() else {
        return LayoutShape::Foreign;
    };
    // Shapes first: in a two-pane tab the final layout of a left drop has
    // the release-time shape, and a foreign change that happens to produce
    // the expected shape is harmless.
    let shape = controller::split_layout_fingerprint(layout);
    if shape == shapes.final_shape {
        LayoutShape::Final
    } else if shape == shapes.inserted {
        LayoutShape::Inserted
    } else if shape == shapes.removed {
        LayoutShape::Removed
    } else if layout_fingerprint(layout) == plan.fingerprint {
        LayoutShape::Release
    } else {
        LayoutShape::Foreign
    }
}

#[derive(Clone)]
struct PendingPaneRelocation {
    plan: RelocationPlan,
    phase: RelocationPhase,
}

fn pane_drag_past_slop(drag: &PaneDrag) -> bool {
    (drag.pointer.0 - drag.origin.0).abs() > PANE_DRAG_SLOP_PX
        || (drag.pointer.1 - drag.origin.1).abs() > PANE_DRAG_SLOP_PX
}

/// Pane rect in window coordinates from its layout fractions.
fn pane_window_rect(
    layout: &PaneLayout,
    pane_id: &str,
    surface: (f32, f32, f32, f32),
) -> Option<(f32, f32, f32, f32)> {
    let pane = layout.panes.iter().find(|pane| pane.pane_id == pane_id)?;
    let (fx, fy, fw, fh) = layout_rect_fractions(layout.area, pane.rect)?;
    Some(fractions_to_window(surface, (fx, fy, fw, fh)))
}

fn fractions_to_window(
    surface: (f32, f32, f32, f32),
    fractions: (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    (
        surface.0 + fractions.0 * surface.2,
        surface.1 + fractions.1 * surface.3,
        fractions.2 * surface.2,
        fractions.3 * surface.3,
    )
}

fn layout_rect_fractions(area: LayoutRect, rect: LayoutRect) -> Option<(f32, f32, f32, f32)> {
    let area_w = f32::from(area.width);
    let area_h = f32::from(area.height);
    if area_w == 0. || area_h == 0. {
        return None;
    }
    Some((
        (f32::from(rect.x) - f32::from(area.x)) / area_w,
        (f32::from(rect.y) - f32::from(area.y)) / area_h,
        f32::from(rect.width) / area_w,
        f32::from(rect.height) / area_h,
    ))
}

/// Five-zone hit test over every other pane of the tab (design §5.3).
/// The source pane itself is never a target.
fn pane_drop_hover(
    layout: &PaneLayout,
    source_pane_id: &str,
    surface: (f32, f32, f32, f32),
    pointer: (f32, f32),
) -> Option<PaneDropHover> {
    layout
        .panes
        .iter()
        .filter(|pane| pane.pane_id != source_pane_id)
        .find_map(|pane| {
            let rect = pane_window_rect(layout, &pane.pane_id, surface)?;
            let zone = drop_zone(
                ZoneRect {
                    x: rect.0,
                    y: rect.1,
                    width: rect.2,
                    height: rect.3,
                },
                pointer.0,
                pointer.1,
            )?;
            Some(PaneDropHover {
                target_pane_id: pane.pane_id.clone(),
                zone,
                target_rect: rect,
            })
        })
}

/// Top-left of the floating preview: the pointer keeps its grab offset, and
/// the 1.015 scale grows the card around its centre.
fn pane_drag_preview_rect(drag: &PaneDrag) -> (f32, f32, f32, f32) {
    let (w, h) = (drag.source_rect.2, drag.source_rect.3);
    let scaled_w = w * PANE_DRAG_PREVIEW_SCALE;
    let scaled_h = h * PANE_DRAG_PREVIEW_SCALE;
    (
        drag.pointer.0 - drag.grab_offset.0 - (scaled_w - w) / 2.,
        drag.pointer.1 - drag.grab_offset.1 - (scaled_h - h) / 2.,
        scaled_w,
        scaled_h,
    )
}

/// Pane and split geometry of a tab as surface fractions, with the split at
/// `path` drawn at `ratio` instead of its authoritative value (design §5.4).
/// Rects are laid out exactly as Herdr will lay them out for that ratio
/// (`split_rect`: whole cells, first child rounded), so the frame the
/// preview shows at release is the frame the authoritative `layout.updated`
/// brings back; a continuous preview sat up to half a cell away from it and
/// jumped on release.
#[derive(Clone, Debug, PartialEq)]
struct SqueezedLayout {
    panes: Vec<(String, (f32, f32, f32, f32))>,
    splits: Vec<SqueezedSplit>,
}

#[derive(Clone, Debug, PartialEq)]
struct SqueezedSplit {
    path: Vec<bool>,
    rect: (f32, f32, f32, f32),
    /// Divider position along the split axis, as a surface fraction.
    line: f32,
}

impl SqueezedLayout {
    fn pane(&self, pane_id: &str) -> Option<(f32, f32, f32, f32)> {
        self.panes
            .iter()
            .find(|(id, _)| id == pane_id)
            .map(|(_, rect)| *rect)
    }

    fn split(&self, path: &[bool]) -> Option<((f32, f32, f32, f32), f32)> {
        self.splits
            .iter()
            .find(|split| split.path == path)
            .map(|split| (split.rect, split.line))
    }
}

/// The tab's geometry with the given split ratios applied (the dragged
/// split plus the descendants `pinned_ratios` retunes), in whole cells like
/// Herdr, as surface fractions. Ratios are clamped the way Herdr clamps them.
fn squeezed_layout(layout: &PaneLayout, ratios: &[(Vec<bool>, f32)]) -> Option<SqueezedLayout> {
    let mut tree = rebuild_tree(layout)?;
    if layout.area.width == 0 || layout.area.height == 0 {
        return None;
    }
    let clamped: Vec<(Vec<bool>, f32)> = ratios
        .iter()
        .map(|(path, ratio)| (path.clone(), valid_split_ratio(*ratio)))
        .collect();
    ocherdr_core::apply_ratios(&mut tree.root, &clamped);
    let mut out = SqueezedLayout {
        panes: Vec::new(),
        splits: Vec::new(),
    };
    squeeze_node(
        &tree.root,
        layout.area,
        layout.area,
        &mut Vec::new(),
        &mut out,
    );
    Some(out)
}

/// The ratios a divider drag applies: the dragged split at `ratio` and every
/// same-direction descendant retuned so its divider stays on its cell.
fn split_drag_ratios(layout: &PaneLayout, path: &[bool], ratio: f32) -> Vec<(Vec<bool>, f32)> {
    rebuild_tree(layout)
        .map(|tree| ocherdr_core::pinned_ratios(&tree, path, ratio))
        .unwrap_or_default()
}

fn squeeze_node(
    node: &LayoutNode,
    rect: LayoutRect,
    area: LayoutRect,
    current: &mut Vec<bool>,
    out: &mut SqueezedLayout,
) {
    // `area` is non-empty (checked by the caller), so the fractions exist.
    let fractions = |rect| layout_rect_fractions(area, rect).unwrap_or((0., 0., 0., 0.));
    match node {
        LayoutNode::Pane(pane_id) => out.panes.push((pane_id.clone(), fractions(rect))),
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let (first_rect, second_rect) = split_rect(rect, *direction, *ratio);
            let (fx, fy, fw, fh) = fractions(first_rect);
            let line = match direction {
                SplitDirection::Right => fx + fw,
                SplitDirection::Down => fy + fh,
            };
            out.splits.push(SqueezedSplit {
                path: current.clone(),
                rect: fractions(rect),
                line,
            });
            current.push(false);
            squeeze_node(first, first_rect, area, current, out);
            current.pop();
            current.push(true);
            squeeze_node(second, second_rect, area, current, out);
            current.pop();
        }
    }
}

fn lerp_rect(from: (f32, f32, f32, f32), to: (f32, f32, f32, f32), t: f32) -> (f32, f32, f32, f32) {
    (
        from.0 + (to.0 - from.0) * t,
        from.1 + (to.1 - from.1) * t,
        from.2 + (to.2 - from.2) * t,
        from.3 + (to.3 - from.3) * t,
    )
}

/// Whether the authoritative layout is the one the plan predicted: same
/// split shape, same pane set, and the fingerprint has moved on from the
/// release-time value (so a ratio-only `layout.updated` is not mistaken for
/// the swap landing).
fn layout_settles_plan(layout: &PaneLayout, plan: &RelocationPlan) -> bool {
    if layout_fingerprint(layout) == plan.fingerprint {
        return false;
    }
    if layout.zoomed != plan.topology.zoomed {
        return false;
    }
    let splits: Vec<(Vec<bool>, SplitDirection)> = layout
        .splits
        .iter()
        .filter_map(|split| Some((split.path()?, split.direction)))
        .collect();
    if splits != plan.topology.splits {
        return false;
    }
    let mut live: Vec<&str> = layout
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_str())
        .collect();
    let mut expected: Vec<&str> = plan.topology.panes.iter().map(String::as_str).collect();
    live.sort_unstable();
    expected.sort_unstable();
    live == expected
}

/// Whether the tab still looks like it did when the plan was made, i.e. the
/// authoritative event has not arrived yet.
fn layout_still_matches_plan(layout: &PaneLayout, plan: &RelocationPlan) -> bool {
    layout_fingerprint(layout) == plan.fingerprint
}

impl RelocationPlan {
    /// Temporary tabs of an insert as the event stream reports them: tabs of
    /// the plan's workspace that did not exist at release and hold nothing
    /// but the source pane. Hidden from the tab strip and tab navigation
    /// alongside the id the step-1 response names (design §7.2), so the
    /// tab never flashes for the frames between the event and the response.
    fn unlisted_temp_tabs<'a>(
        &'a self,
        snapshot: &'a HierarchySnapshot,
    ) -> impl Iterator<Item = &'a str> + 'a {
        let inserting = matches!(self.intent, RelocationIntent::Insert { .. });
        snapshot
            .tabs_for(&self.workspace_id)
            .filter(move |tab| inserting && !self.known_tab_ids.contains(&tab.tab_id))
            .filter(|tab| {
                snapshot
                    .panes_for(&tab.tab_id)
                    .all(|pane| pane.pane_id == self.source_pane_id)
            })
            .map(|tab| tab.tab_id.as_str())
    }

    /// Both intents are same-tab only: the drop model never offers a pane
    /// of another tab as a target.
    fn is_supported(&self) -> bool {
        self.source_tab_id == self.target_tab_id
            && match self.intent {
                RelocationIntent::Swap => true,
                RelocationIntent::Insert { .. } => self.insert_shapes.is_some(),
            }
    }

    /// Pane ids the prediction lays out, in tree order.
    fn predicted_pane_ids(&self) -> impl Iterator<Item = &str> {
        self.predicted_rects
            .iter()
            .map(|pane| pane.pane_id.as_str())
    }

    /// Frame to show for the source pane while the plan is pending, when the
    /// runtime has none of its own yet.
    fn frame_for(&self, pane_id: &str) -> Option<RenderedFrame> {
        (pane_id == self.source_pane_id)
            .then(|| self.visual_snapshot.clone())
            .flatten()
    }

    fn predicted_fractions(&self) -> Vec<(String, (f32, f32, f32, f32))> {
        self.predicted_rects
            .iter()
            .filter_map(|pane| {
                Some((
                    pane.pane_id.clone(),
                    layout_rect_fractions(self.area, pane.rect)?,
                ))
            })
            .collect()
    }
}

#[cfg(test)]
struct SettlingSeed {
    plan: RelocationPlan,
    from: Vec<(String, (f32, f32, f32, f32))>,
}

#[cfg(test)]
impl SettlingSeed {
    fn into_settling(self, started: Instant) -> PendingPaneRelocation {
        PendingPaneRelocation {
            plan: self.plan,
            phase: RelocationPhase::Settling {
                started,
                from: self.from,
            },
        }
    }
}

impl PendingPaneRelocation {
    /// Pane fractions to render for this tab right now, or `None` when the
    /// pane is not part of the plan.
    fn display_fractions(
        &self,
        pane_id: &str,
        layout: Option<&PaneLayout>,
        now: Instant,
        reduce_motion: bool,
    ) -> Option<(f32, f32, f32, f32)> {
        match &self.phase {
            RelocationPhase::Swapping { .. }
            | RelocationPhase::Parking
            | RelocationPhase::Inserting { .. }
            | RelocationPhase::CorrectingOrder { .. } => self
                .plan
                .predicted_fractions()
                .into_iter()
                .find(|(id, _)| id == pane_id)
                .map(|(_, rect)| rect),
            RelocationPhase::Parked { .. } => None,
            RelocationPhase::Settling { started, from } => {
                let from = from.iter().find(|(id, _)| id == pane_id).map(|(_, r)| *r)?;
                let to = layout.and_then(|layout| {
                    let pane = layout.panes.iter().find(|pane| pane.pane_id == pane_id)?;
                    layout_rect_fractions(layout.area, pane.rect)
                })?;
                let progress = ochub_ui::anim::linear_progress(
                    *started,
                    PANE_SETTLE_ANIMATION,
                    now,
                    reduce_motion,
                );
                Some(lerp_rect(
                    from,
                    to,
                    ochub_ui::anim::ease_out_quint(progress),
                ))
            }
        }
    }

    fn is_settled(&self, now: Instant, reduce_motion: bool) -> bool {
        match &self.phase {
            RelocationPhase::Settling { started, .. } => {
                reduce_motion || now.saturating_duration_since(*started) >= PANE_SETTLE_ANIMATION
            }
            _ => false,
        }
    }
}

/// A reorder Herdr has accepted but not yet confirmed with a `moved` event.
/// While it is set the lists refuse new reorders: their indices would be
/// computed from an order Herdr is about to replace. Holding the request task
/// here is what keeps it alive, so dropping this drops the request too.
struct PendingReorder {
    _request: Task<()>,
    /// Release-time projection kept on screen until Herdr publishes the
    /// authoritative order. Tabs and workspaces share this settle.
    display: Option<PendingListReorder>,
}

#[derive(Clone, Debug, PartialEq)]
struct PendingListReorder {
    list: ReorderList,
    order: Vec<String>,
    source_index: usize,
    hover: ReorderHover,
    /// Window-local origin of the drag ghost at mouse-up. The real row starts
    /// here and settles into the projected empty slot.
    released_origin: (f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
struct ReorderDrag {
    list: ReorderList,
    source_index: usize,
    /// Ids in list order at press. Membership or order change cancels.
    order: Vec<String>,
    /// The prior gap lets each new declarative animation start where the last
    /// one ended instead of replaying from the authoritative layout.
    previous_hover: ReorderHover,
    hover: ReorderHover,
    origin: (f32, f32),
    pointer: (f32, f32),
    /// Where inside the source row the pointer grabbed it. Measured at press,
    /// so the drag cannot exist before the row has been laid out.
    grab_offset: (f32, f32),
    /// Slot rect at press. Ghost size stays on this even if the live canvas
    /// span is rewritten by the squeeze animation.
    source_rect: (f32, f32, f32, f32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReorderList {
    Workspaces,
    Tabs { workspace_id: String },
}

#[derive(Clone, Debug, Default)]
struct ReorderMetrics {
    workspaces: Vec<ReorderSpan>,
    tabs: Vec<ReorderSpan>,
}

#[derive(Clone, Debug)]
struct ReorderSpan {
    id: String,
    rect: (f32, f32, f32, f32),
}

fn reorder_past_slop(drag: &ReorderDrag) -> bool {
    (drag.pointer.0 - drag.origin.0).abs() > REORDER_SLOP_PX
        || (drag.pointer.1 - drag.origin.1).abs() > REORDER_SLOP_PX
}

fn reorder_display_positions(
    order: &[String],
    source_index: usize,
    hover: ReorderHover,
) -> Vec<usize> {
    let mut positions = (0..order.len()).collect::<Vec<_>>();
    let Some(insert_index) = reorder_insert_index(order.len(), source_index, hover) else {
        return positions;
    };
    let destination = if insert_index > source_index {
        insert_index - 1
    } else {
        insert_index
    };
    positions[source_index] = destination;
    if destination < source_index {
        for position in &mut positions[destination..source_index] {
            *position += 1;
        }
    } else {
        for position in &mut positions[source_index + 1..=destination] {
            *position -= 1;
        }
    }
    positions
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReorderAxis {
    Horizontal,
    Vertical,
}

fn reorder_axis(list: &ReorderList) -> ReorderAxis {
    match list {
        ReorderList::Workspaces => ReorderAxis::Vertical,
        ReorderList::Tabs { .. } => ReorderAxis::Horizontal,
    }
}

/// Pixel shift along the list axis for each original index. `spans` are
/// `(origin, extent)` in original order. Horizontal tabs and vertical
/// workspaces pass the same numbers through this function.
fn reorder_display_shifts(spans: &[(f32, f32)], positions: &[usize], gap: f32) -> Vec<f32> {
    let mut originals_by_position = vec![0; positions.len()];
    for (original, position) in positions.iter().copied().enumerate() {
        originals_by_position[position] = original;
    }
    let mut target = spans[0].0;
    let mut shifts = vec![0.; positions.len()];
    for original in originals_by_position {
        shifts[original] = target - spans[original].0;
        target += spans[original].1 + gap;
    }
    shifts
}

fn reorder_axis_offset(shift: f32, axis: ReorderAxis) -> (f32, f32) {
    match axis {
        ReorderAxis::Horizontal => (shift, 0.),
        ReorderAxis::Vertical => (0., shift),
    }
}

fn reorder_list_bounds(rects: &[(f32, f32, f32, f32)]) -> (f32, f32, f32, f32) {
    let mut min_x = rects[0].0;
    let mut min_y = rects[0].1;
    let mut max_x = rects[0].0 + rects[0].2;
    let mut max_y = rects[0].1 + rects[0].3;
    for rect in &rects[1..] {
        min_x = min_x.min(rect.0);
        min_y = min_y.min(rect.1);
        max_x = max_x.max(rect.0 + rect.2);
        max_y = max_y.max(rect.1 + rect.3);
    }
    (min_x, min_y, max_x - min_x, max_y - min_y)
}

/// Ghost origin so the dragged row stays on its list axis.
///
/// Tabs lock `top` to the strip and clamp `left`; workspaces lock `left` to
/// the sidebar and clamp `top`. Drop targeting still uses the pointer's
/// coordinate along that same axis, including when the pointer has left the
/// strip — there is no tear-out in this round.
fn reorder_ghost_origin(
    pointer: (f32, f32),
    grab_offset: (f32, f32),
    list: (f32, f32, f32, f32),
    ghost_size: (f32, f32),
    axis: ReorderAxis,
) -> (f32, f32) {
    let free = (pointer.0 - grab_offset.0, pointer.1 - grab_offset.1);
    match axis {
        ReorderAxis::Horizontal => {
            let max_x = (list.0 + list.2 - ghost_size.0).max(list.0);
            (free.0.clamp(list.0, max_x), list.1)
        }
        ReorderAxis::Vertical => {
            let max_y = (list.1 + list.3 - ghost_size.1).max(list.1);
            (list.0, free.1.clamp(list.1, max_y))
        }
    }
}

struct ReorderSlotOffsets {
    previous: Vec<(f32, f32)>,
    current: Vec<(f32, f32)>,
}

fn reorder_slot_offsets(
    source_index: usize,
    motion: ReorderMotion,
    positions: &[usize],
    previous_positions: &[usize],
    rects: &[(f32, f32, f32, f32)],
    gap: f32,
    axis: ReorderAxis,
) -> ReorderSlotOffsets {
    let along = |rect: (f32, f32, f32, f32)| match axis {
        ReorderAxis::Horizontal => (rect.0, rect.2),
        ReorderAxis::Vertical => (rect.1, rect.3),
    };
    let spans = rects.iter().copied().map(along).collect::<Vec<_>>();
    let to_offsets = |positions: &[usize]| {
        reorder_display_shifts(&spans, positions, gap)
            .into_iter()
            .map(|shift| reorder_axis_offset(shift, axis))
            .collect::<Vec<_>>()
    };
    let mut previous = to_offsets(previous_positions);
    if let ReorderMotion::Settling { released_origin } = motion {
        previous[source_index] = (
            released_origin.0 - rects[source_index].0,
            released_origin.1 - rects[source_index].1,
        );
    }
    ReorderSlotOffsets {
        previous,
        current: to_offsets(positions),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ReorderMotion {
    Dragging,
    Settling { released_origin: (f32, f32) },
}

#[derive(Clone, Debug, PartialEq)]
struct ReorderProjection {
    source_id: String,
    source_index: usize,
    positions: Vec<usize>,
    previous_positions: Vec<usize>,
    motion: ReorderMotion,
}

/// Derive display positions without mutating the authoritative snapshot. The
/// same mapping drives both pointer-time squeezing and the request-time settle
/// for tabs and workspaces. A changed authoritative order always wins over a
/// prediction based on stale input, including an order published by another
/// client.
fn reorder_projection(
    list: &ReorderList,
    authoritative_order: &[String],
    drag: Option<&ReorderDrag>,
    pending: Option<&PendingListReorder>,
) -> Option<ReorderProjection> {
    let dragging = drag.and_then(|drag| {
        if drag.list != *list || !reorder_past_slop(drag) {
            return None;
        }
        Some((
            drag.order.as_slice(),
            drag.source_index,
            drag.previous_hover,
            drag.hover,
            ReorderMotion::Dragging,
        ))
    });
    let pending = pending.and_then(|pending| {
        (pending.list == *list).then_some((
            pending.order.as_slice(),
            pending.source_index,
            pending.hover,
            pending.hover,
            ReorderMotion::Settling {
                released_origin: pending.released_origin,
            },
        ))
    });
    let (order, source_index, previous_hover, hover, motion) = dragging.or(pending)?;
    if order != authoritative_order {
        return None;
    }
    let source_id = order.get(source_index)?.clone();
    Some(ReorderProjection {
        source_id,
        source_index,
        positions: reorder_display_positions(order, source_index, hover),
        previous_positions: reorder_display_positions(order, source_index, previous_hover),
        motion,
    })
}

#[derive(Clone, Debug, PartialEq)]
struct SplitDrag {
    workspace_id: String,
    tab_id: String,
    path: Vec<bool>,
    /// Topology at press. Ratio-derived geometry is omitted so a nested
    /// ancestor `layout.updated` does not void the gesture.
    layout: SplitLayoutFingerprint,
    direction: SplitDirection,
    rect: LayoutRect,
    grab_offset: f32,
    preview_ratio: f32,
    start_ratio: f32,
}

/// A divider drag that has been released: the ratios sent to Herdr, kept
/// as the squeeze preview until the last `layout.updated` of the batch
/// lands, so the intermediate layouts (dragged split moved, nested ones
/// not yet retuned) never flash on screen.
#[derive(Clone, Debug, PartialEq)]
struct PendingSplitCommit {
    tab_id: String,
    /// Topology at release; any other change voids the preview.
    layout: SplitLayoutFingerprint,
    /// Dragged split first, then the retuned descendants (request order).
    ratios: Vec<(Vec<bool>, f32)>,
    /// Distinguishes a late response from a replaced commit.
    serial: u64,
    /// Requests still without a response.
    outstanding: usize,
    /// Split ratios of the tab's layout as last seen, and how many ratio
    /// changes landed since release: Herdr emits one `layout.updated` per
    /// request, so once every request is answered and as many changes have
    /// landed the batch is over even if Herdr kept other ratios.
    last_ratios: Vec<f32>,
    layouts_seen: usize,
}

impl PendingSplitCommit {
    /// Whether every ratio of the batch is what the authoritative layout
    /// shows (within the f32 → JSON → f32 round trip).
    fn landed(&self, layout: &PaneLayout) -> bool {
        self.ratios.iter().all(|(path, ratio)| {
            layout.splits.iter().any(|split| {
                split.path().as_deref() == Some(path) && (split.ratio - ratio).abs() < 1e-4
            })
        })
    }

    /// Count a layout whose ratios differ from the last one seen. Returns
    /// whether the batch is over: every ratio landed, or every request is
    /// answered and one layout per request has come in.
    fn observe(&mut self, layout: &PaneLayout) -> bool {
        let ratios = split_ratios_of(layout);
        if ratios != self.last_ratios {
            self.last_ratios = ratios;
            self.layouts_seen += 1;
        }
        self.landed(layout) || (self.outstanding == 0 && self.layouts_seen >= self.ratios.len())
    }
}

/// Split ratios in layout order, the part of a layout `layout_fingerprint`
/// leaves out.
fn split_ratios_of(layout: &PaneLayout) -> Vec<f32> {
    layout.splits.iter().map(|split| split.ratio).collect()
}

/// Split tree shape and which pane sits at each preorder leaf.
/// Paths and directions only: Herdr recomputes split/pane rects from ratios,
/// so including those would cancel a nested drag when an ancestor ratio changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SplitLayoutFingerprint {
    zoomed: bool,
    splits: Vec<(Vec<bool>, SplitDirection)>,
    panes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteForm {
    Create,
    Edit(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionSource {
    ThisMac,
    Saved,
    SshConfig,
}

impl ConnectionSource {
    fn label(self, i18n: I18n) -> &'static str {
        i18n.text(match self {
            Self::ThisMac => k::HOSTS_SOURCE_THIS_MAC,
            Self::Saved => k::HOSTS_SOURCE_SAVED_SHORT,
            Self::SshConfig => k::HOSTS_SOURCE_SSH_CONFIG,
        })
    }

    fn description(self, i18n: I18n) -> &'static str {
        i18n.text(match self {
            Self::ThisMac => k::HOSTS_SOURCE_THIS_MAC_DESCRIPTION,
            Self::Saved => k::HOSTS_SOURCE_SAVED,
            Self::SshConfig => k::HOSTS_SOURCE_SSH_CONFIG_READONLY,
        })
    }
}

fn connection_source(profile: &ConnectionProfile) -> ConnectionSource {
    if matches!(profile, ConnectionProfile::Local { .. }) {
        ConnectionSource::ThisMac
    } else if profile.id().starts_with("manual-") {
        ConnectionSource::Saved
    } else {
        ConnectionSource::SshConfig
    }
}

fn is_saved_profile(profile: &ConnectionProfile) -> bool {
    profile.id().starts_with("manual-")
}

fn ssh_destination(profile: &ConnectionProfile) -> Option<&str> {
    match profile {
        ConnectionProfile::Ssh { destination, .. } => Some(destination.as_str()),
        ConnectionProfile::Local { .. } => None,
    }
}

fn ssh_config_covered_by_saved(profiles: &[ConnectionProfile], destination: &str) -> bool {
    profiles
        .iter()
        .any(|profile| is_saved_profile(profile) && ssh_destination(profile) == Some(destination))
}

fn remember_recent(recents: &mut Vec<String>, id: &str) {
    recents.retain(|existing| existing != id);
    recents.insert(0, id.to_owned());
    recents.truncate(8);
}

fn normalize_recent_host_id(id: &str, profiles: &[ConnectionProfile]) -> Option<String> {
    if profiles.iter().any(|profile| profile.id() == id) {
        return Some(id.to_owned());
    }
    let legacy_alias = id
        .strip_prefix("ssh-")
        .and_then(|rest| rest.split_once('-').map(|(_, alias)| alias))?;
    profiles
        .iter()
        .find(|profile| ssh_destination(profile) == Some(legacy_alias))
        .map(|profile| profile.id().to_owned())
}

fn parse_host_tags(value: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for tag in value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        if !tags
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tag))
        {
            tags.push(tag.to_owned());
        }
    }
    tags
}

fn switch_requires_confirm(from: usize, to: usize, live_session: bool) -> bool {
    from != to && live_session
}

fn profile_index_by_id(profiles: &[ConnectionProfile], id: &str) -> Option<usize> {
    profiles.iter().position(|profile| profile.id() == id)
}

fn confirmed_host_index(overlay: &Overlay, profiles: &[ConnectionProfile]) -> Option<usize> {
    let id = match overlay {
        Overlay::ConfirmSwitchProfile { id, .. } | Overlay::ConfirmRemoveProfile(id) => id.as_str(),
        _ => return None,
    };
    profile_index_by_id(profiles, id)
}

fn next_manual_profile_id(profiles: &[ConnectionProfile]) -> u64 {
    profiles
        .iter()
        .filter_map(|profile| profile.id().strip_prefix("manual-"))
        .filter_map(|suffix| suffix.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

fn profile_display_label(profile: &ConnectionProfile, i18n: I18n) -> String {
    if matches!(profile, ConnectionProfile::Local { .. }) {
        i18n.text(k::HOSTS_SOURCE_THIS_MAC).to_owned()
    } else {
        profile.label().to_owned()
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn profile_endpoint(profile: &ConnectionProfile) -> String {
    match profile {
        ConnectionProfile::Local { .. } => "localhost".into(),
        ConnectionProfile::Ssh {
            destination, port, ..
        } => port.map_or_else(
            || destination.clone(),
            |port| format!("{destination}:{port}"),
        ),
    }
}

fn profile_matches_search(profile: &ConnectionProfile, query: &str, i18n: I18n) -> bool {
    if query.is_empty() {
        return true;
    }
    profile.label().to_lowercase().contains(query)
        || profile_endpoint(profile).to_lowercase().contains(query)
        || connection_source(profile)
            .label(i18n)
            .to_lowercase()
            .contains(query)
        || connection_source(profile)
            .description(i18n)
            .to_lowercase()
            .contains(query)
}

fn ssh_config_entry_is_hidden(profiles: &[ConnectionProfile], profile: &ConnectionProfile) -> bool {
    connection_source(profile) == ConnectionSource::SshConfig
        && ssh_destination(profile)
            .is_some_and(|destination| ssh_config_covered_by_saved(profiles, destination))
}

fn host_display_label_for(
    profile: &ConnectionProfile,
    metadata: Option<&HostMetadata>,
    i18n: I18n,
) -> String {
    metadata
        .and_then(|metadata| metadata.display_name.clone())
        .unwrap_or_else(|| profile_display_label(profile, i18n))
}

fn host_fits_filter(
    profile: &ConnectionProfile,
    filter: &HostFilter,
    metadata: Option<&HostMetadata>,
    recent_ids: &[String],
    orphaned: &HashSet<String>,
    health: &HashMap<String, HostHealthView>,
) -> bool {
    match filter {
        HostFilter::All => true,
        HostFilter::Favorites => metadata.is_some_and(|value| value.favorite),
        HostFilter::Recent => recent_ids.iter().any(|id| id == profile.id()),
        HostFilter::Attention => {
            orphaned.contains(profile.id())
                || health.get(profile.id()).is_some_and(|health| match health {
                    HostHealthView::Checking { .. } => false,
                    HostHealthView::Checked { cached, .. } => {
                        cached.status != HostHealthStatus::Ready
                    }
                })
        }
        HostFilter::Source(source) => connection_source(profile) == *source,
        HostFilter::Group(group) => {
            metadata.and_then(|value| value.group.as_deref()) == Some(group.as_str())
        }
        HostFilter::Tag(tag) => {
            metadata.is_some_and(|value| value.tags.iter().any(|candidate| candidate == tag))
        }
    }
}

struct HostCatalog<'a> {
    profiles: &'a [ConnectionProfile],
    metadata: &'a HashMap<String, HostMetadata>,
    recent_ids: &'a [String],
    orphaned: &'a HashSet<String>,
    health: &'a HashMap<String, HostHealthView>,
}

fn visible_host_indices(
    catalog: &HostCatalog<'_>,
    filter: &HostFilter,
    query: &str,
    current_index: usize,
    i18n: I18n,
) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let recent_positions = catalog
        .recent_ids
        .iter()
        .enumerate()
        .map(|(position, id)| (id.as_str(), position))
        .collect::<HashMap<_, _>>();
    let mut indexes = catalog
        .profiles
        .iter()
        .enumerate()
        .filter(|(_, profile)| {
            if ssh_config_entry_is_hidden(catalog.profiles, profile) {
                return false;
            }
            let meta = catalog.metadata.get(profile.id());
            let search_matches = profile_matches_search(profile, &query, i18n)
                || meta.is_some_and(|metadata| {
                    metadata
                        .display_name
                        .as_deref()
                        .is_some_and(|name| name.to_lowercase().contains(&query))
                        || metadata
                            .group
                            .as_deref()
                            .is_some_and(|group| group.to_lowercase().contains(&query))
                        || metadata
                            .tags
                            .iter()
                            .any(|tag| tag.to_lowercase().contains(&query))
                });
            search_matches
                && host_fits_filter(
                    profile,
                    filter,
                    meta,
                    catalog.recent_ids,
                    catalog.orphaned,
                    catalog.health,
                )
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    indexes.sort_by_key(|index| {
        let profile = &catalog.profiles[*index];
        let meta = catalog.metadata.get(profile.id());
        (
            usize::from(*index != current_index),
            usize::from(!meta.is_some_and(|value| value.favorite)),
            recent_positions
                .get(profile.id())
                .copied()
                .unwrap_or(usize::MAX),
            host_display_label_for(profile, meta, i18n).to_lowercase(),
        )
    });
    indexes
}

fn install_appearance(appearance: &AppearanceSettings, window_appearance: WindowAppearance) {
    let mut family =
        theme::find_family(&appearance.theme_family).unwrap_or_else(theme::ochub_family);
    let effect = appearance.backdrop.theme_effect();
    let content_opacity = if appearance.backdrop == BackdropMode::Opaque {
        100
    } else {
        config::values::opacity_percent_u8(appearance.background_opacity)
    };
    let sidebar_opacity = if appearance.backdrop == BackdropMode::Opaque {
        100
    } else {
        content_opacity.saturating_add(6).min(100)
    };
    for palette in [&mut family.light, &mut family.dark] {
        palette.effects.window_background = effect;
        palette.effects.content_opacity = content_opacity;
        palette.effects.sidebar_opacity = sidebar_opacity;
    }
    theme::install_family(&family, appearance.mode.theme_mode(), window_appearance);
}

fn missing_theme_notice(theme_family: &str, i18n: I18n) -> Option<FailureNotice> {
    if theme::find_family(theme_family).is_some() {
        return None;
    }
    Some(notification_for(
        FailureKind::MissingTheme,
        &i18n.missing_theme_detail(theme_family),
        i18n,
    ))
}

fn load_settings() -> config::LoadedApp {
    match config::AppPaths::user() {
        Some(paths) => config::load_app(&paths),
        None => config::LoadedApp {
            settings: Settings::default(),
            appearance: AppearanceSettings::default(),
            language: Language::default(),
            document: config::ConfigDocument::new(),
        },
    }
}

fn bind_enter_submit<T: 'static>(
    input: &Entity<TextInput>,
    host: WeakEntity<T>,
    cx: &mut Context<T>,
    on_enter: impl Fn(&mut T, &mut Window, &mut Context<T>) + 'static,
) {
    input.update(cx, move |input, _| {
        input.set_on_enter(move |window, cx| {
            host.update(cx, |this, cx| on_enter(this, window, cx)).ok();
        });
    });
}

fn main() {
    application()
        .with_assets(OcHerdrAssets)
        .run(|cx: &mut App| {
            let loaded = load_settings();
            I18n::install(loaded.language);
            if let Some(directory) = dirs::config_dir() {
                theme::set_themes_dir(directory.join("OcHerdr/themes"));
            }
            ochub_ui::install(cx);
            install_appearance(&loaded.appearance, cx.window_appearance());
            cx.on_window_closed(|cx, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1260.), px(820.)),
                        cx,
                    ))),
                    window_min_size: Some(size(px(860.), px(560.))),
                    window_background: theme::window_background_appearance(),
                    titlebar: Some(TitlebarOptions {
                        title: Some(SharedString::new_static("OcHerdr")),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(18.), px(18.))),
                    }),
                    // The tab bar sits in the titlebar strip. Left to AppKit,
                    // dragging a tab pill drags the window instead of
                    // reordering the tab. The header's empty areas call
                    // `start_window_move` themselves.
                    app_owns_titlebar_drag: true,
                    ..Default::default()
                },
                move |window, cx| {
                    window.set_window_title("OcHerdr");
                    let view = cx.new(|cx| OcHerdrView::new_with(loaded, window, cx));
                    let focus = view.read(cx).focus.clone();
                    focus.focus(window, cx);
                    view
                },
            )
            .expect("open OcHerdr window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_order() -> Vec<String> {
        ["a", "b", "c", "d"].map(str::to_owned).to_vec()
    }

    fn tabs_list() -> ReorderList {
        ReorderList::Tabs {
            workspace_id: "w".into(),
        }
    }

    fn pending_list_reorder(list: ReorderList, order: &[String]) -> PendingListReorder {
        PendingListReorder {
            list,
            order: order.to_vec(),
            source_index: 1,
            hover: ReorderHover::Item {
                index: 2,
                trailing: true,
            },
            released_origin: (640., 18.),
        }
    }

    fn two_pane_layout() -> PaneLayout {
        let rect = |x, y, width, height| LayoutRect {
            x,
            y,
            width,
            height,
        };
        PaneLayout {
            workspace_id: "w".into(),
            tab_id: "t".into(),
            zoomed: false,
            area: rect(0, 0, 100, 50),
            focused_pane_id: "a".into(),
            panes: vec![
                ocherdr_core::LayoutPane {
                    pane_id: "a".into(),
                    focused: true,
                    rect: rect(0, 0, 50, 50),
                },
                ocherdr_core::LayoutPane {
                    pane_id: "b".into(),
                    focused: false,
                    rect: rect(50, 0, 50, 50),
                },
            ],
            splits: vec![LayoutSplit {
                id: "split_0_root".into(),
                direction: SplitDirection::Right,
                ratio: 0.5,
                rect: rect(0, 0, 100, 50),
            }],
        }
    }

    const PANE_SURFACE: (f32, f32, f32, f32) = (10., 20., 400., 200.);

    fn pane_drag_at(pointer: (f32, f32)) -> PaneDrag {
        let layout = two_pane_layout();
        let source_rect = pane_window_rect(&layout, "a", PANE_SURFACE).unwrap();
        PaneDrag {
            workspace_id: "w".into(),
            tab_id: "t".into(),
            pane_id: "a".into(),
            fingerprint: layout_fingerprint(&layout),
            origin: pointer,
            pointer,
            grab_offset: (pointer.0 - source_rect.0, pointer.1 - source_rect.1),
            source_rect,
            hover: None,
            edge_drops: false,
            pressed_at: Instant::now(),
        }
    }

    fn close(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-6
            && (a.1 - b.1).abs() < 1e-6
            && (a.2 - b.2).abs() < 1e-6
            && (a.3 - b.3).abs() < 1e-6
    }

    #[test]
    fn a_squeezed_layout_follows_the_preview_ratio_in_whole_cells() {
        let layout = two_pane_layout();
        let squeezed = squeezed_layout(&layout, &[(vec![], 0.7)]).unwrap();
        assert!(close(squeezed.pane("a").unwrap(), (0., 0., 0.7, 1.)));
        assert!(close(squeezed.pane("b").unwrap(), (0.7, 0., 0.3, 1.)));
        let (rect, line) = squeezed.split(&[]).unwrap();
        assert!(close(rect, (0., 0., 1., 1.)));
        assert!((line - 0.7).abs() < 1e-6);
        // A path that is not in the tree leaves everything authoritative.
        let untouched = squeezed_layout(&layout, &[(vec![true], 0.7)]).unwrap();
        assert!(close(untouched.pane("a").unwrap(), (0., 0., 0.5, 1.)));
        // Cells, like Herdr: 0.333 of 100 columns is 33 columns, and the
        // divider sits on that column, not at 33.3.
        let fine = squeezed_layout(&layout, &[(vec![], 0.333)]).unwrap();
        assert!((fine.pane("a").unwrap().2 - 0.33).abs() < 1e-6);
        assert!((fine.split(&[]).unwrap().1 - 0.33).abs() < 1e-6);
        // Out-of-range ratios are clamped the way Herdr clamps them.
        let clamped = squeezed_layout(&layout, &[(vec![], 0.01)]).unwrap();
        assert!((clamped.pane("a").unwrap().2 - 0.1).abs() < 1e-6);
    }

    /// The squeeze preview and the settled render must agree: for any ratio
    /// the preview's rects equal the rects the normal renderer produces for
    /// the authoritative layout Herdr returns for that ratio. An odd-sized
    /// nested `Down` split is the case that jumped half a cell on release.
    #[test]
    fn the_squeeze_preview_matches_the_settled_layout_for_the_same_ratio() {
        let rect = |x, y, width, height| LayoutRect {
            x,
            y,
            width,
            height,
        };
        let area = rect(0, 0, 101, 41);
        // a | (b / c)
        let tree = LayoutNode::Split {
            direction: SplitDirection::Right,
            ratio: 0.5,
            first: Box::new(LayoutNode::Pane("a".into())),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Down,
                ratio: 0.5,
                first: Box::new(LayoutNode::Pane("b".into())),
                second: Box::new(LayoutNode::Pane("c".into())),
            }),
        };
        let settled = |tree: &LayoutNode| -> PaneLayout {
            let predicted = ocherdr_core::LayoutTree {
                root: tree.clone(),
                area,
            };
            PaneLayout {
                workspace_id: "w".into(),
                tab_id: "t".into(),
                zoomed: false,
                area,
                focused_pane_id: "a".into(),
                panes: predicted
                    .pane_rects()
                    .iter()
                    .map(|pane| ocherdr_core::LayoutPane {
                        pane_id: pane.pane_id.clone(),
                        focused: pane.pane_id == "a",
                        rect: pane.rect,
                    })
                    .collect(),
                splits: predicted
                    .splits()
                    .iter()
                    .enumerate()
                    .map(|(index, split)| LayoutSplit {
                        id: split_path_id(index, &split.path),
                        direction: split.direction,
                        ratio: split.ratio,
                        rect: split.rect,
                    })
                    .collect(),
            }
        };
        let before = settled(&tree);
        for (path, ratio) in [(vec![true], 0.5_f32), (vec![true], 0.37), (vec![], 0.61)] {
            let squeezed = squeezed_layout(&before, &[(path.clone(), ratio)]).unwrap();
            let mut retuned = tree.clone();
            set_ratio_at(&mut retuned, &path, ratio);
            let after = settled(&retuned);
            for pane in &after.panes {
                let rendered = layout_rect_fractions(after.area, pane.rect).unwrap();
                let preview = squeezed.pane(&pane.pane_id).unwrap();
                assert!(
                    close(preview, rendered),
                    "{} at {path:?}={ratio}: preview {preview:?} vs settled {rendered:?}",
                    pane.pane_id
                );
            }
            for split in &after.splits {
                let split_path = split.path().unwrap();
                let (first, _) = split_rect(split.rect, split.direction, split.ratio);
                let (fx, fy, fw, fh) = layout_rect_fractions(after.area, first).unwrap();
                let edge = match split.direction {
                    SplitDirection::Right => fx + fw,
                    SplitDirection::Down => fy + fh,
                };
                let (_, line) = squeezed.split(&split_path).unwrap();
                assert!(
                    (line - edge).abs() < 1e-6,
                    "divider {split_path:?}: preview {line} vs pane edge {edge}"
                );
            }
        }
    }

    fn split_path_id(index: usize, path: &[bool]) -> String {
        if path.is_empty() {
            return format!("split_{index}_root");
        }
        let steps: String = path.iter().map(|s| if *s { '1' } else { '0' }).collect();
        format!("split_{index}_{steps}")
    }

    fn set_ratio_at(node: &mut LayoutNode, path: &[bool], new_ratio: f32) {
        if let LayoutNode::Split {
            ratio,
            first,
            second,
            ..
        } = node
        {
            match path.split_first() {
                None => *ratio = new_ratio,
                Some((true, rest)) => set_ratio_at(second, rest, new_ratio),
                Some((false, rest)) => set_ratio_at(first, rest, new_ratio),
            }
        }
    }

    #[test]
    fn a_pane_drag_starts_only_past_six_pixels() {
        let mut drag = pane_drag_at((30., 40.));
        drag.pointer = (36., 40.);
        assert!(!pane_drag_past_slop(&drag), "6 px is still a click");
        drag.pointer = (36.5, 40.);
        assert!(pane_drag_past_slop(&drag));
        drag.pointer = (30., 33.);
        assert!(pane_drag_past_slop(&drag), "either axis counts");
    }

    #[test]
    fn the_preview_keeps_the_grab_offset_and_grows_around_its_centre() {
        let mut drag = pane_drag_at((30., 40.));
        assert_eq!(drag.source_rect, (10., 20., 200., 200.));
        assert_eq!(drag.grab_offset, (20., 20.));
        drag.pointer = (130., 90.);
        let (x, y, w, h) = pane_drag_preview_rect(&drag);
        assert!((w - 203.).abs() < 1e-3 && (h - 203.).abs() < 1e-3);
        assert!((x - (110. - 1.5)).abs() < 1e-3, "{x}");
        assert!((y - (70. - 1.5)).abs() < 1e-3, "{y}");
    }

    #[test]
    fn drop_hover_uses_the_core_five_zones_and_never_targets_the_source() {
        let layout = two_pane_layout();
        // Centre of pane b.
        let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (310., 120.)).unwrap();
        assert_eq!(hover.target_pane_id, "b");
        assert_eq!(hover.zone, DropZone::Center);
        assert!(hover.droppable(false));
        // Right edge of pane b.
        let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (405., 120.)).unwrap();
        assert_eq!(hover.zone, DropZone::Right);
        assert!(!hover.droppable(false), "edges wait for phase 3");
        assert!(hover.droppable(true));
        // Top edge of pane b.
        let hover = pane_drop_hover(&layout, "a", PANE_SURFACE, (310., 24.)).unwrap();
        assert_eq!(hover.zone, DropZone::Up);
        // Over the source pane itself: nothing.
        assert!(pane_drop_hover(&layout, "a", PANE_SURFACE, (100., 120.)).is_none());
        // Outside the surface: nothing.
        assert!(pane_drop_hover(&layout, "a", PANE_SURFACE, (500., 120.)).is_none());
    }

    fn swap_plan(layout: &PaneLayout) -> RelocationPlan {
        RelocationPlan {
            operation_id: 1,
            source_pane_id: "a".into(),
            source_tab_id: "t".into(),
            target_pane_id: "b".into(),
            target_tab_id: "t".into(),
            intent: RelocationIntent::Swap,
            fingerprint: layout_fingerprint(layout),
            topology: SplitLayoutFingerprint {
                zoomed: layout.zoomed,
                splits: layout
                    .splits
                    .iter()
                    .filter_map(|split| Some((split.path()?, split.direction)))
                    .collect(),
                panes: layout.panes.iter().map(|p| p.pane_id.clone()).collect(),
            },
            area: layout.area,
            predicted_rects: predict_swap(layout, "a", "b").unwrap(),
            visual_snapshot: None,
            workspace_id: "w".into(),
            known_tab_ids: HashSet::from(["t".to_owned()]),
            insert_shapes: None,
        }
    }

    #[test]
    fn a_plan_settles_on_the_swapped_layout_and_is_invalidated_by_anything_else() {
        let layout = two_pane_layout();
        let plan = swap_plan(&layout);
        assert!(layout_still_matches_plan(&layout, &plan));
        assert!(!layout_settles_plan(&layout, &plan), "nothing landed yet");

        let mut swapped = layout.clone();
        swapped.panes.swap(0, 1);
        swapped.focused_pane_id = "a".into();
        assert!(!layout_still_matches_plan(&swapped, &plan));
        assert!(layout_settles_plan(&swapped, &plan));

        let mut ratio_only = layout.clone();
        ratio_only.splits[0].ratio = 0.7;
        assert!(
            layout_still_matches_plan(&ratio_only, &plan),
            "a divider move keeps the plan waiting"
        );

        let mut extra_pane = swapped.clone();
        extra_pane.panes.push(ocherdr_core::LayoutPane {
            pane_id: "c".into(),
            focused: false,
            rect: LayoutRect::default(),
        });
        assert!(!layout_settles_plan(&extra_pane, &plan));
        assert!(!layout_still_matches_plan(&extra_pane, &plan));

        let mut zoomed = swapped.clone();
        zoomed.zoomed = true;
        assert!(!layout_settles_plan(&zoomed, &plan));

        let predicted = plan.predicted_fractions();
        assert_eq!(predicted[0].0, "a");
        assert!(
            (predicted[0].1.0 - 0.5).abs() < 1e-6,
            "a is predicted on the right"
        );
    }

    #[test]
    fn settling_moves_from_the_prediction_to_authority_and_lands_at_once_under_reduce_motion() {
        let layout = two_pane_layout();
        let plan = swap_plan(&layout);
        let mut swapped = layout.clone();
        swapped.panes.swap(0, 1);
        // Authority put the split at 0.6 while we predicted 0.5.
        swapped.panes[0].rect.width = 60;
        swapped.panes[1].rect.x = 60;
        swapped.panes[1].rect.width = 40;
        let started = Instant::now();
        let pending = SettlingSeed {
            from: plan.predicted_fractions(),
            plan,
        }
        .into_settling(started);
        let at_start = pending
            .display_fractions("a", Some(&swapped), started, false)
            .unwrap();
        assert!((at_start.0 - 0.5).abs() < 1e-6, "starts on the prediction");
        let at_end = pending
            .display_fractions("a", Some(&swapped), started + PANE_SETTLE_ANIMATION, false)
            .unwrap();
        assert!((at_end.0 - 0.6).abs() < 1e-6, "ends on authority");
        assert!(pending.is_settled(started + PANE_SETTLE_ANIMATION, false));
        assert!(!pending.is_settled(started, false));
        let reduced = pending
            .display_fractions("a", Some(&swapped), started, true)
            .unwrap();
        assert!(
            (reduced.0 - 0.6).abs() < 1e-6,
            "reduce motion lands immediately"
        );
        assert!(pending.is_settled(started, true));
    }

    #[test]
    fn display_positions_stay_put_before_crossing_a_midpoint() {
        assert_eq!(
            reorder_display_positions(
                &item_order(),
                1,
                ReorderHover::Item {
                    index: 1,
                    trailing: false,
                },
            ),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn display_positions_move_the_crossed_right_neighbor_into_the_hole() {
        assert_eq!(
            reorder_display_positions(
                &item_order(),
                1,
                ReorderHover::Item {
                    index: 2,
                    trailing: true,
                },
            ),
            [0, 2, 1, 3]
        );
    }

    #[test]
    fn display_positions_move_the_crossed_left_neighbor_into_the_hole() {
        assert_eq!(
            reorder_display_positions(
                &item_order(),
                2,
                ReorderHover::Item {
                    index: 1,
                    trailing: false,
                },
            ),
            [0, 2, 1, 3]
        );
    }

    #[test]
    fn display_positions_stay_in_bounds_at_both_ends() {
        for (source_index, hover) in [
            (
                2,
                ReorderHover::Item {
                    index: 0,
                    trailing: false,
                },
            ),
            (0, ReorderHover::AfterLast),
        ] {
            let positions = reorder_display_positions(&item_order(), source_index, hover);
            assert_eq!(positions.len(), 4);
            assert!(positions.into_iter().all(|position| position < 4));
        }
    }

    #[test]
    fn display_shifts_use_the_same_numbers_on_either_axis() {
        let positions = reorder_display_positions(
            &item_order(),
            1,
            ReorderHover::Item {
                index: 2,
                trailing: true,
            },
        );
        let spans = [(0., 10.), (14., 10.), (28., 10.), (42., 10.)];
        let shifts = reorder_display_shifts(&spans, &positions, 4.);
        assert_eq!(shifts, [0., 14., -14., 0.]);
        assert_eq!(
            shifts
                .iter()
                .map(|&shift| reorder_axis_offset(shift, ReorderAxis::Horizontal))
                .collect::<Vec<_>>(),
            [(0., 0.), (14., 0.), (-14., 0.), (0., 0.)]
        );
        assert_eq!(
            shifts
                .iter()
                .map(|&shift| reorder_axis_offset(shift, ReorderAxis::Vertical))
                .collect::<Vec<_>>(),
            [(0., 0.), (0., 14.), (0., -14.), (0., 0.)]
        );
    }

    #[test]
    fn a_pointer_below_the_tab_strip_does_not_take_the_ghost_with_it() {
        let strip = (260., 9., 400., 28.);
        let size = (120., 28.);
        let grab = (40., 14.);
        let pointer = (400., 200.);
        let origin = reorder_ghost_origin(pointer, grab, strip, size, ReorderAxis::Horizontal);
        assert_eq!(origin, (360., 9.));
        assert_ne!(
            pointer.1 - grab.1,
            origin.1,
            "unclamped follow would drop the ghost into the terminal"
        );
    }

    #[test]
    fn a_pointer_beside_the_sidebar_does_not_take_the_ghost_with_it() {
        let list = (8., 120., 236., 120.);
        let size = (236., 30.);
        let grab = (20., 10.);
        let pointer = (600., 180.);
        let origin = reorder_ghost_origin(pointer, grab, list, size, ReorderAxis::Vertical);
        assert_eq!(origin.0, 8.);
        assert_eq!(origin.1, 170.);
    }

    #[test]
    fn tab_preview_origin_centers_under_the_tab() {
        let tab = (320., 10., TAB_PILL_WIDTH, TAB_PILL_HEIGHT);
        let (x, y) = tab_preview_origin(tab, 800.);
        assert_eq!(x, 240.);
        assert_eq!(x + TAB_PREVIEW_WIDTH / 2., tab.0 + tab.2 / 2.);
        assert_eq!(y, tab.1 + tab.3 + TAB_PREVIEW_GAP);
    }

    #[test]
    fn tab_preview_origin_clamps_to_the_left_margin() {
        let tab = (0., 10., TAB_PILL_WIDTH, TAB_PILL_HEIGHT);
        let unclamped = tab.0 + tab.2 / 2. - TAB_PREVIEW_WIDTH / 2.;
        let (x, y) = tab_preview_origin(tab, 800.);
        assert!(unclamped < TAB_PREVIEW_MARGIN);
        assert_eq!(x, TAB_PREVIEW_MARGIN);
        assert_eq!(y, tab.1 + tab.3 + TAB_PREVIEW_GAP);
    }

    #[test]
    fn tab_preview_origin_clamps_to_the_right_margin() {
        let window_width = 800.;
        let tab = (
            window_width - TAB_PILL_WIDTH,
            10.,
            TAB_PILL_WIDTH,
            TAB_PILL_HEIGHT,
        );
        let unclamped = tab.0 + tab.2 / 2. - TAB_PREVIEW_WIDTH / 2.;
        let max_x = window_width - TAB_PREVIEW_WIDTH - TAB_PREVIEW_MARGIN;
        let (x, y) = tab_preview_origin(tab, window_width);
        assert!(unclamped > max_x);
        assert_eq!(x, max_x);
        assert_eq!(y, tab.1 + tab.3 + TAB_PREVIEW_GAP);
    }

    #[test]
    fn ghost_origin_clamps_the_free_axis_to_the_list() {
        let strip = (260., 9., 400., 28.);
        let size = (120., 28.);
        let grab = (40., 14.);
        assert_eq!(
            reorder_ghost_origin((0., 12.), grab, strip, size, ReorderAxis::Horizontal),
            (260., 9.)
        );
        assert_eq!(
            reorder_ghost_origin((900., 12.), grab, strip, size, ReorderAxis::Horizontal),
            (540., 9.)
        );
        let list = (8., 120., 236., 120.);
        let size = (236., 30.);
        assert_eq!(
            reorder_ghost_origin((20., 0.), grab, list, size, ReorderAxis::Vertical),
            (8., 120.)
        );
        assert_eq!(
            reorder_ghost_origin((20., 800.), grab, list, size, ReorderAxis::Vertical),
            (8., 210.)
        );
    }

    #[test]
    fn an_in_flight_tab_move_keeps_the_predicted_display_order() {
        let order = item_order();
        let pending = pending_list_reorder(tabs_list(), &order);
        let projection = reorder_projection(&tabs_list(), &order, None, Some(&pending))
            .expect("the pending request must keep its release-time projection");

        assert_eq!(projection.positions, [0, 2, 1, 3]);
        assert_eq!(projection.previous_positions, projection.positions);
        assert_eq!(
            projection.motion,
            ReorderMotion::Settling {
                released_origin: (640., 18.)
            }
        );
    }

    #[test]
    fn an_in_flight_workspace_move_uses_the_same_projection() {
        let order = item_order();
        let pending = pending_list_reorder(ReorderList::Workspaces, &order);
        let projection = reorder_projection(&ReorderList::Workspaces, &order, None, Some(&pending))
            .expect("workspaces settle with the same mapping as tabs");
        assert_eq!(projection.positions, [0, 2, 1, 3]);
    }

    #[test]
    fn a_different_authoritative_order_overrides_the_pending_prediction() {
        let original = item_order();
        let pending = pending_list_reorder(tabs_list(), &original);
        let authoritative = ["c", "a", "b", "d"].map(str::to_owned).to_vec();

        assert!(
            reorder_projection(&tabs_list(), &authoritative, None, Some(&pending)).is_none(),
            "a prediction based on stale order must never mask the published order"
        );
    }

    #[test]
    fn legacy_connection_settings_keep_host_fields_without_appearance() {
        let settings: Settings = serde_json::from_str(
            r#"{"connections":[],"appearance":{"theme_family":"ember"},"language":"english"}"#,
        )
        .unwrap();

        assert!(settings.connections.is_empty());
        assert!(settings.host_metadata.is_empty());
        assert!(settings.host_groups.is_empty());
        assert!(settings.host_health.is_empty());
        let value = serde_json::to_value(&settings).unwrap();
        assert!(value.get("appearance").is_none());
        assert!(value.get("language").is_none());
    }

    #[test]
    fn legacy_recent_ssh_ids_migrate_to_stable_alias_ids() {
        let profiles = vec![
            ConnectionProfile::default(),
            ConnectionProfile::Ssh {
                id: "ssh-config:build-box".into(),
                label: "build-box".into(),
                destination: "build-box".into(),
                port: None,
                identity_file: None,
                herdr_path: "herdr".into(),
            },
        ];
        assert_eq!(
            normalize_recent_host_id("ssh-7-build-box", &profiles).as_deref(),
            Some("ssh-config:build-box")
        );
    }

    #[test]
    fn host_tags_are_trimmed_and_deduplicated_case_insensitively() {
        assert_eq!(
            parse_host_tags(" production, gpu,Production, , arm64 "),
            ["production", "gpu", "arm64"]
        );
    }

    #[test]
    fn remote_search_matches_labels_endpoints_and_sources() {
        let profile = ConnectionProfile::Ssh {
            id: "ssh-0-build".into(),
            label: "Build box".into(),
            destination: "builder@example.net".into(),
            port: Some(2222),
            identity_file: None,
            herdr_path: "herdr".into(),
        };

        let i18n = I18n::new(Language::English);
        assert!(profile_matches_search(&profile, "build", i18n));
        assert!(profile_matches_search(&profile, "2222", i18n));
        assert!(profile_matches_search(&profile, "ssh config", i18n));
        assert!(!profile_matches_search(&profile, "production", i18n));
    }

    #[test]
    fn recent_hosts_move_to_the_front_and_stay_bounded() {
        let mut recents = vec!["a".into(), "b".into(), "c".into()];
        remember_recent(&mut recents, "b");
        assert_eq!(recents, ["b", "a", "c"]);
        for index in 0..10 {
            remember_recent(&mut recents, &format!("h{index}"));
        }
        assert_eq!(recents.len(), 8);
        assert_eq!(recents[0], "h9");
    }

    #[test]
    fn switching_hosts_confirms_only_when_leaving_a_live_session() {
        assert!(!switch_requires_confirm(0, 0, true));
        assert!(!switch_requires_confirm(1, 2, false));
        assert!(switch_requires_confirm(1, 2, true));
    }

    fn ssh_host(id: &str, label: &str) -> ConnectionProfile {
        ConnectionProfile::Ssh {
            id: id.into(),
            label: label.into(),
            destination: label.into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        }
    }

    #[test]
    fn a_host_confirmation_follows_the_host_id_after_the_list_is_reordered() {
        let alpha = ssh_host("manual-1", "alpha");
        let beta = ssh_host("manual-2", "beta");
        let gamma = ssh_host("manual-3", "gamma");
        let overlay = Overlay::ConfirmSwitchProfile {
            id: beta.id().to_owned(),
            from_hosts: false,
        };

        let original = [alpha.clone(), beta.clone(), gamma.clone()];
        assert_eq!(confirmed_host_index(&overlay, &original), Some(1));

        let reordered = [gamma.clone(), alpha.clone(), beta.clone()];
        let index = confirmed_host_index(&overlay, &reordered).expect("host still exists");
        assert_eq!(reordered[index].id(), "manual-2");
        assert_ne!(
            reordered[1].id(),
            "manual-2",
            "the old index now points at a different host"
        );

        let remaining = [gamma, alpha];
        assert_eq!(
            confirmed_host_index(
                &Overlay::ConfirmRemoveProfile(beta.id().to_owned()),
                &remaining
            ),
            None
        );
    }

    #[test]
    fn cell_metric_presets_write_ghostty_percent_values() {
        assert_eq!(CellWidthChoice::Tight.metric().unwrap().to_config(), "-10%");
        assert_eq!(CellWidthChoice::Normal.metric(), None);
        assert_eq!(CellWidthChoice::Wide.metric().unwrap().to_config(), "10%");
        assert_eq!(
            CellHeightChoice::Compact.metric().unwrap().to_config(),
            "-8%"
        );
        assert_eq!(CellHeightChoice::Normal.metric(), None);
        assert_eq!(
            CellHeightChoice::Relaxed.metric().unwrap().to_config(),
            "12%"
        );
        assert_eq!(CellHeightChoice::Loose.metric().unwrap().to_config(), "20%");
    }

    #[test]
    fn a_missing_theme_family_warns_and_stays_in_settings() {
        let requested = "vanished-theme";
        assert!(theme::find_family(requested).is_none());
        assert!(theme::find_family(theme::DEFAULT_THEME_FAMILY).is_some());

        let english = I18n::new(Language::English);
        let notice = missing_theme_notice(requested, english).expect("missing theme must warn");
        assert_eq!(
            notice.level,
            ochub_ui::notifications::NotificationLevel::Warning
        );
        assert_eq!(notice.title, "Theme not found");
        assert_eq!(
            notice.message,
            "The theme vanished-theme in your settings does not exist. Using the default theme."
        );
        assert!(missing_theme_notice(theme::DEFAULT_THEME_FAMILY, english).is_none());

        let chinese = I18n::new(Language::SimplifiedChinese);
        let zh = missing_theme_notice(requested, chinese).expect("missing theme must warn in zh");
        assert_eq!(zh.title, "找不到主题");
        assert_eq!(
            zh.message,
            "配置里的主题 vanished-theme 不存在，已使用默认主题。"
        );

        let appearance = AppearanceSettings {
            theme_family: requested.into(),
            ..AppearanceSettings::default()
        };
        assert_eq!(appearance.theme_family, requested);
    }

    #[test]
    fn keys_go_to_the_terminal_only_when_no_overlay_is_open() {
        let target = HierarchyTarget::Pane {
            id: "p".into(),
            label: "p".into(),
        };
        let overlays = [
            Overlay::None,
            Overlay::NodeManager,
            Overlay::RemoteForm(RemoteForm::Create),
            Overlay::RemoteForm(RemoteForm::Edit(0)),
            Overlay::Appearance,
            Overlay::HostSwitcher,
            Overlay::ContextMenu(HierarchyContextMenu {
                target: target.clone(),
                x: 0.,
                y: 0.,
            }),
            Overlay::Rename(target.clone()),
            Overlay::ConfirmClose(target),
            Overlay::ConfirmRemoveWorktree {
                workspace_id: "w1".into(),
                label: "feature".into(),
                prompt: RemoveWorktreePrompt::Safe,
            },
            Overlay::WorktreeCreate {
                workspace_id: "w1".into(),
                advanced: false,
            },
            Overlay::WorktreeOpen(WorktreeOpenState::Loading {
                owner: SessionKey {
                    profile_id: "local".into(),
                    session_name: "default".into(),
                },
                workspace_id: "w1".into(),
            }),
            Overlay::ConfirmRemoveProfile("manual-1".into()),
            Overlay::ConfirmSwitchProfile {
                id: "local".into(),
                from_hosts: false,
            },
            Overlay::ConfirmSwitchProfile {
                id: "manual-1".into(),
                from_hosts: true,
            },
            Overlay::ConfirmBulkRemove,
        ];
        for overlay in overlays {
            assert_eq!(
                key_goes_to_terminal(&overlay),
                matches!(overlay, Overlay::None),
                "{overlay:?}"
            );
        }
    }

    #[test]
    fn saved_hosts_hide_the_matching_ssh_config_entry() {
        let profiles = vec![
            ConnectionProfile::default(),
            ConnectionProfile::Ssh {
                id: "manual-1".into(),
                label: "Build".into(),
                destination: "build".into(),
                port: None,
                identity_file: None,
                herdr_path: "herdr".into(),
            },
            ConnectionProfile::Ssh {
                id: "ssh-0-build".into(),
                label: "build".into(),
                destination: "build".into(),
                port: None,
                identity_file: None,
                herdr_path: "herdr".into(),
            },
        ];
        assert!(ssh_config_covered_by_saved(&profiles, "build"));
        assert!(!ssh_config_covered_by_saved(&profiles, "prod"));
        assert_eq!(connection_source(&profiles[0]), ConnectionSource::ThisMac);
        assert_eq!(connection_source(&profiles[1]), ConnectionSource::Saved);
        assert_eq!(connection_source(&profiles[2]), ConnectionSource::SshConfig);
    }

    fn sample_visible_hosts() -> (Vec<ConnectionProfile>, HashMap<String, HostMetadata>) {
        let profiles = vec![
            ConnectionProfile::default(),
            ConnectionProfile::Ssh {
                id: "manual-1".into(),
                label: "Alpha".into(),
                destination: "alpha.example".into(),
                port: None,
                identity_file: None,
                herdr_path: "herdr".into(),
            },
            ConnectionProfile::Ssh {
                id: "manual-2".into(),
                label: "Beta".into(),
                destination: "beta.example".into(),
                port: None,
                identity_file: None,
                herdr_path: "herdr".into(),
            },
        ];
        let mut metadata = HashMap::new();
        metadata.insert(
            "manual-1".into(),
            HostMetadata {
                favorite: true,
                ..HostMetadata::default()
            },
        );
        (profiles, metadata)
    }

    fn indices_for(filter: HostFilter) -> (Vec<ConnectionProfile>, Vec<usize>) {
        let (profiles, metadata) = sample_visible_hosts();
        let recent_ids = Vec::<String>::new();
        let orphaned = HashSet::new();
        let health = HashMap::new();
        let indexes = visible_host_indices(
            &HostCatalog {
                profiles: &profiles,
                metadata: &metadata,
                recent_ids: &recent_ids,
                orphaned: &orphaned,
                health: &health,
            },
            &filter,
            "",
            0,
            I18n::new(Language::English),
        );
        (profiles, indexes)
    }

    #[test]
    fn changing_the_host_filter_changes_the_visible_index_set() {
        let (_, all) = indices_for(HostFilter::All);
        let (_, favorites) = indices_for(HostFilter::Favorites);
        assert_ne!(all, favorites);
        assert!(all.contains(&1) && all.contains(&2));
        assert_eq!(favorites, vec![1]);
    }

    #[test]
    fn visible_host_indices_are_always_in_range_of_the_profile_list() {
        let (profiles, all) = indices_for(HostFilter::All);
        let (_, favorites) = indices_for(HostFilter::Favorites);
        for index in all.iter().chain(&favorites) {
            assert!(*index < profiles.len());
        }
    }

    #[test]
    fn a_filter_that_matches_nothing_returns_no_indices() {
        let (_, indexes) = indices_for(HostFilter::Tag("no-such-tag".into()));
        assert!(indexes.is_empty());
    }

    // ---- Insert transaction (design §7.2) ----

    fn insert_plan(layout: &PaneLayout, edge: DropEdge) -> RelocationPlan {
        let steps = predict_relocation_steps(layout, "a", "b", edge, 0.5).unwrap();
        RelocationPlan {
            operation_id: 2,
            source_pane_id: "a".into(),
            source_tab_id: "t".into(),
            target_pane_id: "b".into(),
            target_tab_id: "t".into(),
            intent: RelocationIntent::Insert { edge, ratio: 0.5 },
            fingerprint: layout_fingerprint(layout),
            topology: controller::split_layout_fingerprint(layout),
            area: layout.area,
            predicted_rects: steps.final_layout.panes.clone(),
            visual_snapshot: None,
            workspace_id: "w".into(),
            known_tab_ids: HashSet::from(["t".to_owned()]),
            insert_shapes: Some(InsertShapes::from_steps(&steps)),
        }
    }

    fn parked() -> ParkedPane {
        ParkedPane {
            temp_tab_id: "t-tmp".into(),
            pane_id: "a".into(),
        }
    }

    fn inserting(responded: bool, layout_seen: bool) -> RelocationPhase {
        RelocationPhase::Inserting {
            temp_tab_id: "t-tmp".into(),
            moved_pane_id: "a".into(),
            responded,
            layout_seen,
        }
    }

    #[test]
    fn a_right_drop_walks_parking_inserting_settle() {
        use RelocationAction as A;
        use RelocationPhase as P;
        use RelocationSignal as S;
        let (phase, action) = advance_insert_phase(P::Parking, S::Parked(Some(parked())), false);
        assert_eq!(phase, Some(inserting(false, false)));
        assert_eq!(
            action,
            A::SendInsert,
            "step 2 goes out inside the step-1 callback"
        );
        // The removed layout lands: benign.
        let (phase, action) =
            advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Removed), false);
        assert_eq!(phase, Some(inserting(false, false)));
        assert_eq!(action, A::None);
        // Response before the final layout.
        let (phase, action) = advance_insert_phase(phase.unwrap(), S::Inserted(true), false);
        assert_eq!(phase, Some(inserting(true, false)));
        assert_eq!(action, A::None);
        let (phase, action) =
            advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Final), false);
        assert_eq!(phase, Some(inserting(true, true)));
        assert_eq!(action, A::Settle);
        // The other order: layout first, then the response settles.
        let (phase, action) = advance_insert_phase(
            inserting(false, false),
            S::Layout(LayoutShape::Inserted),
            false,
        );
        assert_eq!(phase, Some(inserting(false, true)));
        assert_eq!(action, A::None);
        let (_, action) = advance_insert_phase(phase.unwrap(), S::Inserted(true), false);
        assert_eq!(action, A::Settle);
    }

    #[test]
    fn a_left_drop_adds_the_order_correction_before_settling() {
        use RelocationAction as A;
        use RelocationPhase as P;
        use RelocationSignal as S;
        let (phase, action) =
            advance_insert_phase(inserting(false, false), S::Inserted(true), true);
        assert_eq!(
            phase,
            Some(P::CorrectingOrder {
                responded: false,
                layout_seen: false
            })
        );
        assert_eq!(action, A::SendSwap);
        // The step-2 layout (source second) is an intermediate, not a landing;
        // so is a late step-1 layout arriving after the step-2 response.
        let (phase, action) =
            advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Removed), true);
        assert_eq!(action, A::None);
        let (phase, action) =
            advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Inserted), true);
        assert_eq!(action, A::None);
        let (phase, action) = advance_insert_phase(phase.unwrap(), S::Reordered(true), true);
        assert_eq!(
            phase,
            Some(P::CorrectingOrder {
                responded: true,
                layout_seen: false
            })
        );
        assert_eq!(action, A::None);
        let (_, action) = advance_insert_phase(phase.unwrap(), S::Layout(LayoutShape::Final), true);
        assert_eq!(action, A::Settle);
        // While still Inserting, the step-2 layout of a left drop is not a
        // landing either.
        let (phase, action) = advance_insert_phase(
            inserting(false, false),
            S::Layout(LayoutShape::Inserted),
            true,
        );
        assert_eq!(phase, Some(inserting(false, false)));
        assert_eq!(action, A::None);
    }

    #[test]
    fn every_failure_branch_lands_where_the_design_says() {
        use RelocationAction as A;
        use RelocationPhase as P;
        use RelocationSignal as S;
        // Step 1 fails: revert, nothing else.
        assert_eq!(
            advance_insert_phase(P::Parking, S::Parked(None), false),
            (None, A::Revert)
        );
        // Step 2 fails: Parked with the temp tab, then Retry re-issues step 2.
        let (phase, action) =
            advance_insert_phase(inserting(false, false), S::Inserted(false), true);
        let parked_phase = P::Parked {
            temp_tab_id: "t-tmp".into(),
            moved_pane_id: "a".into(),
        };
        assert_eq!(phase, Some(parked_phase.clone()));
        assert_eq!(action, A::Park);
        let (phase, action) = advance_insert_phase(parked_phase.clone(), S::Retry, true);
        assert_eq!(phase, Some(inserting(false, false)));
        assert_eq!(action, A::SendInsert);
        // Parked shows authority: layouts do not disturb it.
        assert_eq!(
            advance_insert_phase(parked_phase.clone(), S::Layout(LayoutShape::Foreign), true),
            (Some(parked_phase), A::None)
        );
        // Step 3 fails: Misordered, plan dropped, layout kept.
        assert_eq!(
            advance_insert_phase(
                P::CorrectingOrder {
                    responded: false,
                    layout_seen: false
                },
                S::Reordered(false),
                true
            ),
            (None, A::Misordered)
        );
        // A foreign layout at any in-flight phase aborts to authority.
        assert_eq!(
            advance_insert_phase(P::Parking, S::Layout(LayoutShape::Foreign), false),
            (None, A::Revert)
        );
        assert_eq!(
            advance_insert_phase(
                inserting(true, false),
                S::Layout(LayoutShape::Foreign),
                false
            ),
            (None, A::Revert)
        );
        assert_eq!(
            advance_insert_phase(
                P::CorrectingOrder {
                    responded: true,
                    layout_seen: false
                },
                S::Layout(LayoutShape::Foreign),
                true
            ),
            (None, A::Revert)
        );
        // Out-of-order signals are ignored.
        assert_eq!(
            advance_insert_phase(P::Parking, S::Inserted(true), false),
            (Some(P::Parking), A::None)
        );
        assert_eq!(
            advance_insert_phase(P::Parking, S::Reordered(true), false),
            (Some(P::Parking), A::None)
        );
    }

    #[test]
    fn insert_layouts_are_classified_against_the_predicted_shapes() {
        let layout = two_pane_layout();
        // `a` onto the right edge of `b`: final = [b | a].
        let plan = insert_plan(&layout, DropEdge::Right);
        assert_eq!(
            classify_insert_layout(&layout, &plan),
            LayoutShape::Release,
            "unchanged layout"
        );
        let mut removed = layout.clone();
        removed.panes.remove(0);
        removed.panes[0].rect = layout.area;
        removed.splits.clear();
        assert_eq!(
            classify_insert_layout(&removed, &plan),
            LayoutShape::Removed
        );
        let mut final_layout = layout.clone();
        final_layout.panes.swap(0, 1);
        final_layout.panes[0].rect = LayoutRect {
            x: 0,
            y: 0,
            width: 50,
            height: 50,
        };
        final_layout.panes[1].rect = LayoutRect {
            x: 50,
            y: 0,
            width: 50,
            height: 50,
        };
        assert_eq!(
            classify_insert_layout(&final_layout, &plan),
            LayoutShape::Final,
            "right/down: inserted == final"
        );
        let mut foreign = layout.clone();
        foreign.panes.push(ocherdr_core::LayoutPane {
            pane_id: "c".into(),
            focused: false,
            rect: layout.area,
        });
        assert_eq!(
            classify_insert_layout(&foreign, &plan),
            LayoutShape::Foreign
        );

        // `a` onto the left edge of `b`: step 2 gives [b | a], the swap
        // gives [a | b], which is the release shape again.
        let plan = insert_plan(&layout, DropEdge::Left);
        assert_eq!(
            classify_insert_layout(&final_layout, &plan),
            LayoutShape::Inserted
        );
        assert_eq!(classify_insert_layout(&layout, &plan), LayoutShape::Final);
        assert!(plan.intent.corrects_order());
        assert!(plan.is_supported());
        let swap = swap_plan(&layout);
        assert!(!swap.intent.corrects_order());
    }

    #[test]
    fn a_pending_insert_renders_the_prediction_until_parked() {
        let layout = two_pane_layout();
        let plan = insert_plan(&layout, DropEdge::Right);
        let now = Instant::now();
        for phase in [
            RelocationPhase::Parking,
            inserting(false, false),
            RelocationPhase::CorrectingOrder {
                responded: false,
                layout_seen: false,
            },
        ] {
            let pending = PendingPaneRelocation {
                plan: plan.clone(),
                phase,
            };
            let rect = pending
                .display_fractions("a", Some(&layout), now, false)
                .expect("predicted");
            assert!(
                (rect.0 - 0.5).abs() < 1e-6,
                "`a` is drawn on the right: {rect:?}"
            );
            assert!(pending.phase.locks_tab());
            assert!(!pending.is_settled(now, true));
        }
        let parked = PendingPaneRelocation {
            plan,
            phase: RelocationPhase::Parked {
                temp_tab_id: "t-tmp".into(),
                moved_pane_id: "a".into(),
            },
        };
        assert!(
            parked
                .display_fractions("a", Some(&layout), now, false)
                .is_none()
        );
        assert!(!parked.phase.locks_tab());
        assert_eq!(parked.phase.parked_tab_id(), Some("t-tmp"));
        assert_eq!(inserting(false, false).hidden_tab_id(), Some("t-tmp"));
        assert_eq!(parked.phase.hidden_tab_id(), None, "parked tabs are shown");
    }

    /// The event stream can name the temporary tab before (or keep it after)
    /// the responses do: an unknown tab of the workspace holding only the
    /// source pane is the temporary tab; anything else stays visible.
    #[test]
    fn an_unlisted_tab_holding_only_the_source_pane_is_the_temp_tab() {
        let layout = two_pane_layout();
        let plan = insert_plan(&layout, DropEdge::Right);
        let pane = |pane_id: &str, tab_id: &str| {
            json!({
                "pane_id": pane_id,
                "terminal_id": pane_id,
                "workspace_id": "w",
                "tab_id": tab_id,
                "focused": false,
            })
        };
        let tab = |tab_id: &str, number: usize| {
            json!({
                "tab_id": tab_id,
                "workspace_id": "w",
                "number": number,
                "label": tab_id,
                "focused": false,
                "pane_count": 1,
            })
        };
        let snapshot: HierarchySnapshot = serde_json::from_value(json!({
            "version": "0.9.0",
            "protocol": 14,
            "tabs": [tab("t", 1), tab("t-tmp", 2), tab("t-other", 3), tab("t-empty", 4)],
            "panes": [
                pane("b", "t"),
                pane("a", "t-tmp"),
                pane("c", "t-other"),
            ],
        }))
        .unwrap();
        let hidden: Vec<&str> = plan.unlisted_temp_tabs(&snapshot).collect();
        assert_eq!(
            hidden,
            vec!["t-tmp", "t-empty"],
            "the tab with the source pane and a pane-less newcomer are hidden; \
             a foreign tab and the known tab are not"
        );
        let swap = swap_plan(&layout);
        assert_eq!(
            swap.unlisted_temp_tabs(&snapshot).count(),
            0,
            "a swap creates no tab and hides nothing"
        );
    }

    #[test]
    fn keyboard_move_picks_the_neighbour_and_cycles_zones() {
        let layout = two_pane_layout();
        assert_eq!(
            keyboard_neighbour(&layout, "a", DropEdge::Right).as_deref(),
            Some("b")
        );
        assert_eq!(keyboard_neighbour(&layout, "a", DropEdge::Left), None);
        assert_eq!(keyboard_neighbour(&layout, "a", DropEdge::Up), None);
        assert_eq!(
            keyboard_neighbour(&layout, "b", DropEdge::Left).as_deref(),
            Some("a")
        );
        assert_eq!(
            next_keyboard_zone(DropZone::Center, false),
            DropZone::Center
        );
        assert_eq!(next_keyboard_zone(DropZone::Center, true), DropZone::Left);
        assert_eq!(next_keyboard_zone(DropZone::Down, true), DropZone::Center);
        let mode = KeyboardPaneMove {
            workspace_id: "w".into(),
            tab_id: "t".into(),
            pane_id: "a".into(),
            fingerprint: 0,
            target: Some(PaneDropHover {
                target_pane_id: "b".into(),
                zone: DropZone::Left,
                target_rect: (0., 0., 0., 0.),
            }),
            edge_drops: false,
        };
        assert!(!mode.droppable(), "edges need the flag");
        let mode = KeyboardPaneMove {
            edge_drops: true,
            ..mode
        };
        assert!(mode.droppable());
    }
}
