use super::*;

#[test]
fn maps_key_names_to_macos_virtual_keycodes() {
    assert_eq!(macos_keycode("a"), 0x00);
    assert_eq!(macos_keycode("C"), 0x08);
    assert_eq!(macos_keycode("pageup"), 0x74);
    assert_eq!(macos_keycode("return"), 0x24);
    assert_eq!(macos_keycode("enter"), 0x24);
    assert_eq!(macos_keycode("up"), 0x7e);
    assert_eq!(macos_keycode("unknown"), KEYCODE_UNIDENTIFIED);
    assert_eq!(macos_keycode("ü"), KEYCODE_UNIDENTIFIED);
}

#[test]
fn maps_modifier_bits() {
    let modifiers = ghostty_modifiers(KeyModifiers {
        control: true,
        alt: true,
        shift: false,
        platform: false,
    });
    assert_ne!(modifiers & ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL, 0);
    assert_ne!(modifiers & ffi::ghostty_input_mods_e_GHOSTTY_MODS_ALT, 0);
}

#[test]
fn mouse_capture_sequences_reset_modes_before_enabling_requested_encoding() {
    let disabled = mouse_capture_sequence(false, true);
    assert_eq!(disabled, MOUSE_REPORTING_RESET);

    let enabled = mouse_capture_sequence(true, false);
    assert!(enabled.starts_with(MOUSE_REPORTING_RESET));
    assert!(enabled.ends_with(b"\x1b[?1006h"));
    assert!(!enabled.ends_with(b"\x1b[?1016h"));

    let pixels = mouse_capture_sequence(true, true);
    assert!(pixels.starts_with(MOUSE_REPORTING_RESET));
    assert!(pixels.ends_with(b"\x1b[?1016h"));
}

#[test]
fn palette_config_uses_light_background_and_ansi_slots() {
    let palette = TerminalPalette {
        dark: false,
        background: 0xEFF1F5,
        foreground: 0x4C4F69,
        cursor: 0x8839EF,
        selection: 0xCCD0DA,
        ansi: [
            0x5C5F77, 0xD20F39, 0x40A02B, 0xDF8E1D, 0x1E66F5, 0xEA76CB, 0x179299, 0xACB0BE,
            0x6C6F85, 0xD20F39, 0x40A02B, 0xDF8E1D, 0x1E66F5, 0xEA76CB, 0x179299, 0x4C4F69,
        ],
        font_family: "SF Mono".into(),
        font_size: 14,
        font_features: vec!["-calt".into(), "-liga".into(), "-dlig".into()],
        thicken: true,
        thicken_strength: 80,
        cell_width: Some("-8%".into()),
        cell_height: Some("12%".into()),
        padding_x: 2,
        padding_y: 4,
    };
    let config = palette.config_text();
    assert!(config.contains("background = #EFF1F5"));
    assert!(config.contains("foreground = #4C4F69"));
    assert!(config.contains("palette = 0=#5C5F77"));
    assert!(config.contains("palette = 15=#4C4F69"));
    assert!(config.contains("font-family = \"SF Mono\""));
    assert!(config.contains("font-family-bold = \"SF Mono\""));
    assert!(config.contains("font-family-bold = \"PingFang SC\""));
    assert!(config.contains("font-size = 14"));
    assert!(config.contains("font-feature = -calt"));
    assert!(config.contains("font-thicken = true"));
    assert!(config.contains("font-thicken-strength = 80"));
    assert!(config.contains("adjust-cell-width = -8%"));
    assert!(config.contains("adjust-cell-height = 12%"));
    assert!(config.contains("window-padding-x = 2"));
    assert!(config.contains("window-padding-y = 4"));
    let builtin = TerminalPalette {
        font_family: String::new(),
        font_features: Vec::new(),
        thicken: false,
        thicken_strength: 255,
        cell_width: None,
        cell_height: None,
        padding_x: 0,
        padding_y: 0,
        ..palette.clone()
    };
    let builtin_config = builtin.config_text();
    assert!(
        !builtin_config.contains("font-family = "),
        "{builtin_config}"
    );
    assert!(builtin_config.contains("font-family-bold = \"JetBrains Mono\""));
    assert!(builtin_config.contains("font-family-bold = \"PingFang SC\""));
    assert!(!builtin_config.contains("Menlo"));
    let jbm = builtin_config
        .find("font-family-bold = \"JetBrains Mono\"")
        .unwrap();
    let cjk = builtin_config
        .find("font-family-bold = \"PingFang SC\"")
        .unwrap();
    assert!(jbm < cjk);
    assert!(!builtin_config.contains("font-thicken"));
    assert!(!builtin_config.contains("adjust-cell-width"));
    assert_ne!(palette.signature(), builtin.signature());
}

#[test]
fn default_ghostty_config_anchors_latin_bold_before_cjk() {
    let mut out = String::new();
    write_font_families_with(&mut out, "", &["PingFang SC", "Hiragino Sans"]);
    assert!(
        !out.contains("font-family = "),
        "regular list stays empty so embedded JetBrains Mono remains Latin regular\n{out}"
    );
    let jbm = out.find("font-family-bold = \"JetBrains Mono\"").unwrap();
    let cjk = out.find("font-family-bold = \"PingFang SC\"").unwrap();
    assert!(jbm < cjk);
    assert!(!out.contains("Menlo"));
}

#[test]
fn cjk_fallback_order_follows_preferred_language_tags() {
    assert_eq!(
        cjk_families_for_languages(&["zh-Hant-TW"])[0],
        "PingFang TC"
    );
    assert_eq!(cjk_families_for_languages(&["ja-JP"])[0], "Hiragino Sans");
    assert_eq!(
        cjk_families_for_languages(&["ko-KR"])[0],
        "Apple SD Gothic Neo"
    );
    assert_eq!(
        cjk_families_for_languages(&["zh-Hans-CN"])[0],
        "PingFang SC"
    );
    assert_eq!(cjk_families_for_languages(&["en-US"])[0], "PingFang SC");
}

#[test]
fn ghostty_config_puts_the_primary_bold_face_ahead_of_cjk_fallbacks() {
    let mut config = String::new();
    write_font_families_with(
        &mut config,
        "Menlo",
        &cjk_families_for_languages(&["ja-JP"]),
    );
    let primary_bold = config.find("font-family-bold = \"Menlo\"").unwrap();
    let cjk_bold = config.find("font-family-bold = \"Hiragino Sans\"").unwrap();
    assert!(primary_bold < cjk_bold);
    let primary = config.find("font-family = \"Menlo\"").unwrap();
    let cjk = config.find("font-family = \"Hiragino Sans\"").unwrap();
    assert!(primary < cjk);
}

#[test]
fn clipboard_write_prefers_plain_text_payload() {
    let mime_html = CString::new("text/html").unwrap();
    let html = CString::new("<b>x</b>").unwrap();
    let mime_plain = CString::new("text/plain").unwrap();
    let plain = CString::new("hello\nworld").unwrap();
    let contents = [
        ffi::ghostty_clipboard_content_s {
            mime: mime_html.as_ptr(),
            data: html.as_ptr(),
        },
        ffi::ghostty_clipboard_content_s {
            mime: mime_plain.as_ptr(),
            data: plain.as_ptr(),
        },
    ];
    assert_eq!(
        clipboard_text_from_items(&contents).as_deref(),
        Some("hello\nworld")
    );
}

#[test]
fn clipboard_write_skips_empty_payloads() {
    let mime = CString::new("text/plain").unwrap();
    let data = CString::new("").unwrap();
    let contents = [ffi::ghostty_clipboard_content_s {
        mime: mime.as_ptr(),
        data: data.as_ptr(),
    }];
    assert_eq!(clipboard_text_from_items(&contents), None);
    assert_eq!(clipboard_text_from_contents(std::ptr::null(), 2), None);
}

fn test_palette() -> TerminalPalette {
    TerminalPalette {
        dark: true,
        background: 0x000000,
        foreground: 0xffffff,
        cursor: 0xffffff,
        selection: 0x444444,
        ansi: [0; 16],
        font_family: String::new(),
        font_size: 12,
        font_features: Vec::new(),
        thicken: false,
        thicken_strength: 0,
        cell_width: None,
        cell_height: None,
        padding_x: 0,
        padding_y: 0,
    }
}

/// Ghostty writes to the pty from its IO thread, so bytes follow a key
/// event asynchronously. Wait for the first bytes, then for the queue
/// to stay quiet so a multi-write sequence is read whole.
fn pty_bytes(terminal: &Terminal) -> Vec<u8> {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut quiet = 0;
    loop {
        let _ = Terminal::tick_runtime();
        let mut received = false;
        while let Some(bytes) = terminal.try_input() {
            out.extend(bytes);
            received = true;
        }
        if received {
            quiet = 0;
        } else if !out.is_empty() {
            quiet += 1;
            if quiet >= 5 {
                break;
            }
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    out
}

fn press(terminal: &Terminal, key: &str, text: Option<&str>, modifiers: KeyModifiers) -> Vec<u8> {
    assert!(
        terminal.send_key(KeyAction::Press, key, text, modifiers),
        "Ghostty should handle {key} with {modifiers:?}"
    );
    pty_bytes(terminal)
}

const NONE: KeyModifiers = KeyModifiers {
    control: false,
    alt: false,
    shift: false,
    platform: false,
};
const SHIFT: KeyModifiers = KeyModifiers {
    shift: true,
    ..NONE
};
const CTRL: KeyModifiers = KeyModifiers {
    control: true,
    ..NONE
};
const ALT: KeyModifiers = KeyModifiers { alt: true, ..NONE };
const SUPER: KeyModifiers = KeyModifiers {
    platform: true,
    ..NONE
};

/// Real libghostty surface: the bytes are whatever Ghostty's encoder
/// produces, asserted from observation rather than assumption. GPUI
/// reports Enter/Tab with a control character in `key_char`, letters
/// with the typed character, and Ctrl/Alt chords without text.
#[test]
fn keys_are_encoded_by_ghostty_for_the_terminal_modes_in_effect() {
    let terminal = Terminal::new(80, 24, 100, &test_palette()).expect("ghostty surface");
    terminal.set_focus(true);

    // Legacy encoding (no keyboard protocol negotiated).
    assert_eq!(press(&terminal, "enter", Some("\n"), NONE), b"\r");
    assert_eq!(
        press(&terminal, "enter", Some("\n"), SHIFT),
        b"\x1b[27;2;13~",
        "Ghostty disambiguates Shift+Enter with the xterm modifyOtherKeys form"
    );
    assert_eq!(press(&terminal, "c", None, CTRL), [0x03]);
    assert_eq!(press(&terminal, "b", None, ALT), b"\x1bb");
    assert_eq!(press(&terminal, "up", None, NONE), b"\x1b[A");
    assert_eq!(press(&terminal, "left", None, NONE), b"\x1b[D");
    assert_eq!(press(&terminal, "tab", Some("\t"), SHIFT), b"\x1b[Z");
    assert_eq!(press(&terminal, "tab", Some("\t"), NONE), b"\t");
    assert_eq!(press(&terminal, "backspace", None, NONE), [0x7f]);
    assert_eq!(press(&terminal, "a", Some("A"), SHIFT), b"A");
    assert_eq!(press(&terminal, "ü", Some("ü"), NONE), "ü".as_bytes());
    // macOS editing chords kept from Ghostty's defaults.
    assert_eq!(press(&terminal, "left", None, SUPER), [0x01]);
    assert_eq!(press(&terminal, "backspace", None, SUPER), [0x15]);
    assert_eq!(press(&terminal, "left", None, ALT), b"\x1bb");
    // Ghostty's own app shortcuts are cleared: nothing is swallowed and
    // nothing is written for a ⌘ chord the legacy encoding cannot express.
    assert!(!terminal.send_key(KeyAction::Press, "k", None, SUPER));
    assert!(!terminal.send_key(KeyAction::Press, "d", None, SUPER));
    assert!(!terminal.send_key(KeyAction::Press, "=", None, SUPER));
    assert!(terminal.try_input().is_none());
    // Releases are silent outside the kitty protocol.
    assert!(!terminal.send_key(KeyAction::Release, "a", None, NONE));

    // Application cursor keys.
    terminal.apply_frame(b"\x1b[?1h", false);
    assert_eq!(press(&terminal, "up", None, NONE), b"\x1bOA");
    terminal.apply_frame(b"\x1b[?1l", false);

    // Kitty keyboard protocol: disambiguate escape codes.
    terminal.apply_frame(b"\x1b[>1u", false);
    assert_eq!(press(&terminal, "enter", Some("\n"), SHIFT), b"\x1b[13;2u");
    assert_eq!(press(&terminal, "enter", Some("\n"), NONE), b"\r");
    assert_eq!(press(&terminal, "c", None, CTRL), b"\x1b[99;5u");
    assert_eq!(press(&terminal, "b", None, ALT), b"\x1b[98;3u");
    assert_eq!(press(&terminal, "tab", Some("\t"), SHIFT), b"\x1b[9;2u");
    assert_eq!(press(&terminal, "up", None, NONE), b"\x1b[A");
    assert_eq!(press(&terminal, "backspace", None, NONE), [0x7f]);
    assert_eq!(press(&terminal, "a", Some("a"), NONE), b"a");
    terminal.apply_frame(b"\x1b[<u", false);

    // Kitty keyboard protocol with event types: releases are reported.
    terminal.apply_frame(b"\x1b[>3u", false);
    assert_eq!(press(&terminal, "c", None, CTRL), b"\x1b[99;5u");
    assert!(terminal.send_key(KeyAction::Release, "c", None, CTRL));
    assert_eq!(pty_bytes(&terminal), b"\x1b[99;5:3u");
    terminal.apply_frame(b"\x1b[<u", false);

    // xterm modifyOtherKeys = 2.
    terminal.apply_frame(b"\x1b[>4;2m", false);
    assert_eq!(
        press(&terminal, "enter", Some("\n"), SHIFT),
        b"\x1b[27;2;13~"
    );
}
