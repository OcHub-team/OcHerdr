use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use gpui_platform::application;
use ocherdr_core::{
    AgentStatus, ConnectionProfile, HierarchySnapshot, PaneInfo, Selection, SessionSummary,
    SplitDirection,
};
use ocherdr_herdr::{
    EventSubscription, HerdrError, SessionConnection, TerminalCommand, TerminalMode,
    TerminalSession, attach_command, discover_sessions, open_system_terminal, request_socket,
    ssh_host_aliases,
};
use ocherdr_terminal::{KeyModifiers, RenderedFrame, Terminal};
use ochub_ui::components::{
    ButtonSize, ButtonTone, button, context_menu, context_menu_item, empty_state, field,
    icon_button_tone, icon_only_button_tone, modal_body, modal_card, modal_footer, modal_header,
    modal_overlay, segmented, spinner, status_dot,
};
use ochub_ui::gpui::{
    App, AppContext, AssetSource, Bounds, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ObjectFit, Render, SharedString,
    TitlebarOptions, Window, WindowAppearance, WindowBounds, WindowOptions, div, point, prelude::*,
    px, size, surface,
};
use ochub_ui::icons::{IconName, icon};
use ochub_ui::text_input::{TextInput, TextInputEvent};
use ochub_ui::{assets, theme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod controller;
mod i18n;
mod ui;

use i18n::{I18n, Language};

const SIDEBAR_WIDTH: f32 = 252.;
const HEADER_HEIGHT: f32 = 46.;
const STATUS_BAR_HEIGHT: f32 = 28.;
const PANE_HEADER_HEIGHT: f32 = 26.;
// macOS-style corner hierarchy: compact controls stay tight while sheets and
// panels step up evenly instead of using exaggerated capsule radii.
const CORNER_MODAL: f32 = 14.;
const CORNER_PANEL: f32 = 10.;
const CORNER_CONTROL: f32 = 7.;
const CORNER_COMPACT: f32 = 5.;
const HERDR_SETTINGS_SECTIONS: [&str; 6] = [
    "Theme",
    "Indicators",
    "Sound",
    "Toast",
    "Pane labels",
    "Integrations",
];

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
    events: Option<EventSubscription>,
    snapshot: Option<HierarchySnapshot>,
}

struct PaneRuntime {
    session: TerminalSession,
    terminal: Terminal,
    frame: Option<RenderedFrame>,
    mode: TerminalMode,
    size: (u16, u16),
    pixel_size: (u32, u32),
    frame_context: u64,
    color_scheme_dark: bool,
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
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme_family: default_theme_family(),
            mode: AppearanceMode::Dark,
            backdrop: BackdropMode::Blurred,
            background_opacity: default_background_opacity(),
        }
    }
}

fn default_theme_family() -> String {
    theme::DEFAULT_THEME_FAMILY.to_owned()
}

const fn default_background_opacity() -> u8 {
    92
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    connections: Vec<ConnectionProfile>,
    #[serde(default)]
    appearance: AppearanceSettings,
    #[serde(default)]
    language: Language,
}

struct OcHerdrView {
    profiles: Vec<ConnectionProfile>,
    profile_index: usize,
    sessions: Vec<SessionSummary>,
    session_index: Option<usize>,
    connection: Option<SessionConnection>,
    events: Option<EventSubscription>,
    snapshot: Option<HierarchySnapshot>,
    selection: Selection,
    operation: Option<SharedString>,
    error: Option<SharedString>,
    focus: FocusHandle,
    load_epoch: u64,
    event_epoch: u64,
    snapshot_refreshing: bool,
    terminal_epoch: u64,
    panes: HashMap<String, PaneRuntime>,
    node_manager_open: bool,
    add_remote_open: bool,
    appearance_open: bool,
    herdr_settings_open: bool,
    herdr_settings_section: usize,
    managed_profile_index: usize,
    pending_remove_profile: Option<usize>,
    pending_close: Option<HierarchyTarget>,
    rename_target: Option<HierarchyTarget>,
    context_menu: Option<HierarchyContextMenu>,
    prefix_pending: bool,
    remote_label: Entity<TextInput>,
    remote_destination: Entity<TextInput>,
    remote_port: Entity<TextInput>,
    remote_identity_file: Entity<TextInput>,
    remote_herdr_path: Entity<TextInput>,
    remote_search: Entity<TextInput>,
    rename_input: Entity<TextInput>,
    appearance: AppearanceSettings,
    i18n: I18n,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectionSource {
    Current,
    Saved,
    SshConfig,
}

impl ConnectionSource {
    fn label(self, i18n: I18n) -> &'static str {
        i18n.text(match self {
            Self::Current => "CURRENT",
            Self::Saved => "SAVED",
            Self::SshConfig => "SSH CONFIG",
        })
    }

    fn description(self, i18n: I18n) -> &'static str {
        i18n.text(match self {
            Self::Current => "This Mac",
            Self::Saved => "Saved in OcHerdr",
            Self::SshConfig => "Imported from ~/.ssh/config",
        })
    }
}

fn connection_source(profile: &ConnectionProfile) -> ConnectionSource {
    if matches!(profile, ConnectionProfile::Local { .. }) {
        ConnectionSource::Current
    } else if profile.id().starts_with("manual-") {
        ConnectionSource::Saved
    } else {
        ConnectionSource::SshConfig
    }
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
    appearance: &AppearanceSettings,
    language: Language,
) -> std::result::Result<(), String> {
    let connections = profiles
        .iter()
        .filter(|profile| profile.id().starts_with("manual-"))
        .cloned()
        .collect();
    let settings = Settings {
        connections,
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
        assert_eq!(settings.language, Language::System);
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
}
