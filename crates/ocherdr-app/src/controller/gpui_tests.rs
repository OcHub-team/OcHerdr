//! GPUI `TestAppContext` harness for controller wiring that unit tests cannot reach.
//!
//! These tests drive production `OcHerdrView` / `HostCenter` methods through the
//! same `Context<T>` / `Window` / `Entity<T>` path the GUI uses. They do not
//! introduce a second status type or a test-only controller.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext, prelude::*};
use ocherdr_core::{
    AgentInfo, AgentStatus, ConnectionProfile, HierarchySnapshot, PaneInfo, ReorderHover,
    Selection, SessionSummary, TabInfo, WorkspaceInfo,
};
use ocherdr_herdr::{HostHealthStatus, SessionConnection};
use serde_json::{Value, json};

use crate::host_center::HostCenter;
use crate::{
    AgentOutputState, AgentPromptPhase, AppearanceSettings, CachedHostHealth, EventStreamState,
    HostHealthView, I18n, Language, OcHerdrView, PendingListReorder, ReorderList, Settings,
    TAB_PILL_WIDTH, TAB_PREVIEW_DELAY, TAB_PREVIEW_GAP, TAB_PREVIEW_HEIGHT, TAB_PREVIEW_WIDTH,
    install_appearance, reorder_projection,
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
    events: Option<Sender<QueuedEvent>>,
    /// Every request the live-events server answered, in arrival order.
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

struct QueuedEvent {
    payload: Value,
    written: SyncSender<()>,
}

struct StalePaneHerdr {
    socket_path: PathBuf,
    snapshot_requests: Arc<AtomicUsize>,
    agent_status_rejections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl StalePaneHerdr {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind stale pane socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking stale pane socket");
        let snapshot_requests = Arc::new(AtomicUsize::new(0));
        let agent_status_rejections = Arc::new(AtomicUsize::new(0));
        let server_snapshot_requests = snapshot_requests.clone();
        let server_agent_status_rejections = agent_status_rejections.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let snapshot = agent_snapshot();
        let server = thread::spawn(move || {
            serve_stale_pane_subscribe(
                listener,
                server_stop,
                snapshot,
                server_snapshot_requests,
                server_agent_status_rejections,
            );
        });
        Self {
            socket_path,
            snapshot_requests,
            agent_status_rejections,
            stop,
            server: Some(server),
            _dir: dir,
        }
    }
}

impl Drop for StalePaneHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

impl FakeHerdr {
    fn snapshot_ok_subscribe_rejected() -> Self {
        Self::start(None, serve_snapshot_ok_subscribe_rejected)
    }

    fn snapshot_with_live_events(snapshot: HierarchySnapshot) -> Self {
        let (events, receiver) = mpsc::channel();
        let requests: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        let mut fake = Self::start(Some(events), move |listener, stop| {
            serve_snapshot_with_live_events(listener, stop, snapshot, receiver, log);
        });
        fake.requests = requests;
        fake
    }

    fn requests_for(&self, method: &str) -> Vec<Value> {
        self.requests
            .lock()
            .expect("fake herdr request log")
            .iter()
            .filter(|request| request.get("method") == Some(&json!(method)))
            .cloned()
            .collect()
    }

    fn start(
        events: Option<Sender<QueuedEvent>>,
        serve: impl FnOnce(UnixListener, Arc<AtomicBool>) + Send + 'static,
    ) -> Self {
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
        let server = thread::spawn(move || serve(listener, server_stop));
        Self {
            herdr_path,
            events,
            requests: Arc::new(Mutex::new(Vec::new())),
            stop,
            server: Some(server),
            _dir: dir,
        }
    }

    fn socket_path(&self) -> PathBuf {
        self._dir.path().join("herdr.sock")
    }

    /// Do not return until the live subscription has accepted and flushed the
    /// event. This makes the fixture prove its injection happened before the
    /// view assertions use that event as evidence.
    fn send_event(&self, payload: Value) {
        let events = self.events.as_ref().expect("live event fixture");
        let (written, observed) = mpsc::sync_channel(0);
        events
            .send(QueuedEvent { payload, written })
            .expect("queue fake Herdr event");
        observed
            .recv_timeout(Duration::from_secs(1))
            .expect("fake Herdr event must be flushed to the subscribed stream");
    }
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.events = None;
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

#[derive(Clone)]
enum PromptReply {
    Success,
    Blocked,
    BlockedAfter(Arc<AtomicBool>),
}

struct FakeAgentHerdr {
    socket_path: PathBuf,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl FakeAgentHerdr {
    fn new(prompt_reply: PromptReply) -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake agent socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking fake agent socket");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let server = thread::spawn(move || {
            serve_agent_requests(listener, prompt_reply, server_requests, server_stop)
        });
        Self {
            socket_path,
            requests,
            stop,
            server: Some(server),
            _dir: dir,
        }
    }

    fn requests_for(&self, method: &str) -> Vec<Value> {
        self.requests
            .lock()
            .expect("fake agent request log")
            .iter()
            .filter(|request| request.get("method") == Some(&json!(method)))
            .cloned()
            .collect()
    }
}

impl Drop for FakeAgentHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn serve_agent_requests(
    listener: UnixListener,
    prompt_reply: PromptReply,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => reply_to_agent_request(stream, &prompt_reply, &requests, &stop),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

fn reply_to_agent_request(
    stream: UnixStream,
    prompt_reply: &PromptReply,
    requests: &Mutex<Vec<Value>>,
    stop: &AtomicBool,
) {
    let _ = stream.set_nonblocking(false);
    let mut reader = BufReader::new(stream.try_clone().expect("clone fake agent stream"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let request: Value = serde_json::from_str(&line).expect("decode agent request");
    requests
        .lock()
        .expect("fake agent request log")
        .push(request.clone());
    let id = request.get("id").cloned().expect("request id");
    if request.get("method") == Some(&json!("agent.prompt"))
        && let PromptReply::BlockedAfter(gate) = prompt_reply
    {
        while !gate.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(2));
        }
    }
    let response = match request.get("method").and_then(Value::as_str) {
        Some("agent.get") => json!({
            "id": id,
            "result": {
                "type": "agent_info",
                "agent": { "pane_id": "p1", "name": "reviewer" },
            },
        }),
        Some("agent.read") => json!({
            "id": id,
            "result": {
                "type": "pane_read",
                "read": { "text": "recent output\n", "truncated": false },
            },
        }),
        Some("agent.rename") => json!({
            "id": id,
            "result": {
                "type": "agent_info",
                "agent": { "pane_id": "p1", "name": "server-confirmed" },
            },
        }),
        Some("session.snapshot") => json!({
            "id": id,
            "result": { "snapshot": agent_snapshot() },
        }),
        Some("agent.prompt") if matches!(prompt_reply, PromptReply::Success) => json!({
            "id": id,
            "result": { "type": "agent_prompted" },
        }),
        Some("agent.prompt") => json!({
            "id": id,
            "error": {
                "code": "agent_blocked",
                "message": "agent reviewer is blocked",
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
    let mut payload = serde_json::to_vec(&response).expect("encode fake agent reply");
    payload.push(b'\n');
    let mut stream = stream;
    let _ = stream.write_all(&payload);
    let _ = stream.flush();
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

fn serve_snapshot_with_live_events(
    listener: UnixListener,
    stop: Arc<AtomicBool>,
    snapshot: HierarchySnapshot,
    events: Receiver<QueuedEvent>,
    requests: Arc<Mutex<Vec<Value>>>,
) {
    let mut events = Some(events);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted streams inherit the listener's non-blocking mode;
                // a request that has not fully arrived yet must block, not
                // kill the server thread with `WouldBlock`.
                let _ = stream.set_nonblocking(false);
                let mut reader = BufReader::new(stream.try_clone().expect("clone fake stream"));
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                    continue;
                }
                let request: Value = serde_json::from_str(&line).expect("parse fake request");
                requests
                    .lock()
                    .expect("fake herdr request log")
                    .push(request.clone());
                let id = request.get("id").cloned().unwrap_or(json!(""));
                match request.get("method").and_then(Value::as_str) {
                    Some("events.subscribe") => match events.take() {
                        Some(receiver) => {
                            thread::spawn(move || stream_fake_events(stream, id, receiver));
                        }
                        // A snapshot with panes makes the view open a second
                        // subscription for agent status: acknowledge it and
                        // hold the stream open without ever emitting.
                        None => {
                            thread::spawn(move || hold_fake_subscription(stream, id));
                        }
                    },
                    Some("session.snapshot") => write_fake_response(
                        stream,
                        json!({
                            "id": id,
                            "result": { "snapshot": snapshot },
                        }),
                    ),
                    Some("tab.move") | Some("layout.set_split_ratio") => write_fake_response(
                        stream,
                        json!({
                            "id": id,
                            "result": { "type": "ok" },
                        }),
                    ),
                    // A swap against `reject` plays a Herdr that refuses (for
                    // example a zoomed tab); anything else is accepted and the
                    // test injects the matching `layout.updated` itself.
                    Some("pane.swap") if request["params"]["target_pane_id"] == json!("reject") => {
                        write_fake_response(
                            stream,
                            json!({
                                "id": id,
                                "error": {
                                    "code": "zoomed_tab",
                                    "message": "cannot swap panes in a zoomed tab",
                                },
                            }),
                        )
                    }
                    Some("pane.swap") => write_fake_response(
                        stream,
                        json!({
                            "id": id,
                            "result": {
                                "type": "pane_swap",
                                "swap": {
                                    "changed": true,
                                    "source_pane_id": request["params"]["source_pane_id"],
                                    "target_pane_id": request["params"]["target_pane_id"],
                                    "focused_pane_id": request["params"]["source_pane_id"],
                                    "layout": Value::Null,
                                },
                            },
                        }),
                    ),
                    // The fixture plays an old Herdr when asked to move into
                    // `unsupported`: the request enum fails to deserialize.
                    Some("pane.move")
                        if request["params"]["target_tab_id"] == json!("unsupported") =>
                    {
                        write_fake_response(
                            stream,
                            json!({
                                "id": id,
                                "error": {
                                    "code": "invalid_request",
                                    "message": "invalid request: unknown variant `pane.move`, expected one of `pane.list`, `pane.get`",
                                },
                            }),
                        )
                    }
                    Some("pane.move") => write_fake_response(
                        stream,
                        json!({
                            "id": id,
                            "result": {
                                "type": "ok",
                                "pane": { "pane_id": request["params"]["pane_id"] },
                                "created_tab": { "tab_id": "t-created", "number": 9 },
                            },
                        }),
                    ),
                    other => write_fake_response(
                        stream,
                        json!({
                            "id": id,
                            "error": {
                                "code": "unexpected",
                                "message": format!("unexpected method {other:?}"),
                            },
                        }),
                    ),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

fn stream_fake_events(mut stream: UnixStream, id: Value, events: Receiver<QueuedEvent>) {
    write_fake_response(
        stream.try_clone().expect("clone event stream"),
        json!({
            "id": id,
            "result": { "type": "subscription_started" },
        }),
    );
    for event in events {
        let mut payload = serde_json::to_vec(&event.payload).expect("encode fake Herdr event");
        payload.push(b'\n');
        if stream.write_all(&payload).is_err() || stream.flush().is_err() {
            return;
        }
        event
            .written
            .send(())
            .expect("confirm fake Herdr event write");
    }
}

fn hold_fake_subscription(mut stream: UnixStream, id: Value) {
    write_fake_response(
        stream.try_clone().expect("clone event stream"),
        json!({
            "id": id,
            "result": { "type": "subscription_started" },
        }),
    );
    // Blocks until the client drops its end.
    let mut byte = [0u8; 1];
    let _ = stream.read(&mut byte);
}

fn serve_stale_pane_subscribe(
    listener: UnixListener,
    stop: Arc<AtomicBool>,
    snapshot: HierarchySnapshot,
    snapshot_requests: Arc<AtomicUsize>,
    agent_status_rejections: Arc<AtomicUsize>,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => reply_to_stale_pane_request(
                stream,
                &snapshot,
                &snapshot_requests,
                &agent_status_rejections,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

fn reply_to_stale_pane_request(
    stream: UnixStream,
    snapshot: &HierarchySnapshot,
    snapshot_requests: &AtomicUsize,
    agent_status_rejections: &AtomicUsize,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone fake Herdr stream"));
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read fake Herdr request");
    let request: Value = serde_json::from_str(&line).expect("decode fake Herdr request");
    let id = request.get("id").cloned().expect("request id");
    match request.get("method").and_then(Value::as_str) {
        Some("session.snapshot") => {
            snapshot_requests.fetch_add(1, Ordering::Relaxed);
            write_fake_response(
                stream,
                json!({
                    "id": id,
                    "result": { "snapshot": snapshot },
                }),
            );
        }
        Some("events.subscribe")
            if request["params"]["subscriptions"]
                .as_array()
                .is_some_and(|subscriptions| {
                    subscriptions.iter().any(|subscription| {
                        subscription.get("type") == Some(&json!("pane.agent_status_changed"))
                            && subscription.get("pane_id").is_some()
                    })
                }) =>
        {
            agent_status_rejections.fetch_add(1, Ordering::Relaxed);
            write_fake_response(
                stream,
                json!({
                    "id": id,
                    "error": {
                        "code": "pane_not_found",
                        "message": "pane w19:p3 not found",
                    },
                }),
            );
        }
        other => write_fake_response(
            stream,
            json!({
                "id": id,
                "error": {
                    "code": "unexpected",
                    "message": format!("unexpected method {other:?}"),
                },
            }),
        ),
    }
}

fn write_fake_response(mut stream: UnixStream, response: Value) {
    let mut payload = serde_json::to_vec(&response).expect("encode fake Herdr response");
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .expect("write fake Herdr response");
    stream.flush().expect("flush fake Herdr response");
}

fn test_tab(tab_id: &str, number: usize, label: &str) -> TabInfo {
    TabInfo {
        tab_id: tab_id.into(),
        workspace_id: "w".into(),
        number,
        label: label.into(),
        focused: number == 1,
        pane_count: 0,
        agent_status: AgentStatus::Idle,
    }
}

fn three_tab_snapshot() -> HierarchySnapshot {
    HierarchySnapshot {
        focused_workspace_id: Some("w".into()),
        focused_tab_id: Some("t-a".into()),
        workspaces: vec![WorkspaceInfo {
            workspace_id: "w".into(),
            number: 1,
            label: "workspace".into(),
            focused: true,
            pane_count: 0,
            tab_count: 3,
            active_tab_id: "t-a".into(),
            agent_status: AgentStatus::Idle,
            tokens: Default::default(),
            worktree: None,
        }],
        tabs: vec![
            test_tab("t-a", 1, "alpha"),
            test_tab("t-b", 2, "beta"),
            test_tab("t-c", 3, "gamma"),
        ],
        ..Default::default()
    }
}

fn overflowing_tab_snapshot() -> HierarchySnapshot {
    let tabs = (1..=12)
        .map(|number| test_tab(&format!("t-{number}"), number, &format!("tab {number}")))
        .collect();
    HierarchySnapshot {
        focused_workspace_id: Some("w".into()),
        focused_tab_id: Some("t-1".into()),
        workspaces: vec![WorkspaceInfo {
            workspace_id: "w".into(),
            number: 1,
            label: "workspace".into(),
            focused: true,
            pane_count: 0,
            tab_count: 12,
            active_tab_id: "t-1".into(),
            agent_status: AgentStatus::Idle,
            tokens: Default::default(),
            worktree: None,
        }],
        tabs,
        ..Default::default()
    }
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

fn agent_snapshot() -> HierarchySnapshot {
    HierarchySnapshot {
        version: "0.8.2".into(),
        protocol: 20,
        focused_workspace_id: Some("w1".into()),
        focused_tab_id: Some("t1".into()),
        focused_pane_id: Some("p1".into()),
        workspaces: vec![WorkspaceInfo {
            workspace_id: "w1".into(),
            number: 1,
            label: "agent checks".into(),
            focused: true,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: "t1".into(),
            agent_status: AgentStatus::Idle,
            tokens: HashMap::new(),
            worktree: None,
        }],
        tabs: vec![TabInfo {
            tab_id: "t1".into(),
            workspace_id: "w1".into(),
            number: 1,
            label: "agent".into(),
            focused: true,
            pane_count: 1,
            agent_status: AgentStatus::Idle,
        }],
        panes: vec![PaneInfo {
            pane_id: "p1".into(),
            terminal_id: "term-p1".into(),
            workspace_id: "w1".into(),
            tab_id: "t1".into(),
            focused: true,
            cwd: None,
            foreground_cwd: None,
            label: None,
            agent: Some("claude".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: Some("Claude Code".into()),
            agent_status: AgentStatus::Idle,
            state_labels: HashMap::new(),
            tokens: HashMap::new(),
            revision: 1,
        }],
        layouts: Vec::new(),
        agents: vec![AgentInfo {
            pane_id: "p1".into(),
            name: Some("reviewer".into()),
        }],
    }
}

fn connect_agent_view(view: &mut OcHerdrView, fake: &FakeAgentHerdr) {
    let session = SessionSummary {
        name: "agent-test".into(),
        running: true,
        socket_path: fake.socket_path.clone(),
        session_dir: fake._dir.path().join("agent-test"),
        default: false,
    };
    view.connection = Some(
        SessionConnection::connect(&view.profiles[0], &session)
            .expect("connect fake agent session"),
    );
    view.sessions = vec![session];
    view.session_index = Some(0);
    view.snapshot = Some(agent_snapshot());
    view.selection = Selection {
        connection_id: "local".into(),
        session_name: Some("agent-test".into()),
        workspace_id: Some("w1".into()),
        tab_id: Some("t1".into()),
        pane_id: Some("p1".into()),
    };
    view.event_stream = EventStreamState::Live;
    view.operation = None;
}

fn click_agent_row(cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds("agent-p1")
        .expect("agent row should be in the rendered tree");
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();
}

fn click_send_prompt(cx: &mut VisualTestContext) {
    let bounds = cx
        .debug_bounds("send-agent-prompt")
        .expect("send prompt should be in the rendered tree");
    cx.simulate_click(bounds.center(), gpui::Modifiers::default());
    cx.run_until_parked();
}

fn session_name(view: &OcHerdrView) -> Option<&str> {
    view.current_session().map(|session| session.name.as_str())
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
fn clicking_an_agent_row_reads_its_name_and_recent_output(cx: &mut TestAppContext) {
    let fake = FakeAgentHerdr::new(PromptReply::Success);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        connect_agent_view(this, &fake);
        cx.notify();
    });

    click_agent_row(cx);

    let reads = fake.requests_for("agent.read");
    assert_eq!(reads.len(), 1, "the row callback must issue one agent.read");
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
    click_agent_row(cx);
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
    click_agent_row(cx);
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
    click_agent_row(cx);
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
    click_agent_row(cx);
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

fn pane_move_capable_snapshot() -> HierarchySnapshot {
    HierarchySnapshot {
        version: "0.7.0".into(),
        protocol: 14,
        ..three_tab_snapshot()
    }
}

/// Wire the view to the fake socket directly and pull the snapshot through
/// `resync_snapshot`, the same path every live refresh takes. This skips
/// `reload`'s `herdr session list` process spawn, which is the one step in
/// that path that can fail under fork pressure and would otherwise leave the
/// capability assertions racing the host's load rather than the code.
fn connect_view_to_fake_and_resync(
    view: &Entity<OcHerdrView>,
    fake: &FakeHerdr,
    cx: &mut VisualTestContext,
) {
    view.update(cx, |this, cx| {
        point_local_profile_at_fake(this, fake);
        let session = SessionSummary {
            name: "alpha".into(),
            running: true,
            socket_path: fake.socket_path(),
            session_dir: fake._dir.path().join("alpha"),
            default: false,
        };
        this.connection = Some(
            SessionConnection::connect(&this.profiles[0], &session)
                .expect("connect fake herdr session"),
        );
        this.sessions = vec![session];
        this.session_index = Some(0);
        // Subscribe the way `reload` does, so injected fake events reach the
        // production `apply_event_batch` path.
        let subscription = this
            .connection
            .as_ref()
            .expect("connected above")
            .subscribe_background()
            .expect("subscribe to fake herdr events");
        this.event_listen = Some(OcHerdrView::listen_events(subscription, cx));
        this.event_stream = EventStreamState::Live;
        let epoch = this.event_epoch;
        this.resync_snapshot(epoch, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.connection.is_some(),
            "the direct connection stays wired"
        );
        assert!(
            this.snapshot.is_some() && !this.snapshot_refreshing,
            "the fake's snapshot must be applied before the capability is read"
        );
    });
}

#[gpui::test]
fn invoke_with_response_hands_the_whole_result_to_the_callback(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_with_live_events(pane_move_capable_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);

    let received: Arc<Mutex<Option<Result<Value, String>>>> = Arc::new(Mutex::new(None));
    let sink = received.clone();
    view.update(cx, |this, cx| {
        assert!(
            this.pane_move_supported(),
            "snapshot advertises protocol 14"
        );
        this.invoke_with_response(
            "pane.move",
            json!({ "pane_id": "p-1", "target_tab_id": "t-b" }),
            move |this, result, _cx| {
                assert!(
                    this.operation.is_none(),
                    "the running indicator clears before the callback runs"
                );
                *sink.lock().unwrap() = Some(result.map_err(|error| error.to_string()));
            },
            cx,
        );
        assert!(this.operation.is_some(), "the request shows as running");
    });
    cx.run_until_parked();

    let result = received
        .lock()
        .unwrap()
        .take()
        .expect("callback runs once the socket answers")
        .expect("fake Herdr accepted the move");
    assert_eq!(result["created_tab"]["tab_id"], json!("t-created"));
    assert_eq!(result["pane"]["pane_id"], json!("p-1"));
    view.read_with(cx, |this, _| {
        assert!(this.operation.is_none());
        assert!(this.pane_move_supported());
    });
}

#[gpui::test]
fn an_unknown_pane_move_method_degrades_the_capability_and_reports_the_error(
    cx: &mut TestAppContext,
) {
    let fake = FakeHerdr::snapshot_with_live_events(pane_move_capable_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);

    let received: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let sink = received.clone();
    view.update(cx, |this, cx| {
        assert!(
            this.pane_move_supported(),
            "snapshot advertises protocol 14"
        );
        this.invoke_with_response(
            "pane.move",
            json!({ "pane_id": "p-1", "target_tab_id": "unsupported" }),
            move |_this, result, _cx| {
                *sink.lock().unwrap() = Some(result.expect_err("fake rejects").to_string());
            },
            cx,
        );
    });
    cx.run_until_parked();

    let error = received
        .lock()
        .unwrap()
        .take()
        .expect("callback sees the error");
    assert!(error.contains("unknown variant"), "{error}");
    view.read_with(cx, |this, _| {
        assert!(this.operation.is_none());
        assert!(
            !this.pane_move_supported(),
            "an unknown-method rejection flips the capability off"
        );
    });
}

#[gpui::test]
fn a_snapshot_without_pane_move_metadata_leaves_the_capability_off(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_with_live_events(three_tab_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);

    view.read_with(cx, |this, _| {
        assert!(this.connection.is_some());
        assert!(!this.pane_move_supported());
    });
}

// ---- Pane drag: centre swap (design §5, §7, §14.2) ----

fn layout_rect(x: u16, y: u16, width: u16, height: u16) -> ocherdr_core::LayoutRect {
    ocherdr_core::LayoutRect {
        x,
        y,
        width,
        height,
    }
}

fn split_pane(pane_id: &str, focused: bool) -> PaneInfo {
    PaneInfo {
        pane_id: pane_id.into(),
        terminal_id: format!("term-{pane_id}"),
        workspace_id: "w".into(),
        tab_id: "t-a".into(),
        focused,
        cwd: None,
        foreground_cwd: None,
        label: Some(pane_id.to_uppercase()),
        agent: None,
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        display_agent: None,
        agent_status: AgentStatus::Idle,
        state_labels: HashMap::new(),
        tokens: HashMap::new(),
        revision: 1,
    }
}

/// `t-a` holds `p-left | p-right` split down the middle of a 120×40 area.
fn two_pane_layout(left: &str, right: &str) -> ocherdr_core::PaneLayout {
    ocherdr_core::PaneLayout {
        workspace_id: "w".into(),
        tab_id: "t-a".into(),
        zoomed: false,
        area: layout_rect(0, 0, 120, 40),
        focused_pane_id: left.into(),
        panes: vec![
            ocherdr_core::LayoutPane {
                pane_id: left.into(),
                focused: true,
                rect: layout_rect(0, 0, 60, 40),
            },
            ocherdr_core::LayoutPane {
                pane_id: right.into(),
                focused: false,
                rect: layout_rect(60, 0, 60, 40),
            },
        ],
        splits: vec![ocherdr_core::LayoutSplit {
            id: "split_0_root".into(),
            direction: ocherdr_core::SplitDirection::Right,
            ratio: 0.5,
            rect: layout_rect(0, 0, 120, 40),
        }],
    }
}

fn two_pane_snapshot() -> HierarchySnapshot {
    let mut snapshot = pane_move_capable_snapshot();
    snapshot.focused_pane_id = Some("p-left".into());
    snapshot.panes = vec![split_pane("p-left", true), split_pane("p-right", false)];
    snapshot.layouts = vec![two_pane_layout("p-left", "p-right")];
    snapshot
}

/// Surface the panes are laid out into, in window pixels: the same numbers
/// the canvas measures in production, set directly so the gesture math is
/// deterministic and independent of the test window's size.
const SURFACE: (f32, f32, f32, f32) = (100., 50., 600., 400.);

fn connect_two_pane_view(
    cx: &mut TestAppContext,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| {
        this.terminal_surface_bounds = Some(SURFACE);
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });
    (fake, view, cx)
}

/// Grab `p-left` by its handle and drop it at `release` (window pixels).
fn drag_left_pane_to(view: &Entity<OcHerdrView>, release: (f32, f32), cx: &mut VisualTestContext) {
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    view.update_in(cx, |this, window, cx| {
        assert!(
            this.begin_pane_drag("p-left".into(), grab),
            "the handle arms a drag"
        );
        assert!(this.update_pane_drag((grab.0 + 40., grab.1 + 30.), cx));
        assert!(this.update_pane_drag(release, cx));
        assert!(this.finish_pane_drag(release, window, cx));
    });
}

fn swapped_layout_event() -> Value {
    json!({
        "event": "layout_updated",
        "data": {
            "type": "layout_updated",
            "layout": two_pane_layout("p-right", "p-left"),
        }
    })
}

#[gpui::test]
fn a_centre_drop_sends_one_pane_swap_and_settles_on_the_matching_layout(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    // Centre of the right pane: x = 100 + 600 * 0.75, y = 50 + 200.
    drag_left_pane_to(&view, (550., 250.), cx);
    view.read_with(cx, |this, _| {
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(
            this.tab_relocation_locked("t-a"),
            "the plan locks the tab while the swap is in flight"
        );
        let pending = this.pane_relocations.get("t-a").expect("plan pending");
        assert_eq!(pending.plan.source_pane_id, "p-left");
        assert_eq!(pending.plan.target_pane_id, "p-right");
        let predicted = this
            .displayed_pane_fractions(
                this.snapshot.as_ref().and_then(|s| s.layout_for("t-a")),
                "p-left",
                Instant::now(),
                false,
            )
            .expect("predicted rect");
        assert!(
            (predicted.0 - 0.5).abs() < 1e-6,
            "the source pane is drawn on the right immediately: {predicted:?}"
        );
    });
    cx.run_until_parked();

    let swaps = fake.requests_for("pane.swap");
    assert_eq!(swaps.len(), 1, "exactly one pane.swap: {swaps:?}");
    assert_eq!(swaps[0]["params"]["source_pane_id"], json!("p-left"));
    assert_eq!(swaps[0]["params"]["target_pane_id"], json!("p-right"));
    view.read_with(cx, |this, _| {
        let pending = this
            .pane_relocations
            .get("t-a")
            .expect("still waiting for layout");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::Swapping {
                responded: true,
                layout_seen: false
            }
        );
    });

    fake.send_event(swapped_layout_event());
    thread::sleep(Duration::from_millis(20));
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a");
        assert!(
            pending.is_none()
                || matches!(
                    pending.map(|p| &p.phase),
                    Some(crate::RelocationPhase::Settling { .. })
                ),
            "response + matching layout.updated moves the plan into Settling"
        );
    });
    thread::sleep(Duration::from_millis(220));
    view.update(cx, |this, _| {
        this.expire_pane_motion(Instant::now(), false);
        assert!(
            !this.tab_relocation_locked("t-a"),
            "the settle animation releases the tab"
        );
        let layout = this
            .snapshot
            .as_ref()
            .and_then(|s| s.layout_for("t-a"))
            .expect("layout");
        assert_eq!(
            layout.panes[0].pane_id, "p-right",
            "authoritative layout on screen"
        );
    });
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
}

#[gpui::test]
fn an_invalid_drop_sends_nothing_and_returns_home(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    // Well outside every pane.
    drag_left_pane_to(&view, (SURFACE.0 + SURFACE.2 + 80., 20.), cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(
            this.pane_relocations.is_empty(),
            "no plan without a drop zone"
        );
        assert_eq!(
            this.pane_drag_return.as_ref().map(|flight| flight.to),
            Some((SURFACE.0, SURFACE.1, SURFACE.2 / 2., SURFACE.3)),
            "the preview flies back to the source slot"
        );
    });
    assert!(fake.requests_for("pane.swap").is_empty());
}

#[gpui::test]
fn an_edge_drop_is_not_droppable_in_phase_two(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    // Far right edge of the right pane: zone Right, computed but not droppable.
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    let edge = (SURFACE.0 + SURFACE.2 - 6., SURFACE.1 + 200.);
    view.update_in(cx, |this, window, cx| {
        assert!(this.begin_pane_drag("p-left".into(), grab));
        this.update_pane_drag(edge, cx);
        let crate::SurfaceDrag::Pane(drag) = &this.surface_drag else {
            panic!("dragging");
        };
        let hover = drag.hover.as_ref().expect("hover over the right pane");
        assert_eq!(hover.target_pane_id, "p-right");
        assert_eq!(hover.zone, ocherdr_core::DropZone::Right);
        assert!(!hover.droppable(drag.edge_drops));
        this.finish_pane_drag(edge, window, cx);
        assert!(this.pane_relocations.is_empty());
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.swap").is_empty());
}

#[gpui::test]
fn escape_cancels_a_pane_drag_without_a_request(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    view.update_in(cx, |this, window, cx| {
        assert!(this.begin_pane_drag("p-left".into(), grab));
        this.update_pane_drag((550., 250.), cx);
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Pane(_)));
        let escape = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: Default::default(),
                key: "escape".into(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(this.handle_app_shortcut(&escape, window, cx));
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(
            this.pane_drag_return.is_some(),
            "the lifted preview returns"
        );
        // Releasing afterwards is a no-op: nothing is dragging any more.
        assert!(!this.finish_pane_drag((550., 250.), window, cx));
    });
    cx.run_until_parked();
    assert!(fake.requests_for("pane.swap").is_empty());
    view.read_with(cx, |this, _| assert!(this.pane_relocations.is_empty()));
}

#[gpui::test]
fn a_layout_that_does_not_match_the_plan_reverts_to_authority(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    drag_left_pane_to(&view, (550., 250.), cx);
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.swap").len(), 1);

    // Someone else split the tab before our swap landed: the pane set no
    // longer matches the plan.
    let mut layout = two_pane_layout("p-right", "p-left");
    layout.panes.push(ocherdr_core::LayoutPane {
        pane_id: "p-new".into(),
        focused: false,
        rect: layout_rect(60, 20, 60, 20),
    });
    layout.panes[1].rect = layout_rect(60, 0, 60, 20);
    layout.splits.push(ocherdr_core::LayoutSplit {
        id: "split_1_1".into(),
        direction: ocherdr_core::SplitDirection::Down,
        ratio: 0.5,
        rect: layout_rect(60, 0, 60, 40),
    });
    fake.send_event(json!({
        "event": "layout_updated",
        "data": { "type": "layout_updated", "layout": layout }
    }));
    thread::sleep(Duration::from_millis(20));
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.pane_relocations.is_empty(),
            "a mismatching layout.updated drops the prediction"
        );
        let rect = this
            .displayed_pane_fractions(
                this.snapshot.as_ref().and_then(|s| s.layout_for("t-a")),
                "p-left",
                Instant::now(),
                false,
            )
            .expect("authoritative rect");
        assert!(
            (rect.3 - 0.5).abs() < 1e-6,
            "authoritative geometry wins: {rect:?}"
        );
    });
}

#[gpui::test]
fn a_rejected_swap_drops_the_plan_and_unlocks_the_tab(cx: &mut TestAppContext) {
    let mut snapshot = two_pane_snapshot();
    snapshot.panes[1].pane_id = "reject".into();
    snapshot.layouts = vec![two_pane_layout("p-left", "reject")];
    let fake = FakeHerdr::snapshot_with_live_events(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| this.headless_terminals = true);
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| this.terminal_surface_bounds = Some(SURFACE));
    drag_left_pane_to(&view, (550., 250.), cx);
    view.read_with(cx, |this, _| assert!(this.tab_relocation_locked("t-a")));
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
    view.read_with(cx, |this, _| {
        assert!(
            !this.tab_relocation_locked("t-a"),
            "the error response releases the lock"
        );
    });
}

#[gpui::test]
fn a_locked_tab_refuses_a_second_drag_and_pane_close(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    drag_left_pane_to(&view, (550., 250.), cx);
    view.update(cx, |this, cx| {
        assert!(
            !this.begin_pane_drag("p-right".into(), (SURFACE.0 + 400., SURFACE.1 + 12.)),
            "one plan per tab"
        );
        this.request_close(
            crate::HierarchyTarget::Pane {
                id: "p-right".into(),
                label: "P-RIGHT".into(),
            },
            cx,
        );
        assert!(
            matches!(this.overlay, crate::Overlay::None),
            "pane close is refused while the plan is pending"
        );
        assert!(this.pane_resize_frozen("p-right"), "grids are frozen");
    });
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
}
