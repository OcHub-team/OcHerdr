use super::*;

#[gpui::test]
fn command_comma_opens_ocherdr_settings_in_place(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    view.update_in(cx, |this, window, cx| {
        this.focus.focus(window, cx);
        let event = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    platform: true,
                    ..Default::default()
                },
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

/// The strip's move areas are laid out as full-height siblings of the
/// controls, so "empty strip space" is exactly what they cover: the gutter
/// before the first tab and everything between `+` and the toolbar. A press
/// there reaches no tab, so selection and the reorder machinery stay put.
#[gpui::test]
fn empty_tab_strip_space_is_a_full_height_window_move_area(cx: &mut TestAppContext) {
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

    let lead = cx
        .debug_bounds("tab-strip-lead")
        .expect("leading move area");
    let space = cx
        .debug_bounds("tab-strip-space")
        .expect("trailing move area");
    let first = cx.debug_bounds("tab-t-a").expect("first tab");
    let last = cx.debug_bounds("tab-t-c").expect("last tab");
    // The strip's content box: its height minus the 1px bottom border.
    assert_eq!(lead.size.height, gpui::px(HEADER_HEIGHT - 1.));
    assert_eq!(space.size.height, gpui::px(HEADER_HEIGHT - 1.));
    assert_eq!(lead.size.width, gpui::px(TAB_STRIP_LEAD_INSET));
    assert!(
        lead.origin.x + lead.size.width <= first.origin.x,
        "the gutter ends where the first tab starts: {lead:?} vs {first:?}"
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
        assert!(!bounds.intersects(&space) && !bounds.intersects(&lead));
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

    let command = gpui::Modifiers {
        platform: true,
        ..Default::default()
    };
    cx.simulate_modifiers_change(command);
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.command_held));
    let hint = cx
        .debug_bounds("tab-shortcut-t-a")
        .expect("hint appears while Command is down");
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

    // Cmd-Tab away: the release happens in another app, so losing key
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
    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(500.)));
    cx.run_until_parked();

    let alpha_before = cx.debug_bounds("tab-t-a").expect("alpha tab bounds");
    let long_before = cx.debug_bounds("tab-t-b").expect("long tab bounds");
    assert_eq!(alpha_before.size.width, gpui::px(TAB_PILL_WIDTH));
    assert_eq!(long_before.size.width, gpui::px(TAB_PILL_WIDTH));
    assert_eq!(cx.debug_bounds("tab-title-fade-t-a"), None);
    assert!(cx.debug_bounds("tab-title-fade-t-b").is_some());
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
fn a_closed_event_poll_marks_the_stream_lost_and_stops_rescheduling(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);

    let keep = view.update(cx, |this, cx| {
        this.event_stream = EventStreamState::Live;
        this.apply_event_batch(None, cx)
    });

    view.read_with(cx, |this, _| {
        assert!(
            matches!(this.event_stream, EventStreamState::Lost(_)),
            "a closed poll must write Lost onto the view, not leave Live/Idle"
        );
    });
    assert!(
        !keep,
        "a closed poll has nothing left to wait for and must not reschedule"
    );
}

#[gpui::test]
fn startup_replay_batch_refreshes_the_visible_current_snapshot_immediately(
    cx: &mut TestAppContext,
) {
    let current = two_pane_snapshot();
    let fake = FakeHerdr::snapshot_with_live_events(current.clone());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();

    view.update(cx, |this, cx| {
        let session = SessionSummary {
            name: "alpha".into(),
            running: true,
            socket_path: fake.socket_path(),
            session_dir: fake._dir.path().join("alpha"),
            default: false,
        };
        this.connection = Some(
            SessionConnection::connect(&this.profiles[0], &session).expect("connect fake Herdr"),
        );
        this.sessions = vec![session];
        this.session_index = Some(0);
        let mut stale = current;
        stale.panes.retain(|pane| pane.pane_id != "p-right");
        this.snapshot = Some(stale);
        this.startup_replay_sync = Some(crate::StartupReplaySync::Draining { serial: 1 });

        assert!(this.apply_event_batch(
            Some(vec![Ok(ocherdr_core::HerdrEvent::LayoutUpdated {
                layout: two_pane_layout("p-right", "p-left"),
            })]),
            cx,
        ));
    });
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert!(
            this.snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.pane("p-right").is_some()),
            "the first replay batch must fetch current state instead of waiting for replay quiet"
        );
    });
    assert!(
        !fake.requests_for("session.snapshot").is_empty(),
        "the replay invalidation must issue an authoritative refresh"
    );
}

#[gpui::test]
fn reload_writes_a_rejected_subscription_as_lost_instead_of_idle(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_ok_subscribe_rejected();
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();

    view.update(cx, |this, cx| {
        point_local_profile_at_fake(this, &fake);
        this.reload(None, cx);
    });
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert!(
            matches!(&this.event_stream, EventStreamState::Lost(detail) if detail.contains("events.subscribe rejected")),
            "reload must write the failed subscribe as Lost, not Idle"
        );
        assert_eq!(
            session_name(this),
            Some("alpha"),
            "the rejected subscribe still selected a running session"
        );
    });
}

#[gpui::test]
fn stale_pane_agent_status_subscribe_resyncs_without_notifying(cx: &mut TestAppContext) {
    let fake = StalePaneHerdr::new();
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();

    view.update(cx, |this, cx| {
        let session = SessionSummary {
            name: "stale-pane-test".into(),
            running: true,
            socket_path: fake.socket_path.clone(),
            session_dir: fake._dir.path().join("stale-pane-test"),
            default: false,
        };
        this.connection = Some(
            SessionConnection::connect(&this.profiles[0], &session)
                .expect("connect stale pane session"),
        );
        this.snapshot = Some(agent_snapshot());
        this.ensure_agent_status_stream(cx);
    });
    cx.run_until_parked();

    let rejection_count = fake.agent_status_rejections.load(Ordering::Relaxed);
    assert!(
        rejection_count > 0,
        "the fixture must prove it matched and rejected pane.agent_status_changed"
    );
    assert_eq!(
        rejection_count, 1,
        "an unchanged pane snapshot must not turn resync into a subscribe loop"
    );
    assert_eq!(
        fake.snapshot_requests.load(Ordering::Relaxed),
        1,
        "pane_not_found must trigger one snapshot resync"
    );
    view.read_with(cx, |this, cx| {
        assert_eq!(
            this.notifications.read(cx).history().count(),
            0,
            "a stale pane set must resync without producing an ApplyLiveUpdate notification"
        );
    });
}

#[gpui::test]
fn a_rejected_tab_move_returns_the_display_to_authoritative_order(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_ok_subscribe_rejected();
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();

    view.update(cx, |this, cx| {
        point_local_profile_at_fake(this, &fake);
        this.reload(None, cx);
    });
    cx.run_until_parked();

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

    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert!(
            this.pending_reorder.is_none(),
            "a rejected request must release both the reorder gate and its prediction"
        );
    });
}

#[gpui::test]
fn a_conflicting_tab_moved_event_replaces_the_pending_projection_with_authority(
    cx: &mut TestAppContext,
) {
    let initial = three_tab_snapshot();
    let fake = FakeHerdr::snapshot_with_live_events(initial);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();

    view.update(cx, |this, cx| {
        point_local_profile_at_fake(this, &fake);
        this.reload(None, cx);
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(crate::STARTUP_REPLAY_QUIET_DELAY);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert_eq!(this.startup_replay_sync, None);
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
            .expect("the accepted move remains pending until a moved event arrives");
        assert_eq!(projection.positions, [2, 0, 1]);
    });

    let authoritative_tabs = vec![
        test_tab("t-c", 3, "gamma"),
        test_tab("t-a", 1, "alpha"),
        test_tab("t-b", 2, "beta"),
    ];
    fake.send_event(json!({
        "event": "tab_moved",
        "data": {
            "type": "tab_moved",
            "workspace_id": "w",
            "tab_id": "t-a",
            "insert_index": 1,
            "tabs": authoritative_tabs,
        }
    }));
    // The socket fixture has proved the write completed. Let the detached
    // production EventStream reader enqueue it before draining GPUI work.
    thread::sleep(Duration::from_millis(20));
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        let snapshot = this
            .snapshot
            .as_ref()
            .expect("the live event applies to the production view snapshot");
        let authoritative = snapshot
            .tabs_for("w")
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            authoritative,
            ["t-c", "t-a", "t-b"],
            "the flushed fake event must reach the production event path"
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
            "the authoritative moved event also releases the reorder gate"
        );
    });
}

#[gpui::test]
fn clicking_the_status_bar_reconnects_the_current_session(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_ok_subscribe_rejected();
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
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert_eq!(
            session_name(this),
            Some("work"),
            "status-bar reconnect must call reload with the current session name, not None and not a no-op"
        );
        assert!(
            matches!(&this.event_stream, EventStreamState::Lost(detail) if detail.contains("events.subscribe rejected")),
            "the reconnect reload must still record the rejected subscribe as Lost"
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
        let index = center
            .profiles
            .iter()
            .position(|profile| profile.id() == "manual-1")
            .expect("saved host is in the catalog");
        center.host_health.insert(
            "manual-1".into(),
            HostHealthView::Checking {
                previous: Some(Box::new(ready_health())),
            },
        );
        center.invalidate_probe_for_saved_host(index, cx);
        assert!(
            !center.host_health.contains_key("manual-1"),
            "saving a host must discard the old probe, not restore the previous Cached result"
        );
    });
}
