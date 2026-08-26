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
    AgentInfo, AgentStatus, ConnectionProfile, HerdrEvent, HierarchySnapshot, PaneInfo,
    ReorderHover, Selection, SessionSummary, TabInfo, WorkspaceInfo,
};
use ocherdr_herdr::{HostHealthStatus, SessionConnection, TerminalMode};
use serde_json::{Value, json};

use super::split_drag_from_press;
use crate::host_center::HostCenter;
use crate::{
    AgentOutputState, AgentPromptPhase, AppearanceSettings, CachedHostHealth, EventStreamState,
    HEADER_HEIGHT, HostHealthView, I18n, Language, OcHerdrView, PendingListReorder, ReorderList,
    Settings, TAB_PILL_WIDTH, TAB_PREVIEW_DELAY, TAB_PREVIEW_GAP, TAB_PREVIEW_HEIGHT,
    TAB_PREVIEW_WIDTH, TAB_STRIP_LEAD_INSET, install_appearance, reorder_projection,
};

use creation::{
    created_pane, created_tab, created_workspace, created_workspace_pane, created_workspace_tab,
};
use relocation::{layout_event, parked_pane_json, send_events, single_pane_layout, temp_tab};

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
    /// Same race for `tab.create` / `workspace.create`: Herdr broadcasts
    /// `tab.created` / `pane.created` before it answers the request.
    events_before_create_response: Mutex<Vec<Value>>,
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
        // `terminal session control|observe <pane> …` streams control
        // commands on stdin: log them per pane so tests can read what the
        // view sent, and hold the stream open the way Herdr would.
        let log_dir = dir.path().display();
        std::fs::write(
            &herdr_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"session\" ] && [ \"$2\" = \"list\" ]; then\ncat <<'EOF'\n{sessions}\nEOF\nexit 0\nfi\nwhile [ $# -gt 0 ] && [ \"$1\" != \"terminal\" ]; do shift; done\nif [ \"$1\" = \"terminal\" ]; then\ncat >> \"{log_dir}/terminal-$4.jsonl\"\nexit 0\nfi\necho \"unexpected: $*\" >&2\nexit 1\n"
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

    /// Base64 payloads of every `terminal.input` the view wrote to the
    /// pane's control stream, in order.
    fn terminal_inputs(&self, pane_id: &str) -> Vec<String> {
        let path = self._dir.path().join(format!("terminal-{pane_id}.jsonl"));
        let Ok(log) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        log.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|command| command["type"] == json!("terminal.input"))
            .filter_map(|command| command["bytes"].as_str().map(str::to_owned))
            .collect()
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
        Some("agent.focus") => json!({
            "id": id,
            "result": { "type": "ok" },
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
    // Grows with what the fake creates, the way a real Herdr's snapshot
    // would: the resync that follows an agent-status resubscribe must not
    // erase a tab this fake just answered `tab.create` with.
    let mut snapshot = snapshot;
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
                    Some("tab.create") | Some("workspace.create") => {
                        let early = std::mem::take(
                            &mut *script
                                .events_before_create_response
                                .lock()
                                .expect("early create events"),
                        );
                        if !early.is_empty() {
                            script.broadcast(early);
                            thread::sleep(Duration::from_millis(60));
                        }
                        let result = if request["method"] == json!("tab.create") {
                            snapshot.tabs.push(created_tab());
                            snapshot.panes.push(created_pane());
                            json!({
                                "type": "tab_created",
                                "tab": created_tab(),
                                "root_pane": created_pane(),
                            })
                        } else {
                            snapshot.workspaces.push(created_workspace());
                            snapshot.tabs.push(created_workspace_tab());
                            snapshot.panes.push(created_workspace_pane());
                            json!({
                                "type": "workspace_created",
                                "workspace": created_workspace(),
                                "tab": created_workspace_tab(),
                                "root_pane": created_workspace_pane(),
                            })
                        };
                        write_fake_response(stream, json!({ "id": id, "result": result }))
                    }
                    Some("agent.focus") => {
                        write_fake_response(stream, json!({ "id": id, "result": { "type": "ok" } }))
                    }
                    Some("tab.rename") => {
                        let tab_id = request["params"]["tab_id"].as_str().unwrap_or_default();
                        let label = request["params"]["label"].as_str().unwrap_or_default();
                        let Some(tab) = snapshot.tabs.iter_mut().find(|tab| tab.tab_id == tab_id)
                        else {
                            write_fake_response(
                                stream,
                                json!({
                                    "id": id,
                                    "error": {
                                        "code": "tab_not_found",
                                        "message": format!("tab {tab_id} not found"),
                                    },
                                }),
                            );
                            continue;
                        };
                        tab.label = label.to_owned();
                        write_fake_response(
                            stream,
                            json!({
                                "id": id,
                                "result": { "type": "tab_info", "tab": tab },
                            }),
                        );
                    }
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

fn agent_row_center(cx: &mut VisualTestContext) -> gpui::Point<gpui::Pixels> {
    cx.debug_bounds("agent-p1")
        .expect("agent row should be in the rendered tree")
        .center()
}

fn click_agent_row(cx: &mut VisualTestContext) {
    let center = agent_row_center(cx);
    cx.simulate_click(center, gpui::Modifiers::default());
    cx.run_until_parked();
}

/// Double-click on the row: the second press/release carries `click_count: 2`.
fn double_click_agent_row(cx: &mut VisualTestContext) {
    let center = agent_row_center(cx);
    cx.simulate_click(center, gpui::Modifiers::default());
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: center,
        modifiers: Default::default(),
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position: center,
        modifiers: Default::default(),
        click_count: 2,
    });
    cx.run_until_parked();
}

/// Open the agent panel the way the sidebar row does now: the context
/// menu's "Details" entry.
fn open_agent_row_details(cx: &mut VisualTestContext) {
    let center = agent_row_center(cx);
    cx.simulate_mouse_down(center, gpui::MouseButton::Right, gpui::Modifiers::default());
    cx.run_until_parked();
    let details = cx
        .debug_bounds("agent-menu-details")
        .expect("the agent row's context menu leads with Details");
    cx.simulate_click(details.center(), gpui::Modifiers::default());
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

mod shell;
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

mod creation;
mod pane;
mod relocation;
