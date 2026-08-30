//! Typed OcHerdr config keys (spec 3.1 and 3.2).

use std::path::PathBuf;

use crate::i18n::Language;
use crate::{AppearanceMode, AppearanceSettings, BackdropMode, TerminalFontSettings};

use super::document::{ConfigDocument, ParseWarning};

pub fn is_known_key(key: &str) -> bool {
    matches!(
        key,
        "font-family"
            | "font-size"
            | "font-thicken"
            | "font-thicken-strength"
            | "font-feature"
            | "adjust-cell-width"
            | "adjust-cell-height"
            | "background"
            | "foreground"
            | "cursor-color"
            | "cursor-text"
            | "selection-background"
            | "selection-foreground"
            | "palette"
            | "background-opacity"
            | "background-blur"
            | "window-padding-x"
            | "window-padding-y"
            | "theme"
            | "terminal-theme"
            | "appearance-mode"
            | "window-backdrop"
            | "language"
            | "pane-edge-relocation"
            | "file-panel-open"
            | "file-panel-width"
            | "file-panel-show-hidden"
            | "file-editor"
    )
}

/// Experimental switches are read like any other key but survive
/// `strip_known_keys` (restore-appearance-defaults must not silently turn
/// them off).
pub fn is_experimental_key(key: &str) -> bool {
    matches!(
        key,
        "pane-edge-relocation"
            | "file-panel-open"
            | "file-panel-width"
            | "file-panel-show-hidden"
            | "file-editor"
    )
}

/// Ghostty metric delta: `1`, `-2`, or `20%`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MetricModifier {
    Absolute(i32),
    Percent(f64),
}

impl MetricModifier {
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        if let Some(number) = input.strip_suffix('%') {
            let percent: f64 = number.trim().parse().ok()?;
            return Some(Self::Percent(percent));
        }
        Some(Self::Absolute(input.parse().ok()?))
    }

    pub fn to_config(self) -> String {
        match self {
            Self::Absolute(value) => value.to_string(),
            Self::Percent(value) => {
                if value == value.trunc() {
                    format!("{}%", value as i64)
                } else {
                    format!("{value}%")
                }
            }
        }
    }
}

/// Terminal color `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        let hex = input.strip_prefix('#').unwrap_or(input);
        match hex.len() {
            3 => {
                let value = u32::from_str_radix(hex, 16).ok()?;
                let r = (value >> 8) & 0xf;
                let g = (value >> 4) & 0xf;
                let b = value & 0xf;
                Some(Self(
                    (r << 20) | (r << 16) | (g << 12) | (g << 8) | (b << 4) | b,
                ))
            }
            6 => Some(Self(u32::from_str_radix(hex, 16).ok()?)),
            8 => Some(Self(u32::from_str_radix(hex, 16).ok()? >> 8)),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        format!("#{:06x}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeRef {
    Name(String),
    Pair { light: String, dark: String },
}

impl ThemeRef {
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        if let Some(pair) = parse_theme_pair(input) {
            return Some(pair);
        }
        Some(Self::Name(input.to_owned()))
    }

    pub fn to_config(&self) -> String {
        match self {
            Self::Name(name) => name.clone(),
            Self::Pair { light, dark } => format!("light:{light},dark:{dark}"),
        }
    }

    pub fn display_id(&self) -> String {
        match self {
            Self::Name(name) => name.clone(),
            Self::Pair { light, dark } => format!("light:{light},dark:{dark}"),
        }
    }
}

fn parse_theme_pair(input: &str) -> Option<ThemeRef> {
    let mut light = None;
    let mut dark = None;
    for part in input.split(',') {
        let part = part.trim();
        if let Some(name) = part.strip_prefix("light:") {
            light = Some(name.trim().to_owned());
        } else if let Some(name) = part.strip_prefix("dark:") {
            dark = Some(name.trim().to_owned());
        }
    }
    Some(ThemeRef::Pair {
        light: light.filter(|name| !name.is_empty())?,
        dark: dark.filter(|name| !name.is_empty())?,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppConfig {
    pub font_family: Vec<String>,
    pub font_size: f32,
    pub font_thicken: bool,
    pub font_thicken_strength: u8,
    pub font_feature: Vec<String>,
    pub adjust_cell_width: Option<MetricModifier>,
    pub adjust_cell_height: Option<MetricModifier>,
    pub background: Option<Color>,
    pub foreground: Option<Color>,
    pub cursor_color: Option<Color>,
    pub cursor_text: Option<Color>,
    pub selection_background: Option<Color>,
    pub selection_foreground: Option<Color>,
    pub palette: [Option<Color>; 16],
    pub extra_palette: Vec<(u16, Color)>,
    pub background_opacity: f64,
    pub background_blur: u8,
    pub window_padding_x: u32,
    pub window_padding_y: u32,
    pub theme: ThemeRef,
    pub terminal_theme: Option<ThemeRef>,
    pub appearance_mode: AppearanceMode,
    pub window_backdrop: BackdropMode,
    pub language: Language,
    /// Design §13 step 3: four-edge pane relocation via the two-step
    /// `pane.move` orchestration. Off until it graduates (step 4).
    pub pane_edge_relocation: bool,
    pub file_panel_open: bool,
    pub file_panel_width: f32,
    pub file_panel_show_hidden: bool,
    pub file_editor: Option<PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            font_family: Vec::new(),
            font_size: 13.0,
            font_thicken: false,
            font_thicken_strength: 255,
            font_feature: Vec::new(),
            adjust_cell_width: None,
            adjust_cell_height: None,
            background: None,
            foreground: None,
            cursor_color: None,
            cursor_text: None,
            selection_background: None,
            selection_foreground: None,
            palette: [None; 16],
            extra_palette: Vec::new(),
            background_opacity: 1.0,
            background_blur: 0,
            window_padding_x: 0,
            window_padding_y: 0,
            theme: ThemeRef::Name(crate::default_theme_family()),
            terminal_theme: None,
            appearance_mode: AppearanceMode::Dark,
            window_backdrop: BackdropMode::Blurred,
            language: Language::System,
            pane_edge_relocation: false,
            file_panel_open: false,
            file_panel_width: crate::FILE_PANEL_DEFAULT_WIDTH,
            file_panel_show_hidden: false,
            file_editor: None,
        }
    }
}

impl AppConfig {
    pub fn from_document(document: &ConfigDocument) -> (Self, Vec<ParseWarning>) {
        let mut config = Self::default();
        let mut warnings = Vec::new();
        for (line, key, value) in document.assignments() {
            apply_assignment(&mut config, &mut warnings, line, key, value);
        }
        (config, warnings)
    }
}

fn apply_assignment(
    config: &mut AppConfig,
    warnings: &mut Vec<ParseWarning>,
    line: usize,
    key: &str,
    value: &str,
) {
    if !is_known_key(key) {
        warnings.push(ParseWarning {
            line,
            key: key.to_owned(),
            message: format!("unknown key `{key}`"),
        });
        return;
    }
    if value.is_empty() {
        reset_key(config, key);
        return;
    }
    match key {
        "font-family" => config.font_family.push(value.to_owned()),
        "font-size" => match value.parse::<f32>() {
            Ok(size) => config.font_size = size,
            Err(_) => invalid(warnings, line, key, value),
        },
        "font-thicken" => match parse_bool(value) {
            Some(flag) => config.font_thicken = flag,
            None => invalid(warnings, line, key, value),
        },
        "pane-edge-relocation" => match parse_bool(value) {
            Some(flag) => config.pane_edge_relocation = flag,
            None => invalid(warnings, line, key, value),
        },
        "file-panel-open" => match parse_bool(value) {
            Some(flag) => config.file_panel_open = flag,
            None => invalid(warnings, line, key, value),
        },
        "file-panel-width" => match value.parse::<f32>() {
            Ok(width)
                if (crate::FILE_PANEL_MIN_WIDTH..=crate::FILE_PANEL_MAX_WIDTH).contains(&width) =>
            {
                config.file_panel_width = width
            }
            _ => invalid(warnings, line, key, value),
        },
        "file-panel-show-hidden" => match parse_bool(value) {
            Some(flag) => config.file_panel_show_hidden = flag,
            None => invalid(warnings, line, key, value),
        },
        "file-editor" => config.file_editor = Some(PathBuf::from(value)),
        "font-thicken-strength" => match value.parse::<u8>() {
            Ok(strength) => config.font_thicken_strength = strength,
            Err(_) => invalid(warnings, line, key, value),
        },
        "font-feature" => config.font_feature.push(value.to_owned()),
        "adjust-cell-width" => match MetricModifier::parse(value) {
            Some(metric) => config.adjust_cell_width = Some(metric),
            None => invalid(warnings, line, key, value),
        },
        "adjust-cell-height" => match MetricModifier::parse(value) {
            Some(metric) => config.adjust_cell_height = Some(metric),
            None => invalid(warnings, line, key, value),
        },
        "background" => assign_color(&mut config.background, warnings, line, key, value),
        "foreground" => assign_color(&mut config.foreground, warnings, line, key, value),
        "cursor-color" => assign_color(&mut config.cursor_color, warnings, line, key, value),
        "cursor-text" => assign_color(&mut config.cursor_text, warnings, line, key, value),
        "selection-background" => {
            assign_color(&mut config.selection_background, warnings, line, key, value)
        }
        "selection-foreground" => {
            assign_color(&mut config.selection_foreground, warnings, line, key, value)
        }
        "palette" => apply_palette(config, warnings, line, value),
        "background-opacity" => match parse_opacity(value) {
            Some(opacity) => config.background_opacity = opacity,
            None => invalid(warnings, line, key, value),
        },
        "background-blur" => match parse_blur(value) {
            Some(blur) => config.background_blur = blur,
            None => invalid(warnings, line, key, value),
        },
        "window-padding-x" => match parse_padding(value) {
            Some(padding) => config.window_padding_x = padding,
            None => invalid(warnings, line, key, value),
        },
        "window-padding-y" => match parse_padding(value) {
            Some(padding) => config.window_padding_y = padding,
            None => invalid(warnings, line, key, value),
        },
        "theme" => match ThemeRef::parse(value) {
            Some(theme) => config.theme = theme,
            None => invalid(warnings, line, key, value),
        },
        "terminal-theme" => match ThemeRef::parse(value) {
            Some(theme) => config.terminal_theme = Some(theme),
            None => invalid(warnings, line, key, value),
        },
        "appearance-mode" => match AppearanceMode::from_config(value) {
            Some(mode) => config.appearance_mode = mode,
            None => invalid(warnings, line, key, value),
        },
        "window-backdrop" => match BackdropMode::from_config(value) {
            Some(backdrop) => config.window_backdrop = backdrop,
            None => invalid(warnings, line, key, value),
        },
        "language" => match Language::from_config(value) {
            Some(language) => config.language = language,
            None => invalid(warnings, line, key, value),
        },
        _ => {}
    }
}

fn reset_key(config: &mut AppConfig, key: &str) {
    let default = AppConfig::default();
    match key {
        "font-family" => config.font_family.clear(),
        "font-size" => config.font_size = default.font_size,
        "font-thicken" => config.font_thicken = default.font_thicken,
        "font-thicken-strength" => config.font_thicken_strength = default.font_thicken_strength,
        "font-feature" => config.font_feature.clear(),
        "adjust-cell-width" => config.adjust_cell_width = None,
        "adjust-cell-height" => config.adjust_cell_height = None,
        "background" => config.background = None,
        "foreground" => config.foreground = None,
        "cursor-color" => config.cursor_color = None,
        "cursor-text" => config.cursor_text = None,
        "selection-background" => config.selection_background = None,
        "selection-foreground" => config.selection_foreground = None,
        "palette" => {
            config.palette = [None; 16];
            config.extra_palette.clear();
        }
        "background-opacity" => config.background_opacity = default.background_opacity,
        "background-blur" => config.background_blur = default.background_blur,
        "window-padding-x" => config.window_padding_x = default.window_padding_x,
        "window-padding-y" => config.window_padding_y = default.window_padding_y,
        "theme" => config.theme = default.theme,
        "terminal-theme" => config.terminal_theme = None,
        "appearance-mode" => config.appearance_mode = default.appearance_mode,
        "window-backdrop" => config.window_backdrop = default.window_backdrop,
        "language" => config.language = default.language,
        "pane-edge-relocation" => config.pane_edge_relocation = default.pane_edge_relocation,
        "file-panel-open" => config.file_panel_open = default.file_panel_open,
        "file-panel-width" => config.file_panel_width = default.file_panel_width,
        "file-panel-show-hidden" => config.file_panel_show_hidden = default.file_panel_show_hidden,
        "file-editor" => config.file_editor = None,
        _ => {}
    }
}

fn assign_color(
    slot: &mut Option<Color>,
    warnings: &mut Vec<ParseWarning>,
    line: usize,
    key: &str,
    value: &str,
) {
    match Color::parse(value) {
        Some(color) => *slot = Some(color),
        None => invalid(warnings, line, key, value),
    }
}

fn apply_palette(
    config: &mut AppConfig,
    warnings: &mut Vec<ParseWarning>,
    line: usize,
    value: &str,
) {
    let Some((index_text, color_text)) = value.split_once('=') else {
        invalid(warnings, line, "palette", value);
        return;
    };
    let Ok(index) = index_text.trim().parse::<u16>() else {
        invalid(warnings, line, "palette", value);
        return;
    };
    let Some(color) = Color::parse(color_text) else {
        invalid(warnings, line, "palette", value);
        return;
    };
    if let Some(slot) = config.palette.get_mut(index as usize) {
        *slot = Some(color);
        return;
    }
    config.extra_palette.push((index, color));
    warnings.push(ParseWarning {
        line,
        key: "palette".to_owned(),
        message: format!("palette index {index} is outside 0-15 ({})", color.to_hex()),
    });
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_opacity(value: &str) -> Option<f64> {
    let opacity: f64 = value.trim().parse().ok()?;
    (0.0..=1.0).contains(&opacity).then_some(opacity)
}

fn parse_blur(value: &str) -> Option<u8> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(20),
        "false" => Some(0),
        other => other.parse().ok(),
    }
}

fn parse_padding(value: &str) -> Option<u32> {
    value.split(',').next()?.trim().parse().ok()
}

fn invalid(warnings: &mut Vec<ParseWarning>, line: usize, key: &str, value: &str) {
    warnings.push(ParseWarning {
        line,
        key: key.to_owned(),
        message: format!("invalid `{key}` value `{value}`"),
    });
}

pub fn appearance_from_config(config: &AppConfig) -> AppearanceSettings {
    AppearanceSettings {
        theme_family: config.theme.display_id(),
        terminal_theme: config.terminal_theme.as_ref().map(ThemeRef::to_config),
        mode: config.appearance_mode,
        backdrop: config.window_backdrop,
        background_opacity: config.background_opacity,
        window_padding_x: config.window_padding_x,
        window_padding_y: config.window_padding_y,
        palette: config.palette.map(|slot| slot.map(|color| color.0)),
        font: TerminalFontSettings {
            family: config.font_family.first().cloned().unwrap_or_default(),
            size: config.font_size,
            features: config.font_feature.clone(),
            thicken: config.font_thicken,
            thicken_strength: config.font_thicken_strength,
            cell_width: config.adjust_cell_width,
            cell_height: config.adjust_cell_height,
        },
    }
}

pub fn strip_known_keys(document: &mut super::document::ConfigDocument) {
    let keys: Vec<String> = document
        .assignments()
        .filter(|(_, key, _)| is_known_key(key) && !is_experimental_key(key))
        .map(|(_, key, _)| key.to_owned())
        .collect();
    for key in keys {
        document.remove(&key);
    }
}

pub fn format_font_size(size: f32) -> String {
    if size == size.trunc() {
        format!("{}", size as i32)
    } else {
        format!("{size}")
    }
}

pub fn format_opacity(opacity: f64) -> String {
    if opacity == 1.0 {
        "1".to_owned()
    } else {
        format!("{opacity:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

pub fn format_opacity_percent(percent: u8) -> String {
    format_opacity(f64::from(percent) / 100.0)
}

pub fn opacity_percent_u8(opacity: f64) -> u8 {
    (opacity * 100.0).round().clamp(0.0, 100.0) as u8
}

pub const NO_LIGATURES: [&str; 3] = ["-calt", "-liga", "-dlig"];

pub fn no_ligature_features() -> Vec<String> {
    NO_LIGATURES.map(str::to_owned).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parses_hash_hex_and_writes_lowercase() {
        assert_eq!(Color::parse("#C0FFEE"), Some(Color(0xc0ffee)));
        assert_eq!(Color::parse("112233"), Some(Color(0x112233)));
        assert_eq!(Color::parse("#0f0"), Some(Color(0x00ff00)));
        assert_eq!(Color(0xc0ffee).to_hex(), "#c0ffee");
        assert!(Color::parse("not-a-color").is_none());
    }

    #[test]
    fn metric_modifier_parses_ghostty_absolute_and_percent_forms() {
        assert_eq!(
            MetricModifier::parse("1"),
            Some(MetricModifier::Absolute(1))
        );
        assert_eq!(
            MetricModifier::parse("-2"),
            Some(MetricModifier::Absolute(-2))
        );
        assert_eq!(
            MetricModifier::parse("20%"),
            Some(MetricModifier::Percent(20.0))
        );
        assert_eq!(MetricModifier::Absolute(-2).to_config(), "-2");
        assert_eq!(MetricModifier::Percent(20.0).to_config(), "20%");
        assert!(MetricModifier::parse("").is_none());
        assert!(MetricModifier::parse("wide").is_none());
    }

    #[test]
    fn known_keys_parse_into_app_config_and_unknown_keys_warn() {
        let source = "\
theme = ochub
font-size = 14.5
font-thicken = true
font-thicken-strength = 80
font-family = \"Maple Mono\"
font-family = Menlo
font-feature = -liga
adjust-cell-width = 20%
adjust-cell-height = -2
background-opacity = 0.84
background-blur = 12
window-padding-x = 4
window-padding-y = 8
appearance-mode = light
window-backdrop = opaque
language = zh-Hans
palette = 3=#ff00aa
background = #112233
keybind = ignore-me
";
        let document = ConfigDocument::parse(source);
        let (config, warnings) = AppConfig::from_document(&document);
        assert_eq!(config.font_size, 14.5);
        assert!(config.font_thicken);
        assert_eq!(config.font_thicken_strength, 80);
        assert_eq!(config.font_family, ["Maple Mono", "Menlo"]);
        assert_eq!(config.font_feature, ["-liga"]);
        assert_eq!(
            config.adjust_cell_width,
            Some(MetricModifier::Percent(20.0))
        );
        assert_eq!(
            config.adjust_cell_height,
            Some(MetricModifier::Absolute(-2))
        );
        assert_eq!(config.background_opacity, 0.84);
        assert_eq!(config.background_blur, 12);
        assert_eq!(config.window_padding_x, 4);
        assert_eq!(config.window_padding_y, 8);
        assert_eq!(config.appearance_mode, AppearanceMode::Light);
        assert_eq!(config.window_backdrop, BackdropMode::Opaque);
        assert_eq!(config.language, Language::SimplifiedChinese);
        assert_eq!(config.palette[3], Some(Color(0xff00aa)));
        assert_eq!(config.background, Some(Color(0x112233)));
        assert_eq!(config.theme, ThemeRef::Name("ochub".into()));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].key, "keybind");
        assert!(warnings[0].message.contains("unknown"));
    }

    #[test]
    fn invalid_values_on_known_keys_warn_and_keep_defaults() {
        let document = ConfigDocument::parse("font-size = huge\nbackground-opacity = 4\n");
        let (config, warnings) = AppConfig::from_document(&document);
        assert_eq!(config.font_size, 13.0);
        assert_eq!(config.background_opacity, 1.0);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn appearance_settings_keep_free_ghostty_values() {
        let probe = "#c0ffee";
        let source = "\
font-size = 13.5
background-opacity = 0.85
window-padding-x = 2
window-padding-y = 7
font-thicken-strength = 80
font-feature = ss01
adjust-cell-height = 1
terminal-theme = imported-probe
palette = 3=#c0ffee
";
        assert!(
            source.contains("13.5")
                && source.contains("0.85")
                && source.contains("ss01")
                && source.contains(probe)
                && source.contains("adjust-cell-height = 1"),
            "fixture must carry the free values this test claims to preserve"
        );
        let document = ConfigDocument::parse(source);
        let (config, _) = AppConfig::from_document(&document);
        let appearance = appearance_from_config(&config);
        assert_eq!(appearance.font.size, 13.5);
        assert_eq!(appearance.background_opacity, 0.85);
        assert_eq!(appearance.window_padding_x, 2);
        assert_eq!(appearance.window_padding_y, 7);
        assert_eq!(appearance.font.thicken_strength, 80);
        assert_eq!(appearance.font.features, ["ss01"]);
        assert_eq!(
            appearance.font.cell_height,
            Some(MetricModifier::Absolute(1))
        );
        assert_eq!(appearance.terminal_theme.as_deref(), Some("imported-probe"));
        assert_eq!(appearance.palette[3], Some(0xc0ffee));
        assert_eq!(appearance.mode, AppearanceMode::Dark);
    }

    #[test]
    fn strip_known_keys_leaves_comments_and_unknown_assignments() {
        let source = "\
# keep this
font-size = 18
mystery-option = wow
language = en
";
        assert!(source.contains("# keep this"));
        assert!(source.contains("mystery-option = wow"));
        let mut document = ConfigDocument::parse(source);
        strip_known_keys(&mut document);
        let written = document.serialize();
        assert!(written.contains("# keep this"));
        assert!(written.contains("mystery-option = wow"));
        assert!(!written.contains("font-size"));
        assert!(!written.contains("language"));
    }

    #[test]
    fn pane_edge_relocation_defaults_off_parses_bools_and_survives_a_strip() {
        let (config, warnings) = AppConfig::from_document(&ConfigDocument::parse(""));
        assert!(!config.pane_edge_relocation);
        assert!(warnings.is_empty());
        let (config, warnings) =
            AppConfig::from_document(&ConfigDocument::parse("pane-edge-relocation = true\n"));
        assert!(config.pane_edge_relocation);
        assert!(warnings.is_empty());
        let (config, warnings) =
            AppConfig::from_document(&ConfigDocument::parse("pane-edge-relocation = maybe\n"));
        assert!(!config.pane_edge_relocation);
        assert_eq!(warnings.len(), 1);
        let mut document = ConfigDocument::parse("pane-edge-relocation = true\nfont-size = 12\n");
        strip_known_keys(&mut document);
        let written = document.serialize();
        assert!(written.contains("pane-edge-relocation = true"));
        assert!(!written.contains("font-size"));
    }

    #[test]
    fn file_panel_settings_parse_validate_and_survive_appearance_reset() {
        let source = "file-panel-open = true\nfile-panel-width = 420\nfile-panel-show-hidden = true\nfile-editor = \"/Applications/Visual Studio Code.app\"\n";
        let (config, warnings) = AppConfig::from_document(&ConfigDocument::parse(source));
        assert!(warnings.is_empty());
        assert!(config.file_panel_open);
        assert_eq!(config.file_panel_width, 420.);
        assert!(config.file_panel_show_hidden);
        assert_eq!(
            config.file_editor,
            Some(PathBuf::from("/Applications/Visual Studio Code.app"))
        );

        let (invalid, warnings) =
            AppConfig::from_document(&ConfigDocument::parse("file-panel-width = 900\n"));
        assert_eq!(invalid.file_panel_width, crate::FILE_PANEL_DEFAULT_WIDTH);
        assert_eq!(warnings.len(), 1);

        let mut document = ConfigDocument::parse(&format!("{source}font-size = 12\n"));
        strip_known_keys(&mut document);
        let written = document.serialize();
        assert!(written.contains("file-panel-open = true"));
        assert!(written.contains("file-panel-width = 420"));
        assert!(written.contains("file-panel-show-hidden = true"));
        assert!(written.contains("file-editor = \"/Applications/Visual Studio Code.app\""));
        assert!(!written.contains("font-size"));
    }

    #[test]
    fn theme_ref_parses_light_dark_pairs() {
        let theme = ThemeRef::parse(" light:Rose Pine Dawn, dark:Rose Pine ").unwrap();
        assert_eq!(
            theme,
            ThemeRef::Pair {
                light: "Rose Pine Dawn".into(),
                dark: "Rose Pine".into(),
            }
        );
        assert_eq!(theme.to_config(), "light:Rose Pine Dawn,dark:Rose Pine");
        assert_eq!(
            ThemeRef::parse("ochub"),
            Some(ThemeRef::Name("ochub".into()))
        );
    }
}
