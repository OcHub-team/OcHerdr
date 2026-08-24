use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use gpui_platform::application;
use ocherdr_core::{
    AgentInfo, AgentNameError, AgentStatus, AgentStatusHandoff, ConnectionProfile, HerdrEvent,
    HierarchySnapshot, LayoutRect, LayoutSplit, PaneInfo, ReorderHover, Selection, SessionSummary,
    SnapshotUpdate, SplitDirection, WorktreeInfo, WorktreeSourceInfo, reorder_insert_index,
    split_ratio_from_drag,
};
use ocherdr_herdr::{
    EventSubscription, HerdrError, HostHealthStatus, SessionConnection, TerminalCommand,
    TerminalMode, TerminalSession, attach_command, discover_sessions, open_system_terminal,
    request_socket,
};
use ocherdr_terminal::{KeyModifiers, RenderedFrame, Terminal, TerminalPalette};
use ochub_ui::components::{
    ButtonSize, ButtonTone, busy_button, button, context_menu, context_menu_item, disabled_button,
    empty_state, field, field_with_error, icon_button_tone, icon_only_button_tone, modal_body,
    modal_card, modal_footer, modal_header, modal_overlay, spinner, status_dot,
};
use ochub_ui::gpui::{
    Animation, AnimationExt, App, AppContext, AssetSource, Bounds, ClipboardItem, Context,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, Task,
    TitlebarOptions, UTF16Selection, WeakEntity, Window, WindowAppearance, WindowBounds,
    WindowOptions, canvas, div, ease_out_quint, point, prelude::*, px, relative, size, surface,
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
const STATUS_BAR_HEIGHT: f32 = 28.;
const PANE_HEADER_HEIGHT: f32 = 26.;
const SPLIT_HANDLE_HIT_PX: f32 = 10.;
const SPLIT_HANDLE_VISUAL_PX: f32 = 4.;
const REORDER_SLOP_PX: f32 = 4.;
const TAB_REORDER_GAP_PX: f32 = 4.;
const REORDER_ANIMATION: Duration = Duration::from_millis(180);
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
    /// Observe for every pane that is not selected; takeover for the selected
    /// pane. Snapshot panes stay alive across tabs so hidden terminals keep
    /// their Ghostty surface, last Metal frame, and observe stream.
    session: TerminalSession,
    terminal: Terminal,
    frame: Option<RenderedFrame>,
    mode: TerminalMode,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AppearanceSettings {
    #[serde(default = "default_theme_family")]
    theme_family: String,
    #[serde(default)]
    mode: AppearanceMode,
    #[serde(default)]
    backdrop: BackdropMode,
    #[serde(default)]
    background_opacity: OpacityChoice,
    #[serde(default)]
    font: TerminalFontSettings,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme_family: default_theme_family(),
            mode: AppearanceMode::Dark,
            backdrop: BackdropMode::Blurred,
            background_opacity: OpacityChoice::default(),
            font: TerminalFontSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TerminalFontSettings {
    #[serde(default)]
    family: String,
    #[serde(default)]
    size: FontSizeChoice,
    #[serde(default = "default_true")]
    ligatures: bool,
    #[serde(default)]
    thicken: bool,
    #[serde(default)]
    cell_width_percent: CellWidthChoice,
    #[serde(default)]
    cell_height_percent: CellHeightChoice,
}

impl Default for TerminalFontSettings {
    fn default() -> Self {
        Self {
            family: String::new(),
            size: FontSizeChoice::default(),
            ligatures: true,
            thicken: false,
            cell_width_percent: CellWidthChoice::default(),
            cell_height_percent: CellHeightChoice::default(),
        }
    }
}

fn default_theme_family() -> String {
    theme::DEFAULT_THEME_FAMILY.to_owned()
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OpacityChoice {
    #[default]
    P100,
    P92,
    P84,
    P72,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum FontSizeChoice {
    Pt11,
    Pt12,
    #[default]
    Pt13,
    Pt14,
    Pt15,
    Pt16,
    Pt18,
    Pt20,
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

impl OpacityChoice {
    const ALL: [Self; 4] = [Self::P100, Self::P92, Self::P84, Self::P72];

    fn nearest(value: u8) -> Self {
        let mut best = Self::P100;
        let mut best_diff = best.value().abs_diff(value);
        for choice in Self::ALL {
            let diff = choice.value().abs_diff(value);
            if diff < best_diff {
                best = choice;
                best_diff = diff;
            }
        }
        best
    }

    fn index(self) -> usize {
        match self {
            Self::P100 => 0,
            Self::P92 => 1,
            Self::P84 => 2,
            Self::P72 => 3,
        }
    }

    fn value(self) -> u8 {
        match self {
            Self::P100 => 100,
            Self::P92 => 92,
            Self::P84 => 84,
            Self::P72 => 72,
        }
    }
}

impl FontSizeChoice {
    const ALL: [Self; 8] = [
        Self::Pt11,
        Self::Pt12,
        Self::Pt13,
        Self::Pt14,
        Self::Pt15,
        Self::Pt16,
        Self::Pt18,
        Self::Pt20,
    ];

    fn nearest(value: u8) -> Self {
        let mut best = Self::Pt11;
        let mut best_diff = best.value().abs_diff(value);
        for choice in Self::ALL {
            let diff = choice.value().abs_diff(value);
            if diff < best_diff {
                best = choice;
                best_diff = diff;
            }
        }
        best
    }

    fn index(self) -> usize {
        match self {
            Self::Pt11 => 0,
            Self::Pt12 => 1,
            Self::Pt13 => 2,
            Self::Pt14 => 3,
            Self::Pt15 => 4,
            Self::Pt16 => 5,
            Self::Pt18 => 6,
            Self::Pt20 => 7,
        }
    }

    fn value(self) -> u8 {
        match self {
            Self::Pt11 => 11,
            Self::Pt12 => 12,
            Self::Pt13 => 13,
            Self::Pt14 => 14,
            Self::Pt15 => 15,
            Self::Pt16 => 16,
            Self::Pt18 => 18,
            Self::Pt20 => 20,
        }
    }
}

impl CellWidthChoice {
    const ALL: [Self; 3] = [Self::Tight, Self::Normal, Self::Wide];

    fn nearest(value: i8) -> Self {
        let mut best = Self::Tight;
        let mut best_diff = best.value().abs_diff(value);
        for choice in Self::ALL {
            let diff = choice.value().abs_diff(value);
            if diff < best_diff {
                best = choice;
                best_diff = diff;
            }
        }
        best
    }

    fn index(self) -> usize {
        match self {
            Self::Tight => 0,
            Self::Normal => 1,
            Self::Wide => 2,
        }
    }

    fn value(self) -> i8 {
        match self {
            Self::Tight => -10,
            Self::Normal => 0,
            Self::Wide => 10,
        }
    }
}

impl CellHeightChoice {
    const ALL: [Self; 4] = [Self::Compact, Self::Normal, Self::Relaxed, Self::Loose];

    fn nearest(value: i8) -> Self {
        let mut best = Self::Compact;
        let mut best_diff = best.value().abs_diff(value);
        for choice in Self::ALL {
            let diff = choice.value().abs_diff(value);
            if diff < best_diff {
                best = choice;
                best_diff = diff;
            }
        }
        best
    }

    fn index(self) -> usize {
        match self {
            Self::Compact => 0,
            Self::Normal => 1,
            Self::Relaxed => 2,
            Self::Loose => 3,
        }
    }

    fn value(self) -> i8 {
        match self {
            Self::Compact => -8,
            Self::Normal => 0,
            Self::Relaxed => 12,
            Self::Loose => 20,
        }
    }
}

impl Serialize for OpacityChoice {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(serializer)
    }
}

impl Serialize for FontSizeChoice {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(serializer)
    }
}

impl Serialize for CellWidthChoice {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(serializer)
    }
}

impl Serialize for CellHeightChoice {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OpacityChoice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::nearest(u8::deserialize(deserializer)?))
    }
}

impl<'de> Deserialize<'de> for FontSizeChoice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::nearest(u8::deserialize(deserializer)?))
    }
}

impl<'de> Deserialize<'de> for CellWidthChoice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::nearest(i8::deserialize(deserializer)?))
    }
}

impl<'de> Deserialize<'de> for CellHeightChoice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::nearest(i8::deserialize(deserializer)?))
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
    prefix_pending: bool,
    surface_drag: SurfaceDrag,
    pending_reorder: Option<PendingReorder>,
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

impl Overlay {
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
    Text { pane_id: String },
    Split(SplitDrag),
    Reorder(ReorderDrag),
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
        appearance.background_opacity.value()
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
    fn appearance_settings_snap_values_that_are_not_in_the_select_lists() {
        let appearance: AppearanceSettings = serde_json::from_value(json!({
            "theme_family": "kept-as-is",
            "background_opacity": 0,
            "font": {
                "size": 17,
                "cell_width_percent": 50,
                "cell_height_percent": -3
            }
        }))
        .unwrap();

        assert_eq!(appearance.theme_family, "kept-as-is");
        assert_eq!(appearance.background_opacity.value(), 72);
        assert_eq!(appearance.font.size.value(), 16);
        assert_eq!(appearance.font.cell_width_percent.value(), 10);
        assert_eq!(appearance.font.cell_height_percent.value(), 0);

        let already_valid: AppearanceSettings = serde_json::from_value(json!({
            "background_opacity": 92,
            "font": { "size": 13, "cell_width_percent": 0, "cell_height_percent": 0 }
        }))
        .unwrap();
        assert_eq!(already_valid.background_opacity.value(), 92);
        assert_eq!(already_valid.font.size.value(), 13);
        assert_eq!(already_valid.font.cell_width_percent.value(), 0);
        assert_eq!(already_valid.font.cell_height_percent.value(), 0);
    }

    /// The select row paints the option at index() as the current one, so a
    /// variant inserted into ALL without renumbering would silently select a
    /// neighbour. Exhaustiveness is checked by the compiler; the numbering is not.
    #[test]
    fn every_choice_reports_the_index_it_occupies_in_its_option_list() {
        for (position, choice) in OpacityChoice::ALL.into_iter().enumerate() {
            assert_eq!(choice.index(), position, "opacity {choice:?}");
        }
        for (position, choice) in FontSizeChoice::ALL.into_iter().enumerate() {
            assert_eq!(choice.index(), position, "font size {choice:?}");
        }
        for (position, choice) in CellWidthChoice::ALL.into_iter().enumerate() {
            assert_eq!(choice.index(), position, "cell width {choice:?}");
        }
        for (position, choice) in CellHeightChoice::ALL.into_iter().enumerate() {
            assert_eq!(choice.index(), position, "cell height {choice:?}");
        }
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
}
