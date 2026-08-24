//! GPUI `TestAppContext` harness for controller wiring that unit tests cannot reach.
//!
//! These tests drive production `OcHerdrView` / `HostCenter` methods through the
//! same `Context<T>` / `Window` / `Entity<T>` path the GUI uses. They do not
//! introduce a second status type or a test-only controller.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualTestContext, prelude::*};
use ocherdr_core::{ConnectionProfile, HierarchySnapshot, SessionSummary};
use ocherdr_herdr::HostHealthStatus;
use serde_json::{Value, json};

use crate::host_center::HostCenter;
use crate::{
    AppearanceSettings, CachedHostHealth, EventStreamState, HostHealthView, I18n, Language,
    OcHerdrView, Settings, install_appearance,
};

fn install_app(cx: &mut TestAppContext) {
    cx.update(|cx| {
        ochub_ui::install(cx);
        I18n::install(Language::English);
        install_appearance(&AppearanceSettings::default(), cx.window_appearance());
    });
}

/// Construct the production view, then drop the constructor's `reload` so a
/// real Herdr on PATH cannot apply over the test's setup.
fn open_view(cx: &mut TestAppContext) -> (Entity<OcHerdrView>, &mut VisualTestContext) {
    install_app(cx);
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = OcHerdrView::new(Settings::default(), window, cx);
        view.load_epoch = view.load_epoch.wrapping_add(1);
        view.operation = None;
        view
    });
    (view, cx)
}

struct FakeHerdr {
    herdr_path: PathBuf,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl FakeHerdr {
    fn snapshot_ok_subscribe_rejected() -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake herdr socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking fake herdr socket");

        let sessions = json!({
            "sessions": [
                session_json("alpha", &socket_path, dir.path()),
                session_json("work", &socket_path, dir.path()),
            ]
        });
        let herdr_path = dir.path().join("herdr");
        std::fs::write(
            &herdr_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"session\" ] && [ \"$2\" = \"list\" ]; then\ncat <<'EOF'\n{sessions}\nEOF\nexit 0\nfi\necho \"unexpected: $*\" >&2\nexit 1\n"
            ),
        )
        .expect("write fake herdr");
        let mut permissions = std::fs::metadata(&herdr_path)
            .expect("fake herdr metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&herdr_path, permissions).expect("chmod fake herdr");

        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let server =
            thread::spawn(move || serve_snapshot_ok_subscribe_rejected(listener, server_stop));
        Self {
            herdr_path,
            stop,
            server: Some(server),
            _dir: dir,
        }
    }
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn session_json(name: &str, socket_path: &std::path::Path, session_dir: &std::path::Path) -> Value {
    json!({
        "name": name,
        "running": true,
        "socket_path": socket_path,
        "session_dir": session_dir.join(name),
        "default": false,
    })
}

fn serve_snapshot_ok_subscribe_rejected(listener: UnixListener, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => reply_to_herdr_socket(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

fn reply_to_herdr_socket(stream: UnixStream) {
    let _ = stream.set_nonblocking(false);
    let mut reader = BufReader::new(stream.try_clone().expect("clone fake herdr stream"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let request: Value = serde_json::from_str(&line).unwrap_or_else(|_| json!({}));
    let id = request.get("id").cloned().unwrap_or(json!(""));
    let response = match request.get("method").and_then(Value::as_str) {
        Some("session.snapshot") => json!({
            "id": id,
            "result": { "snapshot": HierarchySnapshot::default() },
        }),
        Some("events.subscribe") => json!({
            "id": id,
            "error": {
                "code": "unknown_type",
                "message": "events.subscribe rejected",
            },
        }),
        other => json!({
            "id": id,
            "error": {
                "code": "unexpected",
                "message": format!("unexpected method {other:?}"),
            },
        }),
    };
    let mut payload = serde_json::to_vec(&response).expect("encode fake herdr reply");
    payload.push(b'\n');
    let mut stream = stream;
    let _ = stream.write_all(&payload);
    let _ = stream.flush();
}

fn point_local_profile_at_fake(view: &mut OcHerdrView, fake: &FakeHerdr) {
    view.profiles[0] = ConnectionProfile::Local {
        herdr_path: fake.herdr_path.to_string_lossy().into_owned(),
    };
}

fn saved_host_settings() -> Settings {
    Settings {
        connections: vec![ConnectionProfile::Ssh {
            id: "manual-1".into(),
            label: "alpha".into(),
            destination: "alpha.example".into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        }],
        ..Settings::default()
    }
}

fn ready_health() -> HostHealthView {
    HostHealthView::Checked {
        cached: CachedHostHealth {
            status: HostHealthStatus::Ready,
            checked_at: 1,
            herdr_version: Some("0.8.1".into()),
            session_count: Some(2),
            latency_ms: 12,
        },
        detail: String::new(),
    }
}

fn session_name(view: &OcHerdrView) -> Option<&str> {
    view.current_session().map(|session| session.name.as_str())
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
