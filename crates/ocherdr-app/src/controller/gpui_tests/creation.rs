use super::relocation::send_events;
use super::*;

pub(super) fn created_tab() -> TabInfo {
    TabInfo {
        tab_id: "t-new".into(),
        workspace_id: "w".into(),
        number: 4,
        label: "4".into(),
        focused: true,
        pane_count: 1,
        agent_status: AgentStatus::Idle,
    }
}

pub(super) fn created_pane() -> PaneInfo {
    let mut pane = split_pane("p-new", true);
    pane.tab_id = "t-new".into();
    pane
}

pub(super) fn created_workspace() -> WorkspaceInfo {
    WorkspaceInfo {
        workspace_id: "w-new".into(),
        number: 2,
        label: "fresh".into(),
        focused: true,
        pane_count: 1,
        tab_count: 1,
        active_tab_id: "t-w-new".into(),
        agent_status: AgentStatus::Idle,
        tokens: Default::default(),
        worktree: None,
    }
}

pub(super) fn created_workspace_tab() -> TabInfo {
    TabInfo {
        tab_id: "t-w-new".into(),
        workspace_id: "w-new".into(),
        number: 1,
        label: "1".into(),
        focused: true,
        pane_count: 1,
        agent_status: AgentStatus::Idle,
    }
}

pub(super) fn created_workspace_pane() -> PaneInfo {
    let mut pane = split_pane("p-w-new", true);
    pane.workspace_id = "w-new".into();
    pane.tab_id = "t-w-new".into();
    pane
}

/// What Herdr broadcasts for `tab.create`: `tab.created → pane.created`
/// (`tab.focused` too, which the sticky selection ignores on purpose).
fn tab_create_events() -> Vec<Value> {
    vec![
        json!({ "event": "tab_created", "data": { "type": "tab_created", "tab": created_tab() } }),
        json!({ "event": "pane_created", "data": { "type": "pane_created", "pane": created_pane() } }),
        json!({ "event": "tab_focused", "data": { "type": "tab_focused", "tab_id": "t-new", "workspace_id": "w" } }),
    ]
}

fn workspace_create_events() -> Vec<Value> {
    vec![
        json!({
            "event": "workspace_created",
            "data": { "type": "workspace_created", "workspace": created_workspace() }
        }),
        json!({
            "event": "tab_created",
            "data": { "type": "tab_created", "tab": created_workspace_tab() }
        }),
        json!({
            "event": "pane_created",
            "data": { "type": "pane_created", "pane": created_workspace_pane() }
        }),
    ]
}

fn connect_three_tab_view(
    cx: &mut TestAppContext,
    script: PaneMoveScript,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events_and_script(three_tab_snapshot(), script);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.read_with(cx, |this, _| {
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });
    (fake, view, cx)
}

fn assert_selected(
    view: &Entity<OcHerdrView>,
    cx: &mut VisualTestContext,
    ids: (&str, &str, &str),
) {
    view.read_with(cx, |this, _| {
        assert_eq!(this.selection.workspace_id.as_deref(), Some(ids.0));
        assert_eq!(this.selection.tab_id.as_deref(), Some(ids.1));
        assert_eq!(this.selection.pane_id.as_deref(), Some(ids.2));
        assert!(this.pending_created_tab.is_none(), "nothing left pending");
    });
}

#[gpui::test]
fn a_created_tab_is_selected_when_its_events_beat_the_response(cx: &mut TestAppContext) {
    let script = PaneMoveScript::default();
    *script.events_before_create_response.lock().unwrap() = tab_create_events();
    let (fake, view, cx) = connect_three_tab_view(cx, script);

    view.update(cx, |this, cx| this.create_tab(cx));
    cx.run_until_parked();

    let creates = fake.requests_for("tab.create");
    assert_eq!(creates.len(), 1);
    assert_eq!(
        creates[0]["params"],
        json!({ "workspace_id": "w", "focus": true, "env": {} })
    );
    assert_selected(&view, cx, ("w", "t-new", "p-new"));
}

#[gpui::test]
fn a_created_tab_is_selected_once_its_events_arrive_after_the_response(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_three_tab_view(cx, PaneMoveScript::default());

    view.update(cx, |this, cx| this.create_tab(cx));
    cx.run_until_parked();

    assert_eq!(fake.requests_for("tab.create").len(), 1);
    view.read_with(cx, |this, _| {
        assert_eq!(
            this.selection.tab_id.as_deref(),
            Some("t-a"),
            "the tab is not in the snapshot yet, so the selection stays put"
        );
        assert_eq!(this.pending_created_tab.as_deref(), Some("t-new"));
    });

    send_events(&fake, tab_create_events(), cx);

    assert_selected(&view, cx, ("w", "t-new", "p-new"));
}

#[gpui::test]
fn a_tab_created_elsewhere_does_not_steal_the_selection(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_three_tab_view(cx, PaneMoveScript::default());

    send_events(&fake, tab_create_events(), cx);

    view.read_with(cx, |this, _| {
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
        assert!(this.pending_created_tab.is_none());
    });
}

#[gpui::test]
fn a_created_workspace_is_selected_with_its_first_tab_and_pane(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_three_tab_view(cx, PaneMoveScript::default());

    view.update(cx, |this, cx| this.create_workspace(cx));
    cx.run_until_parked();
    assert_eq!(fake.requests_for("workspace.create").len(), 1);
    view.read_with(cx, |this, _| {
        assert_eq!(this.selection.workspace_id.as_deref(), Some("w"));
        assert_eq!(this.pending_created_tab.as_deref(), Some("t-w-new"));
    });

    send_events(&fake, workspace_create_events(), cx);

    assert_selected(&view, cx, ("w-new", "t-w-new", "p-w-new"));
}

#[gpui::test]
fn a_created_workspace_is_selected_when_its_events_beat_the_response(cx: &mut TestAppContext) {
    let script = PaneMoveScript::default();
    *script.events_before_create_response.lock().unwrap() = workspace_create_events();
    let (_fake, view, cx) = connect_three_tab_view(cx, script);

    view.update(cx, |this, cx| this.create_workspace(cx));
    cx.run_until_parked();

    assert_selected(&view, cx, ("w-new", "t-w-new", "p-w-new"));
}

#[gpui::test]
fn clicking_an_agent_row_jumps_to_its_pane_and_asks_herdr_to_focus_it(cx: &mut TestAppContext) {
    let fake = FakeAgentHerdr::new(PromptReply::Success);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        this.headless_terminals = true;
        this.selection.tab_id = None;
        this.selection.pane_id = None;
        cx.notify();
    });

    click_agent_row(cx);

    let focuses = fake.requests_for("agent.focus");
    assert_eq!(focuses.len(), 1, "one agent.focus for the clicked pane");
    assert_eq!(focuses[0].get("params"), Some(&json!({ "target": "p1" })));
    assert!(
        fake.requests_for("agent.read").is_empty(),
        "a click does not open the panel"
    );
    view.read_with(cx, |this, _| {
        assert_eq!(this.selection.workspace_id.as_deref(), Some("w1"));
        assert_eq!(this.selection.tab_id.as_deref(), Some("t1"));
        assert_eq!(this.selection.pane_id.as_deref(), Some("p1"));
        assert!(
            matches!(this.overlay, crate::Overlay::None),
            "the agent panel stays closed on a single click"
        );
    });
}

#[gpui::test]
fn right_clicking_an_agent_row_offers_details_which_opens_the_panel(cx: &mut TestAppContext) {
    let fake = FakeAgentHerdr::new(PromptReply::Success);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        this.headless_terminals = true;
        cx.notify();
    });

    let center = agent_row_center(cx);
    cx.simulate_mouse_down(center, gpui::MouseButton::Right, gpui::Modifiers::default());
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            matches!(&this.overlay, crate::Overlay::ContextMenu(menu) if menu.agent_details),
            "secondary click opens the agent row's context menu"
        );
    });
    assert!(fake.requests_for("agent.focus").is_empty());

    let details = cx
        .debug_bounds("agent-menu-details")
        .expect("Details entry rendered");
    cx.simulate_click(details.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    view.read_with(cx, |this, _| {
        assert!(
            matches!(&this.overlay, crate::Overlay::AgentPanel { pane_id } if pane_id == "p1"),
            "Details opens the agent panel"
        );
    });
    assert_eq!(fake.requests_for("agent.read").len(), 1);
}

#[gpui::test]
fn double_clicking_an_agent_row_opens_the_panel(cx: &mut TestAppContext) {
    let fake = FakeAgentHerdr::new(PromptReply::Success);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        this.headless_terminals = true;
        cx.notify();
    });

    double_click_agent_row(cx);

    view.read_with(cx, |this, _| {
        assert!(
            matches!(&this.overlay, crate::Overlay::AgentPanel { pane_id } if pane_id == "p1"),
            "the second click of a double-click opens the panel"
        );
    });
    assert_eq!(fake.requests_for("agent.read").len(), 1);
}
