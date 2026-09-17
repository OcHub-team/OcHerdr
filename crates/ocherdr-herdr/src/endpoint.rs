//! Client-owned shell connection over Herdr's stable endpoint protocol
//! (generation 1).
//!
//! One endpoint connection carries the session snapshot feed, the composited
//! pane surface, semantic pane input, and an endpoint-scoped JSON API channel.
//! Unlike the numbered private codecs, the handshake negotiates named codecs
//! instead of comparing build versions, so this path is intended to stay
//! compatible across Herdr releases.

use std::collections::{HashMap, VecDeque};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
#[cfg(windows)]
use uds_windows::UnixStream;

use futures::SinkExt as _;
use futures::channel::mpsc::{self as futures_mpsc, Receiver};
use serde_json::{Value, json};
use std::sync::mpsc::{self as std_mpsc, Sender as StdSender};

use crate::endpoint_v1 as ep;
use crate::{HerdrError, Result, TerminalEndpoint};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const ENDPOINT_EVENT_QUEUE_CAPACITY: usize = 4;

/// Outcome of a completed `endpoint.hello.v1` negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointWelcome {
    pub server_version: String,
    pub methods: Vec<String>,
    pub capabilities: Vec<String>,
}

/// One server-pushed unit on an endpoint connection.
#[derive(Debug, Clone, PartialEq)]
pub enum EndpointEvent {
    /// Connection negotiated; carries the advertised methods/capabilities.
    Welcome(EndpointWelcome),
    /// `shell.snapshot.v1` payload: the full session model.
    Snapshot(Box<crate::endpoint_v1::ClientShellSnapshot>),
    /// Complete composited pane surface for the client's selected tab.
    Surface(Box<crate::endpoint_v1::PaneSurfaceFrame>),
    /// Incremental cells against the committed surface.
    SurfacePatch(Box<crate::endpoint_v1::PaneSurfacePatch>),
    /// Fully reassembled endpoint response body for `request_id`.
    Response { request_id: String, data: Vec<u8> },
    /// Legacy private-protocol notify routed to this client.
    Notify {
        kind: crate::TerminalNotificationKind,
        message: String,
        body: Option<String>,
    },
    /// Semantic notification (agent needs attention, finished, …).
    SemanticNotification(crate::endpoint_v1::SemanticNotification),
    /// Unrecoverable shell error the server wants displayed.
    ShellError(String),
    /// OSC 52 clipboard payload emitted by a pane application.
    Clipboard(String),
    /// Outer window title update requested by the session.
    WindowTitle(Option<String>),
    /// Whether the client should capture host mouse input.
    MouseCapture { enabled: bool, sgr_pixels: bool },
    /// The focused pane wants every key reported (Kitty report-all).
    KeyboardReportAll(bool),
    /// Pane-originated BEL characters to ring on the host.
    Bell(u16),
    /// Runtime sound config changed on disk.
    ReloadSoundConfig,
    /// Server is shutting down; the stream ends after this event.
    Shutdown(Option<String>),
}

/// Client → server commands on an endpoint connection.
#[derive(Debug, Clone, PartialEq)]
pub enum EndpointCommand {
    /// Declare the pane surface geometry (and exact cell metrics).
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    /// Endpoint-scoped public API request.
    Request {
        request_id: String,
        method: String,
        params: Value,
    },
    /// Semantic input for one pane.
    PaneInput {
        pane_id: String,
        events: Vec<crate::endpoint_v1::ClientPaneInputEvent>,
    },
    /// Semantic input for the active popup terminal.
    PopupInput {
        terminal_id: String,
        events: Vec<crate::endpoint_v1::ClientPaneInputEvent>,
    },
    /// Host window focus baseline.
    Focus(bool),
    /// Host appearance/palette update.
    HostTheme(crate::endpoint_v1::ClientHostThemeUpdate),
    /// Shell mouse-capture preference.
    MouseCapture(bool),
    /// Clipboard image staged for a pane (or popup) paste.
    ClipboardImage {
        target: EndpointClipboardTarget,
        extension: String,
        bytes: Vec<u8>,
    },
    /// Application-level liveness probe.
    HealthPing,
    /// Internal reply to a server probe; sent by the session worker.
    HealthPong(String),
    /// Graceful disconnect.
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointClipboardTarget {
    Pane(String),
    Popup(String),
}

/// Herdr's endpoint request lane is single-flight: the server answers any
/// overlapping call with `endpoint_busy`. Requests therefore queue here and
/// the reader releases the next one only when the in-flight call's final
/// response chunk arrives.
#[derive(Default)]
struct EndpointRequestLane {
    in_flight: Option<String>,
    queued: VecDeque<(String, String, Value)>,
}

impl EndpointRequestLane {
    /// Takes the lane or queues behind the in-flight request. Returns the
    /// command to write when the lane was free.
    fn submit(
        &mut self,
        request_id: String,
        method: String,
        params: Value,
    ) -> Option<EndpointCommand> {
        if self.in_flight.is_some() {
            self.queued.push_back((request_id, method, params));
            return None;
        }
        self.in_flight = Some(request_id.clone());
        Some(EndpointCommand::Request {
            request_id,
            method,
            params,
        })
    }

    /// Releases `finished` and writes the next queued request, if any.
    fn advance(&mut self, finished: &str, commands: &StdSender<EndpointCommand>) {
        if self.in_flight.as_deref() != Some(finished) {
            return;
        }
        self.in_flight = None;
        if let Some((request_id, method, params)) = self.queued.pop_front()
            && commands
                .send(EndpointCommand::Request {
                    request_id: request_id.clone(),
                    method,
                    params,
                })
                .is_ok()
        {
            self.in_flight = Some(request_id);
        }
    }
}

struct EndpointWireReader {
    stream: UnixStream,
    boot_id: Arc<Mutex<String>>,
    pending_responses: HashMap<String, Vec<u8>>,
}

struct EndpointWireWriter {
    stream: UnixStream,
    boot_id: Arc<Mutex<String>>,
    requests: Arc<Mutex<EndpointRequestLane>>,
    resubmit: StdSender<EndpointCommand>,
}

fn connect(
    endpoint: &TerminalEndpoint,
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
    requests: Arc<Mutex<EndpointRequestLane>>,
    resubmit: StdSender<EndpointCommand>,
) -> Result<(
    EndpointWireReader,
    EndpointWireWriter,
    EndpointWelcome,
    Option<Box<ep::ClientShellSnapshot>>,
)> {
    let mut stream = endpoint.connect()?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;

    let hello = ep::EndpointClientHello::new(
        ep::ClientSurfaceSize {
            cols: cols.max(1),
            rows: rows.max(1),
        },
        cell_width_px,
        cell_height_px,
    );
    let hello = ep::ClientMessage::EndpointControl {
        kind: ep::ENDPOINT_HELLO_KIND.into(),
        data: serde_json::to_string(&hello)?,
    };
    ep::write_message(&mut stream, &hello).map_err(endpoint_protocol_error)?;

    let welcome = read_welcome(&mut stream)?;

    // Endpoint requests carry the snapshot's boot_id, and the caller can
    // queue activation requests the moment spawn returns. Capture the
    // initial snapshot now so those requests always find a boot_id. The
    // snapshot itself is still emitted on the event stream.
    let boot_id = Arc::new(Mutex::new(String::new()));
    let snapshot = read_initial_snapshot(&mut stream, &boot_id)?;

    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    let writer = stream.try_clone()?;
    Ok((
        EndpointWireReader {
            stream,
            boot_id: boot_id.clone(),
            pending_responses: HashMap::new(),
        },
        EndpointWireWriter {
            stream: writer,
            boot_id,
            requests,
            resubmit,
        },
        welcome,
        snapshot,
    ))
}

/// Reads until the first `shell.snapshot.v1` lands, captures its `boot_id`,
/// and returns the snapshot for emission on the event stream. Server health
/// pings get an inline pong; anything else this early is discarded. A
/// timeout leaves the boot id empty instead of failing the connection —
/// requests then fail locally until the reader sees a snapshot, which the
/// request lane absorbs.
fn read_initial_snapshot(
    stream: &mut UnixStream,
    boot_id: &Arc<Mutex<String>>,
) -> Result<Option<Box<ep::ClientShellSnapshot>>> {
    loop {
        let message = match ep::read_message(stream, ep::MAX_GRAPHICS_FRAME_SIZE) {
            Ok(message) => message,
            Err(ep::FramingError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(endpoint_protocol_error(error)),
        };
        match message {
            ep::ServerMessage::EndpointControl { kind, data } => match kind.as_str() {
                ep::ENDPOINT_SNAPSHOT_KIND => {
                    let snapshot: ep::ClientShellSnapshot =
                        serde_json::from_str(&data).map_err(|error| {
                            HerdrError::Protocol(format!(
                                "invalid endpoint snapshot payload: {error}"
                            ))
                        })?;
                    *boot_id.lock().unwrap() = snapshot.boot_id.clone();
                    return Ok(Some(Box::new(snapshot)));
                }
                ep::HEALTH_PING_KIND => {
                    ep::write_message(
                        stream,
                        &ep::ClientMessage::EndpointControl {
                            kind: ep::HEALTH_PONG_KIND.into(),
                            data,
                        },
                    )
                    .map_err(endpoint_protocol_error)?;
                }
                _ => {}
            },
            ep::ServerMessage::ClientShellSnapshot(snapshot) => {
                *boot_id.lock().unwrap() = snapshot.boot_id.clone();
                return Ok(Some(snapshot));
            }
            _ => {}
        }
    }
}

fn read_welcome(stream: &mut UnixStream) -> Result<EndpointWelcome> {
    let message: ep::ServerMessage =
        ep::read_message(stream, ep::MAX_FRAME_SIZE).map_err(endpoint_protocol_error)?;
    let ep::ServerMessage::EndpointControl { kind, data } = message else {
        return Err(HerdrError::Protocol(format!(
            "expected {} endpoint welcome, received {message:?}",
            ep::ENDPOINT_WELCOME_KIND
        )));
    };
    if kind != ep::ENDPOINT_WELCOME_KIND {
        return Err(HerdrError::Protocol(format!(
            "expected {} endpoint welcome, received {kind}",
            ep::ENDPOINT_WELCOME_KIND
        )));
    }
    let welcome: ep::EndpointServerWelcome = serde_json::from_str(&data)?;
    if let Some(error) = welcome.error {
        return Err(HerdrError::Protocol(format!(
            "endpoint rejected generation {} hello: {}: {}",
            ep::ENDPOINT_PROTOCOL_GENERATION,
            error.code,
            error.message
        )));
    }
    if welcome.generation != ep::ENDPOINT_PROTOCOL_GENERATION {
        return Err(HerdrError::Protocol(format!(
            "server negotiated endpoint generation {}; we only speak {}",
            welcome.generation,
            ep::ENDPOINT_PROTOCOL_GENERATION
        )));
    }
    Ok(EndpointWelcome {
        server_version: welcome.server_version,
        methods: welcome.methods,
        capabilities: welcome.capabilities,
    })
}

impl EndpointWireReader {
    /// Blocks for the next endpoint event. `Ok(None)` means the server closed
    /// the stream cleanly. Health pings are answered through `commands` and
    /// never surface here; completed endpoint responses release the request
    /// lane through the same channel.
    fn read_event(
        &mut self,
        commands: &StdSender<EndpointCommand>,
        requests: &Arc<Mutex<EndpointRequestLane>>,
    ) -> Result<Option<EndpointEvent>> {
        loop {
            let message: ep::ServerMessage =
                match ep::read_message(&mut self.stream, ep::MAX_GRAPHICS_FRAME_SIZE) {
                    Ok(message) => message,
                    Err(ep::FramingError::UnexpectedEof) => return Ok(None),
                    Err(error) => return Err(endpoint_protocol_error(error)),
                };
            match message {
                ep::ServerMessage::PaneSurface(frame) => {
                    return Ok(Some(EndpointEvent::Surface(Box::new(frame))));
                }
                ep::ServerMessage::PaneSurfacePatch(patch) => {
                    return Ok(Some(EndpointEvent::SurfacePatch(Box::new(patch))));
                }
                ep::ServerMessage::EndpointControl { kind, data } => match kind.as_str() {
                    ep::ENDPOINT_SNAPSHOT_KIND => {
                        let snapshot: ep::ClientShellSnapshot = serde_json::from_str(&data)
                            .map_err(|error| {
                                HerdrError::Protocol(format!(
                                    "invalid endpoint snapshot payload: {error}"
                                ))
                            })?;
                        *self.boot_id.lock().unwrap() = snapshot.boot_id.clone();
                        return Ok(Some(EndpointEvent::Snapshot(Box::new(snapshot))));
                    }
                    ep::HEALTH_PING_KIND => {
                        let _ = commands.send(EndpointCommand::HealthPong(data));
                        continue;
                    }
                    _ => continue,
                },
                ep::ServerMessage::ClientShellSnapshot(snapshot) => {
                    // Same payload as the named control, on the bincode lane.
                    *self.boot_id.lock().unwrap() = snapshot.boot_id.clone();
                    return Ok(Some(EndpointEvent::Snapshot(snapshot)));
                }
                ep::ServerMessage::ClientShellEndpointResponseChunk {
                    request_id,
                    final_chunk,
                    data,
                    ..
                } => {
                    self.pending_responses
                        .entry(request_id.clone())
                        .or_default()
                        .extend_from_slice(&data);
                    if !final_chunk {
                        continue;
                    }
                    let data = self
                        .pending_responses
                        .remove(&request_id)
                        .unwrap_or_default();
                    requests.lock().unwrap().advance(&request_id, commands);
                    return Ok(Some(EndpointEvent::Response { request_id, data }));
                }
                ep::ServerMessage::SemanticNotification(notification) => {
                    return Ok(Some(EndpointEvent::SemanticNotification(notification)));
                }
                ep::ServerMessage::Notify {
                    kind,
                    message,
                    body,
                } => {
                    let kind = match kind {
                        ep::NotifyKind::Sound => crate::TerminalNotificationKind::Sound,
                        ep::NotifyKind::Toast => crate::TerminalNotificationKind::Toast,
                        ep::NotifyKind::SystemToast => crate::TerminalNotificationKind::SystemToast,
                    };
                    return Ok(Some(EndpointEvent::Notify {
                        kind,
                        message,
                        body,
                    }));
                }
                ep::ServerMessage::ClientShellError { message } => {
                    return Ok(Some(EndpointEvent::ShellError(message)));
                }
                ep::ServerMessage::Clipboard { data } => {
                    return Ok(Some(EndpointEvent::Clipboard(data)));
                }
                ep::ServerMessage::WindowTitle { title } => {
                    return Ok(Some(EndpointEvent::WindowTitle(title)));
                }
                ep::ServerMessage::MouseCapture {
                    enabled,
                    sgr_pixels,
                } => {
                    return Ok(Some(EndpointEvent::MouseCapture {
                        enabled,
                        sgr_pixels,
                    }));
                }
                ep::ServerMessage::ClientShellKeyboardReportAll { enabled } => {
                    return Ok(Some(EndpointEvent::KeyboardReportAll(enabled)));
                }
                ep::ServerMessage::TerminalBell { count } => {
                    return Ok(Some(EndpointEvent::Bell(count)));
                }
                ep::ServerMessage::ReloadSoundConfig => {
                    return Ok(Some(EndpointEvent::ReloadSoundConfig));
                }
                ep::ServerMessage::ServerShutdown { reason } => {
                    return Ok(Some(EndpointEvent::Shutdown(reason)));
                }
                // Direct-attach lanes (Terminal/Graphics/GraphicsFile/…) do not
                // apply to endpoint shells; decode and drop them.
                ep::ServerMessage::Welcome { .. }
                | ep::ServerMessage::Terminal(_)
                | ep::ServerMessage::Graphics { .. }
                | ep::ServerMessage::GraphicsFile { .. }
                | ep::ServerMessage::GraphicsTransmissionRetired { .. }
                | ep::ServerMessage::DirectTerminalKeyboardProtocol { .. } => continue,
            }
        }
    }
}

impl EndpointWireWriter {
    fn send(&mut self, command: EndpointCommand) -> Result<()> {
        let release = command == EndpointCommand::Detach;
        let request_id = match &command {
            EndpointCommand::Request { request_id, .. } => Some(request_id.clone()),
            _ => None,
        };
        let result = self.write(command);
        match &result {
            // Only an I/O failure means the wire is broken. Local failures —
            // a missing boot_id, an oversized frame, a serde error — never
            // reached the server, so the socket stays up and the request
            // lane advances past a call that can never be answered.
            Err(HerdrError::Io(_)) => {
                let _ = self.stream.shutdown(std::net::Shutdown::Both);
            }
            Err(_) => {
                if let Some(request_id) = request_id {
                    self.requests
                        .lock()
                        .unwrap()
                        .advance(&request_id, &self.resubmit);
                }
            }
            Ok(()) => {}
        }
        if release {
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
        result
    }

    fn write(&mut self, command: EndpointCommand) -> Result<()> {
        let message = match command {
            EndpointCommand::Resize {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse,
            } => ep::ClientMessage::ClientShellResize {
                cell_width_px,
                cell_height_px,
                surface_size: ep::ClientSurfaceSize {
                    cols: cols.max(1),
                    rows: rows.max(1),
                },
                pixel_mouse,
            },
            EndpointCommand::Request {
                request_id,
                method,
                params,
            } => ep::ClientMessage::ClientShellEndpointRequest {
                boot_id: self.boot_id()?,
                request: serde_json::to_string(&json!({
                    "id": request_id,
                    "method": method,
                    "params": params,
                }))?,
            },
            EndpointCommand::PaneInput { pane_id, events } => {
                ep::ClientMessage::ClientShellPaneInput { pane_id, events }
            }
            EndpointCommand::PopupInput {
                terminal_id,
                events,
            } => ep::ClientMessage::ClientShellPopupInput {
                terminal_id,
                events,
            },
            EndpointCommand::Focus(focused) => ep::ClientMessage::ClientShellFocus { focused },
            EndpointCommand::HostTheme(update) => {
                ep::ClientMessage::ClientShellHostTheme { update }
            }
            EndpointCommand::MouseCapture(enabled) => {
                ep::ClientMessage::ClientShellMouseCapture { enabled }
            }
            EndpointCommand::ClipboardImage {
                target,
                extension,
                bytes,
            } => {
                if bytes.len() > ep::MAX_CLIPBOARD_IMAGE_PAYLOAD {
                    return Err(HerdrError::Protocol(format!(
                        "clipboard image is {} bytes; endpoint maximum is {} bytes",
                        bytes.len(),
                        ep::MAX_CLIPBOARD_IMAGE_PAYLOAD
                    )));
                }
                let target = match target {
                    EndpointClipboardTarget::Pane(pane_id) => {
                        ep::ClientClipboardImageTarget::Pane(pane_id)
                    }
                    EndpointClipboardTarget::Popup(terminal_id) => {
                        ep::ClientClipboardImageTarget::Popup(terminal_id)
                    }
                };
                ep::ClientMessage::ClipboardImage {
                    target,
                    extension,
                    data: bytes,
                }
            }
            EndpointCommand::HealthPing => ep::ClientMessage::EndpointControl {
                kind: ep::HEALTH_PING_KIND.into(),
                data: ep::HEALTH_PROBE_DATA.into(),
            },
            EndpointCommand::HealthPong(data) => ep::ClientMessage::EndpointControl {
                kind: ep::HEALTH_PONG_KIND.into(),
                data,
            },
            EndpointCommand::Detach => ep::ClientMessage::Detach,
        };
        ep::write_message(&mut self.stream, &message).map_err(endpoint_protocol_error)
    }

    fn boot_id(&self) -> Result<String> {
        let boot_id = self.boot_id.lock().unwrap().clone();
        if boot_id.is_empty() {
            return Err(HerdrError::Protocol(
                "endpoint requests require the initial snapshot boot_id".into(),
            ));
        }
        Ok(boot_id)
    }
}

fn endpoint_protocol_error(error: ep::FramingError) -> HerdrError {
    match error {
        ep::FramingError::Io(error) => HerdrError::Io(error),
        other => HerdrError::Protocol(other.to_string()),
    }
}

/// A live endpoint connection: one per `(profile, session)` owner.
pub struct EndpointSession {
    commands: StdSender<EndpointCommand>,
    request_counter: Arc<AtomicU64>,
    requests: Arc<Mutex<EndpointRequestLane>>,
    alive: Arc<AtomicBool>,
}

/// Cloneable writer half of an [`EndpointSession`]; panes share one session
/// connection and send through their own handle.
#[derive(Clone)]
pub struct EndpointHandle {
    commands: StdSender<EndpointCommand>,
    request_counter: Arc<AtomicU64>,
    requests: Arc<Mutex<EndpointRequestLane>>,
    alive: Arc<AtomicBool>,
}

impl EndpointHandle {
    pub fn send(&self, command: EndpointCommand) -> Result<()> {
        self.commands
            .send(command)
            .map_err(|_| HerdrError::TerminalClosed("endpoint worker stopped".into()))
    }

    /// Queues an endpoint-scoped API request and returns its request id.
    /// Requests are serialized onto Herdr's single-flight lane: the next
    /// call is written only after the in-flight call's response arrives.
    pub fn request(&self, method: &str, params: Value) -> Result<String> {
        let request_id = format!(
            "ocherdr-ep-{}",
            self.request_counter.fetch_add(1, Ordering::Relaxed) + 1
        );
        let command =
            self.requests
                .lock()
                .unwrap()
                .submit(request_id.clone(), method.to_owned(), params);
        if let Some(command) = command {
            self.send(command)?;
        }
        Ok(request_id)
    }

    pub fn pane_input(
        &self,
        pane_id: &str,
        events: Vec<crate::endpoint_v1::ClientPaneInputEvent>,
    ) -> Result<()> {
        self.send(EndpointCommand::PaneInput {
            pane_id: pane_id.to_owned(),
            events,
        })
    }

    pub fn is_closed(&self) -> bool {
        !self.alive.load(Ordering::Acquire)
    }
}

/// A small queue deliberately applies backpressure to Herdr's endpoint reader.
/// Full surfaces can be large; an unbounded queue made a temporarily stalled
/// GPUI retain every frame for every host.
pub type EndpointEventReceiver = Receiver<Result<EndpointEvent>>;

impl EndpointSession {
    /// Connects and performs `endpoint.hello.v1`. The negotiated welcome is the
    /// first item delivered on the event receiver.
    pub fn spawn(
        endpoint: TerminalEndpoint,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> (Self, EndpointEventReceiver) {
        let (command_tx, command_rx) = std_mpsc::channel::<EndpointCommand>();
        let (mut event_tx, event_rx) =
            futures_mpsc::channel::<Result<EndpointEvent>>(ENDPOINT_EVENT_QUEUE_CAPACITY);
        let alive = Arc::new(AtomicBool::new(true));
        let requests = Arc::new(Mutex::new(EndpointRequestLane::default()));
        let worker_alive = alive.clone();
        let worker_tx = command_tx.clone();
        let worker_requests = requests.clone();
        thread::spawn(move || {
            let connection = connect(
                &endpoint,
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                worker_requests.clone(),
                worker_tx.clone(),
            );
            let (mut reader, mut writer, welcome, snapshot) = match connection {
                Ok(connection) => connection,
                Err(error) => {
                    worker_alive.store(false, Ordering::Release);
                    let _ = futures::executor::block_on(event_tx.send(Err(error)));
                    return;
                }
            };
            if futures::executor::block_on(event_tx.send(Ok(EndpointEvent::Welcome(welcome))))
                .is_err()
            {
                return;
            }
            // connect() already consumed the initial snapshot to capture its
            // boot_id; re-emit it so the stream keeps its documented shape.
            if let Some(snapshot) = snapshot
                && futures::executor::block_on(event_tx.send(Ok(EndpointEvent::Snapshot(snapshot))))
                    .is_err()
            {
                return;
            }
            let pong_tx = worker_tx;
            let _writer = thread::spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    let release = command == EndpointCommand::Detach;
                    if writer.send(command).is_err() || release {
                        break;
                    }
                }
            });
            loop {
                match reader.read_event(&pong_tx, &worker_requests) {
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
                request_counter: Arc::new(AtomicU64::new(0)),
                requests,
                alive,
            },
            event_rx,
        )
    }

    /// A shareable handle into this connection's command channel.
    pub fn handle(&self) -> EndpointHandle {
        EndpointHandle {
            commands: self.commands.clone(),
            request_counter: self.request_counter.clone(),
            requests: self.requests.clone(),
            alive: self.alive.clone(),
        }
    }

    pub fn send(&self, command: EndpointCommand) -> Result<()> {
        self.handle().send(command)
    }

    /// Queues an endpoint-scoped API request and returns its request id.
    /// Queues an endpoint-scoped API request and returns its request id.
    /// Requests are serialized onto Herdr's single-flight lane: the next
    /// call is written only after the in-flight call's response arrives.
    pub fn request(&self, method: &str, params: Value) -> Result<String> {
        let request_id = format!(
            "ocherdr-ep-{}",
            self.request_counter.fetch_add(1, Ordering::Relaxed) + 1
        );
        let command =
            self.requests
                .lock()
                .unwrap()
                .submit(request_id.clone(), method.to_owned(), params);
        if let Some(command) = command {
            self.send(command)?;
        }
        Ok(request_id)
    }

    /// `client_shell.surface.set`: starts or stops surface streaming.
    pub fn set_surface_active(&self, active: bool) -> Result<String> {
        self.request("client_shell.surface.set", json!({ "active": active }))
    }

    pub fn is_closed(&self) -> bool {
        !self.alive.load(Ordering::Acquire)
    }
}

impl Drop for EndpointSession {
    fn drop(&mut self) {
        let _ = self.commands.send(EndpointCommand::Detach);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_lane_serializes_calls() {
        let (tx, rx) = std_mpsc::channel();
        let mut lane = EndpointRequestLane::default();

        // The first call takes the lane; everything else queues silently.
        assert!(matches!(
            lane.submit("r1".into(), "a".into(), Value::Null),
            Some(EndpointCommand::Request { .. })
        ));
        assert!(lane.submit("r2".into(), "b".into(), Value::Null).is_none());
        assert!(lane.submit("r3".into(), "c".into(), Value::Null).is_none());
        assert_eq!(lane.queued.len(), 2);

        // Unrelated responses must not release the lane.
        lane.advance("unrelated", &tx);
        assert!(rx.try_recv().is_err());

        // Completing r1 writes r2, completing r2 writes r3, in order.
        lane.advance("r1", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(EndpointCommand::Request { request_id, .. }) if request_id == "r2"
        ));
        lane.advance("r2", &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(EndpointCommand::Request { request_id, .. }) if request_id == "r3"
        ));

        // Drained: the lane is free for the next call.
        lane.advance("r3", &tx);
        assert!(rx.try_recv().is_err());
        assert!(lane.in_flight.is_none());
        assert!(matches!(
            lane.submit("r4".into(), "d".into(), Value::Null),
            Some(EndpointCommand::Request { .. })
        ));
    }

    #[test]
    fn request_lane_recovers_when_writer_is_gone() {
        let (tx, rx) = std_mpsc::channel::<EndpointCommand>();
        let mut lane = EndpointRequestLane::default();
        assert!(lane.submit("r1".into(), "a".into(), Value::Null).is_some());
        assert!(lane.submit("r2".into(), "b".into(), Value::Null).is_none());
        // The writer stopped (dead socket): advancing drops the queued call
        // and leaves the lane clean rather than stuck on a phantom request.
        drop(rx);
        lane.advance("r1", &tx);
        assert!(lane.in_flight.is_none());
    }
}
