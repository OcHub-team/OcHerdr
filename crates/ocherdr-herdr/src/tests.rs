use super::*;
use std::collections::HashMap;
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
        proxy_jump: None,
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
        proxy_jump: None,
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
    assert_eq!(MAX_CLIPBOARD_IMAGE_BYTES, 16 * 1024 * 1024);
}

#[test]
fn bridge_args_omit_session_flag_for_the_default_session() {
    assert_eq!(
        bridge_args("default", "remote-client-bridge"),
        ["remote-client-bridge"]
    );
    assert_eq!(
        bridge_args("default", "remote-api-bridge"),
        ["remote-api-bridge"]
    );
}

#[test]
fn bridge_args_place_session_flag_before_the_subcommand() {
    assert_eq!(
        bridge_args("agents", "remote-client-bridge"),
        ["--session", "agents", "remote-client-bridge"]
    );
    assert_eq!(
        bridge_args("work one", "remote-api-bridge"),
        ["--session", "work one", "remote-api-bridge"]
    );
}

#[test]
fn an_empty_open_channel_does_not_wake_the_ui() {
    use futures::FutureExt as _;
    let (_tx, mut rx) = futures::channel::mpsc::unbounded::<u8>();
    assert!(
        next_batch(&mut rx).now_or_never().is_none(),
        "空通道不应该唤醒 UI"
    );
}

#[test]
fn a_ready_channel_drains_already_queued_items() {
    use futures::FutureExt as _;
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
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
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<u8>();
    drop(tx);
    assert_eq!(next_batch(&mut rx).now_or_never(), Some(None));
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
#[ignore = "requires a live local herdr >= 0.9 server"]
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
    let (session, mut events) = crate::endpoint::EndpointSession::spawn(
        crate::endpoint::EndpointTransport::Local(endpoint),
        80,
        24,
        8,
        17,
    );
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

#[test]
fn ssh_g_output_builds_user_host_destination() {
    let resolved = parse_ssh_g_output(
        "user deploy\nhostname 10.0.1.5\nport 2222\nidentityfile ~/.ssh/prod_key\nproxyjump none\nproxycommand none\n",
    );
    assert_eq!(resolved.destination, "deploy@10.0.1.5");
    assert_eq!(resolved.port, Some(2222));
    assert_eq!(
        resolved.identity_file.as_deref(),
        Some(Path::new("~/.ssh/prod_key"))
    );
    assert_eq!(resolved.proxy_jump, None);
    assert!(!resolved.has_proxy_command);
}

#[test]
fn ssh_g_output_omits_user_when_unset() {
    let resolved = parse_ssh_g_output("hostname example.net\nport 22\n");
    assert_eq!(resolved.destination, "example.net");
}

#[test]
fn ssh_g_output_skips_implicit_default_identity_files() {
    let resolved = parse_ssh_g_output(
        "hostname h\nidentityfile /Users/me/.ssh/id_rsa\nidentityfile /Users/me/.ssh/id_ed25519\n",
    );
    assert_eq!(resolved.identity_file, None);
    // A configured key after the defaults is the one that sticks.
    let resolved = parse_ssh_g_output(
        "hostname h\nidentityfile /Users/me/.ssh/id_rsa\nidentityfile ~/.ssh/deploy_key\n",
    );
    assert_eq!(
        resolved.identity_file.as_deref(),
        Some(Path::new("~/.ssh/deploy_key"))
    );
}

#[test]
fn ssh_g_output_captures_proxy_jump_and_flags_proxy_command() {
    let resolved = parse_ssh_g_output("hostname h\nproxyjump bastion,2222\n");
    assert_eq!(resolved.proxy_jump.as_deref(), Some("bastion,2222"));
    let resolved = parse_ssh_g_output("hostname h\nproxycommand nc -X 5 -x proxy %h %p\n");
    assert!(resolved.has_proxy_command);
}

#[test]
fn ssh_commands_forward_proxy_jump() {
    let profile = ConnectionProfile::Ssh {
        id: "server".into(),
        label: "Server".into(),
        destination: "deploy@example.com".into(),
        port: Some(2202),
        identity_file: Some("/Keys/work".into()),
        proxy_jump: Some("bastion".into()),
        herdr_path: "/opt/herdr".into(),
    };
    assert_eq!(
        ssh_login_command(&profile).as_deref(),
        Some("ssh -p 2202 -i /Keys/work -J bastion deploy@example.com")
    );
    assert_eq!(
        attach_command(&profile, "work"),
        "ssh -t -p 2202 -i /Keys/work -J bastion deploy@example.com 'exec /opt/herdr session attach work'"
    );
}

#[cfg(unix)]
fn spawn_fake_endpoint_server(
    respond: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static,
) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::TempDir::new().unwrap();
    let socket_path = directory.path().join("client.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        respond(stream);
    });
    (directory, socket_path)
}

#[cfg(unix)]
fn next_endpoint_event(
    events: &mut crate::endpoint::EndpointEventReceiver,
) -> Option<Result<crate::endpoint::EndpointEvent>> {
    futures::executor::block_on(futures::StreamExt::next(events))
}

#[cfg(unix)]
#[test]
fn endpoint_handshake_with_fake_server_negotiates_and_captures_the_snapshot() {
    use crate::endpoint::EndpointEvent;
    use crate::endpoint_v1 as ep;

    let welcome = ep::EndpointServerWelcome {
        generation: ep::ENDPOINT_PROTOCOL_GENERATION,
        server_version: "herdr 0.9.0".into(),
        snapshot_codec: ep::ENDPOINT_SNAPSHOT_KIND.into(),
        surface_codec: ep::SURFACE_CODEC_V1.into(),
        input_codec: ep::INPUT_CODEC_V1.into(),
        blob_codec: ep::BLOB_CODEC_V1.into(),
        methods: vec!["client_shell.surface.set".into(), "pane.split".into()],
        capabilities: vec![ep::HEALTH_CHECK_CAPABILITY.into()],
        error: None,
    };
    let (_directory, socket_path) = spawn_fake_endpoint_server(move |mut stream| {
        let hello: ep::ClientMessage = ep::read_message(&mut stream, ep::MAX_FRAME_SIZE).unwrap();
        let ep::ClientMessage::EndpointControl { kind, data } = hello else {
            panic!("expected endpoint hello, got {hello:?}");
        };
        assert_eq!(kind, ep::ENDPOINT_HELLO_KIND);
        let hello: ep::EndpointClientHello = serde_json::from_str(&data).unwrap();
        assert_eq!(hello.generation, ep::ENDPOINT_PROTOCOL_GENERATION);

        ep::write_message(
            &mut stream,
            &ep::ServerMessage::EndpointControl {
                kind: ep::ENDPOINT_WELCOME_KIND.into(),
                data: serde_json::to_string(&welcome).unwrap(),
            },
        )
        .unwrap();
        ep::write_message(
            &mut stream,
            &ep::ServerMessage::EndpointControl {
                kind: ep::ENDPOINT_SNAPSHOT_KIND.into(),
                data: serde_json::to_string(&json!({
                    "boot_id": "boot-1",
                    "revision": 7,
                    "config_diagnostic": null,
                    "product_announcement": null,
                    "update_available": null,
                    "update_install_command": "",
                    "server_keybindings_toml": null,
                    "latest_release_notes_available": false,
                    "integration_updates_available": false,
                    "worktree_directory": "",
                    "release_notes": null,
                    "focused_workspace_id": null,
                    "focused_tab_id": null,
                    "focused_pane_id": null,
                    "tab_bar_right": [],
                    "tab_bar_right_separator": "",
                    "agent_view_label": null,
                    "agent_order": [],
                    "workspaces": [],
                    "tabs": [],
                    "panes": [],
                    "agents": [],
                    "commands": []
                }))
                .unwrap(),
            },
        )
        .unwrap();
        // Keep the connection open until the client detaches.
        let mut buffer = [0_u8; 64];
        let _ = stream.read(&mut buffer);
    });

    let transport = crate::endpoint::EndpointTransport::local(socket_path);
    let (_session, mut events) = crate::endpoint::EndpointSession::spawn(transport, 80, 24, 8, 17);
    let welcome_deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut negotiated = None;
    let mut snapshot = None;
    while std::time::Instant::now() < welcome_deadline
        && (negotiated.is_none() || snapshot.is_none())
    {
        match next_endpoint_event(&mut events) {
            Some(Ok(EndpointEvent::Welcome(welcome))) => negotiated = Some(welcome),
            Some(Ok(EndpointEvent::Snapshot(shell))) => snapshot = Some(shell),
            Some(Ok(_)) => {}
            Some(Err(error)) => panic!("handshake failed: {error}"),
            None => panic!("endpoint stream closed during handshake"),
        }
    }
    let negotiated = negotiated.expect("welcome never arrived");
    assert_eq!(negotiated.server_version, "herdr 0.9.0");
    assert_eq!(
        negotiated.methods,
        ["client_shell.surface.set", "pane.split"]
    );
    let snapshot = snapshot.expect("snapshot never arrived");
    assert_eq!(snapshot.boot_id, "boot-1");
    assert_eq!(snapshot.revision, 7);
}

#[cfg(unix)]
#[test]
fn endpoint_negotiation_failure_names_the_minimum_herdr_version() {
    // A pre-0.9 server does not understand the endpoint hello and drops the
    // connection; the error must tell the user to upgrade instead of falling
    // back to a legacy codec.
    let (_directory, socket_path) = spawn_fake_endpoint_server(|mut stream| {
        let hello: crate::endpoint_v1::ClientMessage =
            crate::endpoint_v1::read_message(&mut stream, crate::endpoint_v1::MAX_FRAME_SIZE)
                .unwrap();
        drop(hello);
        drop(stream);
    });

    let transport = crate::endpoint::EndpointTransport::local(socket_path);
    let (_session, mut events) = crate::endpoint::EndpointSession::spawn(transport, 80, 24, 8, 17);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(std::time::Instant::now() < deadline, "no error arrived");
        match next_endpoint_event(&mut events) {
            Some(Ok(_)) => {}
            Some(Err(error)) => {
                let message = error.to_string();
                assert!(
                    message.contains(MINIMUM_HERDR_VERSION),
                    "error must name {MINIMUM_HERDR_VERSION}: {message}"
                );
                assert!(
                    message.contains("upgrade Herdr"),
                    "error must direct the upgrade: {message}"
                );
                break;
            }
            None => panic!("stream closed without surfacing the negotiation error"),
        }
    }
}

/// Live probe against a running local Herdr >= 0.9 server: prints the
/// endpoint welcome (advertised methods/capabilities) and exercises a few
/// read-only endpoint-scoped API calls to map the endpoint API surface.
/// Run explicitly:
/// `cargo test -p ocherdr-herdr endpoint_probe_live -- --ignored --nocapture`
#[test]
#[ignore = "requires a live local herdr >= 0.9 server"]
fn endpoint_probe_live_endpoint_api_surface() {
    use endpoint::{EndpointCommand, EndpointEvent, EndpointSession};
    use futures::StreamExt as _;

    let home = std::env::var_os("HOME").expect("HOME is set");
    let socket_path = PathBuf::from(home).join(".config/herdr/herdr.sock");
    if !socket_path.exists() {
        eprintln!("no local herdr socket at {socket_path:?}; skipping");
        return;
    }
    let profile = ConnectionProfile::Local {
        herdr_path: "herdr".into(),
    };
    let session = SessionSummary {
        name: DEFAULT_SESSION_NAME.into(),
        default: true,
        running: true,
        socket_path,
        session_dir: PathBuf::new(),
    };
    let connection = SessionConnection::connect(&profile, &session).expect("connect");
    let (endpoint_session, events) =
        EndpointSession::spawn(connection.endpoint_transport(), 80, 24, 8, 16);

    // Bridge the async receiver onto a std channel so the probe can time out.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut events = events;
        while let Some(item) = futures::executor::block_on(events.next()) {
            if tx.send(item).is_err() {
                break;
            }
        }
    });

    let probe_methods = [
        "session.snapshot",
        "workspace.list",
        "agent.list",
        "worktree.list",
    ];
    let mut welcome_printed = false;
    let mut requested = false;
    let mut answered = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline && answered < probe_methods.len() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok(item) = rx.recv_timeout(remaining) else {
            eprintln!("probe: timed out waiting for endpoint events");
            break;
        };
        match item {
            Ok(EndpointEvent::Welcome(welcome)) => {
                welcome_printed = true;
                eprintln!("probe: server_version = {}", welcome.server_version);
                eprintln!("probe: capabilities = {:?}", welcome.capabilities);
                eprintln!("probe: methods ({}):", welcome.methods.len());
                for method in &welcome.methods {
                    eprintln!("  - {method}");
                }
            }
            Ok(EndpointEvent::Snapshot(_)) if !requested && welcome_printed => {
                // boot_id is captured by now; the lane accepts requests.
                requested = true;
                for method in probe_methods {
                    let id = endpoint_session
                        .request(method, json!({}))
                        .expect("queue probe request");
                    eprintln!("probe: queued {method} as {id}");
                }
            }
            Ok(EndpointEvent::Response { request_id, data }) => {
                answered += 1;
                let text = String::from_utf8_lossy(&data);
                eprintln!(
                    "probe: response {request_id}: {}",
                    &text[..text.len().min(400)]
                );
            }
            Ok(EndpointEvent::ShellError(message)) => {
                eprintln!("probe: shell error: {message}");
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("probe: stream error: {error}");
                break;
            }
        }
    }
    let _ = endpoint_session.send(EndpointCommand::Detach);
    eprintln!("probe: done (answered {answered}/{})", probe_methods.len());
}

/// Probe `command.invoke` as a potential generic dispatcher for methods the
/// endpoint lane does not expose directly (session.snapshot, agent.*).
/// Run explicitly:
/// `cargo test -p ocherdr-herdr endpoint_probe_command_invoke -- --ignored --nocapture`
#[test]
#[ignore = "requires a live local herdr >= 0.9 server"]
fn endpoint_probe_command_invoke() {
    use endpoint::{EndpointCommand, EndpointEvent, EndpointSession};
    use futures::StreamExt as _;

    let home = std::env::var_os("HOME").expect("HOME is set");
    let socket_path = PathBuf::from(home).join(".config/herdr/herdr.sock");
    if !socket_path.exists() {
        eprintln!("no local herdr socket; skipping");
        return;
    }
    let profile = ConnectionProfile::Local {
        herdr_path: "herdr".into(),
    };
    let session = SessionSummary {
        name: DEFAULT_SESSION_NAME.into(),
        default: true,
        running: true,
        socket_path,
        session_dir: PathBuf::new(),
    };
    let connection = SessionConnection::connect(&profile, &session).expect("connect");
    let (endpoint_session, events) =
        EndpointSession::spawn(connection.endpoint_transport(), 80, 24, 8, 16);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut events = events;
        while let Some(item) = futures::executor::block_on(events.next()) {
            if tx.send(item).is_err() {
                break;
            }
        }
    });

    // (label, params) candidates for command.invoke's expected shape.
    let attempts: [(&str, Value); 3] = [
        (
            "command-as-string",
            json!({"command": "session.snapshot", "params": {}}),
        ),
        (
            "method-as-string",
            json!({"method": "session.snapshot", "params": {}}),
        ),
        ("bare-name", json!({"name": "session.snapshot"})),
    ];
    let mut requested = false;
    let mut answered = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline && answered < attempts.len() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Ok(item) = rx.recv_timeout(remaining) else {
            eprintln!("probe2: timed out");
            break;
        };
        match item {
            Ok(EndpointEvent::Snapshot(_)) if !requested => {
                requested = true;
                for (label, params) in &attempts {
                    let id = endpoint_session
                        .request("command.invoke", params.clone())
                        .expect("queue");
                    eprintln!("probe2: queued {label} as {id}");
                }
            }
            Ok(EndpointEvent::Response { request_id, data }) => {
                answered += 1;
                let text = String::from_utf8_lossy(&data);
                eprintln!(
                    "probe2: response {request_id}: {}",
                    &text[..text.len().min(500)]
                );
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("probe2: stream error: {error}");
                break;
            }
        }
    }
    let _ = endpoint_session.send(EndpointCommand::Detach);
    eprintln!("probe2: done");
}
