use super::*;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
#[cfg(windows)]
use uds_windows::UnixListener;

#[test]
fn quotes_remote_arguments_without_shell_injection() {
    assert_eq!(posix_quote("plain/path-1"), "plain/path-1");
    assert_eq!(posix_quote("a b'c"), "'a b'\"'\"'c'");
    assert_eq!(posix_quote("$(touch nope)"), "'$(touch nope)'");
}

#[test]
fn default_remote_command_discovers_common_install_locations() {
    let command = remote_herdr_command("herdr", &["session", "list", "--json"]);

    assert!(command.contains("herdr_path=$(command -v herdr"));
    assert!(command.contains("\"$HOME/.local/bin/herdr\""));
    assert!(command.contains("/opt/homebrew/bin/herdr"));
    assert!(command.contains("/home/linuxbrew/.linuxbrew/bin/herdr"));
    assert!(command.contains(".local/share/mise/installs/herdr/*/bin/herdr"));
    assert!(command.contains("exec \"$herdr_path\" session list --json"));
}

#[test]
fn remote_command_quotes_arguments_and_honors_a_custom_path() {
    assert_eq!(
        remote_herdr_command("/opt/Herdr bin/herdr", &["$(touch nope)"]),
        "exec '/opt/Herdr bin/herdr' '$(touch nope)'"
    );
    assert_eq!(
        remote_herdr_command("~/.local/bin/herdr", &["--version"]),
        "exec \"$HOME\"/.local/bin/herdr --version"
    );
}

#[test]
fn parses_only_concrete_ssh_hosts() {
    let hosts = parse_ssh_hosts(
        "Host *\n  ServerAliveInterval 15\nHost work work-alt\nHost build-?\nHost work\n",
    );
    assert_eq!(hosts, vec!["work", "work-alt"]);
}

#[test]
fn classifies_actionable_ssh_failures() {
    assert_eq!(
        classify_ssh_failure("Permission denied (publickey)."),
        HostHealthStatus::AuthenticationRequired
    );
    assert_eq!(
        classify_ssh_failure("Host key verification failed."),
        HostHealthStatus::HostKeyRequired
    );
    assert_eq!(
        classify_ssh_failure("ssh: Could not resolve hostname nowhere"),
        HostHealthStatus::Unreachable
    );
}

#[test]
fn compares_decorated_semantic_versions() {
    assert!(version_at_least("herdr 0.8.1", "0.8.1"));
    assert!(version_at_least("0.9.0-beta.1", "0.8.1"));
    assert!(!version_at_least("herdr 0.7.9", "0.8.1"));
}

#[test]
fn simple_globs_match_config_fragments() {
    assert!(wildcard_matches(b"*.conf", b"work.conf"));
    assert!(wildcard_matches(b"host-?", b"host-a"));
    assert!(!wildcard_matches(b"host-?", b"host-prod"));
}

#[test]
fn attach_command_keeps_session_name_quoted() {
    let profile = ConnectionProfile::Ssh {
        id: "server".into(),
        label: "Server".into(),
        destination: "deploy@example.com".into(),
        port: None,
        identity_file: None,
        herdr_path: "/opt/herdr".into(),
    };
    assert_eq!(
        attach_command(&profile, "work one"),
        "ssh -t deploy@example.com 'exec /opt/herdr session attach '\"'\"'work one'\"'\"''"
    );
}

#[test]
fn interactive_ssh_command_preserves_profile_overrides() {
    let profile = ConnectionProfile::Ssh {
        id: "server".into(),
        label: "Server".into(),
        destination: "deploy@example.com".into(),
        port: Some(2202),
        identity_file: Some("/Keys/work key".into()),
        herdr_path: "herdr".into(),
    };
    assert_eq!(
        ssh_login_command(&profile).as_deref(),
        Some("ssh -p 2202 -i '/Keys/work key' deploy@example.com")
    );
}

#[test]
fn private_socket_path_is_derived_exactly_like_herdr() {
    assert_eq!(
        client_socket_path_from_api(Path::new("/tmp/herdr.sock")),
        PathBuf::from("/tmp/herdr-client.sock")
    );
    assert_eq!(
        client_socket_path_from_api(Path::new("/tmp/custom-api")),
        PathBuf::from("/tmp/custom-api-client.sock")
    );
    assert_eq!(SUPPORTED_TERMINAL_PROTOCOL_VERSIONS, &[20]);
    assert_eq!(MAX_CLIPBOARD_IMAGE_BYTES, 16 * 1024 * 1024);
}

#[test]
fn ssh_tunnel_forwards_public_and_private_sockets_in_one_process() {
    let profile = ConnectionProfile::Ssh {
        id: "server".into(),
        label: "Server".into(),
        destination: "deploy@example.com".into(),
        port: Some(2202),
        identity_file: Some("/Keys/work key".into()),
        herdr_path: "herdr".into(),
    };
    let command = ssh_tunnel_command(
        &profile,
        "/tmp/local-api.sock:/remote/herdr.sock",
        "/tmp/local-client.sock:/remote/herdr-client.sock",
    )
    .unwrap();
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let forwards = arguments
        .windows(2)
        .filter(|arguments| arguments[0] == "-L")
        .map(|arguments| arguments[1].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        forwards,
        [
            "/tmp/local-api.sock:/remote/herdr.sock",
            "/tmp/local-client.sock:/remote/herdr-client.sock",
        ]
    );
    assert!(arguments.windows(2).any(|args| args == ["-p", "2202"]));
    assert!(
        arguments
            .windows(2)
            .any(|args| args == ["-i", "/Keys/work key"])
    );
    assert_eq!(
        arguments.last().map(String::as_str),
        Some("deploy@example.com")
    );
}

#[test]
fn an_empty_open_channel_does_not_wake_the_ui() {
    use futures::FutureExt as _;
    let (_tx, mut rx) = futures_mpsc::unbounded::<u8>();
    assert!(
        next_batch(&mut rx).now_or_never().is_none(),
        "空通道不应该唤醒 UI"
    );
}

#[test]
fn a_ready_channel_drains_already_queued_items() {
    use futures::FutureExt as _;
    let (tx, mut rx) = futures_mpsc::unbounded();
    tx.unbounded_send(1).unwrap();
    tx.unbounded_send(2).unwrap();
    tx.unbounded_send(3).unwrap();
    assert_eq!(
        next_batch(&mut rx).now_or_never(),
        Some(Some(vec![1, 2, 3]))
    );
}

#[test]
fn a_closed_channel_ends_the_stream_instead_of_waiting() {
    use futures::FutureExt as _;
    let (tx, mut rx) = futures_mpsc::unbounded::<u8>();
    drop(tx);
    assert_eq!(next_batch(&mut rx).now_or_never(), Some(None));
}

const PANE_ID_REQUIRED_SUBSCRIPTIONS: &[&str] = &[
    "pane.agent_status_changed",
    "pane.scroll_changed",
    "pane.output_matched",
];

#[test]
fn subscription_list_excludes_types_that_require_pane_id() {
    assert_eq!(EVENT_SUBSCRIPTIONS.len(), 24);
    for kind in ["worktree.created", "worktree.opened", "worktree.removed"] {
        assert!(
            EVENT_SUBSCRIPTIONS.contains(&kind),
            "{kind} must be in the session-wide subscribe list"
        );
    }
    for required in PANE_ID_REQUIRED_SUBSCRIPTIONS {
        assert!(
            !EVENT_SUBSCRIPTIONS.contains(required),
            "{required} requires pane_id and must not be in the session-wide subscribe list"
        );
    }
}

#[test]
fn a_subscription_started_ack_succeeds() {
    parse_subscription_ack(r#"{"id":"ocherdr-events-1","result":{"type":"subscription_started"}}"#)
        .unwrap();
}

#[test]
fn event_lines_are_decoded_from_the_data_payload() {
    let event = parse_event_line(
        r#"{"data":{"type":"workspace_focused","workspace_id":"w1"},"event":"workspace_focused"}"#,
    )
    .unwrap();
    assert_eq!(
        event,
        HerdrEvent::WorkspaceFocused {
            workspace_id: "w1".into()
        }
    );
}

#[test]
fn agent_status_event_lines_use_the_dotted_envelope_without_a_data_type() {
    let working = parse_event_line(
        r#"{"data":{"agent":"t21-probe","agent_status":"working","pane_id":"wC:p4","workspace_id":"wC"},"event":"pane.agent_status_changed"}"#,
    )
    .unwrap();
    assert_eq!(
        working,
        HerdrEvent::PaneAgentStatusChanged {
            pane_id: "wC:p4".into(),
            workspace_id: "wC".into(),
            agent_status: AgentStatus::Working,
            agent: Some("t21-probe".into()),
            title: None,
            display_agent: None,
            state_labels: Default::default(),
        }
    );

    let done = parse_event_line(
        r#"{"data":{"agent":"t21-probe","agent_status":"done","pane_id":"wC:p4","title":"t21 title","workspace_id":"wC"},"event":"pane.agent_status_changed"}"#,
    )
    .unwrap();
    let HerdrEvent::PaneAgentStatusChanged {
        agent_status,
        title,
        ..
    } = done
    else {
        panic!("expected pane agent status changed");
    };
    assert_eq!(agent_status, AgentStatus::Done);
    assert_eq!(title.as_deref(), Some("t21 title"));
}

#[test]
fn session_subscriptions_are_the_session_wide_eventhub_list() {
    let subscriptions = session_subscriptions();
    assert_eq!(subscriptions.len(), EVENT_SUBSCRIPTIONS.len());
    assert_eq!(subscriptions[0], json!({"type": EVENT_SUBSCRIPTIONS[0]}));
    assert!(
        !subscriptions
            .iter()
            .any(|value| value.get("pane_id").is_some())
    );
    assert!(EVENT_SUBSCRIPTIONS.contains(&"pane.agent_detected"));
    assert!(!EVENT_SUBSCRIPTIONS.contains(&"pane.agent_status_changed"));
}

#[test]
fn agent_status_subscriptions_are_parameterized_and_separate_from_the_session_list() {
    // Herdr starts parameterized pane.agent_status_changed at
    // current_sequence; session-wide Event types replay from 0.
    let subscriptions = agent_status_subscriptions(&["wC:p1".into(), "wC:p4".into()]);
    assert_eq!(
        subscriptions,
        vec![
            json!({"type": "pane.agent_status_changed", "pane_id": "wC:p1"}),
            json!({"type": "pane.agent_status_changed", "pane_id": "wC:p4"}),
        ]
    );
}

#[test]
fn unknown_event_types_are_unknown_and_broken_payloads_error() {
    assert_eq!(
        parse_event_line(
            r#"{"data":{"type":"some_future_event","whatever":1},"event":"some_future_event"}"#
        )
        .unwrap(),
        HerdrEvent::Unknown
    );
    let missing_data = parse_event_line(r#"{"event":"pane_updated"}"#).unwrap_err();
    assert!(matches!(missing_data, HerdrError::Protocol(message) if message.contains("`data`")));
    let broken =
        parse_event_line(r#"{"data":{"type":"pane_updated"},"event":"pane_updated"}"#).unwrap_err();
    assert!(matches!(broken, HerdrError::Json(_)));
}

#[test]
fn connect_returns_err_when_subscribe_is_rejected() {
    let directory = tempfile::TempDir::new().unwrap();
    let socket_path = directory.path().join("api.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let mut payload = serde_json::to_vec(&json!({
            "id": "",
            "error": {
                "code": "invalid_request",
                "message": "invalid request: missing field `pane_id`"
            }
        }))
        .unwrap();
        payload.push(b'\n');
        stream.write_all(&payload).unwrap();
        stream.flush().unwrap();
    });
    match EventStream::connect(&socket_path) {
        Err(HerdrError::Api { code, message }) => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("pane_id"));
        }
        Err(error) => panic!("expected invalid_request, got {error:?}"),
        Ok(_) => panic!("rejected subscribe must return Err"),
    }
}

#[test]
fn session_subscribe_sends_only_session_wide_types() {
    let directory = tempfile::TempDir::new().unwrap();
    let socket_path = directory.path().join("api.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        request_tx.send(line).unwrap();
        let mut payload = serde_json::to_vec(&json!({
            "id": "",
            "error": {
                "code": "captured",
                "message": "request captured"
            }
        }))
        .unwrap();
        payload.push(b'\n');
        stream.write_all(&payload).unwrap();
        stream.flush().unwrap();
    });
    match EventStream::connect(&socket_path) {
        Err(HerdrError::Api { code, .. }) => assert_eq!(code, "captured"),
        Err(error) => panic!("expected captured request, got {error:?}"),
        Ok(_) => panic!("subscribe was supposed to be rejected after capture"),
    }
    let request: Value = serde_json::from_str(&request_rx.recv().unwrap()).unwrap();
    assert_eq!(request["method"], "events.subscribe");
    let subscriptions = request["params"]["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), EVENT_SUBSCRIPTIONS.len());
    assert_eq!(subscriptions[0], json!({"type": EVENT_SUBSCRIPTIONS[0]}));
    assert!(
        !subscriptions
            .iter()
            .any(|value| value["type"] == "pane.agent_status_changed")
    );
}

#[test]
fn agent_status_subscribe_sends_only_parameterized_pane_entries() {
    let directory = tempfile::TempDir::new().unwrap();
    let socket_path = directory.path().join("api.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        request_tx.send(line).unwrap();
        let mut payload = serde_json::to_vec(&json!({
            "id": "",
            "error": {
                "code": "captured",
                "message": "request captured"
            }
        }))
        .unwrap();
        payload.push(b'\n');
        stream.write_all(&payload).unwrap();
        stream.flush().unwrap();
    });
    match EventStream::connect_agent_status(&socket_path, &["p1".into(), "p2".into()]) {
        Err(HerdrError::Api { code, .. }) => assert_eq!(code, "captured"),
        Err(error) => panic!("expected captured request, got {error:?}"),
        Ok(_) => panic!("subscribe was supposed to be rejected after capture"),
    }
    let request: Value = serde_json::from_str(&request_rx.recv().unwrap()).unwrap();
    assert_eq!(request["method"], "events.subscribe");
    assert_eq!(
        request["params"]["subscriptions"],
        json!([
            {"type": "pane.agent_status_changed", "pane_id": "p1"},
            {"type": "pane.agent_status_changed", "pane_id": "p2"},
        ])
    );
}

#[test]
fn request_socket_times_out_when_the_server_never_replies() {
    let directory = tempfile::TempDir::new().unwrap();
    let socket_path = directory.path().join("api.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let (held_tx, held_rx) = mpsc::channel();
    thread::spawn(move || {
        held_tx.send(listener.accept().unwrap().0).unwrap();
    });
    let error = request_socket_with_timeout(
        &socket_path,
        "session.snapshot",
        json!({}),
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert!(matches!(error, HerdrError::Timeout(timeout) if timeout == Duration::from_millis(100)));
    let _held = held_rx.recv().unwrap();
}

#[test]
fn request_socket_returns_the_result_when_the_server_replies() {
    let directory = tempfile::TempDir::new().unwrap();
    let socket_path = directory.path().join("api.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let mut payload = serde_json::to_vec(&json!({
            "id": request["id"],
            "result": { "ok": true }
        }))
        .unwrap();
        payload.push(b'\n');
        stream.write_all(&payload).unwrap();
        stream.flush().unwrap();
    });
    let result = request_socket_with_timeout(
        &socket_path,
        "session.snapshot",
        json!({}),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(result, json!({ "ok": true }));
}

#[test]
fn endpoint_handshake_against_live_server() {
    // OCHERDR_TEST_CLIENT_SOCKET overrides the default session socket so the
    // handshake can be exercised against a disposable named session.
    let socket = std::env::var_os("OCHERDR_TEST_CLIENT_SOCKET")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| Path::new(&home).join(".config/herdr/herdr-client.sock"))
        });
    let Some(socket) = socket else {
        return;
    };
    if !socket.exists() {
        eprintln!("skipping live endpoint handshake: {socket:?} absent");
        return;
    }
    let endpoint = crate::TerminalEndpoint::new(socket);
    let (session, mut events) = crate::endpoint::EndpointSession::spawn(endpoint, 80, 24, 8, 17);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut welcome = None;
    let mut snapshot = None;
    while std::time::Instant::now() < deadline && (welcome.is_none() || snapshot.is_none()) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match futures::executor::block_on(futures::StreamExt::next(&mut events)) {
            Some(Ok(crate::endpoint::EndpointEvent::Welcome(w))) => welcome = Some(w),
            Some(Ok(crate::endpoint::EndpointEvent::Snapshot(s))) => snapshot = Some(s),
            Some(Ok(_)) => {}
            Some(Err(error)) => panic!("endpoint handshake failed: {error}"),
            None => panic!("endpoint stream closed before snapshot"),
        }
        let _ = remaining;
    }
    let welcome = welcome.expect("endpoint welcome never arrived");
    let snapshot = snapshot.expect("endpoint snapshot never arrived");
    eprintln!(
        "endpoint handshake OK: server {} ({} methods, {} capabilities), \
         boot_id {} ({} workspaces)",
        welcome.server_version,
        welcome.methods.len(),
        welcome.capabilities.len(),
        snapshot.boot_id,
        snapshot.workspaces.len()
    );

    session
        .send(crate::endpoint::EndpointCommand::Focus(true))
        .unwrap();
    session.set_surface_active(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut frames = 0u32;
    let mut last_frame: Option<Box<crate::endpoint_v1::PaneSurfaceFrame>> = None;
    while std::time::Instant::now() < deadline && frames == 0 {
        match futures::executor::block_on(futures::StreamExt::next(&mut events)) {
            Some(Ok(crate::endpoint::EndpointEvent::Surface(frame))) => {
                frames += 1;
                last_frame = Some(frame.clone());
                eprintln!(
                    "surface frame: {}x{} cells, {} panes, {} splits, {} cells",
                    frame.frame.width,
                    frame.frame.height,
                    frame.panes.len(),
                    frame.splits.len(),
                    frame.frame.cells.len()
                );
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => panic!("surface stream failed: {error}"),
            None => panic!("endpoint stream closed before surface"),
        }
    }
    assert!(frames > 0, "no surface frame after surface.set active");

    let frame = last_frame.expect("expected the captured frame");
    let mut decoder = crate::surface_ansi::SurfaceDecoder::new();
    let updates = decoder.frame(*frame.clone(), false);
    assert!(!updates.is_empty(), "decoder produced no pane updates");
    let mut screens: HashMap<String, vt100::Parser> = HashMap::new();
    for update in &updates {
        let mut screen = vt100::Parser::new(
            update.inner_rect.height.max(1),
            update.inner_rect.width.max(1),
            0,
        );
        screen.process(&update.ansi);
        let text = screen.screen().contents();
        eprintln!(
            "pane {} ({}x{}): {:?}",
            update.pane_id,
            update.inner_rect.width,
            update.inner_rect.height,
            text.chars().take(120).collect::<String>()
        );
        screens.insert(update.pane_id.clone(), screen);
    }

    // Deep phase, only against a disposable session socket: semantic input
    // round-trip, endpoint-tunneled pane.split, multi-pane frames, patches.
    if std::env::var_os("OCHERDR_TEST_CLIENT_SOCKET").is_none() {
        return;
    }
    let marker = "EP-OK-7f3a";
    let first_pane = updates[0].pane_id.clone();
    let handle = session.handle();
    handle
        .pane_input(
            &first_pane,
            vec![
                crate::endpoint_v1::ClientPaneInputEvent::TextCommit(format!("echo {marker}")),
                crate::endpoint_v1::ClientPaneInputEvent::Key {
                    code: crate::endpoint_v1::ClientKeyCode::Enter,
                    modifiers: 0,
                    kind: crate::endpoint_v1::ClientKeyKind::Press,
                    repeat_count: 1,
                    shifted_codepoint: None,
                    generated_text: None,
                    tracks_release: true,
                    physical_key_id: None,
                    windows_record: None,
                },
                crate::endpoint_v1::ClientPaneInputEvent::Key {
                    code: crate::endpoint_v1::ClientKeyCode::Enter,
                    modifiers: 0,
                    kind: crate::endpoint_v1::ClientKeyKind::Release,
                    repeat_count: 1,
                    shifted_codepoint: None,
                    generated_text: None,
                    tracks_release: false,
                    physical_key_id: None,
                    windows_record: None,
                },
            ],
        )
        .unwrap();
    // Endpoint-scoped API tunnel: split the pane and expect a 2-pane frame.
    let split_request = session
        .request(
            "pane.split",
            json!({
                "target_pane_id": first_pane,
                "direction": "right",
                "focus": false,
            }),
        )
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut saw_marker = false;
    let mut saw_response = false;
    let mut saw_patch = false;
    let mut saw_two_panes = false;
    while std::time::Instant::now() < deadline && !(saw_marker && saw_response && saw_two_panes) {
        match futures::executor::block_on(futures::StreamExt::next(&mut events)) {
            Some(Ok(crate::endpoint::EndpointEvent::Surface(frame))) => {
                if frame.panes.len() >= 2 {
                    saw_two_panes = true;
                }
                for update in decoder.frame(*frame, false) {
                    let screen = screens.entry(update.pane_id.clone()).or_insert_with(|| {
                        vt100::Parser::new(
                            update.inner_rect.height.max(1),
                            update.inner_rect.width.max(1),
                            0,
                        )
                    });
                    screen.process(&update.ansi);
                    if screen.screen().contents().contains(marker) {
                        saw_marker = true;
                    }
                }
            }
            Some(Ok(crate::endpoint::EndpointEvent::SurfacePatch(patch))) => {
                saw_patch = true;
                if let Some(updates) = decoder.patch(&patch) {
                    for update in updates {
                        if let Some(screen) = screens.get_mut(&update.pane_id) {
                            screen.process(&update.ansi);
                            if screen.screen().contents().contains(marker) {
                                saw_marker = true;
                            }
                        }
                    }
                }
            }
            Some(Ok(crate::endpoint::EndpointEvent::Response { request_id, data }))
                if request_id == split_request =>
            {
                saw_response = true;
                eprintln!(
                    "pane.split response: {}",
                    String::from_utf8_lossy(&data)
                        .chars()
                        .take(200)
                        .collect::<String>()
                );
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => panic!("endpoint stream failed mid-test: {error}"),
            None => panic!("endpoint stream closed mid-test"),
        }
    }
    assert!(saw_response, "pane.split response never arrived");
    assert!(saw_two_panes, "composited surface never grew to 2 panes");
    assert!(
        saw_marker,
        "semantic input never echoed back through the surface"
    );
    eprintln!("deep phase: split ok, 2 panes, input echoed, patch seen = {saw_patch}");
}
