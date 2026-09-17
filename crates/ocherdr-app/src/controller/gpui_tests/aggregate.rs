use super::*;
use std::collections::HashSet;

use crate::controller::HerdrCapabilities;
use crate::{ParkedHostRuntime, SidebarMode};

fn remote_snapshot() -> HierarchySnapshot {
    HierarchySnapshot {
        version: "0.9.1".into(),
        protocol: 22,
        focused_workspace_id: Some("wr".into()),
        focused_tab_id: Some("tr".into()),
        focused_pane_id: Some("p-remote".into()),
        workspaces: vec![WorkspaceInfo {
            workspace_id: "wr".into(),
            number: 1,
            label: "remote ws".into(),
            focused: true,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: "tr".into(),
            agent_status: AgentStatus::Working,
            tokens: HashMap::new(),
            worktree: None,
        }],
        tabs: vec![TabInfo {
            tab_id: "tr".into(),
            workspace_id: "wr".into(),
            number: 1,
            label: "remote".into(),
            focused: true,
            pane_count: 1,
            agent_status: AgentStatus::Working,
        }],
        panes: vec![PaneInfo {
            pane_id: "p-remote".into(),
            terminal_id: "term-p-remote".into(),
            workspace_id: "wr".into(),
            tab_id: "tr".into(),
            focused: true,
            cwd: None,
            foreground_cwd: None,
            label: None,
            agent: Some("codex".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: Some("Codex".into()),
            agent_status: AgentStatus::Working,
            state_labels: HashMap::new(),
            tokens: HashMap::new(),
            revision: 1,
        }],
        layouts: Vec::new(),
        agents: vec![AgentInfo {
            pane_id: "p-remote".into(),
            name: Some("remote-agent".into()),
        }],
    }
}

fn remote_selection() -> Selection {
    Selection {
        connection_id: "manual-1".into(),
        session_name: Some("remote".into()),
        workspace_id: Some("wr".into()),
        tab_id: Some("tr".into()),
        pane_id: Some("p-remote".into()),
    }
}

/// A parked host keeps a fabricated `SessionConnection`: `Local` profiles do
/// pure socket-path math with no I/O, and the dead path is never dialed
/// because tests never trigger a refresh for it.
fn fake_parked_runtime() -> ParkedHostRuntime {
    let session = SessionSummary {
        name: "remote".into(),
        default: false,
        running: true,
        socket_path: PathBuf::from("/tmp/ocherdr-test-nonexistent/herdr.sock"),
        session_dir: PathBuf::from("/tmp/ocherdr-test-nonexistent"),
    };
    ParkedHostRuntime {
        sessions: vec![session.clone()],
        session_index: Some(0),
        connection: SessionConnection::connect(
            &ConnectionProfile::Local {
                herdr_path: "herdr".into(),
            },
            &session,
        )
        .expect("local connection construction is pure"),
        herdr_capabilities: HerdrCapabilities::default(),
        event_stream: EventStreamState::Live,
        event_listen: None,
        agent_status_listen: None,
        agent_status_panes: HashSet::new(),
        agent_status_handoff: None,
        snapshot: Some(remote_snapshot()),
        selection: remote_selection(),
        session_panes: None,
        pane_viewports: HashMap::new(),
        snapshot_refreshing: false,
        snapshot_refresh_pending: false,
    }
}

fn aggregate_view(cx: &mut TestAppContext) -> (Entity<OcHerdrView>, &mut VisualTestContext) {
    install_app(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = OcHerdrView::new(saved_host_settings(), window, cx);
        view.load_epoch = view.load_epoch.wrapping_add(1);
        view.operation = None;
        view.headless_terminals = true;
        view
    });
    view.update(cx, |view, _| {
        view.snapshot = Some(agent_snapshot());
        view.selection = Selection {
            connection_id: "local".into(),
            session_name: Some("default".into()),
            workspace_id: Some("w1".into()),
            tab_id: Some("t1".into()),
            pane_id: Some("p1".into()),
        };
        view.event_stream = EventStreamState::Live;
        view.parked_hosts
            .insert("manual-1".into(), fake_parked_runtime());
    });
    (view, cx)
}

#[gpui::test]
fn aggregate_sidebar_lists_every_connected_machine(cx: &mut TestAppContext) {
    let (view, cx) = aggregate_view(cx);
    view.update(cx, |this, cx| {
        this.sidebar_mode = SidebarMode::Aggregate;
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("agg-host-manual-1").is_some());
    assert!(cx.debug_bounds("agg-workspace-manual-1-wr").is_some());
    assert!(cx.debug_bounds("agg-agent-manual-1-p-remote").is_some());
    // The active host is a group too, with its own ids; pane ids are scoped
    // per machine so equal ids on two hosts must not collide in the tree.
    assert!(cx.debug_bounds("agg-workspace-local-w1").is_some());
    assert!(cx.debug_bounds("agg-agent-local-p1").is_some());
}

#[gpui::test]
fn aggregate_remote_row_click_activates_host_and_targets_pane(cx: &mut TestAppContext) {
    let (view, cx) = aggregate_view(cx);
    view.update(cx, |this, cx| {
        this.sidebar_mode = SidebarMode::Aggregate;
        cx.notify();
    });
    cx.run_until_parked();
    let center = cx
        .debug_bounds("agg-agent-manual-1-p-remote")
        .expect("remote agent row renders")
        .center();
    cx.simulate_click(center, gpui::Modifiers::default());
    cx.run_until_parked();
    view.update(cx, |this, _| {
        assert_eq!(this.current_profile().id(), "manual-1");
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-remote"));
        assert_eq!(this.selection.workspace_id.as_deref(), Some("wr"));
    });
}

#[gpui::test]
fn aggregate_machine_header_collapses_its_group(cx: &mut TestAppContext) {
    let (view, cx) = aggregate_view(cx);
    view.update(cx, |this, cx| {
        this.sidebar_mode = SidebarMode::Aggregate;
        this.collapsed_aggregate_hosts.insert("manual-1".into());
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("agg-agent-manual-1-p-remote").is_none());
    assert!(cx.debug_bounds("agg-workspace-manual-1-wr").is_none());
    assert!(cx.debug_bounds("agg-host-manual-1").is_some());
}

#[gpui::test]
fn sidebar_mode_toggle_returns_to_single_machine_rows(cx: &mut TestAppContext) {
    let (view, cx) = aggregate_view(cx);
    view.update(cx, |this, cx| {
        this.sidebar_mode = SidebarMode::Aggregate;
        cx.notify();
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("agg-host-manual-1").is_some());
    view.update(cx, |this, cx| this.toggle_sidebar_mode(cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("agg-host-manual-1").is_none());
    assert!(cx.debug_bounds("workspace-w1").is_some());
    view.update(cx, |this, _| {
        assert_eq!(this.sidebar_mode, SidebarMode::Single);
        assert!(this.pending_host_target.is_none());
    });
}
