//! Herdr transports used by OcHerdr.
//!
//! Rendering, semantic input, and session-invalidation pushes use Herdr's
//! stable endpoint protocol over the companion client socket (locally) or an
//! SSH stdio bridge running Herdr's `remote-client-bridge` (remotely). API
//! methods outside the endpoint lane's allowlist — `session.snapshot`,
//! `agent.*`, `pane.move` — use Herdr's public NDJSON API socket directly
//! (locally) or through an `ssh -N -L` StreamLocal forward (remotely; Herdr
//! has no stdio bridge for the API socket).

pub mod endpoint;
pub mod endpoint_v1;
pub mod surface_ansi;

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
#[cfg(windows)]
use uds_windows::UnixStream;

use futures::{FutureExt as _, Stream};
use ocherdr_core::{ConnectionProfile, HierarchySnapshot, MINIMUM_HERDR_VERSION, SessionSummary};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = endpoint_v1::MAX_CLIPBOARD_IMAGE_PAYLOAD;
/// Herdr's name for the implicit session; `--session` is omitted for it.
const DEFAULT_SESSION_NAME: &str = "default";

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
        proxy_jump,
        ..
    } = profile
    else {
        return Ok(());
    };
    let mut command = Command::new(system_ssh());
    add_ssh_common(
        &mut command,
        destination,
        *port,
        identity_file.as_deref(),
        proxy_jump.as_deref(),
    );
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
            proxy_jump,
            herdr_path,
            ..
        } => {
            let mut command = Command::new(system_ssh());
            add_ssh_common(
                &mut command,
                destination,
                *port,
                identity_file.as_deref(),
                proxy_jump.as_deref(),
            );
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
    proxy_jump: Option<&str>,
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
    if let Some(proxy_jump) = proxy_jump {
        command.arg("-J").arg(proxy_jump);
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
    pub(crate) fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub(crate) fn connect(&self) -> io::Result<UnixStream> {
        UnixStream::connect(&self.socket_path)
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

pub struct SessionConnection {
    api: ApiClient,
    endpoint: endpoint::EndpointTransport,
}

impl SessionConnection {
    pub fn connect(profile: &ConnectionProfile, session: &SessionSummary) -> Result<Self> {
        let remote_client_socket = client_socket_path_from_api(&session.socket_path);
        match profile {
            ConnectionProfile::Local { .. } => Ok(Self {
                api: ApiClient(ApiChannel::Local(session.socket_path.clone())),
                endpoint: endpoint::EndpointTransport::local(remote_client_socket),
            }),
            ConnectionProfile::Ssh { .. } => Ok(Self {
                api: ApiClient(ApiChannel::Ssh(SshApiTunnel::spawn(profile, session)?)),
                endpoint: endpoint::EndpointTransport::ssh(profile.clone(), session.name.clone()),
            }),
        }
    }

    pub fn snapshot(&self) -> Result<HierarchySnapshot> {
        let result = self.api_request("session.snapshot", json!({}))?;
        let snapshot = result
            .get("snapshot")
            .cloned()
            .ok_or_else(|| HerdrError::Protocol("snapshot result is missing `snapshot`".into()))?;
        Ok(serde_json::from_value(snapshot)?)
    }

    pub fn api_request(&self, method: &str, params: Value) -> Result<Value> {
        self.api.request(method, params)
    }

    pub fn api_client(&self) -> ApiClient {
        self.api.clone()
    }

    pub fn endpoint_transport(&self) -> endpoint::EndpointTransport {
        self.endpoint.clone()
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

#[derive(Clone)]
enum ApiChannel {
    Local(PathBuf),
    Ssh(SshApiTunnel),
}

/// Cheaply cloneable NDJSON API client that can be moved into background
/// tasks. Each call opens a short-lived socket connection; on SSH profiles
/// the socket is the local end of a StreamLocal forward.
#[derive(Clone)]
pub struct ApiClient(ApiChannel);

impl ApiClient {
    /// One NDJSON JSON-RPC call on Herdr's public API.
    pub fn request(&self, method: &str, params: Value) -> Result<Value> {
        match &self.0 {
            ApiChannel::Local(socket_path) => request_socket(socket_path, method, params),
            ApiChannel::Ssh(tunnel) => request_socket(&tunnel.local_socket, method, params),
        }
    }
}

/// A persistent `ssh -N -L local.sock:remote.sock` process forwarding the
/// remote session's public API socket to a local-only socket file. Herdr
/// ships a stdio bridge only for the client socket (`remote-client-bridge`),
/// so the NDJSON API rides OpenSSH's StreamLocal forwarding instead.
#[derive(Clone)]
struct SshApiTunnel {
    local_socket: PathBuf,
    _state: Arc<SshBridgeState>,
    _directory: Arc<tempfile::TempDir>,
}

impl SshApiTunnel {
    fn spawn(profile: &ConnectionProfile, session: &SessionSummary) -> Result<Self> {
        let ConnectionProfile::Ssh {
            destination,
            port,
            identity_file,
            proxy_jump,
            ..
        } = profile
        else {
            return Err(HerdrError::Protocol(
                "SSH API tunnel requires an SSH profile".into(),
            ));
        };
        #[cfg(target_os = "windows")]
        let temporary_root = std::env::temp_dir();
        #[cfg(not(target_os = "windows"))]
        let temporary_root = PathBuf::from("/tmp");
        let directory = tempfile::Builder::new()
            .prefix("ocherdr-api-")
            .tempdir_in(temporary_root)?;
        let local_socket = directory.path().join("api.sock");
        #[cfg(target_os = "windows")]
        let local_forward = Path::new("api.sock");
        #[cfg(not(target_os = "windows"))]
        let local_forward = local_socket.as_path();
        let forwarding = format!(
            "{}:{}",
            local_forward.display(),
            session.socket_path.display()
        );
        let mut command = Command::new(system_ssh());
        command
            .arg("-N")
            .args(["-o", "ExitOnForwardFailure=yes"])
            .args(["-o", "StreamLocalBindUnlink=yes"])
            .arg("-L")
            .arg(&forwarding);
        add_ssh_common(
            &mut command,
            destination,
            *port,
            identity_file.as_deref(),
            proxy_jump.as_deref(),
        );
        #[cfg(target_os = "windows")]
        command.current_dir(directory.path());
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(8);
        let local_socket = loop {
            if local_socket.exists() {
                break local_socket;
            }
            if let Some(status) = child.try_wait()? {
                let mut message = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut message);
                }
                return Err(HerdrError::Ssh(format!(
                    "API tunnel exited with {status}: {}",
                    message.trim()
                )));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HerdrError::Ssh(
                    "timed out forwarding the remote Herdr API socket".into(),
                ));
            }
            thread::sleep(Duration::from_millis(40));
        };
        // The stderr pipe stays with the child so a later ssh failure keeps
        // its diagnostics; the tunnel itself reports through request errors.
        Ok(Self {
            local_socket,
            _state: SshBridgeState::new(child, Arc::new(Mutex::new(String::new()))),
            _directory: Arc::new(directory),
        })
    }
}

/// State shared between an SSH bridge owner and its watchdog. Killing the
/// child is how every blocked side unblocks.
pub(crate) struct SshBridgeState {
    child: Mutex<Child>,
    stderr: Arc<Mutex<String>>,
}

impl SshBridgeState {
    fn new(child: Child, stderr: Arc<Mutex<String>>) -> Arc<Self> {
        Arc::new(Self {
            child: Mutex::new(child),
            stderr,
        })
    }

    pub(crate) fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }

    /// The tail of the bridge's stderr, when it captured anything.
    pub(crate) fn stderr_tail(&self) -> Option<String> {
        let stderr = self.stderr.lock().ok()?.trim().to_owned();
        (!stderr.is_empty()).then_some(stderr)
    }
}

impl Drop for SshBridgeState {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A spawned `ssh -T <dest> '<discovery script> exec herdr … <subcommand>'`
/// child with piped stdio and a drained stderr tail.
pub(crate) struct SshStdioBridge {
    pub(crate) state: Arc<SshBridgeState>,
    pub(crate) stdin: ChildStdin,
    pub(crate) stdout: ChildStdout,
}

/// `--session` precedes the bridge subcommand and is omitted for the default
/// session, matching Herdr's own remote client argument layout.
fn bridge_args<'a>(session_name: &'a str, subcommand: &'a str) -> Vec<&'a str> {
    let mut args = Vec::with_capacity(3);
    if session_name != DEFAULT_SESSION_NAME {
        args.extend(["--session", session_name]);
    }
    args.push(subcommand);
    args
}

/// Spawns an SSH stdio bridge running Herdr's `remote-client-bridge` against
/// `session_name`. `--session` precedes the subcommand and is omitted for the
/// default session, matching Herdr's own remote client.
pub(crate) fn spawn_ssh_bridge(
    profile: &ConnectionProfile,
    session_name: &str,
    subcommand: &str,
) -> Result<SshStdioBridge> {
    let ConnectionProfile::Ssh {
        destination,
        port,
        identity_file,
        proxy_jump,
        herdr_path,
        ..
    } = profile
    else {
        return Err(HerdrError::Protocol(
            "SSH bridge requires an SSH profile".into(),
        ));
    };
    let args = bridge_args(session_name, subcommand);
    let mut command = Command::new(system_ssh());
    add_ssh_common(
        &mut command,
        destination,
        *port,
        identity_file.as_deref(),
        proxy_jump.as_deref(),
    );
    let remote = remote_herdr_command(herdr_path, &args);
    command
        .arg("--")
        .arg(remote)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| HerdrError::Ssh("ssh bridge stdin was not piped".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| HerdrError::Ssh("ssh bridge stdout was not piped".into()))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| HerdrError::Ssh("ssh bridge stderr was not piped".into()))?;
    let stderr = Arc::new(Mutex::new(String::new()));
    thread::spawn({
        let stderr = stderr.clone();
        move || drain_stderr(stderr_pipe, stderr)
    });
    Ok(SshStdioBridge {
        state: SshBridgeState::new(child, stderr),
        stdin,
        stdout,
    })
}

/// The OpenSSH `-T` flag in `add_ssh_common` already disables the remote pty, so
/// a bridge's stderr carries only ssh diagnostics and remote Herdr errors.
const SSH_STDERR_TAIL_BYTES: usize = 4 * 1024;

fn drain_stderr(pipe: impl Read, sink: Arc<Mutex<String>>) {
    let mut reader = BufReader::new(pipe);
    let mut chunk = [0_u8; 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                if let Ok(mut sink) = sink.lock() {
                    sink.push_str(&String::from_utf8_lossy(&chunk[..count]));
                    if sink.len() > SSH_STDERR_TAIL_BYTES {
                        let keep = sink.len() - SSH_STDERR_TAIL_BYTES;
                        sink.drain(..keep);
                    }
                }
            }
        }
    }
}

fn system_ssh() -> &'static str {
    #[cfg(target_os = "windows")]
    return "ssh.exe";
    #[cfg(not(target_os = "windows"))]
    return "/usr/bin/ssh";
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalNotificationKind {
    Sound,
    Toast,
    SystemToast,
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

/// The effective connection fields `ssh -G` resolves for one config alias.
/// Imported machines persist these so they no longer depend on `~/.ssh/config`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedSshHost {
    /// `user@hostname` ready for the ssh destination argument.
    pub destination: String,
    pub port: Option<u16>,
    pub identity_file: Option<PathBuf>,
    pub proxy_jump: Option<String>,
    /// True when the alias relies on ProxyCommand, which profiles cannot
    /// reproduce; the import flow warns instead of silently breaking.
    pub has_proxy_command: bool,
}

/// Resolves an `~/.ssh/config` alias through `ssh -G` into self-contained
/// connection fields.
pub fn resolve_ssh_alias(alias: &str) -> Result<ResolvedSshHost> {
    let mut command = Command::new(system_ssh());
    command
        .arg("-G")
        .arg(alias)
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    // `ssh -G` only reads local config, but a wedged binary or a blocking
    // Include target must not strand the import UI in "importing" forever.
    let output = command_output_with_timeout(&mut command, Duration::from_secs(10))?;
    if !output.status.success() {
        return Err(HerdrError::Ssh(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let resolved = parse_ssh_g_output(&String::from_utf8_lossy(&output.stdout));
    if resolved.destination.is_empty() {
        return Err(HerdrError::Ssh(format!(
            "ssh -G {alias} produced no hostname"
        )));
    }
    Ok(resolved)
}

/// Runs a command to completion with a wall-clock deadline, killing the
/// child when the deadline passes so callers never wait forever.
fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output> {
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(_) => return Ok(child.wait_with_output()?),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HerdrError::Ssh(format!(
                    "command timed out after {}s",
                    timeout.as_secs()
                )));
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// Identity files OpenSSH probes implicitly; `ssh -G` reports them even
/// when the alias configures none.
const SSH_DEFAULT_IDENTITY_FILES: &[&str] = &[
    ".ssh/id_rsa",
    ".ssh/id_ecdsa",
    ".ssh/id_ecdsa_sk",
    ".ssh/id_ed25519",
    ".ssh/id_ed25519_sk",
    ".ssh/id_dsa",
    ".ssh/id_xmss",
];

fn parse_ssh_g_output(output: &str) -> ResolvedSshHost {
    let mut user = String::new();
    let mut hostname = String::new();
    let mut resolved = ResolvedSshHost::default();
    for line in output.lines() {
        let mut fields = line.splitn(2, char::is_whitespace);
        let (Some(key), Some(value)) = (fields.next(), fields.next()) else {
            continue;
        };
        let value = value.trim();
        match key {
            "user" => user = value.to_owned(),
            "hostname" => hostname = value.to_owned(),
            "port" => resolved.port = value.parse().ok(),
            "identityfile" if resolved.identity_file.is_none() && value != "none" => {
                // `ssh -G` lists implicit defaults alongside configured keys;
                // persisting a default would pin noise into the profile.
                let is_default = SSH_DEFAULT_IDENTITY_FILES
                    .iter()
                    .any(|default| value.ends_with(default));
                if !is_default {
                    resolved.identity_file = Some(PathBuf::from(value));
                }
            }
            "proxyjump" if value != "none" => resolved.proxy_jump = Some(value.to_owned()),
            "proxycommand" if value != "none" => resolved.has_proxy_command = true,
            _ => {}
        }
    }
    resolved.destination = if user.is_empty() {
        hostname
    } else {
        format!("{user}@{hostname}")
    };
    resolved
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
            port,
            identity_file,
            proxy_jump,
            herdr_path,
            ..
        } => {
            let attach = remote_herdr_command(herdr_path, &["session", "attach", session_name]);
            let mut arguments = vec!["ssh".to_owned(), "-t".to_owned()];
            if let Some(port) = port {
                arguments.extend(["-p".into(), port.to_string()]);
            }
            if let Some(identity_file) = identity_file {
                arguments.extend([
                    "-i".into(),
                    posix_quote(&identity_file.display().to_string()),
                ]);
            }
            if let Some(proxy_jump) = proxy_jump {
                arguments.extend(["-J".into(), posix_quote(proxy_jump)]);
            }
            arguments.push(posix_quote(destination));
            arguments.push(posix_quote(&attach));
            arguments.join(" ")
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
        proxy_jump,
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
    if let Some(proxy_jump) = proxy_jump {
        arguments.extend(["-J".into(), posix_quote(proxy_jump)]);
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

#[cfg(target_os = "windows")]
pub fn open_system_terminal(command: &str) -> Result<()> {
    Command::new("wt.exe")
        .args(["powershell.exe", "-NoExit", "-Command", command])
        .spawn()
        .map(|_| ())
        .map_err(|error| HerdrError::Command(format!("failed to open Windows Terminal: {error}")))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn open_system_terminal(command: &str) -> Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let script = format!("{command}; exec {}", posix_quote(&shell));
    let candidates: &[(&str, &[&str])] = &[
        ("x-terminal-emulator", &["-e", "sh", "-lc"]),
        ("gnome-terminal", &["--", "sh", "-lc"]),
        ("konsole", &["-e", "sh", "-lc"]),
        ("kitty", &["sh", "-lc"]),
        ("xterm", &["-e", "sh", "-lc"]),
    ];
    for (program, arguments) in candidates {
        match Command::new(program).args(*arguments).arg(&script).spawn() {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(HerdrError::Command(format!(
                    "failed to open {program}: {error}"
                )));
            }
        }
    }
    Err(HerdrError::Command(
        "no supported terminal emulator was found".into(),
    ))
}

#[cfg(test)]
mod tests;
