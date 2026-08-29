//! Herdr transports used by OcHerdr.
//!
//! Session state uses Herdr's public NDJSON API. Terminal rendering and input use
//! a versioned private-protocol facade over the companion client socket.

mod private_protocol;
mod private_v20;

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use futures::channel::mpsc::{self as futures_mpsc, Receiver, UnboundedReceiver};
use futures::{FutureExt as _, SinkExt as _, Stream};
use ocherdr_core::{
    AgentStatus, ConnectionProfile, HerdrEvent, HierarchySnapshot, MINIMUM_HERDR_VERSION,
    SessionSummary,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;
use thiserror::Error;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = private_protocol::MAX_CLIPBOARD_IMAGE_BYTES;
pub const SUPPORTED_TERMINAL_PROTOCOL_VERSIONS: &[u32] = private_protocol::SUPPORTED_VERSIONS;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEndpoint {
    socket_path: PathBuf,
}

impl TerminalEndpoint {
    fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    fn connect(&self) -> io::Result<UnixStream> {
        UnixStream::connect(&self.socket_path)
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

pub struct SessionConnection {
    api_socket_path: PathBuf,
    terminal_endpoint: TerminalEndpoint,
    _tunnel: Option<SshTunnel>,
}

impl SessionConnection {
    pub fn connect(profile: &ConnectionProfile, session: &SessionSummary) -> Result<Self> {
        let remote_client_socket = client_socket_path_from_api(&session.socket_path);
        match profile {
            ConnectionProfile::Local { .. } => Ok(Self {
                api_socket_path: session.socket_path.clone(),
                terminal_endpoint: TerminalEndpoint::new(remote_client_socket),
                _tunnel: None,
            }),
            ConnectionProfile::Ssh { .. } => {
                let tunnel = SshTunnel::open(profile, &session.socket_path, &remote_client_socket)?;
                Ok(Self {
                    api_socket_path: tunnel.local_api_socket.clone(),
                    terminal_endpoint: TerminalEndpoint::new(tunnel.local_client_socket.clone()),
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
        request_socket(&self.api_socket_path, method, params)
    }

    pub fn subscribe(&self) -> Result<EventStream> {
        EventStream::connect(&self.api_socket_path)
    }

    pub fn subscribe_background(&self) -> Result<EventSubscription> {
        subscribe_events(&self.api_socket_path)
    }

    pub fn socket_path(&self) -> &Path {
        &self.api_socket_path
    }

    pub fn terminal_endpoint(&self) -> TerminalEndpoint {
        self.terminal_endpoint.clone()
    }
}

fn client_socket_path_from_api(api_socket_path: &Path) -> PathBuf {
    let stem = api_socket_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("herdr");
    api_socket_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{stem}-client.sock"))
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
    local_api_socket: PathBuf,
    local_client_socket: PathBuf,
    _directory: TempDir,
}

impl SshTunnel {
    fn open(
        profile: &ConnectionProfile,
        remote_api_socket: &Path,
        remote_client_socket: &Path,
    ) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("ocherdr-")
            .tempdir_in("/tmp")?;
        let local_api_socket = directory.path().join("api.sock");
        let local_client_socket = directory.path().join("client.sock");
        let api_forwarding = format!(
            "{}:{}",
            local_api_socket.display(),
            remote_api_socket.display()
        );
        let client_forwarding = format!(
            "{}:{}",
            local_client_socket.display(),
            remote_client_socket.display()
        );
        let mut command = ssh_tunnel_command(profile, &api_forwarding, &client_forwarding)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if local_api_socket.exists() && local_client_socket.exists() {
                return Ok(Self {
                    child,
                    local_api_socket,
                    local_client_socket,
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
        let _ = child.wait();
        Err(HerdrError::Ssh(
            "timed out opening the Herdr socket tunnel".into(),
        ))
    }
}

fn ssh_tunnel_command(
    profile: &ConnectionProfile,
    api_forwarding: &str,
    client_forwarding: &str,
) -> Result<Command> {
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
        .arg(api_forwarding)
        .arg("-L")
        .arg(client_forwarding);
    if let Some(port) = port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity_file) = identity_file {
        command.arg("-i").arg(identity_file);
    }
    command.arg(destination);
    Ok(command)
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
pub async fn next_batch<T, S>(rx: &mut S) -> Option<Vec<T>>
where
    S: Stream<Item = T> + Unpin,
{
    use futures::StreamExt as _;
    let first = rx.next().await?;
    let mut batch = vec![first];
    while batch.len() < STREAM_BATCH_LIMIT {
        match rx.next().now_or_never().flatten() {
            Some(item) => batch.push(item),
            None => break,
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
    KittyKeyboardReportAll { enabled: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCommand {
    Input(Vec<u8>),
    ClipboardImage {
        extension: String,
        bytes: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    Scroll {
        direction: TerminalScrollDirection,
        lines: u16,
    },
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalScrollDirection {
    Up,
    Down,
}

pub struct TerminalSession {
    commands: Sender<TerminalCommand>,
    alive: Arc<AtomicBool>,
}

/// A small queue deliberately applies backpressure to Herdr's terminal
/// reader. Full frames can be large; an unbounded queue made a temporarily
/// stalled GPUI retain every frame for every pane.
pub type TerminalEventReceiver = Receiver<Result<TerminalEvent>>;
const TERMINAL_EVENT_QUEUE_CAPACITY: usize = 4;

impl TerminalSession {
    pub fn spawn(
        endpoint: TerminalEndpoint,
        protocol: u32,
        target: String,
        mode: TerminalMode,
        cols: u16,
        rows: u16,
    ) -> (Self, TerminalEventReceiver) {
        let (command_tx, command_rx) = mpsc::channel::<TerminalCommand>();
        let (mut event_tx, event_rx) =
            futures_mpsc::channel::<Result<TerminalEvent>>(TERMINAL_EVENT_QUEUE_CAPACITY);
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        thread::spawn(move || {
            let connection = private_protocol::connect(private_protocol::TerminalConnect {
                endpoint: &endpoint,
                protocol,
                target: &target,
                mode,
                cols,
                rows,
                cell_width_px: 0,
                cell_height_px: 0,
            });
            let (mut reader, mut writer) = match connection {
                Ok(connection) => connection,
                Err(error) => {
                    worker_alive.store(false, Ordering::Release);
                    let _ = futures::executor::block_on(event_tx.send(Err(error)));
                    return;
                }
            };
            let _writer = thread::spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    let release = command == TerminalCommand::Release;
                    if writer.send(command).is_err() || release {
                        break;
                    }
                }
            });
            loop {
                match reader.read_event() {
                    Ok(Some(event)) => {
                        if futures::executor::block_on(event_tx.send(Ok(event))).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = futures::executor::block_on(event_tx.send(Err(error)));
                        break;
                    }
                }
            }
            worker_alive.store(false, Ordering::Release);
        });
        (
            Self {
                commands: command_tx,
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
mod tests;
