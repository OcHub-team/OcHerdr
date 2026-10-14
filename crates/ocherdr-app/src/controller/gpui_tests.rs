//! GPUI `TestAppContext` harness for controller wiring that unit tests cannot reach.
//!
//! These tests drive production `OcHerdrView` / `HostCenter` methods through the
//! same `Context<T>` / `Window` / `Entity<T>` path the GUI uses. They do not
//! introduce a second status type or a test-only controller.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext, prelude::*};
use ocherdr_core::{
    AgentInfo, AgentStatus, ConnectionProfile, HierarchySnapshot, PaneInfo, ReorderHover,
    Selection, SessionSummary, TabInfo, WorkspaceInfo,
};
use ocherdr_herdr::endpoint::EndpointEvent;
use ocherdr_herdr::endpoint_v1 as ep;
use ocherdr_herdr::{HostHealthStatus, SessionConnection, TerminalMode};
use serde_json::{Value, json};

use super::split_drag_from_press;
use crate::host_center::{HostCenter, HostCenterEvent, HostSaveThen};
use crate::{
    AgentOutputState, AgentPromptPhase, AppearanceSettings, CachedHostHealth, EventStreamState,
    HEADER_HEIGHT, HostHealthView, HostMetadata, I18n, Language, OcHerdrView, PendingListReorder,
    ReorderList, Settings, TAB_PILL_WIDTH, TAB_PREVIEW_DELAY, TAB_PREVIEW_GAP, TAB_PREVIEW_HEIGHT,
    TAB_PREVIEW_WIDTH, install_appearance, reorder_projection,
};

use creation::{
    created_pane, created_tab, created_workspace, created_workspace_pane, created_workspace_tab,
};
use relocation::{parked_pane_json, single_pane_layout, temp_tab};

fn install_app(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_app_identity("io.github.ochub-team.ocherdr", "OcHerdr");
        ochub_ui::install(cx);
        I18n::install(Language::English);
        install_appearance(&AppearanceSettings::default(), cx.window_appearance());
    });
}

fn app_primary_modifiers() -> gpui::Modifiers {
    let mut modifiers = gpui::Modifiers::default();
    #[cfg(target_os = "macos")]
    {
        modifiers.platform = true;
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control = true;
    }
    modifiers
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
    cx.update(|_window, cx| crate::install_system_notification_handler(&view, cx));
    (view, cx)
}

/// A fake Herdr session host: the NDJSON public API on `herdr.sock` (one
/// request per connection, answered on its own thread so a scripted slow
/// response never blocks a `session.snapshot` pull) plus the endpoint
/// generation-1 shell protocol on `herdr-client.sock` (hello → welcome →
/// initial snapshot push, then request/response lanes, semantic input
/// capture, and server-pushed snapshot/notification frames).
type EndpointPushers = Arc<Mutex<Vec<(u64, mpsc::Sender<ep::ServerMessage>)>>>;
type ClipboardImages = Arc<Mutex<Vec<(String, String, Vec<u8>)>>>;
type SnapshotMutation = Box<dyn FnOnce(&mut HierarchySnapshot) + Send>;

struct FakeHerdr {
    herdr_path: PathBuf,
    /// Authoritative hierarchy the NDJSON `session.snapshot` serves. Tests
    /// and scripted request handlers mutate it, then push a snapshot
    /// notification so the client resyncs the way it would against a live
    /// Herdr.
    state: Arc<Mutex<HierarchySnapshot>>,
    /// Revision counter stamped onto every pushed snapshot envelope.
    revision: Arc<AtomicU64>,
    /// Live endpoint subscriber channels: one per connected shell client.
    pushers: EndpointPushers,
    /// Every NDJSON request the fake answered, in arrival order.
    requests: Arc<Mutex<Vec<Value>>>,
    /// Endpoint-lane request payloads (`{id, method, params}`), in order.
    endpoint_requests: Arc<Mutex<Vec<Value>>>,
    /// Semantic input the endpoint received, per pane, in arrival order.
    #[cfg(target_os = "macos")]
    pane_inputs: Arc<Mutex<HashMap<String, Vec<ep::ClientPaneInputEvent>>>>,
    /// (pane, extension, bytes) of every clipboard image the endpoint got.
    #[cfg(target_os = "macos")]
    clipboard_images: ClipboardImages,
    endpoint_connections: Arc<AtomicUsize>,
    endpoint_disconnects: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
    endpoint_server: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

/// Scripted `pane.move` / create behaviour: how many of the next `new_tab`
/// (step 1) and `tab` (step 2) requests the fake rejects, and which state
/// mutation lands (and is pushed to endpoint subscribers) before the
/// matching response.
#[derive(Default)]
struct PaneMoveScript {
    park_failures: AtomicUsize,
    insert_failures: AtomicUsize,
    /// Mutation applied and pushed *before* the fake writes the next step-1
    /// response: a real Herdr's snapshot push rides the endpoint connection,
    /// so it can beat the response that names the temp tab.
    push_before_park_response: Mutex<Option<SnapshotMutation>>,
    /// Same race for `pane.move { destination: tab }`.
    push_before_insert_response: Mutex<Option<SnapshotMutation>>,
    /// Same race for `tab.create` / `workspace.create`: the handler mutates
    /// the state itself, this flag only decides whether the push lands
    /// before the response.
    push_before_create_response: AtomicBool,
}

impl PaneMoveScript {
    fn take_failure(counter: &AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_ok()
    }
}

impl FakeHerdr {
    /// A pre-0.9 Herdr: the NDJSON snapshot works, but the endpoint rejects
    /// the generation-1 hello with a handshake error.
    fn snapshot_ok_endpoint_rejected() -> Self {
        Self::start(
            HierarchySnapshot::default(),
            PaneMoveScript::default(),
            true,
            true,
        )
    }

    fn serving(snapshot: HierarchySnapshot) -> Self {
        Self::serving_with_script(snapshot, PaneMoveScript::default())
    }

    fn serving_with_script(snapshot: HierarchySnapshot, script: PaneMoveScript) -> Self {
        Self::start(snapshot, script, false, false)
    }

    /// `endpoint_error`: the endpoint welcome carries a handshake rejection.
    /// `strict_api`: the NDJSON socket serves `session.snapshot` only and
    /// rejects every other method (a Herdr too old for the request).
    fn start(
        snapshot: HierarchySnapshot,
        script: PaneMoveScript,
        endpoint_error: bool,
        strict_api: bool,
    ) -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let socket_path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake herdr socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking fake herdr socket");
        let endpoint_socket_path = dir.path().join("herdr-client.sock");
        let endpoint_listener =
            UnixListener::bind(&endpoint_socket_path).expect("bind fake endpoint socket");
        endpoint_listener
            .set_nonblocking(true)
            .expect("nonblocking fake endpoint socket");

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
        let state = Arc::new(Mutex::new(snapshot));
        let revision = Arc::new(AtomicU64::new(1));
        let pushers: EndpointPushers = Arc::new(Mutex::new(Vec::new()));
        let requests: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let endpoint_requests: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let pane_inputs: Arc<Mutex<HashMap<String, Vec<ep::ClientPaneInputEvent>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let clipboard_images: ClipboardImages = Arc::new(Mutex::new(Vec::new()));
        let endpoint_connections = Arc::new(AtomicUsize::new(0));
        let endpoint_disconnects = Arc::new(AtomicUsize::new(0));
        let script = Arc::new(script);

        let server = thread::spawn({
            let (stop, state, revision, pushers, requests, script) = (
                stop.clone(),
                state.clone(),
                revision.clone(),
                pushers.clone(),
                requests.clone(),
                script.clone(),
            );
            move || {
                serve_fake_herdr_api(
                    listener, stop, state, revision, requests, pushers, script, strict_api,
                );
            }
        });
        let endpoint_server = thread::spawn({
            let (stop, revision, pushers, endpoint_requests, pane_inputs) = (
                stop.clone(),
                revision.clone(),
                pushers.clone(),
                endpoint_requests.clone(),
                pane_inputs.clone(),
            );
            let (clipboard_images, endpoint_connections, endpoint_disconnects) = (
                clipboard_images.clone(),
                endpoint_connections.clone(),
                endpoint_disconnects.clone(),
            );
            move || {
                serve_fake_endpoint(
                    endpoint_listener,
                    endpoint_error,
                    revision,
                    pushers,
                    endpoint_requests,
                    pane_inputs,
                    clipboard_images,
                    endpoint_connections,
                    endpoint_disconnects,
                    stop,
                );
            }
        });
        Self {
            herdr_path,
            state,
            revision,
            pushers,
            requests,
            endpoint_requests,
            #[cfg(target_os = "macos")]
            pane_inputs,
            #[cfg(target_os = "macos")]
            clipboard_images,
            endpoint_connections,
            endpoint_disconnects,
            stop,
            server: Some(server),
            endpoint_server: Some(endpoint_server),
            _dir: dir,
        }
    }

    /// Methods of every NDJSON request in arrival order.
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

    /// Endpoint-lane request payloads for one method, in arrival order.
    #[allow(dead_code)]
    fn endpoint_requests_for(&self, method: &str) -> Vec<Value> {
        self.endpoint_requests
            .lock()
            .expect("fake herdr endpoint request log")
            .iter()
            .filter(|request| request.get("method") == Some(&json!(method)))
            .cloned()
            .collect()
    }

    /// Semantic input events the endpoint received for one pane, in order.
    #[cfg(target_os = "macos")]
    fn pane_inputs(&self, pane_id: &str) -> Vec<ep::ClientPaneInputEvent> {
        self.pane_inputs
            .lock()
            .expect("fake herdr pane input log")
            .get(pane_id)
            .cloned()
            .unwrap_or_default()
    }

    /// (pane, extension, bytes) of every bridged clipboard image, in order.
    #[cfg(target_os = "macos")]
    fn clipboard_images(&self) -> Vec<(String, String, Vec<u8>)> {
        self.clipboard_images
            .lock()
            .expect("fake herdr clipboard image log")
            .clone()
    }

    fn endpoint_connections(&self) -> usize {
        self.endpoint_connections.load(Ordering::Relaxed)
    }

    fn endpoint_disconnects(&self) -> usize {
        self.endpoint_disconnects.load(Ordering::Relaxed)
    }

    fn socket_path(&self) -> PathBuf {
        self._dir.path().join("herdr.sock")
    }

    /// The endpoint snapshot envelope every push carries. The payload only
    /// needs to parse: the client treats it as an invalidation cue and pulls
    /// the authoritative hierarchy over NDJSON.
    fn snapshot_push(revision: u64) -> ep::ServerMessage {
        ep::ServerMessage::EndpointControl {
            kind: ep::ENDPOINT_SNAPSHOT_KIND.into(),
            data: serde_json::to_string(&json!({
                "boot_id": "boot-1",
                "revision": revision,
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
            .expect("snapshot push json"),
        }
    }

    /// Apply `mutate` to the served hierarchy, then push a snapshot envelope
    /// to every connected endpoint — exactly how a live Herdr broadcasts a
    /// change.
    fn mutate_and_push(&self, mutate: impl FnOnce(&mut HierarchySnapshot)) {
        {
            let mut state = self.state.lock().expect("fake herdr state");
            mutate(&mut state);
        }
        let revision = self.revision.fetch_add(1, Ordering::Relaxed) + 1;
        let message = Self::snapshot_push(revision);
        for (_, pusher) in self.pushers.lock().expect("fake herdr pushers").iter() {
            let _ = pusher.send(message.clone());
        }
    }
}

/// Mutate the served hierarchy and push the new revision to every endpoint
/// subscriber before answering the request.
fn push_state_before_response(
    state: &Mutex<HierarchySnapshot>,
    revision: &AtomicU64,
    pushers: &Mutex<Vec<(u64, mpsc::Sender<ep::ServerMessage>)>>,
    mutate: Option<SnapshotMutation>,
) {
    {
        let mut state = state.lock().expect("fake herdr state");
        if let Some(mutate) = mutate {
            mutate(&mut state);
        }
    }
    let revision = revision.fetch_add(1, Ordering::Relaxed) + 1;
    let message = FakeHerdr::snapshot_push(revision);
    for (_, pusher) in pushers.lock().expect("fake herdr pushers").iter() {
        let _ = pusher.send(message.clone());
    }
}

#[allow(clippy::too_many_arguments)]
fn serve_fake_herdr_api(
    listener: UnixListener,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<HierarchySnapshot>>,
    revision: Arc<AtomicU64>,
    requests: Arc<Mutex<Vec<Value>>>,
    pushers: EndpointPushers,
    script: Arc<PaneMoveScript>,
    strict_api: bool,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let (state, revision, requests, pushers, script) = (
                    state.clone(),
                    revision.clone(),
                    requests.clone(),
                    pushers.clone(),
                    script.clone(),
                );
                // One request per connection, each on its own thread: a
                // scripted sleep before a `pane.move` response must not stall
                // the `session.snapshot` pull its own push just triggered.
                thread::spawn(move || {
                    reply_to_fake_api_request(
                        stream, &state, &revision, &requests, &pushers, &script, strict_api,
                    )
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn reply_to_fake_api_request(
    stream: UnixStream,
    state: &Mutex<HierarchySnapshot>,
    revision: &AtomicU64,
    requests: &Mutex<Vec<Value>>,
    pushers: &Mutex<Vec<(u64, mpsc::Sender<ep::ServerMessage>)>>,
    script: &PaneMoveScript,
    strict_api: bool,
) {
    let _ = stream.set_nonblocking(false);
    let mut reader = BufReader::new(stream.try_clone().expect("clone fake stream"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    let request: Value = serde_json::from_str(&line).expect("parse fake request");
    requests
        .lock()
        .expect("fake herdr request log")
        .push(request.clone());
    let id = request.get("id").cloned().unwrap_or(json!(""));
    if strict_api {
        let response = match request.get("method").and_then(Value::as_str) {
            Some("session.snapshot") => json!({
                "id": id,
                "result": { "snapshot": state.lock().expect("fake herdr state").clone() },
            }),
            other => json!({
                "id": id,
                "error": {
                    "code": "unexpected",
                    "message": format!("unexpected method {other:?}"),
                },
            }),
        };
        write_fake_response(stream, response);
        return;
    }
    match request.get("method").and_then(Value::as_str) {
        Some("session.snapshot") => {
            let snapshot = state.lock().expect("fake herdr state").clone();
            write_fake_response(
                stream,
                json!({
                    "id": id,
                    "result": { "snapshot": snapshot },
                }),
            )
        }
        Some("tab.create") | Some("workspace.create") => {
            let result = {
                let mut state = state.lock().expect("fake herdr state");
                if request["method"] == json!("tab.create") {
                    state.tabs.push(created_tab());
                    state.panes.push(created_pane());
                    json!({
                        "type": "tab_created",
                        "tab": created_tab(),
                        "root_pane": created_pane(),
                    })
                } else {
                    state.workspaces.push(created_workspace());
                    state.tabs.push(created_workspace_tab());
                    state.panes.push(created_workspace_pane());
                    json!({
                        "type": "workspace_created",
                        "workspace": created_workspace(),
                        "tab": created_workspace_tab(),
                        "root_pane": created_workspace_pane(),
                    })
                }
            };
            if script
                .push_before_create_response
                .swap(false, Ordering::Relaxed)
            {
                push_state_before_response(state, revision, pushers, None);
            }
            write_fake_response(stream, json!({ "id": id, "result": result }))
        }
        Some("agent.focus") => {
            write_fake_response(stream, json!({ "id": id, "result": { "type": "ok" } }))
        }
        Some("tab.rename") => {
            let tab_id = request["params"]["tab_id"].as_str().unwrap_or_default();
            let label = request["params"]["label"].as_str().unwrap_or_default();
            let mut state = state.lock().expect("fake herdr state");
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.tab_id == tab_id) else {
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
                return;
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
        // A swap against `reject` plays a Herdr that refuses (for example a
        // zoomed tab); anything else is accepted and the test pushes the
        // matching layout change itself.
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
        Some("pane.focus") => write_fake_response(
            stream,
            json!({
                "id": id,
                "result": {
                    "type": "pane_info",
                    "pane": { "pane_id": request["params"]["pane_id"] },
                },
            }),
        ),
        // The fixture plays an old Herdr when asked to move into
        // `unsupported`: the request enum fails to deserialize.
        Some("pane.move") if request["params"]["target_tab_id"] == json!("unsupported") => {
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
        // Step 1 of an edge relocation: park in a new tab. Mirrors Herdr's
        // `PaneMoveResult` shape (`created_tab`, `pane`).
        Some("pane.move") if request["params"]["destination"]["type"] == json!("new_tab") => {
            if PaneMoveScript::take_failure(&script.park_failures) {
                write_fake_response(
                    stream,
                    json!({
                        "id": id,
                        "error": { "code": "pane_move_failed", "message": "park refused" },
                    }),
                );
                return;
            }
            let early = script
                .push_before_park_response
                .lock()
                .expect("early park push")
                .take();
            if early.is_some() {
                push_state_before_response(state, revision, pushers, early);
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
        Some("pane.move") if request["params"]["destination"]["type"] == json!("tab") => {
            if PaneMoveScript::take_failure(&script.insert_failures) {
                write_fake_response(
                    stream,
                    json!({
                        "id": id,
                        "error": { "code": "pane_move_failed", "message": "target pane could not be split" },
                    }),
                );
                return;
            }
            let early = script
                .push_before_insert_response
                .lock()
                .expect("early insert push")
                .take();
            if early.is_some() {
                push_state_before_response(state, revision, pushers, early);
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

/// The endpoint accept loop: every connected shell client gets its own
/// connection thread.
#[allow(clippy::too_many_arguments)]
fn serve_fake_endpoint(
    listener: UnixListener,
    endpoint_error: bool,
    revision: Arc<AtomicU64>,
    pushers: EndpointPushers,
    endpoint_requests: Arc<Mutex<Vec<Value>>>,
    pane_inputs: Arc<Mutex<HashMap<String, Vec<ep::ClientPaneInputEvent>>>>,
    clipboard_images: ClipboardImages,
    connections: Arc<AtomicUsize>,
    disconnects: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
) {
    let mut next_id = 0u64;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                next_id += 1;
                let (revision, pushers, endpoint_requests, pane_inputs) = (
                    revision.clone(),
                    pushers.clone(),
                    endpoint_requests.clone(),
                    pane_inputs.clone(),
                );
                let (clipboard_images, connections, disconnects) = (
                    clipboard_images.clone(),
                    connections.clone(),
                    disconnects.clone(),
                );
                thread::spawn(move || {
                    handle_endpoint_connection(
                        stream,
                        next_id,
                        endpoint_error,
                        &revision,
                        &pushers,
                        &endpoint_requests,
                        &pane_inputs,
                        &clipboard_images,
                        &connections,
                        &disconnects,
                    )
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
}

/// One endpoint connection: handshake, initial snapshot push, then a reader
/// loop for the request/input lanes while a writer thread drains pushed
/// server messages onto the same socket.
#[allow(clippy::too_many_arguments)]
fn handle_endpoint_connection(
    stream: UnixStream,
    connection_id: u64,
    endpoint_error: bool,
    revision: &AtomicU64,
    pushers: &Mutex<Vec<(u64, mpsc::Sender<ep::ServerMessage>)>>,
    endpoint_requests: &Mutex<Vec<Value>>,
    pane_inputs: &Mutex<HashMap<String, Vec<ep::ClientPaneInputEvent>>>,
    clipboard_images: &Mutex<Vec<(String, String, Vec<u8>)>>,
    connections: &AtomicUsize,
    disconnects: &AtomicUsize,
) {
    connections.fetch_add(1, Ordering::Relaxed);
    // Blocking reads: `read_message` consumes bytes as it goes, so a timed
    // read that fires mid-frame would misalign every following frame. The
    // connection worker retires on the client's Detach/EOF instead of a
    // polling timeout. macOS accepts inherit the listener's nonblocking flag,
    // so the mode has to be reset explicitly.
    let _ = stream.set_nonblocking(false);
    let mut reader = stream.try_clone().expect("clone endpoint stream");
    let mut writer = stream;
    let hello = match ep::read_message::<_, ep::ClientMessage>(&mut reader, ep::MAX_FRAME_SIZE) {
        Ok(message) => message,
        Err(_) => {
            disconnects.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    let ep::ClientMessage::EndpointControl { kind, .. } = &hello else {
        disconnects.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if kind != ep::ENDPOINT_HELLO_KIND {
        disconnects.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let welcome = ep::EndpointServerWelcome {
        generation: ep::ENDPOINT_PROTOCOL_GENERATION,
        server_version: "herdr 0.9.1".into(),
        snapshot_codec: ep::ENDPOINT_SNAPSHOT_KIND.into(),
        surface_codec: ep::SURFACE_CODEC_V1.into(),
        input_codec: ep::INPUT_CODEC_V1.into(),
        blob_codec: ep::BLOB_CODEC_V1.into(),
        methods: vec![
            "client_shell.surface.set".into(),
            "workspace.focus".into(),
            "tab.focus".into(),
            "pane.focus".into(),
        ],
        capabilities: Vec::new(),
        error: endpoint_error.then(|| ep::EndpointHandshakeError {
            code: "unsupported_generation".into(),
            message: "endpoint generation 1 is not supported by this herdr".into(),
        }),
    };
    if ep::write_message(
        &mut writer,
        &ep::ServerMessage::EndpointControl {
            kind: ep::ENDPOINT_WELCOME_KIND.into(),
            data: serde_json::to_string(&welcome).expect("welcome json"),
        },
    )
    .is_err()
    {
        disconnects.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if welcome.error.is_some() {
        // A rejected hello is terminal for the client: it hangs up. Wait for
        // that so the disconnect accounting stays exact.
        while ep::read_message::<_, ep::ClientMessage>(&mut reader, ep::MAX_FRAME_SIZE).is_ok() {}
        disconnects.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if ep::write_message(
        &mut writer,
        &FakeHerdr::snapshot_push(revision.load(Ordering::Relaxed)),
    )
    .is_err()
    {
        disconnects.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let (tx, rx) = mpsc::channel::<ep::ServerMessage>();
    let responses = tx.clone();
    pushers
        .lock()
        .expect("fake herdr pushers")
        .push((connection_id, tx));
    thread::spawn(move || {
        while let Ok(message) = rx.recv() {
            if ep::write_message(&mut writer, &message).is_err() {
                break;
            }
        }
    });

    loop {
        let message = match ep::read_message::<_, ep::ClientMessage>(
            &mut reader,
            ep::MAX_GRAPHICS_FRAME_SIZE,
        ) {
            Ok(message) => message,
            Err(ep::FramingError::UnexpectedEof) => break,
            Err(error) => {
                eprintln!("fake endpoint post-handshake read error: {error}");
                break;
            }
        };
        match message {
            ep::ClientMessage::ClientShellEndpointRequest { boot_id, request } => {
                let request: Value = serde_json::from_str(&request).expect("endpoint request json");
                endpoint_requests
                    .lock()
                    .expect("fake herdr endpoint request log")
                    .push(request.clone());
                let request_id = request
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let _ = responses.send(ep::ServerMessage::ClientShellEndpointResponseChunk {
                    boot_id,
                    request_id,
                    final_chunk: true,
                    data: serde_json::to_vec(&json!({ "type": "ok" })).expect("lane response"),
                });
            }
            ep::ClientMessage::ClientShellPaneInput { pane_id, events } => {
                pane_inputs
                    .lock()
                    .expect("fake herdr pane input log")
                    .entry(pane_id)
                    .or_default()
                    .extend(events);
            }
            ep::ClientMessage::ClipboardImage {
                target: ep::ClientClipboardImageTarget::Pane(pane_id),
                extension,
                data,
            } => {
                clipboard_images
                    .lock()
                    .expect("fake herdr clipboard image log")
                    .push((pane_id, extension, data));
            }
            ep::ClientMessage::Detach => break,
            _ => {}
        }
    }
    pushers
        .lock()
        .expect("fake herdr pushers")
        .retain(|(id, _)| *id != connection_id);
    drop(responses);
    disconnects.fetch_add(1, Ordering::Relaxed);
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.pushers.lock().expect("fake herdr pushers").clear();
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        if let Some(server) = self.endpoint_server.take() {
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
            proxy_jump: None,
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
mod tab_transfer;
fn pane_move_capable_snapshot() -> HierarchySnapshot {
    HierarchySnapshot {
        version: "0.7.0".into(),
        protocol: 14,
        ..three_tab_snapshot()
    }
}

/// Wire the view to the fake socket directly, spawn the endpoint session the
/// way `reload` does, and pull the snapshot through `resync_snapshot` — the
/// same path every live refresh takes. This skips `reload`'s `herdr session
/// list` process spawn, which is the one step in that path that can fail
/// under fork pressure and would otherwise leave the capability assertions
/// racing the host's load rather than the code.
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
        let owner = this.current_session_key().expect("connected session key");
        if super::support::session_panes_plan(
            this.session_panes.as_ref().map(|session| &session.owner),
            &owner,
        ) == super::support::SessionPanesPlan::Replace
        {
            this.session_panes = Some(crate::SessionPanes::new(owner));
        }
        this.spawn_endpoint_session_for_test(cx);
        let epoch = this.event_epoch;
        this.resync_snapshot(epoch, cx);
    });
    // The handshake, the initial snapshot push, and the NDJSON resync all
    // land on executor tasks fed by worker threads: pump the executor and
    // yield real time until the view is Live with an applied snapshot.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let (ready, state_debug) = view.read_with(cx, |this, _| {
            let ready = matches!(this.event_stream, EventStreamState::Live)
                && this.snapshot.is_some()
                && !this.snapshot_refreshing;
            let has_endpoint = this
                .session_panes
                .as_ref()
                .is_some_and(|session| session.endpoint.is_some());
            (
                ready,
                format!(
                    "stream={:?} snapshot={} refreshing={} endpoint_runtime={} fake_connections={} fake_disconnects={}",
                    this.event_stream,
                    this.snapshot.is_some(),
                    this.snapshot_refreshing,
                    has_endpoint,
                    fake.endpoint_connections(),
                    fake.endpoint_disconnects(),
                ),
            )
        });
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the fake endpoint never went live: {state_debug}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    view.read_with(cx, |this, _| {
        assert!(
            this.connection.is_some(),
            "the direct connection stays wired"
        );
    });
}

/// Mutate the fake's hierarchy, push the new revision to the view's endpoint,
/// and wait until the resync it triggers applies a snapshot that satisfies
/// `applied`.
fn push_state(
    fake: &FakeHerdr,
    view: &Entity<OcHerdrView>,
    cx: &mut VisualTestContext,
    mutate: impl FnOnce(&mut HierarchySnapshot),
    applied: impl Fn(&HierarchySnapshot) -> bool,
) {
    fake.mutate_and_push(mutate);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let done = view.read_with(cx, |this, _| {
            !this.snapshot_refreshing && this.snapshot.as_ref().is_some_and(&applied)
        });
        if done {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the pushed snapshot never landed"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn blank_cell() -> ep::CellData {
    ep::CellData {
        symbol: " ".into(),
        fg: 0,
        bg: 0,
        modifier: 0,
        skip: false,
        hyperlink: None,
    }
}

/// A surface pane whose outer and inner rects coincide, with pixel metrics
/// derived from a 9×18 cell so pointer math stays deterministic.
fn surface_pane(
    pane_id: &str,
    rect: (u16, u16, u16, u16),
    focused: bool,
    mouse_reporting: bool,
    sgr_pixel_mouse: bool,
) -> ep::PaneSurfacePane {
    let (x, y, width, height) = rect;
    let rect = ep::SurfaceRect {
        x,
        y,
        width,
        height,
    };
    ep::PaneSurfacePane {
        pane_id: pane_id.into(),
        content_revision: 1,
        rect,
        inner_rect: rect,
        scrollbar_rect: None,
        scroll: None,
        focused,
        mouse_reporting,
        sgr_pixel_mouse,
        alternate_screen_active: false,
        pixel_width: u32::from(width) * 9,
        pixel_height: u32::from(height) * 18,
    }
}

/// A full surface frame sized to cover `panes`, every cell blank.
fn surface_frame(panes: Vec<ep::PaneSurfacePane>) -> Box<ep::PaneSurfaceFrame> {
    let width = panes
        .iter()
        .map(|pane| pane.rect.x + pane.rect.width)
        .max()
        .unwrap_or(1)
        .max(1);
    let height = panes
        .iter()
        .map(|pane| pane.rect.y + pane.rect.height)
        .max()
        .unwrap_or(1)
        .max(1);
    Box::new(ep::PaneSurfaceFrame {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        surface_revision: 1,
        frame: ep::FrameData {
            cells: vec![blank_cell(); usize::from(width) * usize::from(height)],
            width,
            height,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        },
        panes,
        splits: Vec::new(),
        popup: None,
        graphics: ep::SurfaceGraphicsScene::default(),
    })
}

/// Apply the pushed hierarchy before polling the queued API request. GPUI's
/// deterministic executor runs blocking background work on its test thread,
/// so sleeping in a fake response cannot model two concurrent socket lanes.
fn apply_snapshot_before_response(
    fake: &FakeHerdr,
    view: &Entity<OcHerdrView>,
    mutate: impl FnOnce(&mut HierarchySnapshot),
    cx: &mut VisualTestContext,
) {
    fake.mutate_and_push(mutate);
    let snapshot = fake.state.lock().unwrap().clone();
    view.update(cx, |this, cx| this.apply_session_snapshot(snapshot, cx));
}

/// Feed a surface frame through the production batch path, exactly as the
/// endpoint listener would deliver it.
fn push_surface_frame(
    view: &Entity<OcHerdrView>,
    frame: Box<ep::PaneSurfaceFrame>,
    cx: &mut VisualTestContext,
) {
    view.update(cx, |this, cx| {
        let owner = this.current_session_key().expect("connected session key");
        assert!(
            this.apply_endpoint_batch(&owner, vec![Ok(EndpointEvent::Surface(frame))], cx),
            "the endpoint batch applies to the live session"
        );
    });
    cx.run_until_parked();
}

#[gpui::test]
fn invoke_with_response_hands_the_whole_result_to_the_callback(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(pane_move_capable_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);

    let received: Arc<Mutex<Option<Result<Value, String>>>> = Arc::new(Mutex::new(None));
    let sink = received.clone();
    view.update(cx, |this, cx| {
        assert!(
            this.pane_move_supported(),
            "a live connection enables pane moves"
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
fn an_unknown_pane_move_method_reports_the_error(cx: &mut TestAppContext) {
    let fake = FakeHerdr::serving(pane_move_capable_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);

    let received: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let sink = received.clone();
    view.update(cx, |this, cx| {
        assert!(
            this.pane_move_supported(),
            "a live connection enables pane moves"
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
            this.pane_move_supported(),
            "the capability tracks the connection, not one request's outcome"
        );
    });
}

#[gpui::test]
fn pane_move_requires_a_connection(cx: &mut TestAppContext) {
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, _| {
        this.snapshot = Some(three_tab_snapshot());
    });

    view.read_with(cx, |this, _| {
        assert!(this.connection.is_none());
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
    snapshot.protocol = 20;
    snapshot.version = "0.8.2".into();
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
    let fake = FakeHerdr::serving(two_pane_snapshot());
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

mod aggregate;
mod creation;
mod file_panel;
mod pane;
mod pane_tab_drop;
mod relocation;
mod rendering;
