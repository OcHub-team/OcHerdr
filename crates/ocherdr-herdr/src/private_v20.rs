//! Frozen client-side mirror of Herdr's private protocol version 20.
//!
//! Herdr intentionally does not publish this wire schema as a library crate. OcHerdr
//! therefore keeps the small, serialization-relevant surface here and verifies its
//! discriminants and byte fixtures in tests. Field and variant order are part of the
//! bincode contract; do not reorder them without adding a new versioned module.

#![allow(dead_code)]

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

pub(crate) const PROTOCOL_VERSION: u32 = 20;
pub(crate) const MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;
pub(crate) const MAX_GRAPHICS_FRAME_SIZE: usize = 32 * 1024 * 1024;
pub(crate) const MAX_CLIPBOARD_IMAGE_PAYLOAD: usize = 16 * 1024 * 1024;
const LENGTH_PREFIX_BYTES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RenderEncoding {
    SemanticFrame,
    TerminalAnsi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientKeybindings {
    Server,
    Local { keys_toml: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientLaunchMode {
    App,
    AppDirectGraphics,
    TerminalAttach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientKeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientKeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    Esc,
    Char(char),
    F(u8),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientMouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientMouseKind {
    Down(ClientMouseButton),
    Up(ClientMouseButton),
    Drag(ClientMouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WindowsKeyRecord {
    pub(crate) key_down: bool,
    pub(crate) repeat_count: u16,
    pub(crate) virtual_key_code: u16,
    pub(crate) virtual_scan_code: u16,
    pub(crate) unicode: u16,
    pub(crate) control_key_state: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientKeySource {
    Synthesized,
    Vt { bytes: Vec<u8> },
    WindowsConsole { record: WindowsKeyRecord },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientInputEvent {
    Key {
        code: ClientKeyCode,
        modifiers: u8,
        kind: ClientKeyKind,
        repeat_count: u16,
        generated_text: Option<String>,
        source: ClientKeySource,
    },
    TextCommit(String),
    Mouse {
        kind: ClientMouseKind,
        column: u16,
        row: u16,
        modifiers: u8,
    },
    Paste {
        text: String,
    },
    FocusGained,
    FocusLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AttachScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AttachScrollSource {
    Wheel,
    PageKey { input: Vec<u8> },
}

/// The complete v20 enum is mirrored so every following discriminant stays exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ClientMessage {
    Hello {
        version: u32,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        requested_encoding: RenderEncoding,
        keybindings: ClientKeybindings,
        launch_mode: ClientLaunchMode,
    },
    Input {
        data: Vec<u8>,
    },
    ClipboardImage {
        extension: String,
        data: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    Detach,
    AttachTerminal {
        terminal_id: String,
        takeover: bool,
    },
    AttachScroll {
        source: AttachScrollSource,
        direction: AttachScrollDirection,
        lines: u16,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    },
    InputEvents {
        events: Vec<ClientInputEvent>,
    },
    ObserveTerminal {
        target: String,
    },
    ControlTerminal {
        target: String,
        takeover: bool,
    },
    GraphicsTransmissionResult {
        transfer_id: u64,
        image_id: u32,
        success: bool,
    },
    InputPixels {
        data: Vec<u8>,
        cols: u16,
        rows: u16,
        width_px: u32,
        height_px: u32,
    },
    GraphicsTransmissionStarted {
        transfer_id: u64,
        image_id: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CellData {
    pub(crate) symbol: String,
    pub(crate) fg: u32,
    pub(crate) bg: u32,
    pub(crate) modifier: u16,
    pub(crate) skip: bool,
    pub(crate) hyperlink: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CursorState {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) visible: bool,
    #[serde(default)]
    pub(crate) shape: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FrameData {
    pub(crate) cells: Vec<CellData>,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) cursor: Option<CursorState>,
    pub(crate) hyperlinks: Vec<String>,
    pub(crate) graphics: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalFrame {
    pub(crate) seq: u64,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) full: bool,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NotifyKind {
    Sound,
    Toast,
    SystemToast,
}

/// The complete v20 enum is mirrored so the decoder can safely reject or ignore
/// messages a direct terminal connection does not currently surface to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ServerMessage {
    Welcome {
        version: u32,
        encoding: RenderEncoding,
        error: Option<String>,
    },
    Frame(FrameData),
    Terminal(TerminalFrame),
    Graphics {
        bytes: Vec<u8>,
    },
    ServerShutdown {
        reason: Option<String>,
    },
    Notify {
        kind: NotifyKind,
        message: String,
        body: Option<String>,
    },
    Clipboard {
        data: String,
    },
    WindowTitle {
        title: Option<String>,
    },
    ReloadSoundConfig,
    MouseCapture {
        enabled: bool,
        sgr_pixels: bool,
    },
    KittyKeyboardReportAll {
        enabled: bool,
    },
    PrefixInputSource {
        active: bool,
    },
    TerminalBell {
        count: u16,
    },
    GraphicsFile {
        path: String,
        expected_len: u64,
        image_id: u32,
        transfer_id: u64,
        leading: Vec<u8>,
        control: String,
    },
    GraphicsTransmissionRetired {
        transfer_id: u64,
        image_id: u32,
    },
}

#[derive(Debug)]
pub(crate) enum FramingError {
    Oversized { claimed: usize, max: usize },
    Io(io::Error),
    Bincode(String),
    UnexpectedEof,
}

impl std::fmt::Display for FramingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversized { claimed, max } => {
                write!(formatter, "frame size {claimed} exceeds maximum {max}")
            }
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Bincode(error) => write!(formatter, "bincode error: {error}"),
            Self::UnexpectedEof => formatter.write_str("unexpected end of stream"),
        }
    }
}

impl std::error::Error for FramingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for FramingError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) fn write_message<W: Write, M: Serialize>(
    writer: &mut W,
    message: &M,
) -> Result<(), FramingError> {
    let payload = bincode::serde::encode_to_vec(message, bincode::config::standard())
        .map_err(|error| FramingError::Bincode(error.to_string()))?;
    let length = u32::try_from(payload.len()).map_err(|_| {
        FramingError::Bincode(format!("payload length {} exceeds u32::MAX", payload.len()))
    })?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn read_message<R: Read, M: for<'de> Deserialize<'de>>(
    reader: &mut R,
    maximum: usize,
) -> Result<M, FramingError> {
    let mut length = [0_u8; LENGTH_PREFIX_BYTES];
    read_exact_or_eof(reader, &mut length)?;
    let claimed = u32::from_le_bytes(length) as usize;
    if claimed > maximum {
        return Err(FramingError::Oversized {
            claimed,
            max: maximum,
        });
    }
    let mut payload = vec![0_u8; claimed];
    read_exact_or_eof(reader, &mut payload)?;
    let (message, consumed) =
        bincode::serde::decode_from_slice(&payload, bincode::config::standard())
            .map_err(|error| FramingError::Bincode(error.to_string()))?;
    if consumed != claimed {
        return Err(FramingError::Bincode(format!(
            "decoded {consumed} bytes but payload length was {claimed}; trailing bytes are not allowed"
        )));
    }
    Ok(message)
}

fn read_exact_or_eof<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<(), FramingError> {
    reader.read_exact(buffer).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            FramingError::UnexpectedEof
        } else {
            FramingError::Io(error)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(message: &ClientMessage) -> Vec<u8> {
        bincode::serde::encode_to_vec(message, bincode::config::standard()).unwrap()
    }

    #[test]
    fn v20_client_variant_discriminants_are_frozen() {
        assert_eq!(payload(&ClientMessage::Input { data: Vec::new() })[0], 1);
        assert_eq!(
            payload(&ClientMessage::ClipboardImage {
                extension: String::new(),
                data: Vec::new(),
            })[0],
            2
        );
        assert_eq!(
            payload(&ClientMessage::ObserveTerminal {
                target: String::new(),
            })[0],
            8
        );
        assert_eq!(
            payload(&ClientMessage::ControlTerminal {
                target: String::new(),
                takeover: false,
            })[0],
            9
        );
    }

    #[test]
    fn v20_golden_client_payloads_are_frozen() {
        let hello = ClientMessage::Hello {
            version: 20,
            cols: 80,
            rows: 24,
            cell_width_px: 8,
            cell_height_px: 16,
            requested_encoding: RenderEncoding::TerminalAnsi,
            keybindings: ClientKeybindings::Server,
            launch_mode: ClientLaunchMode::TerminalAttach,
        };
        assert_eq!(
            payload(&hello),
            [0x00, 0x14, 0x50, 0x18, 0x08, 0x10, 0x01, 0x00, 0x02]
        );

        let image = ClientMessage::ClipboardImage {
            extension: "png".into(),
            data: vec![1, 2, 3],
        };
        assert_eq!(
            payload(&image),
            [0x02, 0x03, b'p', b'n', b'g', 0x03, 0x01, 0x02, 0x03]
        );

        let mut framed = Vec::new();
        write_message(&mut framed, &image).unwrap();
        assert_eq!(
            framed,
            [
                0x09, 0x00, 0x00, 0x00, 0x02, 0x03, b'p', b'n', b'g', 0x03, 0x01, 0x02, 0x03,
            ]
        );
    }

    #[test]
    fn framing_reassembles_fragmented_reads_and_rejects_trailing_bytes() {
        let expected = ServerMessage::Terminal(TerminalFrame {
            seq: 7,
            width: 120,
            height: 40,
            full: true,
            bytes: b"frame".to_vec(),
        });
        let mut framed = Vec::new();
        write_message(&mut framed, &expected).unwrap();
        let decoded: ServerMessage = read_message(&mut framed.as_slice(), MAX_FRAME_SIZE).unwrap();
        assert_eq!(decoded, expected);

        let mut payload =
            bincode::serde::encode_to_vec(&expected, bincode::config::standard()).unwrap();
        payload.push(0xff);
        let mut corrupt = (payload.len() as u32).to_le_bytes().to_vec();
        corrupt.extend(payload);
        assert!(matches!(
            read_message::<_, ServerMessage>(&mut corrupt.as_slice(), MAX_FRAME_SIZE),
            Err(FramingError::Bincode(_))
        ));
    }

    #[test]
    fn oversized_length_is_rejected_before_allocation() {
        let bytes = ((MAX_FRAME_SIZE + 1) as u32).to_le_bytes();
        assert!(matches!(
            read_message::<_, ServerMessage>(&mut bytes.as_slice(), MAX_FRAME_SIZE),
            Err(FramingError::Oversized { .. })
        ));
    }
}
