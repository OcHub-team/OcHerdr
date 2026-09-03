mod support;
use support::*;

use super::super::*;
use ochub_ui::layout::{group, row, row_label, section_header};
use ochub_ui::scrollbar::{VerticalScrollbar, contain_vertical_scroll};

impl HostCenter {
    pub(crate) fn render_node_manager(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let indexes = self.filtered_profile_indexes(cx);
        let count = indexes.len();
        self.sync_host_list_state(&indexes, cx);
        let list_state = self.host_list_state.clone();
        let list = ochub_ui::gpui::list(
            list_state.clone(),
            cx.processor(move |this, ix: usize, _window, cx| {
                match this.host_list_revision.indexes.get(ix).copied() {
                    Some(index) => div()
                        .w_full()
                        .pb_1()
                        .child(this.managed_host_row(index, cx))
                        .into_any_element(),
                    None => empty_state(
                        IconName::Search,
                        this.i18n.text(k::HOSTS_SEARCH_EMPTY_TITLE),
                        this.i18n.text(k::HOSTS_SEARCH_EMPTY_BODY),
                        None,
                    )
                    .into_any_element(),
                }
            }),
        );
        let detail = if self.form().is_some() {
            self.render_remote_form(cx).into_any_element()
        } else if self.host_bulk_mode {
            self.render_bulk_inspector(cx).into_any_element()
        } else {
            self.render_remote_detail(cx).into_any_element()
        };

        div()
            .id("host-center")
            .role(ochub_ui::gpui::Role::Region)
            .aria_label(i18n.text(k::HOSTS_CENTER_TITLE))
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
                            i18n.text(k::HOSTS_CENTER_BACK),
                            IconName::ChevronLeft,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
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
                                    .child(i18n.text(k::HOSTS_CENTER_TITLE)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::muted())
                                    .child(i18n.text(k::HOSTS_CENTER_SUBTITLE)),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().w(px(236.)).child(self.remote_search.clone()))
                    .child(
                        icon_button_tone(
                            "refresh-common-hosts",
                            i18n.text(k::COMMON_REFRESH),
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
                                i18n.text(k::COMMON_DONE)
                            } else {
                                i18n.text(k::COMMON_SELECT)
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
                            i18n.text(k::HOSTS_NEW),
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
                                    .child(format!("{count} {}", i18n.text(k::HOSTS_COUNT_SUFFIX))),
                            )
                            .child(
                                div()
                                    .id("host-center-list")
                                    .role(ochub_ui::gpui::Role::List)
                                    .aria_label(i18n.text(k::HOSTS_TITLE))
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .w_full()
                                    .on_scroll_wheel(contain_vertical_scroll(list_state.clone()))
                                    .child(
                                        list.with_sizing_behavior(
                                            ochub_ui::gpui::ListSizingBehavior::Auto,
                                        )
                                        .flex_1()
                                        .min_h_0()
                                        .w_full()
                                        .py_2(),
                                    )
                                    .child(VerticalScrollbar::new(
                                        ochub_ui::gpui::ElementId::Name(
                                            "host-center-list-scrollbar".into(),
                                        ),
                                        list_state,
                                    )),
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
                i18n.text(k::HOSTS_FILTERS_ALL),
                HostFilter::All,
                self.host_filter_count(&HostFilter::All),
                cx,
            ),
            self.host_nav_item(
                "host-filter-favorites",
                i18n.text(k::HOSTS_FILTERS_FAVORITES),
                HostFilter::Favorites,
                self.host_filter_count(&HostFilter::Favorites),
                cx,
            ),
            self.host_nav_item(
                "host-filter-recent",
                i18n.text(k::HOSTS_FILTERS_RECENT),
                HostFilter::Recent,
                self.host_filter_count(&HostFilter::Recent),
                cx,
            ),
            self.host_nav_item(
                "host-filter-attention",
                i18n.text(k::HOSTS_FILTERS_ATTENTION),
                HostFilter::Attention,
                self.host_filter_count(&HostFilter::Attention),
                cx,
            ),
        ];

        items.push(host_nav_heading(i18n.text(k::HOSTS_NAV_GROUPS)));
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
            items.push(host_nav_heading(i18n.text(k::HOSTS_TAGS)));
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

        items.push(host_nav_heading(i18n.text(k::HOSTS_NAV_SOURCES)));
        for (id, label, source) in [
            (
                "host-filter-saved",
                i18n.text(k::HOSTS_SOURCE_SAVED),
                ConnectionSource::Saved,
            ),
            (
                "host-filter-ssh-config",
                i18n.text(k::HOSTS_SOURCE_SSH_CONFIG),
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

        let nav_scroll = self.host_nav_scroll.clone();
        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(196.))
            .flex_none()
            .min_h_0()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .child(
                div()
                    .id("host-center-navigation")
                    .role(ochub_ui::gpui::Role::Navigation)
                    .aria_label(i18n.text(k::HOSTS_FILTERS_ARIA))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(&nav_scroll)
                    .on_scroll_wheel(contain_vertical_scroll(nav_scroll.clone()))
                    .px_2()
                    .py_3()
                    .children(items),
            )
            .child(VerticalScrollbar::new(
                ochub_ui::gpui::ElementId::Name("host-center-navigation-scrollbar".into()),
                nav_scroll,
            ))
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
        !ssh_config_entry_is_hidden(&self.profiles, profile)
            && host_fits_filter(
                profile,
                filter,
                self.host_metadata.get(profile.id()),
                &self.recent_connection_ids,
                &self.orphaned_ssh_hosts,
                &self.host_health,
            )
    }

    /// Rebuild the virtual list when the visible set or the inputs that produce
    /// it change. `ListState` addresses rows by index, so a stale count paints
    /// the wrong host or reads past the end.
    fn sync_host_list_state(&mut self, indexes: &[usize], cx: &App) {
        let revision = HostListRevision {
            filter: self.host_filter.clone(),
            query: self.remote_search.read(cx).content().trim().to_owned(),
            bulk: self.host_bulk_mode,
            indexes: indexes.to_vec(),
        };
        let count = indexes.len().max(1);
        if self.host_list_revision != revision || self.host_list_state.item_count() != count {
            self.host_list_state.reset(count);
            self.host_list_revision = revision;
        }
    }

    fn host_filter_title(&self) -> SharedString {
        let i18n = self.i18n;
        match &self.host_filter {
            HostFilter::All => i18n.text(k::HOSTS_FILTERS_ALL).into(),
            HostFilter::Favorites => i18n.text(k::HOSTS_FILTERS_FAVORITES).into(),
            HostFilter::Recent => i18n.text(k::HOSTS_FILTERS_RECENT).into(),
            HostFilter::Attention => i18n.text(k::HOSTS_FILTERS_ATTENTION).into(),
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
            index == self.managed_profile_index && !matches!(self.form(), Some(RemoteForm::Create));
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
                self.display_label(index),
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
                                    .child(self.display_label(index)),
                            )
                            .when(active, |row| {
                                row.child(host_pill(i18n.text(k::COMMON_CURRENT)))
                            }),
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
                            i18n.text(k::HOSTS_FAVORITE_REMOVE)
                        } else {
                            i18n.text(k::HOSTS_FAVORITE_ADD)
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
                i18n.text(k::HOSTS_SELECT_PROMPT_TITLE),
                i18n.text(k::HOSTS_SELECT_PROMPT_BODY),
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
        let group_name = metadata
            .group
            .clone()
            .unwrap_or_else(|| i18n.text(k::HOSTS_UNGROUPED).to_owned());
        let tags = if metadata.tags.is_empty() {
            i18n.text(k::HOSTS_NO_TAGS).to_owned()
        } else {
            metadata.tags.join(" · ")
        };
        let (identity, herdr_path) = match &profile {
            ConnectionProfile::Local { herdr_path } => (
                i18n.text(k::HOSTS_SYSTEM_DEFAULT).to_owned(),
                herdr_path.clone(),
            ),
            ConnectionProfile::Ssh {
                identity_file,
                herdr_path,
                ..
            } => (
                identity_file
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| i18n.text(k::HOSTS_SSH_CONFIG_OR_AGENT).into()),
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
                                            .child(self.display_label(index)),
                                    )
                                    .when(active, |header| {
                                        header.child(host_pill(i18n.text(k::COMMON_CURRENT)))
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
                                    i18n.text(k::HOSTS_FAVORITED)
                                } else {
                                    i18n.text(k::HOSTS_FAVORITE)
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
                                i18n.text(k::COMMON_EDIT),
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
            .child({
                let inspector_scroll = self.host_inspector_scroll.clone();
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("host-inspector-scroll")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .overflow_y_scroll()
                            .track_scroll(&inspector_scroll)
                            .on_scroll_wheel(contain_vertical_scroll(inspector_scroll.clone()))
                            .gap_6()
                            .px_6()
                            .py_5()
                            .child(self.render_health_panel(index, health.as_ref(), cx))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(section_header(i18n.text(k::HOSTS_ORGANIZATION), None))
                                    .child(group(vec![
                                        inspector_row(i18n.text(k::HOSTS_GROUP), group_name),
                                        inspector_row(i18n.text(k::HOSTS_TAGS), tags),
                                    ])),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(section_header(i18n.text(k::HOSTS_CONNECTION), None))
                                    .child(group(vec![
                                        inspector_row(
                                            i18n.text(k::COMMON_SOURCE),
                                            source.description(i18n).to_owned(),
                                        ),
                                        inspector_row(i18n.text(k::HOSTS_IDENTITY), identity),
                                        inspector_row(
                                            i18n.text(k::HOSTS_HERDR_COMMAND),
                                            herdr_path,
                                        ),
                                    ])),
                            ),
                    )
                    .child(VerticalScrollbar::new(
                        ochub_ui::gpui::ElementId::Name("host-inspector-scroll-scrollbar".into()),
                        inspector_scroll,
                    ))
            })
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
                            .child(i18n.text(k::HOSTS_OPENSSH_TRUST)),
                    )
                    .when(matches!(profile, ConnectionProfile::Ssh { .. }), |footer| {
                        footer.child(
                            button(
                                "open-host-terminal",
                                i18n.text(k::HOSTS_OPEN_IN_TERMINAL),
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
                                i18n.text(k::HOSTS_RECONNECT)
                            } else {
                                i18n.text(k::HOSTS_CONNECT)
                            },
                            ButtonTone::Primary,
                            ButtonSize::Md,
                        )
                        .on_click(cx.listener(
                            move |this, _, _window, cx| this.select_live_profile(index, cx),
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
        let mut detail = i18n.text(k::HOSTS_HEALTH_GUIDANCE_UNCHECKED).to_owned();
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
                pieces.push(format!("{count} {}", i18n.text(k::HOSTS_SESSIONS_SUFFIX)));
            }
            pieces.push(i18n.checked_ago(unix_timestamp().saturating_sub(cached.checked_at)));
            facts = pieces.join(" · ");
        }
        let checking = matches!(health, Some(HostHealthView::Checking { .. }));
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
                            i18n.text(k::HOSTS_TEST_CONNECTION),
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
        let creating = matches!(self.form(), Some(RemoteForm::Create));
        let index = match self.form() {
            Some(RemoteForm::Edit(index)) => Some(index),
            _ => None,
        };
        let source = index
            .and_then(|index| self.profiles.get(index))
            .map(connection_source)
            .unwrap_or(ConnectionSource::Saved);
        let active = index == Some(self.profile_index);
        let form_scroll = self.host_form_scroll.clone();
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
                                    i18n.text(k::HOSTS_NEW)
                                } else {
                                    i18n.text(k::HOSTS_EDIT)
                                },
                            ))
                            .child(div().text_xs().text_color(theme::muted()).child(
                                if source == ConnectionSource::SshConfig {
                                    i18n.text(k::HOSTS_FORM_SSH_READONLY)
                                } else {
                                    i18n.text(k::HOSTS_FORM_CHANGES_NEXT_CONNECT)
                                },
                            )),
                    )
                    .child(
                        icon_only_button_tone(
                            "close-host-form",
                            i18n.text(k::HOSTS_FORM_KEEP_CURRENT),
                            IconName::Close,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.close_add_remote(cx))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("host-form-scroll")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .overflow_y_scroll()
                            .track_scroll(&form_scroll)
                            .on_scroll_wheel(contain_vertical_scroll(form_scroll.clone()))
                            .gap_4()
                            .px_6()
                            .py_5()
                            .child(field(
                                i18n.text(k::COMMON_NAME),
                                false,
                                Some(i18n.text(k::HOSTS_FORM_NAME_DESCRIPTION).into()),
                                self.remote_label.clone(),
                            ))
                            .child(if source == ConnectionSource::SshConfig {
                                field(
                                    i18n.text(k::HOSTS_FORM_DESTINATION),
                                    false,
                                    Some(i18n.text(k::HOSTS_FORM_DESTINATION_SSH_MANAGED).into()),
                                    readonly_field_control(
                                        self.profiles
                                            .get(index.unwrap_or_default())
                                            .map(profile_endpoint)
                                            .unwrap_or_default(),
                                    ),
                                )
                                .into_any_element()
                            } else {
                                field(
                                    i18n.text(k::HOSTS_FORM_DESTINATION),
                                    true,
                                    Some(i18n.text(k::HOSTS_FORM_DESTINATION_DESCRIPTION).into()),
                                    self.remote_destination.clone(),
                                )
                                .into_any_element()
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .child(div().flex_1().min_w_0().child(field(
                                        i18n.text(k::HOSTS_GROUP),
                                        false,
                                        Some(i18n.text(k::HOSTS_FORM_GROUP_DESCRIPTION).into()),
                                        self.remote_group.clone(),
                                    )))
                                    .child(div().flex_1().min_w_0().child(field(
                                        i18n.text(k::HOSTS_TAGS),
                                        false,
                                        Some(i18n.text(k::HOSTS_FORM_TAGS_DESCRIPTION).into()),
                                        self.remote_tags.clone(),
                                    ))),
                            )
                            .child(
                                div()
                                    .id("remote-advanced-toggle")
                                    .role(ochub_ui::gpui::Role::Button)
                                    .tab_stop(false)
                                    .aria_label(i18n.text(k::COMMON_ADVANCED))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .h(px(32.))
                                    .text_sm()
                                    .text_color(theme::subtext())
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.toggle_remote_advanced(cx)
                                    }))
                                    .child(icon(
                                        if self.remote_advanced_open {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        },
                                        theme::muted(),
                                        13.,
                                    ))
                                    .child(i18n.text(k::HOSTS_FORM_ADVANCED_OVERRIDES)),
                            )
                            .when(self.remote_advanced_open, |form| {
                                form.child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap_3()
                                        .child(div().flex_1().min_w_0().child(field(
                                            i18n.text(k::HOSTS_FORM_PORT),
                                            false,
                                            Some(i18n.text(k::HOSTS_FORM_PORT_DESCRIPTION).into()),
                                            self.remote_port.clone(),
                                        )))
                                        .child(
                                            div().flex_1().min_w_0().child(field(
                                                i18n.text(k::HOSTS_HERDR_COMMAND),
                                                false,
                                                Some(
                                                    i18n.text(
                                                        k::HOSTS_FORM_HERDR_COMMAND_DESCRIPTION,
                                                    )
                                                    .into(),
                                                ),
                                                self.remote_herdr_path.clone(),
                                            )),
                                        )
                                        .child(
                                            div().flex_1().min_w_0().child(field(
                                                i18n.text(k::HOSTS_FORM_IDENTITY_FILE),
                                                false,
                                                Some(
                                                    i18n.text(
                                                        k::HOSTS_FORM_IDENTITY_FILE_DESCRIPTION,
                                                    )
                                                    .into(),
                                                ),
                                                self.remote_identity_file.clone(),
                                            )),
                                        ),
                                )
                            }),
                    )
                    .child(VerticalScrollbar::new(
                        ochub_ui::gpui::ElementId::Name("host-form-scroll-scrollbar".into()),
                        form_scroll,
                    )),
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
                                    i18n.text(k::HOSTS_FORM_REMOVE_SAVED),
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
                            i18n.text(k::HOSTS_FORM_KEEP_CURRENT),
                            ButtonTone::Neutral,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.close_add_remote(cx))),
                    )
                    .child(
                        button(
                            "save-host-form",
                            i18n.text(k::COMMON_SAVE),
                            ButtonTone::Neutral,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| this.save_remote(false, cx))),
                    )
                    .child(
                        button(
                            "save-connect-host-form",
                            if active {
                                i18n.text(k::HOSTS_FORM_SAVE_RECONNECT)
                            } else {
                                i18n.text(k::HOSTS_FORM_SAVE_CONNECT)
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
                    .child(i18n.text(k::HOSTS_BULK_PROMPT)),
            )
            .child(div().w_full().max_w(px(520.)).child(group(vec![
                        field(
                            i18n.text(k::HOSTS_GROUP),
                            false,
                            None,
                            self.remote_group.clone(),
                        )
                        .px_4()
                        .py_3()
                        .into_any_element(),
                        field(
                            i18n.text(k::HOSTS_TAGS),
                            false,
                            None,
                            self.remote_tags.clone(),
                        )
                        .px_4()
                        .py_3()
                        .into_any_element(),
                    ])))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button(
                            "bulk-organize-hosts",
                            i18n.text(k::HOSTS_BULK_APPLY),
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
                            i18n.text(k::HOSTS_FAVORITE_ADD),
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
                            i18n.text(k::HOSTS_FAVORITE_REMOVE),
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
                    i18n.text(k::HOSTS_BULK_REMOVE_ELLIPSIS),
                    IconName::Trash,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.request_bulk_remove(cx))),
            )
    }
}

impl OcHerdrView {
    pub(super) fn render_host_switcher(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let entries = {
            let center = self.host_center.read(cx);
            center
                .switcher_entries()
                .into_iter()
                .filter_map(|index| {
                    let profile = center.profiles().get(index).cloned()?;
                    let label = center.display_label(index);
                    Some((index, profile, label, index == self.profile_index))
                })
                .collect::<Vec<_>>()
        };
        let mut items = Vec::new();
        for (index, profile, label, active) in entries {
            let profile_id = profile.id().to_owned();
            let state = self.host_connection_state(&profile_id);
            let state_label = i18n.host_connection_status(state);
            let row_label = format!("{label}, {state_label}");
            let dot_color = match state {
                HostConnectionState::Disconnected => theme::muted(),
                HostConnectionState::Connecting => theme::yellow(),
                HostConnectionState::Connected => theme::green(),
                HostConnectionState::Degraded => theme::red(),
            };
            let can_disconnect = matches!(
                state,
                HostConnectionState::Connected | HostConnectionState::Degraded
            );
            let disconnect_label = i18n.disconnect_host(&label);
            let disconnect_profile_id = profile_id.clone();
            let disconnect_debug_id = profile_id.clone();
            items.push(
                div()
                    .id(("switch-host", index))
                    .role(ochub_ui::gpui::Role::Button)
                    .tab_stop(false)
                    .aria_label(row_label)
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
                    .child(status_dot(dot_color))
                    .child(icon(
                        if matches!(profile, ConnectionProfile::Local { .. }) {
                            IconName::Desktop
                        } else {
                            IconName::Globe
                        },
                        if active {
                            theme::accent()
                        } else {
                            theme::muted()
                        },
                        13.,
                    ))
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(label))
                    .when(can_disconnect, |row| {
                        row.child(icon_action_tooltip(
                            "disconnect-host",
                            disconnect_label.clone(),
                            icon_only_button_tone(
                                ("disconnect-host", index),
                                disconnect_label,
                                IconName::Close,
                                ButtonTone::Danger,
                                ButtonSize::Sm,
                            )
                            .size(px(20.))
                            .rounded_full()
                            .debug_selector(move || {
                                format!("disconnect-host-{disconnect_debug_id}")
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.disconnect_host(&disconnect_profile_id, cx)
                                },
                            )),
                        ))
                    })
                    .into_any_element(),
            );
        }
        items.push(
            div()
                .id("switch-host-manage")
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(i18n.text(k::HOSTS_MANAGE))
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
                .child(i18n.text(k::HOSTS_MANAGE_ELLIPSIS))
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
                            .child(i18n.text(k::HOSTS_SWITCH)),
                    )
                    .children(items),
            )
    }
}

// Compact muted labels for the 196px filter rail. `section_header` is text_sm
// on `theme::text()` and too loud for this sidebar.
