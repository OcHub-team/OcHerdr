//! Portable terminal model used on Linux and Windows.
//!
//! Herdr already owns the PTY. This layer only mirrors its output, encodes
//! input, and exposes styled viewport rows for GPUI to paint.

use std::pin::Pin;
use std::sync::mpsc::{self, Receiver as StdReceiver, Sender as StdSender};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures::Stream;
use futures::channel::mpsc::{Receiver, Sender};
use thiserror::Error;

const MOUSE_REPORTING_RESET: &[u8] =
    b"\x1b[?1006l\x1b[?1016l\x1b[?1015l\x1b[?1005l\x1b[?1003l\x1b[?1002l\x1b[?1000l";
const MOUSE_REPORTING_ENABLE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h";
const PIXEL_MOUSE_REPORTING_ENABLE: &[u8] = b"\x1b[?1016h";

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("portable terminal state is unavailable")]
    Initialization,
    #[error("terminal grid must contain at least one row and column")]
    InvalidGrid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceMouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalPalette {
    pub dark: bool,
    pub background: u32,
    pub background_opacity: u8,
    pub foreground: u32,
    pub cursor: u32,
    pub cursor_text: u32,
    pub selection: u32,
    pub selection_foreground: u32,
    pub ansi: [u32; 16],
    pub font_family: String,
    pub font_size: u8,
    pub font_features: Vec<String>,
    pub thicken: bool,
    pub thicken_strength: u8,
    pub cell_width: Option<String>,
    pub cell_height: Option<String>,
    pub padding_x: u32,
    pub padding_y: u32,
}

impl TerminalPalette {
    pub fn signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        Hash::hash(self, &mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceSize {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortableTextRun {
    pub len: usize,
    pub foreground: u32,
    pub background: u32,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortableLine {
    pub text: String,
    pub runs: Vec<PortableTextRun>,
}

#[derive(Clone, Debug)]
pub struct RenderedFrame {
    pub lines: Arc<[PortableLine]>,
    pub width_px: u32,
    pub height_px: u32,
    pub host_context: u64,
    pub font_family: Arc<str>,
    pub font_size: u8,
    pub cell_height_px: u32,
    pub padding_x: u32,
    pub padding_y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridPoint {
    row: u16,
    column: u16,
}

struct State {
    parser: vt100::Parser,
    palette: TerminalPalette,
    size: SurfaceSize,
    host_context: u64,
    selection: Option<(GridPoint, GridPoint)>,
    selection_anchor: Option<GridPoint>,
    mouse: GridPoint,
    pressed_mouse_button: Option<SurfaceMouseButton>,
    kitty_report_all: bool,
    preedit: Option<String>,
}

pub struct Terminal {
    state: Mutex<State>,
    frames_tx: Mutex<Sender<()>>,
    frames: Receiver<()>,
    input_tx: StdSender<Vec<u8>>,
    input: StdReceiver<Vec<u8>>,
}

impl Terminal {
    pub fn new(
        cols: u16,
        rows: u16,
        scrollback: usize,
        palette: &TerminalPalette,
    ) -> Result<Self, TerminalError> {
        if cols == 0 || rows == 0 {
            return Err(TerminalError::InvalidGrid);
        }
        let (frames_tx, frames) = futures::channel::mpsc::channel(1);
        let (input_tx, input) = mpsc::channel();
        let size = initial_size(cols, rows, palette);
        let terminal = Self {
            state: Mutex::new(State {
                parser: vt100::Parser::new(rows, cols, scrollback),
                palette: palette.clone(),
                size,
                host_context: 0,
                selection: None,
                selection_anchor: None,
                mouse: GridPoint { row: 0, column: 0 },
                pressed_mouse_button: None,
                kitty_report_all: false,
                preedit: None,
            }),
            frames_tx: Mutex::new(frames_tx),
            frames,
            input_tx,
            input,
        };
        terminal.refresh();
        Ok(terminal)
    }

    pub fn tick_runtime() -> Result<(), TerminalError> {
        Ok(())
    }

    pub fn apply_frame(&self, bytes: &[u8], _full: bool) {
        if bytes.is_empty() {
            return;
        }
        self.with_state(|state| state.parser.process(bytes));
        self.refresh();
    }

    pub fn set_grid_size(&self, cols: u16, rows: u16) -> Result<SurfaceSize, TerminalError> {
        if cols == 0 || rows == 0 {
            return Err(TerminalError::InvalidGrid);
        }
        let size = self.with_state(|state| {
            state.parser.screen_mut().set_size(rows, cols);
            state.size.columns = cols;
            state.size.rows = rows;
            state.size.width_px = u32::from(cols) * state.size.cell_width_px
                + state.palette.padding_x.saturating_mul(2);
            state.size.height_px = u32::from(rows) * state.size.cell_height_px
                + state.palette.padding_y.saturating_mul(2);
            state.size
        });
        self.refresh();
        Ok(size)
    }

    pub fn resize_pixels(
        &self,
        width_px: u32,
        height_px: u32,
        scale_factor: f64,
        host_context: u64,
    ) -> SurfaceSize {
        let size = self.with_state(|state| {
            let scale = scale_factor.max(1.0);
            let font_px = (f64::from(state.palette.font_size.clamp(8, 32)) * scale).round();
            state.size.cell_width_px = adjusted_cell_width(&state.palette, font_px);
            state.size.cell_height_px = adjusted_cell_height(&state.palette, font_px);
            let horizontal_padding = state.palette.padding_x.saturating_mul(2);
            let vertical_padding = state.palette.padding_y.saturating_mul(2);
            let columns = width_px
                .saturating_sub(horizontal_padding)
                .checked_div(state.size.cell_width_px.max(1))
                .unwrap_or(1)
                .clamp(1, u32::from(u16::MAX)) as u16;
            let rows = height_px
                .saturating_sub(vertical_padding)
                .checked_div(state.size.cell_height_px.max(1))
                .unwrap_or(1)
                .clamp(1, u32::from(u16::MAX)) as u16;
            if (rows, columns) != state.parser.screen().size() {
                state.parser.screen_mut().set_size(rows, columns);
            }
            state.size = SurfaceSize {
                columns,
                rows,
                width_px: width_px.max(1),
                height_px: height_px.max(1),
                cell_width_px: state.size.cell_width_px,
                cell_height_px: state.size.cell_height_px,
            };
            state.host_context = host_context;
            state.size
        });
        self.refresh();
        size
    }

    pub fn surface_size(&self) -> SurfaceSize {
        self.with_state(|state| state.size)
    }

    pub fn try_frame(&mut self) -> Result<Option<RenderedFrame>, TerminalError> {
        if self.frames.try_recv().is_err() {
            return Ok(None);
        }
        while self.frames.try_recv().is_ok() {}
        Ok(Some(self.snapshot()))
    }

    pub fn poll_frame(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<RenderedFrame, TerminalError>>> {
        match Pin::new(&mut self.frames).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(())) => {
                while self.frames.try_recv().is_ok() {}
                Poll::Ready(Some(Ok(self.snapshot())))
            }
        }
    }

    pub fn try_input(&self) -> Option<Vec<u8>> {
        self.input.try_recv().ok()
    }

    pub fn send_key(
        &self,
        action: KeyAction,
        key: &str,
        text: Option<&str>,
        modifiers: KeyModifiers,
    ) -> bool {
        let report_release = self.with_state(|state| state.kitty_report_all);
        if action == KeyAction::Release && !report_release {
            return true;
        }
        let application_cursor =
            self.with_state(|state| state.parser.screen().application_cursor());
        let Some(mut encoded) = encode_key(
            action,
            key,
            text,
            modifiers,
            application_cursor,
            report_release,
        ) else {
            return false;
        };
        if modifiers.alt && !encoded.starts_with(b"\x1b") {
            encoded.insert(0, 0x1b);
        }
        self.queue_input(encoded);
        true
    }

    pub fn paste(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bracketed = self.with_state(|state| state.parser.screen().bracketed_paste());
        let bytes = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
        } else {
            text.as_bytes().to_vec()
        };
        self.queue_input(bytes);
    }

    pub fn set_preedit(&self, text: Option<&str>) {
        self.with_state(|state| {
            state.preedit = text.filter(|value| !value.is_empty()).map(str::to_owned)
        });
        self.refresh();
    }

    pub fn ime_point(&self) -> (f64, f64, f64, f64) {
        self.with_state(|state| {
            let (row, column) = state.parser.screen().cursor_position();
            (
                f64::from(state.palette.padding_x + u32::from(column) * state.size.cell_width_px),
                f64::from(state.palette.padding_y + u32::from(row) * state.size.cell_height_px),
                f64::from(state.size.cell_width_px),
                f64::from(state.size.cell_height_px),
            )
        })
    }

    pub fn mouse_pos(&self, x: f64, y: f64, modifiers: KeyModifiers) {
        let report = self.with_state(|state| {
            let previous = state.mouse;
            state.mouse = point_from_pixels(state, x, y);
            if state.mouse == previous || modifiers.shift {
                return None;
            }
            encode_mouse_motion(state, modifiers)
        });
        if let Some(bytes) = report {
            self.queue_input(bytes);
        }
    }

    pub fn mouse_button(
        &self,
        pressed: bool,
        button: SurfaceMouseButton,
        modifiers: KeyModifiers,
    ) -> bool {
        let report = self.with_state(|state| {
            use vt100::MouseProtocolMode;
            if pressed {
                state.pressed_mouse_button = Some(button);
            } else if state.pressed_mouse_button == Some(button) {
                state.pressed_mouse_button = None;
            }
            let mode = state.parser.screen().mouse_protocol_mode();
            if modifiers.shift
                || matches!(mode, MouseProtocolMode::None)
                || (!pressed && matches!(mode, MouseProtocolMode::Press))
            {
                return None;
            }
            encode_mouse(state, pressed, button, modifiers)
        });
        if let Some(bytes) = report {
            self.queue_input(bytes);
            true
        } else {
            false
        }
    }

    pub fn mouse_captured(&self) -> bool {
        self.with_state(|state| {
            !matches!(
                state.parser.screen().mouse_protocol_mode(),
                vt100::MouseProtocolMode::None
            )
        })
    }

    pub fn set_mouse_capture(&self, enabled: bool, sgr_pixels: bool) {
        let mut sequence = MOUSE_REPORTING_RESET.to_vec();
        if enabled {
            sequence.extend_from_slice(MOUSE_REPORTING_ENABLE);
            if sgr_pixels {
                sequence.extend_from_slice(PIXEL_MOUSE_REPORTING_ENABLE);
            }
        }
        self.apply_frame(&sequence, false);
    }

    pub fn set_kitty_keyboard_report_all(&self, enabled: bool) {
        self.with_state(|state| state.kitty_report_all = enabled);
    }

    pub fn has_selection(&self) -> bool {
        self.with_state(|state| state.selection.is_some())
    }

    pub fn read_selection(&self) -> Option<String> {
        self.with_state(|state| {
            let (start, end) = ordered_selection(state.selection?);
            Some(state.parser.screen().contents_between(
                start.row,
                start.column,
                end.row,
                end.column.saturating_add(1),
            ))
        })
    }

    pub fn select_all_visible(&self) -> bool {
        self.with_state(|state| {
            state.selection = Some((
                GridPoint { row: 0, column: 0 },
                GridPoint {
                    row: state.size.rows.saturating_sub(1),
                    column: state.size.columns.saturating_sub(1),
                },
            ));
        });
        self.refresh();
        true
    }

    pub fn begin_text_selection(&self, x: f64, y: f64, modifiers: KeyModifiers) -> bool {
        self.mouse_pos(x, y, modifiers);
        let captured = self.mouse_captured() && !modifiers.shift;
        let _ = self.mouse_button(true, SurfaceMouseButton::Left, modifiers);
        if captured {
            return true;
        }
        self.with_state(|state| {
            state.selection_anchor = Some(state.mouse);
            state.selection = Some((state.mouse, state.mouse));
        });
        self.refresh();
        false
    }

    pub fn update_text_selection(&self, x: f64, y: f64, modifiers: KeyModifiers) {
        self.mouse_pos(x, y, modifiers);
        self.with_state(|state| {
            if let Some(anchor) = state.selection_anchor {
                state.selection = Some((anchor, state.mouse));
            }
        });
        self.refresh();
    }

    pub fn end_text_selection(&self, point: Option<(f64, f64)>, modifiers: KeyModifiers) {
        if let Some((x, y)) = point {
            self.update_text_selection(x, y, modifiers);
        }
        let captured = self.mouse_captured() && !modifiers.shift;
        let _ = self.mouse_button(false, SurfaceMouseButton::Left, modifiers);
        if !captured {
            self.with_state(|state| state.selection_anchor = None);
        }
        self.refresh();
    }

    pub fn set_focus(&self, _focused: bool) {}

    pub fn set_color_scheme(&self, dark: bool) {
        self.with_state(|state| state.palette.dark = dark);
        self.refresh();
    }

    pub fn apply_palette(&self, palette: &TerminalPalette) -> Result<(), TerminalError> {
        self.with_state(|state| state.palette = palette.clone());
        self.refresh();
        Ok(())
    }

    pub fn refresh(&self) {
        let _ = self
            .frames_tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .try_send(());
    }

    pub fn read_visible_text(&self) -> Option<String> {
        Some(self.with_state(|state| state.parser.screen().contents()))
    }

    fn queue_input(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.input_tx.send(bytes);
        }
    }

    fn snapshot(&self) -> RenderedFrame {
        self.with_state(|state| {
            let screen = state.parser.screen();
            let palette = &state.palette;
            let mut lines = Vec::with_capacity(usize::from(state.size.rows));
            for row in 0..state.size.rows {
                let mut text = String::new();
                let mut runs = Vec::<PortableTextRun>::new();
                for column in 0..state.size.columns {
                    let Some(cell) = screen.cell(row, column) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let contents = if cell.has_contents() {
                        cell.contents()
                    } else {
                        " "
                    };
                    text.push_str(contents);
                    let selected = state.selection.is_some_and(|selection| {
                        point_in_selection(GridPoint { row, column }, selection)
                    });
                    let cursor = !screen.hide_cursor() && screen.cursor_position() == (row, column);
                    let mut foreground = resolve_color(cell.fgcolor(), palette, true);
                    let mut background = resolve_color(cell.bgcolor(), palette, false);
                    if cell.inverse() {
                        std::mem::swap(&mut foreground, &mut background);
                    }
                    if selected {
                        background = palette.selection;
                    }
                    if cursor {
                        background = palette.cursor;
                        foreground = palette.background;
                    }
                    let run = PortableTextRun {
                        len: contents.len(),
                        foreground,
                        background,
                        bold: cell.bold(),
                        dim: cell.dim(),
                        italic: cell.italic(),
                        underline: cell.underline(),
                    };
                    if let Some(previous) = runs.last_mut()
                        && previous.foreground == run.foreground
                        && previous.background == run.background
                        && previous.bold == run.bold
                        && previous.dim == run.dim
                        && previous.italic == run.italic
                        && previous.underline == run.underline
                    {
                        previous.len += run.len;
                    } else {
                        runs.push(run);
                    }
                }
                lines.push(PortableLine { text, runs });
            }
            RenderedFrame {
                lines: lines.into(),
                width_px: state.size.width_px,
                height_px: state.size.height_px,
                host_context: state.host_context,
                font_family: Arc::from(if palette.font_family.trim().is_empty() {
                    default_monospace_family()
                } else {
                    palette.font_family.trim()
                }),
                font_size: palette.font_size.clamp(8, 32),
                cell_height_px: state.size.cell_height_px,
                padding_x: palette.padding_x,
                padding_y: palette.padding_y,
            }
        })
    }

    fn with_state<T>(&self, apply: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply(&mut state)
    }
}

fn initial_size(cols: u16, rows: u16, palette: &TerminalPalette) -> SurfaceSize {
    let font_px = f64::from(palette.font_size.clamp(8, 32));
    let cell_width_px = adjusted_cell_width(palette, font_px);
    let cell_height_px = adjusted_cell_height(palette, font_px);
    SurfaceSize {
        columns: cols,
        rows,
        width_px: u32::from(cols) * cell_width_px + palette.padding_x.saturating_mul(2),
        height_px: u32::from(rows) * cell_height_px + palette.padding_y.saturating_mul(2),
        cell_width_px,
        cell_height_px,
    }
}

fn adjusted_cell_width(palette: &TerminalPalette, font_px: f64) -> u32 {
    let base = font_px * 0.62;
    apply_adjustment(base, palette.cell_width.as_deref())
        .round()
        .max(1.0) as u32
}

fn adjusted_cell_height(palette: &TerminalPalette, font_px: f64) -> u32 {
    let base = font_px * 1.35;
    apply_adjustment(base, palette.cell_height.as_deref())
        .round()
        .max(1.0) as u32
}

fn apply_adjustment(base: f64, adjustment: Option<&str>) -> f64 {
    let Some(value) = adjustment.map(str::trim).filter(|value| !value.is_empty()) else {
        return base;
    };
    if let Some(percent) = value
        .strip_suffix('%')
        .and_then(|value| value.parse::<f64>().ok())
    {
        return base * percent.max(10.0) / 100.0;
    }
    value.parse::<f64>().map_or(base, |value| base + value)
}

fn default_monospace_family() -> &'static str {
    #[cfg(target_os = "windows")]
    return "Cascadia Mono";
    #[cfg(not(target_os = "windows"))]
    return "DejaVu Sans Mono";
}

fn point_from_pixels(state: &State, x: f64, y: f64) -> GridPoint {
    let column = ((x.max(0.0) as u32).saturating_sub(state.palette.padding_x)
        / state.size.cell_width_px.max(1))
    .min(u32::from(state.size.columns.saturating_sub(1))) as u16;
    let row = ((y.max(0.0) as u32).saturating_sub(state.palette.padding_y)
        / state.size.cell_height_px.max(1))
    .min(u32::from(state.size.rows.saturating_sub(1))) as u16;
    GridPoint { row, column }
}

fn ordered_selection(selection: (GridPoint, GridPoint)) -> (GridPoint, GridPoint) {
    let (a, b) = selection;
    if (a.row, a.column) <= (b.row, b.column) {
        (a, b)
    } else {
        (b, a)
    }
}

fn point_in_selection(point: GridPoint, selection: (GridPoint, GridPoint)) -> bool {
    let (start, end) = ordered_selection(selection);
    (point.row, point.column) >= (start.row, start.column)
        && (point.row, point.column) <= (end.row, end.column)
}

fn resolve_color(color: vt100::Color, palette: &TerminalPalette, foreground: bool) -> u32 {
    match color {
        vt100::Color::Default => {
            if foreground {
                palette.foreground
            } else {
                palette.background
            }
        }
        vt100::Color::Idx(index) if index < 16 => palette.ansi[usize::from(index)],
        vt100::Color::Idx(index) if index < 232 => {
            let value = index - 16;
            let component = |part: u8| {
                if part == 0 {
                    0
                } else {
                    55 + u32::from(part) * 40
                }
            };
            let red = component(value / 36);
            let green = component((value / 6) % 6);
            let blue = component(value % 6);
            (red << 16) | (green << 8) | blue
        }
        vt100::Color::Idx(index) => {
            let gray = 8 + u32::from(index.saturating_sub(232)) * 10;
            (gray << 16) | (gray << 8) | gray
        }
        vt100::Color::Rgb(red, green, blue) => {
            (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue)
        }
    }
}

fn encode_key(
    action: KeyAction,
    key: &str,
    text: Option<&str>,
    modifiers: KeyModifiers,
    application_cursor: bool,
    kitty_report_all: bool,
) -> Option<Vec<u8>> {
    if kitty_report_all {
        let codepoint = key
            .chars()
            .next()
            .filter(|_| key.chars().count() == 1)
            .map(u32::from);
        if let Some(codepoint) = codepoint {
            let event = match action {
                KeyAction::Press => 1,
                KeyAction::Repeat => 2,
                KeyAction::Release => 3,
            };
            let modifier = 1
                + u8::from(modifiers.shift)
                + u8::from(modifiers.alt) * 2
                + u8::from(modifiers.control) * 4
                + u8::from(modifiers.platform) * 8;
            return Some(format!("\x1b[{codepoint};{modifier}:{event}u").into_bytes());
        }
    }
    if action == KeyAction::Release {
        return None;
    }
    let lower = key.to_ascii_lowercase();
    let cursor_prefix = if application_cursor { "\x1bO" } else { "\x1b[" };
    let special = match lower.as_str() {
        "enter" | "return" => "\r",
        "tab" if modifiers.shift => "\x1b[Z",
        "tab" => "\t",
        "backspace" => "\x7f",
        "escape" => "\x1b",
        "up" => return Some(format!("{cursor_prefix}A").into_bytes()),
        "down" => return Some(format!("{cursor_prefix}B").into_bytes()),
        "right" => return Some(format!("{cursor_prefix}C").into_bytes()),
        "left" => return Some(format!("{cursor_prefix}D").into_bytes()),
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "insert" => "\x1b[2~",
        "delete" => "\x1b[3~",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
        _ => "",
    };
    if !special.is_empty() {
        return Some(special.as_bytes().to_vec());
    }
    let character = if lower == "space" {
        Some(' ')
    } else {
        key.chars().next().filter(|_| key.chars().count() == 1)
    };
    if modifiers.control {
        let character = character?.to_ascii_uppercase();
        let code = match character {
            '@' | ' ' => 0,
            'A'..='Z' => character as u8 - b'A' + 1,
            '[' => 27,
            '\\' => 28,
            ']' => 29,
            '^' => 30,
            '_' => 31,
            '?' => 127,
            _ => return None,
        };
        return Some(vec![code]);
    }
    text.filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .map(|value| value.as_bytes().to_vec())
        .or_else(|| character.map(|value| value.to_string().into_bytes()))
}

fn encode_mouse(
    state: &State,
    pressed: bool,
    button: SurfaceMouseButton,
    modifiers: KeyModifiers,
) -> Option<Vec<u8>> {
    use vt100::MouseProtocolEncoding;
    let mut code = match button {
        SurfaceMouseButton::Left => 0,
        SurfaceMouseButton::Middle => 1,
        SurfaceMouseButton::Right => 2,
    };
    code += u8::from(modifiers.shift) * 4;
    code += u8::from(modifiers.alt) * 8;
    code += u8::from(modifiers.control) * 16;
    let column = u32::from(state.mouse.column) + 1;
    let row = u32::from(state.mouse.row) + 1;
    match state.parser.screen().mouse_protocol_encoding() {
        MouseProtocolEncoding::Sgr => Some(
            format!(
                "\x1b[<{code};{column};{row}{}",
                if pressed { 'M' } else { 'm' }
            )
            .into_bytes(),
        ),
        MouseProtocolEncoding::Default => {
            let code = if pressed { code } else { 3 };
            Some(vec![
                0x1b,
                b'[',
                b'M',
                32 + code,
                (32 + column.min(223)) as u8,
                (32 + row.min(223)) as u8,
            ])
        }
        _ => None,
    }
}

fn encode_mouse_motion(state: &State, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    use vt100::MouseProtocolMode;
    let button = match state.parser.screen().mouse_protocol_mode() {
        MouseProtocolMode::AnyMotion => state.pressed_mouse_button,
        MouseProtocolMode::ButtonMotion => Some(state.pressed_mouse_button?),
        MouseProtocolMode::None | MouseProtocolMode::Press | MouseProtocolMode::PressRelease => {
            return None;
        }
    };
    let mut code = match button {
        Some(SurfaceMouseButton::Left) => 0,
        Some(SurfaceMouseButton::Middle) => 1,
        Some(SurfaceMouseButton::Right) => 2,
        None => 3,
    } + 32;
    code += u8::from(modifiers.alt) * 8;
    code += u8::from(modifiers.control) * 16;
    encode_mouse_code(state, code, true)
}

fn encode_mouse_code(state: &State, code: u8, pressed: bool) -> Option<Vec<u8>> {
    use vt100::MouseProtocolEncoding;
    let column = u32::from(state.mouse.column) + 1;
    let row = u32::from(state.mouse.row) + 1;
    match state.parser.screen().mouse_protocol_encoding() {
        MouseProtocolEncoding::Sgr => Some(
            format!(
                "\x1b[<{code};{column};{row}{}",
                if pressed { 'M' } else { 'm' }
            )
            .into_bytes(),
        ),
        MouseProtocolEncoding::Default => Some(vec![
            0x1b,
            b'[',
            b'M',
            32 + code,
            (32 + column.min(223)) as u8,
            (32 + row.min(223)) as u8,
        ]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> TerminalPalette {
        TerminalPalette {
            dark: true,
            background: 0x101010,
            background_opacity: 100,
            foreground: 0xf0f0f0,
            cursor: 0xffffff,
            cursor_text: 0x101010,
            selection: 0x335577,
            selection_foreground: 0xf0f0f0,
            ansi: [0; 16],
            font_family: String::new(),
            font_size: 14,
            font_features: Vec::new(),
            thicken: false,
            thicken_strength: 0,
            cell_width: None,
            cell_height: None,
            padding_x: 0,
            padding_y: 0,
        }
    }

    #[test]
    fn mirrors_output_and_encodes_cursor_keys() {
        let mut terminal = Terminal::new(10, 2, 100, &palette()).unwrap();
        terminal.apply_frame(b"hello", false);
        let frame = terminal.try_frame().unwrap().unwrap();
        assert!(frame.lines[0].text.starts_with("hello"));
        assert!(terminal.send_key(KeyAction::Press, "up", None, KeyModifiers::default()));
        assert_eq!(terminal.try_input().unwrap(), b"\x1b[A");
    }

    #[test]
    fn mouse_reporting_is_tui_first_and_shift_remains_selection() {
        let terminal = Terminal::new(20, 10, 100, &palette()).unwrap();
        assert!(!terminal.mouse_captured());

        terminal.set_mouse_capture(true, false);
        assert!(terminal.mouse_captured());
        terminal.mouse_pos(16., 16., KeyModifiers::default());
        let motion = terminal.try_input().expect("any-motion report");
        assert!(motion.starts_with(b"\x1b[<35;"));
        assert!(motion.ends_with(b"M"));

        assert!(terminal.mouse_button(true, SurfaceMouseButton::Right, KeyModifiers::default(),));
        let press = terminal.try_input().expect("secondary-button press");
        assert!(press.starts_with(b"\x1b[<2;"));
        assert!(press.ends_with(b"M"));

        let shift = KeyModifiers {
            shift: true,
            ..Default::default()
        };
        assert!(!terminal.mouse_button(true, SurfaceMouseButton::Left, shift));
        assert!(terminal.try_input().is_none());

        terminal.set_mouse_capture(false, false);
        assert!(!terminal.mouse_captured());
    }
}
