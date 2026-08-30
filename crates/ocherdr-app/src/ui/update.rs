use super::super::*;
use crate::a11y::apply_dialog;
use ochub_ui::gpui::AnyElement;

impl OcHerdrView {
    pub(super) fn render_update_dialog(
        &mut self,
        state: &UpdateDialog,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let later = || {
            button(
                "update-later",
                i18n.text(k::UPDATE_ACTION_LATER),
                ButtonTone::Neutral,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, window, cx| this.close_update_dialog(window, cx)))
            .into_any_element()
        };

        let (body, footer): (AnyElement, Vec<AnyElement>) = match state {
            UpdateDialog::Checking => (
                update_copy(i18n.text(k::UPDATE_CHECKING), None),
                vec![
                    later(),
                    busy_button(
                        "update-checking",
                        i18n.text(k::UPDATE_CHECKING),
                        ButtonTone::Primary,
                        ButtonSize::Sm,
                        true,
                    )
                    .into_any_element(),
                ],
            ),
            UpdateDialog::Current { version } => {
                let close = button(
                    "update-close-current",
                    i18n.text(k::COMMON_CLOSE),
                    ButtonTone::Primary,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, window, cx| this.close_update_dialog(window, cx)))
                .into_any_element();
                (
                    update_copy(
                        crate::tf!(i18n, k::UPDATE_CURRENT, current = version),
                        Some(i18n.text(k::UPDATE_SECURITY).to_owned()),
                    ),
                    vec![close],
                )
            }
            UpdateDialog::Available(info) => {
                let latest = info.latest_version.as_deref().unwrap_or_default();
                let mut content = div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_sm().text_color(theme::text()).child(crate::tf!(
                        i18n,
                        k::UPDATE_AVAILABLE,
                        latest = latest
                    )))
                    .child(div().text_xs().text_color(theme::muted()).child(crate::tf!(
                        i18n,
                        k::UPDATE_AVAILABLE_DETAIL,
                        current = &info.current_version
                    )))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(i18n.text(k::UPDATE_SECURITY)),
                    );
                if let Some(notes) = info
                    .release_notes
                    .as_deref()
                    .filter(|notes| !notes.trim().is_empty())
                {
                    content = content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(i18n.text(k::UPDATE_NOTES)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child(release_notes_excerpt(notes)),
                            ),
                    );
                }

                let release_url = info.release_url.clone();
                let release = button(
                    "update-open-release",
                    i18n.text(k::UPDATE_ACTION_OPEN_RELEASE),
                    ButtonTone::Neutral,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.open_update_release_page(release_url.clone(), cx)
                }))
                .into_any_element();
                let mut actions = vec![later(), release];
                if info.can_self_install {
                    let info = info.clone();
                    actions.push(
                        button(
                            "update-install",
                            i18n.text(k::UPDATE_ACTION_INSTALL),
                            ButtonTone::Primary,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.install_update(info.clone(), cx)
                        }))
                        .into_any_element(),
                    );
                }
                (content.into_any_element(), actions)
            }
            UpdateDialog::Downloading {
                version,
                downloaded,
                total,
            } => {
                let progress = match total {
                    Some(total) => crate::tf!(
                        i18n,
                        k::UPDATE_PROGRESS,
                        downloaded = mebibytes(*downloaded),
                        total = mebibytes(*total)
                    ),
                    None => crate::tf!(
                        i18n,
                        k::UPDATE_PROGRESS_UNKNOWN,
                        downloaded = mebibytes(*downloaded)
                    ),
                };
                (
                    update_copy(
                        crate::tf!(i18n, k::UPDATE_INSTALLING, version = version),
                        Some(progress),
                    ),
                    vec![
                        busy_button(
                            "update-installing",
                            i18n.text(k::UPDATE_ACTION_INSTALL),
                            ButtonTone::Primary,
                            ButtonSize::Sm,
                            true,
                        )
                        .into_any_element(),
                    ],
                )
            }
            UpdateDialog::Failed {
                message,
                release_url,
            } => {
                let release_url = release_url.clone();
                let release = button(
                    "update-failed-open-release",
                    i18n.text(k::UPDATE_ACTION_OPEN_RELEASE),
                    ButtonTone::Neutral,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.open_update_release_page(release_url.clone(), cx)
                }))
                .into_any_element();
                let retry = button(
                    "update-retry",
                    i18n.text(k::UPDATE_ACTION_RETRY),
                    ButtonTone::Primary,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.confirm_update_dialog(cx)))
                .into_any_element();
                (
                    update_copy(i18n.text(k::UPDATE_FAILED), Some(message.clone())),
                    vec![later(), release, retry],
                )
            }
            UpdateDialog::Installed { version } => (
                update_copy(
                    crate::tf!(i18n, k::UPDATE_INSTALLED, version = version),
                    None,
                ),
                vec![
                    busy_button(
                        "update-restarting",
                        i18n.text(k::UPDATE_ACTION_INSTALL),
                        ButtonTone::Primary,
                        ButtonSize::Sm,
                        true,
                    )
                    .into_any_element(),
                ],
            ),
        };

        modal_overlay(
            apply_dialog(
                modal_card().w(px(540.)),
                "software-update-dialog",
                i18n.text(k::UPDATE_TITLE),
            )
            .child(modal_header(i18n.text(k::UPDATE_TITLE)))
            .child(modal_body().child(body))
            .child(modal_footer(footer)),
        )
        .top_0()
        .left_0()
        .track_focus(&self.dialog_focus)
        .on_key_down(cx.listener(|this, event, window, cx| {
            this.handle_overlay_key(event, window, cx);
        }))
    }
}

fn update_copy(primary: impl Into<SharedString>, secondary: Option<String>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .text_color(theme::text())
                .child(primary.into()),
        )
        .children(secondary.map(|text| div().text_xs().text_color(theme::muted()).child(text)))
        .into_any_element()
}

fn mebibytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024. * 1024.))
}

fn release_notes_excerpt(notes: &str) -> String {
    const LIMIT: usize = 1_200;
    let mut characters = notes.trim().chars();
    let excerpt = characters.by_ref().take(LIMIT).collect::<String>();
    if characters.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_notes_are_bounded_without_splitting_unicode() {
        let notes = "更".repeat(1_201);
        let excerpt = release_notes_excerpt(&notes);
        assert_eq!(excerpt.chars().count(), 1_201);
        assert!(excerpt.ends_with('…'));
    }
}
