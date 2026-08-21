use super::super::*;

impl OcHerdrView {
    pub(super) fn render_node_manager(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let query = self.remote_search.read(cx).content().trim().to_lowercase();
        let mut rows = Vec::new();
        for source in [
            ConnectionSource::Current,
            ConnectionSource::Saved,
            ConnectionSource::SshConfig,
        ] {
            let matches = self
                .profiles
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, profile)| {
                    connection_source(profile) == source
                        && profile_matches_search(profile, &query, i18n)
                })
                .collect::<Vec<_>>();
            if matches.is_empty() {
                continue;
            }
            rows.push(remote_group_label(source.label(i18n), matches.len()).into_any_element());
            for (index, profile) in matches {
                let selected = index == self.managed_profile_index;
                let active = index == self.profile_index;
                let display_label = if matches!(profile, ConnectionProfile::Local { .. }) {
                    i18n.text("Local").to_owned()
                } else {
                    profile.label().to_owned()
                };
                let node_icon = if matches!(profile, ConnectionProfile::Local { .. }) {
                    IconName::Desktop
                } else {
                    IconName::Globe
                };
                rows.push(
                    div()
                        .id(("managed-node", index))
                        .role(ochub_ui::gpui::Role::Button)
                        .aria_label(format!(
                            "{} · {}",
                            display_label,
                            profile_endpoint(&profile)
                        ))
                        .flex()
                        .items_center()
                        .gap_3()
                        .min_h(px(54.))
                        .mx_2()
                        .px_3()
                        .py_2()
                        .rounded(px(CORNER_CONTROL))
                        .bg(if selected {
                            theme::selection()
                        } else {
                            theme::surface().alpha(0.)
                        })
                        .hover(|style| style.bg(theme::surface_hover()))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.select_managed_profile(index, cx)
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(30.))
                                .rounded(px(CORNER_CONTROL))
                                .bg(if selected {
                                    theme::accent_soft()
                                } else {
                                    theme::inset()
                                })
                                .child(icon(
                                    node_icon,
                                    if selected {
                                        theme::accent()
                                    } else {
                                        theme::muted()
                                    },
                                    15.,
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme::text())
                                        .child(display_label.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(theme::muted())
                                        .child(profile_endpoint(&profile)),
                                ),
                        )
                        .when(active, |row| {
                            row.child(status_dot(if self.error.is_some() {
                                theme::red()
                            } else if self.operation.is_some() {
                                theme::yellow()
                            } else {
                                theme::green()
                            }))
                        })
                        .into_any_element(),
                );
            }
        }
        if rows.is_empty() {
            rows.push(
                empty_state(
                    IconName::Search,
                    i18n.text("No matching hosts"),
                    i18n.text("Try a host name, SSH alias, or address."),
                    None,
                )
                .into_any_element(),
            );
        }
        let detail = if self.add_remote_open {
            self.render_add_remote(cx).into_any_element()
        } else {
            self.render_remote_detail(cx).into_any_element()
        };
        let card = modal_card()
            .w(px(840.))
            .h(px(580.))
            .rounded(px(CORNER_MODAL))
            .child(
                modal_header(i18n.text("Remote connections")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            icon_button_tone(
                                "add-managed-node",
                                i18n.text("New SSH"),
                                IconName::Add,
                                if self.add_remote_open {
                                    ButtonTone::Primary
                                } else {
                                    ButtonTone::Neutral
                                },
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(|this, _, _window, cx| this.open_add_remote(cx))),
                        )
                        .child(
                            icon_only_button_tone(
                                "close-node-manager",
                                i18n.text("Close"),
                                IconName::Close,
                                ButtonTone::Ghost,
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.close_node_manager(window, cx),
                            )),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(310.))
                            .flex_none()
                            .min_h_0()
                            .border_r_1()
                            .border_color(theme::border())
                            .bg(theme::sidebar_background())
                            .child(div().p_3().child(self.remote_search.clone()))
                            .child(
                                div()
                                    .id("managed-node-scroll")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_scroll()
                                    .pb_3()
                                    .children(rows),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .bg(theme::content_background())
                            .child(detail),
                    ),
            );
        modal_overlay(card).top_0().left_0()
    }

    fn render_remote_detail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let Some(profile) = self.profiles.get(self.managed_profile_index).cloned() else {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .child(i18n.text("Select a connection"));
        };
        let index = self.managed_profile_index;
        let active = index == self.profile_index;
        let saved = connection_source(&profile) == ConnectionSource::Saved;
        let display_label = if matches!(profile, ConnectionProfile::Local { .. }) {
            i18n.text("Local").to_owned()
        } else {
            profile.label().to_owned()
        };
        let source = connection_source(&profile).description(i18n);
        let (identity, herdr_path) = match &profile {
            ConnectionProfile::Local { herdr_path } => {
                (i18n.text("System default").to_owned(), herdr_path.clone())
            }
            ConnectionProfile::Ssh {
                identity_file,
                herdr_path,
                ..
            } => (
                identity_file
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| i18n.text("SSH config or agent").into()),
                herdr_path.clone(),
            ),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .px_6()
                    .pt_6()
                    .pb_5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(44.))
                            .rounded(px(CORNER_PANEL))
                            .bg(theme::accent_soft())
                            .child(icon(
                                if matches!(profile, ConnectionProfile::Local { .. }) {
                                    IconName::Desktop
                                } else {
                                    IconName::Globe
                                },
                                theme::accent(),
                                20.,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(display_label),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .text_color(theme::muted())
                                    .child(profile_endpoint(&profile)),
                            ),
                    )
                    .when(active, |header| {
                        header.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded(px(CORNER_COMPACT))
                                .bg(theme::green_soft())
                                .text_xs()
                                .text_color(theme::green())
                                .child(status_dot(theme::green()))
                                .child(i18n.text("Connected")),
                        )
                    }),
            )
            .child(
                div()
                    .mx_6()
                    .rounded(px(CORNER_PANEL))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .child(remote_detail_row(
                        i18n.text("Source"),
                        source.to_owned(),
                        true,
                    ))
                    .child(remote_detail_row(i18n.text("Identity"), identity, true))
                    .child(remote_detail_row(
                        i18n.text("Herdr command"),
                        herdr_path,
                        false,
                    )),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_6()
                    .py_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(i18n.text("Uses OpenSSH config, keys, agent, and known_hosts.")),
                    )
                    .when(saved, |footer| {
                        footer.child(
                            icon_only_button_tone(
                                "remove-managed-node-detail",
                                i18n.text("Remove saved host"),
                                IconName::Trash,
                                ButtonTone::Ghost,
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(
                                move |this, _, _window, cx| this.request_remove_node(index, cx),
                            )),
                        )
                    })
                    .child(
                        button(
                            "connect-managed-node",
                            if active {
                                i18n.text("Reconnect")
                            } else {
                                i18n.text("Connect")
                            },
                            ButtonTone::Primary,
                            ButtonSize::Md,
                        )
                        .on_click(
                            cx.listener(move |this, _, _window, cx| this.choose_node(index, cx)),
                        ),
                    ),
            )
    }

    fn render_add_remote(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let cancel = button(
            "cancel-add-remote",
            i18n.text("Cancel"),
            ButtonTone::Neutral,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.close_add_remote(cx)))
        .into_any_element();
        let connect = button(
            "save-add-remote",
            i18n.text("Save & connect"),
            ButtonTone::Primary,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.save_remote(cx)))
        .into_any_element();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_6()
                    .py_5()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(36.))
                            .rounded(px(CORNER_CONTROL))
                            .bg(theme::accent_soft())
                            .child(icon(IconName::Globe, theme::accent(), 17.)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(i18n.text("New SSH connection")),
                            )
                            .child(div().text_xs().text_color(theme::muted()).child(
                                i18n.text("Save a host, then discover its Herdr sessions."),
                            )),
                    ),
            )
            .child(
                div()
                    .id("new-ssh-form-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .gap_3()
                    .px_6()
                    .py_5()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child(field(
                                i18n.text("Label"),
                                false,
                                Some(i18n.text("Name shown in OcHerdr.").into()),
                                self.remote_label.clone(),
                            )))
                            .child(div().w(px(132.)).flex_none().child(field(
                                i18n.text("Port"),
                                false,
                                Some(i18n.text("Uses SSH config when empty.").into()),
                                self.remote_port.clone(),
                            ))),
                    )
                    .child(field(
                        i18n.text("Destination"),
                        true,
                        Some(
                            i18n.text("SSH alias or user@host from ~/.ssh/config.")
                                .into(),
                        ),
                        self.remote_destination.clone(),
                    ))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(
                                div().flex_1().min_w_0().child(field(
                                    i18n.text("Identity file"),
                                    false,
                                    Some(
                                        i18n.text("Optional key path; SSH agent still works.")
                                            .into(),
                                    ),
                                    self.remote_identity_file.clone(),
                                )),
                            )
                            .child(div().w(px(150.)).flex_none().child(field(
                                i18n.text("Herdr command"),
                                false,
                                Some(i18n.text("Remote command or path.").into()),
                                self.remote_herdr_path.clone(),
                            ))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .px_6()
                    .py_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(cancel)
                    .child(connect),
            )
    }
}

fn remote_group_label(label: &'static str, count: usize) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_4()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::muted())
        .child(label)
        .child(count.to_string())
}

fn remote_detail_row(label: &'static str, value: String, separated: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .min_h(px(46.))
        .px_4()
        .when(separated, |row| {
            row.border_b_1().border_color(theme::border())
        })
        .child(
            div()
                .w(px(118.))
                .flex_none()
                .text_xs()
                .text_color(theme::muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(theme::text())
                .child(value),
        )
}
