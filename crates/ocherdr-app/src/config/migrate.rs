//! First-launch move of `appearance` / `language` out of connections.json.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::NamedTempFile;

use crate::i18n::Language;
use crate::{AppearanceSettings, Settings};

use super::document::ConfigDocument;
use super::values::{AppConfig, appearance_from_config};

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub dir: PathBuf,
}

impl AppPaths {
    pub fn user() -> Option<Self> {
        dirs::config_dir().map(|directory| Self {
            dir: directory.join("OcHerdr"),
        })
    }

    pub fn connections(&self) -> PathBuf {
        self.dir.join("connections.json")
    }

    pub fn config(&self) -> PathBuf {
        self.dir.join("config")
    }

    pub fn themes(&self) -> PathBuf {
        self.dir.join("themes")
    }

    pub fn ui_state(&self) -> PathBuf {
        self.dir.join("ui-state.json")
    }
}

#[derive(Clone)]
pub struct LoadedApp {
    pub settings: Settings,
    pub appearance: AppearanceSettings,
    pub language: Language,
    pub document: ConfigDocument,
    pub ui_state: UiState,
    pub persistence: ConfigStore,
    pub storage_notices: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PersistenceAccess {
    pub connections: bool,
    pub config: bool,
    pub ui_state: bool,
}

#[derive(Clone)]
pub struct ConfigStore {
    inner: Arc<Mutex<ConfigStoreInner>>,
}

struct ConfigStoreInner {
    paths: AppPaths,
    access: PersistenceAccess,
    connections: Option<Vec<u8>>,
    config: Option<Vec<u8>>,
    ui_state: Option<Vec<u8>>,
    #[cfg(test)]
    _temporary: Option<Arc<tempfile::TempDir>>,
}

#[derive(Clone, Copy)]
enum StoreDomain {
    Connections,
    Config,
    UiState,
}

impl ConfigStore {
    fn new(paths: AppPaths, mut access: PersistenceAccess) -> Self {
        let connections = initial_snapshot(&paths.connections(), &mut access.connections);
        let config = initial_snapshot(&paths.config(), &mut access.config);
        let ui_state = initial_snapshot(&paths.ui_state(), &mut access.ui_state);
        Self {
            inner: Arc::new(Mutex::new(ConfigStoreInner {
                paths,
                access,
                connections,
                config,
                ui_state,
                #[cfg(test)]
                _temporary: None,
            })),
        }
    }

    pub fn disabled() -> Self {
        Self::new(
            AppPaths {
                dir: PathBuf::new(),
            },
            PersistenceAccess::default(),
        )
    }

    #[cfg(all(test, unix))]
    pub fn temporary() -> Self {
        let temporary = Arc::new(tempfile::TempDir::new().expect("temporary config directory"));
        let store = Self::new(
            AppPaths {
                dir: temporary.path().to_path_buf(),
            },
            PersistenceAccess {
                connections: true,
                config: true,
                ui_state: true,
            },
        );
        store.inner.lock()._temporary = Some(temporary);
        store
    }

    #[cfg(test)]
    fn access(&self) -> PersistenceAccess {
        self.inner.lock().access
    }

    pub fn write_connections(&self, settings: &Settings) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
        self.write(StoreDomain::Connections, &bytes)
    }

    pub fn write_config(&self, document: &ConfigDocument) -> Result<(), String> {
        let path = self.inner.lock().paths.config();
        if document.is_empty() && !path.exists() {
            return Ok(());
        }
        self.write(StoreDomain::Config, document.serialize().as_bytes())
    }

    pub fn write_ui_state(&self, state: &UiState) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
        self.write(StoreDomain::UiState, &bytes)
    }

    fn write(&self, domain: StoreDomain, bytes: &[u8]) -> Result<(), String> {
        let mut inner = self.inner.lock();
        let path = match domain {
            StoreDomain::Connections => inner.paths.connections(),
            StoreDomain::Config => inner.paths.config(),
            StoreDomain::UiState => inner.paths.ui_state(),
        };
        let writable = match domain {
            StoreDomain::Connections => inner.access.connections,
            StoreDomain::Config => inner.access.config,
            StoreDomain::UiState => inner.access.ui_state,
        };
        if !writable {
            return Err(format!(
                "{} was not loaded safely; refusing to overwrite it",
                path.display()
            ));
        }
        let lock_path = lock_path(&path);
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| error.to_string())?;
        lock.lock_exclusive().map_err(|error| error.to_string())?;
        let current = read_optional(&path)?;
        let expected = match domain {
            StoreDomain::Connections => &inner.connections,
            StoreDomain::Config => &inner.config,
            StoreDomain::UiState => &inner.ui_state,
        };
        if &current != expected {
            match domain {
                StoreDomain::Connections => inner.access.connections = false,
                StoreDomain::Config => inner.access.config = false,
                StoreDomain::UiState => inner.access.ui_state = false,
            }
            return Err(format!(
                "{} changed in another OcHerdr process; refusing to overwrite the newer data",
                path.display()
            ));
        }
        atomic_write(&path, bytes)?;
        match domain {
            StoreDomain::Connections => inner.connections = Some(bytes.to_vec()),
            StoreDomain::Config => inner.config = Some(bytes.to_vec()),
            StoreDomain::UiState => inner.ui_state = Some(bytes.to_vec()),
        }
        Ok(())
    }
}

const UI_STATE_SCHEMA_VERSION: u32 = 1;

const fn ui_state_schema_version() -> u32 {
    UI_STATE_SCHEMA_VERSION
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default = "ui_state_schema_version")]
    pub(crate) schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_connection_id: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            schema_version: UI_STATE_SCHEMA_VERSION,
            selected_connection_id: None,
        }
    }
}

enum FileLoad<T> {
    Missing,
    Loaded(T),
    Recovered { value: T, notice: String },
    Protected { detail: String },
}

pub fn load_app(paths: &AppPaths) -> LoadedApp {
    let connections_path = paths.connections();
    let mut storage_notices = Vec::new();
    let (json, mut connections_writable) = match load_file(&connections_path, parse_connections) {
        FileLoad::Missing => (json!({}), true),
        FileLoad::Loaded(json) => (json, true),
        FileLoad::Recovered { value, notice } => {
            storage_notices.push(notice);
            (value, true)
        }
        FileLoad::Protected { detail } => {
            storage_notices.push(detail);
            (json!({}), false)
        }
    };
    let had_appearance = json.get("appearance").is_some();
    let had_language = json.get("language").is_some();
    let legacy_appearance = json
        .get("appearance")
        .and_then(|value| serde_json::from_value::<LegacyAppearance>(value.clone()).ok());
    let legacy_language = json
        .get("language")
        .and_then(|value| serde_json::from_value::<Language>(value.clone()).ok());

    let config_path = paths.config();
    let (mut document, config_missing, mut config_writable) =
        match load_file(&config_path, parse_config) {
            FileLoad::Missing => (ConfigDocument::new(), true, true),
            FileLoad::Loaded(document) => (document, false, true),
            FileLoad::Recovered { value, notice } => {
                storage_notices.push(notice);
                (value, false, true)
            }
            FileLoad::Protected { detail } => {
                storage_notices.push(detail);
                (ConfigDocument::new(), false, false)
            }
        };
    if config_missing && (had_appearance || had_language) {
        document = document_from_legacy(
            legacy_appearance.as_ref().unwrap_or(&LegacyAppearance {
                theme_family: crate::default_theme_family(),
                ..LegacyAppearance::default()
            }),
            legacy_language.unwrap_or_default(),
        );
        if let Err(error) = write_config(paths, &document) {
            config_writable = false;
            storage_notices.push(format!(
                "Could not migrate {}: {error}. Writes to this file are disabled for this run.",
                config_path.display()
            ));
        }
    }
    if had_appearance || had_language {
        match settings_from_json(strip_legacy(json.clone()))
            .and_then(|settings| write_connections(paths, &settings))
        {
            Ok(()) => {}
            Err(error) => {
                connections_writable = false;
                storage_notices.push(format!(
                    "Could not migrate {}: {error}. Writes to this file are disabled for this run.",
                    connections_path.display()
                ));
            }
        }
    }

    let settings = settings_from_json(strip_legacy(json))
        .expect("connections were validated before their migration was applied");
    let (app_config, _warnings) = AppConfig::from_document(&document);
    let ui_state_path = paths.ui_state();
    let (ui_state, mut ui_state_writable) = match load_file(&ui_state_path, parse_ui_state) {
        FileLoad::Missing => (
            UiState {
                selected_connection_id: settings.recent_connection_ids.first().cloned(),
                ..UiState::default()
            },
            true,
        ),
        FileLoad::Loaded(state) => (state, true),
        FileLoad::Recovered { value, notice } => {
            storage_notices.push(notice);
            (value, true)
        }
        FileLoad::Protected { detail } => {
            storage_notices.push(detail);
            (UiState::default(), false)
        }
    };
    if !ui_state_path.exists()
        && ui_state.selected_connection_id.is_some()
        && let Err(error) = write_ui_state(paths, &ui_state)
    {
        ui_state_writable = false;
        storage_notices.push(format!(
            "Could not migrate {}: {error}. Writes to this file are disabled for this run.",
            ui_state_path.display()
        ));
    }
    LoadedApp {
        settings,
        appearance: appearance_from_config(&app_config),
        language: app_config.language,
        document,
        ui_state,
        persistence: ConfigStore::new(
            paths.clone(),
            PersistenceAccess {
                connections: connections_writable,
                config: config_writable,
                ui_state: ui_state_writable,
            },
        ),
        storage_notices,
    }
}

pub fn write_config(paths: &AppPaths, document: &ConfigDocument) -> Result<(), String> {
    if document.is_empty() && !paths.config().exists() {
        return Ok(());
    }
    atomic_write(&paths.config(), document.serialize().as_bytes())
}

pub fn write_connections(paths: &AppPaths, settings: &Settings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    atomic_write(&paths.connections(), &bytes)
}

fn write_ui_state(paths: &AppPaths, state: &UiState) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
    atomic_write(&paths.ui_state(), &bytes)
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
struct LegacyAppearance {
    #[serde(default = "crate::default_theme_family")]
    theme_family: String,
    #[serde(default)]
    mode: crate::AppearanceMode,
    #[serde(default)]
    backdrop: crate::BackdropMode,
    #[serde(default = "legacy_opacity")]
    background_opacity: u8,
    #[serde(default)]
    font: LegacyFont,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct LegacyFont {
    #[serde(default)]
    family: String,
    #[serde(default = "legacy_font_size")]
    size: u8,
    #[serde(default = "legacy_true")]
    ligatures: bool,
    #[serde(default)]
    thicken: bool,
    #[serde(default)]
    cell_width_percent: i8,
    #[serde(default)]
    cell_height_percent: i8,
}

impl Default for LegacyFont {
    fn default() -> Self {
        Self {
            family: String::new(),
            size: legacy_font_size(),
            ligatures: true,
            thicken: false,
            cell_width_percent: 0,
            cell_height_percent: 0,
        }
    }
}

fn legacy_opacity() -> u8 {
    100
}

fn legacy_font_size() -> u8 {
    13
}

fn legacy_true() -> bool {
    true
}

fn document_from_legacy(appearance: &LegacyAppearance, language: Language) -> ConfigDocument {
    let mut document = ConfigDocument::new();
    document.set("theme", &appearance.theme_family);
    document.set("appearance-mode", appearance.mode.as_config());
    document.set("window-backdrop", appearance.backdrop.as_config());
    document.set(
        "background-opacity",
        &super::values::format_opacity_percent(appearance.background_opacity),
    );
    if !appearance.font.family.is_empty() {
        document.set("font-family", &appearance.font.family);
    }
    document.set("font-size", &appearance.font.size.to_string());
    if appearance.font.thicken {
        document.set("font-thicken", "true");
    }
    if !appearance.font.ligatures {
        document.set_repeatable("font-feature", &super::values::no_ligature_features());
    }
    if appearance.font.cell_width_percent != 0 {
        document.set(
            "adjust-cell-width",
            &format!("{}%", appearance.font.cell_width_percent),
        );
    }
    if appearance.font.cell_height_percent != 0 {
        document.set(
            "adjust-cell-height",
            &format!("{}%", appearance.font.cell_height_percent),
        );
    }
    document.set("language", language.as_config());
    document
}

fn parse_connections(bytes: &[u8]) -> Result<Value, String> {
    let value = serde_json::from_slice::<Value>(bytes).map_err(|error| error.to_string())?;
    settings_from_json(strip_legacy(value.clone()))?;
    Ok(value)
}

fn parse_config(bytes: &[u8]) -> Result<ConfigDocument, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    Ok(ConfigDocument::parse(text))
}

fn parse_ui_state(bytes: &[u8]) -> Result<UiState, String> {
    let state = serde_json::from_slice::<UiState>(bytes).map_err(|error| error.to_string())?;
    if state.schema_version > UI_STATE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported schema version {} (maximum supported is {UI_STATE_SCHEMA_VERSION})",
            state.schema_version
        ));
    }
    Ok(state)
}

fn strip_legacy(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.remove("appearance");
        map.remove("language");
    }
    value
}

fn settings_from_json(value: Value) -> Result<Settings, String> {
    let settings = serde_json::from_value::<Settings>(value).map_err(|error| error.to_string())?;
    if settings.schema_version > crate::SETTINGS_SCHEMA_VERSION {
        return Err(format!(
            "unsupported schema version {} (maximum supported is {})",
            settings.schema_version,
            crate::SETTINGS_SCHEMA_VERSION
        ));
    }
    Ok(settings)
}

fn load_file<T>(path: &Path, parse: impl Fn(&[u8]) -> Result<T, String>) -> FileLoad<T> {
    let primary = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FileLoad::Missing,
        Err(error) => {
            return FileLoad::Protected {
                detail: protected_notice(path, &error.to_string()),
            };
        }
    };
    match parse(&primary) {
        Ok(value) => FileLoad::Loaded(value),
        Err(primary_error) => {
            let backup_path = backup_path(path);
            let backup = match fs::read(&backup_path) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return FileLoad::Protected {
                        detail: protected_notice(path, &primary_error),
                    };
                }
            };
            let value = match parse(&backup) {
                Ok(value) => value,
                Err(backup_error) => {
                    return FileLoad::Protected {
                        detail: protected_notice(
                            path,
                            &format!(
                                "{primary_error}; backup {} is also invalid: {backup_error}",
                                backup_path.display()
                            ),
                        ),
                    };
                }
            };
            match restore_backup(path, &primary, &backup) {
                Ok(corrupt_path) => FileLoad::Recovered {
                    value,
                    notice: format!(
                        "Recovered {} from its last-known-good backup. The invalid file was preserved at {}.",
                        path.display(),
                        corrupt_path.display()
                    ),
                },
                Err(error) => FileLoad::Protected {
                    detail: protected_notice(
                        path,
                        &format!(
                            "{primary_error}; a valid backup exists at {}, but recovery failed: {error}",
                            backup_path.display()
                        ),
                    ),
                },
            }
        }
    }
}

fn protected_notice(path: &Path, error: &str) -> String {
    format!(
        "Could not safely load {}: {error}. Writes to this file are disabled for this run so the existing data cannot be overwritten.",
        path.display()
    )
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn initial_snapshot(path: &Path, writable: &mut bool) -> Option<Vec<u8>> {
    if !*writable {
        return None;
    }
    match read_optional(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            *writable = false;
            None
        }
    }
}

fn corrupt_path(path: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".corrupt-{timestamp}"));
    PathBuf::from(name)
}

fn restore_backup(path: &Path, invalid: &[u8], backup: &[u8]) -> Result<PathBuf, String> {
    let corrupt = corrupt_path(path);
    atomic_replace(&corrupt, invalid)?;
    atomic_replace(path, backup)?;
    Ok(corrupt)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Ok(existing) = fs::read(path) {
        atomic_replace(&backup_path(path), &existing)?;
    }
    atomic_replace(path, bytes)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map_err(|error| error.error.to_string())?;
    sync_directory(parent)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppearanceMode, BackdropMode};

    fn sample_appearance() -> LegacyAppearance {
        LegacyAppearance {
            theme_family: "ember".into(),
            mode: AppearanceMode::Light,
            backdrop: BackdropMode::Opaque,
            background_opacity: 84,
            font: LegacyFont {
                family: "Distinct Mono".into(),
                size: 18,
                ligatures: false,
                thicken: true,
                cell_width_percent: 10,
                cell_height_percent: 12,
            },
        }
    }

    #[test]
    fn legacy_connections_json_moves_appearance_and_language_into_config() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        let connections = paths.connections();
        let json = json!({
            "connections": [{
                "kind": "ssh",
                "id": "manual-1",
                "label": "Build",
                "destination": "build.example",
                "herdr_path": "herdr"
            }],
            "recent_connection_ids": ["manual-1"],
            "appearance": {
                "theme_family": "ember",
                "mode": "light",
                "backdrop": "opaque",
                "background_opacity": 84,
                "font": {
                    "family": "Distinct Mono",
                    "size": 18,
                    "ligatures": false,
                    "thicken": true,
                    "cell_width_percent": 10,
                    "cell_height_percent": 12
                }
            },
            "language": "english"
        });
        let original = serde_json::to_string(&json).expect("legacy json");
        assert!(
            original.contains("\"appearance\""),
            "fixture must contain an appearance section or the migration test proves nothing"
        );
        assert!(
            original.contains("\"language\""),
            "fixture must contain a language section or the migration test proves nothing"
        );
        fs::create_dir_all(&paths.dir).expect("create dir");
        fs::write(&connections, serde_json::to_vec_pretty(&json).unwrap()).expect("write json");

        let loaded = load_app(&paths);

        let written_json = fs::read_to_string(&connections).expect("rewritten json");
        assert!(
            !written_json.contains("\"appearance\""),
            "appearance must leave connections.json\n{written_json}"
        );
        assert!(
            !written_json.contains("\"language\""),
            "language must leave connections.json\n{written_json}"
        );
        assert!(written_json.contains("manual-1"));

        let written_config = fs::read_to_string(paths.config()).expect("config file");
        assert!(
            paths.config().is_file(),
            "migration must create the config file"
        );
        assert!(written_config.contains("theme = ember"));
        assert!(written_config.contains("appearance-mode = light"));
        assert!(written_config.contains("window-backdrop = opaque"));
        assert!(written_config.contains("background-opacity = 0.84"));
        assert!(written_config.contains("font-family = \"Distinct Mono\""));
        assert!(written_config.contains("font-size = 18"));
        assert!(written_config.contains("font-thicken = true"));
        assert!(written_config.contains("font-feature = -liga"));
        assert!(written_config.contains("adjust-cell-width = 10%"));
        assert!(written_config.contains("adjust-cell-height = 12%"));
        assert!(written_config.contains("language = en"));
        assert_eq!(loaded.appearance.theme_family, "ember");
        assert_eq!(loaded.appearance.mode, AppearanceMode::Light);
        assert_eq!(loaded.language, Language::English);
        assert_eq!(loaded.settings.connections.len(), 1);
        assert_eq!(loaded.document.get("theme"), Some("ember"));
    }

    #[test]
    fn existing_config_wins_when_json_still_has_legacy_sections() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        fs::create_dir_all(&paths.dir).expect("create dir");
        fs::write(
            paths.connections(),
            r#"{
                "connections": [],
                "appearance": { "theme_family": "ember", "mode": "light" },
                "language": "english"
            }"#,
        )
        .unwrap();
        let existing = "# mine\ntheme = ochub\nlanguage = zh-Hans\n";
        assert!(existing.contains("# mine"));
        fs::write(paths.config(), existing).unwrap();

        let loaded = load_app(&paths);
        let written_config = fs::read_to_string(paths.config()).unwrap();
        assert_eq!(written_config, existing);
        assert_eq!(loaded.language, Language::SimplifiedChinese);
        assert_eq!(loaded.appearance.theme_family, "ochub");
        let written_json = fs::read_to_string(paths.connections()).unwrap();
        assert!(!written_json.contains("\"appearance\""));
    }

    #[test]
    fn missing_files_load_spec_defaults() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        let loaded = load_app(&paths);
        assert!(loaded.document.is_empty());
        assert_eq!(
            loaded.appearance.theme_family,
            crate::default_theme_family()
        );
        assert_eq!(loaded.appearance.mode, AppearanceMode::Dark);
        assert_eq!(loaded.appearance.background_opacity, 1.0);
        assert_eq!(loaded.appearance.font.size, 13.0);
        assert_eq!(loaded.language, Language::System);
        assert!(!paths.config().exists());
        assert!(!paths.connections().exists());
    }

    #[test]
    fn document_from_legacy_writes_the_old_json_values() {
        let document = document_from_legacy(&sample_appearance(), Language::English);
        let text = document.serialize();
        assert!(text.contains("theme = ember"));
        assert!(text.contains("language = en"));
        assert!(text.contains("font-size = 18"));
        assert!(text.contains("font-feature = -dlig"));
    }

    #[test]
    fn invalid_connections_are_protected_instead_of_overwritten() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        fs::create_dir_all(&paths.dir).unwrap();
        let invalid = b"{ this is not json";
        fs::write(paths.connections(), invalid).unwrap();

        let loaded = load_app(&paths);

        assert!(!loaded.persistence.access().connections);
        assert!(loaded.settings.connections.is_empty());
        assert_eq!(fs::read(paths.connections()).unwrap(), invalid);
        assert!(loaded.storage_notices.iter().any(|notice| {
            notice.contains("Writes to this file are disabled")
                && notice.contains("connections.json")
        }));
        let mut document = ConfigDocument::new();
        document.set("appearance-mode", "light");
        loaded.persistence.write_config(&document).unwrap();
        assert_eq!(
            fs::read_to_string(paths.config()).unwrap(),
            "appearance-mode = light\n"
        );
    }

    #[test]
    fn atomic_writes_keep_the_previous_connections_as_a_backup() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        let first = Settings {
            recent_connection_ids: vec!["first".into()],
            ..Settings::default()
        };
        let second = Settings {
            recent_connection_ids: vec!["second".into()],
            ..Settings::default()
        };

        write_connections(&paths, &first).unwrap();
        write_connections(&paths, &second).unwrap();

        let current: Settings =
            serde_json::from_slice(&fs::read(paths.connections()).unwrap()).unwrap();
        let backup: Settings =
            serde_json::from_slice(&fs::read(backup_path(&paths.connections())).unwrap()).unwrap();
        assert_eq!(current.recent_connection_ids, ["second"]);
        assert_eq!(backup.recent_connection_ids, ["first"]);
    }

    #[test]
    fn a_valid_backup_recovers_an_invalid_primary_without_losing_evidence() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        let good = Settings {
            recent_connection_ids: vec!["safe".into()],
            ..Settings::default()
        };
        write_connections(&paths, &good).unwrap();
        let invalid = b"truncated";
        fs::write(paths.connections(), invalid).unwrap();
        fs::write(
            backup_path(&paths.connections()),
            serde_json::to_vec_pretty(&good).unwrap(),
        )
        .unwrap();

        let loaded = load_app(&paths);

        assert!(loaded.persistence.access().connections);
        assert_eq!(loaded.settings.recent_connection_ids, ["safe"]);
        assert!(loaded.storage_notices.iter().any(|notice| {
            notice.contains("last-known-good backup") && notice.contains("corrupt-")
        }));
        let restored: Settings =
            serde_json::from_slice(&fs::read(paths.connections()).unwrap()).unwrap();
        assert_eq!(restored.recent_connection_ids, ["safe"]);
        let preserved = fs::read_dir(&paths.dir)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("connections.json.corrupt-")
            })
            .expect("invalid primary is preserved");
        assert_eq!(fs::read(preserved.path()).unwrap(), invalid);
    }

    #[test]
    fn ui_state_round_trips_the_selected_connection() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        write_ui_state(
            &paths,
            &UiState {
                selected_connection_id: Some("manual-1".into()),
                ..UiState::default()
            },
        )
        .unwrap();

        let loaded = load_app(&paths);

        assert!(loaded.persistence.access().ui_state);
        assert_eq!(
            loaded.ui_state.selected_connection_id.as_deref(),
            Some("manual-1")
        );
    }

    #[test]
    fn missing_ui_state_migrates_the_most_recent_connection() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        write_connections(
            &paths,
            &Settings {
                recent_connection_ids: vec!["manual-1".into(), "local".into()],
                ..Settings::default()
            },
        )
        .unwrap();

        let loaded = load_app(&paths);

        assert_eq!(
            loaded.ui_state.selected_connection_id.as_deref(),
            Some("manual-1")
        );
        assert!(paths.ui_state().is_file());
    }

    #[test]
    fn a_stale_process_cannot_overwrite_a_newer_connections_revision() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = AppPaths {
            dir: dir.path().to_path_buf(),
        };
        write_connections(&paths, &Settings::default()).unwrap();
        let first = load_app(&paths);
        let stale = load_app(&paths);
        let newer = Settings {
            recent_connection_ids: vec!["newer".into()],
            ..Settings::default()
        };
        let older = Settings {
            recent_connection_ids: vec!["stale".into()],
            ..Settings::default()
        };

        first.persistence.write_connections(&newer).unwrap();
        let error = stale.persistence.write_connections(&older).unwrap_err();

        assert!(error.contains("changed in another OcHerdr process"));
        assert!(!stale.persistence.access().connections);
        let current: Settings =
            serde_json::from_slice(&fs::read(paths.connections()).unwrap()).unwrap();
        assert_eq!(current.recent_connection_ids, ["newer"]);
    }
}
