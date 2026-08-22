use super::super::*;

impl OcHerdrView {
    pub(super) fn render_node_manager(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let indexes = self.filtered_profile_indexes(cx);
        let count = indexes.len();
        let mut rows = indexes
            .into_iter()
            .map(|index| self.managed_host_row(index, cx))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            rows.push(
                empty_state(
                    IconName::Search,
                    i18n.text("No matching hosts"),
                    i18n.text("Adjust the search or choose another filter."),
                    None,
                )
                .into_any_element(),
            );
        }
        let detail = if self.remote_form == RemoteForm::Create
            || matches!(self.remote_form, RemoteForm::Edit(_))
        {
            self.render_remote_form(cx).into_any_element()
        } else if self.host_bulk_mode {
            self.render_bulk_inspector(cx).into_any_element()
        } else {
            self.render_remote_detail(cx).into_any_element()
        };

        div()
            .id("host-center")
            .role(ochub_ui::gpui::Role::Region)
            .aria_label(i18n.text("Host center"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(theme::content_background())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(64.))
                    .flex_none()
                    .px_4()
                    .border_b_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .child(
                        icon_only_button_tone(
                            "close-host-center",
                            i18n.text("Back to workspace"),
                            IconName::ChevronLeft,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_node_manager(window, cx)),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(i18n.text("Host center")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child(i18n.text("Connections, organization, and diagnostics")),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().w(px(236.)).child(self.remote_search.clone()))
                    .child(
                        icon_button_tone(
                            "refresh-common-hosts",
                            i18n.text("Refresh"),
                            IconName::Refresh,
                            ButtonTone::Neutral,
                            ButtonSize::Sm,
                        )
                        .on_click(
                            cx.listener(|this, _, _window, cx| this.refresh_common_host_health(cx)),
                        ),
                    )
                    .child(
                        button(
                            "toggle-host-selection",
                            if self.host_bulk_mode {
                                i18n.text("Done")
                            } else {
                                i18n.text("Select")
                            },
                            if self.host_bulk_mode {
                                ButtonTone::Primary
                            } else {
                                ButtonTone::Neutral
                            },
                            ButtonSize::Sm,
                        )
                        .on_click(
                            cx.listener(|this, _, _window, cx| this.toggle_host_bulk_mode(cx)),
                        ),
                    )
                    .child(
                        icon_button_tone(
                            "add-managed-node",
                            i18n.text("New host"),
                            IconName::Add,
                            ButtonTone::Primary,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.open_add_remote(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.render_host_navigation(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(332.))
                            .flex_none()
                            .min_h_0()
                            .border_r_1()
                            .border_color(theme::border())
                            .bg(theme::sidebar_background())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .h(px(42.))
                                    .px_4()
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child(self.host_filter_title())
                                    .child(format!("{count} {}", i18n.text("hosts"))),
                            )
                            .child(
                                div()
                                    .id("host-center-list")
                                    .role(ochub_ui::gpui::Role::List)
                                    .aria_label(i18n.text("Hosts"))
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_scroll()
                                    .py_2()
                                    .children(rows),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .bg(theme::content_background())
                            .child(detail),
                    ),
            )
    }

    fn render_host_navigation(&self, cx: &mut Context<Self>) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        let mut items = vec![
            self.host_nav_item(
                "host-filter-all",
                i18n.text("All hosts"),
                HostFilter::All,
                self.host_filter_count(&HostFilter::All),
                cx,
            ),
            self.host_nav_item(
                "host-filter-favorites",
                i18n.text("Favorites"),
                HostFilter::Favorites,
                self.host_filter_count(&HostFilter::Favorites),
                cx,
            ),
            self.host_nav_item(
                "host-filter-recent",
                i18n.text("Recent"),
                HostFilter::Recent,
                self.host_filter_count(&HostFilter::Recent),
                cx,
            ),
            self.host_nav_item(
                "host-filter-attention",
                i18n.text("Needs attention"),
                HostFilter::Attention,
                self.host_filter_count(&HostFilter::Attention),
                cx,
            ),
        ];

        items.push(host_nav_heading(i18n.text("Groups")));
        for (index, group) in self.host_groups.iter().enumerate() {
            let filter = HostFilter::Group(group.clone());
            items.push(self.host_nav_item(
                ("host-filter-group", index),
                group.clone(),
                filter.clone(),
                self.host_filter_count(&filter),
                cx,
            ));
        }

        let mut tags = self
            .host_metadata
            .values()
            .flat_map(|metadata| metadata.tags.iter().cloned())
            .collect::<Vec<_>>();
        tags.sort_by_key(|tag| tag.to_lowercase());
        tags.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        if !tags.is_empty() {
            items.push(host_nav_heading(i18n.text("Tags")));
            for (index, tag) in tags.into_iter().enumerate() {
                let filter = HostFilter::Tag(tag.clone());
                items.push(self.host_nav_item(
                    ("host-filter-tag", index),
                    format!("# {tag}"),
                    filter.clone(),
                    self.host_filter_count(&filter),
                    cx,
                ));
            }
        }

        items.push(host_nav_heading(i18n.text("Sources")));
        for (id, label, source) in [
            (
                "host-filter-saved",
                i18n.text("Saved in OcHerdr"),
                ConnectionSource::Saved,
            ),
            (
                "host-filter-ssh-config",
                i18n.text("SSH config"),
                ConnectionSource::SshConfig,
            ),
        ] {
            let filter = HostFilter::Source(source);
            items.push(self.host_nav_item(
                id,
                label,
                filter.clone(),
                self.host_filter_count(&filter),
                cx,
            ));
        }

        div()
            .id("host-center-navigation")
            .role(ochub_ui::gpui::Role::Navigation)
            .aria_label(i18n.text("Host filters"))
            .flex()
            .flex_col()
            .w(px(196.))
            .flex_none()
            .min_h_0()
            .overflow_scroll()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .px_2()
            .py_3()
            .children(items)
            .into_any_element()
    }

    fn host_nav_item(
        &self,
        id: impl Into<ochub_ui::gpui::ElementId>,
        label: impl Into<SharedString>,
        filter: HostFilter,
        count: usize,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let selected = self.host_filter == filter;
        div()
            .id(id)
            .role(ochub_ui::gpui::Role::Button)
            .tab_stop(false)
            .aria_selected(selected)
            .flex()
            .items_center()
            .gap_2()
            .h(px(32.))
            .px_2()
            .rounded(px(CORNER_COMPACT))
            .bg(if selected {
                theme::selection()
            } else {
                theme::surface().alpha(0.)
            })
            .text_sm()
            .text_color(if selected {
                theme::text()
            } else {
                theme::subtext()
            })
            .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _, _window, cx| this.set_host_filter(filter.clone(), cx)),
            )
            .child(div().flex_1().min_w_0().truncate().child(label.into()))
            .child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .child(count.to_string()),
            )
            .into_any_element()
    }

    fn host_filter_count(&self, filter: &HostFilter) -> usize {
        self.profiles
            .iter()
            .enumerate()
            .filter(|(index, _)| self.host_matches_filter(*index, filter))
            .count()
    }

    fn host_matches_filter(&self, index: usize, filter: &HostFilter) -> bool {
        let Some(profile) = self.profiles.get(index) else {
            return false;
        };
        if connection_source(profile) == ConnectionSource::SshConfig
            && ssh_destination(profile)
                .is_some_and(|destination| ssh_config_covered_by_saved(&self.profiles, destination))
        {
            return false;
        }
        let metadata = self.host_metadata.get(profile.id());
        match filter {
            HostFilter::All => true,
            HostFilter::Favorites => metadata.is_some_and(|value| value.favorite),
            HostFilter::Recent => self
                .recent_connection_ids
                .iter()
                .any(|id| id == profile.id()),
            HostFilter::Attention => {
                self.orphaned_ssh_hosts.contains(profile.id())
                    || self
                        .host_health
                        .get(profile.id())
                        .is_some_and(|health| match health {
                            HostHealthView::Checking => false,
                            HostHealthView::Checked { cached, .. } => {
                                cached.status != HostHealthStatus::Ready
                            }
                        })
            }
            HostFilter::Source(source) => connection_source(profile) == *source,
            HostFilter::Group(group) => {
                metadata.and_then(|value| value.group.as_deref()) == Some(group.as_str())
            }
            HostFilter::Tag(tag) => {
                metadata.is_some_and(|value| value.tags.iter().any(|candidate| candidate == tag))
            }
        }
    }

    fn host_filter_title(&self) -> SharedString {
        let i18n = self.i18n;
        match &self.host_filter {
            HostFilter::All => i18n.text("All hosts").into(),
            HostFilter::Favorites => i18n.text("Favorites").into(),
            HostFilter::Recent => i18n.text("Recent").into(),
            HostFilter::Attention => i18n.text("Needs attention").into(),
            HostFilter::Source(source) => source.label(i18n).into(),
            HostFilter::Group(group) | HostFilter::Tag(group) => group.clone().into(),
        }
    }

    fn managed_host_row(&self, index: usize, cx: &mut Context<Self>) -> ochub_ui::gpui::AnyElement {
        let Some(profile) = self.profiles.get(index).cloned() else {
            return div().into_any_element();
        };
        let i18n = self.i18n;
        let selected =
            index == self.managed_profile_index && self.remote_form != RemoteForm::Create;
        let active = index == self.profile_index;
        let metadata = self
            .host_metadata
            .get(profile.id())
            .cloned()
            .unwrap_or_default();
        let favorite = metadata.favorite;
        let bulk_selected = self.host_bulk_selection.contains(profile.id());
        let status = self.host_health.get(profile.id());
        let (status_color, status_text) = host_health_summary(status, i18n);
        let organization = metadata
            .group
            .or_else(|| metadata.tags.first().cloned())
            .unwrap_or_else(|| connection_source(&profile).label(i18n).to_owned());
        div()
            .id(("managed-host", index))
            .role(ochub_ui::gpui::Role::Button)
            .tab_stop(false)
            .aria_label(format!(
                "{} · {} · {status_text}",
                self.host_display_label(index),
                profile_endpoint(&profile)
            ))
            .aria_selected(if self.host_bulk_mode {
                bulk_selected
            } else {
                selected
            })
            .flex()
            .items_center()
            .gap_3()
            .min_h(px(64.))
            .mx_2()
            .px_3()
            .py_2()
            .rounded(px(CORNER_CONTROL))
            .bg(if selected || bulk_selected {
                theme::selection()
            } else {
                theme::surface().alpha(0.)
            })
            .hover(|style| style.bg(theme::surface_hover()))
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _, _window, cx| this.select_managed_profile(index, cx)),
            )
            .child(if self.host_bulk_mode && index != 0 {
                selection_mark(bulk_selected).into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(28.))
                    .rounded(px(CORNER_COMPACT))
                    .bg(if selected {
                        theme::accent_soft()
                    } else {
                        theme::inset()
                    })
                    .child(icon(
                        if matches!(profile, ConnectionProfile::Local { .. }) {
                            IconName::Desktop
                        } else {
                            IconName::Globe
                        },
                        if selected {
                            theme::accent()
                        } else {
                            theme::muted()
                        },
                        14.,
                    ))
                    .into_any_element()
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme::text())
                                    .child(self.host_display_label(index)),
                            )
                            .when(active, |row| row.child(host_pill(i18n.text("Current")))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(theme::muted())
                            .child(
                                div()
                                    .max_w(px(150.))
                                    .truncate()
                                    .child(profile_endpoint(&profile)),
                            )
                            .child("·")
                            .child(div().truncate().child(organization)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(status_color)
                            .child(status_dot(status_color))
                            .child(status_text),
                    ),
            )
            .when(!self.host_bulk_mode && index != 0, |row| {
                row.child(
                    div()
                        .id(("favorite-host", index))
                        .role(ochub_ui::gpui::Role::Button)
                        .tab_stop(false)
                        .aria_label(if favorite {
                            i18n.text("Remove from favorites")
                        } else {
                            i18n.text("Add to favorites")
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(28.))
                        .rounded(px(CORNER_COMPACT))
                        .text_sm()
                        .text_color(if favorite {
                            theme::accent()
                        } else {
                            theme::muted()
                        })
                        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::accent()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| cx.stop_propagation()),
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.toggle_host_favorite(index, cx)
                        }))
                        .child(if favorite { "★" } else { "☆" }),
                )
            })
            .into_any_element()
    }

    fn render_remote_detail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let Some(profile) = self.profiles.get(self.managed_profile_index).cloned() else {
            return empty_state(
                IconName::Globe,
                i18n.text("Select a host"),
                i18n.text("Choose a host to inspect its connection and health."),
                None,
            );
        };
        let index = self.managed_profile_index;
        let active = index == self.profile_index;
        let source = connection_source(&profile);
        let metadata = self
            .host_metadata
            .get(profile.id())
            .cloned()
            .unwrap_or_default();
        let favorite = metadata.favorite;
        let group = metadata
            .group
            .clone()
            .unwrap_or_else(|| i18n.text("Ungrouped").to_owned());
        let tags = if metadata.tags.is_empty() {
            i18n.text("No tags").to_owned()
        } else {
            metadata.tags.join(" · ")
        };
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
        let health = self.host_health.get(profile.id()).cloned();

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
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme::text())
                                            .child(self.host_display_label(index)),
                                    )
                                    .when(active, |header| {
                                        header.child(host_pill(i18n.text("Current")))
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .text_color(theme::muted())
                                    .child(profile_endpoint(&profile)),
                            ),
                    )
                    .when(index != 0, |header| {
                        header.child(
                            button(
                                "favorite-selected-host",
                                if favorite {
                                    i18n.text("Favorited")
                                } else {
                                    i18n.text("Favorite")
                                },
                                ButtonTone::Ghost,
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(
                                move |this, _, _window, cx| this.toggle_host_favorite(index, cx),
                            )),
                        )
                    })
                    .when(index != 0, |header| {
                        header.child(
                            icon_button_tone(
                                "edit-selected-host",
                                i18n.text("Edit"),
                                IconName::Pencil,
                                ButtonTone::Neutral,
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(
                                move |this, _, _window, cx| this.open_edit_remote(index, cx),
                            )),
                        )
                    }),
            )
            .child(
                div()
                    .id("host-inspector-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .gap_6()
                    .px_6()
                    .py_5()
                    .child(self.render_health_panel(index, health.as_ref(), cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(inspector_heading(i18n.text("Organization")))
                            .child(
                                div()
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .child(remote_detail_row(i18n.text("Group"), group, true))
                                    .child(remote_detail_row(i18n.text("Tags"), tags, false)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(inspector_heading(i18n.text("Connection")))
                            .child(
                                div()
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .child(remote_detail_row(
                                        i18n.text("Source"),
                                        source.description(i18n).to_owned(),
                                        true,
                                    ))
                                    .child(remote_detail_row(i18n.text("Identity"), identity, true))
                                    .child(remote_detail_row(
                                        i18n.text("Herdr command"),
                                        herdr_path,
                                        false,
                                    )),
                            ),
                    ),
            )
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
                            .child(i18n.text("OpenSSH remains the source of keys and trust.")),
                    )
                    .when(matches!(profile, ConnectionProfile::Ssh { .. }), |footer| {
                        footer.child(
                            button(
                                "open-host-terminal",
                                i18n.text("Open in Terminal"),
                                ButtonTone::Neutral,
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(
                                move |this, _, _window, cx| {
                                    this.open_managed_host_in_terminal(index, cx)
                                },
                            )),
                        )
                    })
                    .child(
                        button(
                            "connect-managed-host",
                            if active {
                                i18n.text("Reconnect")
                            } else {
                                i18n.text("Connect")
                            },
                            ButtonTone::Primary,
                            ButtonSize::Md,
                        )
                        .on_click(cx.listener(
                            move |this, _, _window, cx| this.request_choose_node(index, cx),
                        )),
                    ),
            )
    }

    fn render_health_panel(
        &self,
        index: usize,
        health: Option<&HostHealthView>,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        let (color, label) = host_health_summary(health, i18n);
        let mut detail = i18n.text("Run a check to verify SSH and Herdr.").to_owned();
        let mut facts = String::new();
        if let Some(HostHealthView::Checked {
            cached,
            detail: raw_detail,
        }) = health
        {
            detail = host_health_guidance(cached.status, i18n).to_owned();
            if !raw_detail.is_empty() && cached.status != HostHealthStatus::Ready {
                detail = raw_detail.clone();
            }
            let mut pieces = vec![format!("{} ms", cached.latency_ms)];
            if let Some(version) = &cached.herdr_version {
                pieces.push(version.clone());
            }
            if let Some(count) = cached.session_count {
                pieces.push(format!("{count} {}", i18n.text("sessions")));
            }
            pieces.push(i18n.checked_ago(unix_timestamp().saturating_sub(cached.checked_at)));
            facts = pieces.join(" · ");
        }
        let checking = matches!(health, Some(HostHealthView::Checking));
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded(px(CORNER_PANEL))
            .bg(health_surface(health))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_dot(color))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(label),
                    )
                    .child(if checking {
                        div().child(spinner(color, 13.)).into_any_element()
                    } else {
                        button(
                            "test-selected-host",
                            i18n.text("Test connection"),
                            ButtonTone::Neutral,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.test_managed_host(index, cx)
                        }))
                        .into_any_element()
                    }),
            )
            .child(div().text_sm().text_color(theme::subtext()).child(detail))
            .when(!facts.is_empty(), |panel| {
                panel.child(div().text_xs().text_color(theme::muted()).child(facts))
            })
            .into_any_element()
    }

    fn render_remote_form(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let creating = self.remote_form == RemoteForm::Create;
        let index = match self.remote_form {
            RemoteForm::Edit(index) => Some(index),
            _ => None,
        };
        let source = index
            .and_then(|index| self.profiles.get(index))
            .map(connection_source)
            .unwrap_or(ConnectionSource::Saved);
        let active = index == Some(self.profile_index);
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
                            .flex_col()
                            .flex_1()
                            .gap_1()
                            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(
                                if creating {
                                    i18n.text("New host")
                                } else {
                                    i18n.text("Edit host")
                                },
                            ))
                            .child(div().text_xs().text_color(theme::muted()).child(
                                if source == ConnectionSource::SshConfig {
                                    i18n.text(
                                        "SSH config stays read-only; these are local overrides.",
                                    )
                                } else {
                                    i18n.text("Connection changes apply the next time you connect.")
                                },
                            )),
                    )
                    .child(
                        icon_only_button_tone(
                            "close-host-form",
                            i18n.text("Keep current values"),
                            IconName::Close,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.close_add_remote(cx))),
                    ),
            )
            .child(
                div()
                    .id("host-form-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .gap_4()
                    .px_6()
                    .py_5()
                    .child(field(
                        i18n.text("Name"),
                        false,
                        Some(i18n.text("Name shown in OcHerdr.").into()),
                        self.remote_label.clone(),
                    ))
                    .child(if source == ConnectionSource::SshConfig {
                        remote_readonly_field(
                            i18n.text("Destination"),
                            self.profiles
                                .get(index.unwrap_or_default())
                                .map(profile_endpoint)
                                .unwrap_or_default(),
                            i18n.text("Managed by ~/.ssh/config"),
                        )
                        .into_any_element()
                    } else {
                        field(
                            i18n.text("Destination"),
                            true,
                            Some(
                                i18n.text("SSH alias or user@host from ~/.ssh/config.")
                                    .into(),
                            ),
                            self.remote_destination.clone(),
                        )
                        .into_any_element()
                    })
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(div().flex_1().min_w_0().child(field(
                                i18n.text("Group"),
                                false,
                                Some(i18n.text("One group provides the primary location.").into()),
                                self.remote_group.clone(),
                            )))
                            .child(div().flex_1().min_w_0().child(field(
                                i18n.text("Tags"),
                                false,
                                Some(i18n.text("Separate multiple tags with commas.").into()),
                                self.remote_tags.clone(),
                            ))),
                    )
                    .child(
                        div()
                            .id("remote-advanced-toggle")
                            .role(ochub_ui::gpui::Role::Button)
                            .tab_stop(false)
                            .aria_label(i18n.text("Advanced"))
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(32.))
                            .text_sm()
                            .text_color(theme::subtext())
                            .cursor_pointer()
                            .on_click(
                                cx.listener(|this, _, _window, cx| this.toggle_remote_advanced(cx)),
                            )
                            .child(icon(
                                if self.remote_advanced_open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                theme::muted(),
                                13.,
                            ))
                            .child(i18n.text("Advanced connection overrides")),
                    )
                    .when(self.remote_advanced_open, |form| {
                        form.child(
                            div()
                                .flex()
                                .items_start()
                                .gap_3()
                                .child(div().flex_1().min_w_0().child(field(
                                    i18n.text("Port"),
                                    false,
                                    Some(i18n.text("Uses SSH config when empty.").into()),
                                    self.remote_port.clone(),
                                )))
                                .child(div().flex_1().min_w_0().child(field(
                                    i18n.text("Herdr command"),
                                    false,
                                    Some(i18n.text("Remote command or path.").into()),
                                    self.remote_herdr_path.clone(),
                                )))
                                .child(div().flex_1().min_w_0().child(field(
                                    i18n.text("Identity file"),
                                    false,
                                    Some(i18n.text("SSH agent still works when empty.").into()),
                                    self.remote_identity_file.clone(),
                                ))),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_6()
                    .py_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .when(
                        index.is_some_and(|index| is_saved_profile(&self.profiles[index]))
                            && !active,
                        |footer| {
                            let index = index.unwrap_or_default();
                            footer.child(
                                icon_button_tone(
                                    "remove-managed-host",
                                    i18n.text("Remove saved host"),
                                    IconName::Trash,
                                    ButtonTone::Ghost,
                                    ButtonSize::Sm,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _window, cx| this.request_remove_node(index, cx),
                                )),
                            )
                        },
                    )
                    .child(div().flex_1())
                    .child(
                        button(
                            "cancel-host-form",
                            i18n.text("Keep current values"),
                            ButtonTone::Neutral,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.close_add_remote(cx))),
                    )
                    .child(
                        button(
                            "save-host-form",
                            i18n.text("Save"),
                            ButtonTone::Neutral,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.save_remote(false, cx))),
                    )
                    .child(
                        button(
                            "save-connect-host-form",
                            if active {
                                i18n.text("Save & reconnect")
                            } else {
                                i18n.text("Save & connect")
                            },
                            ButtonTone::Primary,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.save_remote(true, cx))),
                    ),
            )
    }

    fn render_bulk_inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let count = self.host_bulk_selection.len();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_4()
            .px_8()
            .child(
                div()
                    .size(px(48.))
                    .rounded(px(CORNER_PANEL))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::accent_soft())
                    .child(icon(IconName::Check, theme::accent(), 20.)),
            )
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(i18n.selected_hosts(count)),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_sm()
                    .text_color(theme::muted())
                    .child(i18n.text(
                        "Choose hosts in the list, then apply a lightweight organization action.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .w_full()
                    .max_w(px(520.))
                    .child(div().flex_1().min_w_0().child(field(
                        i18n.text("Group"),
                        false,
                        None,
                        self.remote_group.clone(),
                    )))
                    .child(div().flex_1().min_w_0().child(field(
                        i18n.text("Tags"),
                        false,
                        None,
                        self.remote_tags.clone(),
                    ))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button(
                            "bulk-organize-hosts",
                            i18n.text("Apply organization"),
                            ButtonTone::Primary,
                            ButtonSize::Sm,
                        )
                        .on_click(
                            cx.listener(|this, _, _window, cx| this.bulk_apply_organization(cx)),
                        ),
                    )
                    .child(
                        button(
                            "bulk-favorite-hosts",
                            i18n.text("Add to favorites"),
                            ButtonTone::Neutral,
                            ButtonSize::Sm,
                        )
                        .on_click(
                            cx.listener(|this, _, _window, cx| this.bulk_set_favorite(true, cx)),
                        ),
                    )
                    .child(
                        button(
                            "bulk-unfavorite-hosts",
                            i18n.text("Remove from favorites"),
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(
                            cx.listener(|this, _, _window, cx| this.bulk_set_favorite(false, cx)),
                        ),
                    ),
            )
            .child(
                icon_button_tone(
                    "bulk-remove-host-data",
                    i18n.text("Remove local data…"),
                    IconName::Trash,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.request_bulk_remove(cx))),
            )
    }

    pub(super) fn render_host_switcher(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let mut items = Vec::new();
        for index in self.host_switcher_entries() {
            let Some(profile) = self.profiles.get(index).cloned() else {
                continue;
            };
            let active = index == self.profile_index;
            let label = self.host_display_label(index);
            items.push(
                div()
                    .id(("switch-host", index))
                    .role(ochub_ui::gpui::Role::Button)
                    .tab_stop(false)
                    .aria_label(label.clone())
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(32.))
                    .px_2()
                    .rounded(px(CORNER_COMPACT))
                    .bg(if active {
                        theme::selection()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _window, cx| {
                            this.request_choose_node(index, cx)
                        }),
                    )
                    .child(icon(
                        if matches!(profile, ConnectionProfile::Local { .. }) {
                            IconName::Desktop
                        } else {
                            IconName::Globe
                        },
                        theme::muted(),
                        13.,
                    ))
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(label))
                    .when(active, |row| row.child(status_dot(theme::green())))
                    .into_any_element(),
            );
        }
        items.push(
            div()
                .id("switch-host-manage")
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(i18n.text("Manage hosts"))
                .flex()
                .items_center()
                .h(px(32.))
                .px_2()
                .mt_1()
                .rounded(px(CORNER_COMPACT))
                .text_xs()
                .text_color(theme::muted())
                .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _window, cx| this.open_node_manager(cx)))
                .child(i18n.text("Manage hosts…"))
                .into_any_element(),
        );
        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close_host_switcher(cx)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(8.))
                    .bottom(px(STATUS_BAR_HEIGHT + 6.))
                    .w(px(SIDEBAR_WIDTH - 16.))
                    .rounded(px(CORNER_PANEL))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .p_2()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .px_2()
                            .pb_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::muted())
                            .child(i18n.text("Switch host")),
                    )
                    .children(items),
            )
    }
}

fn host_nav_heading(label: &'static str) -> ochub_ui::gpui::AnyElement {
    div()
        .px_2()
        .pt_4()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::muted())
        .child(label)
        .into_any_element()
}

fn selection_mark(selected: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded(px(CORNER_COMPACT))
        .border_1()
        .border_color(if selected {
            theme::accent()
        } else {
            theme::border_strong()
        })
        .bg(if selected {
            theme::accent_fill()
        } else {
            theme::surface()
        })
        .when(selected, |mark| {
            mark.child(icon(IconName::Check, theme::accent_text(), 12.))
        })
}

fn host_pill(label: &'static str) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded(px(CORNER_COMPACT))
        .bg(theme::accent_soft())
        .text_xs()
        .text_color(theme::accent())
        .child(label)
}

fn inspector_heading(label: &'static str) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::muted())
        .child(label)
}

fn remote_detail_row(label: &'static str, value: String, separated: bool) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .min_h(px(44.))
        .when(separated, |row| {
            row.border_b_1().border_color(theme::border())
        })
        .child(
            div()
                .w(px(126.))
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

fn remote_readonly_field(
    label: &'static str,
    value: String,
    hint: &'static str,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::subtext())
                .child(label),
        )
        .child(
            div()
                .flex()
                .items_center()
                .h(px(34.))
                .px_3()
                .rounded(px(CORNER_CONTROL))
                .border_1()
                .border_color(theme::border())
                .bg(theme::inset())
                .text_sm()
                .text_color(theme::subtext())
                .child(value),
        )
        .child(div().text_xs().text_color(theme::muted()).child(hint))
}

fn host_health_summary(
    health: Option<&HostHealthView>,
    i18n: I18n,
) -> (ochub_ui::gpui::Rgba, &'static str) {
    match health {
        Some(HostHealthView::Checking) => (theme::yellow(), i18n.text("Checking…")),
        Some(HostHealthView::Checked { cached, .. }) => match cached.status {
            HostHealthStatus::Ready => (theme::green(), i18n.text("Ready")),
            HostHealthStatus::SshOnly => (theme::yellow(), i18n.text("Herdr not ready")),
            HostHealthStatus::UnsupportedHerdr => {
                (theme::yellow(), i18n.text("Herdr update required"))
            }
            HostHealthStatus::AuthenticationRequired => {
                (theme::yellow(), i18n.text("Authentication required"))
            }
            HostHealthStatus::HostKeyRequired => {
                (theme::yellow(), i18n.text("Host key needs attention"))
            }
            HostHealthStatus::Unreachable => (theme::red(), i18n.text("Unreachable")),
            HostHealthStatus::Failed => (theme::red(), i18n.text("Check failed")),
        },
        None => (theme::muted(), i18n.text("Not checked")),
    }
}

fn host_health_guidance(status: HostHealthStatus, i18n: I18n) -> &'static str {
    i18n.text(match status {
        HostHealthStatus::Ready => "SSH and Herdr are ready.",
        HostHealthStatus::SshOnly => "SSH works, but Herdr could not be found on this host.",
        HostHealthStatus::UnsupportedHerdr => "Update Herdr or configure a newer executable path.",
        HostHealthStatus::AuthenticationRequired => {
            "Open Terminal to complete authentication, then check again."
        }
        HostHealthStatus::HostKeyRequired => "Open Terminal to review and enroll this host key.",
        HostHealthStatus::Unreachable => "Check the alias, network, VPN, and SSH port.",
        HostHealthStatus::Failed => "Review the SSH error, adjust the host, and try again.",
    })
}

fn health_surface(health: Option<&HostHealthView>) -> ochub_ui::gpui::Rgba {
    match health {
        Some(HostHealthView::Checked { cached, .. }) => match cached.status {
            HostHealthStatus::Ready => theme::green_soft(),
            HostHealthStatus::Unreachable | HostHealthStatus::Failed => theme::red_soft(),
            _ => theme::yellow_soft(),
        },
        Some(HostHealthView::Checking) => theme::yellow_soft(),
        None => theme::inset(),
    }
}
