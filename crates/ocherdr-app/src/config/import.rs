//! Build a Ghostty import plan. Does not write files or touch the UI.

use std::fs;
use std::path::{Path, PathBuf};

use ochub_ui::theme::{ThemeColor, ThemeFamily, ochub_family};

use crate::theme_ansi::{ThemeAnsi, ThemeAnsiPalette, serialize_theme_file};

use super::document::{ConfigDocument, ParseWarning};
use super::values::{AppConfig, Color, ThemeRef, is_known_key};

fn is_repeatable_key(key: &str) -> bool {
    matches!(key, "font-family" | "font-feature" | "palette")
}

const GHOSTTY_APP_THEMES: &str = "/Applications/Ghostty.app/Contents/Resources/ghostty/themes";

#[derive(Clone, Debug)]
pub struct GhosttyImportPaths {
    pub xdg_config: PathBuf,
    pub app_support_config: PathBuf,
    pub app_themes: PathBuf,
}

impl GhosttyImportPaths {
    pub fn user() -> Option<Self> {
        let home = dirs::home_dir()?;
        Some(Self {
            xdg_config: home.join(".config/ghostty/config"),
            app_support_config: dirs::config_dir()?.join("com.mitchellh.ghostty/config"),
            app_themes: PathBuf::from(GHOSTTY_APP_THEMES),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhosttyImportError {
    ConfigNotFound { searched: Vec<PathBuf> },
    ThemesMissing { path: PathBuf },
    ThemeMissing { name: String, path: PathBuf },
    Io { path: PathBuf, error: String },
    ThemeFile { error: String },
}

impl std::fmt::Display for GhosttyImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfigNotFound { searched } => {
                let places = searched
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "Ghostty config not found (looked in {places})")
            }
            Self::ThemesMissing { path } => write!(
                f,
                "Ghostty.app was not found (expected themes at {}). Install Ghostty to import its themes.",
                path.display()
            ),
            Self::ThemeMissing { name, path } => {
                write!(
                    f,
                    "Ghostty theme `{name}` was not found at {}",
                    path.display()
                )
            }
            Self::Io { path, error } => {
                write!(f, "could not read {}: {error}", path.display())
            }
            Self::ThemeFile { error } => {
                write!(f, "could not build the imported theme file: {error}")
            }
        }
    }
}

impl std::error::Error for GhosttyImportError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigUpdate {
    pub key: String,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownKey {
    pub key: String,
    pub value: String,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhosttyThemePalette {
    pub background: Option<Color>,
    pub foreground: Option<Color>,
    pub cursor_color: Option<Color>,
    pub cursor_text: Option<Color>,
    pub selection_background: Option<Color>,
    pub selection_foreground: Option<Color>,
    pub palette: [Option<Color>; 16],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedGhosttyTheme {
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub light: GhosttyThemePalette,
    pub dark: GhosttyThemePalette,
    /// JSON that T29-C would write to `themes/`. Built with T29-B's
    /// `serialize_theme_file` so `light.ansi` / `dark.ansi` survive.
    pub file_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhosttyImportPlan {
    pub source: PathBuf,
    pub updates: Vec<ConfigUpdate>,
    pub unknown_keys: Vec<UnknownKey>,
    pub themes: Vec<ImportedGhosttyTheme>,
    pub terminal_theme: Option<ThemeRef>,
    pub warnings: Vec<ParseWarning>,
}

pub fn plan_ghostty_import(
    paths: &GhosttyImportPaths,
) -> Result<GhosttyImportPlan, GhosttyImportError> {
    let source = find_ghostty_config(paths)?;
    let text = fs::read_to_string(&source).map_err(|error| GhosttyImportError::Io {
        path: source.clone(),
        error: error.to_string(),
    })?;
    plan_ghostty_import_from(&text, &source, &paths.app_themes)
}

pub fn plan_ghostty_import_from(
    text: &str,
    source: &Path,
    themes_dir: &Path,
) -> Result<GhosttyImportPlan, GhosttyImportError> {
    complete_import_plan(plan_ghostty_keys(text, source), themes_dir)
}

/// Keys and unknown assignments only. Theme files are not read, so a missing
/// Ghostty.app still yields a plan the settings page can confirm.
pub fn plan_ghostty_keys(text: &str, source: &Path) -> GhosttyKeyPlan {
    let document = ConfigDocument::parse(text);
    let (config, parse_warnings) = AppConfig::from_document(&document);
    let mut unknown_keys = Vec::new();
    let mut updates: Vec<ConfigUpdate> = Vec::new();
    let mut ghostty_theme = None;
    let mut warnings = parse_warnings;
    for (index, color) in &config.extra_palette {
        unknown_keys.push(UnknownKey {
            key: "palette".to_owned(),
            value: format!("{index}={}", color.to_hex()),
            line: 0,
        });
    }

    for (line, key, value) in document.assignments() {
        if key == "theme" {
            ghostty_theme = ThemeRef::parse(value);
            continue;
        }
        if !is_known_key(key) {
            unknown_keys.push(UnknownKey {
                key: key.to_owned(),
                value: value.to_owned(),
                line,
            });
            continue;
        }
        if value.is_empty() {
            continue;
        }
        push_update(&mut updates, key, value, is_repeatable_key(key));
    }

    warnings.retain(|warning| !warning.message.starts_with("unknown key"));

    GhosttyKeyPlan {
        source: source.to_path_buf(),
        updates,
        unknown_keys,
        warnings,
        ghostty_theme,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhosttyKeyPlan {
    pub source: PathBuf,
    pub updates: Vec<ConfigUpdate>,
    pub unknown_keys: Vec<UnknownKey>,
    pub warnings: Vec<ParseWarning>,
    pub ghostty_theme: Option<ThemeRef>,
}

impl GhosttyKeyPlan {
    pub fn without_themes(self) -> GhosttyImportPlan {
        GhosttyImportPlan {
            source: self.source,
            updates: self.updates,
            unknown_keys: self.unknown_keys,
            themes: Vec::new(),
            terminal_theme: None,
            warnings: self.warnings,
        }
    }
}

fn complete_import_plan(
    keys: GhosttyKeyPlan,
    themes_dir: &Path,
) -> Result<GhosttyImportPlan, GhosttyImportError> {
    let GhosttyKeyPlan {
        source,
        updates,
        unknown_keys,
        warnings,
        ghostty_theme,
    } = keys;
    let mut plan = GhosttyImportPlan {
        source,
        updates,
        unknown_keys,
        themes: Vec::new(),
        terminal_theme: None,
        warnings,
    };
    let Some(theme) = ghostty_theme else {
        return Ok(plan);
    };
    let imported = import_theme(theme, themes_dir)?;
    plan.terminal_theme = Some(ThemeRef::Name(imported.id.clone()));
    plan.themes.push(imported);
    Ok(plan)
}

pub fn apply_ghostty_import_plan(
    document: &mut ConfigDocument,
    plan: &GhosttyImportPlan,
    themes_dir: &Path,
) -> Result<(), GhosttyImportError> {
    if !plan.themes.is_empty() {
        fs::create_dir_all(themes_dir).map_err(|error| GhosttyImportError::Io {
            path: themes_dir.to_path_buf(),
            error: error.to_string(),
        })?;
    }
    for theme in &plan.themes {
        if theme.file_name.contains("..")
            || theme.file_name.contains('/')
            || theme.file_name.contains('\\')
        {
            return Err(GhosttyImportError::ThemeFile {
                error: format!("refusing to write theme file `{}`", theme.file_name),
            });
        }
        let path = themes_dir.join(&theme.file_name);
        fs::write(&path, &theme.file_json).map_err(|error| GhosttyImportError::Io {
            path,
            error: error.to_string(),
        })?;
    }
    for update in &plan.updates {
        if is_repeatable_key(&update.key) {
            document.set_repeatable(&update.key, &update.values);
            continue;
        }
        let Some(value) = update.values.last() else {
            continue;
        };
        document.set(&update.key, value);
    }
    if let Some(theme) = &plan.terminal_theme {
        document.set("terminal-theme", &theme.to_config());
    }
    Ok(())
}

fn find_ghostty_config(paths: &GhosttyImportPaths) -> Result<PathBuf, GhosttyImportError> {
    if paths.xdg_config.is_file() {
        return Ok(paths.xdg_config.clone());
    }
    if paths.app_support_config.is_file() {
        return Ok(paths.app_support_config.clone());
    }
    Err(GhosttyImportError::ConfigNotFound {
        searched: vec![paths.xdg_config.clone(), paths.app_support_config.clone()],
    })
}

fn push_update(updates: &mut Vec<ConfigUpdate>, key: &str, value: &str, repeatable: bool) {
    if repeatable {
        if let Some(existing) = updates.iter_mut().find(|update| update.key == key) {
            existing.values.push(value.to_owned());
            return;
        }
    } else if let Some(existing) = updates.iter_mut().find(|update| update.key == key) {
        existing.values = vec![value.to_owned()];
        return;
    }
    updates.push(ConfigUpdate {
        key: key.to_owned(),
        values: vec![value.to_owned()],
    });
}

fn import_theme(
    theme: ThemeRef,
    themes_dir: &Path,
) -> Result<ImportedGhosttyTheme, GhosttyImportError> {
    if !themes_dir.is_dir() {
        return Err(GhosttyImportError::ThemesMissing {
            path: themes_dir.to_path_buf(),
        });
    }
    match theme {
        ThemeRef::Name(name) => {
            let palette = read_theme_palette(themes_dir, &name)?;
            let slug = theme_slug(&name);
            let id = format!("imported-{slug}");
            imported_theme(id, name, palette.clone(), palette)
        }
        ThemeRef::Pair { light, dark } => {
            let light_palette = read_theme_palette(themes_dir, &light)?;
            let dark_palette = read_theme_palette(themes_dir, &dark)?;
            let slug = format!("{}-{}", theme_slug(&light), theme_slug(&dark));
            imported_theme(
                format!("imported-{slug}"),
                format!("{light} / {dark}"),
                light_palette,
                dark_palette,
            )
        }
    }
}

fn imported_theme(
    id: String,
    name: String,
    light: GhosttyThemePalette,
    dark: GhosttyThemePalette,
) -> Result<ImportedGhosttyTheme, GhosttyImportError> {
    let family = imported_family(&id, &name);
    let ansi = ThemeAnsi {
        light: ThemeAnsiPalette {
            ansi: ansi_from_palette(&light),
        },
        dark: ThemeAnsiPalette {
            ansi: ansi_from_palette(&dark),
        },
    };
    let file_json =
        serialize_theme_file(&family, &ansi).map_err(|error| GhosttyImportError::ThemeFile {
            error: error.to_string(),
        })?;
    Ok(ImportedGhosttyTheme {
        file_name: format!("{id}.ochub-theme.json"),
        id,
        name,
        light,
        dark,
        file_json,
    })
}

fn imported_family(id: &str, name: &str) -> ThemeFamily {
    let mut family = ochub_family();
    family.id = id.to_owned();
    family.name = name.to_owned();
    family.author = "Ghostty".to_owned();
    family.description = format!("Imported from Ghostty ({name})");
    family
}

fn ansi_from_palette(palette: &GhosttyThemePalette) -> Option<[ThemeColor; 16]> {
    let mut colors = [ThemeColor::new(0); 16];
    for (index, slot) in palette.palette.iter().enumerate() {
        colors[index] = ThemeColor::new(slot.as_ref()?.0);
    }
    Some(colors)
}

fn read_theme_palette(
    themes_dir: &Path,
    name: &str,
) -> Result<GhosttyThemePalette, GhosttyImportError> {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(GhosttyImportError::ThemeMissing {
            name: name.to_owned(),
            path: themes_dir.join(name),
        });
    }
    let path = themes_dir.join(name);
    let text = fs::read_to_string(&path).map_err(|error| {
        if !path.exists() {
            GhosttyImportError::ThemeMissing {
                name: name.to_owned(),
                path,
            }
        } else {
            GhosttyImportError::Io {
                path,
                error: error.to_string(),
            }
        }
    })?;
    let document = ConfigDocument::parse(&text);
    let (config, _) = AppConfig::from_document(&document);
    Ok(GhosttyThemePalette {
        background: config.background,
        foreground: config.foreground,
        cursor_color: config.cursor_color,
        cursor_text: config.cursor_text,
        selection_background: config.selection_background,
        selection_foreground: config.selection_foreground,
        palette: config.palette,
    })
}

fn theme_slug(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() {
        "theme".to_owned()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_theme(dir: &Path, name: &str, background: &str) {
        let body = format!(
            "\
palette = 0=#111111
palette = 1=#ff0000
palette = 2=#00ff00
palette = 3=#ffff00
palette = 4=#0000ff
palette = 5=#ff00ff
palette = 6=#00ffff
palette = 7=#dddddd
palette = 8=#444444
palette = 9=#ff8888
palette = 10=#88ff88
palette = 11=#ffff88
palette = 12=#8888ff
palette = 13=#ff88ff
palette = 14=#88ffff
palette = 15=#ffffff
background = {background}
foreground = #eeeeee
cursor-color = #ffcc00
cursor-text = #000000
selection-background = #333333
selection-foreground = #ffffff
"
        );
        fs::write(dir.join(name), body).expect("write theme");
    }

    #[test]
    fn import_plan_maps_known_keys_lists_unknown_keys_and_loads_theme_colors() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let themes = dir.path().join("themes");
        fs::create_dir(&themes).unwrap();
        let probe = "#c0ffee";
        write_theme(&themes, "Probe", probe);
        let written = fs::read_to_string(themes.join("Probe")).unwrap();
        assert!(
            written.contains(probe),
            "fixture must contain the unique color or the importer test cannot prove it read the file"
        );

        let config = "\
# ghostty
font-family = \"JetBrains Mono\"
font-size = 15
font-thicken = true
adjust-cell-width = 1
background-opacity = 0.9
window-padding-x = 2
theme = Probe
keybind = ctrl+c=copy
shell-integration = zsh
";
        let source = dir.path().join("config");
        fs::write(&source, config).unwrap();
        let plan = plan_ghostty_import_from(config, &source, &themes).expect("plan");

        assert!(
            plan.updates
                .iter()
                .any(|update| update.key == "font-family" && update.values == ["JetBrains Mono"])
        );
        assert!(
            plan.updates
                .iter()
                .any(|update| update.key == "font-size" && update.values == ["15"])
        );
        assert!(
            plan.updates
                .iter()
                .any(|update| update.key == "adjust-cell-width" && update.values == ["1"])
        );
        assert!(
            !plan.updates.iter().any(|update| update.key == "theme"),
            "Ghostty theme becomes terminal-theme via an imported family, not our UI theme"
        );
        assert_eq!(plan.unknown_keys.len(), 2);
        assert!(plan.unknown_keys.iter().any(|key| key.key == "keybind"));
        assert!(
            plan.unknown_keys
                .iter()
                .any(|key| key.key == "shell-integration")
        );
        assert_eq!(
            plan.terminal_theme,
            Some(ThemeRef::Name("imported-probe".into()))
        );
        assert_eq!(plan.themes.len(), 1);
        assert_eq!(plan.themes[0].id, "imported-probe");
        assert_eq!(plan.themes[0].file_name, "imported-probe.ochub-theme.json");
        assert_eq!(plan.themes[0].dark.background, Color::parse(probe));
        assert_eq!(plan.themes[0].light.background, Color::parse(probe));
        assert_eq!(plan.themes[0].dark.palette[1], Color::parse("#ff0000"));
    }

    #[test]
    fn imported_theme_json_injects_ansi_that_ochub_ui_serialization_drops() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let themes = dir.path().join("themes");
        fs::create_dir(&themes).unwrap();
        let probe = "#c0ffee";
        let mut body = String::new();
        for index in 0..16 {
            let color = if index == 0 { probe } else { "#111111" };
            body.push_str(&format!("palette = {index}={color}\n"));
        }
        body.push_str("background = #191724\nforeground = #eeeeee\n");
        fs::write(themes.join("Probe"), &body).unwrap();
        assert!(
            body.contains(probe),
            "fixture must contain the unique color or this test cannot prove the write path kept it"
        );

        let plan = plan_ghostty_import_from("theme = Probe\n", &dir.path().join("config"), &themes)
            .expect("plan");
        let json = &plan.themes[0].file_json;
        let value: serde_json::Value = serde_json::from_str(json).expect("imported json");
        assert!(
            value["dark"]["ansi"].is_array(),
            "T29-B helper must leave dark.ansi on the document\n{json}"
        );
        assert!(
            value["light"]["ansi"].is_array(),
            "T29-B helper must leave light.ansi on the document\n{json}"
        );
        assert!(
            json.to_ascii_lowercase().contains("c0ffee"),
            "injected ansi must keep the Ghostty color\n{json}"
        );

        let family: ochub_ui::theme::ThemeFamily =
            serde_json::from_str(json).expect("ochub-ui ignores extra ansi keys");
        let without_helper = serde_json::to_string(&family).expect("ochub-ui serialize");
        assert!(
            !without_helper.to_ascii_lowercase().contains("c0ffee"),
            "serializing through ochub-ui alone must drop ansi; that is why the importer calls serialize_theme_file\n{without_helper}"
        );
        assert_eq!(family.schema_version, ochub_family().schema_version);
        assert_eq!(family.id, "imported-probe");
    }

    #[test]
    fn import_plan_loads_light_and_dark_theme_pair() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let themes = dir.path().join("themes");
        fs::create_dir(&themes).unwrap();
        write_theme(&themes, "Rose Pine Dawn", "#faf4ed");
        write_theme(&themes, "Rose Pine", "#191724");
        let plan = plan_ghostty_import_from(
            "theme = light:Rose Pine Dawn,dark:Rose Pine\nfont-size = 13\n",
            &dir.path().join("config"),
            &themes,
        )
        .expect("plan");
        assert_eq!(plan.themes.len(), 1);
        assert_eq!(plan.themes[0].id, "imported-rose-pine-dawn-rose-pine");
        assert_eq!(plan.themes[0].light.background, Color::parse("#faf4ed"));
        assert_eq!(plan.themes[0].dark.background, Color::parse("#191724"));
        assert_eq!(
            plan.terminal_theme,
            Some(ThemeRef::Name("imported-rose-pine-dawn-rose-pine".into()))
        );
    }

    #[test]
    fn missing_ghostty_app_is_a_clear_error_not_a_panic() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let missing = dir.path().join("no-such-app/themes");
        let error =
            plan_ghostty_import_from("theme = Probe\n", &dir.path().join("config"), &missing)
                .expect_err("themes dir missing");
        match error {
            GhosttyImportError::ThemesMissing { ref path } => assert_eq!(path, &missing),
            other => panic!("expected ThemesMissing, got {other}"),
        }
        assert!(
            error.to_string().contains("Ghostty.app was not found"),
            "{error}"
        );
    }

    #[test]
    fn missing_ghostty_config_lists_the_paths_that_were_tried() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let paths = GhosttyImportPaths {
            xdg_config: dir.path().join("xdg/config"),
            app_support_config: dir.path().join("app/config"),
            app_themes: dir.path().join("themes"),
        };
        let error = plan_ghostty_import(&paths).expect_err("no config");
        match error {
            GhosttyImportError::ConfigNotFound { searched } => {
                assert_eq!(searched[0], paths.xdg_config);
                assert_eq!(searched[1], paths.app_support_config);
            }
            other => panic!("expected ConfigNotFound, got {other}"),
        }
    }

    #[test]
    fn user_import_paths_point_at_ghostty_locations() {
        let Some(paths) = GhosttyImportPaths::user() else {
            return;
        };
        assert!(
            paths
                .xdg_config
                .ends_with(std::path::Path::new(".config/ghostty/config"))
        );
        assert!(
            paths
                .app_support_config
                .ends_with("com.mitchellh.ghostty/config")
        );
        assert_eq!(paths.app_themes, PathBuf::from(GHOSTTY_APP_THEMES));
    }

    #[test]
    fn applying_an_import_plan_writes_theme_json_and_only_the_mapped_keys() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let themes = dir.path().join("themes");
        fs::create_dir(&themes).unwrap();
        let probe = "#c0ffee";
        write_theme(&themes, "Probe", probe);
        let config = "\
font-size = 15
theme = Probe
keybind = ctrl+c=copy
";
        assert!(
            fs::read_to_string(themes.join("Probe"))
                .unwrap()
                .contains(probe),
            "fixture must contain the unique color or apply cannot prove it copied the theme"
        );
        let plan =
            plan_ghostty_import_from(config, &dir.path().join("config"), &themes).expect("plan");
        assert_eq!(
            plan.themes[0].dark.background,
            Color::parse(probe),
            "the plan must actually carry the unique Ghostty background"
        );
        let dest = dir.path().join("imported-themes");
        let mut document = ConfigDocument::parse("# keep\nmystery-option = wow\nfont-size = 13\n");
        apply_ghostty_import_plan(&mut document, &plan, &dest).expect("apply");
        let written = document.serialize();
        assert!(written.contains("# keep"));
        assert!(written.contains("mystery-option = wow"));
        assert!(written.contains("font-size = 15"));
        assert!(written.contains("terminal-theme = imported-probe"));
        assert!(
            !written.contains("keybind"),
            "unknown keys must not be copied into OcHerdr config"
        );
        assert!(
            !written.contains("theme = Probe"),
            "Ghostty theme is imported as a family, not as our UI theme"
        );
        let json = fs::read_to_string(dest.join("imported-probe.ochub-theme.json"))
            .expect("imported theme file");
        assert_eq!(
            json, plan.themes[0].file_json,
            "apply must write the plan JSON byte-for-byte, not reserialize through ochub-ui"
        );
    }

    #[test]
    fn xdg_config_is_preferred_over_application_support() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let xdg = dir.path().join("xdg");
        let app = dir.path().join("app");
        fs::create_dir_all(&xdg).unwrap();
        fs::create_dir_all(&app).unwrap();
        fs::write(xdg.join("config"), "font-size = 11\n").unwrap();
        fs::write(app.join("config"), "font-size = 22\n").unwrap();
        let themes = dir.path().join("themes");
        fs::create_dir(&themes).unwrap();
        let paths = GhosttyImportPaths {
            xdg_config: xdg.join("config"),
            app_support_config: app.join("config"),
            app_themes: themes,
        };
        let plan = plan_ghostty_import(&paths).expect("plan");
        assert_eq!(plan.source, paths.xdg_config);
        assert!(
            plan.updates
                .iter()
                .any(|update| update.key == "font-size" && update.values == ["11"])
        );
    }
}
