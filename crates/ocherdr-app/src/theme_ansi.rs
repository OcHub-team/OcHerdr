//! Terminal 16-color overlays stored beside ochub-ui theme tokens.
//!
//! ochub-ui's [`ThemeFamily`] has no ANSI field and does not deny unknown
//! keys, so the same `.ochub-theme.json` can carry `light.ansi` / `dark.ansi`
//! for OcHerdr while the shared library keeps reading only UI tokens.
//! Serializing through ochub-ui alone drops those keys; use
//! [`serialize_theme_file`] or [`inject_theme_ansi`] when writing.

use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow};
use ochub_ui::theme::{self, ThemeColor, ThemeFamily};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// ANSI palettes nested the same way as [`ThemeFamily`]: `light` / `dark`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeAnsi {
    #[serde(default)]
    pub light: ThemeAnsiPalette,
    #[serde(default)]
    pub dark: ThemeAnsiPalette,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeAnsiPalette {
    #[serde(default)]
    pub ansi: Option<[ThemeColor; 16]>,
}

impl ThemeAnsi {
    pub fn colors(self, dark: bool) -> Option<[u32; 16]> {
        let palette = if dark {
            self.dark.ansi
        } else {
            self.light.ansi
        };
        palette.map(|colors| colors.map(|color| color.0))
    }

    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        Self::from_json(&fs::read_to_string(path)?)
    }
}

/// Built-in OcHub / Ember palettes, or the extra keys on a user theme file.
pub fn overlay_for(family: Option<&ThemeFamily>) -> ThemeAnsi {
    match family.map(|family| family.id.as_str()) {
        None | Some(theme::DEFAULT_THEME_FAMILY) => ochub_overlay(),
        Some(theme::EMBER_THEME_FAMILY) => ember_overlay(),
        Some(id) => user_overlay(id),
    }
}

fn user_overlay(id: &str) -> ThemeAnsi {
    theme::load_registry()
        .themes
        .iter()
        .find(|record| record.family.id == id)
        .and_then(|record| record.path.as_deref())
        .and_then(|path| ThemeAnsi::from_path(path).ok())
        .unwrap_or_default()
}

/// Serialize a family the way ochub-ui would, then put ANSI back on `light` /
/// `dark`. Call this instead of `serde_json::to_string` when the file should
/// keep a terminal palette.
pub fn serialize_theme_file(family: &ThemeFamily, ansi: &ThemeAnsi) -> Result<String> {
    let mut document = serde_json::to_value(family)?;
    inject_theme_ansi(&mut document, ansi)?;
    Ok(serde_json::to_string_pretty(&document)?)
}

pub fn inject_theme_ansi(document: &mut Value, ansi: &ThemeAnsi) -> Result<()> {
    merge_variant(document, "light", ansi.light.ansi)?;
    merge_variant(document, "dark", ansi.dark.ansi)?;
    Ok(())
}

/// Overlay colors when present, otherwise the token-derived 16-color table.
pub fn resolved_ansi(overlay: ThemeAnsi, theme: &theme::Theme, dark: bool) -> [u32; 16] {
    overlay
        .colors(dark)
        .unwrap_or_else(|| ansi_from_theme(theme, dark))
}

pub fn apply_overrides(mut colors: [u32; 16], overrides: &[Option<u32>; 16]) -> [u32; 16] {
    for (index, color) in overrides.iter().enumerate() {
        if let Some(color) = color {
            colors[index] = *color;
        }
    }
    colors
}

pub fn ansi_from_theme(theme: &theme::Theme, dark: bool) -> [u32; 16] {
    let (black, bright_black) = split_gray_pair(theme.bg.0, theme.text.0, dark);
    let (red, bright_red) = split_pair(theme.red.0, dark);
    let (green, bright_green) = split_pair(theme.green.0, dark);
    let (yellow, bright_yellow) = split_pair(theme.yellow.0, dark);
    let (blue, bright_blue) = split_pair(theme.accent.0, dark);
    let (magenta, bright_magenta) = split_pair(theme.mauve.0, dark);
    let (cyan, bright_cyan) = split_pair(theme.teal.0, dark);
    let (white, bright_white) = split_pair(theme.subtext.0, dark);
    [
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        bright_black,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        bright_white,
    ]
}

fn split_gray_pair(bg: u32, text: u32, dark: bool) -> (u32, u32) {
    let toward_text = if dark { 22 } else { 28 };
    split_pair(mix_rgb(bg, text, toward_text), dark)
}

fn split_pair(color: u32, dark: bool) -> (u32, u32) {
    let dim = mix_rgb(color, 0x000000, if dark { 28 } else { 16 });
    let bright = mix_rgb(color, 0xFFFFFF, if dark { 16 } else { 28 });
    if ansi_luma(bright) > ansi_luma(dim) && dim != bright {
        return (dim, bright);
    }
    let bright = mix_rgb(dim, 0xFFFFFF, 42);
    if ansi_luma(bright) > ansi_luma(dim) && dim != bright {
        return (dim, bright);
    }
    (mix_rgb(bright, 0x000000, 36), bright)
}

fn mix_rgb(from: u32, to: u32, percent: u32) -> u32 {
    let percent = percent.min(100) as i32;
    let mix = |from: u32, to: u32| {
        let from = from as i32;
        let to = to as i32;
        (from + (to - from) * percent / 100).clamp(0, 255) as u32
    };
    (mix((from >> 16) & 0xff, (to >> 16) & 0xff) << 16)
        | (mix((from >> 8) & 0xff, (to >> 8) & 0xff) << 8)
        | mix(from & 0xff, to & 0xff)
}

pub(crate) fn ansi_luma(color: u32) -> u32 {
    299 * ((color >> 16) & 0xff) + 587 * ((color >> 8) & 0xff) + 114 * (color & 0xff)
}

fn merge_variant(
    document: &mut Value,
    variant: &'static str,
    ansi: Option<[ThemeColor; 16]>,
) -> Result<()> {
    let Some(colors) = ansi else {
        return Ok(());
    };
    let Some(object) = document.get_mut(variant).and_then(Value::as_object_mut) else {
        return Err(anyhow!("theme JSON is missing the {variant} object"));
    };
    object.insert("ansi".to_string(), serde_json::to_value(colors)?);
    Ok(())
}

fn rgb_palette(values: [u32; 16]) -> [ThemeColor; 16] {
    values.map(ThemeColor::new)
}

/// UI red/green are muted for chrome; `ls`, diffs, and agent CLIs need
/// distinct bright slots.
const OCHUB_DARK_ANSI: [u32; 16] = [
    0x2C2D28, 0xB54C48, 0x2F7A4C, 0xB08A32, 0x355EA8, 0x6A56A8, 0x1F7F78, 0xB8B7AE, 0x6E6F64,
    0xFF8A82, 0x6FD496, 0xF0C46A, 0x7DB0FF, 0xC4A8FF, 0x5ED4C8, 0xF4F3EA,
];
const OCHUB_LIGHT_ANSI: [u32; 16] = [
    0x3A3A34, 0x9A322C, 0x1A5C34, 0x7A5008, 0x1A4AB0, 0x4C3C90, 0x085C54, 0x8A8982, 0x6B6A64,
    0xD4564C, 0x2D9A58, 0xC48A18, 0x4A82EE, 0x8B70D8, 0x1AA89A, 0xD0CFC6,
];
const EMBER_DARK_ANSI: [u32; 16] = [
    0x2A1E16, 0xB44A38, 0x4A7A32, 0xB07A24, 0x3A5A98, 0x8A5A9A, 0x2A7A6A, 0xC8B8A4, 0x7A5A42,
    0xF07058, 0x88C058, 0xE8B040, 0x6A8AD8, 0xC890E0, 0x58C8B0, 0xF7EEE5,
];
const EMBER_LIGHT_ANSI: [u32; 16] = [
    0x3A2A1C, 0x9A3028, 0x2A5C28, 0x7A5008, 0x2A4890, 0x5C3878, 0x0A5C50, 0x8A7A68, 0x6B5A48,
    0xD45640, 0x3A9A48, 0xC48A18, 0x4A72D0, 0x8B68C0, 0x1A9888, 0xE8D8C8,
];

pub fn ochub_overlay() -> ThemeAnsi {
    ThemeAnsi {
        light: ThemeAnsiPalette {
            ansi: Some(rgb_palette(OCHUB_LIGHT_ANSI)),
        },
        dark: ThemeAnsiPalette {
            ansi: Some(rgb_palette(OCHUB_DARK_ANSI)),
        },
    }
}

pub fn ember_overlay() -> ThemeAnsi {
    ThemeAnsi {
        light: ThemeAnsiPalette {
            ansi: Some(rgb_palette(EMBER_LIGHT_ANSI)),
        },
        dark: ThemeAnsiPalette {
            ansi: Some(rgb_palette(EMBER_DARK_ANSI)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_family(id: &str) -> ThemeFamily {
        let mut family = theme::ochub_family();
        family.id = id.into();
        family.name = id.into();
        family.author.clear();
        family.description.clear();
        family
    }

    #[test]
    fn ochub_ui_serialization_omits_ansi() {
        let json = serde_json::to_string(&theme::ochub_family()).expect("serialize family");
        assert!(
            !json.contains("\"ansi\""),
            "ochub-ui must not write ansi; inject_theme_ansi exists because of that"
        );
    }

    #[test]
    fn theme_files_without_ansi_yield_none() {
        let family = user_family("legacy-user");
        let json = serde_json::to_string_pretty(&family).expect("serialize family");
        assert!(
            !json.contains("\"ansi\""),
            "fixture JSON must not mention ansi"
        );

        let overlay = ThemeAnsi::from_json(&json).expect("legacy theme without ansi must parse");
        assert_eq!(overlay.light.ansi, None);
        assert_eq!(overlay.dark.ansi, None);
        assert_eq!(overlay.colors(true), None);
        assert_eq!(overlay.colors(false), None);
    }

    #[test]
    fn serialize_theme_file_injects_ansi_that_ochub_ui_ignores() {
        let family = user_family("imported-ghostty");
        let ansi = ochub_overlay();
        let json = serialize_theme_file(&family, &ansi).expect("serialize with ansi");
        let value: Value = serde_json::from_str(&json).expect("parse injected json");
        assert!(
            value["light"]["ansi"].is_array(),
            "fixture must actually contain light.ansi"
        );
        assert!(
            value["dark"]["ansi"].is_array(),
            "fixture must actually contain dark.ansi"
        );
        assert_eq!(value["light"]["ansi"].as_array().map(Vec::len), Some(16));
        assert_eq!(value["dark"]["ansi"].as_array().map(Vec::len), Some(16));
        assert_eq!(value["schemaVersion"], family.schema_version);

        let decoded: ThemeFamily =
            serde_json::from_str(&json).expect("ochub-ui must ignore the extra ansi keys");
        assert_eq!(decoded.id, family.id);
        assert_eq!(decoded.schema_version, family.schema_version);
        assert_eq!(decoded.dark.bg, family.dark.bg);

        let overlay = ThemeAnsi::from_json(&json).expect("OcHerdr reader keeps ansi");
        assert_eq!(overlay, ansi);
    }

    #[test]
    fn serialize_without_ansi_omits_the_key() {
        let family = user_family("derived-only");
        let json = serialize_theme_file(&family, &ThemeAnsi::default()).expect("serialize");
        assert!(
            !json.contains("\"ansi\""),
            "None ansi must not be written as a key"
        );
    }

    #[test]
    fn from_path_reads_the_same_file_as_ochub_ui() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("with-ansi.ochub-theme.json");
        let family = user_family("with-ansi");
        let ansi = ember_overlay();
        let json = serialize_theme_file(&family, &ansi).expect("serialize");
        std::fs::write(&path, &json).expect("write fixture");

        let decoded: ThemeFamily =
            serde_json::from_str(&json).expect("ochub-ui reader accepts the file");
        assert_eq!(decoded.id, "with-ansi");

        let overlay = ThemeAnsi::from_path(&path).expect("OcHerdr reader loads ansi");
        assert_eq!(overlay, ansi);
        assert_eq!(overlay.colors(true), Some(EMBER_DARK_ANSI));
        assert_eq!(overlay.colors(false), Some(EMBER_LIGHT_ANSI));
    }

    #[test]
    fn overlay_for_uses_built_in_tables_for_ochub_and_ember() {
        assert_eq!(
            overlay_for(Some(&theme::ochub_family())).colors(true),
            Some(OCHUB_DARK_ANSI)
        );
        assert_eq!(
            overlay_for(Some(&theme::ember_family())).colors(false),
            Some(EMBER_LIGHT_ANSI)
        );
        assert_eq!(overlay_for(None).colors(true), Some(OCHUB_DARK_ANSI));
        assert_ne!(OCHUB_DARK_ANSI, EMBER_DARK_ANSI);
    }

    #[test]
    fn overlay_for_a_custom_family_without_a_file_is_empty() {
        let family = user_family("scarlet");
        assert!(
            overlay_for(Some(&family)).colors(true).is_none(),
            "no user file means no ansi overlay"
        );
    }

    #[test]
    fn overlay_for_reads_user_file_through_the_registry() {
        let directory = tempfile::tempdir().expect("temp dir");
        theme::set_themes_dir(directory.path());
        let family = user_family("wired-ansi");
        let ansi = ember_overlay();
        let json = serialize_theme_file(&family, &ansi).expect("serialize");
        let value: Value = serde_json::from_str(&json).expect("parse fixture");
        assert!(
            value["dark"]["ansi"].is_array(),
            "fixture must actually contain ansi"
        );
        std::fs::write(directory.path().join("wired-ansi.ochub-theme.json"), json)
            .expect("write fixture");
        theme::reload_registry();
        let loaded = theme::find_family("wired-ansi").expect("registry must load the file");
        assert_eq!(overlay_for(Some(&loaded)), ansi);
    }
}
