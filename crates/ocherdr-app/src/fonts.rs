//! Installed monospaced families for the appearance font picker.

use std::collections::BTreeSet;
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
use core_foundation::array::CFArray;
#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
#[cfg(target_os = "macos")]
use std::ffi::c_void;

const MONO_NEEDLES: &[&str] = &[
    "andale",
    "anonymous",
    "berkeley",
    "cascadia",
    "code",
    "commit",
    "console",
    "courier",
    "cozette",
    "dejavu sans mono",
    "droid sans mono",
    "envy",
    "fira",
    "fixedsys",
    "fragment mono",
    "geist",
    "gohufont",
    "hack",
    "ibm plex mono",
    "inconsolata",
    "input",
    "intel one mono",
    "iosevka",
    "jetbrains",
    "liberation mono",
    "lilex",
    "maple",
    "md io",
    "menlo",
    "mona",
    "monaco",
    "mono",
    "nerd",
    "nimbus mono",
    "noto sans mono",
    "ocr",
    "operator",
    "plex mono",
    "pragmata",
    "pt mono",
    "recursive",
    "red hat mono",
    "sarasa",
    "sf mono",
    "source code",
    "spleen",
    "terminus",
    "ubuntu mono",
    "victor",
];

#[cfg(target_os = "macos")]
#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontManagerCopyAvailableFontFamilyNames() -> *const c_void;
}

/// Installed families that look like programming/monospace faces, sorted.
pub fn monospace_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(discover)
}

pub fn looks_like_mono(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    MONO_NEEDLES.iter().any(|needle| name.contains(needle))
}

fn discover() -> Vec<String> {
    let mut families = BTreeSet::new();
    for name in installed_family_names() {
        if looks_like_mono(&name) {
            families.insert(name);
        }
    }
    families.into_iter().collect()
}

#[cfg(target_os = "macos")]
fn installed_family_names() -> Vec<String> {
    unsafe {
        let array_ref = CTFontManagerCopyAvailableFontFamilyNames();
        if array_ref.is_null() {
            return Vec::new();
        }
        let array = CFArray::<CFString>::wrap_under_create_rule(array_ref.cast());
        let mut names = Vec::with_capacity(array.len() as usize);
        for index in 0..array.len() {
            if let Some(name) = array.get(index) {
                let name = (*name).to_string();
                if !name.is_empty() && !name.starts_with('.') {
                    names.push(name);
                }
            }
        }
        names
    }
}

#[cfg(target_os = "windows")]
fn installed_family_names() -> Vec<String> {
    [
        "Cascadia Code",
        "Cascadia Mono",
        "Consolas",
        "Courier New",
        "JetBrains Mono",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn installed_family_names() -> Vec<String> {
    [
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono",
        "Ubuntu Mono",
        "JetBrains Mono",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_programming_fonts() {
        assert!(looks_like_mono("Menlo"));
        assert!(looks_like_mono("SF Mono"));
        assert!(looks_like_mono("JetBrains Mono"));
        assert!(looks_like_mono("Fira Code"));
        assert!(!looks_like_mono("New York"));
        assert!(!looks_like_mono("PingFang SC"));
    }
}
