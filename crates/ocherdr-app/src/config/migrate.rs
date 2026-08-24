//! First-launch move of `appearance` / `language` out of connections.json.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

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
}

#[derive(Clone)]
pub struct LoadedApp {
    pub settings: Settings,
    pub appearance: AppearanceSettings,
    pub language: Language,
    pub document: ConfigDocument,
}

pub fn load_app(paths: &AppPaths) -> LoadedApp {
    let connections_path = paths.connections();
    let json = read_json(&connections_path);
    let had_appearance = json.get("appearance").is_some();
    let had_language = json.get("language").is_some();
    let legacy_appearance = json
        .get("appearance")
        .and_then(|value| serde_json::from_value::<LegacyAppearance>(value.clone()).ok());
    let legacy_language = json
        .get("language")
        .and_then(|value| serde_json::from_value::<Language>(value.clone()).ok());

    let mut document = read_config(&paths.config());
    let config_missing = !paths.config().is_file();
    if config_missing && (had_appearance || had_language) {
        document = document_from_legacy(
            legacy_appearance.as_ref().unwrap_or(&LegacyAppearance {
                theme_family: crate::default_theme_family(),
                ..LegacyAppearance::default()
            }),
            legacy_language.unwrap_or_default(),
        );
        let _ = write_config(paths, &document);
    }
    if had_appearance || had_language {
        let stripped = strip_legacy(json.clone());
        let _ = write_json(&connections_path, &stripped);
    }

    let settings = settings_from_json(strip_legacy(json));
    let (app_config, _warnings) = AppConfig::from_document(&document);
    LoadedApp {
        settings,
        appearance: appearance_from_config(&app_config),
        language: app_config.language,
        document,
    }
}

pub fn write_config(paths: &AppPaths, document: &ConfigDocument) -> Result<(), String> {
    if document.is_empty() && !paths.config().exists() {
        return Ok(());
    }
    fs::create_dir_all(&paths.dir).map_err(|error| error.to_string())?;
    fs::write(paths.config(), document.serialize()).map_err(|error| error.to_string())
}

pub fn write_connections(paths: &AppPaths, settings: &Settings) -> Result<(), String> {
    fs::create_dir_all(&paths.dir).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(paths.connections(), bytes).map_err(|error| error.to_string())
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

fn read_json(path: &Path) -> Value {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!({}))
}

fn read_config(path: &Path) -> ConfigDocument {
    fs::read_to_string(path)
        .map(|text| ConfigDocument::parse(&text))
        .unwrap_or_default()
}

fn strip_legacy(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.remove("appearance");
        map.remove("language");
    }
    value
}

fn settings_from_json(value: Value) -> Settings {
    serde_json::from_value(value).unwrap_or_default()
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
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
}
