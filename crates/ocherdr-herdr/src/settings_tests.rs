use super::*;
use crate::{
    private_protocol::{TerminalConnect, connect},
    private_v20 as wire,
};
use std::os::unix::net::UnixListener;

#[test]
fn settings_client_uses_local_binding_and_detaches_without_taking_a_pane() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let hello: wire::ClientMessage =
            wire::read_message(&mut stream, wire::MAX_FRAME_SIZE).unwrap();
        match hello {
            wire::ClientMessage::Hello {
                launch_mode: wire::ClientLaunchMode::App,
                keybindings: wire::ClientKeybindings::Local { keys_toml },
                ..
            } => {
                assert!(keys_toml.contains("settings = \"f12\""));
            }
            other => panic!("expected full app with local bindings, got {other:?}"),
        }
        wire::write_message(
            &mut stream,
            &wire::ServerMessage::Welcome {
                version: 20,
                encoding: wire::RenderEncoding::TerminalAnsi,
                error: None,
            },
        )
        .unwrap();
        let open: wire::ClientMessage =
            wire::read_message(&mut stream, wire::MAX_FRAME_SIZE).unwrap();
        assert!(matches!(open, wire::ClientMessage::InputEvents { events }
            if matches!(events.as_slice(), [wire::ClientInputEvent::Key { code: wire::ClientKeyCode::F(12), .. }])));
        // No ControlTerminal/AttachTerminal, nor any shell input, may appear.
        let close: wire::ClientMessage =
            wire::read_message(&mut stream, wire::MAX_FRAME_SIZE).unwrap();
        assert_eq!(close, wire::ClientMessage::Detach);
    });
    let (session, _events) =
        TerminalSession::spawn_settings(TerminalEndpoint::new(path), 20, 120, 40);
    drop(session);
    server.join().unwrap();
}

/// Optional compatibility test against a real binary. Everything (config,
/// state, sockets, cwd and pane processes) belongs to a temporary test server.
/// Run with HERDR_TEST_BIN=/absolute/path/herdr cargo test -p ocherdr-herdr
/// live_settings -- --ignored --nocapture
#[test]
#[ignore = "requires HERDR_TEST_BIN; starts an isolated temporary Herdr server"]
fn live_settings_preserve_pane_control_and_custom_keybindings() {
    let binary =
        std::env::var("HERDR_TEST_BIN").expect("set HERDR_TEST_BIN to an absolute executable path");
    let dir = tempfile::Builder::new()
        .prefix("oc-settings-")
        .tempdir_in("/tmp")
        .unwrap();
    let config = dir.path().join("config.toml");
    let original = "onboarding = false\n[terminal]\ndefault_shell = \"/bin/sh\"\nshell_mode = \"non_login\"\n[update]\nversion_check = false\nmanifest_check = false\n[keys]\nprefix = \"ctrl+a\"\nsettings = \"prefix+x\"\n";
    fs::write(&config, original).unwrap();
    let api = dir
        .path()
        .join("herdr/sessions/ocherdr-settings-test/herdr.sock");
    let log = fs::File::create(dir.path().join("server.log")).unwrap();
    let child = Command::new(binary)
        .args(["--session", "ocherdr-settings-test", "server"])
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_STATE_HOME", dir.path())
        .env("HERDR_CONFIG_PATH", &config)
        .env("HERDR_DISABLE_SOUND", "1")
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_SESSION")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .env_remove("HERDR_PANE_ID")
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    struct TestServer(Child);
    impl Drop for TestServer {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut server = TestServer(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !api.exists() {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "test server exited: {}",
            fs::read_to_string(dir.path().join("server.log")).unwrap()
        );
        assert!(
            Instant::now() < deadline,
            "test server did not create {}",
            api.display()
        );
        thread::sleep(Duration::from_millis(30));
    }
    let created = request_socket(
        &api,
        "workspace.create",
        json!({ "cwd": dir.path(), "label": "settings-test", "focus": true }),
    )
    .unwrap();
    let pane = created["root_pane"]["pane_id"]
        .as_str()
        .expect("workspace root pane");
    let endpoint = TerminalEndpoint::new(api.with_file_name("herdr-client.sock"));
    let (mut pane_reader, mut pane_writer) = connect(TerminalConnect {
        endpoint: &endpoint,
        protocol: 20,
        target: pane,
        mode: TerminalMode::Control,
        settings: false,
        cols: 100,
        rows: 30,
        cell_width_px: 8,
        cell_height_px: 16,
    })
    .unwrap();
    // Continuously drain the control stream so backpressure cannot affect the test.
    let pane_frames = thread::spawn(move || while let Ok(Some(_)) = pane_reader.read_event() {});
    pane_writer
        .send(TerminalCommand::Resize {
            cols: 100,
            rows: 30,
            cell_width_px: 8,
            cell_height_px: 16,
        })
        .unwrap();
    let before = request_socket(&api, "session.snapshot", json!({})).unwrap();
    let (mut settings_reader, mut settings_writer) = connect(TerminalConnect {
        endpoint: &endpoint,
        protocol: 20,
        target: "",
        mode: TerminalMode::Observe,
        settings: true,
        cols: 160,
        rows: 50,
        cell_width_px: 8,
        cell_height_px: 16,
    })
    .unwrap();
    let (screens_tx, screens_rx) = mpsc::channel();
    let settings_frames = thread::spawn(move || {
        let mut parser = vt100::Parser::new(50, 160, 0);
        while let Ok(Some(event)) = settings_reader.read_event() {
            if let TerminalEvent::Frame(frame) = event {
                parser.process(&frame.bytes);
                if screens_tx.send(parser.screen().contents()).is_err() {
                    break;
                }
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut screen = String::new();
    while Instant::now() < deadline {
        if let Ok(next) = screens_rx.recv_timeout(Duration::from_millis(250)) {
            screen = next;
        }
        if screen.contains("settings") && screen.contains("theme") {
            break;
        }
    }
    assert!(
        screen.contains("settings") && screen.contains("theme"),
        "did not enter settings: {screen}"
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        original,
        "opening settings must not overwrite custom keys"
    );
    settings_writer
        .send(TerminalCommand::Input(b"\x1b[B\r".to_vec()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while fs::read_to_string(&config).unwrap() == original && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let saved = fs::read_to_string(&config).unwrap();
    assert!(
        saved.contains("[theme]"),
        "TUI Apply must persist a theme: {saved}"
    );
    assert!(
        saved.contains("prefix = \"ctrl+a\"") && saved.contains("settings = \"prefix+x\""),
        "client bindings must not overwrite server bindings"
    );
    settings_writer.send(TerminalCommand::Release).unwrap();
    settings_frames.join().unwrap();
    // The original control socket must remain writable after the full app leaves.
    pane_writer
        .send(TerminalCommand::Input(
            b"stty size > pane-size; printf 'settings-control-ok\\n' > control-ok\r".to_vec(),
        ))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !dir.path().join("control-ok").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("control-ok")).unwrap(),
        "settings-control-ok\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("pane-size"))
            .unwrap()
            .trim(),
        "30 100",
        "the settings viewport must not resize a controlled pane"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut snapshot = request_socket(&api, "session.snapshot", json!({})).unwrap();
    while snapshot["snapshot"]["layouts"] != before["snapshot"]["layouts"]
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(20));
        snapshot = request_socket(&api, "session.snapshot", json!({})).unwrap();
    }
    assert_eq!(
        snapshot["snapshot"]["layouts"], before["snapshot"]["layouts"],
        "shared layout must recover after the full client leaves"
    );
    assert_eq!(
        snapshot["snapshot"]["focused_pane_id"],
        before["snapshot"]["focused_pane_id"]
    );
    eprintln!(
        "settings opened and saved; custom keys, pane control, 100×30 PTY size, layout and focus preserved"
    );
    pane_writer.send(TerminalCommand::Release).unwrap();
    pane_frames.join().unwrap();
    let _ = request_socket(&api, "server.stop", json!({}));
    server.0.wait().unwrap();
}
