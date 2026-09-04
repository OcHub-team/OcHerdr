use std::path::Path;
use std::process::Command;

use super::super::*;
use crate::a11y::apply_dialog;
use crate::config::import::{
    GhosttyImportError, GhosttyImportPaths, GhosttyImportPlan, plan_ghostty_import,
    plan_ghostty_keys,
};
use crate::config::values::{
    MetricModifier, NO_LIGATURES, format_font_size, format_opacity, no_ligature_features,
};
use crate::i18n::Key;
use ochub_ui::layout::{
    self, SelectRowEvent, SelectRowState, action_row, content_column, group, page_header, row,
    row_label, scroll_body, section_header, select_row, switch_row,
};

/// Dialog-only appearance state. Persisted values live on [`AppearanceSettings`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppearanceUi {
    selected_slot: Option<u8>,
    sheet: AppearanceSheet,
    status: Option<SharedString>,
}

impl Default for AppearanceUi {
    fn default() -> Self {
        Self {
            selected_slot: None,
            sheet: AppearanceSheet::None,
            status: None,
        }
    }
}

impl AppearanceUi {
    pub(crate) fn dismiss_sheet(&mut self) -> bool {
        if matches!(self.sheet, AppearanceSheet::None) {
            return false;
        }
        self.sheet = AppearanceSheet::None;
        true
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum AppearanceSheet {
    #[default]
    None,
    ImportPreview(Box<GhosttyImportPreview>),
    RestoreConfirm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GhosttyImportPreview {
    source: Option<String>,
    ghostty_app_found: bool,
    recognized: Vec<(String, String)>,
    unknown: Vec<String>,
    plan: Option<GhosttyImportPlan>,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThickenStrengthChoice {
    Subtle,
    Medium,
    Strong,
    Max,
}

impl ThickenStrengthChoice {
    const ALL: [Self; 4] = [Self::Subtle, Self::Medium, Self::Strong, Self::Max];

    fn value(self) -> u8 {
        match self {
            Self::Subtle => 64,
            Self::Medium => 128,
            Self::Strong => 192,
            Self::Max => 255,
        }
    }
}

const PADDING_CHOICES: [u32; 6] = [0, 4, 8, 12, 16, 24];
const PALETTE_PRESETS: [u32; 8] = [
    0x1C1C1C, 0xC41A16, 0x1B8A4A, 0xC7A000, 0x1A5FB4, 0x813D9C, 0x0E9AA7, 0xF5F5F5,
];
const PALETTE_SLOT_KEYS: [Key; 16] = [
    k::APPEARANCE_PALETTE_BLACK,
    k::APPEARANCE_PALETTE_RED,
    k::APPEARANCE_PALETTE_GREEN,
    k::APPEARANCE_PALETTE_YELLOW,
    k::APPEARANCE_PALETTE_BLUE,
    k::APPEARANCE_PALETTE_MAGENTA,
    k::APPEARANCE_PALETTE_CYAN,
    k::APPEARANCE_PALETTE_WHITE,
    k::APPEARANCE_PALETTE_BRIGHT_BLACK,
    k::APPEARANCE_PALETTE_BRIGHT_RED,
    k::APPEARANCE_PALETTE_BRIGHT_GREEN,
    k::APPEARANCE_PALETTE_BRIGHT_YELLOW,
    k::APPEARANCE_PALETTE_BRIGHT_BLUE,
    k::APPEARANCE_PALETTE_BRIGHT_MAGENTA,
    k::APPEARANCE_PALETTE_BRIGHT_CYAN,
    k::APPEARANCE_PALETTE_BRIGHT_WHITE,
];
const GHOSTTY_APP_THEMES: &str = "/Applications/Ghostty.app/Contents/Resources/ghostty/themes";

impl OcHerdrView {
    pub(super) fn render_appearance(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let appearance_scroll = self.appearance_scroll.clone();
        let header = page_header(
            i18n.text(k::APPEARANCE_TITLE),
            Some(i18n.text(k::APPEARANCE_SUBTITLE).into()),
        )
        .pl(px(78.))
        .child(
            button(
                "close-appearance",
                i18n.text(k::COMMON_DONE),
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, window, cx| this.close_appearance(window, cx))),
        );
        let sheet = self.render_appearance_sheet(cx);
        apply_dialog(
            // Settings are a reading surface, not part of the frosted terminal
            // canvas. Keep the page opaque even when the workspace uses glass.
            layout::page().bg(theme::current().bg.rgba()),
            "appearance-dialog",
            i18n.text(k::APPEARANCE_TITLE),
        )
        .absolute()
        .inset_0()
        .occlude()
        .child(header)
        .child(scroll_body(
            "appearance-body",
            &appearance_scroll,
            self.appearance_column(cx),
        ))
        .children(sheet)
    }

    fn appearance_column(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        content_column()
            .child(section_header(
                self.i18n.text(k::APPEARANCE_GENERAL_LABEL),
                None,
            ))
            .child(group(vec![
                self.language_row(cx).into_any_element(),
                self.agent_notifications_row(cx).into_any_element(),
            ]))
            .child(section_header(
                self.i18n.text(k::APPEARANCE_THEME_LABEL),
                Some(self.i18n.text(k::APPEARANCE_THEME_DESCRIPTION).into()),
            ))
            .child(self.render_theme_family_list(cx))
            .child(group(vec![
                self.mode_row(cx).into_any_element(),
                self.terminal_theme_row(cx).into_any_element(),
            ]))
            .child(section_header(
                self.i18n.text(k::APPEARANCE_WINDOW_LABEL),
                None,
            ))
            .child(group(vec![
                self.backdrop_row(cx).into_any_element(),
                self.opacity_row(cx).into_any_element(),
                self.padding_row(cx, true).into_any_element(),
                self.padding_row(cx, false).into_any_element(),
            ]))
            .child(section_header(
                self.i18n.text(k::APPEARANCE_FONT_LABEL),
                Some(self.i18n.text(k::APPEARANCE_FONT_DESCRIPTION).into()),
            ))
            .child(self.render_font_family_list(cx))
            .child(group(vec![
                self.font_size_row(cx).into_any_element(),
                self.thicken_row(cx).into_any_element(),
                self.thicken_strength_row(cx).into_any_element(),
                self.font_feature_row(cx).into_any_element(),
                self.cell_width_row(cx).into_any_element(),
                self.cell_height_row(cx).into_any_element(),
            ]))
            .child(section_header(
                self.i18n.text(k::APPEARANCE_PALETTE_LABEL),
                Some(self.i18n.text(k::APPEARANCE_PALETTE_DESCRIPTION).into()),
            ))
            .child(self.render_palette_grid(cx))
            .child(self.render_palette_detail(cx))
            .child(section_header(
                self.i18n.text(k::APPEARANCE_CONFIG_LABEL),
                None,
            ))
            .child(group(vec![
                self.import_row(cx).into_any_element(),
                self.open_config_row(cx).into_any_element(),
                self.restore_row(cx).into_any_element(),
            ]))
            .children(self.appearance_ui.status.clone().map(|status| {
                div()
                    .w_full()
                    .px_4()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(status)
            }))
    }

    fn language_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        select_row(
            "language",
            i18n.text(k::COMMON_LANGUAGE_LABEL),
            Some(i18n.text(k::COMMON_LANGUAGE_DESCRIPTION).into()),
            &[
                i18n.text(k::COMMON_SYSTEM),
                i18n.text(k::COMMON_LANGUAGE_ENGLISH),
                i18n.text(k::COMMON_LANGUAGE_SIMPLIFIED_CHINESE),
            ],
            self.i18n.preference().index(),
            self.select_row_state("language"),
            appearance_select(cx, "language", |this, index, _window, cx| {
                this.set_language(Language::from_index(index), cx);
            }),
        )
    }

    fn agent_notifications_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let listener = cx.listener(|this, _: &(), _window, cx| {
            this.set_agent_notifications(!this.agent_notifications, cx);
        });
        switch_row(
            "agent-notifications",
            i18n.text(k::APPEARANCE_NOTIFICATIONS_LABEL),
            Some(i18n.text(k::APPEARANCE_NOTIFICATIONS_DESCRIPTION).into()),
            self.agent_notifications,
            false,
            move |window, cx| listener(&(), window, cx),
        )
    }

    fn mode_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        select_row(
            "appearance-mode",
            i18n.text(k::APPEARANCE_MODE_LABEL),
            Some(i18n.text(k::APPEARANCE_MODE_DESCRIPTION).into()),
            &[
                i18n.text(k::COMMON_SYSTEM),
                i18n.text(k::APPEARANCE_MODE_LIGHT),
                i18n.text(k::APPEARANCE_MODE_DARK),
            ],
            self.appearance.mode.index(),
            self.select_row_state("appearance-mode"),
            appearance_select(cx, "appearance-mode", |this, index, window, cx| {
                let mode = match index {
                    1 => AppearanceMode::Light,
                    2 => AppearanceMode::Dark,
                    _ => AppearanceMode::System,
                };
                this.set_appearance_mode(mode, window, cx);
            }),
        )
    }

    fn terminal_theme_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let mut labels = vec![i18n.text(k::APPEARANCE_THEME_TERMINAL_FOLLOW).to_owned()];
        let mut ids = vec![None];
        for record in &theme::load_registry().themes {
            labels.push(record.family.name.clone());
            ids.push(Some(record.family.id.clone()));
        }
        let current = self.appearance.terminal_theme.clone();
        if let Some(id) = current.as_deref()
            && !ids.iter().any(|entry| entry.as_deref() == Some(id))
        {
            labels.push(id.to_owned());
            ids.push(Some(id.to_owned()));
        }
        let selected = terminal_theme_index(current.as_deref(), &ids);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        select_row(
            "appearance-terminal-theme",
            i18n.text(k::APPEARANCE_THEME_TERMINAL_LABEL),
            Some(i18n.text(k::APPEARANCE_THEME_TERMINAL_DESCRIPTION).into()),
            &label_refs,
            selected,
            self.select_row_state("appearance-terminal-theme"),
            appearance_select(
                cx,
                "appearance-terminal-theme",
                move |this, index, window, cx| {
                    this.set_terminal_theme(ids.get(index).cloned().flatten(), window, cx);
                },
            ),
        )
    }

    fn backdrop_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        select_row(
            "appearance-backdrop",
            i18n.text(k::APPEARANCE_BACKDROP_LABEL),
            Some(i18n.text(k::APPEARANCE_BACKDROP_DESCRIPTION).into()),
            &[
                i18n.text(k::APPEARANCE_BACKDROP_OPAQUE),
                i18n.text(k::APPEARANCE_BACKDROP_CLEAR),
                i18n.text(k::APPEARANCE_BACKDROP_BLUR),
            ],
            self.appearance.backdrop.index(),
            self.select_row_state("appearance-backdrop"),
            appearance_select(cx, "appearance-backdrop", |this, index, window, cx| {
                let backdrop = match index {
                    1 => BackdropMode::Transparent,
                    2 => BackdropMode::Blurred,
                    _ => BackdropMode::Opaque,
                };
                this.set_backdrop_mode(backdrop, window, cx);
            }),
        )
    }

    fn opacity_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let (labels, values, selected) = opacity_choices(self.appearance.background_opacity);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        select_row(
            "appearance-opacity",
            i18n.text(k::APPEARANCE_OPACITY_LABEL),
            Some(i18n.text(k::APPEARANCE_OPACITY_DESCRIPTION).into()),
            &label_refs,
            selected,
            self.select_row_state("appearance-opacity"),
            appearance_select(cx, "appearance-opacity", move |this, index, window, cx| {
                let Some(&opacity) = values.get(index) else {
                    return;
                };
                this.set_background_opacity(opacity, window, cx);
            }),
        )
    }

    fn padding_row(&mut self, cx: &mut Context<Self>, horizontal: bool) -> impl IntoElement {
        let i18n = self.i18n;
        let current = if horizontal {
            self.appearance.window_padding_x
        } else {
            self.appearance.window_padding_y
        };
        let (labels, values, selected) = padding_choices(current);
        let (id, label, description) = if horizontal {
            (
                "appearance-padding-x",
                k::APPEARANCE_WINDOW_PADDING_X_LABEL,
                k::APPEARANCE_WINDOW_PADDING_X_DESCRIPTION,
            )
        } else {
            (
                "appearance-padding-y",
                k::APPEARANCE_WINDOW_PADDING_Y_LABEL,
                k::APPEARANCE_WINDOW_PADDING_Y_DESCRIPTION,
            )
        };
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        select_row(
            id,
            i18n.text(label),
            Some(i18n.text(description).into()),
            &label_refs,
            selected,
            self.select_row_state(id),
            appearance_select(cx, id, move |this, index, window, cx| {
                let Some(&value) = values.get(index) else {
                    return;
                };
                this.set_window_padding(horizontal, value, window, cx);
            }),
        )
    }

    fn font_size_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let (labels, values, selected) = font_size_choices(self.appearance.font.size);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        select_row(
            "appearance-font-size",
            i18n.text(k::APPEARANCE_FONT_SIZE_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_SIZE_DESCRIPTION).into()),
            &label_refs,
            selected,
            self.select_row_state("appearance-font-size"),
            appearance_select(
                cx,
                "appearance-font-size",
                move |this, index, window, cx| {
                    let Some(&size) = values.get(index) else {
                        return;
                    };
                    this.set_font_size(size, window, cx);
                },
            ),
        )
    }

    fn font_feature_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let (labels, values, selected) = font_feature_choices(i18n, &self.appearance.font.features);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        select_row(
            "appearance-font-feature",
            i18n.text(k::APPEARANCE_FONT_FEATURE_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_FEATURE_DESCRIPTION).into()),
            &label_refs,
            selected,
            self.select_row_state("appearance-font-feature"),
            appearance_select(
                cx,
                "appearance-font-feature",
                move |this, index, window, cx| {
                    let Some(features) = values.get(index).cloned() else {
                        return;
                    };
                    this.set_font_features(features, window, cx);
                },
            ),
        )
    }

    fn thicken_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let thicken_listener = cx.listener(|this, _: &(), window, cx| {
            this.set_font_thicken(!this.appearance.font.thicken, window, cx);
        });
        switch_row(
            "appearance-thicken",
            i18n.text(k::APPEARANCE_FONT_THICKEN_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_THICKEN_DESCRIPTION).into()),
            self.appearance.font.thicken,
            false,
            move |window, cx| thicken_listener(&(), window, cx),
        )
    }

    fn thicken_strength_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let (labels, values, selected) =
            thicken_strength_choices(i18n, self.appearance.font.thicken_strength);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        select_row(
            "appearance-thicken-strength",
            i18n.text(k::APPEARANCE_FONT_THICKEN_STRENGTH_LABEL),
            Some(
                i18n.text(k::APPEARANCE_FONT_THICKEN_STRENGTH_DESCRIPTION)
                    .into(),
            ),
            &label_refs,
            selected,
            SelectRowState::new(
                !self.appearance.font.thicken,
                self.open_select.as_deref() == Some("appearance-thicken-strength"),
            ),
            appearance_select(
                cx,
                "appearance-thicken-strength",
                move |this, index, window, cx| {
                    let Some(&strength) = values.get(index) else {
                        return;
                    };
                    this.set_font_thicken_strength(strength, window, cx);
                },
            ),
        )
    }

    fn cell_width_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let (labels, values, selected) = cell_width_choices(i18n, self.appearance.font.cell_width);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        select_row(
            "appearance-cell-width",
            i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_DESCRIPTION).into()),
            &label_refs,
            selected,
            self.select_row_state("appearance-cell-width"),
            appearance_select(
                cx,
                "appearance-cell-width",
                move |this, index, window, cx| {
                    let Some(&metric) = values.get(index) else {
                        return;
                    };
                    this.set_cell_width(metric, window, cx);
                },
            ),
        )
    }

    fn cell_height_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let (labels, values, selected) =
            cell_height_choices(i18n, self.appearance.font.cell_height);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        select_row(
            "appearance-cell-height",
            i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_DESCRIPTION).into()),
            &label_refs,
            selected,
            self.select_row_state("appearance-cell-height"),
            appearance_select(
                cx,
                "appearance-cell-height",
                move |this, index, window, cx| {
                    let Some(&metric) = values.get(index) else {
                        return;
                    };
                    this.set_cell_height(metric, window, cx);
                },
            ),
        )
    }

    fn import_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let import = cx.listener(|this, _: &(), _window, cx| this.request_ghostty_import(cx));
        action_row(
            "appearance-import",
            i18n.text(k::APPEARANCE_CONFIG_IMPORT_LABEL),
            Some(i18n.text(k::APPEARANCE_CONFIG_IMPORT_DESCRIPTION).into()),
            i18n.text(k::APPEARANCE_CONFIG_IMPORT_ACTION),
            ButtonTone::Primary,
            false,
            move |window, cx| import(&(), window, cx),
        )
    }

    fn open_config_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let open = cx.listener(|this, _: &(), _window, cx| this.open_ocherdr_config(cx));
        action_row(
            "appearance-open-config",
            i18n.text(k::APPEARANCE_CONFIG_OPEN_LABEL),
            Some(i18n.text(k::APPEARANCE_CONFIG_OPEN_DESCRIPTION).into()),
            i18n.text(k::APPEARANCE_CONFIG_OPEN_ACTION),
            ButtonTone::Neutral,
            false,
            move |window, cx| open(&(), window, cx),
        )
    }

    fn restore_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let restore = cx.listener(|this, _: &(), _window, cx| {
            this.appearance_ui.sheet = AppearanceSheet::RestoreConfirm;
            cx.notify();
        });
        action_row(
            "appearance-restore",
            i18n.text(k::APPEARANCE_CONFIG_RESTORE_LABEL),
            Some(i18n.text(k::APPEARANCE_CONFIG_RESTORE_DESCRIPTION).into()),
            i18n.text(k::APPEARANCE_CONFIG_RESTORE_ACTION),
            ButtonTone::Danger,
            false,
            move |window, cx| restore(&(), window, cx),
        )
    }

    fn render_theme_family_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let mut family_rows = Vec::new();
        for record in &theme::load_registry().themes {
            let family = record.family.clone();
            let family_id = family.id.clone();
            let selected = family_id == self.appearance.theme_family;
            let palette = if theme::is_dark() {
                family.dark
            } else {
                family.light
            };
            let description = if family.description.is_empty() {
                i18n.text(k::APPEARANCE_THEME_FALLBACK_DESCRIPTION)
                    .to_owned()
            } else {
                family.description.clone()
            };
            family_rows.push(
                div()
                    .id(ochub_ui::gpui::ElementId::Name(
                        format!("appearance-family-{family_id}").into(),
                    ))
                    .role(ochub_ui::gpui::Role::Button)
                    .tab_stop(false)
                    .aria_label(i18n.use_theme_label(&family.name))
                    .aria_selected(selected)
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_h(px(52.))
                    .px_3()
                    .py_2()
                    .rounded(px(CORNER_CONTROL))
                    .border_1()
                    .border_color(if selected {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(if selected {
                        theme::selection()
                    } else {
                        theme::surface()
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme_family(family_id.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(3.))
                            .p_1()
                            .rounded(px(CORNER_CONTROL))
                            .bg(palette.bg.rgba())
                            .border_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(CORNER_COMPACT))
                                    .bg(palette.accent_fill.rgba()),
                            )
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(CORNER_COMPACT))
                                    .bg(palette.surface.rgba()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .gap(px(2.))
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(family.name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child(description),
                            ),
                    )
                    .when(selected, |row| row.child(status_dot(theme::accent())))
                    .into_any_element(),
            );
        }
        div()
            .id("appearance-theme-list")
            .role(ochub_ui::gpui::Role::List)
            .aria_label(i18n.text(k::APPEARANCE_THEME_LABEL))
            .w_full()
            .grid()
            .grid_cols(2)
            .gap_2()
            .children(family_rows)
    }

    fn render_font_family_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let selected_family = self.appearance.font.family.clone();
        let mut families = vec![(
            String::new(),
            i18n.text(k::APPEARANCE_FONT_JETBRAINS).to_owned(),
        )];
        for family in crate::fonts::monospace_families() {
            if family != "JetBrains Mono" {
                families.push((family.clone(), family.clone()));
            }
        }
        if !selected_family.is_empty()
            && !families
                .iter()
                .any(|(family, _)| family == &selected_family)
        {
            families.insert(1, (selected_family.clone(), selected_family.clone()));
        }
        let rows = families
            .into_iter()
            .map(|(family, label)| {
                let selected = family == selected_family;
                let preview_family = family.clone();
                div()
                    .id(ochub_ui::gpui::ElementId::Name(
                        format!(
                            "appearance-font-{}",
                            if family.is_empty() {
                                "ghostty"
                            } else {
                                &family
                            }
                        )
                        .into(),
                    ))
                    .role(ochub_ui::gpui::Role::Button)
                    .tab_stop(false)
                    .aria_label(label.clone())
                    .aria_selected(selected)
                    .flex()
                    .items_center()
                    .justify_between()
                    .min_h(px(36.))
                    .px_3()
                    .rounded(px(CORNER_CONTROL))
                    .border_1()
                    .border_color(if selected {
                        theme::accent()
                    } else {
                        theme::border()
                    })
                    .bg(if selected {
                        theme::selection()
                    } else {
                        theme::surface()
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_font_family(family.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .when(!preview_family.is_empty(), |label| {
                                label.font_family(preview_family.clone())
                            })
                            .child(label),
                    )
                    .when(selected, |row| row.child(status_dot(theme::accent())))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .id("appearance-font-list")
            .role(ochub_ui::gpui::Role::List)
            .aria_label(i18n.text(k::APPEARANCE_FONT_LABEL))
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(220.))
            .overflow_scroll()
            .children(rows)
    }

    fn render_palette_grid(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let colors = palette_grid_colors(&self.appearance);
        let selected = self.appearance_ui.selected_slot;
        let swatches = colors.into_iter().enumerate().map(|(index, color)| {
            let slot = index as u8;
            let is_selected = selected == Some(slot);
            let name = i18n.text(PALETTE_SLOT_KEYS[index]);
            let hex = theme::ThemeColor::new(color).hex();
            div()
                .id(ochub_ui::gpui::ElementId::Name(
                    format!("appearance-palette-{index}").into(),
                ))
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(palette_slot_label(i18n, index))
                .aria_selected(is_selected)
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .p_1()
                .rounded(px(CORNER_CONTROL))
                .border_1()
                .border_color(if is_selected {
                    theme::accent()
                } else {
                    theme::border()
                })
                .bg(if is_selected {
                    theme::selection()
                } else {
                    theme::surface()
                })
                .hover(|style| style.bg(theme::surface_hover()))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.appearance_ui.selected_slot = Some(slot);
                    cx.notify();
                }))
                .child(
                    div()
                        .size(px(28.))
                        .rounded(px(CORNER_COMPACT))
                        .bg(theme::ThemeColor::new(color).rgba())
                        .border_1()
                        .border_color(theme::border()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::muted())
                        .child(name.to_owned()),
                )
                .child(div().text_xs().text_color(theme::muted()).child(hex))
                .into_any_element()
        });
        div()
            .id("appearance-palette-grid")
            .role(ochub_ui::gpui::Role::List)
            .aria_label(i18n.text(k::APPEARANCE_PALETTE_LABEL))
            .w_full()
            .grid()
            .grid_cols(8)
            .gap_2()
            .children(swatches)
    }

    fn render_palette_detail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let Some(slot) = self.appearance_ui.selected_slot else {
            return row()
                .child(row_label(
                    i18n.text(k::APPEARANCE_PALETTE_SELECT_HINT),
                    None,
                ))
                .into_any_element();
        };
        let index = slot as usize;
        let colors = palette_grid_colors(&self.appearance);
        let color = colors[index];
        let overridden = self.appearance.palette[index].is_some();
        let hex = theme::ThemeColor::new(color).hex();
        let reset = cx.listener(move |this, _: &(), window, cx| {
            this.set_palette_slot(slot, None, window, cx);
        });
        let presets = PALETTE_PRESETS.into_iter().map(|preset| {
            let selected = self.appearance.palette[index] == Some(preset);
            let label = crate::tf!(
                i18n,
                k::APPEARANCE_PALETTE_SELECT,
                name = theme::ThemeColor::new(preset).hex()
            );
            div()
                .id(ochub_ui::gpui::ElementId::Name(
                    format!("appearance-palette-preset-{preset:06X}").into(),
                ))
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(label)
                .size(px(22.))
                .rounded(px(CORNER_COMPACT))
                .bg(theme::ThemeColor::new(preset).rgba())
                .border_1()
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::border()
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_palette_slot(slot, Some(preset), window, cx);
                }))
                .into_any_element()
        });
        group(vec![
            row()
                .child(row_label(palette_slot_label(i18n, index), Some(hex.into())))
                .child(div().flex().items_center().gap_1().children(presets))
                .into_any_element(),
            action_row(
                "appearance-palette-reset",
                i18n.text(k::APPEARANCE_PALETTE_RESET),
                Some(i18n.text(k::APPEARANCE_PALETTE_RESET_DESCRIPTION).into()),
                i18n.text(k::APPEARANCE_PALETTE_RESET),
                ButtonTone::Neutral,
                !overridden,
                move |window, cx| reset(&(), window, cx),
            )
            .into_any_element(),
        ])
        .into_any_element()
    }

    fn render_appearance_sheet(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<ochub_ui::gpui::AnyElement> {
        match self.appearance_ui.sheet.clone() {
            AppearanceSheet::None => None,
            AppearanceSheet::ImportPreview(preview) => {
                Some(self.render_import_preview(*preview, cx).into_any_element())
            }
            AppearanceSheet::RestoreConfirm => {
                Some(self.render_restore_confirm(cx).into_any_element())
            }
        }
    }

    fn render_import_preview(
        &mut self,
        preview: GhosttyImportPreview,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let can_import = preview.plan.as_ref().is_some_and(plan_has_changes);
        let cancel = button(
            "cancel-ghostty-import",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| {
            this.appearance_ui.dismiss_sheet();
            cx.notify();
        }))
        .into_any_element();
        let confirm = if can_import {
            button(
                "confirm-ghostty-import",
                i18n.text(k::APPEARANCE_CONFIG_IMPORT_ACTION),
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, window, cx| this.confirm_ghostty_import(window, cx)))
            .into_any_element()
        } else {
            disabled_button(
                "confirm-ghostty-import",
                i18n.text(k::APPEARANCE_CONFIG_IMPORT_ACTION),
                ButtonTone::Primary,
                ButtonSize::Sm,
                true,
            )
            .into_any_element()
        };
        let mut body = modal_body()
            .id("ghostty-import-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if let Some(path) = preview.source.as_deref() {
            body = body.child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme::muted())
                    .whitespace_normal()
                    .child(crate::tf!(
                        i18n,
                        k::APPEARANCE_CONFIG_IMPORT_SOURCE,
                        path = path
                    )),
            );
        } else {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(theme::text())
                    .child(i18n.text(k::APPEARANCE_CONFIG_IMPORT_EMPTY)),
            );
        }
        if let Some(error) = preview.error.as_deref() {
            body = body.child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme::yellow())
                    .whitespace_normal()
                    .child(error.to_owned()),
            );
        }
        if !preview.ghostty_app_found {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(theme::yellow())
                    .child(i18n.text(k::APPEARANCE_CONFIG_IMPORT_MISSING_APP)),
            );
        }
        if !preview.recognized.is_empty() {
            body = body
                .child(section_header(
                    i18n.text(k::APPEARANCE_CONFIG_IMPORT_RECOGNIZED),
                    None,
                ))
                .child(group(
                    preview
                        .recognized
                        .iter()
                        .map(|(key, value)| {
                            row()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_color(theme::text())
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .whitespace_normal()
                                                .child(key.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme::muted())
                                                .whitespace_normal()
                                                .child(value.clone()),
                                        ),
                                )
                                .into_any_element()
                        })
                        .collect(),
                ));
        }
        if !preview.unknown.is_empty() {
            body = body
                .child(section_header(
                    i18n.text(k::APPEARANCE_CONFIG_IMPORT_UNKNOWN),
                    None,
                ))
                .child(div().flex().flex_col().gap_1().children(
                    preview.unknown.iter().cloned().map(|key| {
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme::muted())
                            .whitespace_normal()
                            .child(key)
                    }),
                ));
        }
        modal_overlay(
            apply_dialog(
                modal_card().w(px(520.)).max_h(relative(0.84)),
                "ghostty-import-dialog",
                i18n.text(k::APPEARANCE_CONFIG_IMPORT_TITLE),
            )
            .child(modal_header(i18n.text(k::APPEARANCE_CONFIG_IMPORT_TITLE)).flex_none())
            .child(body)
            .child(modal_footer(vec![cancel, confirm]).flex_none()),
        )
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_appearance_sheet_key(event, window, cx);
        }))
    }

    fn render_restore_confirm(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let cancel = button(
            "cancel-restore-appearance",
            i18n.text(k::COMMON_CANCEL),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| {
            this.appearance_ui.dismiss_sheet();
            cx.notify();
        }))
        .into_any_element();
        let confirm = button(
            "confirm-restore-appearance",
            i18n.text(k::APPEARANCE_CONFIG_RESTORE_ACTION),
            ButtonTone::Danger,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.appearance_ui.sheet = AppearanceSheet::None;
            this.restore_appearance_defaults(window, cx);
        }))
        .into_any_element();
        modal_overlay(
            apply_dialog(
                modal_card(),
                "restore-appearance-dialog",
                i18n.text(k::APPEARANCE_CONFIG_RESTORE_TITLE),
            )
            .child(modal_header(i18n.text(k::APPEARANCE_CONFIG_RESTORE_TITLE)))
            .child(
                modal_body().child(
                    div()
                        .text_sm()
                        .text_color(theme::text())
                        .child(i18n.text(k::APPEARANCE_CONFIG_RESTORE_BODY)),
                ),
            )
            .child(modal_footer(vec![cancel, confirm])),
        )
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_appearance_sheet_key(event, window, cx);
        }))
    }

    fn handle_appearance_sheet_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.is_held || event.keystroke.modifiers.modified() {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.appearance_ui.dismiss_sheet();
                cx.stop_propagation();
                cx.notify();
            }
            "enter" | "return" => {
                cx.stop_propagation();
                self.confirm_appearance_sheet(window, cx);
            }
            _ => {}
        }
    }

    fn confirm_appearance_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.appearance_ui.sheet.clone() {
            AppearanceSheet::ImportPreview(_) => self.confirm_ghostty_import(window, cx),
            AppearanceSheet::RestoreConfirm => {
                self.appearance_ui.sheet = AppearanceSheet::None;
                self.restore_appearance_defaults(window, cx);
            }
            AppearanceSheet::None => {}
        }
    }

    fn request_ghostty_import(&mut self, cx: &mut Context<Self>) {
        let preview = match GhosttyImportPaths::user() {
            Some(paths) => build_import_preview(&paths),
            None => GhosttyImportPreview {
                source: None,
                ghostty_app_found: Path::new(GHOSTTY_APP_THEMES).is_dir(),
                recognized: Vec::new(),
                unknown: Vec::new(),
                plan: None,
                error: Some(self.i18n.text(k::APPEARANCE_CONFIG_OPEN_FAILED).into()),
            },
        };
        self.appearance_ui.sheet = AppearanceSheet::ImportPreview(Box::new(preview));
        cx.notify();
    }

    fn confirm_ghostty_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let AppearanceSheet::ImportPreview(preview) =
            std::mem::replace(&mut self.appearance_ui.sheet, AppearanceSheet::None)
        else {
            return;
        };
        let Some(plan) = preview.plan else {
            cx.notify();
            return;
        };
        let Some(paths) = crate::config::AppPaths::user() else {
            self.appearance_ui.status = Some(
                self.i18n
                    .text(k::APPEARANCE_CONFIG_IMPORT_FAILED_PATHS)
                    .into(),
            );
            cx.notify();
            return;
        };
        match crate::config::apply_ghostty_import_plan(&mut self.config, &plan, &paths.themes()) {
            Ok(()) => {
                theme::reload_registry();
                self.sync_appearance_from_document();
                self.apply_appearance(window, cx);
                self.appearance_ui.status =
                    Some(self.i18n.text(k::APPEARANCE_CONFIG_IMPORT_APPLIED).into());
            }
            Err(error) => {
                self.appearance_ui.status = Some(
                    crate::tf!(
                        self.i18n,
                        k::APPEARANCE_CONFIG_IMPORT_FAILED,
                        error = error.to_string()
                    )
                    .into(),
                );
                cx.notify();
            }
        }
    }

    fn open_ocherdr_config(&mut self, cx: &mut Context<Self>) {
        let Some(path) = crate::config::AppPaths::user().map(|paths| paths.config()) else {
            self.appearance_ui.status =
                Some(self.i18n.text(k::APPEARANCE_CONFIG_OPEN_FAILED).into());
            cx.notify();
            return;
        };
        if !path.is_file() {
            self.appearance_ui.status =
                Some(self.i18n.text(k::APPEARANCE_CONFIG_OPEN_MISSING).into());
            cx.notify();
            return;
        }
        let opened = open_path_with_system(&path);
        self.appearance_ui.status = if opened {
            None
        } else {
            Some(self.i18n.text(k::APPEARANCE_CONFIG_OPEN_FAILED).into())
        };
        cx.notify();
    }

    fn restore_appearance_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::config::strip_known_keys(&mut self.config);
        self.sync_appearance_from_document();
        self.appearance_ui = AppearanceUi::default();
        self.open_select = None;
        self.set_language(Language::System, cx);
        self.apply_appearance(window, cx);
    }

    fn sync_appearance_from_document(&mut self) {
        let (config, _) = crate::config::values::AppConfig::from_document(&self.config);
        self.appearance = crate::config::values::appearance_from_config(&config);
        self.agent_notifications = config.agent_notifications;
        self.pane_edge_relocation = config.pane_edge_relocation;
        if self.i18n.preference() != config.language {
            self.i18n.set_preference(config.language);
        }
    }

    fn select_row_state(&self, id: &str) -> SelectRowState {
        SelectRowState::new(false, self.open_select.as_deref() == Some(id))
    }
}

fn open_path_with_system(path: &std::path::Path) -> bool {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    command
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

mod support;
use support::*;

#[cfg(test)]
mod tests;
