use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use gpui_platform::application;
use ocherdr_core::{
    AgentStatus, ConnectionProfile, HerdrEvent, HierarchySnapshot, PaneInfo, Selection,
    SessionSummary, SnapshotUpdate, SplitDirection,
};
use ocherdr_herdr::{
    EventSubscription, HerdrError, HostHealthCheck, HostHealthStatus, SessionConnection,
    TerminalCommand, TerminalMode, TerminalSession, attach_command, check_host, discover_sessions,
    open_system_terminal, request_socket, ssh_host_aliases, ssh_login_command,
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
    WindowAppearance, WindowBounds, WindowOptions, canvas, div, point, prelude::*, px, size,
    surface,
};
use ochub_ui::icons::{IconName, icon};
use ochub_ui::text_input::{TextInput, TextInputEvent};
use ochub_ui::{assets, theme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod a11y;
mod controller;
mod fonts;
mod i18n;
mod ime;
mod ui;

use i18n::{I18n, Language};

const SIDEBAR_WIDTH: f32 = 252.;
const HEADER_HEIGHT: f32 = 46.;
const TAB_PILL_HEIGHT: f32 = 28.;
const STATUS_BAR_HEIGHT: f32 = 28.;
const PANE_HEADER_HEIGHT: f32 = 26.;
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

    fn kind_label(&self) -> &'static str {
        match self {
            Self::Workspace { .. } => "workspace",
            Self::Tab { .. } => "tab",
            Self::Pane { .. } => "pane",
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
    #[serde(default = "default_background_opacity")]
    background_opacity: u8,
    #[serde(default)]
    font: TerminalFontSettings,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme_family: default_theme_family(),
            mode: AppearanceMode::Dark,
            backdrop: BackdropMode::Blurred,
            background_opacity: default_background_opacity(),
            font: TerminalFontSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TerminalFontSettings {
    #[serde(default)]
    family: String,
    #[serde(default = "default_font_size")]
    size: u8,
    #[serde(default = "default_true")]
    ligatures: bool,
    #[serde(default)]
    thicken: bool,
    #[serde(default)]
    cell_width_percent: i8,
    #[serde(default)]
    cell_height_percent: i8,
}

impl Default for TerminalFontSettings {
    fn default() -> Self {
        Self {
            family: String::new(),
            size: default_font_size(),
            ligatures: true,
            thicken: false,
            cell_width_percent: 0,
            cell_height_percent: 0,
        }
    }
}

impl TerminalFontSettings {
    fn clamped(self) -> Self {
        Self {
            size: self.size.clamp(8, 32),
            cell_width_percent: self.cell_width_percent.clamp(-30, 30),
            cell_height_percent: self.cell_height_percent.clamp(-30, 40),
            ..self
        }
    }
}

fn default_theme_family() -> String {
    theme::DEFAULT_THEME_FAMILY.to_owned()
}

const fn default_background_opacity() -> u8 {
    92
}

const fn default_font_size() -> u8 {
    13
}

const fn default_true() -> bool {
    true
}

const FONT_SIZES: [u8; 8] = [11, 12, 13, 14, 15, 16, 18, 20];
const CELL_WIDTHS: [i8; 3] = [-10, 0, 10];
const CELL_HEIGHTS: [i8; 4] = [-8, 0, 12, 20];

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
    Checking,
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

struct OcHerdrView {
    profiles: Vec<ConnectionProfile>,
    profile_index: usize,
    sessions: Vec<SessionSummary>,
    session_index: Option<usize>,
    connection: Option<SessionConnection>,
    event_stream: EventStreamState,
    /// Dropping this cancels the event await loop.
    event_listen: Option<Task<()>>,
    snapshot: Option<HierarchySnapshot>,
    selection: Selection,
    operation: Option<SharedString>,
    error: Option<SharedString>,
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
    managed_profile_index: usize,
    remote_advanced_open: bool,
    recent_connection_ids: Vec<String>,
    host_metadata: HashMap<String, HostMetadata>,
    host_groups: Vec<String>,
    host_health: HashMap<String, HostHealthView>,
    host_filter: HostFilter,
    host_check_epoch: u64,
    host_check_queue: VecDeque<(u64, String, ConnectionProfile)>,
    host_checks_running: usize,
    host_bulk_mode: bool,
    host_bulk_selection: HashSet<String>,
    orphaned_ssh_hosts: HashSet<String>,
    prefix_pending: bool,
    text_drag_pane: Option<String>,
    ime_marked: Option<String>,
    remote_label: Entity<TextInput>,
    remote_destination: Entity<TextInput>,
    remote_port: Entity<TextInput>,
    remote_identity_file: Entity<TextInput>,
    remote_herdr_path: Entity<TextInput>,
    remote_group: Entity<TextInput>,
    remote_tags: Entity<TextInput>,
    remote_search: Entity<TextInput>,
    rename_input: Entity<TextInput>,
    appearance: AppearanceSettings,
    i18n: I18n,
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
    ConfirmRemoveProfile(usize),
    ConfirmSwitchProfile { index: usize, from_hosts: bool },
    ConfirmBulkRemove,
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
            Self::ThisMac => "This Mac",
            Self::Saved => "Saved",
            Self::SshConfig => "SSH config",
        })
    }

    fn description(self, i18n: I18n) -> &'static str {
        i18n.text(match self {
            Self::ThisMac => "Herdr on this computer",
            Self::Saved => "Saved in OcHerdr",
            Self::SshConfig => "Read-only from ~/.ssh/config",
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
        i18n.text("This Mac").to_owned()
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

fn install_appearance(
    appearance: &AppearanceSettings,
    window_appearance: WindowAppearance,
) -> String {
    let mut family =
        theme::find_family(&appearance.theme_family).unwrap_or_else(theme::ochub_family);
    let effect = appearance.backdrop.theme_effect();
    let content_opacity = if appearance.backdrop == BackdropMode::Opaque {
        100
    } else {
        appearance.background_opacity.clamp(40, 100)
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
    let installed_id = family.id.clone();
    theme::install_family(&family, appearance.mode.theme_mode(), window_appearance);
    installed_id
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

fn save_settings(
    profiles: &[ConnectionProfile],
    recent_connection_ids: &[String],
    host_metadata: &HashMap<String, HostMetadata>,
    host_groups: &[String],
    host_health: &HashMap<String, HostHealthView>,
    appearance: &AppearanceSettings,
    language: Language,
) -> std::result::Result<(), String> {
    let connections = profiles
        .iter()
        .filter(|profile| is_saved_profile(profile))
        .cloned()
        .collect();
    let settings = Settings {
        connections,
        recent_connection_ids: recent_connection_ids.to_vec(),
        host_metadata: host_metadata.clone(),
        host_groups: host_groups.to_vec(),
        host_health: host_health
            .iter()
            .filter_map(|(id, health)| match health {
                HostHealthView::Checking => None,
                HostHealthView::Checked { cached, .. } => Some((id.clone(), cached.clone())),
            })
            .collect(),
        appearance: appearance.clone(),
        language,
    };
    let path =
        settings_path().ok_or_else(|| "Application Support directory is unavailable".to_owned())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Settings path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn main() {
    application()
        .with_assets(OcHerdrAssets)
        .run(|cx: &mut App| {
            let mut settings = load_settings();
            let _ = I18n::new(settings.language);
            ochub_ui::install(cx);
            settings.appearance.theme_family =
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
                    window
                        .observe_window_appearance(|window, cx| {
                            let settings = load_settings();
                            if settings.appearance.mode == AppearanceMode::System {
                                install_appearance(&settings.appearance, window.appearance());
                                theme::apply_window_background(window);
                                cx.refresh_windows();
                            }
                        })
                        .detach();
                    let view = cx.new(|cx| OcHerdrView::new(settings, cx));
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
        assert_eq!(settings.appearance.background_opacity, 92);
        assert_eq!(settings.appearance.font, TerminalFontSettings::default());
        assert_eq!(settings.appearance.font.size, 13);
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
            Overlay::ConfirmRemoveProfile(0),
            Overlay::ConfirmSwitchProfile {
                index: 0,
                from_hosts: false,
            },
            Overlay::ConfirmSwitchProfile {
                index: 1,
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
}
