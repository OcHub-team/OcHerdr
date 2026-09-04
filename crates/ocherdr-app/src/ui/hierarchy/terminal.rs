use super::*;

impl OcHerdrView {
    pub(crate) fn render_status_bar(
        &mut self,
        chrome: &ChromeA11y,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let profile = self.current_profile();
        let profile_icon = if matches!(profile, ConnectionProfile::Local { .. }) {
            IconName::Desktop
        } else {
            IconName::Globe
        };
        let profile_label = profile_display_label(&profile, i18n);
        let switcher_open = matches!(self.overlay, Overlay::HostSwitcher);
        let status = if self.prefix_pending {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded(px(CORNER_COMPACT))
                        .bg(theme::accent_fill())
                        .text_color(theme::accent_text())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(i18n.text(k::TERMINAL_PREFIX)),
                )
                .child(i18n.text(k::TERMINAL_PREFIX_HINT))
                .into_any_element()
        } else if let Some(operation) = &self.operation {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(spinner(theme::muted(), 11.))
                .child(operation.clone())
                .into_any_element()
        } else if let EventStreamState::Lost(reason) = &self.event_stream {
            let message = event_stream_lost_copy(i18n);
            div()
                .id((
                    ochub_ui::gpui::ElementId::from("reconnect-live-updates"),
                    reason.clone(),
                ))
                .debug_selector(|| "reconnect-live-updates".into())
                // The enclosing `status-message` control already carries the button role
                // and the localized name; a nested one would announce twice.
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.reload(this.selection.session_name.clone(), cx);
                }))
                .child(status_dot(theme::red()))
                .child(message)
                .into_any_element()
        } else if let Some(snapshot) = &self.snapshot {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(theme::green()))
                .child(event_stream_status_copy(i18n, &self.event_stream, snapshot))
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_dot(theme::muted()))
                .child(i18n.text(k::TERMINAL_NO_SESSION))
                .into_any_element()
        };
        apply_region(div().id(chrome.status.id), &chrome.status)
            .flex()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::sidebar_background())
            .text_xs()
            .text_color(theme::muted())
            .child(
                apply_control(div().id("status-profile"), &chrome.status_profile)
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(px(SIDEBAR_WIDTH))
                    .h_full()
                    .px_3()
                    .border_r_1()
                    .border_color(theme::border())
                    .bg(if switcher_open {
                        theme::surface_hover()
                    } else {
                        theme::surface().alpha(0.)
                    })
                    .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| this.toggle_host_switcher(cx)))
                    .child(icon(profile_icon, theme::muted(), 13.))
                    .child(div().flex_1().min_w_0().truncate().child(profile_label)),
            )
            .child(
                apply_control(div().id("status-message"), &chrome.status_message)
                    .flex()
                    .items_center()
                    .min_w_0()
                    .px_3()
                    .child(status),
            )
    }

    pub(crate) fn render_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        let Some(snapshot) = self.snapshot.clone() else {
            let cta = button(
                "retry-empty",
                i18n.text(k::COMMON_REFRESH),
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, _window, cx| this.reload(None, cx)))
            .into_any_element();
            return div()
                .id("empty-terminals")
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(i18n.text(k::TERMINAL_NO_RUNNING_SESSION))
                .flex()
                .items_center()
                .justify_center()
                .flex_1()
                .bg(theme::content_background())
                .child(empty_state(
                    IconName::Terminal,
                    i18n.text(k::TERMINAL_NO_RUNNING_SESSION),
                    i18n.text(k::TERMINAL_NO_RUNNING_SESSION_BODY),
                    Some(cta),
                ))
                .into_any_element();
        };
        let Some(tab_id) = self.selection.tab_id.clone() else {
            return div()
                .id("empty-tabs")
                .role(ochub_ui::gpui::Role::Button)
                .tab_stop(false)
                .aria_label(i18n.text(k::TERMINAL_NO_TABS))
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(empty_state(
                    IconName::Layers,
                    i18n.text(k::TERMINAL_NO_TABS),
                    i18n.text(k::TERMINAL_NO_TABS_BODY),
                    None,
                ))
                .into_any_element();
        };
        let tab_id = tab_id.as_str();
        let layout = snapshot.layout_for(tab_id).cloned();
        let panes = self.rendered_panes_for_tab(&snapshot, tab_id);
        let view = cx.entity();
        let now = Instant::now();
        let reduce_motion = cx.reduce_motion();
        if self.expire_pane_motion(now, reduce_motion) {
            cx.notify();
        }
        if self.tab_resize_just_thawed(tab_id) {
            let pane_ids = panes
                .iter()
                .map(|pane| pane.pane_id.clone())
                .collect::<Vec<_>>();
            self.refresh_thawed_pane_bodies(&pane_ids, window, cx);
        }
        let scale = window.scale_factor();
        let pane_drag = match &self.surface_drag {
            SurfaceDrag::Pane(drag) if pane_drag_past_slop(drag) => Some(drag.clone()),
            _ => None,
        };
        if pane_drag
            .as_ref()
            .and_then(|drag| drag.layout_preview.as_ref())
            .is_some_and(|preview| preview.is_animating(now, reduce_motion))
        {
            window.request_animation_frame();
        }
        let keyboard_move = self
            .pane_keyboard_move
            .clone()
            .filter(|mode| mode.tab_id == tab_id);
        let drag_source_id = pane_drag
            .as_ref()
            .map(|drag| drag.pane_id.clone())
            .or_else(|| keyboard_move.as_ref().map(|mode| mode.pane_id.clone()));
        let draggable = layout.as_ref().is_some_and(|layout| {
            !layout.zoomed && (layout.panes.len() > 1 || self.pane_move_supported())
        }) && !self.tab_relocation_locked(tab_id);
        let mut elements = Vec::new();
        for pane in panes {
            if pane_drag
                .as_ref()
                .is_some_and(|drag| drag.tab_target.is_some() && drag.pane_id == pane.pane_id)
            {
                continue;
            }
            let fractions = self
                .displayed_pane_fractions(layout.as_ref(), &pane.pane_id, now, reduce_motion)
                .unwrap_or((0., 0., 1., 1.));
            let selected = self.selection.pane_id.as_deref() == Some(&pane.pane_id);
            let pane_id = pane.pane_id.clone();
            let pane_target = HierarchyTarget::Pane {
                id: pane.pane_id.clone(),
                label: pane.display_name().to_owned(),
            };
            let frame = self
                .pane(&pane.pane_id)
                .and_then(|runtime| runtime.frame.clone())
                .or_else(|| {
                    drag_source_id
                        .as_deref()
                        .filter(|source| *source == pane.pane_id)
                        .and_then(|_| self.pane_drag_snapshot.clone())
                })
                .or_else(|| {
                    self.pane_relocations
                        .get(tab_id)
                        .and_then(|pending| pending.plan.frame_for(&pane.pane_id))
                });
            let waiting = frame.is_none();
            let screen_text = if window.is_a11y_active() && !waiting {
                self.pane(&pane.pane_id)
                    .and_then(|runtime| runtime.terminal.read_visible_text())
            } else {
                None
            };
            let a11y = pane_a11y(&pane, selected, screen_text.as_deref(), waiting, i18n);
            // A frozen grid keeps its rendered size inside the clipped body so
            // the last frame stays put while only the shell moves.
            let frozen = if self.pane_resize_frozen(&pane.pane_id) {
                self.pane(&pane.pane_id).and_then(|runtime| {
                    (runtime.pixel_size.0 > 0 && runtime.pixel_size.1 > 0).then(|| {
                        (
                            runtime.pixel_size.0 as f32 / scale,
                            runtime.pixel_size.1 as f32 / scale,
                        )
                    })
                })
            } else {
                None
            };
            let handle = draggable.then(|| {
                let handle_pane_id = pane_id.clone();
                render_pane_drag_handle(&pane, selected, i18n)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event, window, cx| {
                            this.press_pane_handle(handle_pane_id.clone(), event, window, cx);
                        }),
                    )
                    .into_any_element()
            });
            let presentation = PanePresentation {
                fractions,
                source_slot: drag_source_id.as_deref() == Some(pane_id.as_str()),
                frozen,
                handle,
            };
            let scroll_pane_id = pane_id.clone();
            let left_mouse_pane_id = pane_id.clone();
            let move_mouse_pane_id = pane_id.clone();
            let right_mouse_down_pane_id = pane_id.clone();
            let right_mouse_up_pane_id = pane_id.clone();
            let right_mouse_up_out_pane_id = pane_id.clone();
            let middle_mouse_down_pane_id = pane_id.clone();
            let middle_mouse_up_pane_id = pane_id.clone();
            let middle_mouse_up_out_pane_id = pane_id.clone();
            elements.push(
                render_pane(
                    PaneRenderInput {
                        pane,
                        frame,
                        presentation,
                        a11y,
                    },
                    i18n,
                    view.clone(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_mouse_down(left_mouse_pane_id.clone(), event, window, cx);
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_mouse_up(event, window, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_mouse_up(event, window, cx);
                    }),
                )
                .on_mouse_move(cx.listener(move |this, event, window, cx| {
                    this.pane_surface_mouse_move(&move_mouse_pane_id, event, window, cx);
                }))
                .on_scroll_wheel(
                    cx.listener(move |this, event: &ScrollWheelEvent, _window, cx| {
                        this.scroll_pane(&scroll_pane_id, event, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event, window, cx| {
                        if !this.pane_aux_mouse_down(
                            right_mouse_down_pane_id.clone(),
                            SurfaceMouseButton::Right,
                            event,
                            window,
                            cx,
                        ) {
                            this.open_context_menu(pane_target.clone(), event, window, cx);
                        }
                    }),
                )
                .on_mouse_up(
                    MouseButton::Right,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_aux_mouse_up(
                            &right_mouse_up_pane_id,
                            SurfaceMouseButton::Right,
                            event,
                            window,
                            cx,
                        );
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Right,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_aux_mouse_up(
                            &right_mouse_up_out_pane_id,
                            SurfaceMouseButton::Right,
                            event,
                            window,
                            cx,
                        );
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_aux_mouse_down(
                            middle_mouse_down_pane_id.clone(),
                            SurfaceMouseButton::Middle,
                            event,
                            window,
                            cx,
                        );
                    }),
                )
                .on_mouse_up(
                    MouseButton::Middle,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_aux_mouse_up(
                            &middle_mouse_up_pane_id,
                            SurfaceMouseButton::Middle,
                            event,
                            window,
                            cx,
                        );
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Middle,
                    cx.listener(move |this, event, window, cx| {
                        this.pane_aux_mouse_up(
                            &middle_mouse_up_out_pane_id,
                            SurfaceMouseButton::Middle,
                            event,
                            window,
                            cx,
                        );
                    }),
                )
                .into_any_element(),
            );
        }
        let squeezed = layout.as_ref().and_then(|layout| {
            self.squeezed_tab_layout(layout)
                .or_else(|| squeezed_layout(layout, &[]))
        });
        let dragged_split = match &self.surface_drag {
            SurfaceDrag::Split(drag) if drag.tab_id == tab_id => Some(drag.path.clone()),
            _ => None,
        };
        // A draft relocation can change split topology, so authoritative
        // handles would sit across the moving shells. Keep the canvas clean
        // until the drag ends; the tab is already gesture-locked.
        let split_handles = layout
            .as_ref()
            .filter(|layout| pane_drag.is_none() && !layout.zoomed)
            .map(|layout| {
                layout
                    .splits
                    .iter()
                    .filter_map(|split| {
                        let path = split.path()?;
                        // While a divider is dragged every handle in the tab
                        // follows the squeeze preview, including nested ones
                        // whose rects the dragged split moves.
                        let geometry = squeezed.as_ref()?.split(&path)?;
                        let dragging = dragged_split.as_deref() == Some(path.as_slice());
                        render_split_handle(
                            split,
                            geometry,
                            dragging,
                            layout.tab_id.clone(),
                            i18n,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>()
            });
        let split_overlay = match &self.surface_drag {
            SurfaceDrag::Split(drag) if drag.tab_id == tab_id => {
                Some(render_split_drag_overlay(drag, cx))
            }
            _ => None,
        };
        let pane_drag_overlay = pane_drag
            .as_ref()
            .filter(|drag| drag.tab_id == tab_id)
            .map(|drag| self.render_pane_drag_overlay(&snapshot, drag, reduce_motion, cx));
        let keyboard_move_overlay = keyboard_move
            .as_ref()
            .map(|mode| self.render_keyboard_move_overlay(&snapshot, mode, reduce_motion));
        let parked_notice = self
            .parked_relocation(tab_id)
            .map(|pending| render_parked_notice(tab_id, &pending.plan, i18n, cx));
        let tab_transfer_notice = self.pending_tab_transfer.as_ref().and_then(|transfer| {
            (transfer.phase == TabTransferPhase::Failed)
                .then(|| render_tab_transfer_notice(transfer.target_tab_id.is_some(), i18n, cx))
        });
        let origin = self.surface_origin();
        let pane_return = self
            .pane_drag_return
            .clone()
            .filter(|_| !reduce_motion)
            .and_then(|flight| {
                window.request_animation_frame();
                render_pane_drag_return(
                    &snapshot,
                    &flight,
                    origin,
                    self.pane(&flight.pane_id)
                        .and_then(|runtime| runtime.frame.clone())
                        .or_else(|| self.pane_drag_snapshot.clone()),
                    i18n,
                )
            });
        if self
            .pane_relocations
            .get(tab_id)
            .is_some_and(|pending| matches!(pending.phase, RelocationPhase::Settling { .. }))
        {
            window.request_animation_frame();
        }
        let ime_view = cx.entity();
        let ime_focus = self.focus.clone();
        let surface_view = cx.entity();
        div()
            .id("terminal-surface")
            .relative()
            .focusable()
            .tab_stop(true)
            .track_focus(&self.focus)
            // Focused descendant of the root on_key_down. send_key stops
            // propagation after handling, so the same keystroke is not
            // dispatched twice on the bubble path.
            .on_key_down(cx.listener(|this, event, window, cx| this.send_key(event, window, cx)))
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                this.pane_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pane_mouse_up(event, window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pane_mouse_up(event, window, cx)),
            )
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            // The gaps around rounded panes are part of the terminal canvas,
            // so they must use the same background as each terminal frame.
            .bg(theme::current().bg.rgba())
            .children(elements)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        surface_view.update(cx, |this, _cx| {
                            this.terminal_surface_bounds = Some((
                                f32::from(bounds.origin.x),
                                f32::from(bounds.origin.y),
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                            ));
                        });
                    },
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &ime_focus,
                            ElementInputHandler::new(bounds, ime_view),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(split_handles.into_iter().flatten())
            .children(split_overlay)
            .children(pane_drag_overlay)
            .children(keyboard_move_overlay)
            .children(pane_return)
            .children(parked_notice)
            .children(tab_transfer_notice)
            .into_any_element()
    }

    /// Keyboard move mode (design §11): the chosen target's highlight and an
    /// accessible status describing the pending intent.
    fn render_keyboard_move_overlay(
        &self,
        snapshot: &HierarchySnapshot,
        mode: &KeyboardPaneMove,
        reduce_motion: bool,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        let origin = self.surface_origin();
        let droppable = mode.droppable();
        let target_name = mode.target.as_ref().map(|hover| {
            snapshot
                .pane(&hover.target_pane_id)
                .map(|pane| pane.display_name().to_owned())
                .unwrap_or_else(|| hover.target_pane_id.clone())
        });
        let state_text = keyboard_move_state_text(
            mode.target
                .as_ref()
                .zip(target_name.as_deref())
                .map(|(hover, name)| (name, hover.zone)),
            droppable,
            i18n,
        );
        let highlight = mode
            .target
            .as_ref()
            .map(|hover| render_drop_highlight(hover, droppable, origin, reduce_motion));
        let zone_label = mode
            .target
            .as_ref()
            .map(|hover| render_drop_zone_label(hover, droppable, origin, reduce_motion, i18n));
        let source_name = snapshot
            .pane(&mode.pane_id)
            .map(|pane| pane.display_name().to_owned())
            .unwrap_or_else(|| mode.pane_id.clone());
        div()
            .id("pane-keyboard-move-overlay")
            .role(ochub_ui::gpui::Role::Status)
            .aria_label(i18n.move_pane_mode(&source_name))
            .aria_value(state_text.clone())
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(highlight)
            .children(zone_label)
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(8.))
                    .px_3()
                    .py_1()
                    .rounded(px(CORNER_CONTROL))
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::accent())
                    .text_xs()
                    .text_color(theme::subtext())
                    .child(format!(
                        "{} — {state_text}",
                        i18n.move_pane_mode(&source_name)
                    )),
            )
            .into_any_element()
    }

    /// Everything painted above the panes while one is lifted: the target
    /// highlight and the floating preview (design §5.2).
    fn render_pane_drag_overlay(
        &self,
        snapshot: &HierarchySnapshot,
        drag: &PaneDrag,
        reduce_motion: bool,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        let i18n = self.i18n;
        // Gesture math lives in window pixels; this overlay is a child of the
        // `relative()` surface, so everything painted here is surface-local.
        let origin = self.surface_origin();
        let droppable = drag.tab_target.is_some()
            || drag.template_hover.is_some()
            || drag
                .hover
                .as_ref()
                .is_some_and(|hover| hover.droppable(drag.edge_drops));
        let zone = drag.hover.as_ref().map(|hover| hover.zone);
        let state_text = match drag.tab_target.as_ref() {
            Some(PaneTabDropTarget::NewTab) => i18n.text(k::TERMINAL_DROP_NEW_TAB).to_owned(),
            Some(PaneTabDropTarget::Existing { tab_id, .. }) => snapshot
                .tabs
                .iter()
                .find(|tab| tab.tab_id == *tab_id)
                .map(|tab| i18n.drop_move_into_tab(&tab.label))
                .unwrap_or_else(|| i18n.text(k::TERMINAL_DROP_INVALID).to_owned()),
            None => drag
                .template_hover
                .as_ref()
                .map(|hover| hover.placement.template.label(i18n).to_owned())
                .unwrap_or_else(|| pane_drag_state_text(zone, droppable, i18n).to_owned()),
        };
        let highlight = drag
            .hover
            .as_ref()
            .filter(|_| droppable)
            .map(|hover| render_drop_highlight(hover, true, origin, reduce_motion));
        let template_highlight = self
            .terminal_surface_bounds
            .and_then(|surface| render_pane_template_target_highlight(drag, surface, origin, i18n));
        // The zone label goes above the preview: the floating card follows
        // the pointer, which sits inside the target, so a label drawn in the
        // highlight itself would be hidden under the card.
        let zone_label = drag
            .hover
            .as_ref()
            .filter(|_| droppable)
            .map(|hover| render_drop_zone_label(hover, true, origin, reduce_motion, i18n));
        let pane = snapshot.pane(&drag.pane_id).cloned();
        let palette = drag.layout_templates.then(|| {
            let pane_count = snapshot
                .layout_for(&drag.tab_id)
                .map(|layout| layout.panes.len())
                .unwrap_or_default();
            self.terminal_surface_bounds.and_then(|surface| {
                render_pane_template_palette(drag, pane_count, surface, i18n, cx)
            })
        });
        let frame = self
            .pane(&drag.pane_id)
            .and_then(|runtime| runtime.frame.clone())
            .or_else(|| self.pane_drag_snapshot.clone());
        let opacity = if drag.hover.is_some() && !droppable {
            PANE_DRAG_INVALID_OPACITY
        } else {
            PANE_DRAG_PREVIEW_OPACITY
        };
        let lifted = surface_local(pane_drag_preview_rect(drag), origin);
        let resting = surface_local(
            (
                drag.pointer.0 - drag.grab_offset.0,
                drag.pointer.1 - drag.grab_offset.1,
                drag.source_rect.2,
                drag.source_rect.3,
            ),
            origin,
        );
        let preview = pane.map(|pane| {
            let card = pane_preview_card(&pane, frame, lifted, opacity, i18n);
            if reduce_motion {
                card.into_any_element()
            } else {
                card.with_animation(
                    "pane-drag-lift",
                    Animation::new(PANE_DRAG_LIFT_ANIMATION).with_easing(ease_out_quint()),
                    move |card, t| {
                        let (x, y, w, h) = lerp_rect(resting, lifted, t);
                        card.left(px(x))
                            .top(px(y))
                            .w(px(w))
                            .h(px(h))
                            .opacity(1. + (opacity - 1.) * t)
                    },
                )
                .into_any_element()
            }
        });
        div()
            .id("pane-drag-overlay")
            .role(ochub_ui::gpui::Role::Status)
            .aria_label(
                i18n.drag_pane_handle(
                    snapshot
                        .pane(&drag.pane_id)
                        .map(|pane| pane.display_name())
                        .unwrap_or(&drag.pane_id),
                ),
            )
            .aria_value(state_text)
            .absolute()
            .size_full()
            .cursor_grabbing()
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                this.pane_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pane_mouse_up(event, window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pane_mouse_up(event, window, cx)),
            )
            .children(highlight)
            .children(template_highlight)
            .children(preview)
            .children(zone_label)
            .children(palette.into_iter().flatten())
            .into_any_element()
    }

    pub(crate) fn render_reorder_overlay(
        &self,
        drag: &ReorderDrag,
        cx: &mut Context<Self>,
    ) -> ochub_ui::gpui::AnyElement {
        div()
            .id("reorder-drag-overlay")
            .absolute()
            .size_full()
            .cursor_grabbing()
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                this.pane_mouse_move(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pane_mouse_up(event, window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.pane_mouse_up(event, window, cx)),
            )
            .when_some(
                reorder_ghost(
                    self.snapshot.as_ref(),
                    &self.reorder_metrics,
                    drag,
                    &drag.order[drag.source_index],
                ),
                |overlay, ghost| overlay.child(ghost),
            )
            .into_any_element()
    }

    pub(crate) fn render_tab_preview(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<ochub_ui::gpui::AnyElement> {
        if matches!(self.surface_drag, SurfaceDrag::Reorder(_)) {
            self.dismiss_tab_preview();
            return None;
        }
        let tab_id = self.tab_preview_id.as_ref()?;
        let rect = self
            .reorder_metrics
            .tabs
            .iter()
            .find(|span| span.id == *tab_id)
            .map(|span| span.rect)?;
        let window_width = f32::from(window.viewport_size().width);
        let (x, _) = tab_preview_origin(rect, window_width);
        let title = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .tabs
                    .iter()
                    .find(|tab| tab.tab_id == *tab_id)
                    .map(|tab| tab.label.clone())
            })
            .unwrap_or_default();
        let card = self.tab_preview_card(tab_id, title);
        Some(
            div()
                .id("tab-preview-layer")
                .absolute()
                // Inset auto would use the static in-flow position (T28).
                .top(px(rect.1 + rect.3))
                .left(px(x))
                .pt(px(TAB_PREVIEW_GAP))
                .occlude()
                .on_hover(cx.listener(|this, hovered, _window, cx| {
                    this.set_tab_preview_hovered(*hovered, cx);
                }))
                .child(card.into_element())
                .into_any_element(),
        )
    }
}

/// What a left press on empty tab-strip space does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabStripPress {
    /// Single press: hand the drag to the window server.
    MoveWindow,
    /// Second click of a double-click: the titlebar action macOS is
    /// configured for (zoom or minimise).
    TitlebarDoubleClick,
}

/// Decides between a window drag and the titlebar double-click action from
/// the press's `click_count`.
pub(crate) fn tab_strip_press(click_count: usize) -> TabStripPress {
    if click_count == 2 {
        TabStripPress::TitlebarDoubleClick
    } else {
        TabStripPress::MoveWindow
    }
}
