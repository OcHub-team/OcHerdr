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
    layout_fingerprint, predict_insert_pane, predict_relocation_steps, predict_remove_pane,
    predict_swap, rebuild_tree, reorder_insert_index, split_ratio_from_drag, split_rect,
    valid_split_ratio,
};
use ocherdr_files::{
    BackendKind as FileBackendKind, FileEntry, FileService, FileVersion, TransferMonitor,
    TransferProgress,
};
use ocherdr_herdr::{
    EventSubscription, HerdrError, HostHealthStatus, MAX_CLIPBOARD_IMAGE_BYTES, SessionConnection,
    TerminalCommand, TerminalMode, TerminalNotificationKind, TerminalScrollDirection,
    TerminalSession, attach_command, discover_sessions, open_system_terminal, request_socket,
};
use ocherdr_terminal::{KeyAction, KeyModifiers, RenderedFrame, Terminal, TerminalPalette};
use ochub_ui::anim::Transition;
use ochub_ui::components::{
    ButtonSize, ButtonTone, busy_button, button, context_menu, context_menu_item, disabled_button,
    empty_state, field, field_with_error, icon_button_tone, icon_only_button_tone, modal_body,
    modal_card, modal_footer, modal_header, modal_overlay, spinner, status_dot,
};
use ochub_ui::gpui::{
    Anchor, Animation, AnimationExt, App, AppContext, AssetSource, Bounds, ClickEvent,
    ClipboardEntry, ClipboardItem, Context, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, ExternalPaths, FocusHandle, Focusable, FontWeight, IntoElement, KeyBinding,
    KeyDownEvent, Keystroke, Menu, MenuItem, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathPromptOptions, Render, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, SystemNotification, Task, TextOverflow, TextRun,
    TitlebarOptions, UTF16Selection, WeakEntity, Window, WindowAppearance, WindowBounds,
    WindowOptions, anchored, canvas, deferred, div, ease_out_quint, linear_color_stop,
    linear_gradient, point, prelude::*, px, relative, size,
};
#[cfg(not(target_os = "macos"))]
use ochub_ui::gpui::{
    FontStyle, FontWeight as GpuiFontWeight, StyledText, UnderlineStyle, font, rgba,
};
#[cfg(target_os = "macos")]
use ochub_ui::gpui::{ObjectFit, surface};
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
mod file_panel;
mod fonts;
mod host_center;
mod host_model;
mod i18n;
mod ime;
mod notify;
mod pane_model;
mod pane_tab_drop;
mod pane_templates;
mod reorder_model;
mod theme_ansi;
mod ui;
mod update;

pub(crate) use file_panel::*;
pub(crate) use host_model::*;
pub(crate) use pane_model::*;
pub(crate) use pane_tab_drop::*;
pub(crate) use pane_templates::*;
pub(crate) use reorder_model::*;

use host_center::{HostCenter, HostCenterEvent, HostRollback, HostSaveThen};
use i18n::{I18n, Language, k};
use notify::{FailureKind, FailureNotice, notification_for};

gpui::actions!(ocherdr, [Quit, CheckForUpdates]);

const SIDEBAR_WIDTH: f32 = 252.;
const HEADER_HEIGHT: f32 = 46.;
const TAB_PILL_HEIGHT: f32 = 28.;
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
/// A hover intent changes the local draft tree immediately; pane shells ease
/// into the new geometry while hit testing stays on the authoritative layout.
const PANE_DRAG_LAYOUT_ANIMATION: Duration = Duration::from_millis(140);
const PANE_DROP_ZONE_ANIMATION: Duration = Duration::from_millis(100);
const PANE_DRAG_RETURN_ANIMATION: Duration = Duration::from_millis(120);
const PANE_SETTLE_ANIMATION: Duration = Duration::from_millis(160);
/// Canvas measurements can arrive from several transient startup and layout
/// frames. Coalesce them before resizing Ghostty or replacing an observer so
/// stale geometry never makes a pane repeatedly blank and reflow.
const PANE_RESIZE_SETTLE_DELAY: Duration = Duration::from_millis(120);
/// Ghostty surface construction is intentionally spread across UI turns.
/// Creating every visible pane in one snapshot callback can starve GPUI long
/// enough for the app to look hung.
const PANE_MOUNT_BATCH_SIZE: usize = 1;
const PANE_MOUNT_DELAY: Duration = Duration::from_millis(1);
/// Keep recently visited pane surfaces alive across tab switches. This avoids
/// recreating Metal/Ghostty state on every switch while bounding private socket,
/// framebuffer, and scrollback usage for sessions with many tabs.
const PANE_RUNTIME_CACHE_LIMIT: usize = 24;
/// Herdr does not mark the end of retained EventHub replay. This quiet period
/// only decides when OcHerdr may resume applying incremental payloads; current
/// snapshots are refreshed throughout replay and remain visible immediately.
const STARTUP_REPLAY_QUIET_DELAY: Duration = Duration::from_millis(150);
/// Share of the target pane it keeps on an edge drop (design §5.3: 0.5 in
/// the first version; presets come later).
const PANE_EDGE_DROP_RATIO: f32 = 0.5;
// macOS-style corner hierarchy: compact controls stay tight while sheets and
// panels step up evenly instead of using exaggerated capsule radii.
const CORNER_MODAL: f32 = 14.;
const CORNER_PANEL: f32 = 10.;
const CORNER_CONTROL: f32 = 7.;
const CORNER_COMPACT: f32 = 5.;

#[cfg(target_os = "macos")]
const PRIMARY_SHORTCUT_SYMBOL: &str = "⌘";
#[cfg(not(target_os = "macos"))]
const PRIMARY_SHORTCUT_SYMBOL: &str = "Ctrl+";

fn primary_modifier(modifiers: ochub_ui::gpui::Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    return modifiers.platform;
    #[cfg(not(target_os = "macos"))]
    return modifiers.control;
}

fn only_primary_modifier(modifiers: ochub_ui::gpui::Modifiers) -> bool {
    primary_modifier(modifiers)
        && !modifiers.alt
        && if cfg!(target_os = "macos") {
            !modifiers.control
        } else {
            !modifiers.platform
        }
}

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

#[cfg(target_os = "macos")]
fn terminal_frame_element(
    frame: RenderedFrame,
    frozen_size: Option<(f32, f32)>,
) -> ochub_ui::gpui::AnyElement {
    let surface = surface(frame.pixel_buffer)
        .with_frame_lifetime(frame.lifetime)
        .object_fit(ObjectFit::Contain);
    match frozen_size {
        Some((width, height)) => surface
            .absolute()
            .top_0()
            .left_0()
            .w(px(width))
            .h(px(height))
            .into_any_element(),
        None => surface.w_full().h_full().into_any_element(),
    }
}

#[cfg(not(target_os = "macos"))]
fn terminal_frame_element(
    frame: RenderedFrame,
    frozen_size: Option<(f32, f32)>,
) -> ochub_ui::gpui::AnyElement {
    let font_family = SharedString::from(frame.font_family.to_string());
    let line_height = frame.cell_height_px.max(1) as f32;
    let rows = frame
        .lines
        .iter()
        .map(|line| {
            let runs = line
                .runs
                .iter()
                .map(|run| {
                    let mut terminal_font = font(font_family.clone());
                    terminal_font.weight = if run.bold {
                        GpuiFontWeight::BOLD
                    } else {
                        GpuiFontWeight::NORMAL
                    };
                    terminal_font.style = if run.italic {
                        FontStyle::Italic
                    } else {
                        FontStyle::Normal
                    };
                    let mut color = ochub_ui::gpui::Hsla::from(rgba((run.foreground << 8) | 0xff));
                    if run.dim {
                        color = color.alpha(0.6);
                    }
                    TextRun {
                        len: run.len,
                        font: terminal_font,
                        color,
                        background_color: Some(rgba((run.background << 8) | 0xff).into()),
                        underline: run.underline.then_some(UnderlineStyle {
                            thickness: px(1.),
                            color: None,
                            wavy: false,
                        }),
                        strikethrough: None,
                    }
                })
                .collect::<Vec<_>>();
            div()
                .h(px(line_height))
                .line_height(px(line_height))
                .whitespace_nowrap()
                .child(StyledText::new(line.text.clone()).with_runs(runs))
        })
        .collect::<Vec<_>>();
    let content = div()
        .size_full()
        .overflow_hidden()
        .pl(px(frame.padding_x as f32))
        .pt(px(frame.padding_y as f32))
        .text_size(px(frame.font_size as f32))
        .font_family(font_family)
        .children(rows);
    match frozen_size {
        Some((width, height)) => content
            .absolute()
            .top_0()
            .left_0()
            .w(px(width))
            .h(px(height))
            .into_any_element(),
        None => content.into_any_element(),
    }
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
                                pane.child(terminal_frame_element(frame, None))
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum HostConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Degraded,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupReplaySync {
    Draining { serial: u64 },
    Refreshing,
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
    /// Panes this OcHerdr instance currently controls. A first visible pane
    /// starts with non-takeover control so its PTY adopts the measured local
    /// viewport; direct interaction upgrades an observer with takeover.
    /// Ownership is per terminal, so distinct panes can stay controlled at
    /// the same time. Hidden or remotely taken-over panes are removed.
    controls: HashMap<String, TerminalMode>,
    /// Pane ids for which this session already made its one automatic,
    /// non-takeover control attempt. Busy panes fall back to observation and
    /// must not reconnect-loop; an explicit interaction can still take over.
    automatic_control_attempts: HashSet<String>,
    /// Monotonic recency clock used only for hidden-pane cache eviction.
    access_serial: u64,
}

/// A live Herdr session parked while another host is visible. Ownership of
/// the connection and pane runtimes stays here, so switching hosts does not
/// kill the SSH tunnel or send terminal `Detach` messages.
struct ParkedHostRuntime {
    sessions: Vec<SessionSummary>,
    session_index: Option<usize>,
    connection: SessionConnection,
    herdr_capabilities: HerdrCapabilities,
    event_stream: EventStreamState,
    event_listen: Option<Task<()>>,
    snapshot: Option<HierarchySnapshot>,
    selection: Selection,
    session_panes: Option<SessionPanes>,
    pane_viewports: HashMap<String, MeasuredPaneViewport>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MeasuredPaneViewport {
    body_bounds: (f32, f32, f32, f32),
    pixels: (u32, u32),
    scale_factor: f64,
    /// False after Ghostty resolves the viewport below Herdr's 4x2 minimum.
    /// A changed measurement makes it eligible again.
    mountable: bool,
}

impl SessionPanes {
    fn new(owner: SessionKey) -> Self {
        Self {
            owner,
            panes: HashMap::new(),
            controls: HashMap::new(),
            automatic_control_attempts: HashSet::new(),
            access_serial: 0,
        }
    }
}

struct PaneRuntime {
    /// First-visible panes attempt non-takeover control so the remote PTY is
    /// sized before it paints. Untouched panes observe; panes already controlled
    /// by this OcHerdr keep their stream across tab switches. Explicit
    /// interaction can promote an observer with takeover.
    session: TerminalSession,
    terminal: Terminal,
    frame: Option<RenderedFrame>,
    mode: TerminalMode,
    /// The selected pane: the only one that receives keyboard, IME, and
    /// mouse input and reports terminal focus. It can stay focused while
    /// observing, so a direct interaction can promote it to control.
    focused: bool,
    size: (u16, u16),
    pixel_size: (u32, u32),
    /// True after this pane's body has supplied an actual local viewport.
    /// Bootstrap frames use 80×24 and must not reach the Metal surface first.
    viewport_ready: bool,
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
    /// Latest authoritative pixel size waiting for layout to settle. Older
    /// timer callbacks compare the serial and become no-ops.
    pending_resize: Option<PendingPaneResize>,
    /// Last `SessionPanes::access_serial` at which this pane was visible.
    last_visible_serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingPaneResize {
    serial: u64,
    pixels: (u32, u32),
    scale_factor: f64,
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
    /// Opened from the sidebar agent list: leads with "Details", the only way
    /// that list reaches the agent panel now that a click jumps to the pane.
    agent_details: bool,
}

#[derive(Clone, Debug)]
struct FileContextMenu {
    entry: FileEntry,
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
    /// Live hosts other than `profile_index`. Removing an entry is the
    /// explicit disconnect operation and releases its Herdr clients/tunnel.
    parked_hosts: HashMap<String, ParkedHostRuntime>,
    /// Hosts whose most recent explicit connection attempt failed. This is UI
    /// state only; it never causes an automatic reconnect.
    failed_hosts: HashSet<String>,
    /// What the connected Herdr can do, derived from the last full snapshot.
    herdr_capabilities: HerdrCapabilities,
    event_stream: EventStreamState,
    /// Dropping this cancels the session-wide event await loop.
    event_listen: Option<Task<()>>,
    /// Retained events are invalidations until replay goes quiet. Unlike the
    /// old barrier, every replay batch refreshes the visible current snapshot.
    startup_replay_sync: Option<StartupReplaySync>,
    startup_replay_serial: u64,
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
    update_info: Option<update::UpdateInfo>,
    update_state: update::UpdateState,
    update_checking: bool,
    update_installing: bool,
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
    /// Geometry belongs to the lightweight pane shell, not the Ghostty
    /// runtime. It lets a pane be measured before its native surface exists.
    pane_viewports: HashMap<String, MeasuredPaneViewport>,
    pane_mount_scheduled: bool,
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
    /// The last key-down was an OcHerdr shortcut; swallow its key-up.
    suppress_key_release: bool,
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
    /// One-shot tab-bar detach (`pane.move` to a new or existing tab).
    pane_detaches: HashMap<String, PendingPaneDetach>,
    /// Whole-tab 2/3/4-pane template rebuilds. Like a relocation, these keep
    /// the final geometry optimistic while Herdr parks and reinserts panes.
    pane_template_commits: HashMap<String, PendingPaneTemplateCommit>,
    /// A cancelled or invalid pane drag flying back to its slot.
    pane_drag_return: Option<PaneDragReturn>,
    /// Tabs observed in a render while their terminal geometry was frozen.
    /// The first authoritative render removes the id and actively thaws every
    /// cached pane body; equal preview/final rects otherwise skip GPUI measure.
    pane_resize_frozen_tabs: HashSet<String>,
    /// Monotonic identity for coalesced terminal-resize commits.
    pane_resize_serial: u64,
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
    file_panel: FilePanelState,
    file_name_input: Entity<TextInput>,
    file_path_input: Entity<TextInput>,
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
    FileContextMenu(FileContextMenu),
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
    ConfirmBulkRemove,
    AgentPanel {
        pane_id: String,
    },
    Update(UpdateDialog),
}

#[derive(Clone, Debug)]
enum UpdateDialog {
    Checking,
    Current {
        version: String,
    },
    Available(update::UpdateInfo),
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Failed {
        message: String,
        release_url: String,
    },
    Installed {
        version: String,
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
                | Self::ConfirmBulkRemove
                | Self::Update(_)
        )
    }

    fn host_center(&self) -> bool {
        matches!(
            self,
            Self::NodeManager
                | Self::RemoteForm(_)
                | Self::ConfirmRemoveProfile(_)
                | Self::ConfirmBulkRemove
        )
    }
}

fn key_goes_to_terminal(overlay: &Overlay) -> bool {
    matches!(overlay, Overlay::None)
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

/// Adds a discoverable hover label to a compact icon action. Deferred paint
/// keeps the tooltip above terminal textures and other later siblings. The
/// label stays pointer-transparent so an invisible tooltip cannot mask nearby
/// controls before it is shown.
fn icon_action_tooltip(
    group: &'static str,
    label: impl Into<SharedString>,
    action: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    div()
        .relative()
        .flex_none()
        .group(group)
        .child(action)
        .child(
            deferred(
                anchored()
                    .anchor(Anchor::TopCenter)
                    .offset(point(px(0.), px(7.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id(ElementId::Name(format!("{group}-popup").into()))
                            .role(ochub_ui::gpui::Role::Tooltip)
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .max_w(px(260.))
                            .px_2()
                            .py_1()
                            .rounded(px(CORNER_COMPACT))
                            .border_1()
                            .border_color(theme::border())
                            .bg(theme::overlay())
                            .shadow(theme::shadow_popover())
                            .text_xs()
                            .text_color(theme::text())
                            .whitespace_normal()
                            .child(label),
                    ),
            )
            .priority(40),
        )
}

fn quit_app(_: &Quit, cx: &mut App) {
    cx.quit();
}

fn main() {
    application()
        .with_assets(OcHerdrAssets)
        .run(|cx: &mut App| {
            cx.set_app_identity("io.github.ochub-team.ocherdr", "OcHerdr");
            cx.on_system_notification_response(|_, cx| cx.activate(true));
            let loaded = load_settings();
            let menu_i18n = I18n::new(loaded.language);
            I18n::install(loaded.language);
            if let Some(directory) = dirs::config_dir() {
                theme::set_themes_dir(directory.join("OcHerdr/themes"));
            }
            ochub_ui::install(cx);
            cx.on_action(quit_app);
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus([Menu::new("OcHerdr").items([
                MenuItem::action(menu_i18n.text(k::UPDATE_MENU_CHECK), CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Quit OcHerdr", Quit),
            ])]);
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
mod main_tests;
