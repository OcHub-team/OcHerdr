use super::*;
#[cfg(target_os = "macos")]
use gpui::Focusable;

/// Pump the executor until the view's stream shows `Lost` with `expected` in
/// the detail. The endpoint handshake and socket reads run on real threads,
/// so a bare `run_until_parked` can return before the failure lands on the
/// view — and a reconnect test starts from an older `Lost` it must wait past.
fn wait_for_event_stream_lost(
    view: &Entity<OcHerdrView>,
    cx: &mut VisualTestContext,
    expected: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let lost = view.read_with(cx, |this, _| {
            matches!(&this.event_stream, EventStreamState::Lost(detail) if detail.contains(expected))
        });
        if lost {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the endpoint failure never marked the stream Lost with {expected:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// A Herdr too old for `tab.move`: the endpoint generation-1 shell protocol
/// works, but the NDJSON API serves `session.snapshot` only and rejects every
/// other method. `FakeHerdr` has no named constructor for this combination.
fn tab_move_rejecting_fake(snapshot: HierarchySnapshot) -> FakeHerdr {
    FakeHerdr::start(snapshot, PaneMoveScript::default(), false, true)
}

/// Supply pane geometry and drive the mount scheduler so the visible tab
/// spawns real Ghostty mirrors. relocation.rs keeps an identical private
/// helper; this copy is local because that one is not shared.
#[cfg(target_os = "macos")]
fn measure_two_terminal_bodies(view: &Entity<OcHerdrView>, cx: &mut VisualTestContext) {
    view.update_in(cx, |this, window, cx| {
        for (pane_id, x) in [("p-left", 0.), ("p-right", 400.)] {
            this.sync_measured_pane_body(
                pane_id,
                gpui::Bounds {
                    origin: gpui::point(gpui::px(x), gpui::px(0.)),
                    size: gpui::size(gpui::px(400.), gpui::px(300.)),
                },
                window,
                cx,
            );
        }
        // The production scheduler spreads one mount over each wall-clock
        // turn. GPUI's parked test executor does not advance that timer, so
        // drive the same two batches explicitly after supplying geometry.
        for _ in 0..2 {
            this.pane_mount_scheduled = false;
            this.ensure_session_terminals(cx);
        }
    });
    cx.run_until_parked();
}

#[gpui::test]
fn command_comma_opens_ocherdr_settings_in_place(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    view.update_in(cx, |this, window, cx| {
        this.focus.focus(window, cx);
        let event = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: app_primary_modifiers(),
                key: ",".into(),
                key_char: Some(",".into()),
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(this.handle_app_shortcut(&event, window, cx));
    });
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert!(matches!(this.overlay, crate::Overlay::Appearance));
    });
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn settings_toolbar_opens_the_explanation_overlay_and_close_restores_pane_input(
    cx: &mut TestAppContext,
) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    measure_two_terminal_bodies(&view, cx);
    let before = view.read_with(cx, |this, _| this.selection.clone());

    // Baseline: with the surface focused, a key reaches the selected pane
    // through the endpoint's semantic input lane.
    view.update_in(cx, |this, window, cx| this.focus.focus(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    let deadline = Instant::now() + Duration::from_secs(10);
    while fake.pane_inputs("p-left").is_empty() {
        assert!(
            Instant::now() < deadline,
            "the pane never received the baseline key"
        );
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }
    let pane_inputs_before = fake.pane_inputs("p-left").len();

    let gear = cx
        .debug_bounds("open-herdr-settings")
        .expect("settings toolbar button");
    cx.simulate_click(gear.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("close-herdr-settings").is_some(),
        "the overlay renders the explanation panel instead of a settings client"
    );
    view.update_in(cx, |this, window, _| {
        assert!(matches!(this.overlay, crate::Overlay::HerdrSettings));
        assert!(this.herdr_settings.is_some());
        assert!(!crate::key_goes_to_terminal(&this.overlay));
        assert!(
            this.dialog_focus.is_focused(window),
            "the explanation dialog takes focus"
        );
    });
    assert_eq!(
        fake.endpoint_connections(),
        1,
        "endpoint-generation servers get no embedded settings client"
    );

    // Explicitly drive a key through GPUI. The dialog consumes it, so it
    // reaches neither the pane mounted behind the overlay nor any settings
    // connection.
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(
        fake.pane_inputs("p-left").len(),
        pane_inputs_before,
        "the overlay must swallow keys instead of leaking them to the pane"
    );
    view.read_with(cx, |this, _| {
        assert!(matches!(this.overlay, crate::Overlay::HerdrSettings));
    });

    let close = cx
        .debug_bounds("close-herdr-settings")
        .expect("settings close button");
    cx.simulate_click(close.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    view.update_in(cx, |this, window, cx| {
        assert!(matches!(this.overlay, crate::Overlay::None));
        assert!(this.herdr_settings.is_none());
        assert!(this.focus.is_focused(window));
        assert_eq!(this.selection.pane_id, before.pane_id);
        assert_eq!(this.selection.tab_id, before.tab_id);
        // Reopening uses a fresh entity and remains in the window.
        this.open_herdr_settings(cx);
        assert!(matches!(this.overlay, crate::Overlay::HerdrSettings));
        this.close_herdr_settings(window, cx);
    });
    assert_eq!(
        fake.endpoint_connections(),
        1,
        "closing the overlay leaves the session endpoint as the only connection"
    );
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn command_backspace_edits_the_focused_gpui_field_before_terminal_input(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    view.update_in(cx, |this, window, cx| {
        this.agent_prompt_input.update(cx, |input, cx| {
            input.set_content("first line\nsecond line", cx);
        });
        this.agent_prompt_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        let event = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: app_primary_modifiers(),
                key: "backspace".into(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(ochub_ui::text_input::dispatch_focused_editing_key(
            &event, window, cx
        ));
    });
    view.read_with(cx, |this, cx| {
        assert_eq!(
            this.agent_prompt_input.read(cx).content().as_ref(),
            "first line\n"
        );
    });
}

#[gpui::test]
fn overflowing_tab_bar_scrolls_horizontally_with_the_wheel(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    view.update(cx, |this, cx| {
        this.snapshot = Some(overflowing_tab_snapshot());
        this.selection = Selection {
            connection_id: "local".into(),
            workspace_id: Some("w".into()),
            tab_id: Some("t-1".into()),
            ..Default::default()
        };
        cx.notify();
    });
    cx.simulate_resize(gpui::size(gpui::px(700.), gpui::px(500.)));
    cx.run_until_parked();

    let tab_scroll = view.read_with(cx, |this, _| this.tab_scroll.clone());
    assert!(
        tab_scroll.max_offset().x > gpui::px(0.),
        "the fixture must overflow the tab strip"
    );
    let before = tab_scroll.offset().x;
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: tab_scroll.bounds().center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-120.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });

    assert!(
        tab_scroll.offset().x < before,
        "a vertical wheel gesture over the tab strip must reveal tabs to the right"
    );

    tab_scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
    view.update(cx, |this, cx| this.select_tab_number(9, cx));
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-9"));
    });
    assert!(
        tab_scroll.offset().x < gpui::px(0.),
        "selecting a hidden tab by number must scroll it into view"
    );
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn command_backspace_dispatch_preserves_suffix_and_is_undoable(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_rename(
            crate::HierarchyTarget::Tab {
                id: "test".into(),
                label: "test".into(),
            },
            window,
            cx,
        );
        this.rename_input
            .update(cx, |input, cx| input.set_content("中文😀tail", cx));
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("left left left left cmd-backspace");
    view.read_with(cx, |this, cx| {
        assert_eq!(this.rename_input.read(cx).content().as_ref(), "tail")
    });
    cx.simulate_keystrokes("cmd-z");
    view.read_with(cx, |this, cx| {
        assert_eq!(this.rename_input.read(cx).content().as_ref(), "中文😀tail")
    });
    cx.simulate_keystrokes("cmd-a cmd-backspace");
    view.read_with(cx, |this, cx| {
        assert_eq!(this.rename_input.read(cx).content().as_ref(), "")
    });
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn command_backspace_handles_dynamically_created_fields_and_preserves_composition(
    cx: &mut TestAppContext,
) {
    use gpui::EntityInputHandler;
    let (view, cx) = open_view(cx);
    // Deliberately not one of the fields stored on OcHerdrView or HostCenter.
    let input = cx.update(|_, cx| cx.new(|cx| crate::TextInput::new(cx, "dynamic")));
    cx.run_until_parked();
    let event = gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            key: "backspace".into(),
            modifiers: app_primary_modifiers(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    };
    view.update_in(cx, |_this, window, cx| {
        input.update(cx, |input, cx| input.set_content("first\n中文😀", cx));
        input.read(cx).focus_handle(cx).focus(window, cx);
        assert!(ochub_ui::text_input::dispatch_focused_editing_key(
            &event, window, cx
        ));
        assert_eq!(input.read(cx).content().as_ref(), "first\n");
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "候选", None, window, cx)
        });
        assert!(ochub_ui::text_input::dispatch_focused_editing_key(
            &event, window, cx
        ));
        assert_eq!(input.read(cx).content().as_ref(), "first\n候选");
    });
}

/// Tabs start flush with the content area's left edge. Everything between `+`
/// and the toolbar remains a full-height move area, so a press there reaches
/// no tab and leaves selection and reordering untouched.
#[gpui::test]
fn tabs_are_flush_left_and_trailing_space_moves_the_window(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    view.update(cx, |this, cx| {
        this.snapshot = Some(three_tab_snapshot());
        this.selection = Selection {
            connection_id: "local".into(),
            workspace_id: Some("w".into()),
            tab_id: Some("t-a".into()),
            ..Default::default()
        };
        cx.notify();
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();

    let bar = cx.debug_bounds("tab-bar").expect("tab bar");
    let space = cx
        .debug_bounds("tab-strip-space")
        .expect("trailing move area");
    let first = cx.debug_bounds("tab-t-a").expect("first tab");
    let last = cx.debug_bounds("tab-t-c").expect("last tab");
    // The strip's content box: its height minus the 1px bottom border.
    assert_eq!(space.size.height, gpui::px(HEADER_HEIGHT - 1.));
    assert_eq!(
        first.origin.x, bar.origin.x,
        "the first tab is flush with the left edge: {bar:?} vs {first:?}"
    );
    assert!(
        space.origin.x > last.origin.x + last.size.width,
        "the free space starts after the last tab and `+`: {space:?} vs {last:?}"
    );
    assert!(
        space.size.width > gpui::px(0.),
        "the remaining strip space stays a hittable move area: {space:?}"
    );
    for tab in ["tab-t-a", "tab-t-b", "tab-t-c"] {
        let bounds = cx.debug_bounds(tab).unwrap();
        assert!(!bounds.intersects(&space));
    }
    // The press itself cannot be simulated: the test platform's
    // `start_window_move` is `unimplemented!()`.
}

fn open_close_tab_dialog(view: &Entity<OcHerdrView>, cx: &mut VisualTestContext) {
    view.update_in(cx, |this, window, cx| {
        this.focus.focus(window, cx);
        this.request_close(
            crate::HierarchyTarget::Tab {
                id: "t-a".into(),
                label: "alpha".into(),
            },
            cx,
        );
    });
    cx.run_until_parked();
    view.update_in(cx, |this, window, _| {
        assert!(matches!(this.overlay, crate::Overlay::ConfirmClose(_)));
        assert!(
            this.dialog_focus.is_focused(window),
            "the dialog takes focus when it opens"
        );
        assert!(!this.focus.is_focused(window));
    });
    assert!(
        cx.debug_bounds("confirm-close-target-hint-↩").is_some(),
        "the primary button carries the return hint"
    );
    assert!(
        cx.debug_bounds("cancel-close-target-hint-esc").is_some(),
        "cancel carries the esc hint"
    );
}

#[gpui::test]
fn confirm_dialog_takes_focus_and_enter_runs_the_primary_action(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    open_close_tab_dialog(&view, cx);

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.update_in(cx, |this, window, _| {
        assert!(matches!(this.overlay, crate::Overlay::None));
        assert!(
            this.focus.is_focused(window),
            "focus returns to the terminal surface"
        );
    });
    let closes = fake.requests_for("tab.close");
    assert_eq!(closes.len(), 1, "enter closes the tab: {closes:?}");
    assert_eq!(closes[0]["params"]["tab_id"], json!("t-a"));
}

#[gpui::test]
fn confirm_dialog_escape_cancels_without_a_request(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    open_close_tab_dialog(&view, cx);

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.update_in(cx, |this, window, _| {
        assert!(matches!(this.overlay, crate::Overlay::None));
        assert!(this.focus.is_focused(window));
    });
    assert!(fake.requests_for("tab.close").is_empty());
}

/// The dialog focuses itself, so it does not depend on the terminal having
/// been focused before it opened (a toolbar click does not focus the surface).
#[gpui::test]
fn confirm_dialog_receives_keys_even_when_nothing_was_focused(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    view.update_in(cx, |this, window, cx| {
        window.blur();
        this.request_close(
            crate::HierarchyTarget::Tab {
                id: "t-a".into(),
                label: "alpha".into(),
            },
            cx,
        );
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(matches!(this.overlay, crate::Overlay::None))
    });
    assert_eq!(fake.requests_for("tab.close").len(), 1);
}

#[gpui::test]
fn tab_rename_enter_submits_once_and_applies_the_success_response(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    view.update_in(cx, |this, window, cx| {
        this.open_rename(
            crate::HierarchyTarget::Tab {
                id: "t-a".into(),
                label: "alpha".into(),
            },
            window,
            cx,
        );
        this.rename_input
            .update(cx, |input, cx| input.set_content("renamed", cx));
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let renames = fake.requests_for("tab.rename");
    assert_eq!(
        renames.len(),
        1,
        "Enter must submit exactly once: {renames:?}"
    );
    assert_eq!(
        renames[0].get("params"),
        Some(&json!({ "tab_id": "t-a", "label": "renamed" }))
    );
    view.read_with(cx, |this, _| {
        assert!(matches!(this.overlay, crate::Overlay::None));
        assert_eq!(
            this.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.tabs.iter().find(|tab| tab.tab_id == "t-a"))
                .map(|tab| tab.label.as_str()),
            Some("renamed"),
            "a successful rename response must update the tab even if its event is delayed",
        );
    });
}

#[gpui::test]
fn tab_shortcut_hints_show_only_while_command_is_held(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    view.update_in(cx, |this, window, cx| {
        this.snapshot = Some(three_tab_snapshot());
        this.selection = Selection {
            connection_id: "local".into(),
            workspace_id: Some("w".into()),
            tab_id: Some("t-a".into()),
            ..Default::default()
        };
        this.focus.focus(window, cx);
        cx.notify();
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();
    assert_eq!(cx.debug_bounds("tab-shortcut-t-a"), None, "hidden at rest");
    let title_at_rest = cx.debug_bounds("tab-title-t-a").expect("title");

    let command = app_primary_modifiers();
    cx.simulate_modifiers_change(command);
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.command_held));
    let hint = cx
        .debug_bounds("tab-shortcut-t-a")
        .expect("hint appears while the primary modifier is down");
    let tab = cx.debug_bounds("tab-t-a").unwrap();
    assert!(tab.contains(&hint.center()));
    assert_eq!(
        cx.debug_bounds("tab-title-t-a").unwrap(),
        title_at_rest,
        "the hint must not move the title"
    );

    cx.simulate_modifiers_change(gpui::Modifiers::default());
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(!this.command_held));
    assert_eq!(
        cx.debug_bounds("tab-shortcut-t-a"),
        None,
        "hidden on release"
    );

    // Switching apps can hide the release, so losing key
    // status must drop the hints on its own.
    view.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    cx.simulate_modifiers_change(command);
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.command_held));
    cx.deactivate_window();
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(!this.command_held));
}

#[gpui::test]
fn fixed_width_tab_hover_reveals_close_then_delayed_preview(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    view.update(cx, |this, cx| {
        let mut snapshot = three_tab_snapshot();
        snapshot.tabs[1].label = "a deliberately long tab title that must be truncated".into();
        snapshot.tabs[1].pane_count = 1;
        snapshot.panes.push(PaneInfo {
            pane_id: "p-preview".into(),
            terminal_id: "term-preview".into(),
            workspace_id: "w".into(),
            tab_id: "t-b".into(),
            focused: false,
            cwd: None,
            foreground_cwd: None,
            label: None,
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: None,
            agent_status: AgentStatus::Idle,
            state_labels: HashMap::new(),
            tokens: HashMap::new(),
            revision: 1,
        });
        this.snapshot = Some(snapshot);
        this.selection = Selection {
            connection_id: "local".into(),
            workspace_id: Some("w".into()),
            tab_id: Some("t-a".into()),
            ..Default::default()
        };
        cx.notify();
    });
    // Keep all three fixed-width tabs inside the scroll viewport. Toolbar text
    // uses platform font metrics, so 900px can clip the second tab on CI even
    // though its pre-clip debug bounds still report the full pill.
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(500.)));
    cx.run_until_parked();

    let alpha_before = cx.debug_bounds("tab-t-a").expect("alpha tab bounds");
    let long_before = cx.debug_bounds("tab-t-b").expect("long tab bounds");
    assert_eq!(alpha_before.size.width, gpui::px(TAB_PILL_WIDTH));
    assert_eq!(long_before.size.width, gpui::px(TAB_PILL_WIDTH));
    assert_eq!(cx.debug_bounds("tab-title-fade-t-a"), None);
    assert!(cx.debug_bounds("tab-title-fade-t-b").is_some());
    assert_eq!(cx.debug_bounds("close-tab-t-b"), None);
    assert_eq!(cx.debug_bounds("tab-preview-t-b"), None);
    view.read_with(cx, |this, _| {
        assert_eq!(this.hovered_tab_id, None);
        assert_eq!(
            this.tab_close_reveals["t-b"].value(Instant::now(), true),
            0.
        );
    });

    cx.simulate_mouse_move(long_before.center(), None, gpui::Modifiers::default());
    cx.run_until_parked();

    let long_after = cx.debug_bounds("tab-t-b").expect("hovered long tab bounds");
    let title_after = cx
        .debug_bounds("tab-title-t-b")
        .expect("centered title bounds");
    let close_after = cx
        .debug_bounds("close-tab-t-b")
        .expect("revealed close bounds");
    assert_eq!(long_after, long_before, "hover must not reflow the tab");
    assert_eq!(title_after.center().x, long_after.center().x);
    assert!(close_after.center().x < long_after.center().x);
    assert_eq!(cx.debug_bounds("tab-preview-t-b"), None);
    view.read_with(cx, |this, _| {
        assert_eq!(this.hovered_tab_id.as_deref(), Some("t-b"));
        assert_eq!(
            this.tab_close_reveals["t-b"].value(Instant::now(), true),
            1.
        );
    });

    cx.executor()
        .advance_clock(TAB_PREVIEW_DELAY - Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(cx.debug_bounds("tab-preview-t-b"), None);

    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    let preview = cx
        .debug_bounds("tab-preview-t-b")
        .expect("preview appears after the configured hover delay");
    let preview_title = cx
        .debug_bounds("tab-preview-title-t-b")
        .expect("preview exposes the complete title region");
    let preview_pane = cx
        .debug_bounds("tab-preview-pane-t-b-0")
        .expect("preview composes the tab pane into the card");
    assert_eq!(preview.size.width, gpui::px(TAB_PREVIEW_WIDTH));
    assert!(preview_title.size.width > gpui::px(TAB_PILL_WIDTH));
    assert_eq!(preview_pane.size.width, gpui::px(TAB_PREVIEW_WIDTH - 2.));
    assert_eq!(preview_pane.size.height, gpui::px(TAB_PREVIEW_HEIGHT));
    assert_eq!(
        preview.origin.x + preview.size.width / 2.,
        long_after.center().x,
        "preview must be centered under the hovered tab"
    );
    assert_eq!(
        preview.origin.y,
        long_after.origin.y + long_after.size.height + gpui::px(TAB_PREVIEW_GAP),
        "preview must sit below the tab, not at the cursor"
    );

    cx.simulate_click(close_after.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(matches!(
            &this.overlay,
            crate::Overlay::ConfirmClose(crate::HierarchyTarget::Tab { id, .. }) if id == "t-b"
        ));
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });
}

#[gpui::test]
fn the_details_entry_reads_the_agents_name_and_recent_output(cx: &mut TestAppContext) {
    let fake = FakeAgentHerdr::new(PromptReply::Success);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        cx.notify();
    });

    open_agent_row_details(cx);

    let reads = fake.requests_for("agent.read");
    assert_eq!(
        reads.len(),
        1,
        "opening the panel must issue one agent.read"
    );
    assert_eq!(
        reads[0].get("params"),
        Some(&json!({
            "target": "p1",
            "source": "recent",
            "format": "text",
        }))
    );
    view.read_with(cx, |this, cx| {
        assert_eq!(
            this.agent_name_input.read(cx).content().as_ref(),
            "reviewer",
            "the editable value must come from AgentInfo.name"
        );
        assert!(matches!(
            &this.agent_output,
            AgentOutputState::Ready { text, truncated: false } if text == "recent output\n"
        ));
    });
}

#[gpui::test]
fn rename_uses_the_agent_info_returned_by_herdr(cx: &mut TestAppContext) {
    let fake = FakeAgentHerdr::new(PromptReply::Success);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        cx.notify();
    });
    open_agent_row_details(cx);
    view.update(cx, |this, cx| {
        this.agent_name_input
            .update(cx, |input, cx| input.set_content("requested-name", cx));
    });
    let save = cx
        .debug_bounds("save-agent-name")
        .expect("save agent name should be in the rendered tree");

    cx.simulate_click(save.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    let renames = fake.requests_for("agent.rename");
    assert_eq!(renames.len(), 1);
    assert_eq!(
        renames[0].get("params"),
        Some(&json!({ "target": "p1", "name": "requested-name" }))
    );
    view.read_with(cx, |this, cx| {
        assert_eq!(
            this.agent_name_input.read(cx).content().as_ref(),
            "server-confirmed",
            "the response is authoritative; the submitted or display name is not"
        );
    });
}

#[gpui::test]
fn clicking_send_issues_the_exact_non_waiting_prompt(cx: &mut TestAppContext) {
    let fake = FakeAgentHerdr::new(PromptReply::Success);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        cx.notify();
    });
    open_agent_row_details(cx);
    view.update(cx, |this, cx| {
        this.agent_prompt_input
            .update(cx, |input, cx| input.set_content("  preserve me  ", cx));
    });

    click_send_prompt(cx);

    let prompts = fake.requests_for("agent.prompt");
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        prompts[0].get("params"),
        Some(&json!({ "target": "p1", "text": "  preserve me  " }))
    );
    assert!(prompts[0]["params"].get("wait").is_none());
    view.read_with(cx, |this, _| {
        assert!(matches!(
            this.agent_prompts.get("p1"),
            Some(AgentPromptPhase::Sent)
        ));
    });
}

#[gpui::test]
fn agent_blocked_sends_once_and_writes_the_failed_prompt_state(cx: &mut TestAppContext) {
    let fake = FakeAgentHerdr::new(PromptReply::Blocked);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        cx.notify();
    });
    open_agent_row_details(cx);
    view.update(cx, |this, cx| {
        this.agent_prompt_input
            .update(cx, |input, cx| input.set_content("blocked prompt", cx));
    });

    click_send_prompt(cx);

    let prompts = fake.requests_for("agent.prompt");
    assert_eq!(
        prompts.len(),
        1,
        "agent_blocked must not retry the mutation"
    );
    view.read_with(cx, |this, _| {
        assert!(matches!(
            this.agent_prompts.get("p1"),
            Some(AgentPromptPhase::Blocked { message }) if message == "agent reviewer is blocked"
        ));
    });
}

#[gpui::test]
fn a_prompt_failure_completes_after_its_panel_closes(cx: &mut TestAppContext) {
    let release = Arc::new(AtomicBool::new(false));
    let fake = FakeAgentHerdr::new(PromptReply::BlockedAfter(release.clone()));
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        cx.notify();
    });
    open_agent_row_details(cx);
    view.update(cx, |this, cx| {
        this.agent_prompt_input
            .update(cx, |input, cx| input.set_content("finish after close", cx));
        this.submit_agent_prompt(cx);
        this.set_overlay(crate::Overlay::None, cx);
    });
    view.read_with(cx, |this, _| {
        assert!(matches!(this.overlay, crate::Overlay::None));
        assert!(matches!(
            this.agent_prompts.get("p1"),
            Some(AgentPromptPhase::Sending { .. })
        ));
    });

    release.store(true, Ordering::Relaxed);
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert!(matches!(
            this.agent_prompts.get("p1"),
            Some(AgentPromptPhase::Blocked { message }) if message == "agent reviewer is blocked"
        ));
    });
    assert_eq!(fake.requests_for("agent.prompt").len(), 1);
}

#[gpui::test]
fn a_closed_endpoint_stream_marks_the_stream_lost_and_stops_rescheduling(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);

    let keep = view.update(cx, |this, cx| {
        assert!(
            matches!(this.event_stream, EventStreamState::Live),
            "the connected fixture starts Live"
        );
        let owner = this.current_session_key().expect("connected session");
        this.apply_endpoint_batch(
            &owner,
            vec![Err(ocherdr_herdr::HerdrError::EventStreamClosed(
                "endpoint worker stopped".into(),
            ))],
            cx,
        )
    });

    view.read_with(cx, |this, _| {
        assert!(
            matches!(&this.event_stream, EventStreamState::Lost(detail) if detail.contains("endpoint worker stopped")),
            "a closed stream must write Lost onto the view, not leave Live/Idle"
        );
    });
    assert!(
        !keep,
        "a closed stream has nothing left to wait for and must not reschedule"
    );
}

#[gpui::test]
fn a_pushed_snapshot_refreshes_the_visible_current_snapshot_immediately(cx: &mut TestAppContext) {
    // The served hierarchy starts without p-right; the push carries the
    // current state.
    let mut stale = two_pane_snapshot();
    stale.panes.retain(|pane| pane.pane_id != "p-right");
    stale.layouts = vec![single_pane_layout("t-a", "p-left")];
    let fake = FakeHerdr::serving(stale);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.read_with(cx, |this, _| {
        assert!(
            this.snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.pane("p-right").is_none()),
            "the fixture starts with a stale single-pane snapshot"
        );
    });
    let pulls_before = fake.requests_for("session.snapshot").len();

    push_state(
        &fake,
        &view,
        cx,
        |snapshot| {
            snapshot.panes.push(split_pane("p-right", false));
            snapshot.layouts = vec![two_pane_layout("p-left", "p-right")];
        },
        |snapshot| snapshot.pane("p-right").is_some(),
    );

    view.read_with(cx, |this, _| {
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(
            snapshot.pane("p-right").is_some(),
            "the push must fetch current state instead of leaving the stale frame on screen"
        );
        assert_eq!(
            snapshot.layout_for("t-a").map(|layout| layout.panes.len()),
            Some(2),
        );
    });
    assert!(
        fake.requests_for("session.snapshot").len() > pulls_before,
        "the push invalidation must issue an authoritative refresh"
    );
}

#[gpui::test]
fn reload_writes_a_rejected_endpoint_handshake_as_lost_instead_of_idle(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_ok_endpoint_rejected();
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();

    view.update(cx, |this, cx| {
        point_local_profile_at_fake(this, &fake);
        this.reload(None, cx);
    });
    wait_for_event_stream_lost(&view, cx, "endpoint rejected generation");

    view.read_with(cx, |this, _| {
        assert!(
            matches!(&this.event_stream, EventStreamState::Lost(detail) if detail.contains("endpoint rejected generation")),
            "reload must write the refused handshake as Lost, not Idle"
        );
        assert_eq!(
            session_name(this),
            Some("alpha"),
            "the refused handshake still selected a running session"
        );
    });
}

#[gpui::test]
fn a_snapshot_push_for_a_stale_pane_set_resyncs_without_notifying(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(agent_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.read_with(cx, |this, _| {
        assert!(
            this.snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.pane("p1").is_some()),
            "the fixture starts with the pane on screen"
        );
    });
    let pulls_before = fake.requests_for("session.snapshot").len();

    // The server no longer has the pane the client is showing.
    push_state(
        &fake,
        &view,
        cx,
        |snapshot| {
            snapshot.panes.clear();
            snapshot.agents.clear();
            snapshot.focused_pane_id = None;
            for workspace in &mut snapshot.workspaces {
                workspace.pane_count = 0;
            }
            for tab in &mut snapshot.tabs {
                tab.pane_count = 0;
            }
        },
        |snapshot| snapshot.panes.is_empty(),
    );

    assert_eq!(
        fake.requests_for("session.snapshot").len(),
        pulls_before + 1,
        "a stale pane set must trigger exactly one snapshot resync"
    );
    view.read_with(cx, |this, cx| {
        assert!(
            this.snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.pane("p1").is_none()),
            "the authoritative snapshot replaced the stale pane set"
        );
        assert_eq!(
            this.notifications.read(cx).history().count(),
            0,
            "a stale pane set must resync without producing an ApplyLiveUpdate notification"
        );
    });
}

#[gpui::test]
fn a_rejected_tab_move_returns_the_display_to_authoritative_order(cx: &mut TestAppContext) {
    let mut snapshot = three_tab_snapshot();
    snapshot.tabs.truncate(2);
    let fake = tab_move_rejecting_fake(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);

    let order = ["t-a", "t-b"].map(str::to_owned).to_vec();
    let list = ReorderList::Tabs {
        workspace_id: "w".into(),
    };
    let settling = PendingListReorder {
        list: list.clone(),
        order: order.clone(),
        source_index: 0,
        hover: ReorderHover::AfterLast,
        released_origin: (520., 18.),
    };
    view.update(cx, |this, cx| {
        this.submit_reorder(&list, "t-a".into(), 2, Some(settling), cx);
    });
    view.read_with(cx, |this, _| {
        let pending = this
            .pending_reorder
            .as_ref()
            .and_then(|pending| pending.display.as_ref());
        let projection = reorder_projection(&list, &order, None, pending)
            .expect("the request has not failed yet, so its projection is visible");
        assert_eq!(projection.positions, [1, 0]);
    });

    // The strict API rejects `tab.move`; the response lands on a worker
    // thread, so pump until the gate opens.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let cleared = view.read_with(cx, |this, _| this.pending_reorder.is_none());
        if cleared {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the rejected tab.move never released the reorder gate"
        );
        thread::sleep(Duration::from_millis(10));
    }

    view.read_with(cx, |this, _| {
        assert!(
            this.pending_reorder.is_none(),
            "a rejected request must release both the reorder gate and its prediction"
        );
    });
}

#[gpui::test]
fn a_conflicting_pushed_snapshot_replaces_the_pending_projection_with_authority(
    cx: &mut TestAppContext,
) {
    let fake = FakeHerdr::serving(three_tab_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.read_with(cx, |this, _| {
        assert!(!this.snapshot_refreshing);
    });
    let original = ["t-a", "t-b", "t-c"].map(str::to_owned).to_vec();
    let list = ReorderList::Tabs {
        workspace_id: "w".into(),
    };
    let settling = PendingListReorder {
        list: list.clone(),
        order: original.clone(),
        source_index: 0,
        hover: ReorderHover::AfterLast,
        released_origin: (520., 18.),
    };
    view.update(cx, |this, cx| {
        this.submit_reorder(&list, "t-a".into(), 3, Some(settling), cx);
    });
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        let pending = this
            .pending_reorder
            .as_ref()
            .and_then(|pending| pending.display.as_ref());
        let projection = reorder_projection(&list, &original, None, pending)
            .expect("the accepted move remains pending until a snapshot push arrives");
        assert_eq!(projection.positions, [2, 0, 1]);
    });

    // The server's authoritative order conflicts with the prediction.
    push_state(
        &fake,
        &view,
        cx,
        |snapshot| {
            snapshot.tabs = vec![
                test_tab("t-c", 3, "gamma"),
                test_tab("t-a", 1, "alpha"),
                test_tab("t-b", 2, "beta"),
            ];
        },
        |snapshot| {
            snapshot
                .tabs
                .iter()
                .map(|tab| tab.tab_id.as_str())
                .eq(["t-c", "t-a", "t-b"])
        },
    );

    view.read_with(cx, |this, _| {
        let snapshot = this
            .snapshot
            .as_ref()
            .expect("the pushed snapshot applies to the production view snapshot");
        let authoritative = snapshot
            .tabs_for("w")
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            authoritative,
            ["t-c", "t-a", "t-b"],
            "the pushed snapshot must reach the production resync path"
        );
        let pending = this
            .pending_reorder
            .as_ref()
            .and_then(|pending| pending.display.as_ref());
        assert!(
            reorder_projection(
                &ReorderList::Tabs {
                    workspace_id: "w".into(),
                },
                &authoritative,
                None,
                pending
            )
            .is_none(),
            "the renderer must use conflicting authority, not pending [t-b, t-c, t-a]"
        );
        assert!(
            this.pending_reorder.is_none(),
            "the authoritative snapshot also releases the reorder gate"
        );
    });
}

#[gpui::test]
fn clicking_the_status_bar_reconnects_the_current_session(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_ok_endpoint_rejected();
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();

    view.update(cx, |this, cx| {
        point_local_profile_at_fake(this, &fake);
        this.operation = None;
        this.event_stream = EventStreamState::Lost("event worker stopped".into());
        this.selection.session_name = Some("work".into());
        this.sessions = vec![
            SessionSummary {
                name: "alpha".into(),
                running: true,
                socket_path: PathBuf::new(),
                session_dir: PathBuf::new(),
                default: false,
            },
            SessionSummary {
                name: "work".into(),
                running: true,
                socket_path: PathBuf::new(),
                session_dir: PathBuf::new(),
                default: false,
            },
        ];
        this.session_index = Some(1);
        cx.notify();
    });

    let bounds = cx
        .debug_bounds("reconnect-live-updates")
        .expect("status bar reconnect control should be in the rendered tree");
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    wait_for_event_stream_lost(&view, cx, "endpoint rejected generation");

    view.read_with(cx, |this, _| {
        assert_eq!(
            session_name(this),
            Some("work"),
            "status-bar reconnect must call reload with the current session name, not None and not a no-op"
        );
        assert!(
            matches!(&this.event_stream, EventStreamState::Lost(detail) if detail.contains("endpoint rejected generation")),
            "the reconnect reload must still record the refused handshake as Lost"
        );
    });
}

#[gpui::test]
fn saving_a_host_discards_its_probe_instead_of_restoring_it(cx: &mut TestAppContext) {
    install_app(cx);
    let center = cx.new(|cx| {
        HostCenter::new(
            saved_host_settings(),
            I18n::new(Language::English),
            cx.focus_handle(),
            cx,
        )
    });

    center.update(cx, |center, cx| {
        center.host_health.insert(
            "manual-1".into(),
            HostHealthView::Checking {
                previous: Some(Box::new(ready_health())),
            },
        );
        center.invalidate_probe_for_saved_host("manual-1", cx);
        assert!(
            !center.host_health.contains_key("manual-1"),
            "saving a host must discard the old probe, not restore the previous Cached result"
        );
    });
}

#[gpui::test]
fn ssh_import_stores_a_self_contained_profile(cx: &mut TestAppContext) {
    install_app(cx);
    let center = cx.new(|cx| {
        HostCenter::new(
            saved_host_settings(),
            I18n::new(Language::English),
            cx.focus_handle(),
            cx,
        )
    });

    center.update(cx, |center, cx| {
        center.ssh_candidates = vec!["prod-box".into(), "dev-vm".into()];
        center.finish_ssh_import(
            "prod-box".into(),
            ocherdr_herdr::ResolvedSshHost {
                destination: "deploy@10.0.1.5".into(),
                port: Some(2222),
                identity_file: Some(std::path::PathBuf::from("~/.ssh/prod_key")),
                proxy_jump: Some("bastion".into()),
                has_proxy_command: false,
            },
            cx,
        );

        let index = center
            .profiles
            .iter()
            .position(|profile| profile.id() == "manual-2")
            .expect("imported host lands in the managed catalog");
        let ConnectionProfile::Ssh {
            label,
            destination,
            port,
            identity_file,
            proxy_jump,
            ..
        } = &center.profiles[index]
        else {
            panic!("imported host must be an ssh profile");
        };
        assert_eq!(label, "prod-box");
        assert_eq!(destination, "deploy@10.0.1.5");
        assert_eq!(*port, Some(2222));
        assert_eq!(
            identity_file.as_deref(),
            Some(std::path::Path::new("~/.ssh/prod_key"))
        );
        assert_eq!(proxy_jump.as_deref(), Some("bastion"));
        assert_eq!(center.managed_profile_index, index);
        assert_eq!(
            center.host_metadata["manual-2"].source_alias.as_deref(),
            Some("prod-box"),
            "the origin alias is the dedup key for the import section"
        );
        assert_eq!(center.ssh_candidates, vec!["dev-vm".to_owned()]);
    });
}

#[gpui::test]
fn ssh_section_toggle_flips_collapsed_state(cx: &mut TestAppContext) {
    install_app(cx);
    let center = cx.new(|cx| {
        HostCenter::new(
            saved_host_settings(),
            I18n::new(Language::English),
            cx.focus_handle(),
            cx,
        )
    });

    center.update(cx, |center, cx| {
        assert!(!center.ssh_section_open);
        center.toggle_ssh_section(cx);
        assert!(center.ssh_section_open);
        center.toggle_ssh_section(cx);
        assert!(!center.ssh_section_open);
    });
}

#[gpui::test]
fn ssh_import_persists_without_navigating_the_overlay(cx: &mut TestAppContext) {
    install_app(cx);
    let center = cx.new(|cx| {
        HostCenter::new(
            saved_host_settings(),
            I18n::new(Language::English),
            cx.focus_handle(),
            cx,
        )
    });
    let saved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let captured = saved.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&center, move |_, event: &HostCenterEvent, _| {
            if let HostCenterEvent::HostSaved { id, then, .. } = event {
                captured.borrow_mut().push((id.clone(), *then));
            }
        })
    });

    center.update(cx, |center, cx| {
        center.ssh_candidates = vec!["prod-box".into()];
        center.finish_ssh_import(
            "prod-box".into(),
            ocherdr_herdr::ResolvedSshHost {
                destination: "deploy@10.0.1.5".into(),
                port: None,
                identity_file: None,
                proxy_jump: None,
                has_proxy_command: false,
            },
            cx,
        );
    });

    assert_eq!(
        saved.borrow().as_slice(),
        &[("manual-2".to_owned(), HostSaveThen::Stay)],
        "a background import must persist without disturbing open forms or overlays"
    );
}

#[gpui::test]
fn ssh_import_migrates_legacy_ssh_config_organization(cx: &mut TestAppContext) {
    install_app(cx);
    let center = cx.new(|cx| {
        let mut settings = saved_host_settings();
        settings.host_metadata.insert(
            "ssh-config:prod-box".into(),
            HostMetadata {
                favorite: true,
                group: Some("prod".into()),
                tags: vec!["critical".into()],
                ..HostMetadata::default()
            },
        );
        HostCenter::new(
            settings,
            I18n::new(Language::English),
            cx.focus_handle(),
            cx,
        )
    });

    center.update(cx, |center, cx| {
        center.ssh_candidates = vec!["prod-box".into()];
        center.finish_ssh_import(
            "prod-box".into(),
            ocherdr_herdr::ResolvedSshHost {
                destination: "deploy@10.0.1.5".into(),
                port: None,
                identity_file: None,
                proxy_jump: None,
                has_proxy_command: false,
            },
            cx,
        );

        let metadata = &center.host_metadata["manual-2"];
        assert!(metadata.favorite);
        assert_eq!(metadata.group.as_deref(), Some("prod"));
        assert_eq!(metadata.tags, vec!["critical".to_owned()]);
        assert_eq!(metadata.source_alias.as_deref(), Some("prod-box"));
        assert!(
            !center.host_metadata.contains_key("ssh-config:prod-box"),
            "the legacy entry is consumed by the migration"
        );
    });
}

#[gpui::test]
fn shortcut_help_filters_and_returns_focus_without_terminal_actions(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    cx.simulate_resize(gpui::size(gpui::px(960.), gpui::px(640.)));
    view.update_in(cx, |this, window, cx| {
        this.focus.focus(window, cx);
    });
    cx.run_until_parked();
    let help = crate::shortcuts::COMMANDS
        .iter()
        .find(|(command, _)| *command == crate::shortcuts::Command::Help)
        .unwrap()
        .1;
    cx.simulate_keystrokes(help.stroke());
    cx.run_until_parked();
    view.read_with(cx, |this, cx| {
        assert!(matches!(this.overlay, crate::Overlay::ShortcutHelp));
        assert!(!this.prefix_pending);
        assert_eq!(this.shortcut_help_search.read(cx).content().as_ref(), "");
    });
    let search_before = cx
        .debug_bounds("shortcut-help-search")
        .expect("search field");
    cx.simulate_input("new tab");
    cx.run_until_parked();
    assert_eq!(
        cx.debug_bounds("shortcut-help-search"),
        Some(search_before),
        "filtering must keep the search field in place"
    );
    view.read_with(cx, |this, cx| {
        assert_eq!(
            this.shortcut_help_search.read(cx).content().as_ref(),
            "new tab"
        )
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(matches!(this.overlay, crate::Overlay::None))
    });
}

#[gpui::test]
fn shortcut_help_does_not_replace_an_editing_overlay(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    view.update(cx, |this, cx| {
        this.overlay = crate::Overlay::Appearance;
        this.open_shortcut_help(cx);
        assert!(matches!(this.overlay, crate::Overlay::Appearance));
    });
}

#[gpui::test]
fn notification_center_keeps_shell_shortcuts_and_closing_key_release(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    view.update_in(cx, |this, window, cx| {
        this.focus.focus(window, cx);
        this.notifications.update(cx, |host, cx| {
            let id = host.error("Connection failed", "Test detail", cx);
            host.show_details(id, cx);
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-b");
    view.read_with(cx, |this, cx| {
        assert!(this.notifications.read(cx).is_center_open());
        assert!(
            !this.prefix_pending,
            "notification input must not arm terminal shortcuts"
        );
        assert!(matches!(this.overlay, crate::Overlay::None));
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.update_in(cx, |this, window, cx| {
        assert!(!this.notifications.read(cx).is_center_open());
        assert!(this.focus.is_focused(window));
        assert!(
            this.consumed_key_releases
                .consume(&gpui::Keystroke::parse("escape").unwrap()),
            "the closing Escape release must not be forwarded to the terminal"
        );
    });
}
