use super::*;

pub(super) fn appearance_select(
    cx: &mut Context<OcHerdrView>,
    id: &'static str,
    on_select: impl Fn(&mut OcHerdrView, usize, &mut Window, &mut Context<OcHerdrView>) + 'static,
) -> impl Fn(SelectRowEvent, &mut Window, &mut App) + 'static {
    let listener = cx.listener(move |this, event: &SelectRowEvent, window, cx| {
        if let Some(index) = apply_select_event(&mut this.open_select, id, *event) {
            on_select(this, index, window, cx);
        } else {
            cx.notify();
        }
    });
    move |event, window, cx| listener(&event, window, cx)
}

pub(super) fn apply_select_event(
    open_select: &mut Option<SharedString>,
    id: &str,
    event: SelectRowEvent,
) -> Option<usize> {
    match event {
        SelectRowEvent::Open(true) => {
            *open_select = Some(id.into());
            None
        }
        SelectRowEvent::Open(false) => {
            *open_select = None;
            None
        }
        SelectRowEvent::Select(index) => {
            *open_select = None;
            Some(index)
        }
    }
}

const FONT_SIZE_PRESETS: [f32; 8] = [11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 18.0, 20.0];
const OPACITY_PRESETS: [f64; 4] = [1.0, 0.92, 0.84, 0.72];

pub(super) fn font_size_choices(current: f32) -> (Vec<String>, Vec<f32>, usize) {
    let (values, selected) = inject_current(&FONT_SIZE_PRESETS, current, |value| {
        format_font_size(*value) == format_font_size(current)
    });
    let labels = values.iter().copied().map(format_font_size).collect();
    (labels, values, selected)
}

pub(super) fn opacity_choices(current: f64) -> (Vec<String>, Vec<f64>, usize) {
    let (values, selected) = inject_current(&OPACITY_PRESETS, current, |value| {
        format_opacity(*value) == format_opacity(current)
    });
    let labels = values.iter().copied().map(opacity_percent_label).collect();
    (labels, values, selected)
}

pub(super) fn opacity_percent_label(opacity: f64) -> String {
    let percent = opacity * 100.0;
    if (percent - percent.round()).abs() < 1e-6 {
        format!("{}%", percent.round() as i32)
    } else {
        let text = format!("{percent:.2}");
        format!("{}%", text.trim_end_matches('0').trim_end_matches('.'))
    }
}

pub(super) fn padding_choices(current: u32) -> (Vec<String>, Vec<u32>, usize) {
    let (values, selected) = inject_current(&PADDING_CHOICES, current, |value| *value == current);
    let labels = values.iter().map(u32::to_string).collect();
    (labels, values, selected)
}

pub(super) fn thicken_strength_choices(i18n: I18n, current: u8) -> (Vec<String>, Vec<u8>, usize) {
    let presets = ThickenStrengthChoice::ALL.map(ThickenStrengthChoice::value);
    let (values, selected) = inject_current(&presets, current, |value| *value == current);
    let labels = values
        .iter()
        .map(|&value| {
            ThickenStrengthChoice::ALL
                .into_iter()
                .find(|choice| choice.value() == value)
                .map(|choice| {
                    i18n.text(match choice {
                        ThickenStrengthChoice::Subtle => k::APPEARANCE_FONT_THICKEN_STRENGTH_SUBTLE,
                        ThickenStrengthChoice::Medium => k::APPEARANCE_FONT_THICKEN_STRENGTH_MEDIUM,
                        ThickenStrengthChoice::Strong => k::APPEARANCE_FONT_THICKEN_STRENGTH_STRONG,
                        ThickenStrengthChoice::Max => k::APPEARANCE_FONT_THICKEN_STRENGTH_MAX,
                    })
                    .to_owned()
                })
                .unwrap_or_else(|| value.to_string())
        })
        .collect();
    (labels, values, selected)
}

pub(super) fn font_feature_choices(
    i18n: I18n,
    current: &[String],
) -> (Vec<String>, Vec<Vec<String>>, usize) {
    let presets = vec![Vec::new(), no_ligature_features()];
    let (values, selected) = inject_current(&presets, current.to_vec(), |value| value == current);
    let labels = values
        .iter()
        .map(|features| font_feature_label(i18n, features))
        .collect();
    (labels, values, selected)
}

pub(super) fn font_feature_label(i18n: I18n, features: &[String]) -> String {
    if features.is_empty() {
        i18n.text(k::APPEARANCE_FONT_FEATURE_DEFAULT).to_owned()
    } else if features
        .iter()
        .map(String::as_str)
        .eq(NO_LIGATURES.iter().copied())
    {
        i18n.text(k::APPEARANCE_FONT_FEATURE_NO_LIGATURES)
            .to_owned()
    } else {
        features.join(", ")
    }
}

pub(super) fn cell_width_choices(
    i18n: I18n,
    current: Option<MetricModifier>,
) -> (Vec<String>, Vec<Option<MetricModifier>>, usize) {
    let presets: Vec<Option<MetricModifier>> = CellWidthChoice::ALL
        .into_iter()
        .map(CellWidthChoice::metric)
        .collect();
    let (values, selected) = inject_current(&presets, current, |value| *value == current);
    let labels = values
        .iter()
        .map(|metric| cell_width_label(i18n, *metric))
        .collect();
    (labels, values, selected)
}

pub(super) fn cell_height_choices(
    i18n: I18n,
    current: Option<MetricModifier>,
) -> (Vec<String>, Vec<Option<MetricModifier>>, usize) {
    let presets: Vec<Option<MetricModifier>> = CellHeightChoice::ALL
        .into_iter()
        .map(CellHeightChoice::metric)
        .collect();
    let (values, selected) = inject_current(&presets, current, |value| *value == current);
    let labels = values
        .iter()
        .map(|metric| cell_height_label(i18n, *metric))
        .collect();
    (labels, values, selected)
}

pub(super) fn cell_width_label(i18n: I18n, metric: Option<MetricModifier>) -> String {
    match CellWidthChoice::matching(metric) {
        Some(CellWidthChoice::Tight) => i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_TIGHT).to_owned(),
        Some(CellWidthChoice::Normal) => i18n.text(k::COMMON_DEFAULT).to_owned(),
        Some(CellWidthChoice::Wide) => i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_WIDE).to_owned(),
        None => metric
            .map(MetricModifier::to_config)
            .unwrap_or_else(|| i18n.text(k::COMMON_DEFAULT).to_owned()),
    }
}

pub(super) fn cell_height_label(i18n: I18n, metric: Option<MetricModifier>) -> String {
    match CellHeightChoice::matching(metric) {
        Some(CellHeightChoice::Compact) => {
            i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_COMPACT).to_owned()
        }
        Some(CellHeightChoice::Normal) => i18n.text(k::COMMON_DEFAULT).to_owned(),
        Some(CellHeightChoice::Relaxed) => {
            i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_RELAXED).to_owned()
        }
        Some(CellHeightChoice::Loose) => i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_LOOSE).to_owned(),
        None => metric
            .map(MetricModifier::to_config)
            .unwrap_or_else(|| i18n.text(k::COMMON_DEFAULT).to_owned()),
    }
}

fn inject_current<T: Clone>(
    presets: &[T],
    current: T,
    same: impl Fn(&T) -> bool,
) -> (Vec<T>, usize) {
    if let Some(index) = presets.iter().position(&same) {
        return (presets.to_vec(), index);
    }
    let mut values = presets.to_vec();
    values.push(current);
    let index = values.len() - 1;
    (values, index)
}

pub(super) fn terminal_theme_index(selected: Option<&str>, ids: &[Option<String>]) -> usize {
    match selected {
        None => 0,
        Some(id) => ids
            .iter()
            .position(|entry| entry.as_deref() == Some(id))
            .unwrap_or(0),
    }
}

pub(super) fn palette_slot_label(i18n: I18n, index: usize) -> String {
    crate::tf!(
        i18n,
        k::APPEARANCE_PALETTE_SLOT,
        name = i18n.text(PALETTE_SLOT_KEYS[index]),
        index = index
    )
}

pub(super) fn palette_grid_colors(appearance: &AppearanceSettings) -> [u32; 16] {
    let dark = theme::is_dark();
    crate::theme_ansi::apply_overrides(
        crate::theme_ansi::resolved_ansi(
            crate::controller::terminal_overlay(appearance, dark),
            &theme::current(),
            dark,
        ),
        &appearance.palette,
    )
}

pub(super) fn plan_has_changes(plan: &GhosttyImportPlan) -> bool {
    !plan.updates.is_empty() || !plan.themes.is_empty() || plan.terminal_theme.is_some()
}

pub(super) fn build_import_preview(paths: &GhosttyImportPaths) -> GhosttyImportPreview {
    match plan_ghostty_import(paths) {
        Ok(plan) => import_preview_from_plan(plan, paths.app_themes.is_dir()),
        Err(GhosttyImportError::ConfigNotFound { .. }) => GhosttyImportPreview {
            source: None,
            ghostty_app_found: paths.app_themes.is_dir(),
            recognized: Vec::new(),
            unknown: Vec::new(),
            plan: None,
            error: None,
        },
        Err(GhosttyImportError::ThemesMissing { .. }) => keys_only_preview(paths),
        Err(error) => GhosttyImportPreview {
            source: None,
            ghostty_app_found: paths.app_themes.is_dir(),
            recognized: Vec::new(),
            unknown: Vec::new(),
            plan: None,
            error: Some(error.to_string()),
        },
    }
}

pub(super) fn keys_only_preview(paths: &GhosttyImportPaths) -> GhosttyImportPreview {
    let source = if paths.xdg_config.is_file() {
        Some(paths.xdg_config.clone())
    } else if paths.app_support_config.is_file() {
        Some(paths.app_support_config.clone())
    } else {
        None
    };
    let Some(path) = source else {
        return GhosttyImportPreview {
            source: None,
            ghostty_app_found: false,
            recognized: Vec::new(),
            unknown: Vec::new(),
            plan: None,
            error: None,
        };
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            import_preview_from_plan(plan_ghostty_keys(&text, &path).without_themes(), false)
        }
        Err(error) => GhosttyImportPreview {
            source: Some(path.display().to_string()),
            ghostty_app_found: false,
            recognized: Vec::new(),
            unknown: Vec::new(),
            plan: None,
            error: Some(error.to_string()),
        },
    }
}

pub(super) fn import_preview_from_plan(
    plan: GhosttyImportPlan,
    ghostty_app_found: bool,
) -> GhosttyImportPreview {
    let mut recognized = Vec::new();
    for update in &plan.updates {
        recognized.push((update.key.clone(), update.values.join(", ")));
    }
    if let Some(theme) = &plan.terminal_theme {
        recognized.push(("terminal-theme".into(), theme.to_config()));
    }
    for theme in &plan.themes {
        recognized.push((theme.file_name.clone(), theme.name.clone()));
    }
    let unknown = plan
        .unknown_keys
        .iter()
        .map(|key| format!("{} = {}", key.key, key.value))
        .collect();
    GhosttyImportPreview {
        source: Some(plan.source.display().to_string()),
        ghostty_app_found,
        recognized,
        unknown,
        plan: Some(plan),
        error: None,
    }
}
