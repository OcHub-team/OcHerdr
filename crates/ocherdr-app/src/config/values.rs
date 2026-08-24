//! Typed OcHerdr config keys (spec 3.1 and 3.2).

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

    pub fn as_percent_i8(self) -> i8 {
        match self {
            Self::Percent(value) => value.round() as i8,
            Self::Absolute(_) => 0,
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
    use crate::{CellHeightChoice, CellWidthChoice, FontSizeChoice, OpacityChoice};

    let font_size = config.font_size.round().clamp(1.0, 255.0) as u8;
    let opacity_percent = (config.background_opacity * 100.0)
        .round()
        .clamp(0.0, 255.0) as u8;
    let ligatures = !config
        .font_feature
        .iter()
        .any(|feature| matches!(feature.trim(), "-liga" | "-calt" | "-dlig"));
    AppearanceSettings {
        theme_family: config.theme.display_id(),
        mode: config.appearance_mode,
        backdrop: config.window_backdrop,
        background_opacity: OpacityChoice::nearest(opacity_percent),
        font: TerminalFontSettings {
            family: config.font_family.first().cloned().unwrap_or_default(),
            size: FontSizeChoice::nearest(font_size),
            ligatures,
            thicken: config.font_thicken,
            cell_width_percent: CellWidthChoice::nearest(
                config
                    .adjust_cell_width
                    .map(MetricModifier::as_percent_i8)
                    .unwrap_or(0),
            ),
            cell_height_percent: CellHeightChoice::nearest(
                config
                    .adjust_cell_height
                    .map(MetricModifier::as_percent_i8)
                    .unwrap_or(0),
            ),
        },
    }
}

pub fn format_opacity_percent(percent: u8) -> String {
    if percent == 100 {
        "1".to_owned()
    } else {
        format!("{:.2}", f64::from(percent) / 100.0)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellHeightChoice, CellWidthChoice, FontSizeChoice, OpacityChoice};

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
    fn appearance_settings_map_from_app_config() {
        let config = AppConfig {
            font_size: 16.0,
            background_opacity: 0.72,
            font_family: vec!["Menlo".into()],
            font_feature: vec!["-liga".into()],
            adjust_cell_width: Some(MetricModifier::Percent(-10.0)),
            adjust_cell_height: Some(MetricModifier::Percent(12.0)),
            appearance_mode: AppearanceMode::Light,
            ..AppConfig::default()
        };
        let appearance = appearance_from_config(&config);
        assert_eq!(appearance.font.size, FontSizeChoice::Pt16);
        assert_eq!(appearance.background_opacity, OpacityChoice::P72);
        assert_eq!(appearance.font.family, "Menlo");
        assert!(!appearance.font.ligatures);
        assert_eq!(appearance.font.cell_width_percent, CellWidthChoice::Tight);
        assert_eq!(
            appearance.font.cell_height_percent,
            CellHeightChoice::Relaxed
        );
        assert_eq!(appearance.mode, AppearanceMode::Light);
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
