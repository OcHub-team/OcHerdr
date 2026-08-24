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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualTestContext, prelude::*};
use ocherdr_core::{
    AgentInfo, AgentStatus, ConnectionProfile, HierarchySnapshot, PaneInfo, Selection,
    SessionSummary, TabInfo, WorkspaceInfo,
};
use ocherdr_herdr::{HostHealthStatus, SessionConnection};
use serde_json::{Value, json};

use crate::host_center::HostCenter;
use crate::{
    AgentOutputState, AgentPromptPhase, AppearanceSettings, CachedHostHealth, EventStreamState,
    HostHealthView, I18n, Language, OcHerdrView, Settings, install_appearance,
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
