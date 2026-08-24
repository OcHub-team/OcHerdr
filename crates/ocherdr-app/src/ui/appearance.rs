use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::*;
use crate::a11y::apply_dialog;
use crate::i18n::Key;
use ochub_ui::layout::{
    self, SelectRowEvent, SelectRowState, action_row, content_column, group, page_header, row,
    row_label, scroll_body, section_header, select_row, switch_row,
};

/// UI-only appearance state that is not on [`AppearanceSettings`] yet.
///
/// T29-A owns config persistence; T29-B owns `Theme.ansi`. This struct holds
/// the controls those APIs will feed, so the page can be operated before they
/// merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppearanceUi {
    /// `None` follows the UI theme.
    ///
    /// TODO(T29-A): persist as `terminal-theme` (`id` or `light:X,dark:Y`).
    terminal_theme: Option<String>,
    /// TODO(T29-A): persist as `window-padding-x`.
    padding_x: u32,
    /// TODO(T29-A): persist as `window-padding-y`.
    padding_y: u32,
    /// TODO(T29-A): persist as `font-thicken-strength`.
    thicken_strength: u8,
    /// TODO(T29-A): persist as repeated `palette = N=#rrggbb`.
    palette_overrides: [Option<u32>; 16],
    selected_slot: Option<u8>,
    sheet: AppearanceSheet,
    status: Option<SharedString>,
}

impl Default for AppearanceUi {
    fn default() -> Self {
        Self {
            terminal_theme: None,
            padding_x: 0,
            padding_y: 0,
            thicken_strength: ThickenStrengthChoice::default().value(),
            palette_overrides: [None; 16],
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
    ImportPreview(GhosttyImportPreview),
    RestoreConfirm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GhosttyImportPreview {
    source: Option<String>,
    ghostty_app_found: bool,
    recognized: Vec<(String, String)>,
    unknown: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ThickenStrengthChoice {
    Subtle,
    Medium,
    Strong,
    #[default]
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

    fn index(self) -> usize {
        match self {
            Self::Subtle => 0,
            Self::Medium => 1,
            Self::Strong => 2,
            Self::Max => 3,
        }
    }

    fn nearest(value: u8) -> Self {
        let mut best = Self::Max;
        let mut best_diff = best.value().abs_diff(value);
        for choice in Self::ALL {
            let diff = choice.value().abs_diff(value);
            if diff < best_diff {
                best = choice;
                best_diff = diff;
            }
        }
        best
    }
}

const PADDING_CHOICES: [u32; 6] = [0, 4, 8, 12, 16, 24];
const PADDING_LABELS: [&str; 6] = ["0", "4", "8", "12", "16", "24"];
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
            layout::page(),
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
            .child(group(vec![self.language_row(cx).into_any_element()]))
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
                self.ligatures_row(cx).into_any_element(),
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
        let selected = terminal_theme_index(self.appearance_ui.terminal_theme.as_deref(), &ids);
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
                move |this, index, _, cx| {
                    this.appearance_ui.terminal_theme = ids.get(index).cloned().flatten();
                    cx.notify();
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
        // TODO(T29-A): `background-opacity` becomes a free 0..1 f64.
        let i18n = self.i18n;
        select_row(
            "appearance-opacity",
            i18n.text(k::APPEARANCE_OPACITY_LABEL),
            Some(i18n.text(k::APPEARANCE_OPACITY_DESCRIPTION).into()),
            &["100%", "92%", "84%", "72%"],
            self.appearance.background_opacity.index(),
            self.select_row_state("appearance-opacity"),
            appearance_select(cx, "appearance-opacity", |this, index, window, cx| {
                let Some(&opacity) = OpacityChoice::ALL.get(index) else {
                    return;
                };
                this.set_background_opacity(opacity, window, cx);
            }),
        )
    }

    fn padding_row(&mut self, cx: &mut Context<Self>, horizontal: bool) -> impl IntoElement {
        let i18n = self.i18n;
        let (id, label, description, selected) = if horizontal {
            (
                "appearance-padding-x",
                k::APPEARANCE_WINDOW_PADDING_X_LABEL,
                k::APPEARANCE_WINDOW_PADDING_X_DESCRIPTION,
                padding_index(self.appearance_ui.padding_x),
            )
        } else {
            (
                "appearance-padding-y",
                k::APPEARANCE_WINDOW_PADDING_Y_LABEL,
                k::APPEARANCE_WINDOW_PADDING_Y_DESCRIPTION,
                padding_index(self.appearance_ui.padding_y),
            )
        };
        select_row(
            id,
            i18n.text(label),
            Some(i18n.text(description).into()),
            &PADDING_LABELS,
            selected,
            self.select_row_state(id),
            appearance_select(cx, id, move |this, index, _, cx| {
                let Some(&value) = PADDING_CHOICES.get(index) else {
                    return;
                };
                if horizontal {
                    this.appearance_ui.padding_x = value;
                } else {
                    this.appearance_ui.padding_y = value;
                }
                cx.notify();
            }),
        )
    }

    fn font_size_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // TODO(T29-A): `font-size` becomes a free f32; keep the enum until then.
        let i18n = self.i18n;
        let size_labels = FontSizeChoice::ALL.map(|size| size.value().to_string());
        let size_refs = size_labels.each_ref().map(String::as_str);
        select_row(
            "appearance-font-size",
            i18n.text(k::APPEARANCE_FONT_SIZE_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_SIZE_DESCRIPTION).into()),
            &size_refs,
            self.appearance.font.size.index(),
            self.select_row_state("appearance-font-size"),
            appearance_select(cx, "appearance-font-size", |this, index, window, cx| {
                let Some(&size) = FontSizeChoice::ALL.get(index) else {
                    return;
                };
                this.set_font_size(size, window, cx);
            }),
        )
    }

    fn ligatures_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // TODO(T29-A): replace this bool with repeated `font-feature` values.
        let i18n = self.i18n;
        let ligatures_listener = cx.listener(|this, _: &(), window, cx| {
            this.set_font_ligatures(!this.appearance.font.ligatures, window, cx);
        });
        switch_row(
            "appearance-ligatures",
            i18n.text(k::APPEARANCE_FONT_LIGATURES_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_LIGATURES_DESCRIPTION).into()),
            self.appearance.font.ligatures,
            false,
            move |window, cx| ligatures_listener(&(), window, cx),
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
        select_row(
            "appearance-thicken-strength",
            i18n.text(k::APPEARANCE_FONT_THICKEN_STRENGTH_LABEL),
            Some(
                i18n.text(k::APPEARANCE_FONT_THICKEN_STRENGTH_DESCRIPTION)
                    .into(),
            ),
            &[
                i18n.text(k::APPEARANCE_FONT_THICKEN_STRENGTH_SUBTLE),
                i18n.text(k::APPEARANCE_FONT_THICKEN_STRENGTH_MEDIUM),
                i18n.text(k::APPEARANCE_FONT_THICKEN_STRENGTH_STRONG),
                i18n.text(k::APPEARANCE_FONT_THICKEN_STRENGTH_MAX),
            ],
            ThickenStrengthChoice::nearest(self.appearance_ui.thicken_strength).index(),
            SelectRowState::new(
                !self.appearance.font.thicken,
                self.open_select.as_deref() == Some("appearance-thicken-strength"),
            ),
            appearance_select(cx, "appearance-thicken-strength", |this, index, _, cx| {
                let Some(&choice) = ThickenStrengthChoice::ALL.get(index) else {
                    return;
                };
                this.appearance_ui.thicken_strength = choice.value();
                cx.notify();
            }),
        )
    }

    fn cell_width_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        select_row(
            "appearance-cell-width",
            i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_DESCRIPTION).into()),
            &[
                i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_TIGHT),
                i18n.text(k::COMMON_DEFAULT),
                i18n.text(k::APPEARANCE_FONT_CELL_WIDTH_WIDE),
            ],
            self.appearance.font.cell_width_percent.index(),
            self.select_row_state("appearance-cell-width"),
            appearance_select(cx, "appearance-cell-width", |this, index, window, cx| {
                let Some(&percent) = CellWidthChoice::ALL.get(index) else {
                    return;
                };
                this.set_cell_width(percent, window, cx);
            }),
        )
    }

    fn cell_height_row(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        select_row(
            "appearance-cell-height",
            i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_LABEL),
            Some(i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_DESCRIPTION).into()),
            &[
                i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_COMPACT),
                i18n.text(k::COMMON_DEFAULT),
                i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_RELAXED),
                i18n.text(k::APPEARANCE_FONT_CELL_HEIGHT_LOOSE),
            ],
            self.appearance.font.cell_height_percent.index(),
            self.select_row_state("appearance-cell-height"),
            appearance_select(cx, "appearance-cell-height", |this, index, window, cx| {
                let Some(&percent) = CellHeightChoice::ALL.get(index) else {
                    return;
                };
                this.set_cell_height(percent, window, cx);
            }),
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
        let colors = displayed_ansi(placeholder_ansi(), &self.appearance_ui.palette_overrides);
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
        let colors = displayed_ansi(placeholder_ansi(), &self.appearance_ui.palette_overrides);
        let color = colors[index];
        let overridden = self.appearance_ui.palette_overrides[index].is_some();
        let hex = theme::ThemeColor::new(color).hex();
        let reset = cx.listener(move |this, _: &(), _window, cx| {
            clear_slot_override(&mut this.appearance_ui.palette_overrides, slot);
            cx.notify();
        });
        let presets = PALETTE_PRESETS.into_iter().map(|preset| {
            let selected = self.appearance_ui.palette_overrides[index] == Some(preset);
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
                .on_click(cx.listener(move |this, _, _window, cx| {
                    set_slot_override(&mut this.appearance_ui.palette_overrides, slot, preset);
                    cx.notify();
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
                Some(self.render_import_preview(preview, cx).into_any_element())
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
        let can_import = preview.source.is_some();
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
            .on_click(cx.listener(|this, _, _window, cx| this.confirm_ghostty_import(cx)))
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
        let mut body = modal_body();
        if let Some(path) = preview.source.as_deref() {
            body = body.child(div().text_xs().text_color(theme::muted()).child(crate::tf!(
                i18n,
                k::APPEARANCE_CONFIG_IMPORT_SOURCE,
                path = path
            )));
        } else {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(theme::text())
                    .child(i18n.text(k::APPEARANCE_CONFIG_IMPORT_EMPTY)),
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
                                .child(row_label(key.clone(), Some(value.clone().into())))
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
                .child(
                    div().flex().flex_col().gap_1().children(
                        preview
                            .unknown
                            .iter()
                            .cloned()
                            .map(|key| div().text_xs().text_color(theme::muted()).child(key)),
                    ),
                );
        }
        modal_overlay(
            apply_dialog(
                modal_card().w(px(520.)),
                "ghostty-import-dialog",
                i18n.text(k::APPEARANCE_CONFIG_IMPORT_TITLE),
            )
            .child(modal_header(i18n.text(k::APPEARANCE_CONFIG_IMPORT_TITLE)))
            .child(body)
            .child(modal_footer(vec![cancel, confirm])),
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
            AppearanceSheet::ImportPreview(_) => self.confirm_ghostty_import(cx),
            AppearanceSheet::RestoreConfirm => {
                self.appearance_ui.sheet = AppearanceSheet::None;
                self.restore_appearance_defaults(window, cx);
            }
            AppearanceSheet::None => {}
        }
    }

    fn request_ghostty_import(&mut self, cx: &mut Context<Self>) {
        let sources =
            collect_import_sources(&ghostty_config_candidates(), Path::new(GHOSTTY_APP_THEMES));
        self.appearance_ui.sheet =
            AppearanceSheet::ImportPreview(placeholder_import_preview(&sources));
        cx.notify();
    }

    fn confirm_ghostty_import(&mut self, cx: &mut Context<Self>) {
        // TODO(T29-A): `ConfigStore::import_ghostty(preview)` should parse the
        // Ghostty config with the shared key=value parser, map §3.1 keys, copy
        // `theme = X` files from Ghostty.app into `themes/imported-<slug>.json`,
        // and set `terminal-theme`. Do not parse or write config here.
        self.appearance_ui.sheet = AppearanceSheet::None;
        self.appearance_ui.status =
            Some(self.i18n.text(k::APPEARANCE_CONFIG_IMPORT_DEFERRED).into());
        cx.notify();
    }

    fn open_ocherdr_config(&mut self, cx: &mut Context<Self>) {
        // TODO(T29-A): share the canonical `config` path with the file writer.
        let Some(path) = ocherdr_config_path() else {
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
        let opened = Command::new("open")
            .arg(&path)
            .status()
            .is_ok_and(|status| status.success());
        self.appearance_ui.status = if opened {
            None
        } else {
            Some(self.i18n.text(k::APPEARANCE_CONFIG_OPEN_FAILED).into())
        };
        cx.notify();
    }

    fn restore_appearance_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        restore_appearance_state(&mut self.appearance, &mut self.appearance_ui);
        self.open_select = None;
        self.set_language(Language::System, cx);
        self.apply_appearance(window, cx);
    }

    fn select_row_state(&self, id: &str) -> SelectRowState {
        SelectRowState::new(false, self.open_select.as_deref() == Some(id))
    }
}

fn appearance_select(
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

fn apply_select_event(
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

fn padding_index(value: u32) -> usize {
    PADDING_CHOICES
        .iter()
        .position(|&choice| choice == value)
        .unwrap_or(0)
}

fn terminal_theme_index(selected: Option<&str>, ids: &[Option<String>]) -> usize {
    match selected {
        None => 0,
        Some(id) => ids
            .iter()
            .position(|entry| entry.as_deref() == Some(id))
            .unwrap_or(0),
    }
}

fn palette_slot_label(i18n: I18n, index: usize) -> String {
    crate::tf!(
        i18n,
        k::APPEARANCE_PALETTE_SLOT,
        name = i18n.text(PALETTE_SLOT_KEYS[index]),
        index = index
    )
}

/// Display-only 16-color grid until T29-B exposes `Theme.ansi`.
///
/// TODO(T29-B): replace with `family.dark/light.ansi` (falling back to
/// `ansi_from_theme` when `ansi` is `None`). Do not keep a second copy of the
/// OcHub/Ember tables here.
fn placeholder_ansi() -> [u32; 16] {
    let theme = theme::current();
    [
        theme.bg.0,
        theme.red.0,
        theme.green.0,
        theme.yellow.0,
        theme.accent.0,
        theme.mauve.0,
        theme.teal.0,
        theme.text.0,
        theme.overlay.0,
        theme.red_hover.0,
        theme.green.0,
        theme.peach.0,
        theme.accent_hover.0,
        theme.mauve.0,
        theme.teal.0,
        theme.subtext.0,
    ]
}

fn displayed_ansi(base: [u32; 16], overrides: &[Option<u32>; 16]) -> [u32; 16] {
    let mut colors = base;
    for (index, override_color) in overrides.iter().enumerate() {
        if let Some(color) = override_color {
            colors[index] = *color;
        }
    }
    colors
}

fn set_slot_override(overrides: &mut [Option<u32>; 16], slot: u8, color: u32) {
    overrides[slot as usize] = Some(color);
}

fn clear_slot_override(overrides: &mut [Option<u32>; 16], slot: u8) {
    overrides[slot as usize] = None;
}

fn restore_appearance_state(appearance: &mut AppearanceSettings, ui: &mut AppearanceUi) {
    *appearance = AppearanceSettings::default();
    *ui = AppearanceUi::default();
}

struct ImportSources {
    config: Option<PathBuf>,
    ghostty_app_found: bool,
}

fn ghostty_config_candidates() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    vec![
        home.join(".config/ghostty/config"),
        home.join("Library/Application Support/com.mitchellh.ghostty/config"),
    ]
}

fn ocherdr_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("OcHerdr/config"))
}

fn collect_import_sources(candidates: &[PathBuf], themes_dir: &Path) -> ImportSources {
    ImportSources {
        config: candidates.iter().find(|path| path.is_file()).cloned(),
        ghostty_app_found: themes_dir.is_dir(),
    }
}

/// Placeholder preview. Does **not** read the Ghostty file.
///
/// TODO(T29-A): `fn preview_ghostty_import(path, themes_dir) -> Result<Preview, _>`
/// should parse with the shared config parser, map §3.1 keys, and list unknown
/// keys instead of this canned list.
fn placeholder_import_preview(sources: &ImportSources) -> GhosttyImportPreview {
    let source = sources
        .config
        .as_ref()
        .map(|path| path.display().to_string());
    if source.is_none() {
        return GhosttyImportPreview {
            source,
            ghostty_app_found: sources.ghostty_app_found,
            recognized: Vec::new(),
            unknown: Vec::new(),
        };
    }
    GhosttyImportPreview {
        source,
        ghostty_app_found: sources.ghostty_app_found,
        recognized: vec![
            ("font-family".into(), "JetBrains Mono".into()),
            ("font-size".into(), "13".into()),
            ("background-opacity".into(), "0.92".into()),
            ("window-padding-x".into(), "0".into()),
            ("palette".into(), "1=#c41a16".into()),
        ],
        unknown: vec!["keybind".into(), "shell-integration".into()],
    }
}

#[cfg(test)]
mod tests {
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
    fn palette_override_replaces_only_that_slot() {
        let base = [0x111111; 16];
        let mut overrides = [None; 16];
        set_slot_override(&mut overrides, 3, 0xC7A000);
        let colors = displayed_ansi(base, &overrides);
        assert_eq!(colors[3], 0xC7A000);
        assert!(
            colors
                .iter()
                .enumerate()
                .all(|(index, color)| index == 3 || *color == 0x111111)
        );
        clear_slot_override(&mut overrides, 3);
        assert_eq!(displayed_ansi(base, &overrides), base);
    }

    #[test]
    fn restoring_defaults_clears_theme_choice_and_placeholder_overrides() {
        let mut appearance = AppearanceSettings {
            theme_family: "ember".into(),
            mode: AppearanceMode::Light,
            backdrop: BackdropMode::Opaque,
            ..AppearanceSettings::default()
        };
        let mut ui = AppearanceUi {
            terminal_theme: Some("ember".into()),
            padding_x: 12,
            selected_slot: Some(1),
            ..AppearanceUi::default()
        };
        set_slot_override(&mut ui.palette_overrides, 1, 0xFF0000);
        restore_appearance_state(&mut appearance, &mut ui);
        let defaults = AppearanceSettings::default();
        assert_eq!(appearance.theme_family, defaults.theme_family);
        assert_eq!(appearance.mode, defaults.mode);
        assert_eq!(appearance.backdrop, defaults.backdrop);
        assert_eq!(appearance.font, defaults.font);
        assert_eq!(ui, AppearanceUi::default());
    }

    #[test]
    fn import_sources_prefer_the_first_existing_ghostty_config() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let found = dir.path().join("config");
        let themes = dir.path().join("themes");
        std::fs::write(&found, "font-size = 13\n").unwrap();
        std::fs::create_dir(&themes).unwrap();
        assert!(
            found.is_file(),
            "fixture file must exist before we trust the finder"
        );
        assert!(
            themes.is_dir(),
            "fixture themes dir must exist before we trust the finder"
        );
        assert!(!missing.exists());
        let sources = collect_import_sources(&[missing.clone(), found.clone()], &themes);
        assert_eq!(sources.config.as_deref(), Some(found.as_path()));
        assert!(sources.ghostty_app_found);
        let empty = collect_import_sources(&[missing], &dir.path().join("no-app"));
        assert_eq!(empty.config, None);
        assert!(!empty.ghostty_app_found);
    }

    #[test]
    fn placeholder_import_preview_lists_unknown_keys_when_a_config_exists() {
        let preview = placeholder_import_preview(&ImportSources {
            config: Some(PathBuf::from("/tmp/ghostty-config")),
            ghostty_app_found: false,
        });
        assert_eq!(preview.source.as_deref(), Some("/tmp/ghostty-config"));
        assert!(!preview.ghostty_app_found);
        assert!(!preview.recognized.is_empty());
        assert!(
            preview
                .unknown
                .iter()
                .any(|key| key == "keybind" || key == "shell-integration"),
            "unknown keys must stay visible so they are not silently dropped"
        );
        let recognized: Vec<&str> = preview
            .recognized
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        for key in &preview.unknown {
            assert!(
                !recognized.contains(&key.as_str()),
                "unknown key `{key}` must not also appear as recognized"
            );
        }
    }

    #[test]
    fn placeholder_import_preview_is_empty_when_no_config_exists() {
        let preview = placeholder_import_preview(&ImportSources {
            config: None,
            ghostty_app_found: true,
        });
        assert_eq!(preview.source, None);
        assert!(preview.ghostty_app_found);
        assert!(preview.recognized.is_empty());
        assert!(preview.unknown.is_empty());
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
}
