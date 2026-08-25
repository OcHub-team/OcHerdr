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
    HEADER_HEIGHT, HostHealthView, I18n, Language, OcHerdrView, PendingListReorder, ReorderList,
    Settings, TAB_PILL_WIDTH, TAB_PREVIEW_DELAY, TAB_PREVIEW_GAP, TAB_PREVIEW_HEIGHT,
    TAB_PREVIEW_WIDTH, TAB_STRIP_LEAD_INSET, install_appearance, reorder_projection,
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
    /// How the fake answers `pane.move` (scripted failures).
    script: Arc<PaneMoveScript>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

/// Scripted `pane.move` behaviour: how many of the next `new_tab` (step 1)
/// and `tab` (step 2) requests the fake rejects.
#[derive(Default)]
struct PaneMoveScript {
    park_failures: AtomicUsize,
    insert_failures: AtomicUsize,
    /// Events the fake broadcasts on the live subscription *before* it
    /// writes the next step-1 response: events ride their own socket, so
    /// in production they can beat the response that names the temp tab.
    events_before_park_response: Mutex<Vec<Value>>,
    /// The live subscription's queue, filled in by the constructor.
    event_queue: Mutex<Option<Sender<QueuedEvent>>>,
}

impl PaneMoveScript {
    /// Push `events` to the subscribed stream and wait until each one is
    /// flushed, the same proof `FakeHerdr::send_event` demands.
    fn broadcast(&self, events: Vec<Value>) {
        let queue = self.event_queue.lock().expect("event queue");
        let Some(queue) = queue.as_ref() else {
            return;
        };
        for payload in events {
            let (written, observed) = mpsc::sync_channel(0);
            queue
                .send(QueuedEvent { payload, written })
                .expect("queue fake Herdr event");
            observed
                .recv_timeout(Duration::from_secs(1))
                .expect("fake Herdr event must be flushed to the subscribed stream");
        }
    }
}

impl PaneMoveScript {
    fn take_failure(counter: &AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_ok()
    }
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
        Self::snapshot_with_live_events_and_script(snapshot, PaneMoveScript::default())
    }

    fn snapshot_with_live_events_and_script(
        snapshot: HierarchySnapshot,
        script: PaneMoveScript,
    ) -> Self {
        let (events, receiver) = mpsc::channel();
        let requests: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        *script.event_queue.lock().expect("event queue") = Some(events.clone());
        let script = Arc::new(script);
        let server_script = script.clone();
        let mut fake = Self::start(Some(events), move |listener, stop| {
            serve_snapshot_with_live_events(listener, stop, snapshot, receiver, log, server_script);
        });
        fake.requests = requests;
        fake.script = script;
        fake
    }

    /// Methods of every request in arrival order.
    fn request_methods(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("fake herdr request log")
            .iter()
            .filter_map(|request| request.get("method")?.as_str().map(str::to_owned))
            .collect()
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
            script: Arc::new(PaneMoveScript::default()),
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
    script: Arc<PaneMoveScript>,
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
                    // Step 1 of an edge relocation: park in a new tab. Mirrors
                    // Herdr's `PaneMoveResult` shape (`created_tab`, `pane`).
                    Some("pane.move")
                        if request["params"]["destination"]["type"] == json!("new_tab") =>
                    {
                        if PaneMoveScript::take_failure(&script.park_failures) {
                            write_fake_response(
                                stream,
                                json!({
                                    "id": id,
                                    "error": { "code": "pane_move_failed", "message": "park refused" },
                                }),
                            );
                            continue;
                        }
                        let early = std::mem::take(
                            &mut *script
                                .events_before_park_response
                                .lock()
                                .expect("early park events"),
                        );
                        if !early.is_empty() {
                            script.broadcast(early);
                            // Let the client's reader thread queue them before
                            // the response can be read.
                            thread::sleep(Duration::from_millis(60));
                        }
                        let pane_id = request["params"]["pane_id"].clone();
                        write_fake_response(
                            stream,
                            json!({
                                "id": id,
                                "result": {
                                    "type": "pane_move",
                                    "move_result": {
                                        "changed": true,
                                        "previous_pane_id": pane_id,
                                        "previous_workspace_id": "w",
                                        "previous_tab_id": "t-a",
                                        "pane": parked_pane_json(pane_id.as_str().unwrap_or("")),
                                        "target_layout": single_pane_layout("t-tmp", pane_id.as_str().unwrap_or("")),
                                        "created_tab": temp_tab(),
                                        "focused_pane_id": pane_id,
                                    },
                                },
                            }),
                        )
                    }
                    // Step 2: back into the original tab beside the target.
                    Some("pane.move")
                        if request["params"]["destination"]["type"] == json!("tab") =>
                    {
                        if PaneMoveScript::take_failure(&script.insert_failures) {
                            write_fake_response(
                                stream,
                                json!({
                                    "id": id,
                                    "error": { "code": "pane_move_failed", "message": "target pane could not be split" },
                                }),
                            );
                            continue;
                        }
                        let pane_id = request["params"]["pane_id"].clone();
                        write_fake_response(
                            stream,
                            json!({
                                "id": id,
                                "result": {
                                    "type": "pane_move",
                                    "move_result": {
                                        "changed": true,
                                        "previous_pane_id": pane_id,
                                        "previous_workspace_id": "w",
                                        "previous_tab_id": "t-tmp",
                                        "pane": { "pane_id": pane_id, "tab_id": "t-a" },
                                        "target_layout": Value::Null,
                                        "closed_tab_id": "t-tmp",
                                        "focused_pane_id": pane_id,
                                    },
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
        space.size.width > gpui::px(100.),
        "the free space fills the strip"
    );
    for tab in ["tab-t-a", "tab-t-b", "tab-t-c"] {
        let bounds = cx.debug_bounds(tab).unwrap();
        assert!(!bounds.intersects(&space) && !bounds.intersects(&lead));
    }
    // The press itself cannot be simulated: the test platform's
    // `start_window_move` is `unimplemented!()`.
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
fn an_edge_drop_is_not_droppable_with_the_flag_off(cx: &mut TestAppContext) {
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

/// Esc is handled by the root `on_key_down`, which GPUI dispatches along
/// the focused element's ancestry; with nothing focused only the window's
/// root node (the view wrapper, above our root div) receives the key. The
/// handle press stops propagation, so it cannot rely on the surface's
/// focus-on-click: it focuses the surface itself, and Esc then reaches the
/// drag whether or not anything was focused before the grab.
#[gpui::test]
fn escape_reaches_the_drag_through_window_dispatch_regardless_of_focus(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    let grab = (SURFACE.0 + 12., SURFACE.1 + 12.);
    let press = gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: gpui::point(gpui::px(grab.0), gpui::px(grab.1)),
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    };
    for surface_focused_before in [false, true] {
        view.update_in(cx, |this, window, cx| {
            if surface_focused_before {
                this.focus.focus(window, cx);
            } else {
                window.blur();
            }
            assert_eq!(this.focus.is_focused(window), surface_focused_before);
            this.press_pane_handle("p-left".into(), &press, window, cx);
            assert!(
                this.focus.is_focused(window),
                "the grab focuses the surface"
            );
            assert!(this.update_pane_drag((550., 250.), cx));
            assert!(matches!(this.surface_drag, crate::SurfaceDrag::Pane(_)));
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        view.update(cx, |this, _| {
            assert!(
                matches!(this.surface_drag, crate::SurfaceDrag::Idle),
                "Esc cancels with surface_focused_before={surface_focused_before}"
            );
            assert!(this.pane_drag_return.is_some(), "the preview flies back");
            this.pane_drag_return = None;
        });
    }
    assert!(fake.requests_for("pane.swap").is_empty());
    assert!(fake.requests_for("pane.move").is_empty());
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

// ---- Split drag: squeeze preview (design §5.4) ----

#[gpui::test]
fn a_split_drag_squeezes_the_preview_and_sends_one_set_split_ratio(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_two_pane_view(cx);
    // The divider sits at x = SURFACE.0 + 300 (ratio 0.5 of 600 px); the
    // pointer lands at 0.7.
    let press = (SURFACE.0 + 300., SURFACE.1 + 100.);
    let release = (SURFACE.0 + 420., SURFACE.1 + 100.);
    view.update(cx, |this, cx| {
        let snapshot = this.snapshot.clone().expect("snapshot");
        let layout = snapshot.layout_for("t-a").expect("layout");
        let split = layout.splits[0].clone();
        let drag = super::split_drag_from_press("t-a".into(), &split, layout, SURFACE, press)
            .expect("split drag");
        this.surface_drag = crate::SurfaceDrag::Split(drag);
        assert!(this.update_split_drag(release, cx));
        assert!(
            this.pane_resize_frozen("p-left") && this.pane_resize_frozen("p-right"),
            "terminal surfaces are not resized while the divider is dragged"
        );
        let left = this
            .displayed_pane_fractions(Some(layout), "p-left", Instant::now(), false)
            .expect("left rect");
        let right = this
            .displayed_pane_fractions(Some(layout), "p-right", Instant::now(), false)
            .expect("right rect");
        assert!((left.2 - 0.7).abs() < 1e-3, "left shell squeezes: {left:?}");
        assert!(
            (right.0 - 0.7).abs() < 1e-3,
            "right shell follows: {right:?}"
        );
        assert!((right.2 - 0.3).abs() < 1e-3, "{right:?}");
        assert!(this.finish_split_drag(release, cx));
        assert!(matches!(this.surface_drag, crate::SurfaceDrag::Idle));
        assert!(!this.pane_resize_frozen("p-left"), "release unfreezes");
        let back = this
            .displayed_pane_fractions(Some(layout), "p-left", Instant::now(), false)
            .expect("authoritative rect");
        assert!(
            (back.2 - 0.5).abs() < 1e-6,
            "authority until layout.updated"
        );
    });
    cx.run_until_parked();
    let requests = fake.requests_for("layout.set_split_ratio");
    assert_eq!(requests.len(), 1, "one request on release: {requests:?}");
    let ratio = requests[0]["params"]["ratio"].as_f64().expect("ratio");
    assert!((ratio - 0.7).abs() < 1e-3, "{ratio}");
}

// ---- Edge relocation (design §4.2, §7, phase 3) ----

fn temp_tab() -> TabInfo {
    TabInfo {
        tab_id: "t-tmp".into(),
        workspace_id: "w".into(),
        number: 9,
        label: "tmp".into(),
        focused: false,
        pane_count: 1,
        agent_status: AgentStatus::Idle,
    }
}

fn parked_pane_json(pane_id: &str) -> Value {
    let mut pane = split_pane(pane_id, false);
    pane.tab_id = "t-tmp".into();
    serde_json::to_value(pane).expect("pane json")
}

fn single_pane_layout(tab_id: &str, pane_id: &str) -> ocherdr_core::PaneLayout {
    ocherdr_core::PaneLayout {
        workspace_id: "w".into(),
        tab_id: tab_id.into(),
        zoomed: false,
        area: layout_rect(0, 0, 120, 40),
        focused_pane_id: pane_id.into(),
        panes: vec![ocherdr_core::LayoutPane {
            pane_id: pane_id.into(),
            focused: true,
            rect: layout_rect(0, 0, 120, 40),
        }],
        splits: Vec::new(),
    }
}

fn layout_event(layout: ocherdr_core::PaneLayout) -> Value {
    json!({
        "event": "layout_updated",
        "data": { "type": "layout_updated", "layout": layout }
    })
}

/// What Herdr broadcasts for step 1 (`p-left` parked in `t-tmp`):
/// `tab.created → pane.moved → layout.updated(t-a) → layout.updated(t-tmp)`.
fn park_events() -> Vec<Value> {
    vec![
        json!({ "event": "tab_created", "data": { "type": "tab_created", "tab": temp_tab() } }),
        json!({
            "event": "pane_moved",
            "data": {
                "type": "pane_moved",
                "pane": parked_pane_json("p-left"),
                "previous_pane_id": "p-left",
                "previous_workspace_id": "w",
                "previous_tab_id": "t-a",
                "created_tab": temp_tab(),
            }
        }),
        layout_event(single_pane_layout("t-a", "p-right")),
        layout_event(single_pane_layout("t-tmp", "p-left")),
    ]
}

/// What Herdr broadcasts for step 2 (`p-left` back beside `p-right` as the
/// second child): `tab.closed → pane.moved → layout.updated(t-a)`.
fn insert_events() -> Vec<Value> {
    let mut back = split_pane("p-left", true);
    back.tab_id = "t-a".into();
    vec![
        json!({
            "event": "tab_closed",
            "data": { "type": "tab_closed", "tab_id": "t-tmp", "workspace_id": "w" }
        }),
        json!({
            "event": "pane_moved",
            "data": {
                "type": "pane_moved",
                "pane": back,
                "previous_pane_id": "p-left",
                "previous_workspace_id": "w",
                "previous_tab_id": "t-tmp",
                "closed_tab_id": "t-tmp",
            }
        }),
        layout_event(two_pane_layout("p-right", "p-left")),
    ]
}

fn send_events(fake: &FakeHerdr, events: Vec<Value>, cx: &mut VisualTestContext) {
    for event in events {
        fake.send_event(event);
    }
    thread::sleep(Duration::from_millis(30));
    cx.run_until_parked();
}

fn connect_edge_view(
    cx: &mut TestAppContext,
    script: PaneMoveScript,
) -> (FakeHerdr, Entity<OcHerdrView>, &mut VisualTestContext) {
    let fake = FakeHerdr::snapshot_with_live_events_and_script(two_pane_snapshot(), script);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| {
        this.headless_terminals = true;
        this.pane_edge_relocation = true;
    });
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| {
        this.terminal_surface_bounds = Some(SURFACE);
        assert!(this.edge_drops_enabled(), "flag + capability");
    });
    (fake, view, cx)
}

/// Right edge of `p-right`.
const RIGHT_EDGE: (f32, f32) = (SURFACE.0 + SURFACE.2 - 6., SURFACE.1 + 200.);
/// Left edge of `p-right`.
const LEFT_EDGE: (f32, f32) = (SURFACE.0 + 300. + 6., SURFACE.1 + 200.);

fn assert_park_request(request: &Value) {
    assert_eq!(request["method"], json!("pane.move"));
    assert_eq!(request["params"]["pane_id"], json!("p-left"));
    assert_eq!(
        request["params"]["destination"],
        json!({ "type": "new_tab", "workspace_id": "w" })
    );
    assert_eq!(request["params"]["focus"], json!(false));
}

fn assert_insert_request(request: &Value) {
    assert_eq!(request["method"], json!("pane.move"));
    assert_eq!(
        request["params"]["pane_id"],
        json!("p-left"),
        "pane id comes from the step-1 response"
    );
    assert_eq!(request["params"]["destination"]["type"], json!("tab"));
    assert_eq!(request["params"]["destination"]["tab_id"], json!("t-a"));
    assert_eq!(
        request["params"]["destination"]["target_pane_id"],
        json!("p-right")
    );
    assert_eq!(request["params"]["destination"]["split"], json!("right"));
    let ratio = request["params"]["destination"]["ratio"]
        .as_f64()
        .expect("ratio");
    assert!((ratio - 0.5).abs() < 1e-6, "{ratio}");
    assert_eq!(request["params"]["focus"], json!(true));
}

#[gpui::test]
fn a_right_drop_issues_two_pane_moves_back_to_back_and_settles_without_a_resync(
    cx: &mut TestAppContext,
) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    let snapshots_before = fake.requests_for("session.snapshot").len();
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("plan pending");
        assert_eq!(pending.phase, crate::RelocationPhase::Parking);
        assert!(matches!(
            pending.plan.intent,
            crate::RelocationIntent::Insert {
                edge: ocherdr_core::DropEdge::Right,
                ..
            }
        ));
        assert!(this.tab_relocation_locked("t-a"));
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
            "the source is drawn on the right at once: {predicted:?}"
        );
    });
    cx.run_until_parked();

    // Both requests went out before any event was injected, in order, the
    // second built from the first response.
    let methods = fake.request_methods();
    let moves: Vec<usize> = methods
        .iter()
        .enumerate()
        .filter(|(_, m)| *m == "pane.move")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(moves.len(), 2, "{methods:?}");
    assert_eq!(moves[1], moves[0] + 1, "back to back: {methods:?}");
    let requests = fake.requests_for("pane.move");
    assert_park_request(&requests[0]);
    assert_insert_request(&requests[1]);
    assert!(
        fake.requests_for("pane.swap").is_empty(),
        "right needs no swap"
    );
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("still pending");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::Inserting {
                temp_tab_id: "t-tmp".into(),
                moved_pane_id: "p-left".into(),
                responded: true,
                layout_seen: false,
            }
        );
        assert!(this.hidden_tab_ids().contains("t-tmp"));
    });

    // Step 1 events: the temp tab is real in the snapshot but hidden.
    send_events(&fake, park_events(), cx);
    view.read_with(cx, |this, _| {
        let snapshot = this.snapshot.as_ref().expect("snapshot");
        assert!(snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp"));
        assert_eq!(
            snapshot.pane("p-left").map(|p| p.tab_id.as_str()),
            Some("t-tmp")
        );
        let tabs: Vec<String> = this
            .chrome_a11y()
            .tabs
            .items
            .iter()
            .map(|row| row.a11y.id.clone())
            .collect();
        assert!(!tabs.contains(&"t-tmp".to_owned()), "hidden: {tabs:?}");
        let rendered: Vec<String> = this
            .rendered_panes_for_tab(snapshot, "t-a")
            .into_iter()
            .map(|pane| pane.pane_id)
            .collect();
        assert_eq!(rendered, vec!["p-right", "p-left"], "plan pane set");
        assert!(this.tab_relocation_locked("t-a"));
        assert!(this.pane_resize_frozen("p-left"), "frozen while parked");
        assert_eq!(
            this.selection.pane_id.as_deref(),
            Some("p-left"),
            "selection pinned to the moved pane"
        );
        let pending = this.pane_relocations.get("t-a").expect("still pending");
        assert!(matches!(
            pending.phase,
            crate::RelocationPhase::Inserting {
                layout_seen: false,
                ..
            }
        ));
    });

    // Step 2 events: the final layout settles the plan.
    send_events(&fake, insert_events(), cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a");
        assert!(
            pending.is_none()
                || matches!(
                    pending.map(|p| &p.phase),
                    Some(crate::RelocationPhase::Settling { .. })
                ),
            "response + matching layout → Settling: {:?}",
            pending.map(|p| &p.phase)
        );
        assert!(
            !this
                .snapshot
                .as_ref()
                .unwrap()
                .tabs
                .iter()
                .any(|tab| tab.tab_id == "t-tmp")
        );
    });
    thread::sleep(Duration::from_millis(220));
    view.update(cx, |this, _| {
        this.expire_pane_motion(Instant::now(), false);
        assert!(!this.tab_relocation_locked("t-a"));
        let layout = this
            .snapshot
            .as_ref()
            .and_then(|s| s.layout_for("t-a"))
            .expect("layout");
        assert_eq!(layout.panes[0].pane_id, "p-right");
        assert_eq!(layout.panes[1].pane_id, "p-left");
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-left"));
    });
    assert_eq!(
        fake.requests_for("session.snapshot").len(),
        snapshots_before,
        "the incremental pane.moved apply never resyncs"
    );
    assert_eq!(fake.requests_for("pane.move").len(), 2);
}

fn visible_tab_ids(view: &crate::OcHerdrView) -> Vec<String> {
    view.chrome_a11y()
        .tabs
        .items
        .iter()
        .map(|row| row.a11y.id.clone())
        .collect()
}

/// Events and responses ride different sockets: `tab.created` for the
/// temporary tab can be applied while the plan is still `Parking`, before
/// the step-1 response names the tab. The strip must hide it at every step
/// of the executor, not only once the response has been applied.
#[gpui::test]
fn the_temporary_tab_is_hidden_before_the_park_response_names_it(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        events_before_park_response: Mutex::new(park_events()),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_edge_view(cx, script);
    drag_left_pane_to(&view, RIGHT_EDGE, cx);

    // Step the executor one task at a time. The fake answers step 1 only
    // after the four step-1 events are on the wire, so the event task and
    // the response continuation are both runnable after the request.
    let mut parking_with_temp_tab = false;
    let mut ticks = 0;
    loop {
        let ran = cx.executor().tick();
        view.read_with(cx, |this, _| {
            assert!(
                !visible_tab_ids(this).iter().any(|id| id == "t-tmp"),
                "tick {ticks}: the temp tab reached the strip"
            );
            let temp_tab_known = this
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.tabs.iter().any(|tab| tab.tab_id == "t-tmp"));
            if temp_tab_known
                && this.pane_relocations.get("t-a").map(|p| &p.phase)
                    == Some(&crate::RelocationPhase::Parking)
            {
                parking_with_temp_tab = true;
                assert!(this.hidden_tab_ids().contains("t-tmp"));
            }
        });
        ticks += 1;
        if !ran {
            if fake.requests_for("pane.move").len() >= 2 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
            assert!(ticks < 2_000, "step 2 never went out");
        }
    }
    assert!(
        parking_with_temp_tab,
        "the race was exercised: the temp tab was in the snapshot while Parking"
    );
    view.update(cx, |this, cx| {
        assert!(matches!(
            this.pane_relocations.get("t-a").map(|p| &p.phase),
            Some(crate::RelocationPhase::Inserting { temp_tab_id, .. }) if temp_tab_id == "t-tmp"
        ));
        assert_eq!(visible_tab_ids(this), vec!["t-a", "t-b", "t-c"]);
        this.cycle_tab(-1, cx);
        assert_eq!(
            this.selection.tab_id.as_deref(),
            Some("t-c"),
            "tab navigation wraps past the hidden tab"
        );
        this.select_tab_number(1, cx);
        assert_eq!(this.selection.tab_id.as_deref(), Some("t-a"));
    });

    send_events(&fake, insert_events(), cx);
    thread::sleep(Duration::from_millis(220));
    view.update(cx, |this, _| {
        this.expire_pane_motion(Instant::now(), false);
        assert!(!this.tab_relocation_locked("t-a"));
        assert_eq!(visible_tab_ids(this), vec!["t-a", "t-b", "t-c"]);
        assert!(this.hidden_tab_ids().is_empty());
    });
}

#[gpui::test]
fn a_left_drop_adds_a_pane_swap_after_the_second_move(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    drag_left_pane_to(&view, LEFT_EDGE, cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("plan pending");
        assert!(matches!(
            pending.plan.intent,
            crate::RelocationIntent::Insert {
                edge: ocherdr_core::DropEdge::Left,
                ..
            }
        ));
        let predicted = this
            .displayed_pane_fractions(
                this.snapshot.as_ref().and_then(|s| s.layout_for("t-a")),
                "p-left",
                Instant::now(),
                false,
            )
            .expect("predicted rect");
        assert!(
            predicted.0.abs() < 1e-6 && (predicted.2 - 0.5).abs() < 1e-6,
            "left of the target: {predicted:?}"
        );
    });
    cx.run_until_parked();
    let methods = fake.request_methods();
    let tail: Vec<&str> = methods
        .iter()
        .filter(|m| m.starts_with("pane."))
        .map(String::as_str)
        .collect();
    assert_eq!(
        tail,
        vec!["pane.move", "pane.move", "pane.swap"],
        "strictly serial, no event in between: {methods:?}"
    );
    let moves = fake.requests_for("pane.move");
    assert_park_request(&moves[0]);
    assert_insert_request(&moves[1]);
    let swaps = fake.requests_for("pane.swap");
    assert_eq!(swaps[0]["params"]["source_pane_id"], json!("p-left"));
    assert_eq!(swaps[0]["params"]["target_pane_id"], json!("p-right"));
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("still pending");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::CorrectingOrder {
                responded: true,
                layout_seen: false,
            }
        );
    });
    send_events(&fake, park_events(), cx);
    send_events(&fake, insert_events(), cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("still pending");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::CorrectingOrder {
                responded: true,
                layout_seen: false,
            },
            "the step-2 layout (source second) is not the landing"
        );
    });
    // The swap's layout: source first.
    send_events(
        &fake,
        vec![layout_event(two_pane_layout("p-left", "p-right"))],
        cx,
    );
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a");
        assert!(
            pending.is_none()
                || matches!(
                    pending.map(|p| &p.phase),
                    Some(crate::RelocationPhase::Settling { .. })
                ),
            "{:?}",
            pending.map(|p| &p.phase)
        );
    });
    assert_eq!(fake.requests_for("pane.move").len(), 2);
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
}

#[gpui::test]
fn a_failed_second_move_parks_the_pane_and_retry_reissues_it(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        insert_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_edge_view(cx, script);
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 2);
    send_events(&fake, park_events(), cx);
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("parked plan");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::Parked {
                temp_tab_id: "t-tmp".into(),
                moved_pane_id: "p-left".into(),
            }
        );
        assert!(this.parked_relocation("t-a").is_some(), "inline notice");
        assert!(!this.tab_relocation_locked("t-a"), "no prediction, no lock");
        assert!(
            this.hidden_tab_ids().is_empty(),
            "the temp tab is shown while parked"
        );
        assert!(
            this.displayed_pane_fractions(
                this.snapshot.as_ref().and_then(|s| s.layout_for("t-a")),
                "p-right",
                Instant::now(),
                false,
            )
            .is_some_and(|rect| (rect.2 - 1.).abs() < 1e-6),
            "authoritative single-pane layout on screen"
        );
    });
    view.update(cx, |this, cx| this.retry_parked_relocation("t-a", cx));
    cx.run_until_parked();
    let moves = fake.requests_for("pane.move");
    assert_eq!(moves.len(), 3, "retry re-issues step 2 only");
    assert_insert_request(&moves[2]);
    assert!(fake.requests_for("pane.swap").is_empty());
    view.read_with(cx, |this, _| {
        let pending = this.pane_relocations.get("t-a").expect("inserting again");
        assert!(matches!(
            pending.phase,
            crate::RelocationPhase::Inserting {
                responded: true,
                ..
            }
        ));
        assert!(this.tab_relocation_locked("t-a"));
    });
}

#[gpui::test]
fn a_failed_first_move_reverts_without_touching_the_selection(cx: &mut TestAppContext) {
    let script = PaneMoveScript {
        park_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_edge_view(cx, script);
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    view.read_with(cx, |this, _| assert!(this.tab_relocation_locked("t-a")));
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 1);
    view.read_with(cx, |this, _| {
        assert!(this.pane_relocations.is_empty(), "reverted");
        assert_eq!(this.selection.pane_id.as_deref(), Some("p-left"));
    });
}

#[gpui::test]
fn a_foreign_layout_during_an_insert_reverts_to_authority(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 2);
    // Someone split the tab before our move landed.
    let mut layout = two_pane_layout("p-left", "p-right");
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
    send_events(&fake, vec![layout_event(layout)], cx);
    view.read_with(cx, |this, _| {
        assert!(
            this.pane_relocations.is_empty(),
            "fingerprint mismatch aborts"
        );
        assert!(!this.tab_relocation_locked("t-a"));
    });
}

#[gpui::test]
fn edge_zones_need_both_the_flag_and_the_capability(cx: &mut TestAppContext) {
    // Flag off (default) on a capable Herdr.
    let (fake, view, cx) = connect_two_pane_view(cx);
    view.read_with(cx, |this, _| {
        assert!(this.pane_move_supported());
        assert!(!this.edge_drops_enabled());
    });
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| assert!(this.pane_relocations.is_empty()));
    assert!(fake.requests_for("pane.move").is_empty());
    drop(fake);

    // Flag on, Herdr too old for `pane.move`.
    let mut snapshot = two_pane_snapshot();
    snapshot.version = "0.6.0".into();
    snapshot.protocol = 13;
    let fake = FakeHerdr::snapshot_with_live_events(snapshot);
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| {
        this.headless_terminals = true;
        this.pane_edge_relocation = true;
    });
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update(cx, |this, _| this.terminal_surface_bounds = Some(SURFACE));
    view.read_with(cx, |this, _| {
        assert!(!this.pane_move_supported());
        assert!(!this.edge_drops_enabled());
    });
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(this.pane_relocations.is_empty());
        assert!(this.pane_drag_return.is_some(), "the preview returned home");
    });
    assert!(fake.requests_for("pane.move").is_empty());
}

fn key(name: &str, control: bool) -> gpui::KeyDownEvent {
    gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control,
                ..Default::default()
            },
            key: name.into(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    }
}

#[gpui::test]
fn keyboard_move_mode_commits_through_the_same_plan_machinery(cx: &mut TestAppContext) {
    let (fake, view, cx) = connect_edge_view(cx, PaneMoveScript::default());
    view.update_in(cx, |this, window, cx| {
        assert!(this.handle_app_shortcut(&key("b", true), window, cx));
        assert!(this.prefix_pending);
        assert!(this.handle_app_shortcut(&key("m", false), window, cx));
        let mode = this.pane_keyboard_move.as_ref().expect("move mode");
        assert_eq!(mode.pane_id, "p-left");
        assert!(mode.target.is_none());
        // Right arrow picks the neighbour, centre zone.
        assert!(this.handle_app_shortcut(&key("right", false), window, cx));
        let mode = this.pane_keyboard_move.as_ref().expect("move mode");
        let target = mode.target.as_ref().expect("target");
        assert_eq!(target.target_pane_id, "p-right");
        assert_eq!(target.zone, ocherdr_core::DropZone::Center);
        // Tab cycles to the left edge (flag + capability are on).
        assert!(this.handle_app_shortcut(&key("tab", false), window, cx));
        let zone = this
            .pane_keyboard_move
            .as_ref()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .zone;
        assert_eq!(zone, ocherdr_core::DropZone::Left);
        // Esc cancels without a request.
        assert!(this.handle_app_shortcut(&key("escape", false), window, cx));
        assert!(this.pane_keyboard_move.is_none());
        assert!(this.pane_relocations.is_empty());
        // Again, and confirm a left insert.
        assert!(this.handle_app_shortcut(&key("b", true), window, cx));
        assert!(this.handle_app_shortcut(&key("m", false), window, cx));
        assert!(this.handle_app_shortcut(&key("right", false), window, cx));
        assert!(this.handle_app_shortcut(&key("tab", false), window, cx));
        assert!(this.handle_app_shortcut(&key("enter", false), window, cx));
        assert!(this.pane_keyboard_move.is_none());
        let pending = this
            .pane_relocations
            .get("t-a")
            .expect("plan from the keyboard");
        assert!(matches!(
            pending.plan.intent,
            crate::RelocationIntent::Insert {
                edge: ocherdr_core::DropEdge::Left,
                ..
            }
        ));
    });
    cx.run_until_parked();
    assert_eq!(fake.requests_for("pane.move").len(), 2);
    assert_eq!(fake.requests_for("pane.swap").len(), 1);
}

#[gpui::test]
fn a_disconnect_while_parked_restores_the_notice_from_the_reconnect_snapshot(
    cx: &mut TestAppContext,
) {
    let script = PaneMoveScript {
        insert_failures: AtomicUsize::new(1),
        ..PaneMoveScript::default()
    };
    let (fake, view, cx) = connect_edge_view(cx, script);
    drag_left_pane_to(&view, RIGHT_EDGE, cx);
    cx.run_until_parked();
    send_events(&fake, park_events(), cx);
    view.update(cx, |this, cx| {
        assert!(this.parked_relocation("t-a").is_some());
        // The connection drops: local state is abandoned, the parked pane
        // is remembered.
        this.abort_pane_relocations_for_disconnect();
        assert!(this.pane_relocations.is_empty());
        assert!(this.parked_recovery.is_some());
        // Reconnect snapshot still shows the pane in the temporary tab.
        this.restore_parked_recovery(cx);
        let pending = this.parked_relocation("t-a").expect("parked notice back");
        assert_eq!(
            pending.phase,
            crate::RelocationPhase::Parked {
                temp_tab_id: "t-tmp".into(),
                moved_pane_id: "p-left".into(),
            }
        );
        assert!(this.parked_recovery.is_none());
        // A snapshot where the pane already left the temp tab offers nothing.
        this.abort_pane_relocations_for_disconnect();
        this.snapshot = Some(two_pane_snapshot());
        this.restore_parked_recovery(cx);
        assert!(this.pane_relocations.is_empty());
    });
}
