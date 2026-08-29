//! Version-neutral facade over Herdr's versioned private terminal codecs.
//!
//! The rest of OcHerdr must not depend on a versioned wire enum. Adding a future
//! protocol is intentionally confined to this module plus a new `private_vXX` schema.

use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::private_v20 as v20;
use crate::{
    HerdrError, Result, TerminalCommand, TerminalEndpoint, TerminalEvent, TerminalFrame,
    TerminalScrollDirection,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
pub(crate) const SUPPORTED_VERSIONS: &[u32] = &[v20::PROTOCOL_VERSION];
pub(crate) const MAX_CLIPBOARD_IMAGE_BYTES: usize = v20::MAX_CLIPBOARD_IMAGE_PAYLOAD;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolCodec {
    V20,
}

impl ProtocolCodec {
    fn for_version(version: u32) -> Result<Self> {
        match version {
            v20::PROTOCOL_VERSION => Ok(Self::V20),
            _ => Err(HerdrError::Protocol(format!(
                "Herdr private protocol {version} is not supported by this OcHerdr build; supported versions: {SUPPORTED_VERSIONS:?}"
            ))),
        }
    }

    #[cfg(test)]
    fn version(self) -> u32 {
        match self {
            Self::V20 => v20::PROTOCOL_VERSION,
        }
    }
}

pub(crate) struct TerminalWireReader {
    codec: ProtocolCodec,
    stream: UnixStream,
    last_sequence: Option<u64>,
}

pub(crate) struct TerminalWireWriter {
    codec: ProtocolCodec,
    stream: UnixStream,
}

pub(crate) struct TerminalConnect<'a> {
    pub(crate) endpoint: &'a TerminalEndpoint,
    pub(crate) protocol: u32,
    pub(crate) target: &'a str,
    pub(crate) mode: crate::TerminalMode,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) cell_width_px: u32,
    pub(crate) cell_height_px: u32,
}

pub(crate) fn connect(
    request: TerminalConnect<'_>,
) -> Result<(TerminalWireReader, TerminalWireWriter)> {
    let codec = ProtocolCodec::for_version(request.protocol)?;
    let mut stream = request.endpoint.connect()?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;

    match codec {
        ProtocolCodec::V20 => connect_v20(
            &mut stream,
            request.target,
            request.mode,
            request.cols,
            request.rows,
            request.cell_width_px,
            request.cell_height_px,
        )?,
    }

    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    let writer = stream.try_clone()?;
    Ok((
        TerminalWireReader {
            codec,
            stream,
            last_sequence: None,
        },
        TerminalWireWriter {
            codec,
            stream: writer,
        },
    ))
}

fn connect_v20(
    stream: &mut UnixStream,
    target: &str,
    mode: crate::TerminalMode,
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
) -> Result<()> {
    v20::write_message(
        stream,
        &v20::ClientMessage::Hello {
            version: v20::PROTOCOL_VERSION,
            cols: cols.max(1),
            rows: rows.max(1),
            cell_width_px,
            cell_height_px,
            requested_encoding: v20::RenderEncoding::TerminalAnsi,
            keybindings: v20::ClientKeybindings::Server,
            launch_mode: v20::ClientLaunchMode::TerminalAttach,
        },
    )
    .map_err(protocol_error)?;

    let welcome: v20::ServerMessage =
        v20::read_message(stream, v20::MAX_FRAME_SIZE).map_err(protocol_error)?;
    match welcome {
        v20::ServerMessage::Welcome {
            version,
            encoding: v20::RenderEncoding::TerminalAnsi,
            error: None,
        } if version == v20::PROTOCOL_VERSION => {}
        v20::ServerMessage::Welcome {
            version,
            encoding,
            error,
        } => {
            let detail = error.unwrap_or_else(|| {
                format!(
                    "server selected {encoding:?} at protocol {version}; expected TerminalAnsi protocol {}",
                    v20::PROTOCOL_VERSION
                )
            });
            return Err(HerdrError::Protocol(detail));
        }
        message => {
            return Err(HerdrError::Protocol(format!(
                "expected private protocol Welcome, received {message:?}"
            )));
        }
    }

    let attach = match mode {
        crate::TerminalMode::Observe => v20::ClientMessage::ObserveTerminal {
            target: target.to_owned(),
        },
        crate::TerminalMode::Control | crate::TerminalMode::ControlTakeover => {
            v20::ClientMessage::ControlTerminal {
                target: target.to_owned(),
                takeover: mode.takes_over(),
            }
        }
    };
    v20::write_message(stream, &attach).map_err(protocol_error)
}

impl TerminalWireWriter {
    pub(crate) fn send(&mut self, command: TerminalCommand) -> Result<()> {
        match self.codec {
            ProtocolCodec::V20 => self.send_v20(command),
        }
    }

    fn send_v20(&mut self, command: TerminalCommand) -> Result<()> {
        let message = match command {
            TerminalCommand::Input(data) => v20::ClientMessage::Input { data },
            TerminalCommand::ClipboardImage { extension, bytes } => {
                if bytes.len() > v20::MAX_CLIPBOARD_IMAGE_PAYLOAD {
                    return Err(HerdrError::Protocol(format!(
                        "clipboard image is {} bytes; private protocol maximum is {} bytes",
                        bytes.len(),
                        v20::MAX_CLIPBOARD_IMAGE_PAYLOAD
                    )));
                }
                v20::ClientMessage::ClipboardImage {
                    extension,
                    data: bytes,
                }
            }
            TerminalCommand::Resize {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
            } => v20::ClientMessage::Resize {
                cols: cols.max(1),
                rows: rows.max(1),
                cell_width_px,
                cell_height_px,
            },
            TerminalCommand::Scroll { direction, lines } => {
                let direction = match direction {
                    TerminalScrollDirection::Up => v20::AttachScrollDirection::Up,
                    TerminalScrollDirection::Down => v20::AttachScrollDirection::Down,
                };
                v20::ClientMessage::AttachScroll {
                    source: v20::AttachScrollSource::Wheel,
                    direction,
                    lines: lines.max(1),
                    column: None,
                    row: None,
                    modifiers: 0,
                }
            }
            TerminalCommand::Release => v20::ClientMessage::Detach,
        };
        v20::write_message(&mut self.stream, &message).map_err(protocol_error)
    }
}

impl TerminalWireReader {
    pub(crate) fn read_event(&mut self) -> Result<Option<TerminalEvent>> {
        match self.codec {
            ProtocolCodec::V20 => self.read_v20_event(),
        }
    }

    fn read_v20_event(&mut self) -> Result<Option<TerminalEvent>> {
        loop {
            let message: v20::ServerMessage =
                match v20::read_message(&mut self.stream, v20::MAX_GRAPHICS_FRAME_SIZE) {
                    Ok(message) => message,
                    Err(v20::FramingError::UnexpectedEof) => return Ok(None),
                    Err(error) => return Err(protocol_error(error)),
                };
            match message {
                v20::ServerMessage::Terminal(frame) => {
                    if let Some(previous) = self.last_sequence
                        && frame.seq != previous.saturating_add(1)
                        && !frame.full
                    {
                        return Err(HerdrError::Protocol(format!(
                            "terminal frame gap: expected {}, got {}",
                            previous.saturating_add(1),
                            frame.seq
                        )));
                    }
                    self.last_sequence = Some(frame.seq);
                    return Ok(Some(TerminalEvent::Frame(TerminalFrame {
                        seq: frame.seq,
                        width: frame.width,
                        height: frame.height,
                        full: frame.full,
                        bytes: frame.bytes,
                    })));
                }
                v20::ServerMessage::MouseCapture {
                    enabled,
                    sgr_pixels,
                } => {
                    return Ok(Some(TerminalEvent::MouseCapture {
                        enabled,
                        sgr_pixels,
                    }));
                }
                v20::ServerMessage::KittyKeyboardReportAll { enabled } => {
                    return Ok(Some(TerminalEvent::KittyKeyboardReportAll { enabled }));
                }
                v20::ServerMessage::ServerShutdown { reason } => {
                    return Err(HerdrError::TerminalClosed(
                        reason.unwrap_or_else(|| "server closed the terminal stream".into()),
                    ));
                }
                // Direct terminal streams currently do not use these client-local
                // effects. They are decoded so a future facade can expose them without
                // changing the frozen v20 schema.
                v20::ServerMessage::Frame(_)
                | v20::ServerMessage::Graphics { .. }
                | v20::ServerMessage::Notify { .. }
                | v20::ServerMessage::Clipboard { .. }
                | v20::ServerMessage::WindowTitle { .. }
                | v20::ServerMessage::ReloadSoundConfig
                | v20::ServerMessage::PrefixInputSource { .. }
                | v20::ServerMessage::TerminalBell { .. }
                | v20::ServerMessage::GraphicsFile { .. }
                | v20::ServerMessage::GraphicsTransmissionRetired { .. } => continue,
                v20::ServerMessage::Welcome { .. } => {
                    return Err(HerdrError::Protocol(
                        "received a second private protocol Welcome".into(),
                    ));
                }
            }
        }
    }
}

fn protocol_error(error: v20::FramingError) -> HerdrError {
    match error {
        v20::FramingError::Io(error) => HerdrError::Io(error),
        other => HerdrError::Protocol(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    #[test]
    fn protocol_registry_is_explicit_and_rejects_unknown_versions() {
        assert_eq!(
            ProtocolCodec::for_version(20).unwrap().version(),
            v20::PROTOCOL_VERSION
        );
        assert!(matches!(
            ProtocolCodec::for_version(21),
            Err(HerdrError::Protocol(message)) if message.contains("21")
        ));
    }

    #[test]
    fn v20_facade_handshakes_and_maps_terminal_commands_and_events() {
        let directory = tempfile::TempDir::new().unwrap();
        let socket_path = directory.path().join("client.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let hello: v20::ClientMessage =
                v20::read_message(&mut stream, v20::MAX_FRAME_SIZE).unwrap();
            assert_eq!(
                hello,
                v20::ClientMessage::Hello {
                    version: 20,
                    cols: 120,
                    rows: 40,
                    cell_width_px: 0,
                    cell_height_px: 0,
                    requested_encoding: v20::RenderEncoding::TerminalAnsi,
                    keybindings: v20::ClientKeybindings::Server,
                    launch_mode: v20::ClientLaunchMode::TerminalAttach,
                }
            );
            v20::write_message(
                &mut stream,
                &v20::ServerMessage::Welcome {
                    version: 20,
                    encoding: v20::RenderEncoding::TerminalAnsi,
                    error: None,
                },
            )
            .unwrap();
            let attach: v20::ClientMessage =
                v20::read_message(&mut stream, v20::MAX_FRAME_SIZE).unwrap();
            assert_eq!(
                attach,
                v20::ClientMessage::ControlTerminal {
                    target: "pane-7".into(),
                    takeover: true,
                }
            );

            v20::write_message(
                &mut stream,
                &v20::ServerMessage::Terminal(v20::TerminalFrame {
                    seq: 1,
                    width: 120,
                    height: 40,
                    full: true,
                    bytes: b"ansi".to_vec(),
                }),
            )
            .unwrap();
            v20::write_message(
                &mut stream,
                &v20::ServerMessage::MouseCapture {
                    enabled: true,
                    sgr_pixels: false,
                },
            )
            .unwrap();
            v20::write_message(
                &mut stream,
                &v20::ServerMessage::KittyKeyboardReportAll { enabled: true },
            )
            .unwrap();

            (0..4)
                .map(|_| v20::read_message(&mut stream, v20::MAX_GRAPHICS_FRAME_SIZE).unwrap())
                .collect::<Vec<v20::ClientMessage>>()
        });

        let endpoint = TerminalEndpoint::new(socket_path);
        let (mut reader, mut writer) = connect(TerminalConnect {
            endpoint: &endpoint,
            protocol: 20,
            target: "pane-7",
            mode: crate::TerminalMode::ControlTakeover,
            cols: 120,
            rows: 40,
            cell_width_px: 0,
            cell_height_px: 0,
        })
        .unwrap();
        assert_eq!(
            reader.read_event().unwrap(),
            Some(TerminalEvent::Frame(TerminalFrame {
                seq: 1,
                width: 120,
                height: 40,
                full: true,
                bytes: b"ansi".to_vec(),
            }))
        );
        assert_eq!(
            reader.read_event().unwrap(),
            Some(TerminalEvent::MouseCapture {
                enabled: true,
                sgr_pixels: false,
            })
        );
        assert_eq!(
            reader.read_event().unwrap(),
            Some(TerminalEvent::KittyKeyboardReportAll { enabled: true })
        );

        writer
            .send(TerminalCommand::Input(vec![0, 0x1b, 0xff]))
            .unwrap();
        writer
            .send(TerminalCommand::ClipboardImage {
                extension: "png".into(),
                bytes: vec![1, 2, 3],
            })
            .unwrap();
        writer
            .send(TerminalCommand::Resize {
                cols: 90,
                rows: 30,
                cell_width_px: 9,
                cell_height_px: 18,
            })
            .unwrap();
        writer
            .send(TerminalCommand::Scroll {
                direction: TerminalScrollDirection::Up,
                lines: 3,
            })
            .unwrap();

        assert_eq!(
            server.join().unwrap(),
            vec![
                v20::ClientMessage::Input {
                    data: vec![0, 0x1b, 0xff],
                },
                v20::ClientMessage::ClipboardImage {
                    extension: "png".into(),
                    data: vec![1, 2, 3],
                },
                v20::ClientMessage::Resize {
                    cols: 90,
                    rows: 30,
                    cell_width_px: 9,
                    cell_height_px: 18,
                },
                v20::ClientMessage::AttachScroll {
                    source: v20::AttachScrollSource::Wheel,
                    direction: v20::AttachScrollDirection::Up,
                    lines: 3,
                    column: None,
                    row: None,
                    modifiers: 0,
                },
            ]
        );
    }
}
