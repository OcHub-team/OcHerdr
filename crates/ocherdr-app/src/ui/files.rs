use super::*;
use ocherdr_files::EntryKind;
use ochub_ui::scrollbar::contain_vertical_scroll;
use std::path::Path;

impl OcHerdrView {
    pub(super) fn render_file_panel(
        &mut self,
        overlay: bool,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let width = self.file_panel.width;
        let drop_hint = self.i18n.text(k::FILES_UPLOAD_DROP);
        let mut panel = div()
            .id("file-panel")
            .debug_selector(|| "file-panel".to_owned())
            .group("file-panel-drop")
            .role(ochub_ui::gpui::Role::Region)
            .aria_label(self.i18n.text(k::FILES_PANEL))
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .min_h_0()
            .w(px(width))
            .h_full()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::content_background())
            .occlude()
            .can_drop(|value, _, _| value.downcast_ref::<ExternalPaths>().is_some())
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style
                    .bg(theme::accent().alpha(0.08))
                    .border_color(theme::accent())
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                this.file_panel_upload_paths(paths.paths().to_vec(), cx);
            }))
            .child(
                div()
                    .id("file-panel-resize")
                    .absolute()
                    .left(px(-3.))
                    .top_0()
                    .bottom_0()
                    .w(px(7.))
                    .cursor_col_resize()
                    .occlude()
                    .hover(|style| style.bg(theme::accent().alpha(0.45)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event, _window, cx| {
                            this.begin_file_panel_resize(event, cx);
                        }),
                    ),
            )
            .child(self.render_file_panel_header(cx))
            .child(self.render_file_panel_toolbar(cx))
            .child(self.render_file_panel_breadcrumb(cx))
            .when(
                !matches!(self.file_panel.prompt, FilePanelPrompt::None),
                |panel| panel.child(self.render_file_panel_prompt(cx)),
            )
            .child(self.render_file_tree(drop_hint, cx))
            .child(self.render_file_panel_status());
        if overlay {
            panel = panel
                .absolute()
                .right_0()
                .top_0()
                .bottom_0()
                .shadow(theme::shadow_popover());
        }
        panel.into_any_element()
    }

    fn render_file_panel_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let pinned = self.file_panel.pinned;
        let backend = match self.file_panel.backend_kind {
            Some(FileBackendKind::Sftp) => i18n.text(k::FILES_BACKEND_REMOTE),
            Some(FileBackendKind::Local) | None => i18n.text(k::FILES_BACKEND_LOCAL),
        };
        div()
            .flex()
            .items_center()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .gap_2()
            .pl_3()
            .pr_2()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .child(icon(IconName::Folder, theme::accent(), 14.))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .child(i18n.text(k::FILES_PANEL)),
                    )
                    .child(div().text_xs().text_color(theme::muted()).child(backend)),
            )
            .child(
                button(
                    "file-panel-pin",
                    if pinned {
                        i18n.text(k::FILES_FOLLOW)
                    } else {
                        i18n.text(k::FILES_PIN)
                    },
                    if pinned {
                        ButtonTone::Primary
                    } else {
                        ButtonTone::Ghost
                    },
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.toggle_file_panel_pin(cx);
                })),
            )
            .child(icon_action_tooltip(
                "close-file-panel-tooltip",
                i18n.text(k::COMMON_CLOSE),
                icon_only_button_tone(
                    "close-file-panel",
                    i18n.text(k::COMMON_CLOSE),
                    IconName::Close,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.close_file_panel(cx))),
            ))
    }

    fn render_file_panel_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let show_hidden = self.file_panel.show_hidden;
        div()
            .id("file-panel-toolbar")
            .role(ochub_ui::gpui::Role::Toolbar)
            .aria_label(i18n.text(k::FILES_ACTIONS))
            .flex()
            .items_center()
            .justify_between()
            .h(px(38.))
            .flex_none()
            .px_2()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(icon_action_tooltip(
                        "file-new-file-tooltip",
                        i18n.text(k::FILES_NEW_FILE),
                        icon_only_button_tone(
                            "file-new-file",
                            i18n.text(k::FILES_NEW_FILE),
                            IconName::Add,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_create_file_prompt(window, cx);
                        })),
                    ))
                    .child(icon_action_tooltip(
                        "file-new-folder-tooltip",
                        i18n.text(k::FILES_NEW_FOLDER),
                        icon_only_button_tone(
                            "file-new-folder",
                            i18n.text(k::FILES_NEW_FOLDER),
                            IconName::Folder,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_create_directory_prompt(window, cx);
                        })),
                    ))
                    .child(icon_action_tooltip(
                        "file-upload-tooltip",
                        i18n.text(k::FILES_UPLOAD),
                        icon_only_button_tone(
                            "file-upload",
                            i18n.text(k::FILES_UPLOAD),
                            IconName::Cloud,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.choose_file_panel_upload(cx);
                        })),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(icon_action_tooltip(
                        "file-hidden-tooltip",
                        if show_hidden {
                            i18n.text(k::FILES_HIDDEN_HIDE)
                        } else {
                            i18n.text(k::FILES_HIDDEN_SHOW)
                        },
                        icon_only_button_tone(
                            "file-hidden",
                            if show_hidden {
                                i18n.text(k::FILES_HIDDEN_HIDE)
                            } else {
                                i18n.text(k::FILES_HIDDEN_SHOW)
                            },
                            if show_hidden {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            },
                            if show_hidden {
                                ButtonTone::Primary
                            } else {
                                ButtonTone::Ghost
                            },
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.toggle_file_panel_hidden(cx);
                        })),
                    ))
                    .child(icon_action_tooltip(
                        "file-refresh-tooltip",
                        i18n.text(k::FILES_REFRESH),
                        icon_only_button_tone(
                            "file-refresh",
                            i18n.text(k::FILES_REFRESH),
                            IconName::Refresh,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.refresh_file_panel(cx);
                        })),
                    )),
            )
    }

    fn render_file_panel_breadcrumb(
        &mut self,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        if self.file_panel.address_editing {
            let loading = self.file_panel.address_task.is_some();
            let error = self.file_panel.address_error.clone();
            return div()
                .flex()
                .flex_col()
                .flex_none()
                .border_b_1()
                .border_color(theme::border())
                .bg(theme::surface())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(38.))
                        .min_w_0()
                        .gap_1()
                        .px_2()
                        .child(div().min_w_0().flex_1().child(self.file_path_input.clone()))
                        .child(if loading {
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(28.))
                                .child(spinner(theme::muted(), 12.))
                                .into_any_element()
                        } else {
                            icon_only_button_tone(
                                "file-address-submit",
                                self.i18n.text(k::FILES_PATH_GO),
                                IconName::Check,
                                ButtonTone::Primary,
                                ButtonSize::Sm,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_file_panel_address(window, cx);
                            }))
                            .into_any_element()
                        })
                        .child(
                            icon_only_button_tone(
                                "file-address-cancel",
                                self.i18n.text(k::COMMON_CANCEL),
                                IconName::Close,
                                ButtonTone::Ghost,
                                ButtonSize::Sm,
                            )
                            .opacity(if loading { 0.35 } else { 1. })
                            .when(!loading, |button| {
                                button.on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_file_panel_address(window, cx);
                                }))
                            }),
                        ),
                )
                .when_some(error, |bar, error| {
                    bar.child(
                        div()
                            .px_3()
                            .pb_2()
                            .text_xs()
                            .text_color(theme::red())
                            .whitespace_normal()
                            .child(error),
                    )
                })
                .into_any_element();
        }
        let root = self.file_panel.root.clone();
        let crumbs = root.as_deref().map(breadcrumb_paths).unwrap_or_default();
        let has_parent = root.as_deref().and_then(std::path::Path::parent).is_some();
        div()
            .flex()
            .items_center()
            .h(px(34.))
            .flex_none()
            .min_w_0()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                icon_only_button_tone(
                    "file-up",
                    self.i18n.text(k::FILES_PARENT),
                    IconName::ChevronLeft,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .opacity(if has_parent { 1. } else { 0.35 })
                .when(has_parent, |button| {
                    button.on_click(cx.listener(|this, _, _window, cx| {
                        this.navigate_file_panel_up(cx);
                    }))
                }),
            )
            .child(
                div()
                    .id("file-breadcrumb-scroll")
                    .flex()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(
                        crumbs
                            .into_iter()
                            .enumerate()
                            .flat_map(|(index, (label, path))| {
                                let mut elements = Vec::new();
                                if index > 0 {
                                    elements.push(
                                        icon(IconName::ChevronRight, theme::muted(), 9.)
                                            .into_any_element(),
                                    );
                                }
                                elements.push(
                                    div()
                                        .id(("file-crumb", index))
                                        .flex_none()
                                        .max_w(px(112.))
                                        .truncate()
                                        .rounded(px(CORNER_COMPACT))
                                        .px_1()
                                        .py(px(2.))
                                        .text_xs()
                                        .text_color(theme::subtext())
                                        .hover(|style| {
                                            style
                                                .bg(theme::surface_hover())
                                                .text_color(theme::text())
                                        })
                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                            this.navigate_file_panel(path.clone(), cx);
                                        }))
                                        .child(label)
                                        .into_any_element(),
                                );
                                elements
                            }),
                    ),
            )
            .child(icon_action_tooltip(
                "file-address-edit-tooltip",
                self.i18n.text(k::FILES_PATH_EDIT),
                icon_only_button_tone(
                    "file-address-edit",
                    self.i18n.text(k::FILES_PATH_EDIT),
                    IconName::Pencil,
                    ButtonTone::Ghost,
                    ButtonSize::Sm,
                )
                .debug_selector(|| "file-address-edit".into())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_file_panel_address(window, cx);
                })),
            ))
            .into_any_element()
    }

    fn render_file_panel_prompt(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let i18n = self.i18n;
        let prompt = self.file_panel.prompt.clone();
        let busy = self.file_panel.busy.is_some();
        let error = self.file_panel.prompt_error.clone().map(SharedString::from);
        match prompt {
            FilePanelPrompt::None => div().into_any_element(),
            FilePanelPrompt::CreateFile { .. }
            | FilePanelPrompt::CreateDirectory { .. }
            | FilePanelPrompt::Rename { .. } => {
                let (label, action) = match prompt {
                    FilePanelPrompt::CreateFile { .. } => {
                        (i18n.text(k::FILES_NEW_FILE), i18n.text(k::FILES_CREATE))
                    }
                    FilePanelPrompt::CreateDirectory { .. } => {
                        (i18n.text(k::FILES_NEW_FOLDER), i18n.text(k::FILES_CREATE))
                    }
                    FilePanelPrompt::Rename { .. } => {
                        (i18n.text(k::FILES_RENAME), i18n.text(k::COMMON_SAVE))
                    }
                    _ => unreachable!(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(theme::border())
                    .bg(theme::inset())
                    .child(field_with_error(
                        label,
                        true,
                        None,
                        error,
                        self.file_name_input.clone(),
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                button(
                                    "file-prompt-cancel",
                                    i18n.text(k::COMMON_CANCEL),
                                    ButtonTone::Ghost,
                                    ButtonSize::Sm,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.cancel_file_prompt(window, cx);
                                    },
                                )),
                            )
                            .child(if busy {
                                busy_button(
                                    "file-prompt-submit",
                                    action,
                                    ButtonTone::Primary,
                                    ButtonSize::Sm,
                                    true,
                                )
                                .into_any_element()
                            } else {
                                button(
                                    "file-prompt-submit",
                                    action,
                                    ButtonTone::Primary,
                                    ButtonSize::Sm,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_file_prompt(window, cx);
                                }))
                                .into_any_element()
                            }),
                    )
                    .into_any_element()
            }
            FilePanelPrompt::ConfirmDelete { entry } => {
                let body = if self.file_panel.backend_kind == Some(FileBackendKind::Local) {
                    crate::tf!(i18n, k::FILES_DELETE_LOCAL_BODY, name = entry.name)
                } else {
                    crate::tf!(i18n, k::FILES_DELETE_REMOTE_BODY, name = entry.name)
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(theme::border())
                    .bg(theme::inset())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .child(i18n.text(k::FILES_DELETE_TITLE)),
                    )
                    .child(div().text_xs().text_color(theme::subtext()).child(body))
                    .when_some(error, |panel, error| {
                        panel.child(div().text_xs().text_color(theme::red()).child(error))
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                button(
                                    "file-delete-cancel",
                                    i18n.text(k::COMMON_CANCEL),
                                    ButtonTone::Ghost,
                                    ButtonSize::Sm,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.cancel_file_prompt(window, cx);
                                    },
                                )),
                            )
                            .child(if busy {
                                busy_button(
                                    "file-delete-confirm",
                                    i18n.text(k::FILES_DELETE),
                                    ButtonTone::Danger,
                                    ButtonSize::Sm,
                                    true,
                                )
                                .into_any_element()
                            } else {
                                button(
                                    "file-delete-confirm",
                                    i18n.text(k::FILES_DELETE),
                                    ButtonTone::Danger,
                                    ButtonSize::Sm,
                                )
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.confirm_file_delete(cx);
                                }))
                                .into_any_element()
                            }),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_file_tree(
        &mut self,
        drop_hint: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let rows = self.file_panel.rows();
        let selected = self
            .file_panel
            .selected
            .as_ref()
            .map(|entry| entry.path.clone());
        let root_loading = self.file_panel.root_task.is_some();
        let root = self.file_panel.root.clone();
        let root_listed = root
            .as_ref()
            .is_some_and(|root| self.file_panel.children.contains_key(root));
        let error = self.file_panel.error.clone();
        let scroll = self.file_panel.tree_scroll.clone();
        let body = if root_loading {
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .flex_1()
                .text_xs()
                .text_color(theme::muted())
                .child(spinner(theme::muted(), 13.))
                .child(i18n.text(k::FILES_STATUS_CONNECTING))
                .into_any_element()
        } else if let Some(error) = error {
            div()
                .flex()
                .flex_col()
                .items_start()
                .justify_center()
                .gap_3()
                .flex_1()
                .p_4()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::red())
                        .whitespace_normal()
                        .child(error),
                )
                .child(
                    button(
                        "file-retry",
                        i18n.text(k::FILES_RETRY),
                        ButtonTone::Neutral,
                        ButtonSize::Sm,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.refresh_file_panel(cx);
                    })),
                )
                .into_any_element()
        } else if rows.is_empty() && root_listed {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .flex_1()
                .p_4()
                .child(icon(IconName::Folder, theme::muted(), 20.))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::text())
                        .child(i18n.text(k::FILES_EMPTY_TITLE)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(theme::muted())
                        .child(i18n.text(k::FILES_EMPTY_BODY)),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .min_h_0()
                .children(rows.into_iter().enumerate().map(|(index, row)| {
                    let entry = row.entry.clone();
                    let context_entry = entry.clone();
                    let is_selected = selected.as_ref() == Some(&entry.path);
                    let expanded = row.expanded;
                    let size = entry.size.map(human_file_size);
                    let icon_name = match entry.kind {
                        EntryKind::Directory => IconName::Folder,
                        EntryKind::Symlink => IconName::Layers,
                        EntryKind::File => IconName::Code,
                        EntryKind::Other => IconName::Diamond,
                    };
                    let icon_color = if entry.kind == EntryKind::Directory {
                        theme::accent()
                    } else {
                        theme::muted()
                    };
                    div()
                        .id(("file-row", index))
                        .debug_selector(move || format!("file-row-{index}"))
                        .role(ochub_ui::gpui::Role::TreeItem)
                        .aria_label(entry.name.clone())
                        .aria_selected(is_selected)
                        .when(entry.kind == EntryKind::Directory, |row| {
                            row.aria_expanded(expanded)
                        })
                        .flex()
                        .items_center()
                        .h(px(28.))
                        .flex_none()
                        .pr_2()
                        .pl(px(8. + row.depth as f32 * 16.))
                        .gap_1()
                        .text_xs()
                        .text_color(if is_selected {
                            theme::text()
                        } else {
                            theme::subtext()
                        })
                        .bg(if is_selected {
                            theme::sidebar_selected()
                        } else {
                            theme::surface().alpha(0.)
                        })
                        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                            this.activate_file_entry(entry.clone(), event.click_count() >= 2, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event, window, cx| {
                                this.open_file_context_menu(
                                    context_entry.clone(),
                                    event,
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .child(if row.loading {
                            spinner(theme::muted(), 10.).into_any_element()
                        } else if row.entry.kind == EntryKind::Directory {
                            icon(
                                if row.expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                theme::muted(),
                                10.,
                            )
                            .into_any_element()
                        } else {
                            div().w(px(10.)).into_any_element()
                        })
                        .child(icon(icon_name, icon_color, 12.))
                        .child(div().min_w_0().flex_1().truncate().child(row.entry.name))
                        .when_some(size, |row, size| {
                            row.child(div().flex_none().text_color(theme::muted()).child(size))
                        })
                }))
                .into_any_element()
        };
        div()
            .id("file-tree-scroll")
            .debug_selector(|| "file-tree-scroll".to_owned())
            .role(ochub_ui::gpui::Role::Tree)
            .aria_label(i18n.text(k::FILES_PANEL))
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .on_scroll_wheel(contain_vertical_scroll(scroll.clone()))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .child(body)
            .child(
                div()
                    .absolute()
                    .left_2()
                    .right_2()
                    .bottom_2()
                    .rounded(px(CORNER_COMPACT))
                    .bg(theme::overlay().alpha(0.92))
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_center()
                    .text_color(theme::muted())
                    .opacity(0.)
                    .group_drag_over::<ExternalPaths>("file-panel-drop", |style| style.opacity(1.))
                    .child(drop_hint),
            )
    }

    fn render_file_panel_status(&self) -> impl IntoElement {
        let busy = self.file_panel.busy.map(|busy| match busy {
            FileBusyKind::Creating => self.i18n.text(k::FILES_BUSY_CREATING),
            FileBusyKind::Opening => self.i18n.text(k::FILES_BUSY_OPENING),
            FileBusyKind::Renaming => self.i18n.text(k::FILES_BUSY_RENAMING),
            FileBusyKind::Removing => self.i18n.text(k::FILES_BUSY_REMOVING),
            FileBusyKind::Uploading => self.i18n.text(k::FILES_BUSY_UPLOADING),
            FileBusyKind::Downloading => self.i18n.text(k::FILES_BUSY_DOWNLOADING),
        });
        let text = busy
            .map(str::to_owned)
            .or_else(|| self.file_panel.status.clone())
            .unwrap_or_default();
        div()
            .flex()
            .items_center()
            .h(px(24.))
            .flex_none()
            .gap_2()
            .px_3()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .text_xs()
            .text_color(theme::muted())
            .when(busy.is_some(), |status| {
                status.child(spinner(theme::muted(), 10.))
            })
            .child(div().min_w_0().truncate().child(text))
    }

    pub(super) fn render_file_context_menu(
        &mut self,
        menu: FileContextMenu,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        let mut items = Vec::new();
        if menu.entry.kind.is_directory() {
            items.push(
                context_menu_item(
                    "file-menu-open-folder",
                    i18n.text(k::FILES_OPEN_FOLDER),
                    None::<&str>,
                    Some(IconName::Folder),
                    false,
                )
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.open_selected_file_panel_directory(cx);
                }))
                .into_any_element(),
            );
        } else if menu.entry.kind.is_file() {
            let editor = self
                .file_panel
                .editor
                .as_deref()
                .map(file_editor_name)
                .unwrap_or_else(|| i18n.text(k::FILES_EDITOR_SYSTEM).to_owned());
            items.push(
                context_menu_item(
                    "file-menu-open",
                    crate::tf!(i18n, k::FILES_EDITOR_CURRENT, editor = editor),
                    None::<&str>,
                    Some(IconName::Code),
                    false,
                )
                .debug_selector(|| "file-menu-open".into())
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.close_context_menu(cx);
                    this.open_file_panel_entry(cx);
                }))
                .into_any_element(),
            );
            items.push(
                context_menu_item(
                    "file-menu-choose-editor",
                    i18n.text(k::FILES_EDITOR_CHOOSE),
                    None::<&str>,
                    Some(IconName::Pencil),
                    false,
                )
                .debug_selector(|| "file-menu-choose-editor".into())
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.choose_file_panel_editor(cx);
                }))
                .into_any_element(),
            );
            if self.file_panel.editor.is_some() {
                items.push(
                    context_menu_item(
                        "file-menu-system-editor",
                        i18n.text(k::FILES_EDITOR_USE_SYSTEM),
                        None::<&str>,
                        Some(IconName::Refresh),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.use_system_file_panel_editor(cx);
                    }))
                    .into_any_element(),
                );
            }
        }
        items.push(
            div()
                .h(px(1.))
                .mx_1()
                .my_1()
                .bg(theme::border())
                .into_any_element(),
        );
        items.push(
            context_menu_item(
                "file-menu-download",
                i18n.text(k::FILES_DOWNLOAD),
                None::<&str>,
                Some(IconName::Archive),
                false,
            )
            .on_click(cx.listener(|this, _, _window, cx| {
                this.close_context_menu(cx);
                this.choose_file_panel_download(cx);
            }))
            .into_any_element(),
        );
        items.push(
            context_menu_item(
                "file-menu-copy-path",
                i18n.text(k::FILES_COPY_PATH),
                None::<&str>,
                Some(IconName::Copy),
                false,
            )
            .on_click(cx.listener(|this, _, _window, cx| {
                this.close_context_menu(cx);
                this.copy_file_panel_path(cx);
            }))
            .into_any_element(),
        );
        items.push(
            context_menu_item(
                "file-menu-insert-path",
                i18n.text(k::FILES_INSERT_PATH),
                None::<&str>,
                Some(IconName::Terminal),
                false,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.close_context_menu(cx);
                this.insert_file_panel_path(window, cx);
            }))
            .into_any_element(),
        );
        items.push(
            context_menu_item(
                "file-menu-rename",
                i18n.text(k::FILES_RENAME),
                None::<&str>,
                Some(IconName::Pencil),
                false,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.close_context_menu(cx);
                this.open_file_rename_prompt(window, cx);
            }))
            .into_any_element(),
        );
        items.push(
            context_menu_item(
                "file-menu-delete",
                i18n.text(k::FILES_DELETE),
                None::<&str>,
                Some(IconName::Trash),
                true,
            )
            .on_click(cx.listener(|this, _, _window, cx| {
                this.close_context_menu(cx);
                this.request_file_delete(cx);
            }))
            .into_any_element(),
        );
        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close_context_menu(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| this.close_context_menu(cx)),
            )
            .child(
                context_menu("file-context-menu", items)
                    .absolute()
                    .left(px(menu.x))
                    .top(px(menu.y)),
            )
            .into_any_element()
    }

    pub(super) fn render_file_panel_resize_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        div()
            .id("file-panel-resize-overlay")
            .absolute()
            .inset_0()
            .cursor_col_resize()
            .on_mouse_move(cx.listener(|this, event, _window, cx| {
                this.file_panel_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.file_panel_mouse_up(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.file_panel_mouse_up(cx);
                }),
            )
            .into_any_element()
    }
}

fn file_editor_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
