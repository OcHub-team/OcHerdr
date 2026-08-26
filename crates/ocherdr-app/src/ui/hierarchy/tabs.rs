use super::*;

impl OcHerdrView {
    pub(crate) fn render_tab_bar(
        &mut self,
        chrome: &ChromeA11y,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let view = cx.entity();
        let tab_count = chrome.tabs.items.len();
        let authoritative_order = chrome
            .tabs
            .items
            .iter()
            .map(|row| row.a11y.id.clone())
            .collect::<Vec<_>>();
        if self
            .hovered_tab_id
            .as_ref()
            .is_some_and(|hovered| !authoritative_order.contains(hovered))
        {
            self.hovered_tab_id = None;
        }
        if self
            .tab_preview_id
            .as_ref()
            .is_some_and(|id| !authoritative_order.contains(id))
        {
            self.dismiss_tab_preview();
        }
        self.tab_close_reveals
            .retain(|tab_id, _| authoritative_order.contains(tab_id));
        let now = Instant::now();
        let reduce_motion = cx.reduce_motion();
        let close_reveals = authoritative_order
            .iter()
            .map(|tab_id| {
                let target = if self.hovered_tab_id.as_deref() == Some(tab_id.as_str()) {
                    1.
                } else {
                    0.
                };
                let reveal = self
                    .tab_close_reveals
                    .entry(tab_id.clone())
                    .or_insert_with(|| Transition::settled(0., TAB_CLOSE_ANIMATION));
                reveal.retarget(target, now, reduce_motion);
                if reveal.is_animating(now, reduce_motion) {
                    window.request_animation_frame();
                }
                reveal.value(now, reduce_motion)
            })
            .collect::<Vec<_>>();
        self.shortcut_reveal
            .retarget(if self.command_held { 1. } else { 0. }, now, reduce_motion);
        if self.shortcut_reveal.is_animating(now, reduce_motion) {
            window.request_animation_frame();
        }
        let shortcut_reveal = self.shortcut_reveal.value(now, reduce_motion);
        let drag = match &self.surface_drag {
            SurfaceDrag::Reorder(drag) => Some(drag),
            _ => None,
        };
        let pane_tab_drop = match &self.surface_drag {
            SurfaceDrag::Pane(drag) if drag.tab_bar_drops => Some(drag.clone()),
            _ => None,
        };
        let pending = self
            .pending_reorder
            .as_ref()
            .and_then(|pending| pending.display.as_ref());
        let projection = self
            .selection
            .workspace_id
            .as_deref()
            .and_then(|workspace_id| {
                reorder_projection(
                    &ReorderList::Tabs {
                        workspace_id: workspace_id.to_owned(),
                    },
                    &authoritative_order,
                    drag,
                    pending,
                )
            });
        let tab_reorder = projection.and_then(|projection| {
            let rects = authoritative_order
                .iter()
                .map(|id| {
                    self.reorder_metrics
                        .tabs
                        .iter()
                        .find(|span| &span.id == id)
                        .map(|span| span.rect)
                })
                .collect::<Option<Vec<_>>>()?;
            let offsets = reorder_slot_offsets(
                projection.source_index,
                projection.motion,
                &projection.positions,
                &projection.previous_positions,
                &rects,
                TAB_REORDER_GAP_PX,
                ReorderAxis::Horizontal,
            );
            Some((
                projection.source_id,
                projection.positions,
                offsets.previous,
                offsets.current,
                projection.motion,
            ))
        });
        let tabs = chrome
            .tabs
            .items
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let shortcut = tab_key_equivalent(index, tab_count);
                let close_reveal = close_reveals[index];
                let tab_id = row.a11y.id.clone();
                let press_id = tab_id.clone();
                let measure_id = tab_id.clone();
                let hover_id = tab_id.clone();
                let move_hover_id = tab_id.clone();
                let debug_tab_id = tab_id.clone();
                let debug_title_id = tab_id.clone();
                let debug_close_id = tab_id.clone();
                let debug_fade_id = tab_id.clone();
                let debug_shortcut_id = tab_id.clone();
                let measure_view = view.clone();
                let tab_hover_group: SharedString = format!("tab-hover-{tab_id}").into();
                let tab_target = HierarchyTarget::Tab {
                    id: row.a11y.id.clone(),
                    label: row.a11y.name.clone(),
                };
                let close_target = tab_target.clone();
                let selected = row.a11y.selected == Some(true);
                let existing_drop =
                    pane_tab_drop
                        .as_ref()
                        .and_then(|drag| match &drag.tab_target {
                            Some(PaneTabDropTarget::Existing { tab_id: hit, .. })
                                if hit == &tab_id =>
                            {
                                Some(row.a11y.name.clone())
                            }
                            _ => None,
                        });
                let existing_hovered = existing_drop.is_some();
                let drop_pill_id = tab_id.clone();
                let drop_move_id = tab_id.clone();
                let drop_debug_id = tab_id.clone();
                let title_needs_fade = Self::tab_title_needs_fade(&row.a11y.name, window);
                let fade_background = if selected {
                    theme::current().bg.rgba()
                } else {
                    theme::sidebar_background()
                };
                let fade_hover_background = if selected {
                    theme::current().bg.rgba()
                } else {
                    theme::surface_hover()
                };
                let (hidden, display_position, previous_offset, offset, motion) =
                    if let Some((source_id, positions, previous_offsets, offsets, motion)) =
                        &tab_reorder
                    {
                        (
                            source_id == &tab_id && *motion == ReorderMotion::Dragging,
                            positions[index],
                            previous_offsets[index],
                            offsets[index],
                            Some(*motion),
                        )
                    } else {
                        (false, index, (0., 0.), (0., 0.), None)
                    };
                let tab = apply_control(div().id(("main-tab", row.number)), &row.a11y)
                    // The hint is visual only; assistive tech hears the
                    // shortcut whether or not Command is down.
                    .when_some(shortcut.as_deref(), |tab, shortcut| {
                        tab.aria_label(format!("{}, {shortcut}", row.a11y.name))
                    })
                    .relative()
                    .flex()
                    .items_center()
                    .flex_none()
                    .h(px(TAB_PILL_HEIGHT))
                    .w(px(TAB_PILL_WIDTH))
                    .overflow_hidden()
                    .rounded_full()
                    .border_1()
                    .border_color(if existing_hovered {
                        theme::accent()
                    } else if selected {
                        theme::border()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .bg(if existing_hovered {
                        theme::accent().alpha(0.16)
                    } else if selected {
                        theme::current().bg.rgba()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .text_sm()
                    .text_color(if selected {
                        theme::text()
                    } else {
                        theme::muted()
                    })
                    .hover(move |style| {
                        style.bg(if existing_hovered {
                            theme::accent().alpha(0.16)
                        } else if selected {
                            theme::current().bg.rgba()
                        } else {
                            theme::surface_hover()
                        })
                    })
                    .when(tab_count >= 2, |tab| tab.cursor_grab())
                    .when(tab_count < 2, |tab| tab.cursor_pointer())
                    .group(tab_hover_group.clone())
                    .opacity(if hidden { 0. } else { 1. })
                    .debug_selector(move || format!("tab-{debug_tab_id}"))
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        this.set_tab_hovered(hover_id.clone(), *hovered, cx);
                        if !*hovered {
                            let pointer = match &this.surface_drag {
                                SurfaceDrag::Pane(drag) => drag.pointer,
                                _ => return,
                            };
                            if matches!(
                                &this.surface_drag,
                                SurfaceDrag::Pane(drag)
                                    if matches!(
                                        &drag.tab_target,
                                        Some(PaneTabDropTarget::Existing { tab_id, .. })
                                            if tab_id == &drop_pill_id
                                    )
                            ) {
                                this.clear_pane_tab_drop_target(pointer, cx);
                            }
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, _window, cx| {
                            this.press_tab_pill(press_id.clone(), event, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| {
                            this.pane_mouse_up(event, window, cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| {
                            this.pane_mouse_up(event, window, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                        this.set_tab_hovered(move_hover_id.clone(), true, cx);
                        let pointer =
                            (f32::from(event.position.x), f32::from(event.position.y));
                        if this.update_pane_drag_over_tab_pill(
                            pointer,
                            drop_move_id.clone(),
                            cx,
                        ) {
                            cx.stop_propagation();
                            return;
                        }
                        this.pane_mouse_move(event, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event, window, cx| {
                            this.open_context_menu(tab_target.clone(), event, window, cx)
                        }),
                    )
                    .child(
                        div()
                            .id(("tab-title", row.number))
                            .flex_1()
                            .min_w_0()
                            .px(px(TAB_TITLE_ACTION_WELL))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_overflow(TextOverflow::Truncate("".into()))
                            .text_center()
                            .debug_selector(move || format!("tab-title-{debug_title_id}"))
                            .child(row.a11y.name.clone()),
                    )
                    // Absolutely positioned, so showing or hiding the hint
                    // never moves the title.
                    .when_some(shortcut, |tab_row, shortcut| {
                        tab_row.child(
                            div()
                                .absolute()
                                .right(px(10.))
                                .top_0()
                                .bottom_0()
                                .flex()
                                .items_center()
                                .opacity(shortcut_reveal)
                                .when(shortcut_reveal <= f32::EPSILON, |hint| hint.invisible())
                                .when(shortcut_reveal > f32::EPSILON, |hint| {
                                    hint.debug_selector(move || {
                                        format!("tab-shortcut-{debug_shortcut_id}")
                                    })
                                })
                                .text_xs()
                                .text_color(if selected {
                                    theme::text()
                                } else {
                                    theme::muted()
                                })
                                .child(shortcut),
                        )
                    })
                    .when(title_needs_fade, |tab_row| {
                        tab_row.child(
                            div()
                                .debug_selector(move || format!("tab-title-fade-{debug_fade_id}"))
                                .absolute()
                                .right(px(TAB_TITLE_ACTION_WELL))
                                .top(px(1.))
                                .bottom(px(1.))
                                .w(px(TAB_TITLE_FADE_WIDTH))
                                .bg(linear_gradient(
                                    90.,
                                    linear_color_stop(fade_background.alpha(0.), 0.),
                                    linear_color_stop(fade_background, 1.),
                                ))
                                .group_hover(tab_hover_group, move |style| {
                                    style.bg(linear_gradient(
                                        90.,
                                        linear_color_stop(fade_hover_background.alpha(0.), 0.),
                                        linear_color_stop(fade_hover_background, 1.),
                                    ))
                                }),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(7.))
                            .top_0()
                            .bottom_0()
                            .flex()
                            .items_center()
                            .opacity(close_reveal)
                            .when(close_reveal <= f32::EPSILON, |close| close.invisible())
                            .child(
                                icon_only_button_tone(
                                    ("close-tab", row.number),
                                    i18n.text(k::TERMINAL_CLOSE_TAB),
                                    IconName::Close,
                                    ButtonTone::Ghost,
                                    ButtonSize::Sm,
                                )
                                .size(px(18.))
                                .rounded_full()
                                .debug_selector(move || format!("close-tab-{debug_close_id}"))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(
                                    move |this, _, _window, cx| {
                                        this.request_close(close_target.clone(), cx)
                                    },
                                )),
                            ),
                    )
                    .when_some(existing_drop, |tab, name| {
                        let hint = i18n.drop_move_into_tab(&name);
                        tab.aria_label(i18n.drop_move_to_tab(&name)).child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .px_2()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme::accent_text())
                                .debug_selector(move || {
                                    format!("pane-tab-drop-existing-{drop_debug_id}")
                                })
                                .child(icon(IconName::Blocks, theme::accent_text(), 11.))
                                .child(hint),
                        )
                    });
                let tab = if tab_reorder.is_some() {
                    let animation_name = match motion {
                        Some(ReorderMotion::Dragging) => "reorder-shift",
                        Some(ReorderMotion::Settling { .. }) => "reorder-settle",
                        None => unreachable!("tab reorder animation requires a motion phase"),
                    };
                    tab.with_animation(
                        (
                            ElementId::named_usize(animation_name, display_position),
                            tab_id.clone(),
                        ),
                        Animation::new(REORDER_ANIMATION).with_easing(ease_out_quint()),
                        move |tab, delta| {
                            tab.left(px(
                                previous_offset.0 + (offset.0 - previous_offset.0) * delta
                            ))
                            .top(px(
                                previous_offset.1 + (offset.1 - previous_offset.1) * delta
                            ))
                        },
                    )
                    .into_any_element()
                } else {
                    tab.into_any_element()
                };
                div()
                    .id((ElementId::from("tab-slot"), tab_id))
                    .relative()
                    .flex_none()
                    .child(tab)
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                measure_view.update(cx, |this, _cx| {
                                    this.note_reorder_span(
                                        true,
                                        measure_id.clone(),
                                        (
                                            f32::from(bounds.origin.x),
                                            f32::from(bounds.origin.y),
                                            f32::from(bounds.size.width),
                                            f32::from(bounds.size.height),
                                        ),
                                    );
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        // Inset auto would use the static position below the
                        // in-flow pill instead of the slot origin.
                        .top(px(0.))
                        .left(px(0.))
                        .size_full(),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let pane_id_right = self.selection.pane_id.clone();
        let pane_id_down = self.selection.pane_id.clone();
        let pane_id_zoom = self.selection.pane_id.clone();
        let pane_id_close = self.selection.pane_id.clone();
        let node_manager_open = self.overlay.host_center();
        let new_tab_hover = pane_tab_drop
            .as_ref()
            .is_some_and(|drag| drag.tab_target == Some(PaneTabDropTarget::NewTab));
        div()
            .flex()
            .items_center()
            .h(px(HEADER_HEIGHT))
            .pr_2()
            .gap_1()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            // The strip's leading inset is a move area rather than padding so
            // the gutter left of the first tab also drags the window.
            .child(
                self.window_move_area("tab-strip-lead", cx)
                    .w(px(TAB_STRIP_LEAD_INSET)),
            )
            .child(
                apply_list(div().id(chrome.tabs.id), &chrome.tabs)
                    .flex()
                    .items_center()
                    .h_full()
                    .min_w_0()
                    .gap(px(TAB_REORDER_GAP_PX))
                    .overflow_x_scroll()
                    .overflow_y_hidden()
                    .track_scroll(&self.tab_scroll)
                    .children(tabs),
            )
            .when(pane_tab_drop.is_none(), |bar| {
                bar.child(
                    apply_control(
                        icon_only_button_tone(
                            "new-tab",
                            chrome.toolbar.new_tab.name.clone(),
                            IconName::Add,
                            ButtonTone::Ghost,
                            ButtonSize::Sm,
                        )
                        .rounded_full()
                        .debug_selector(|| "new-tab".to_owned()),
                        &chrome.toolbar.new_tab,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| this.create_tab(cx))),
                )
                // Everything between `+` and the toolbar is empty strip, so a
                // drag there means "move the window".
                .child(self.window_move_area("tab-strip-space", cx).flex_1())
            })
            .when(pane_tab_drop.is_some(), |bar| {
                bar.child(Self::render_new_tab_drop_zone(
                    new_tab_hover,
                    chrome.toolbar.new_tab.name.clone(),
                    i18n,
                    cx,
                ))
            })
            .child(div().id("pane-actions").role(ochub_ui::gpui::Role::Toolbar).aria_label(i18n.text(k::TERMINAL_PANE_ACTIONS)).flex().items_center().gap_1().px_2()
            .child(
                apply_control(
                    icon_only_button_tone(
                    "split-right",
                    chrome.toolbar.split_right.name.clone(),
                    IconName::Blocks,
                    ButtonTone::Primary,
                    ButtonSize::Sm,
                ),
                    &chrome.toolbar.split_right,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_right.clone() {
                        this.invoke(
                            "pane.split",
                            json!({ "target_pane_id": pane_id, "direction": SplitDirection::Right, "focus": true, "right_click": "herdr", "env": {} }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "split-down",
                        chrome.toolbar.split_down.name.clone(),
                        IconName::ChevronDown,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.split_down,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_down.clone() {
                        this.invoke(
                            "pane.split",
                            json!({ "target_pane_id": pane_id, "direction": SplitDirection::Down, "focus": true, "right_click": "herdr", "env": {} }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "zoom-pane",
                        chrome.toolbar.zoom.name.clone(),
                        IconName::Eye,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.zoom,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_zoom.clone() {
                        this.invoke(
                            "pane.zoom",
                            json!({ "pane_id": pane_id, "mode": "toggle" }),
                            cx,
                        )
                    }
                })),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "close-pane",
                        chrome.toolbar.close_pane.name.clone(),
                        IconName::Close,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.close_pane,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(pane_id) = pane_id_close.clone() {
                        let label = this
                            .snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.pane(&pane_id))
                            .map(PaneInfo::display_name)
                            .unwrap_or("pane")
                            .to_owned();
                        this.request_close(HierarchyTarget::Pane { id: pane_id, label }, cx)
                    }
                })),
            )
            )
            .child(div().h(px(22.)).w(px(1.)).bg(theme::border()))
            .child(
                apply_control(
                    icon_only_button_tone(
                        "open-appearance",
                        chrome.toolbar.appearance.name.clone(),
                        IconName::Palette,
                        if matches!(self.overlay, Overlay::Appearance) {
                            ButtonTone::Primary
                        } else {
                            ButtonTone::Ghost
                        },
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.appearance,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.open_appearance(cx))),
            )
            .child(
                apply_control(
                    icon_only_button_tone(
                        "open-herdr-settings",
                        chrome.toolbar.herdr_settings.name.clone(),
                        IconName::Settings,
                        ButtonTone::Ghost,
                        ButtonSize::Sm,
                    ),
                    &chrome.toolbar.herdr_settings,
                )
                .on_click(cx.listener(|this, _, _window, cx| this.open_native_tui(cx))),
            )
            .child(apply_control(
                icon_button_tone(
                    "manage-nodes",
                    chrome.toolbar.remote.name.clone(),
                    IconName::Globe,
                    if node_manager_open { ButtonTone::Primary } else { ButtonTone::Neutral },
                    ButtonSize::Sm,
                ),
                &chrome.toolbar.remote,
            ).mr_3().on_click(cx.listener(|this, _, _window, cx| this.open_node_manager(cx))))
    }

    fn render_new_tab_drop_zone(
        hovered: bool,
        new_tab_name: impl Into<SharedString>,
        i18n: I18n,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::Stateful<ochub_ui::gpui::Div> {
        let label = i18n.text(k::TERMINAL_DROP_NEW_TAB);
        let aria = i18n.text(k::TERMINAL_DROP_MOVE_TO_NEW_TAB);
        let new_tab_name = new_tab_name.into();
        div()
            .id("pane-tab-drop-new-tab")
            .debug_selector(|| "pane-tab-drop-new-tab".to_owned())
            .flex()
            .items_center()
            .flex_1()
            .h_full()
            .min_w_0()
            .gap_1()
            .px_1()
            .rounded(px(CORNER_CONTROL))
            .border_1()
            .border_color(if hovered {
                theme::accent()
            } else {
                theme::surface().alpha(0.)
            })
            .bg(if hovered {
                theme::accent().alpha(0.16)
            } else {
                theme::surface().alpha(0.)
            })
            .role(ochub_ui::gpui::Role::Button)
            .aria_label(aria)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let pointer = (f32::from(event.position.x), f32::from(event.position.y));
                if this.update_pane_drag_over_tab_target(pointer, PaneTabDropTarget::NewTab, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| {
                    this.pane_mouse_up(event, window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| {
                    this.pane_mouse_up(event, window, cx);
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    let pointer = match &this.surface_drag {
                        SurfaceDrag::Pane(drag) => drag.pointer,
                        _ => return,
                    };
                    this.clear_pane_tab_drop_target(pointer, cx);
                }
            }))
            .child(
                icon_only_button_tone(
                    "new-tab",
                    new_tab_name,
                    IconName::Add,
                    if hovered {
                        ButtonTone::Primary
                    } else {
                        ButtonTone::Ghost
                    },
                    ButtonSize::Sm,
                )
                .rounded_full()
                .debug_selector(|| "new-tab".to_owned())
                .on_click(cx.listener(|this, _, _window, cx| {
                    if matches!(this.surface_drag, SurfaceDrag::Pane(_)) {
                        return;
                    }
                    this.create_tab(cx);
                })),
            )
            .child(
                div()
                    .id("tab-strip-space")
                    .debug_selector(|| "tab-strip-space".to_owned())
                    .flex()
                    .items_center()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .gap_1()
                    .px_2()
                    .when(hovered, |space| {
                        space
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::accent_text())
                            .child(icon(IconName::Add, theme::accent_text(), 11.))
                            .child(
                                div()
                                    .id("pane-tab-drop-new-tab-hint")
                                    .debug_selector(|| "pane-tab-drop-new-tab-hint".to_owned())
                                    .child(label),
                            )
                    }),
            )
    }
}
