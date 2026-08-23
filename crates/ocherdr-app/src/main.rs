use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use gpui_platform::application;
use ocherdr_core::{
    AgentStatus, AgentStatusHandoff, ConnectionProfile, HerdrEvent, HierarchySnapshot, LayoutRect,
    LayoutSplit, PaneInfo, Selection, SessionSummary, SnapshotUpdate, SplitDirection, WorktreeInfo,
    WorktreeSourceInfo, split_ratio_from_drag,
};
use ocherdr_herdr::{
    EventSubscription, HerdrError, HostHealthStatus, SessionConnection, TerminalCommand,
    TerminalMode, TerminalSession, attach_command, discover_sessions, open_system_terminal,
    request_socket,
};
use ocherdr_terminal::{KeyModifiers, RenderedFrame, Terminal, TerminalPalette};
use ochub_ui::components::{
    ButtonSize, ButtonTone, button, context_menu, context_menu_item, empty_state, field,
    icon_button_tone, icon_only_button_tone, modal_body, modal_card, modal_footer, modal_header,
    modal_overlay, spinner, status_dot,
};
use ochub_ui::gpui::{
    App, AppContext, AssetSource, Bounds, ClipboardItem, Context, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, FontWeight, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Render, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, Task, TitlebarOptions, UTF16Selection, WeakEntity, Window,
    WindowAppearance, WindowBounds, WindowOptions, canvas, div, point, prelude::*, px, relative,
    size, surface,
};
use ochub_ui::icons::{IconName, icon};
use ochub_ui::notifications::NotificationHost;
use ochub_ui::text_input::TextInput;
use ochub_ui::{assets, theme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod a11y;
mod controller;
mod fonts;
mod host_center;
mod i18n;
mod ime;
mod notify;
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
    P100,
    #[default]
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
    #[serde(default)]
    appearance: AppearanceSettings,
    #[serde(default)]
    language: Language,
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
    prefix_pending: bool,
    surface_drag: SurfaceDrag,
    terminal_surface_bounds: Option<(f32, f32, f32, f32)>,
    ime_marked: Option<String>,
    rename_input: Entity<TextInput>,
    worktree_label_input: Entity<TextInput>,
    worktree_branch_input: Entity<TextInput>,
    worktree_base_input: Entity<TextInput>,
    worktree_path_input: Entity<TextInput>,
    /// Dropping this cancels an in-flight `worktree.list`.
    worktree_list_task: Option<Task<()>>,
    appearance: AppearanceSettings,
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

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("OcHerdr/connections.json"))
}

fn load_settings() -> Settings {
    settings_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
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
            let settings = load_settings();
            I18n::install(settings.language);
            if let Some(directory) = dirs::config_dir() {
                theme::set_themes_dir(directory.join("OcHerdr/themes"));
            }
            ochub_ui::install(cx);
            install_appearance(&settings.appearance, cx.window_appearance());
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
                    ..Default::default()
                },
                move |window, cx| {
                    window.set_window_title("OcHerdr");
                    let view = cx.new(|cx| OcHerdrView::new(settings, window, cx));
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

    #[test]
    fn legacy_connection_settings_receive_the_default_appearance() {
        let settings: Settings = serde_json::from_str(r#"{"connections":[]}"#).unwrap();

        assert_eq!(
            settings.appearance.theme_family,
            theme::DEFAULT_THEME_FAMILY
        );
        assert_eq!(settings.appearance.mode, AppearanceMode::Dark);
        assert_eq!(settings.appearance.backdrop, BackdropMode::Blurred);
        assert_eq!(settings.appearance.background_opacity.value(), 92);
        assert_eq!(settings.appearance.font, TerminalFontSettings::default());
        assert_eq!(settings.appearance.font.size.value(), 13);
        assert!(settings.appearance.font.ligatures);
        assert_eq!(settings.language, Language::System);
        assert!(settings.host_metadata.is_empty());
        assert!(settings.host_groups.is_empty());
        assert!(settings.host_health.is_empty());
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
