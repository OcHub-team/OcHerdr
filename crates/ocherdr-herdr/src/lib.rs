//! Public Herdr transports used by OcHerdr.
//!
//! This crate intentionally does not know the private bincode client protocol.
//! State travels over Herdr's public NDJSON socket; terminal bytes travel through
//! `herdr terminal session observe/control`.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures::channel::mpsc::{self as futures_mpsc, UnboundedReceiver};
use ocherdr_core::{
    AgentStatus, ConnectionProfile, HerdrEvent, HierarchySnapshot, MINIMUM_HERDR_VERSION,
    SessionSummary,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;
use thiserror::Error;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum HerdrError {
    #[error("Herdr executable was not found: {0}")]
    MissingExecutable(String),
    #[error("SSH connection failed: {0}")]
    Ssh(String),
    #[error("Herdr command failed: {0}")]
    Command(String),
    #[error("Herdr API returned {code}: {message}")]
    Api { code: String, message: String },
    #[error("Herdr returned an incompatible response: {0}")]
    Protocol(String),
    #[error("terminal stream closed: {0}")]
    TerminalClosed(String),
    #[error("Herdr event stream closed: {0}")]
    EventStreamClosed(String),
    #[error("Herdr did not respond within {0:?}")]
    Timeout(Duration),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, HerdrError>;

impl HerdrError {
    pub fn is_event_payload_error(&self) -> bool {
        matches!(self, Self::Json(_) | Self::Protocol(_))
    }
}

/// A user-facing summary of a host probe. The transport owns classification so
/// the app never has to infer state from raw OpenSSH stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostHealthStatus {
    Ready,
    SshOnly,
    UnsupportedHerdr,
    AuthenticationRequired,
    HostKeyRequired,
    Unreachable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostHealthCheck {
    pub status: HostHealthStatus,
    pub detail: String,
    pub herdr_version: Option<String>,
    pub session_count: Option<usize>,
    pub latency_ms: u64,
}

impl HostHealthCheck {
    fn failed(status: HostHealthStatus, detail: impl Into<String>, started: Instant) -> Self {
        Self {
            status,
            detail: detail.into(),
            herdr_version: None,
            session_count: None,
            latency_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SessionList {
    sessions: Vec<SessionSummary>,
}

pub fn discover_sessions(profile: &ConnectionProfile) -> Result<Vec<SessionSummary>> {
    let output = command_output(profile, &["session", "list", "--json"])?;
    let list: SessionList = serde_json::from_slice(&output)?;
    Ok(list.sessions)
}

pub fn herdr_version(profile: &ConnectionProfile) -> Result<String> {
    let output = command_output(profile, &["--version"])?;
    Ok(String::from_utf8_lossy(&output).trim().to_owned())
}

/// Check the layers OcHerdr needs in order: SSH, a compatible Herdr binary,
/// and session discovery. This deliberately uses BatchMode and the same
/// bounded OpenSSH settings as normal background work.
pub fn check_host(profile: &ConnectionProfile) -> HostHealthCheck {
    let started = Instant::now();
    if matches!(profile, ConnectionProfile::Ssh { .. })
        && let Err(error) = probe_ssh(profile)
    {
        let detail = error.to_string();
        return HostHealthCheck::failed(classify_ssh_failure(&detail), detail, started);
    }

    let version = match herdr_version(profile) {
        Ok(version) => version,
        Err(error) => {
            let detail = error.to_string();
            let status = if matches!(profile, ConnectionProfile::Ssh { .. })
                && looks_like_missing_herdr(&detail)
            {
                HostHealthStatus::SshOnly
            } else {
                classify_ssh_failure(&detail)
            };
            return HostHealthCheck::failed(status, detail, started);
        }
    };
    if !version_at_least(&version, MINIMUM_HERDR_VERSION) {
        return HostHealthCheck {
            status: HostHealthStatus::UnsupportedHerdr,
            detail: format!("Herdr {version} is older than the required {MINIMUM_HERDR_VERSION}"),
            herdr_version: Some(version),
            session_count: None,
            latency_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        };
    }
    match discover_sessions(profile) {
        Ok(sessions) => HostHealthCheck {
            status: HostHealthStatus::Ready,
            detail: "SSH and Herdr are ready".into(),
            herdr_version: Some(version),
            session_count: Some(sessions.len()),
            latency_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        },
        Err(error) => HostHealthCheck::failed(HostHealthStatus::Failed, error.to_string(), started),
    }
}

fn probe_ssh(profile: &ConnectionProfile) -> Result<()> {
    let ConnectionProfile::Ssh {
        destination,
        port,
        identity_file,
        ..
    } = profile
    else {
        return Ok(());
    };
    let mut command = Command::new("/usr/bin/ssh");
    add_ssh_common(&mut command, destination, *port, identity_file.as_deref());
    let output = command
        .arg("--")
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(HerdrError::Ssh(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

fn classify_ssh_failure(detail: &str) -> HostHealthStatus {
    let detail = detail.to_ascii_lowercase();
    if detail.contains("permission denied")
        || detail.contains("authentication failed")
        || detail.contains("too many authentication failures")
    {
        HostHealthStatus::AuthenticationRequired
    } else if detail.contains("host key verification failed")
        || detail.contains("remote host identification has changed")
        || detail.contains("authenticity of host")
    {
        HostHealthStatus::HostKeyRequired
    } else if detail.contains("could not resolve hostname")
        || detail.contains("name or service not known")
        || detail.contains("operation timed out")
        || detail.contains("connection timed out")
        || detail.contains("no route to host")
        || detail.contains("connection refused")
    {
        HostHealthStatus::Unreachable
    } else {
        HostHealthStatus::Failed
    }
}

fn looks_like_missing_herdr(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("remote herdr executable was not found")
        || detail.contains("herdr: command not found")
        || detail.contains("no such file or directory")
}

fn version_at_least(actual: &str, minimum: &str) -> bool {
    fn parts(value: &str) -> [u64; 3] {
        let version = value
            .split_whitespace()
            .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .unwrap_or(value);
        let mut numbers = version
            .split(|character: char| !character.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse::<u64>().ok());
        [
            numbers.next().unwrap_or(0),
            numbers.next().unwrap_or(0),
            numbers.next().unwrap_or(0),
        ]
    }
    parts(actual) >= parts(minimum)
}

pub fn stop_session(profile: &ConnectionProfile, name: &str) -> Result<Value> {
    command_json(profile, &["session", "stop", name, "--json"])
}

pub fn delete_session(profile: &ConnectionProfile, name: &str) -> Result<Value> {
    command_json(profile, &["session", "delete", name, "--json"])
}

fn command_json(profile: &ConnectionProfile, args: &[&str]) -> Result<Value> {
    let output = command_output(profile, args)?;
    Ok(serde_json::from_slice(&output)?)
}

fn command_output(profile: &ConnectionProfile, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = command_for(profile, args)?;
    command
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped());
    let output = command.output().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => HerdrError::MissingExecutable(profile.herdr_path().into()),
        _ => HerdrError::Io(error),
    })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        match profile {
            ConnectionProfile::Ssh { .. } => Err(HerdrError::Ssh(stderr)),
            ConnectionProfile::Local { .. } => Err(HerdrError::Command(stderr)),
        }
    }
}

fn command_for(profile: &ConnectionProfile, args: &[&str]) -> Result<Command> {
    match profile {
        ConnectionProfile::Local { herdr_path } => {
            let mut command = Command::new(resolve_local_herdr(herdr_path));
            command.args(args);
            Ok(command)
        }
        ConnectionProfile::Ssh {
            destination,
            port,
            identity_file,
            herdr_path,
            ..
        } => {
            let mut command = Command::new("/usr/bin/ssh");
            add_ssh_common(&mut command, destination, *port, identity_file.as_deref());
            let remote = remote_herdr_command(herdr_path, args);
            command.arg("--").arg(remote);
            Ok(command)
        }
    }
}

fn remote_herdr_command(configured: &str, args: &[&str]) -> String {
    let arguments = args
        .iter()
        .copied()
        .map(posix_quote)
        .collect::<Vec<_>>()
        .join(" ");
    let arguments = if arguments.is_empty() {
        String::new()
    } else {
        format!(" {arguments}")
    };

    if configured != "herdr" {
        return format!(
            "exec {}{arguments}",
            remote_configured_executable(configured)
        );
    }

    format!(
        r#"herdr_path=$(command -v herdr 2>/dev/null || true)
case "$herdr_path" in
    */mise/shims/herdr) herdr_path= ;;
esac
if [ -n "$herdr_path" ] && [ -x "$herdr_path" ]; then
    exec "$herdr_path"{arguments}
fi
for herdr_path in \
    "$HOME/.local/bin/herdr" \
    "$HOME/.cargo/bin/herdr" \
    /opt/homebrew/bin/herdr \
    /usr/local/bin/herdr \
    /home/linuxbrew/.linuxbrew/bin/herdr \
    "$HOME/.nix-profile/bin/herdr" \
    "/etc/profiles/per-user/${{USER:-}}/bin/herdr" \
    /nix/var/nix/profiles/default/bin/herdr \
    /run/current-system/sw/bin/herdr
do
    if [ -x "$herdr_path" ]; then
        exec "$herdr_path"{arguments}
    fi
done
for herdr_path in \
    "$HOME"/.local/share/mise/installs/herdr/*/bin/herdr \
    "$HOME"/.local/share/mise/installs/herdr/*/herdr \
    "$HOME"/.local/share/mise/installs/github-ogulcancelik-herdr/*/herdr
do
    if [ -x "$herdr_path" ]; then
        exec "$herdr_path"{arguments}
    fi
done
printf '%s\n' 'OcHerdr: remote Herdr executable was not found in PATH or common install locations' >&2
exit 127"#
    )
}

fn remote_configured_executable(configured: &str) -> String {
    configured
        .strip_prefix("~/")
        .map(|path| format!("\"$HOME\"/{}", posix_quote(path)))
        .unwrap_or_else(|| posix_quote(configured))
}

fn resolve_local_herdr(configured: &str) -> PathBuf {
    if configured != "herdr" || Path::new(configured).is_absolute() {
        return configured.into();
    }
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/herdr"),
        PathBuf::from("/usr/local/bin/herdr"),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin/herdr"));
        candidates.push(home.join(".cargo/bin/herdr"));
    }
    for candidate in candidates {
        if candidate.is_file() {
            return candidate;
        }
    }
    configured.into()
}

fn add_ssh_common(
    command: &mut Command,
    destination: &str,
    port: Option<u16>,
    identity_file: Option<&Path>,
) {
    command
        .args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"])
        .args([
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
        ]);
    if let Some(port) = port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity_file) = identity_file {
        command.arg("-i").arg(identity_file);
    }
    command.arg(destination);
}

pub fn posix_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

pub struct SessionConnection {
    socket_path: PathBuf,
    _tunnel: Option<SshTunnel>,
}

impl SessionConnection {
    pub fn connect(profile: &ConnectionProfile, session: &SessionSummary) -> Result<Self> {
        match profile {
            ConnectionProfile::Local { .. } => Ok(Self {
                socket_path: session.socket_path.clone(),
                _tunnel: None,
            }),
            ConnectionProfile::Ssh { .. } => {
                let tunnel = SshTunnel::open(profile, &session.socket_path)?;
                Ok(Self {
                    socket_path: tunnel.local_socket.clone(),
                    _tunnel: Some(tunnel),
                })
            }
        }
    }

    pub fn snapshot(&self) -> Result<HierarchySnapshot> {
        let result = self.invoke("session.snapshot", json!({}))?;
        let snapshot = result
            .get("snapshot")
            .cloned()
            .ok_or_else(|| HerdrError::Protocol("snapshot result is missing `snapshot`".into()))?;
        Ok(serde_json::from_value(snapshot)?)
    }

    pub fn invoke(&self, method: &str, params: Value) -> Result<Value> {
        request_socket(&self.socket_path, method, params)
    }

    pub fn subscribe(&self) -> Result<EventStream> {
        EventStream::connect(&self.socket_path)
    }

    pub fn subscribe_background(&self) -> Result<EventSubscription> {
        subscribe_events(&self.socket_path)
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub fn request_socket(socket_path: &Path, method: &str, params: Value) -> Result<Value> {
    request_socket_with_timeout(socket_path, method, params, REQUEST_TIMEOUT)
}

fn request_socket_with_timeout(
    socket_path: &Path,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let id = format!("ocherdr-{}", REQUEST_ID.fetch_add(1, Ordering::Relaxed));
    write_socket_json(
        &mut stream,
        &json!({ "id": id, "method": method, "params": params }),
        timeout,
    )?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| timeout_or_io(error, timeout))?;
    if line.trim().is_empty() {
        return Err(HerdrError::Protocol("empty API response".into()));
    }
    let value: Value = serde_json::from_str(&line)?;
    if let Some(error) = api_error(&value) {
        return Err(error);
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| HerdrError::Protocol("API response is missing `result`".into()))
}

fn api_error(value: &Value) -> Option<HerdrError> {
    let error = value.get("error")?;
    Some(HerdrError::Api {
        code: error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .into(),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown Herdr API error")
            .into(),
    })
}

fn write_socket_json(stream: &mut UnixStream, value: &Value, timeout: Duration) -> Result<()> {
    let mut payload = serde_json::to_vec(value)?;
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .map_err(|error| timeout_or_io(error, timeout))?;
    stream
        .flush()
        .map_err(|error| timeout_or_io(error, timeout))?;
    Ok(())
}

fn timeout_or_io(error: io::Error, timeout: Duration) -> HerdrError {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => HerdrError::Timeout(timeout),
        _ => HerdrError::Io(error),
    }
}

struct SshTunnel {
    child: Child,
    local_socket: PathBuf,
    _directory: TempDir,
}

impl SshTunnel {
    fn open(profile: &ConnectionProfile, remote_socket: &Path) -> Result<Self> {
        let ConnectionProfile::Ssh {
            destination,
            port,
            identity_file,
            ..
        } = profile
        else {
            return Err(HerdrError::Protocol(
                "SSH tunnel requires an SSH profile".into(),
            ));
        };
        let directory = tempfile::Builder::new()
            .prefix("ocherdr-")
            .tempdir_in("/tmp")?;
        let local_socket = directory.path().join("api.sock");
        let forwarding = format!("{}:{}", local_socket.display(), remote_socket.display());
        let mut command = Command::new("/usr/bin/ssh");
        command
            .args(["-N", "-T", "-o", "BatchMode=yes"])
            .args(["-o", "ConnectTimeout=8", "-o", "ExitOnForwardFailure=yes"])
            .args([
                "-o",
                "StreamLocalBindUnlink=yes",
                "-o",
                "ServerAliveInterval=15",
            ])
            .args(["-o", "ServerAliveCountMax=3", "-L"])
            .arg(&forwarding);
        if let Some(port) = port {
            command.arg("-p").arg(port.to_string());
        }
        if let Some(identity_file) = identity_file {
            command.arg("-i").arg(identity_file);
        }
        command
            .arg(destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if local_socket.exists() {
                return Ok(Self {
                    child,
                    local_socket,
                    _directory: directory,
                });
            }
            if let Some(status) = child.try_wait()? {
                let stderr = child
                    .stderr
                    .take()
                    .map(|pipe| {
                        let mut reader = BufReader::new(pipe);
                        let mut message = String::new();
                        let _ = reader.read_to_string(&mut message);
                        message
                    })
                    .unwrap_or_default();
                return Err(HerdrError::Ssh(format!(
                    "tunnel exited with {status}: {stderr}"
                )));
            }
            thread::sleep(Duration::from_millis(40));
        }
        let _ = child.kill();
        Err(HerdrError::Ssh("timed out opening Herdr API tunnel".into()))
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) const EVENT_SUBSCRIPTIONS: &[&str] = &[
    "workspace.created",
    "workspace.updated",
    "workspace.metadata_updated",
    "workspace.renamed",
    "workspace.moved",
    "workspace.reordered",
    "workspace.closed",
    "workspace.focused",
    "worktree.created",
    "worktree.opened",
    "worktree.removed",
    "tab.created",
    "tab.closed",
    "tab.focused",
    "tab.renamed",
    "tab.moved",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.focused",
    "pane.moved",
    "pane.exited",
    "pane.agent_detected",
    "layout.updated",
];

pub struct EventStream {
    reader: BufReader<UnixStream>,
}

impl EventStream {
    fn connect(socket_path: &Path) -> Result<Self> {
        Self::connect_with_subscriptions(socket_path, session_subscriptions(), "ocherdr-events")
    }

    fn connect_agent_status(socket_path: &Path, pane_ids: &[String]) -> Result<Self> {
        Self::connect_with_subscriptions(
            socket_path,
            agent_status_subscriptions(pane_ids),
            "ocherdr-agent-status",
        )
    }

    fn connect_with_subscriptions(
        socket_path: &Path,
        subscriptions: Vec<Value>,
        id_prefix: &str,
    ) -> Result<Self> {
        let mut stream = UnixStream::connect(socket_path)?;
        stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
        stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
        let id = format!("{id_prefix}-{}", REQUEST_ID.fetch_add(1, Ordering::Relaxed));
        write_socket_json(
            &mut stream,
            &json!({
                "id": id,
                "method": "events.subscribe",
                "params": { "subscriptions": subscriptions }
            }),
            REQUEST_TIMEOUT,
        )?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| timeout_or_io(error, REQUEST_TIMEOUT))?;
        parse_subscription_ack(&line)?;
        // Idle gaps after subscribe are normal; SSH ServerAlive* closes a dead tunnel.
        reader.get_mut().set_read_timeout(None)?;
        Ok(Self { reader })
    }

    pub fn next_event(&mut self) -> Result<Option<HerdrEvent>> {
        let mut line = String::new();
        let count = self.reader.read_line(&mut line)?;
        if count == 0 {
            return Ok(None);
        }
        Ok(Some(parse_event_line(&line)?))
    }
}

fn parse_subscription_ack(line: &str) -> Result<()> {
    let line = line.trim();
    if line.is_empty() {
        return Err(HerdrError::Protocol("empty subscription ack".into()));
    }
    let value: Value = serde_json::from_str(line)?;
    if let Some(error) = api_error(&value) {
        return Err(error);
    }
    match value
        .get("result")
        .and_then(|result| result.get("type"))
        .and_then(Value::as_str)
    {
        Some("subscription_started") => Ok(()),
        Some(other) => Err(HerdrError::Protocol(format!(
            "unexpected subscription ack type `{other}`"
        ))),
        None => Err(HerdrError::Protocol(
            "subscription ack is missing `result.type`".into(),
        )),
    }
}

fn parse_event_line(line: &str) -> Result<HerdrEvent> {
    let value: Value = serde_json::from_str(line)?;
    let Some(data) = value.get("data") else {
        return Err(HerdrError::Protocol("event is missing `data`".into()));
    };
    // Parameterized subscriptions use a different envelope: `event` is a dotted
    // name and `data` has no `type` tag.
    if value.get("event").and_then(Value::as_str) == Some("pane.agent_status_changed") {
        return parse_agent_status_changed(data);
    }
    Ok(serde_json::from_value(data.clone())?)
}

fn parse_agent_status_changed(data: &Value) -> Result<HerdrEvent> {
    #[derive(Deserialize)]
    struct Payload {
        pane_id: String,
        workspace_id: String,
        agent_status: AgentStatus,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        display_agent: Option<String>,
        #[serde(default)]
        state_labels: HashMap<String, String>,
    }
    let payload: Payload = serde_json::from_value(data.clone())?;
    Ok(HerdrEvent::PaneAgentStatusChanged {
        pane_id: payload.pane_id,
        workspace_id: payload.workspace_id,
        agent_status: payload.agent_status,
        agent: payload.agent,
        title: payload.title,
        display_agent: payload.display_agent,
        state_labels: payload.state_labels,
    })
}

/// Session-wide EventHub types. Herdr starts these at sequence 0 and
/// replays retained history. OcHerdr subscribes once at connect and
/// never rebuilds this list.
fn session_subscriptions() -> Vec<Value> {
    EVENT_SUBSCRIPTIONS
        .iter()
        .map(|kind| json!({ "type": kind }))
        .collect()
}

/// Parameterized `pane.agent_status_changed`. Herdr starts these at
/// `current_sequence`, so a rebuild does not replay retained status
/// history.
fn agent_status_subscriptions(pane_ids: &[String]) -> Vec<Value> {
    pane_ids
        .iter()
        .map(|pane_id| {
            json!({
                "type": "pane.agent_status_changed",
                "pane_id": pane_id,
            })
        })
        .collect()
}

pub fn subscribe_events(socket_path: &Path) -> Result<EventSubscription> {
    Ok(EventSubscription::spawn(EventStream::connect(socket_path)?))
}

pub fn subscribe_agent_status(
    socket_path: &Path,
    pane_ids: &[String],
) -> Result<EventSubscription> {
    Ok(EventSubscription::spawn(EventStream::connect_agent_status(
        socket_path,
        pane_ids,
    )?))
}

pub struct EventSubscription {
    events: UnboundedReceiver<Result<HerdrEvent>>,
}

impl EventSubscription {
    pub fn new(events: UnboundedReceiver<Result<HerdrEvent>>) -> Self {
        Self { events }
    }

    fn spawn(mut stream: EventStream) -> Self {
        let (event_tx, event_rx) = futures_mpsc::unbounded();
        thread::spawn(move || {
            loop {
                match stream.next_event() {
                    Ok(Some(event)) => {
                        if event_tx.unbounded_send(Ok(event)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let fatal = !error.is_event_payload_error();
                        if event_tx.unbounded_send(Err(error)).is_err() {
                            break;
                        }
                        if fatal {
                            break;
                        }
                    }
                }
            }
        });
        Self::new(event_rx)
    }

    pub async fn next_batch(&mut self) -> Option<Vec<Result<HerdrEvent>>> {
        next_batch(&mut self.events).await
    }
}

const STREAM_BATCH_LIMIT: usize = 128;

/// Await the next item, then drain already-ready items.
/// `None` means the stream has closed.
pub async fn next_batch<T>(rx: &mut UnboundedReceiver<T>) -> Option<Vec<T>> {
    use futures::StreamExt as _;
    let first = rx.next().await?;
    let mut batch = vec![first];
    while batch.len() < STREAM_BATCH_LIMIT {
        match rx.try_recv() {
            Ok(item) => batch.push(item),
            Err(_) => break,
        }
    }
    Some(batch)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMode {
    Observe,
    /// Requests writable control but leaves an existing controller in place.
    Control,
    /// Replaces an existing controller after an explicit user confirmation.
    ControlTakeover,
}

impl TerminalMode {
    pub const fn is_controlled(self) -> bool {
        matches!(self, Self::Control | Self::ControlTakeover)
    }

    const fn takes_over(self) -> bool {
        matches!(self, Self::ControlTakeover)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type")]
enum TerminalEnvelope {
    #[serde(rename = "terminal.frame")]
    Frame {
        seq: u64,
        encoding: String,
        width: u16,
        height: u16,
        full: bool,
        bytes: String,
    },
    #[serde(rename = "terminal.mouse_capture")]
    MouseCapture { enabled: bool, sgr_pixels: bool },
    #[serde(rename = "terminal.closed")]
    Closed { reason: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFrame {
    pub seq: u64,
    pub width: u16,
    pub height: u16,
    pub full: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Frame(TerminalFrame),
    MouseCapture { enabled: bool, sgr_pixels: bool },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum TerminalControlCommand<'a> {
    #[serde(rename = "terminal.input")]
    Input { bytes: &'a str },
    #[serde(rename = "terminal.resize")]
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    #[serde(rename = "terminal.scroll")]
    Scroll {
        direction: &'a str,
        lines: u16,
        source: &'a str,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    },
    #[serde(rename = "terminal.release")]
    Release {},
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCommand {
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    Scroll {
        direction: &'static str,
        lines: u16,
    },
    Release,
}

impl TerminalCommand {
    fn write_to(&self, stdin: &mut ChildStdin) -> Result<()> {
        match self {
            Self::Input(bytes) => {
                let bytes = base64::engine::general_purpose::STANDARD.encode(bytes);
                serde_json::to_writer(
                    &mut *stdin,
                    &TerminalControlCommand::Input { bytes: &bytes },
                )?
            }
            Self::Resize {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
            } => serde_json::to_writer(
                &mut *stdin,
                &TerminalControlCommand::Resize {
                    cols: *cols,
                    rows: *rows,
                    cell_width_px: *cell_width_px,
                    cell_height_px: *cell_height_px,
                },
            )?,
            Self::Scroll { direction, lines } => serde_json::to_writer(
                &mut *stdin,
                &TerminalControlCommand::Scroll {
                    direction,
                    lines: *lines,
                    source: "wheel",
                    column: None,
                    row: None,
                    modifiers: 0,
                },
            )?,
            Self::Release => {
                serde_json::to_writer(&mut *stdin, &TerminalControlCommand::Release {})?
            }
        }
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }
}

pub struct TerminalStream {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    last_seq: Option<u64>,
    released: bool,
}

impl TerminalStream {
    pub fn spawn(
        profile: &ConnectionProfile,
        session_name: &str,
        target: &str,
        mode: TerminalMode,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        let args = terminal_session_args(session_name, target, mode, cols, rows);
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let mut command = command_for(profile, &refs)?;
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HerdrError::Protocol("terminal stdout was not piped".into()))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            last_seq: None,
            released: false,
        })
    }

    pub fn read_event(&mut self) -> Result<Option<TerminalEvent>> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        match serde_json::from_str::<TerminalEnvelope>(&line)? {
            TerminalEnvelope::Closed { reason } => Err(HerdrError::TerminalClosed(
                reason.unwrap_or_else(|| "server closed the stream".into()),
            )),
            TerminalEnvelope::Frame {
                seq,
                encoding,
                width,
                height,
                full,
                bytes,
            } => {
                if encoding != "ansi" {
                    return Err(HerdrError::Protocol(format!(
                        "unsupported terminal encoding {encoding}"
                    )));
                }
                if let Some(previous) = self.last_seq
                    && seq != previous.saturating_add(1)
                    && !full
                {
                    return Err(HerdrError::Protocol(format!(
                        "terminal frame gap: expected {}, got {seq}",
                        previous.saturating_add(1)
                    )));
                }
                self.last_seq = Some(seq);
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(bytes)
                    .map_err(|error| HerdrError::Protocol(error.to_string()))?;
                Ok(Some(TerminalEvent::Frame(TerminalFrame {
                    seq,
                    width,
                    height,
                    full,
                    bytes,
                })))
            }
            TerminalEnvelope::MouseCapture {
                enabled,
                sgr_pixels,
            } => Ok(Some(TerminalEvent::MouseCapture {
                enabled,
                sgr_pixels,
            })),
        }
    }

    pub fn send(&mut self, command: &TerminalControlCommand<'_>) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| HerdrError::Protocol("terminal stream has no writable input".into()))?;
        serde_json::to_writer(&mut *stdin, command)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    pub fn release(&mut self) -> Result<()> {
        if !self.released {
            self.send(&TerminalControlCommand::Release {})?;
            self.released = true;
        }
        Ok(())
    }
}

fn terminal_session_args(
    session_name: &str,
    target: &str,
    mode: TerminalMode,
    cols: u16,
    rows: u16,
) -> Vec<String> {
    let mut args = Vec::<String>::new();
    if session_name != "default" {
        args.extend(["--session".into(), session_name.into()]);
    }
    args.extend([
        "terminal".into(),
        "session".into(),
        match mode {
            TerminalMode::Observe => "observe",
            TerminalMode::Control | TerminalMode::ControlTakeover => "control",
        }
        .into(),
        target.into(),
    ]);
    if mode.takes_over() {
        args.push("--takeover".into());
    }
    args.extend([
        "--cols".into(),
        cols.max(1).to_string(),
        "--rows".into(),
        rows.max(1).to_string(),
    ]);
    args
}

impl Drop for TerminalStream {
    fn drop(&mut self) {
        let _ = self.release();
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct TerminalSession {
    commands: Sender<TerminalCommand>,
    process_id: Arc<AtomicU32>,
    alive: Arc<AtomicBool>,
}

impl TerminalSession {
    pub fn spawn(
        profile: ConnectionProfile,
        session_name: String,
        target: String,
        mode: TerminalMode,
        cols: u16,
        rows: u16,
    ) -> (Self, UnboundedReceiver<Result<TerminalEvent>>) {
        let (command_tx, command_rx) = mpsc::channel::<TerminalCommand>();
        let (event_tx, event_rx) = futures_mpsc::unbounded::<Result<TerminalEvent>>();
        let process_id = Arc::new(AtomicU32::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let worker_process_id = process_id.clone();
        let worker_alive = alive.clone();
        thread::spawn(move || {
            let stream = TerminalStream::spawn(&profile, &session_name, &target, mode, cols, rows);
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(error) => {
                    worker_alive.store(false, Ordering::Release);
                    let _ = event_tx.unbounded_send(Err(error));
                    return;
                }
            };
            worker_process_id.store(stream.child.id(), Ordering::Release);
            let mut stdin = stream.stdin.take();
            let writer = thread::spawn(move || {
                let Some(mut stdin) = stdin.take() else {
                    return;
                };
                while let Ok(command) = command_rx.recv() {
                    let release = command == TerminalCommand::Release;
                    if command.write_to(&mut stdin).is_err() || release {
                        break;
                    }
                }
            });
            loop {
                match stream.read_event() {
                    Ok(Some(event)) => {
                        if event_tx.unbounded_send(Ok(event)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = event_tx.unbounded_send(Err(error));
                        break;
                    }
                }
            }
            drop(stream);
            worker_process_id.store(0, Ordering::Release);
            worker_alive.store(false, Ordering::Release);
            let _ = writer.join();
        });
        (
            Self {
                commands: command_tx,
                process_id,
                alive,
            },
            event_rx,
        )
    }

    pub fn send(&self, command: TerminalCommand) -> Result<()> {
        self.commands
            .send(command)
            .map_err(|_| HerdrError::TerminalClosed("terminal worker stopped".into()))
    }

    pub fn is_closed(&self) -> bool {
        !self.alive.load(Ordering::Acquire)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.commands.send(TerminalCommand::Release);
        let process_id = self.process_id.load(Ordering::Acquire);
        if process_id != 0 {
            // SAFETY: the PID was obtained from our still-live child process. SIGTERM is
            // idempotent for teardown, and ESRCH simply means the process already exited.
            unsafe {
                libc::kill(process_id as libc::pid_t, libc::SIGTERM);
            }
        }
    }
}

pub fn ssh_host_aliases() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let ssh_dir = home.join(".ssh");
    let mut hosts = Vec::new();
    let mut visited = std::collections::HashSet::new();
    collect_ssh_hosts(&ssh_dir.join("config"), &ssh_dir, &mut visited, &mut hosts);
    hosts
}

fn collect_ssh_hosts(
    path: &Path,
    ssh_dir: &Path,
    visited: &mut std::collections::HashSet<PathBuf>,
    hosts: &mut Vec<String>,
) {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if !visited.insert(canonical) {
        return;
    }
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    for host in parse_ssh_hosts(&contents) {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    for line in contents.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("Include ")
            .or_else(|| line.strip_prefix("include "))
        else {
            continue;
        };
        for include in rest.split_whitespace() {
            let expanded = if let Some(relative) = include.strip_prefix("~/") {
                dirs::home_dir().map(|home| home.join(relative))
            } else {
                let include = PathBuf::from(include);
                Some(if include.is_absolute() {
                    include
                } else {
                    ssh_dir.join(include)
                })
            };
            let Some(expanded) = expanded else {
                continue;
            };
            for included_path in expand_simple_glob(&expanded) {
                collect_ssh_hosts(&included_path, ssh_dir, visited, hosts);
            }
        }
    }
}

fn expand_simple_glob(path: &Path) -> Vec<PathBuf> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    if !file_name.contains(['*', '?']) {
        return vec![path.to_owned()];
    }
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut matches = entries
        .flatten()
        .filter_map(|entry| {
            let candidate = entry.file_name();
            let candidate = candidate.to_str()?;
            wildcard_matches(file_name.as_bytes(), candidate.as_bytes()).then(|| entry.path())
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches
}

fn wildcard_matches(pattern: &[u8], value: &[u8]) -> bool {
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star, mut retry_value) = (None, 0);
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some(pattern_index);
            pattern_index += 1;
            retry_value = value_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            retry_value += 1;
            value_index = retry_value;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

pub fn parse_ssh_hosts(contents: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("Host ")
            .or_else(|| line.strip_prefix("host "))
        else {
            continue;
        };
        for host in rest.split_whitespace() {
            if !host.bytes().any(|byte| b"*?!".contains(&byte))
                && !hosts.iter().any(|known| known == host)
            {
                hosts.push(host.to_owned());
            }
        }
    }
    hosts
}

pub fn attach_command(profile: &ConnectionProfile, session_name: &str) -> String {
    match profile {
        ConnectionProfile::Local { herdr_path } => format!(
            "{} session attach {}",
            posix_quote(herdr_path),
            posix_quote(session_name)
        ),
        ConnectionProfile::Ssh {
            destination,
            herdr_path,
            ..
        } => {
            let attach = remote_herdr_command(herdr_path, &["session", "attach", session_name]);
            format!(
                "ssh -t {} {}",
                posix_quote(destination),
                posix_quote(&attach)
            )
        }
    }
}

/// An interactive OpenSSH command for authentication, host-key enrollment, or
/// manual repair in the user's system Terminal.
pub fn ssh_login_command(profile: &ConnectionProfile) -> Option<String> {
    let ConnectionProfile::Ssh {
        destination,
        port,
        identity_file,
        ..
    } = profile
    else {
        return None;
    };
    let mut arguments = vec!["ssh".to_owned()];
    if let Some(port) = port {
        arguments.extend(["-p".into(), port.to_string()]);
    }
    if let Some(identity_file) = identity_file {
        arguments.extend([
            "-i".into(),
            posix_quote(&identity_file.display().to_string()),
        ]);
    }
    arguments.push(posix_quote(destination));
    Some(arguments.join(" "))
}

#[cfg(target_os = "macos")]
pub fn open_system_terminal(command: &str) -> Result<()> {
    let status = Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "tell application \"Terminal\" to do script (item 1 of argv)",
            "-e",
            "tell application \"Terminal\" to activate",
            "-e",
            "end run",
            "--",
            command,
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(HerdrError::Command(format!(
            "failed to open Terminal: {status}"
        )))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn open_system_terminal(_command: &str) -> Result<()> {
    Err(HerdrError::Command(
        "opening the system terminal is currently supported on macOS only".into(),
    ))
}

#[cfg(test)]
mod tests {
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
    fn terminal_input_serializes_lossless_bytes() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([0, 0x1b, 0x80, 0xff]);
        let value =
            serde_json::to_value(TerminalControlCommand::Input { bytes: &encoded }).unwrap();

        assert_eq!(value["type"], "terminal.input");
        assert_eq!(value["bytes"], "ABuA/w==");
        assert!(value.get("text").is_none());
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

        let takeover =
            terminal_session_args("work", "pane-1", TerminalMode::ControlTakeover, 120, 40);
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
        parse_subscription_ack(
            r#"{"id":"ocherdr-events-1","result":{"type":"subscription_started"}}"#,
        )
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
        assert!(
            matches!(missing_data, HerdrError::Protocol(message) if message.contains("`data`"))
        );
        let broken = parse_event_line(r#"{"data":{"type":"pane_updated"},"event":"pane_updated"}"#)
            .unwrap_err();
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
        assert!(
            matches!(error, HerdrError::Timeout(timeout) if timeout == Duration::from_millis(100))
        );
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
}
