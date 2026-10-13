use super::super::*;
use crate::shortcuts::{COMMANDS, Command};
use ochub_ui::{modal, shortcuts::help_row};

impl OcHerdrView {
    pub(crate) fn open_shortcut_help(&mut self, cx: &mut Context<Self>) {
        // A help invocation must not replace a form with unsaved input.
        if !matches!(self.overlay, Overlay::None | Overlay::ShortcutHelp) {
            return;
        }
        self.shortcut_help_search.update(cx, |input, cx| {
            input.set_content("", cx);
            input.set_placeholder(self.i18n.text(k::SHORTCUTS_SEARCH), cx);
        });
        self.prefix_pending = false;
        self.set_overlay(Overlay::ShortcutHelp, cx);
    }

    pub(super) fn render_shortcut_help(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let query = self
            .shortcut_help_search
            .read(cx)
            .content()
            .trim()
            .to_lowercase();
        let mut entries = Vec::new();
        for (command, shortcut) in COMMANDS {
            let key = match command {
                Command::NewTab => k::SHORTCUTS_NEW_TAB,
                Command::ClosePane => k::SHORTCUTS_CLOSE_PANE,
                Command::CloseWorkspace => k::SHORTCUTS_CLOSE_WORKSPACE,
                Command::NewWorkspace => k::SHORTCUTS_NEW_WORKSPACE,
                Command::ToggleFiles => k::SHORTCUTS_FILES,
                Command::FileAddress => k::SHORTCUTS_ADDRESS,
                Command::Appearance => k::SHORTCUTS_APPEARANCE,
                Command::Copy => k::SHORTCUTS_COPY,
                Command::SelectAll => k::SHORTCUTS_SELECT_ALL,
                Command::PreviousTab => k::SHORTCUTS_PREVIOUS,
                Command::NextTab => k::SHORTCUTS_NEXT,
                Command::Rename => k::SHORTCUTS_RENAME,
                Command::Help => k::SHORTCUTS_HELP,
            };
            entries.push((i18n.text(key), shortcut.label(), shortcut.stroke()));
        }
        for (key, keys) in [
            (
                k::SHORTCUTS_NUMBERED_TAB,
                if cfg!(target_os = "macos") {
                    "⌘1–9"
                } else {
                    "Ctrl+1–9"
                },
            ),
            (k::SHORTCUTS_PREFIX_NEW, "Ctrl+B → C"),
            (k::SHORTCUTS_PREFIX_WORKSPACE, "Ctrl+B → Shift+N"),
            (k::SHORTCUTS_PREFIX_NEXT, "Ctrl+B → N / P"),
            (k::SHORTCUTS_PREFIX_NUMBER, "Ctrl+B → 1–9"),
            (k::SHORTCUTS_PREFIX_RENAME_TAB, "Ctrl+B → Shift+T"),
            (k::SHORTCUTS_PREFIX_RENAME_WORKSPACE, "Ctrl+B → Shift+W"),
            (k::SHORTCUTS_PREFIX_RENAME_PANE, "Ctrl+B → Shift+P"),
            (k::SHORTCUTS_PREFIX_CLOSE_TAB, "Ctrl+B → Shift+X"),
            (k::SHORTCUTS_PREFIX_CLOSE_WORKSPACE, "Ctrl+B → Shift+D"),
            (k::SHORTCUTS_PREFIX_MOVE_WORKSPACE, "Ctrl+B → Shift+J / K"),
            (k::SHORTCUTS_PREFIX_SETTINGS, "Ctrl+B → S"),
            (k::SHORTCUTS_PREFIX_PANE, "Ctrl+B → H / J / K / L"),
            (k::SHORTCUTS_PREFIX_MOVE, "Ctrl+B → M"),
            (k::SHORTCUTS_PREFIX_CANCEL, "Esc"),
        ] {
            entries.push((i18n.text(key), keys.into(), keys));
        }
        let entries: Vec<_> = entries
            .into_iter()
            .filter(|(label, keys, stroke)| {
                format!("{label} {keys} {stroke}")
                    .to_lowercase()
                    .contains(&query)
            })
            .collect();
        let empty = entries.is_empty();
        let content =
            modal::card(window, px(580.))
                .h(px(
                    (f32::from(window.viewport_size().height) - 32.).clamp(0., 640.)
                ))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(modal_header(i18n.text(k::SHORTCUTS_TITLE)))
                .child(
                    div()
                        .id("shortcut-help-search")
                        .debug_selector(|| "shortcut-help-search".into())
                        .px_5()
                        .pb_3()
                        .child(self.shortcut_help_search.clone()),
                )
                .child(
                    modal::body("shortcut-help-body")
                        .flex_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(i18n.text(k::SHORTCUTS_INTRO)),
                        )
                        .children(entries.into_iter().enumerate().map(
                            |(index, (label, keys, _))| {
                                help_row(("shortcut-help-row", index), label, keys)
                            },
                        ))
                        .when(empty, |body| {
                            body.child(div().py_4().child(i18n.text(k::SHORTCUTS_EMPTY)))
                        })
                        .child(
                            div()
                                .pt_3()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(i18n.text(k::SHORTCUTS_EDITING)),
                        ),
                )
                .child(modal_footer(vec![
                    button(
                        "close-shortcut-help",
                        i18n.text(k::COMMON_CLOSE),
                        ButtonTone::Neutral,
                        ButtonSize::Sm,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.set_overlay(Overlay::None, cx)))
                    .into_any_element(),
                ]));
        modal::overlay(
            "shortcut-help-dialog",
            i18n.text(k::SHORTCUTS_TITLE),
            &self.shortcut_help_focus,
            content,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.set_overlay(Overlay::None, cx)),
        )
        .on_action(cx.listener(|this, _: &modal::Dismiss, _, cx| {
            this.consumed_key_releases
                .record(&Keystroke::parse("escape").expect("Escape is a valid key"));
            this.set_overlay(Overlay::None, cx);
        }))
    }
}
