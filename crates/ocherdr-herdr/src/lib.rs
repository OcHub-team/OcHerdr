//! Public Herdr transports used by OcHerdr.
//!
//! This crate intentionally does not know the private bincode client protocol.
//! State travels over Herdr's public NDJSON socket; terminal bytes travel through
//! `herdr terminal session observe/control`.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use ocherdr_core::{ConnectionProfile, HierarchySnapshot, SessionSummary};
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
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, HerdrError>;

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
            let remote = std::iter::once(herdr_path.as_str())
                .chain(args.iter().copied())
                .map(posix_quote)
                .collect::<Vec<_>>()
                .join(" ");
            command.arg("--").arg(remote);
            Ok(command)
        }
    }
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
        Ok(EventSubscription::spawn(self.subscribe()?))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

pub fn request_socket(socket_path: &Path, method: &str, params: Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket_path)?;
    let id = format!("ocherdr-{}", REQUEST_ID.fetch_add(1, Ordering::Relaxed));
    serde_json::to_writer(
        &mut stream,
        &json!({ "id": id, "method": method, "params": params }),
    )?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.trim().is_empty() {
        return Err(HerdrError::Protocol("empty API response".into()));
    }
    let value: Value = serde_json::from_str(&line)?;
    if let Some(error) = value.get("error") {
        return Err(HerdrError::Api {
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
        });
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| HerdrError::Protocol("API response is missing `result`".into()))
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

pub struct EventStream {
    reader: BufReader<UnixStream>,
}

impl EventStream {
    fn connect(socket_path: &Path) -> Result<Self> {
        let mut stream = UnixStream::connect(socket_path)?;
        let id = format!(
            "ocherdr-events-{}",
            REQUEST_ID.fetch_add(1, Ordering::Relaxed)
        );
        let types = [
            "workspace.created",
            "workspace.updated",
            "workspace.metadata_updated",
            "workspace.renamed",
            "workspace.moved",
            "workspace.reordered",
            "workspace.closed",
            "workspace.focused",
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
            "pane.agent_status_changed",
            "pane.scroll_changed",
            "layout.updated",
        ];
        let subscriptions = types
            .into_iter()
            .map(|kind| json!({ "type": kind }))
            .collect::<Vec<_>>();
        serde_json::to_writer(
            &mut stream,
            &json!({
                "id": id,
                "method": "events.subscribe",
                "params": { "subscriptions": subscriptions }
            }),
        )?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    pub fn next_event(&mut self) -> Result<Option<Value>> {
        let mut line = String::new();
        let count = self.reader.read_line(&mut line)?;
        if count == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&line)?))
    }
}

pub struct EventSubscription {
    events: Receiver<Result<Value>>,
}

impl EventSubscription {
    fn spawn(mut stream: EventStream) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        thread::spawn(move || {
            loop {
                match stream.next_event() {
                    Ok(Some(event)) => {
                        if event_tx.send(Ok(event)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = event_tx.send(Err(error));
                        break;
                    }
                }
            }
        });
        Self { events: event_rx }
    }

    pub fn try_event(&self) -> Result<Option<Value>> {
        match self.events.try_recv() {
            Ok(event) => event.map(Some),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMode {
    Observe,
    ControlTakeover,
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

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum TerminalControlCommand<'a> {
    #[serde(rename = "terminal.input")]
    Input { text: &'a str },
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
    Input(String),
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
            Self::Input(text) => {
                serde_json::to_writer(&mut *stdin, &TerminalControlCommand::Input { text })?
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
        let mut args = Vec::<String>::new();
        if session_name != "default" {
            args.extend(["--session".into(), session_name.into()]);
        }
        args.extend([
            "terminal".into(),
            "session".into(),
            match mode {
                TerminalMode::Observe => "observe",
                TerminalMode::ControlTakeover => "control",
            }
            .into(),
            target.into(),
        ]);
        if mode == TerminalMode::ControlTakeover {
            args.push("--takeover".into());
        }
        args.extend([
            "--cols".into(),
            cols.max(1).to_string(),
            "--rows".into(),
            rows.max(1).to_string(),
        ]);
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

    pub fn read_frame(&mut self) -> Result<Option<TerminalFrame>> {
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
                Ok(Some(TerminalFrame {
                    seq,
                    width,
                    height,
                    full,
                    bytes,
                }))
            }
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
    frames: Receiver<Result<TerminalFrame>>,
    process_id: Arc<AtomicU32>,
}

impl TerminalSession {
    pub fn spawn(
        profile: ConnectionProfile,
        session_name: String,
        target: String,
        mode: TerminalMode,
        cols: u16,
        rows: u16,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel::<TerminalCommand>();
        let (frame_tx, frame_rx) = mpsc::channel::<Result<TerminalFrame>>();
        let process_id = Arc::new(AtomicU32::new(0));
        let worker_process_id = process_id.clone();
        thread::spawn(move || {
            let stream = TerminalStream::spawn(&profile, &session_name, &target, mode, cols, rows);
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = frame_tx.send(Err(error));
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
                match stream.read_frame() {
                    Ok(Some(frame)) => {
                        if frame_tx.send(Ok(frame)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = frame_tx.send(Err(error));
                        break;
                    }
                }
            }
            drop(stream);
            worker_process_id.store(0, Ordering::Release);
            let _ = writer.join();
        });
        Self {
            commands: command_tx,
            frames: frame_rx,
            process_id,
        }
    }

    pub fn send(&self, command: TerminalCommand) -> Result<()> {
        self.commands
            .send(command)
            .map_err(|_| HerdrError::TerminalClosed("terminal worker stopped".into()))
    }

    pub fn try_frame(&self) -> Result<Option<TerminalFrame>> {
        match self.frames.try_recv() {
            Ok(frame) => frame.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Ok(None),
        }
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
    parse_ssh_hosts(&fs::read_to_string(home.join(".ssh/config")).unwrap_or_default())
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
    let attach = format!(
        "{} session attach {}",
        posix_quote(profile.herdr_path()),
        posix_quote(session_name)
    );
    match profile {
        ConnectionProfile::Local { .. } => attach,
        ConnectionProfile::Ssh { destination, .. } => {
            format!(
                "ssh -t {} {}",
                posix_quote(destination),
                posix_quote(&attach)
            )
        }
    }
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
    fn quotes_remote_arguments_without_shell_injection() {
        assert_eq!(posix_quote("plain/path-1"), "plain/path-1");
        assert_eq!(posix_quote("a b'c"), "'a b'\"'\"'c'");
        assert_eq!(posix_quote("$(touch nope)"), "'$(touch nope)'");
    }

    #[test]
    fn parses_only_concrete_ssh_hosts() {
        let hosts = parse_ssh_hosts(
            "Host *\n  ServerAliveInterval 15\nHost work work-alt\nHost build-?\nHost work\n",
        );
        assert_eq!(hosts, vec!["work", "work-alt"]);
    }

    #[test]
    fn attach_command_keeps_session_name_quoted() {
        let profile = ConnectionProfile::Ssh {
            id: "server".into(),
            label: "Server".into(),
            destination: "deploy@example.com".into(),
            port: None,
            identity_file: None,
            herdr_path: "herdr".into(),
        };
        assert_eq!(
            attach_command(&profile, "work one"),
            "ssh -t deploy@example.com 'herdr session attach '\"'\"'work one'\"'\"''"
        );
    }
}
