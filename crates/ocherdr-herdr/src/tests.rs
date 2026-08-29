use super::*;

#[test]
fn parses_terminal_mouse_capture_envelope() {
    let envelope: TerminalEnvelope = serde_json::from_str(
        r#"{"type":"terminal.mouse_capture","enabled":true,"sgr_pixels":false}"#,
    )
    .unwrap();

    assert_eq!(
        envelope,
        TerminalEnvelope::MouseCapture {
            enabled: true,
            sgr_pixels: false,
        }
    );
}

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
fn remote_clipboard_image_upload_streams_bytes_with_profile_ssh_options() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::TempDir::new().unwrap();
    let ssh = dir.path().join("ssh");
    let arguments = dir.path().join("arguments");
    let payload = dir.path().join("payload");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\ncat > {}\nprintf '%s' '/tmp/ocherdr-clipboard-images-501/clipboard-123-456-1/image.png'\n",
        posix_quote(&arguments.to_string_lossy()),
        posix_quote(&payload.to_string_lossy()),
    );
    fs::write(&ssh, script).unwrap();
    let mut permissions = fs::metadata(&ssh).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&ssh, permissions).unwrap();

    let identity = dir.path().join("key with space");
    let profile = ConnectionProfile::Ssh {
        id: "server".into(),
        label: "Server".into(),
        destination: "deploy@example.com".into(),
        port: Some(2202),
        identity_file: Some(identity.clone()),
        herdr_path: "herdr".into(),
    };
    let bytes = b"\x89PNG\r\n\x1a\nremote".to_vec();
    let path = upload_remote_clipboard_image_with_ssh(&profile, "PNG", bytes.clone(), &ssh)
        .expect("fake SSH upload");

    assert_eq!(
        path,
        "/tmp/ocherdr-clipboard-images-501/clipboard-123-456-1/image.png"
    );
    assert_eq!(fs::read(payload).unwrap(), bytes);
    let arguments = fs::read_to_string(arguments).unwrap();
    assert!(arguments.lines().any(|line| line == "-p"));
    assert!(arguments.lines().any(|line| line == "2202"));
    assert!(
        arguments
            .lines()
            .any(|line| line == identity.to_string_lossy())
    );
    assert!(arguments.lines().any(|line| line == "deploy@example.com"));
    assert!(arguments.contains("/tmp/ocherdr-clipboard-images-$uid"));
    assert!(arguments.contains("cat > \"$image_path\""));
    assert!(
        !arguments.lines().any(|line| line.starts_with("path=")),
        "`path` is a special zsh parameter tied to PATH"
    );
}

#[test]
fn remote_clipboard_image_script_writes_a_private_remote_file() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::TempDir::new().unwrap();
    let ssh = dir.path().join("ssh");
    fs::write(
        &ssh,
        "#!/bin/sh\nfor argument\ndo\n  remote=$argument\ndone\nshell=/bin/sh\nif [ -x /bin/zsh ]\nthen\n  shell=/bin/zsh\nfi\nexec \"$shell\" -c \"$remote\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&ssh).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&ssh, permissions).unwrap();

    let profile = ConnectionProfile::Ssh {
        id: "server".into(),
        label: "Server".into(),
        destination: "deploy@example.com".into(),
        port: None,
        identity_file: None,
        herdr_path: "herdr".into(),
    };
    let bytes = b"\x89PNG\r\n\x1a\nremote".to_vec();
    let path = upload_remote_clipboard_image_with_ssh(&profile, "png", bytes.clone(), &ssh)
        .expect("execute remote staging script");
    let path = PathBuf::from(path);

    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let upload_dir = path.parent().unwrap().to_owned();
    let staging_dir = upload_dir.parent().unwrap().to_owned();
    assert_eq!(
        fs::metadata(&staging_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );

    fs::remove_file(path).unwrap();
    fs::remove_dir(upload_dir).unwrap();
    let _ = fs::remove_dir(staging_dir);
}

#[test]
fn remote_clipboard_image_validation_rejects_unsafe_content() {
    assert!(validate_remote_clipboard_image("svg", b"<svg/>").is_err());
    assert!(validate_remote_clipboard_image("png", b"not png").is_err());
    assert!(validate_remote_clipboard_image("png", b"").is_err());
    assert_eq!(
        validate_remote_clipboard_image("PNG", b"\x89PNG\r\n\x1a\nrest").unwrap(),
        "png"
    );
}

#[test]
fn remote_clipboard_image_path_allows_only_the_generated_shape() {
    assert!(remote_clipboard_image_path_is_safe(
        "/tmp/ocherdr-clipboard-images-501/clipboard-123-456-1/image.png",
        "png"
    ));
    assert!(!remote_clipboard_image_path_is_safe(
        "/tmp/ocherdr-clipboard-images-501/clipboard-../../bin/run/image.png",
        "png"
    ));
    assert!(!remote_clipboard_image_path_is_safe(
        "/tmp/ocherdr-clipboard-images-user/clipboard-123/image.png",
        "png"
    ));
    assert!(!remote_clipboard_image_path_is_safe(
        "/tmp/ocherdr-clipboard-images-501/clipboard-123/image.png;touch-pwned",
        "png"
    ));
}

#[test]
fn terminal_input_serializes_lossless_bytes() {
    let encoded = base64::engine::general_purpose::STANDARD.encode([0, 0x1b, 0x80, 0xff]);
    let value = serde_json::to_value(TerminalControlCommand::Input { bytes: &encoded }).unwrap();

    assert_eq!(value["type"], "terminal.input");
    assert_eq!(value["bytes"], "ABuA/w==");
    assert!(value.get("text").is_none());
}

#[test]
fn terminal_scroll_serializes_as_a_semantic_wheel_command() {
    let value = serde_json::to_value(TerminalControlCommand::Scroll {
        direction: "up",
        lines: 3,
        source: "wheel",
        column: None,
        row: None,
        modifiers: 0,
    })
    .unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "type": "terminal.scroll",
            "direction": "up",
            "lines": 3,
            "source": "wheel",
            "column": null,
            "row": null,
            "modifiers": 0,
        })
    );
}

#[test]
fn terminal_control_requires_an_explicit_takeover_mode() {
    let control = terminal_session_args("work", "pane-1", TerminalMode::Control, 120, 40);
    assert_eq!(
        control,
        [
            "--session",
            "work",
            "terminal",
            "session",
            "control",
            "pane-1",
            "--cols",
            "120",
            "--rows",
            "40",
        ]
    );

    let takeover = terminal_session_args("work", "pane-1", TerminalMode::ControlTakeover, 120, 40);
    assert!(takeover.contains(&"--takeover".to_owned()));
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
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
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
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
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
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
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
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
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
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
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
