use super::*;

#[test]
fn opening_a_select_closes_the_one_that_was_already_open() {
    let mut open_select = None;
    apply_select_event(&mut open_select, "a", SelectRowEvent::Open(true));
    assert_eq!(open_select.as_deref(), Some("a"));
    apply_select_event(&mut open_select, "b", SelectRowEvent::Open(true));
    assert_eq!(open_select.as_deref(), Some("b"));
}

#[test]
fn font_size_and_opacity_choices_keep_values_that_are_not_presets() {
    let (size_labels, size_values, size_index) = font_size_choices(13.0);
    assert_eq!(size_values[size_index], 13.0);
    assert!(size_labels.iter().any(|label| label == "13"));

    let (size_labels, size_values, size_index) = font_size_choices(13.5);
    assert_eq!(size_values[size_index], 13.5);
    assert!(
        size_labels.iter().any(|label| label == "13.5"),
        "a Ghostty font-size of 13.5 must appear in the list, not snap to 13 or 14"
    );

    let (_, opacity_values, opacity_index) = opacity_choices(0.85);
    assert_eq!(opacity_values[opacity_index], 0.85);
    assert_eq!(opacity_percent_label(0.85), "85%");
}

#[test]
fn padding_and_cell_height_choices_keep_absolute_ghostty_values() {
    let (_, padding, index) = padding_choices(2);
    assert_eq!(padding[index], 2);

    let i18n = I18n::new(Language::English);
    let (_, values, index) = cell_height_choices(i18n, Some(MetricModifier::Absolute(1)));
    assert_eq!(values[index], Some(MetricModifier::Absolute(1)));
    assert_eq!(
        cell_height_label(i18n, Some(MetricModifier::Absolute(1))),
        "1"
    );
}

#[test]
fn palette_grid_uses_theme_ansi_then_config_overrides() {
    let probe = 0xc0ffee;
    let family = theme::ochub_family();
    let overlay = crate::theme_ansi::overlay_for(Some(&family));
    let dark = theme::is_dark();
    let base = overlay
        .colors(dark)
        .expect("ochub ships an explicit ansi table");
    assert_ne!(
        base[3], probe,
        "fixture color must differ from the theme slot"
    );
    assert_ne!(
        base[1],
        theme::current().red.0,
        "ochub ansi must differ from the old token-placeholder red or this test cannot catch a revert"
    );
    let mut appearance = AppearanceSettings::default();
    let colors = palette_grid_colors(&appearance);
    assert_eq!(colors, base);
    appearance.palette[3] = Some(probe);
    let colors = palette_grid_colors(&appearance);
    assert_eq!(colors[3], probe);
    assert_eq!(colors[0], base[0]);
}

#[test]
fn import_preview_lists_recognized_and_unknown_keys_from_the_real_parser() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let themes = dir.path().join("themes");
    std::fs::create_dir(&themes).unwrap();
    let body = "\
font-size = 15
background-opacity = 0.85
keybind = ctrl+c=copy
shell-integration = zsh
";
    assert!(
        body.contains("0.85") && body.contains("keybind"),
        "fixture must contain the values this test claims to surface"
    );
    std::fs::write(&config, body).unwrap();
    let paths = GhosttyImportPaths {
        xdg_config: config.clone(),
        app_support_config: dir.path().join("missing"),
        app_themes: themes,
    };
    let preview = build_import_preview(&paths);
    assert_eq!(preview.source.as_deref(), Some(config.to_str().unwrap()));
    assert!(
        preview
            .recognized
            .iter()
            .any(|(key, value)| key == "font-size" && value == "15")
    );
    assert!(
        preview
            .recognized
            .iter()
            .any(|(key, value)| key == "background-opacity" && value == "0.85")
    );
    assert!(
        preview.unknown.iter().any(|key| key.starts_with("keybind")),
        "unknown keys must stay visible so they are not silently dropped: {:?}",
        preview.unknown
    );
    let recognized: Vec<&str> = preview
        .recognized
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    for key in &preview.unknown {
        let name = key.split('=').next().unwrap_or(key).trim();
        assert!(
            !recognized.contains(&name),
            "unknown key `{key}` must not also appear as recognized"
        );
    }
    assert!(preview.plan.is_some());
}

#[test]
fn import_preview_is_empty_when_no_config_exists() {
    let dir = tempfile::tempdir().unwrap();
    let paths = GhosttyImportPaths {
        xdg_config: dir.path().join("missing"),
        app_support_config: dir.path().join("also-missing"),
        app_themes: dir.path().join("themes"),
    };
    let preview = build_import_preview(&paths);
    assert_eq!(preview.source, None);
    assert!(preview.recognized.is_empty());
    assert!(preview.unknown.is_empty());
    assert!(preview.plan.is_none());
}

#[test]
fn missing_ghostty_app_still_previews_known_keys() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let body = "font-size = 15\ntheme = Probe\n";
    assert!(body.contains("font-size = 15"));
    std::fs::write(&config, body).unwrap();
    let paths = GhosttyImportPaths {
        xdg_config: config,
        app_support_config: dir.path().join("missing"),
        app_themes: dir.path().join("no-app"),
    };
    let preview = build_import_preview(&paths);
    assert!(!preview.ghostty_app_found);
    assert!(
        preview
            .recognized
            .iter()
            .any(|(key, value)| key == "font-size" && value == "15")
    );
    assert!(
        preview
            .plan
            .as_ref()
            .is_some_and(|plan| plan.themes.is_empty() && plan.terminal_theme.is_none())
    );
    assert!(preview.plan.as_ref().is_some_and(plan_has_changes));
}

#[test]
fn restoring_known_keys_leaves_comments_and_unknown_assignments() {
    let mut document = crate::config::ConfigDocument::parse(
        "# keep\nfont-size = 18\nmystery-option = wow\nlanguage = en\n",
    );
    assert!(document.serialize().contains("# keep"));
    crate::config::strip_known_keys(&mut document);
    let written = document.serialize();
    assert!(written.contains("# keep"));
    assert!(written.contains("mystery-option = wow"));
    assert!(!written.contains("font-size"));
    let (config, _) = crate::config::values::AppConfig::from_document(&document);
    let appearance = crate::config::values::appearance_from_config(&config);
    assert_eq!(appearance, AppearanceSettings::default());
}

#[test]
fn dismiss_sheet_only_reports_true_when_a_sheet_was_open() {
    let mut ui = AppearanceUi::default();
    assert!(!ui.dismiss_sheet());
    ui.sheet = AppearanceSheet::RestoreConfirm;
    assert!(ui.dismiss_sheet());
    assert_eq!(ui.sheet, AppearanceSheet::None);
    assert!(!ui.dismiss_sheet());
}
