use std::collections::HashMap;

use ocherdr_core::WorkspaceWorktreeInfo;

use super::*;

fn discovered_session(name: &str, default: bool, running: bool) -> SessionSummary {
    SessionSummary {
        name: name.into(),
        default,
        running,
        socket_path: PathBuf::new(),
        session_dir: PathBuf::new(),
    }
}

#[test]
fn fresh_connections_prefer_default_without_a_session_picker() {
    let sessions = vec![
        discovered_session("named", false, true),
        discovered_session("default", true, true),
    ];
    assert_eq!(preferred_session_index(&sessions, None), Some(1));
    assert_eq!(preferred_session_index(&sessions, Some("named")), Some(0));

    let fallback = vec![
        discovered_session("default", true, false),
        discovered_session("running", false, true),
    ];
    assert_eq!(preferred_session_index(&fallback, None), Some(1));
}

fn persist_notice(kind: FailureKind) -> SettingsPersist {
    SettingsPersist {
        config_error: Some(kind),
        host: None,
        rollback: None,
        domains: PersistDomains {
            config: true,
            ..Default::default()
        },
    }
}

fn revertible_persist(kind: FailureKind, tag: &str) -> SettingsPersist {
    SettingsPersist {
        config_error: None,
        host: Some(HostPersistFollowUp::Revertible { error: kind }),
        rollback: Some(HostRollback::tagged(tag)),
        domains: PersistDomains {
            connections: true,
            ..Default::default()
        },
    }
}

#[test]
fn pane_not_found_agent_status_subscribe_failure_resyncs() {
    let error = HerdrError::Api {
        code: "pane_not_found".into(),
        message: "pane w19:p3 not found".into(),
    };
    assert_eq!(
        agent_status_subscribe_failure_action(&error),
        AgentStatusSubscribeFailureAction::Resync
    );
}

#[test]
fn other_agent_status_subscribe_api_failures_are_reported() {
    let error = HerdrError::Api {
        code: "unknown_type".into(),
        message: "subscription rejected".into(),
    };
    assert_eq!(
        agent_status_subscribe_failure_action(&error),
        AgentStatusSubscribeFailureAction::Report
    );
}

#[test]
fn non_api_agent_status_subscribe_failures_are_reported() {
    let error = HerdrError::EventStreamClosed("socket closed".into());
    assert_eq!(
        agent_status_subscribe_failure_action(&error),
        AgentStatusSubscribeFailureAction::Report
    );
}

#[test]
fn settings_persist_keeps_only_the_latest_unwritten_value() {
    let mut pending = None;
    let started = enqueue_settings_persist(
        &mut pending,
        false,
        persist_notice(FailureKind::SaveAppearance),
    );
    assert_eq!(
        started.and_then(|request| request.config_error),
        Some(FailureKind::SaveAppearance)
    );
    assert!(pending.is_none());

    assert!(
        enqueue_settings_persist(
            &mut pending,
            true,
            persist_notice(FailureKind::SaveLanguage)
        )
        .is_none()
    );
    assert!(
        enqueue_settings_persist(
            &mut pending,
            true,
            persist_notice(FailureKind::SaveAppearance)
        )
        .is_none()
    );
    assert_eq!(
        pending.and_then(|request| request.config_error),
        Some(FailureKind::SaveAppearance)
    );
}

#[test]
fn settings_persist_keeps_a_host_follow_up_when_appearance_replaces_the_waiting_write() {
    let host = HostPersistFollowUp::Revertible {
        error: FailureKind::SaveHost,
    };
    let merged = merge_settings_persist(
        SettingsPersist {
            config_error: None,
            host: Some(host),
            rollback: Some(HostRollback::tagged("before-host")),
            domains: PersistDomains {
                connections: true,
                ..Default::default()
            },
        },
        persist_notice(FailureKind::SaveAppearance),
    );
    assert_eq!(merged.config_error, Some(FailureKind::SaveAppearance));
    assert!(matches!(
        merged.host,
        Some(HostPersistFollowUp::Revertible {
            error: FailureKind::SaveHost,
            ..
        })
    ));
    assert_eq!(
        merged.rollback.as_ref().and_then(HostRollback::tag),
        Some("before-host")
    );
    assert!(merged.domains.connections);
    assert!(merged.domains.config);
}

#[test]
fn merged_revertible_persists_keep_the_earliest_rollback() {
    let merged = merge_settings_persist(
        revertible_persist(FailureKind::UpdateFavorites, "before-first"),
        revertible_persist(FailureKind::ApplyOrganization, "before-second"),
    );
    assert!(matches!(
        merged.host,
        Some(HostPersistFollowUp::Revertible {
            error: FailureKind::ApplyOrganization,
            ..
        })
    ));
    assert_eq!(
        merged.rollback.as_ref().and_then(HostRollback::tag),
        Some("before-first")
    );
}

#[test]
fn a_failed_host_write_keeps_a_queued_write_for_a_different_host() {
    let mut pending = Some(revertible_persist(
        FailureKind::UpdateFavorites,
        "after-alpha-before-beta",
    ));
    let applied =
        persist_failure_rollback(&mut pending, Some(HostRollback::tagged("before-alpha")));
    let pending = pending.expect("beta persist stays queued");
    assert!(applied.is_none());
    assert!(matches!(
        pending.host,
        Some(HostPersistFollowUp::Revertible {
            error: FailureKind::UpdateFavorites,
            ..
        })
    ));
    assert_eq!(
        pending.rollback.as_ref().and_then(HostRollback::tag),
        Some("before-alpha")
    );
}

fn test_pane(pane_id: &str, tab_id: &str) -> PaneInfo {
    PaneInfo {
        pane_id: pane_id.into(),
        terminal_id: pane_id.into(),
        workspace_id: "w".into(),
        tab_id: tab_id.into(),
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
        revision: 0,
    }
}

fn test_tab(tab_id: &str, number: usize, label: &str) -> ocherdr_core::TabInfo {
    ocherdr_core::TabInfo {
        tab_id: tab_id.into(),
        workspace_id: "w".into(),
        number,
        label: label.into(),
        focused: number == 1,
        pane_count: 1,
        agent_status: AgentStatus::Idle,
    }
}

fn two_tab_snapshot() -> HierarchySnapshot {
    HierarchySnapshot {
        tabs: vec![test_tab("t-a", 1, "alpha"), test_tab("t-b", 2, "beta")],
        panes: vec![test_pane("p-a", "t-a"), test_pane("p-b", "t-b")],
        layouts: vec![
            ocherdr_core::PaneLayout {
                workspace_id: "w".into(),
                tab_id: "t-a".into(),
                zoomed: false,
                area: ocherdr_core::LayoutRect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 50,
                },
                focused_pane_id: "p-a".into(),
                panes: vec![ocherdr_core::LayoutPane {
                    pane_id: "p-a".into(),
                    focused: true,
                    rect: ocherdr_core::LayoutRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 50,
                    },
                }],
                splits: Vec::new(),
            },
            ocherdr_core::PaneLayout {
                workspace_id: "w".into(),
                tab_id: "t-b".into(),
                zoomed: false,
                area: ocherdr_core::LayoutRect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 50,
                },
                focused_pane_id: "p-b".into(),
                panes: vec![ocherdr_core::LayoutPane {
                    pane_id: "p-b".into(),
                    focused: true,
                    rect: ocherdr_core::LayoutRect {
                        x: 0,
                        y: 0,
                        width: 50,
                        height: 50,
                    },
                }],
                splits: Vec::new(),
            },
        ],
        ..Default::default()
    }
}

fn status_event(pane_id: &str, status: AgentStatus, agent: &str) -> HerdrEvent {
    HerdrEvent::PaneAgentStatusChanged {
        pane_id: pane_id.into(),
        workspace_id: "w".into(),
        agent_status: status,
        agent: Some(agent.into()),
        title: None,
        display_agent: Some("Codex".into()),
        state_labels: HashMap::new(),
    }
}

#[test]
fn agent_notifications_require_a_real_working_to_terminal_transition() {
    let mut snapshot = two_tab_snapshot();
    snapshot.panes[0].agent = Some("codex".into());
    snapshot.panes[0].display_agent = Some("Codex".into());
    snapshot.panes[0].agent_status = AgentStatus::Working;

    let notices = events::agent_system_notifications(
        &snapshot,
        &[
            Ok(status_event("p-a", AgentStatus::Done, "codex")),
            Ok(status_event("p-a", AgentStatus::Done, "codex")),
            Ok(status_event("p-a", AgentStatus::Working, "codex")),
            Ok(status_event("p-a", AgentStatus::Blocked, "codex")),
        ],
        None,
    );
    assert_eq!(notices.len(), 2, "repeated terminal states must not notify");

    snapshot.panes[0].agent_status = AgentStatus::Idle;
    assert!(
        events::agent_system_notifications(
            &snapshot,
            &[Ok(status_event("p-a", AgentStatus::Done, "codex"))],
            None,
        )
        .is_empty(),
        "an initial idle/done snapshot replay is not a completed work transition"
    );
}

#[test]
fn agent_notifications_ignore_visible_and_stale_agent_events() {
    let mut snapshot = two_tab_snapshot();
    snapshot.panes[0].agent = Some("codex".into());
    snapshot.panes[0].agent_status = AgentStatus::Working;
    let event = status_event("p-a", AgentStatus::Done, "codex");
    assert!(events::agent_system_notifications(&snapshot, &[Ok(event)], Some("p-a")).is_empty());
    assert!(
        events::agent_system_notifications(
            &snapshot,
            &[Ok(status_event("p-a", AgentStatus::Done, "claude"))],
            None,
        )
        .is_empty()
    );
}

#[test]
fn cmd_w_closes_the_selected_split_pane() {
    let mut snapshot = two_tab_snapshot();
    snapshot.panes.push(test_pane("p-a2", "t-a"));
    snapshot.tabs[0].pane_count = 2;
    match cmd_w_close_target(&snapshot, "t-a", Some("p-a2")) {
        Some(HierarchyTarget::Pane { id, .. }) => assert_eq!(id, "p-a2"),
        other => panic!("expected pane close, got {other:?}"),
    }
}

#[test]
fn cmd_w_closes_the_tab_when_it_is_the_last_pane() {
    let snapshot = two_tab_snapshot();
    match cmd_w_close_target(&snapshot, "t-a", Some("p-a")) {
        Some(HierarchyTarget::Tab { id, label }) => {
            assert_eq!(id, "t-a");
            assert_eq!(label, "alpha");
        }
        other => panic!("expected tab close, got {other:?}"),
    }
}

#[test]
fn stale_observe_frames_do_not_replace_or_paint_the_local_display_grid() {
    assert!(!incoming_frame_should_apply(false, (80, 24), (80, 24)));
    assert!(!incoming_frame_should_apply(true, (120, 40), (80, 24)));
    assert!(incoming_frame_should_apply(true, (120, 40), (120, 40)));
}

fn target(pane_id: &str, mode: TerminalMode, focused: bool) -> PaneRuntimeTarget {
    PaneRuntimeTarget {
        pane_id: pane_id.into(),
        mode,
        focused,
    }
}

#[test]
fn tiny_pane_grids_stay_as_shells_until_herdrs_minimum_fits() {
    assert!(!pane_grid_mountable(3, 24));
    assert!(!pane_grid_mountable(80, 1));
    assert!(pane_grid_mountable(4, 2));
}

#[test]
fn session_targets_every_snapshot_pane_as_observers_without_control_intent() {
    let snapshot = two_tab_snapshot();
    let targets = snapshot_runtime_targets(&snapshot, &HashMap::new(), Some("t-a"), Some("p-a"));
    assert_eq!(
        targets,
        vec![
            target("p-a", TerminalMode::Observe, true),
            target("p-b", TerminalMode::Observe, false),
        ]
    );
}

#[test]
fn newly_visible_panes_attempt_non_takeover_control_only_once() {
    let mut session = SessionPanes::new(SessionKey {
        profile_id: "ssh:host".into(),
        session_name: "work".into(),
    });
    let live = HashSet::from(["p-a".to_owned(), "p-b".to_owned()]);
    let first_visible = HashSet::from(["p-a".to_owned()]);

    prime_automatic_terminal_control(&mut session, &first_visible, &live);
    assert_eq!(session.controls.get("p-a"), Some(&TerminalMode::Control));
    assert!(demote_terminal_control(&mut session, "p-a"));

    prime_automatic_terminal_control(&mut session, &first_visible, &live);
    assert!(
        !session.controls.contains_key("p-a"),
        "a busy automatic attempt must fall back without reconnecting"
    );

    let second_visible = HashSet::from(["p-b".to_owned()]);
    prime_automatic_terminal_control(&mut session, &second_visible, &live);
    assert_eq!(session.controls.get("p-b"), Some(&TerminalMode::Control));

    let remaining = HashSet::from(["p-b".to_owned()]);
    prime_automatic_terminal_control(&mut session, &second_visible, &remaining);
    assert!(!session.automatic_control_attempts.contains("p-a"));
}

#[test]
fn explicit_control_targets_the_requested_visible_pane() {
    let snapshot = two_tab_snapshot();
    let controls = HashMap::from([("p-b".to_owned(), TerminalMode::Control)]);
    let targets = snapshot_runtime_targets(&snapshot, &controls, Some("t-b"), Some("p-b"));
    assert_eq!(
        targets,
        vec![
            target("p-a", TerminalMode::Observe, false),
            target("p-b", TerminalMode::Control, true),
        ]
    );
    assert_eq!(
        snapshot_pane_ids(&snapshot),
        ["p-a", "p-b"]
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>()
    );
}

#[test]
fn explicit_controls_coexist_while_focus_stays_on_the_selected_pane() {
    let mut snapshot = two_tab_snapshot();
    snapshot.panes.push(test_pane("p-a2", "t-a"));
    let controls = HashMap::from([
        ("p-a".to_owned(), TerminalMode::Control),
        ("p-a2".to_owned(), TerminalMode::ControlTakeover),
        // Hidden targets are not applied to cached runtimes. The retained
        // control intent restores this mode when the tab becomes visible.
        ("p-b".to_owned(), TerminalMode::Control),
    ]);
    let targets = snapshot_runtime_targets(&snapshot, &controls, Some("t-a"), Some("p-a2"));
    assert_eq!(
        targets,
        vec![
            target("p-a", TerminalMode::Control, false),
            target("p-b", TerminalMode::Observe, false),
            target("p-a2", TerminalMode::ControlTakeover, true),
        ]
    );
    assert_eq!(
        targets.iter().filter(|target| target.focused).count(),
        1,
        "focus stays with the selected pane"
    );
    assert_eq!(
        visible_pane_plan(Some(TerminalMode::Control), false, TerminalMode::Control),
        VisiblePanePlan::Keep,
        "changing focus must not respawn the owned stream"
    );
}

#[test]
fn a_grid_change_reports_the_size_for_every_terminal_stream() {
    let panes = ["controlled", "takeover", "observer"];
    let reported = panes
        .iter()
        .filter(|_| {
            pane_resize_plan(false, (800, 600), (640, 480)) == PaneResizePlan::ResizeAndReport
        })
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(reported, panes);
    assert_eq!(
        pane_resize_plan(false, (800, 600), (800, 600)),
        PaneResizePlan::Skip
    );
    // Frozen while a relocation or split drag previews geometry; the
    // release re-measures and then pushes.
    assert_eq!(
        pane_resize_plan(true, (800, 600), (640, 480)),
        PaneResizePlan::Skip
    );
    assert_eq!(
        pane_resize_plan(false, (800, 600), (640, 480)),
        PaneResizePlan::ResizeAndReport
    );
}

#[test]
fn transient_pane_measurements_coalesce_until_the_latest_size_settles() {
    let pending = PendingPaneResize {
        serial: 7,
        pixels: (640, 480),
        scale_factor: 2.,
    };
    assert_eq!(
        pane_resize_schedule(false, (800, 600), None, (640, 480), 2.),
        PaneResizeSchedule::Replace
    );
    assert_eq!(
        pane_resize_schedule(false, (800, 600), Some(pending), (640, 480), 2.),
        PaneResizeSchedule::Keep,
        "the same measurement does not restart its timer"
    );
    assert_eq!(
        pane_resize_schedule(false, (800, 600), Some(pending), (720, 480), 2.),
        PaneResizeSchedule::Replace,
        "new geometry supersedes the stale timer"
    );
    assert_eq!(
        pane_resize_schedule(true, (800, 600), Some(pending), (720, 480), 2.),
        PaneResizeSchedule::Cancel,
        "a drag freeze invalidates a pending resize"
    );
    assert_eq!(
        pane_resize_schedule(false, (800, 600), Some(pending), (800, 600), 2.),
        PaneResizeSchedule::Cancel,
        "returning to the committed size invalidates a transient resize"
    );
}

#[test]
fn tab_switch_flushes_visible_or_newly_spawned_panes() {
    assert!(should_flush_session_pane(Some("t-a"), Some("t-a"), false));
    assert!(!should_flush_session_pane(Some("t-b"), Some("t-a"), false));
    assert!(should_flush_session_pane(Some("t-b"), Some("t-a"), true));
    assert!(should_flush_session_pane(Some("t-b"), None, true));
}

#[test]
fn tab_switch_keeps_snapshot_panes_instead_of_only_the_visible_tab() {
    let cached = ["p-a", "p-b", "closed"]
        .into_iter()
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let snapshot = two_tab_snapshot();
    let live = snapshot_pane_ids(&snapshot);
    let kept = cached.intersection(&live).cloned().collect::<HashSet<_>>();
    assert!(kept.contains("p-a"));
    assert!(kept.contains("p-b"));
    assert!(!kept.contains("closed"));
}

#[test]
fn switching_the_selected_pane_keeps_the_local_surface() {
    assert_eq!(
        visible_pane_plan(Some(TerminalMode::Observe), false, TerminalMode::Observe),
        VisiblePanePlan::Keep
    );
    assert_eq!(
        visible_pane_plan(Some(TerminalMode::Control), false, TerminalMode::Control),
        VisiblePanePlan::Keep
    );
    assert_eq!(
        visible_pane_plan(Some(TerminalMode::Observe), false, TerminalMode::Control),
        VisiblePanePlan::PromoteToControl
    );
    assert_eq!(
        visible_pane_plan(Some(TerminalMode::Control), false, TerminalMode::Observe),
        VisiblePanePlan::DemoteToObserve
    );
    assert_eq!(
        visible_pane_plan(None, false, TerminalMode::Observe),
        VisiblePanePlan::Spawn
    );
    assert_ne!(
        visible_pane_plan(Some(TerminalMode::Observe), false, TerminalMode::Control),
        VisiblePanePlan::Spawn
    );
    assert_ne!(
        visible_pane_plan(Some(TerminalMode::Control), false, TerminalMode::Observe),
        VisiblePanePlan::Spawn
    );
}

#[test]
fn switching_sessions_does_not_reuse_the_previous_session_panes() {
    let current = SessionKey {
        profile_id: "local".into(),
        session_name: "work".into(),
    };
    let incoming = SessionKey {
        profile_id: "local".into(),
        session_name: "other".into(),
    };
    assert_eq!(
        session_panes_plan(Some(&current), &incoming),
        SessionPanesPlan::Replace
    );
}

#[test]
fn reloading_the_same_session_keeps_existing_session_panes() {
    let owner = SessionKey {
        profile_id: "local".into(),
        session_name: "work".into(),
    };
    assert_eq!(
        session_panes_plan(Some(&owner), &owner),
        SessionPanesPlan::Keep
    );
}

#[test]
fn a_closed_stream_is_respawned_instead_of_kept() {
    assert_eq!(
        visible_pane_plan(Some(TerminalMode::Control), true, TerminalMode::Control),
        VisiblePanePlan::Spawn
    );
    assert_eq!(
        visible_pane_plan(Some(TerminalMode::Observe), true, TerminalMode::Observe),
        VisiblePanePlan::Spawn
    );
}

#[test]
fn a_process_exit_closes_the_stream_without_an_app_error() {
    assert!(is_expected_terminal_exit(&HerdrError::TerminalClosed(
        "terminal t1 exited".into()
    )));
    assert!(is_expected_terminal_exit(&HerdrError::TerminalClosed(
        "terminal worker stopped".into()
    )));
    assert!(!is_expected_terminal_exit(&HerdrError::Protocol(
        "frame gap".into()
    )));
}

#[test]
fn lost_control_demotes_the_pane_to_observe_before_any_reconnect() {
    let mut session = SessionPanes::new(SessionKey {
        profile_id: "local".into(),
        session_name: "work".into(),
    });
    session
        .controls
        .insert("p-a".into(), TerminalMode::ControlTakeover);
    session
        .controls
        .insert("p-b".into(), TerminalMode::ControlTakeover);

    assert!(demote_terminal_control(&mut session, "p-a"));
    assert!(!session.controls.contains_key("p-a"));
    assert_eq!(
        session.controls.get("p-b"),
        Some(&TerminalMode::ControlTakeover),
        "another pane's control must survive this pane's takeover loss"
    );
    assert!(!demote_terminal_control(&mut session, "p-a"));
}

#[test]
fn control_loss_reasons_are_distinguished_from_generic_stream_closure() {
    assert_eq!(
            terminal_control_loss(&HerdrError::TerminalClosed(
                "terminal attach failed: terminal t1 already has an attached client; retry with --takeover".into()
            )),
            Some(TerminalControlLoss::Busy)
        );
    assert_eq!(
        terminal_control_loss(&HerdrError::TerminalClosed(
            "terminal attach taken over".into()
        )),
        Some(TerminalControlLoss::TakenOver)
    );
    assert_eq!(
        terminal_control_loss(&HerdrError::TerminalClosed("terminal t1 exited".into())),
        None
    );
}

#[test]
fn snapshot_refresh_queues_when_one_is_already_in_flight() {
    assert!(snapshot_refresh_should_queue(true));
    assert!(!snapshot_refresh_should_queue(false));
}

#[test]
fn snapshot_handoff_releases_only_when_no_refresh_is_in_flight() {
    assert!(snapshot_handoff_should_release(false));
    assert!(!snapshot_handoff_should_release(true));
}

#[test]
fn pane_move_capability_comes_from_protocol_or_version() {
    assert!(HerdrCapabilities::detect("0.6.0", 14).pane_move);
    assert!(HerdrCapabilities::detect("0.9.2", 20).pane_move);
    assert!(HerdrCapabilities::detect("0.7.0", 0).pane_move);
    assert!(HerdrCapabilities::detect("herdr 0.7.1", 0).pane_move);
    assert!(HerdrCapabilities::detect("v1.0.0-beta.1", 0).pane_move);
    assert!(!HerdrCapabilities::detect("0.6.9", 13).pane_move);
    assert!(HerdrCapabilities::detect("0.7", 13).pane_move);
    assert!(!HerdrCapabilities::detect("", 0).pane_move);
    assert!(!HerdrCapabilities::detect("garbage", 0).pane_move);
    assert!(!HerdrCapabilities::detect("x.y.z", 0).pane_move);
    assert_eq!(
        HerdrCapabilities::default(),
        HerdrCapabilities { pane_move: false }
    );
}

#[test]
fn semver_parsing_is_lenient_but_never_invents_numbers() {
    assert_eq!(parse_semver("0.7.0"), Some([0, 7, 0]));
    assert_eq!(parse_semver("herdr 0.8.1"), Some([0, 8, 1]));
    assert_eq!(parse_semver("v1.2.3-rc.1+build"), Some([1, 2, 3]));
    assert_eq!(parse_semver("2"), Some([2, 0, 0]));
    assert_eq!(parse_semver(""), None);
    assert_eq!(parse_semver("garbage"), None);
}

#[test]
fn unknown_method_errors_are_recognised_by_code_and_message() {
    let api = |code: &str, message: &str| HerdrError::Api {
        code: code.into(),
        message: message.into(),
    };
    assert!(is_unknown_method_error(&api(
        "invalid_request",
        "invalid request: unknown variant `pane.move`, expected one of `pane.list`"
    )));
    assert!(is_unknown_method_error(&api("unknown_method", "nope")));
    assert!(!is_unknown_method_error(&api(
        "invalid_request",
        "invalid request: missing field `target_pane_id`"
    )));
    assert!(!is_unknown_method_error(&api(
        "not_found",
        "pane p-9 not found"
    )));
    assert!(!is_unknown_method_error(&HerdrError::Protocol(
        "empty".into()
    )));
}

#[test]
fn invoke_resyncs_only_commands_that_do_not_emit_events() {
    assert!(command_needs_snapshot_resync("pane.rename"));
    assert!(command_needs_snapshot_resync("pane.close"));
    assert!(!command_needs_snapshot_resync("pane.move"));
    assert!(!command_needs_snapshot_resync("pane.swap"));
    for method in [
        "workspace.create",
        "workspace.close",
        "workspace.rename",
        "tab.create",
        "tab.close",
        "tab.rename",
        "pane.split",
        "pane.zoom",
        "pane.focus_direction",
        "layout.set_split_ratio",
        "workspace.move",
        "tab.move",
        "worktree.create",
        "worktree.open",
        "worktree.remove",
    ] {
        assert!(
            !command_needs_snapshot_resync(method),
            "{method} is pushed back as an event and must not reload the snapshot"
        );
    }
}

fn sample_workspace(id: &str, worktree: Option<WorkspaceWorktreeInfo>) -> WorkspaceInfo {
    WorkspaceInfo {
        workspace_id: id.into(),
        number: 1,
        label: id.into(),
        focused: true,
        pane_count: 1,
        tab_count: 1,
        active_tab_id: format!("{id}:t1"),
        agent_status: AgentStatus::Idle,
        tokens: HashMap::new(),
        worktree,
    }
}

#[test]
fn worktree_create_params_omit_empty_optionals_and_focus() {
    let workspace = sample_workspace("w1", None);
    let params = worktree_create_params(&workspace, "  ", "", " HEAD ", "");
    assert_eq!(params["workspace_id"], "w1");
    assert_eq!(params["focus"], true);
    assert_eq!(params["base"], "HEAD");
    assert!(params.get("label").is_none());
    assert!(params.get("branch").is_none());
    assert!(params.get("path").is_none());
    assert!(params.get("force").is_none());
}

#[test]
fn worktree_repo_params_use_parent_cwd_for_linked_checkouts() {
    let parent = sample_workspace("w1", None);
    assert_eq!(
        Value::Object(worktree_repo_params(&parent)),
        json!({ "workspace_id": "w1" })
    );

    let linked = sample_workspace(
        "w2",
        Some(WorkspaceWorktreeInfo {
            repo_key: "/repo/.git".into(),
            repo_name: "repo".into(),
            repo_root: "/repo".into(),
            checkout_path: "/worktrees/repo/feature".into(),
            is_linked_worktree: true,
        }),
    );
    assert_eq!(
        Value::Object(worktree_repo_params(&linked)),
        json!({ "cwd": "/repo" })
    );
}

#[test]
fn worktree_remove_omits_force_unless_the_user_asked() {
    assert_eq!(
        worktree_remove_params("w1", false),
        json!({ "workspace_id": "w1" })
    );
    assert_eq!(
        worktree_remove_params("w1", true),
        json!({ "workspace_id": "w1", "force": true })
    );
}

#[test]
fn dirty_remove_offers_force_only_for_the_safe_remove_error() {
    let params = json!({ "workspace_id": "w1" });
    let dirty = HerdrError::Api {
        code: "dirty_worktree_requires_force".into(),
        message: "uncommitted changes".into(),
    };
    assert_eq!(
        dirty_worktree_remove_offer("worktree.remove", &params, &dirty).as_deref(),
        Some("w1")
    );
    assert_eq!(
        dirty_worktree_remove_offer(
            "worktree.remove",
            &json!({ "workspace_id": "w1", "force": true }),
            &dirty
        ),
        None
    );
    assert_eq!(
        dirty_worktree_remove_offer("worktree.create", &params, &dirty),
        None
    );
    assert_eq!(
        dirty_worktree_remove_offer(
            "worktree.remove",
            &params,
            &HerdrError::Api {
                code: "not_git_worktree".into(),
                message: "nope".into(),
            }
        ),
        None
    );
}

fn sample_session(name: &str) -> SessionKey {
    SessionKey {
        profile_id: "local".into(),
        session_name: name.into(),
    }
}

fn snapshot_with_workspace(id: &str) -> HierarchySnapshot {
    HierarchySnapshot {
        workspaces: vec![sample_workspace(id, None)],
        ..Default::default()
    }
}

#[test]
fn worktree_list_result_is_ignored_after_the_session_changes() {
    let session_a = sample_session("alpha");
    let session_b = sample_session("beta");
    let loading = Overlay::WorktreeOpen(WorktreeOpenState::Loading {
        owner: session_a.clone(),
        workspace_id: "w1".into(),
    });
    let present = snapshot_with_workspace("w1");
    assert!(worktree_list_applies(
        &loading,
        Some(&session_a),
        "w1",
        &session_a,
        Some(&present),
    ));
    assert!(!worktree_list_applies(
        &loading,
        Some(&session_b),
        "w1",
        &session_a,
        Some(&present),
    ));
    assert!(!worktree_list_applies(
        &loading,
        Some(&session_a),
        "w2",
        &session_a,
        Some(&present),
    ));
    assert!(!worktree_list_applies(
        &Overlay::None,
        Some(&session_a),
        "w1",
        &session_a,
        Some(&present),
    ));
}

#[test]
fn worktree_list_result_is_ignored_after_the_target_workspace_is_gone() {
    let session = sample_session("alpha");
    let loading = Overlay::WorktreeOpen(WorktreeOpenState::Loading {
        owner: session.clone(),
        workspace_id: "w1".into(),
    });
    let present = snapshot_with_workspace("w1");
    let gone = HierarchySnapshot::default();
    assert!(
        worktree_list_applies(&loading, Some(&session), "w1", &session, Some(&present)),
        "a still-open target workspace must still accept the list"
    );
    assert!(
        !worktree_list_applies(&loading, Some(&session), "w1", &session, Some(&gone)),
        "injecting a list result after workspace.closed / matching worktree.removed / resync dropped the target must fail this test"
    );
    assert!(
        !worktree_list_applies(&loading, Some(&session), "w1", &session, None),
        "no snapshot means the target workspace is not known to still exist"
    );
    assert!(
        worktree_open_target_is_missing(&loading, Some(&gone)),
        "gate 1 (event/resync) must drop Loading when the pointed-at workspace is gone"
    );
    assert!(!worktree_open_target_is_missing(&loading, Some(&present)));
    let ready_bound = Overlay::WorktreeOpen(WorktreeOpenState::Ready {
        source: WorktreeSourceInfo {
            repo_key: "/repo/.git".into(),
            repo_name: "repo".into(),
            repo_root: "/repo".into(),
            source_checkout_path: "/repo".into(),
            source_workspace_id: Some("w1".into()),
        },
        worktrees: Vec::new(),
    });
    assert!(worktree_open_target_is_missing(&ready_bound, Some(&gone)));
    let ready_by_repo = Overlay::WorktreeOpen(WorktreeOpenState::Ready {
        source: WorktreeSourceInfo {
            repo_key: "/repo/.git".into(),
            repo_name: "repo".into(),
            repo_root: "/repo".into(),
            source_checkout_path: "/repo".into(),
            source_workspace_id: None,
        },
        worktrees: Vec::new(),
    });
    assert!(
        !worktree_open_target_is_missing(&ready_by_repo, Some(&gone)),
        "a cwd-only list is not bound to a workspace id"
    );
}

#[test]
fn disconnecting_the_event_stream_abandons_an_in_flight_worktree_list() {
    let action = poll_event_stream(&mut HierarchySnapshot::default(), || {
        Err(HerdrError::EventStreamClosed("event worker stopped".into()))
    });
    assert!(
        effects_for(&action).abandon_worktree_list,
        "injecting Disconnect without dropping worktree_list_task must fail this test"
    );
    let loading = Overlay::WorktreeOpen(WorktreeOpenState::Loading {
        owner: sample_session("alpha"),
        workspace_id: "w1".into(),
    });
    assert!(
        matches!(
            overlay_after_abandoning_worktree_list(loading),
            Overlay::None
        ),
        "disconnect must clear the Loading overlay, not leave it for a stale list result"
    );
    assert!(!effects_for(&EventPollAction::Applied { reordered: false }).abandon_worktree_list);
    assert!(!effects_for(&EventPollAction::Idle).abandon_worktree_list);
    assert!(!effects_for(&EventPollAction::Resync { error: None }).abandon_worktree_list);
    let closed_worker = EventPollAction::Disconnect("event worker stopped".into());
    assert!(effects_for(&closed_worker).abandon_worktree_list);
}

#[test]
fn abandoning_a_session_clears_only_the_worktree_open_overlay() {
    let loading = Overlay::WorktreeOpen(WorktreeOpenState::Loading {
        owner: sample_session("alpha"),
        workspace_id: "w1".into(),
    });
    assert!(matches!(
        overlay_after_abandoning_worktree_list(loading),
        Overlay::None
    ));
    assert!(matches!(
        overlay_after_abandoning_worktree_list(Overlay::Appearance),
        Overlay::Appearance
    ));
    let create = Overlay::WorktreeCreate {
        workspace_id: "w1".into(),
        advanced: false,
    };
    assert!(matches!(
        overlay_after_abandoning_worktree_list(create),
        Overlay::WorktreeCreate { .. }
    ));
}

#[test]
fn a_rejected_subscription_is_lost_instead_of_idle() {
    let loaded = LoadedEvents::from_subscribe(Err(HerdrError::Api {
        code: "unknown_type".into(),
        message: "events.subscribe rejected".into(),
    }));
    let LoadedEvents::Lost(detail) = loaded else {
        panic!("a failed subscribe is Lost, not Idle");
    };
    assert!(detail.contains("events.subscribe rejected"));
}

#[test]
fn a_successful_subscription_is_live() {
    let (_tx, rx) = futures::channel::mpsc::unbounded();
    let loaded = LoadedEvents::from_subscribe(Ok(EventSubscription::new(rx)));
    assert!(matches!(loaded, LoadedEvents::Live(_)));
}

#[test]
fn a_dead_event_stream_is_marked_lost_instead_of_idle() {
    let next = poll_event_stream(&mut HierarchySnapshot::default(), || {
        Err(HerdrError::EventStreamClosed("event worker stopped".into()))
    })
    .event_stream();
    assert!(
        matches!(next, Some(EventStreamState::Lost(_))),
        "a closed subscription must become Lost, not Idle"
    );
}

#[test]
fn a_dead_event_stream_does_not_reschedule_the_poll() {
    let action = poll_event_stream(&mut HierarchySnapshot::default(), || {
        Err(HerdrError::EventStreamClosed("event worker stopped".into()))
    });
    assert!(
        !effects_for(&action).reschedule,
        "polling a closed stream has nothing left to wait for"
    );
}

#[test]
fn a_quiet_live_stream_keeps_polling_without_refreshing() {
    let action = poll_event_stream(&mut HierarchySnapshot::default(), || Ok(None));
    assert_eq!(action, EventPollAction::Idle);
    assert!(effects_for(&action).reschedule);
    assert!(action.event_stream().is_none());
}

#[test]
fn closing_the_selected_last_tab_resyncs_instead_of_selecting_the_first_remaining_tab() {
    let mut snapshot = two_tab_snapshot();
    snapshot.tabs.push(test_tab("t-c", 3, "gamma"));
    snapshot.panes.push(test_pane("p-c", "t-c"));
    snapshot.focused_workspace_id = Some("w".into());
    snapshot.focused_tab_id = Some("t-c".into());
    snapshot.focused_pane_id = Some("p-c".into());
    snapshot.workspaces.push(ocherdr_core::WorkspaceInfo {
        workspace_id: "w".into(),
        number: 1,
        label: "one".into(),
        focused: true,
        pane_count: 3,
        tab_count: 3,
        active_tab_id: "t-c".into(),
        agent_status: AgentStatus::Idle,
        tokens: HashMap::new(),
        worktree: None,
    });
    let mut selection = Selection {
        connection_id: "local".into(),
        workspace_id: Some("w".into()),
        tab_id: Some("t-c".into()),
        pane_id: Some("p-c".into()),
        session_name: None,
    };
    let mut events = vec![
        Ok(Some(HerdrEvent::PaneClosed {
            workspace_id: "w".into(),
            pane_id: "p-c".into(),
        })),
        Ok(None),
    ]
    .into_iter();
    let action = apply_event_stream(&mut snapshot, &mut selection, || events.next().unwrap());
    assert_eq!(selection.tab_id.as_deref(), Some("t-c"));
    assert_eq!(action, EventPollAction::Resync { error: None });
}

#[test]
fn pane_updated_is_applied_without_resyncing_the_snapshot() {
    let mut snapshot = two_tab_snapshot();
    snapshot.panes[0].revision = 1;
    let mut updated = snapshot.panes[0].clone();
    updated.revision = 9;
    let mut events = vec![
        Ok(Some(HerdrEvent::PaneUpdated {
            pane: updated.clone(),
        })),
        Ok(None),
    ]
    .into_iter();
    let action = poll_event_stream(&mut snapshot, || events.next().unwrap());
    assert_eq!(action, EventPollAction::Applied { reordered: false });
    assert!(!effects_for(&action).resync);
    assert!(effects_for(&action).apply_local);
    assert!(effects_for(&action).reschedule);
    assert!(action.event_stream().is_none());
    assert_eq!(snapshot.panes[0], updated);
}

#[test]
fn only_an_order_publishing_event_reports_a_settled_reorder() {
    let mut snapshot = two_tab_snapshot();
    let mut updated = snapshot.panes[0].clone();
    updated.revision = 9;
    let mut events = vec![
        Ok(Some(HerdrEvent::PaneUpdated { pane: updated })),
        Ok(None),
    ]
    .into_iter();
    assert_eq!(
        poll_event_stream(&mut snapshot, || events.next().unwrap()),
        EventPollAction::Applied { reordered: false },
        "a pane update leaves the order alone, so a pending reorder is still pending"
    );

    let workspaces = snapshot.workspaces.clone();
    let mut events = vec![
        Ok(Some(HerdrEvent::WorkspaceMoved {
            workspace_id: "w".into(),
            insert_index: 0,
            workspaces,
        })),
        Ok(None),
    ]
    .into_iter();
    assert_eq!(
        poll_event_stream(&mut snapshot, || events.next().unwrap()),
        EventPollAction::Applied { reordered: true }
    );
}

#[test]
fn a_pending_reorder_is_released_by_everything_except_an_empty_poll() {
    assert!(effects_for(&EventPollAction::Applied { reordered: true }).settle_reorder);
    assert!(effects_for(&EventPollAction::Resync { error: None }).settle_reorder);
    assert!(
        effects_for(&EventPollAction::Disconnect("stream died".into())).settle_reorder,
        "a dead stream will never deliver the moved event, so the gate must open"
    );
    assert!(!effects_for(&EventPollAction::Applied { reordered: false }).settle_reorder);
    assert!(
        !effects_for(&EventPollAction::Idle).settle_reorder,
        "an empty poll says nothing about the request still in flight"
    );
}

#[test]
fn applied_poll_effects_do_not_resync() {
    let applied = effects_for(&EventPollAction::Applied { reordered: false });
    assert!(!applied.resync);
    assert!(applied.apply_local);
    assert!(applied.notify);
    assert!(applied.reschedule);
    assert!(applied.error.is_none());
    let resync = effects_for(&EventPollAction::Resync { error: None });
    assert!(resync.resync);
    assert!(!resync.apply_local);
    assert!(resync.reschedule);
}

#[test]
fn a_malformed_event_resyncs_without_dropping_the_stream() {
    let mut events = vec![
        Err(HerdrError::Protocol("event is missing `data`".into())),
        Ok(None),
    ]
    .into_iter();
    let action = poll_event_stream(&mut HierarchySnapshot::default(), || events.next().unwrap());
    let EventPollAction::Resync { error } = &action else {
        panic!("payload errors must resync, got {action:?}");
    };
    assert!(
        error
            .as_ref()
            .is_some_and(|detail| detail.contains("`data`"))
    );
    let effects = effects_for(&action);
    assert!(effects.resync);
    assert!(effects.error.is_some());
    assert!(effects.reschedule);
    assert!(action.event_stream().is_none());
}

#[test]
fn wheel_delta_accumulates_into_terminal_scroll_lines() {
    assert_eq!(
        wheel_scroll_lines(ScrollDelta::Lines(point(0., 3.)), 16., &mut 0.),
        3
    );
    assert_eq!(
        wheel_scroll_lines(ScrollDelta::Lines(point(0., -2.4)), 16., &mut 0.),
        -2
    );
    let mut leftover = 0.;
    assert_eq!(
        wheel_scroll_lines(
            ScrollDelta::Pixels(point(px(0.), px(8.))),
            16.,
            &mut leftover
        ),
        0
    );
    assert!((leftover - 8.).abs() < f32::EPSILON);
    assert_eq!(
        wheel_scroll_lines(
            ScrollDelta::Pixels(point(px(0.), px(10.))),
            16.,
            &mut leftover
        ),
        1
    );
    assert!((leftover - 2.).abs() < f32::EPSILON);
}

mod ui_terminal;
