//! Per-pane ANSI encoder for endpoint surface frames.
//!
//! Herdr's endpoint protocol composites the whole tab into one `FrameData`
//! grid with pane rectangles on top. OcHerdr renders every pane through its
//! own terminal mirror, so this module slices each pane's `inner_rect` out of
//! the composited surface and blits it to ANSI bytes with the same diffing
//! strategy Herdr's own `BlitEncoder` uses (synchronized output, SGR batching,
//! hyperlink tracking). The bytes feed `apply_frame` unchanged.

use std::cmp;
use std::collections::HashMap;
use std::fmt::Write as _;

use unicode_width::UnicodeWidthStr;

use crate::endpoint_v1::{
    CellData, CursorState, PaneSurfaceFrame, PaneSurfacePane, PaneSurfacePatch,
    PaneSurfaceScrollMetrics, SurfaceRect,
};

const REVERSED_MODIFIER: u16 = 1 << 6;
const UNDERLINE_STYLE_SHIFT: u16 = 12;
const UNDERLINE_STYLE_MASK: u16 = 0xF000;

/// ANSI bytes + geometry for one pane after a surface update.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneRenderUpdate {
    pub pane_id: String,
    pub ansi: Vec<u8>,
    pub rect: SurfaceRect,
    pub inner_rect: SurfaceRect,
    pub scrollbar_rect: Option<SurfaceRect>,
    pub focused: bool,
    pub mouse_reporting: bool,
    pub sgr_pixel_mouse: bool,
    pub alternate_screen_active: bool,
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// Full redraw (first frame for the pane, or a size change).
    pub full: bool,
    /// Server-reported scrollback position for this pane's viewport.
    pub scroll: Option<PaneSurfaceScrollMetrics>,
}

/// Tracks the committed surface and per-pane blit state across frames.
#[derive(Default)]
pub struct SurfaceDecoder {
    surface: Option<PaneSurfaceFrame>,
    panes: HashMap<String, PaneBlit>,
}

struct PaneBlit {
    last_size: Option<(u16, u16)>,
    last_cells: Option<Vec<CellData>>,
    last_hyperlinks: Vec<String>,
    last_cursor_visible: Option<(u16, u16)>,
    last_cursor_shape: u8,
}

impl PaneBlit {
    fn new() -> Self {
        Self {
            last_size: None,
            last_cells: None,
            last_hyperlinks: Vec::new(),
            last_cursor_visible: None,
            last_cursor_shape: 0,
        }
    }
}

impl SurfaceDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Commits a full surface frame and returns ANSI bytes for every pane in
    /// it. `repaint` forces a full redraw per pane.
    pub fn frame(&mut self, frame: PaneSurfaceFrame, repaint: bool) -> Vec<PaneRenderUpdate> {
        let mut updates = Vec::with_capacity(frame.panes.len());
        let hyperlinks = frame.frame.hyperlinks.clone();
        for pane in &frame.panes {
            let (cells, cursor) = extract_pane_view(&frame, pane);
            let blit = self
                .panes
                .entry(pane.pane_id.clone())
                .or_insert_with(PaneBlit::new);
            let (ansi, full) = blit.encode(
                &cells,
                pane.inner_rect.width,
                pane.inner_rect.height,
                cursor,
                &hyperlinks,
                repaint,
            );
            updates.push(PaneRenderUpdate {
                pane_id: pane.pane_id.clone(),
                ansi,
                rect: pane.rect,
                inner_rect: pane.inner_rect,
                scrollbar_rect: pane.scrollbar_rect,
                focused: pane.focused,
                mouse_reporting: pane.mouse_reporting,
                sgr_pixel_mouse: pane.sgr_pixel_mouse,
                alternate_screen_active: pane.alternate_screen_active,
                pixel_width: pane.pixel_width,
                pixel_height: pane.pixel_height,
                full,
                scroll: pane.scroll.clone(),
            });
        }
        self.panes
            .retain(|id, _| frame.panes.iter().any(|pane| &pane.pane_id == id));
        self.surface = Some(frame);
        updates
    }

    /// Applies a surface patch. Returns `None` when the patch's base revision
    /// does not match the committed surface — the caller should wait for a
    /// full frame.
    pub fn patch(&mut self, patch: &PaneSurfacePatch) -> Option<Vec<PaneRenderUpdate>> {
        let surface = self.surface.as_mut()?;
        if surface.surface_revision != patch.base_surface_revision {
            return None;
        }
        let width = usize::from(surface.frame.width);
        for row in &patch.rows {
            let base = usize::from(row.y) * width + usize::from(row.x);
            for (offset, cell) in row.cells.iter().enumerate() {
                let idx = base + offset;
                if idx < surface.frame.cells.len() {
                    surface.frame.cells[idx] = cell.clone();
                }
            }
        }
        if let Some(cursor) = patch.cursor {
            surface.frame.cursor = Some(cursor);
        }
        surface.surface_revision = patch.surface_revision;

        let mut updates = Vec::with_capacity(patch.panes.len());
        let hyperlinks = surface.frame.hyperlinks.clone();
        for pane in &patch.panes {
            let (cells, cursor) = extract_pane_view(surface, pane);
            let blit = self
                .panes
                .entry(pane.pane_id.clone())
                .or_insert_with(PaneBlit::new);
            let (ansi, full) = blit.encode(
                &cells,
                pane.inner_rect.width,
                pane.inner_rect.height,
                cursor,
                &hyperlinks,
                false,
            );
            updates.push(PaneRenderUpdate {
                pane_id: pane.pane_id.clone(),
                ansi,
                rect: pane.rect,
                inner_rect: pane.inner_rect,
                scrollbar_rect: pane.scrollbar_rect,
                focused: pane.focused,
                mouse_reporting: pane.mouse_reporting,
                sgr_pixel_mouse: pane.sgr_pixel_mouse,
                alternate_screen_active: pane.alternate_screen_active,
                pixel_width: pane.pixel_width,
                pixel_height: pane.pixel_height,
                full,
                scroll: pane.scroll.clone(),
            });
        }
        Some(updates)
    }
}

/// Extracts a pane's inner-rect subgrid from the composited surface and
/// translates the frame cursor into pane-local coordinates.
fn extract_pane_view(
    surface: &PaneSurfaceFrame,
    pane: &PaneSurfacePane,
) -> (Vec<CellData>, Option<CursorState>) {
    let width = usize::from(surface.frame.width);
    let inner = pane.inner_rect;
    let mut cells = Vec::with_capacity(usize::from(inner.width) * usize::from(inner.height));
    for row in 0..inner.height {
        let base = usize::from(inner.y + row) * width + usize::from(inner.x);
        for col in 0..inner.width {
            let idx = base + usize::from(col);
            cells.push(
                surface
                    .frame
                    .cells
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(blank_cell),
            );
        }
    }
    let cursor = surface.frame.cursor.and_then(|cursor| {
        let in_bounds = cursor.x >= inner.x
            && cursor.x < inner.x.saturating_add(inner.width)
            && cursor.y >= inner.y
            && cursor.y < inner.y.saturating_add(inner.height);
        in_bounds.then(|| CursorState {
            x: cursor.x - inner.x,
            y: cursor.y - inner.y,
            visible: cursor.visible && pane.focused,
            shape: cursor.shape,
        })
    });
    (cells, cursor)
}

fn blank_cell() -> CellData {
    CellData {
        symbol: " ".to_owned(),
        fg: 0,
        bg: 0,
        modifier: 0,
        skip: false,
        hyperlink: None,
    }
}

impl PaneBlit {
    /// Diffs a pane-local cell grid into ANSI bytes. Returns `(bytes, full)`.
    fn encode(
        &mut self,
        cells: &[CellData],
        width: u16,
        height: u16,
        cursor: Option<CursorState>,
        hyperlinks: &[String],
        repaint: bool,
    ) -> (Vec<u8>, bool) {
        let full = repaint || self.last_size != Some((width, height));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x1b[?2026h\x1b[?25l\x1b]8;;\x1b\\");
        if full {
            write_all_cells(&mut bytes, cells, width, height, hyperlinks);
        } else {
            write_changed_cells(
                &mut bytes,
                cells,
                width,
                self.last_cells.as_deref().unwrap_or(&[]),
                hyperlinks,
                &self.last_hyperlinks,
            );
        }
        let host_cursor = resolve_cursor(cursor, width, height, &mut self.last_cursor_visible);
        write_cursor(&mut bytes, host_cursor, &mut self.last_cursor_shape);
        bytes.extend_from_slice(b"\x1b[?2026l");
        // Re-anchor outside the sync block so native IMEs track the real
        // input position (mirrors Herdr's non-Windows behaviour).
        write_cursor_position(&mut bytes, host_cursor.position);
        if host_cursor.visible {
            bytes.extend_from_slice(b"\x1b[?25h");
        } else {
            bytes.extend_from_slice(b"\x1b[?25l");
        }
        self.last_size = Some((width, height));
        self.last_cells = Some(cells.to_vec());
        self.last_hyperlinks = hyperlinks.to_vec();
        (bytes, full)
    }
}

#[derive(Clone, Copy)]
struct HostCursor {
    position: (u16, u16),
    visible: bool,
    shape: u8,
}

fn resolve_cursor(
    cursor: Option<CursorState>,
    width: u16,
    height: u16,
    last_visible: &mut Option<(u16, u16)>,
) -> HostCursor {
    if let Some(cursor) = cursor {
        let position = (
            cursor.x.min(width.saturating_sub(1)),
            cursor.y.min(height.saturating_sub(1)),
        );
        if cursor.visible {
            *last_visible = Some(position);
        }
        return HostCursor {
            position,
            visible: cursor.visible,
            shape: if cursor.shape <= 6 { cursor.shape } else { 0 },
        };
    }
    HostCursor {
        position: last_visible.unwrap_or((width.saturating_sub(1), height.saturating_sub(1))),
        visible: false,
        shape: 0,
    }
}

fn write_cursor_position(bytes: &mut Vec<u8>, (x, y): (u16, u16)) {
    let mut buf = String::with_capacity(12);
    let _ = write!(buf, "\x1b[{};{}H", y + 1, x + 1);
    bytes.extend_from_slice(buf.as_bytes());
}

fn write_cursor(bytes: &mut Vec<u8>, cursor: HostCursor, last_shape: &mut u8) {
    write_cursor_position(bytes, cursor.position);
    if cursor.shape != *last_shape {
        let mut buf = String::with_capacity(8);
        let _ = write!(buf, "\x1b[{} q", cursor.shape);
        bytes.extend_from_slice(buf.as_bytes());
        *last_shape = cursor.shape;
    }
    if cursor.visible {
        bytes.extend_from_slice(b"\x1b[?25h");
    } else {
        bytes.extend_from_slice(b"\x1b[?25l");
    }
}

fn write_all_cells(
    bytes: &mut Vec<u8>,
    cells: &[CellData],
    width: u16,
    height: u16,
    hyperlinks: &[String],
) {
    let mut last_sgr = String::new();
    let mut active_hyperlink: Option<String> = None;
    for row in 0..height {
        let mut to_skip = 0usize;
        let mut next_inline_col = None;
        for col in 0..width {
            if to_skip > 0 {
                to_skip -= 1;
                continue;
            }
            let idx = usize::from(row) * usize::from(width) + usize::from(col);
            let Some(cell) = cells.get(idx) else { continue };
            if cell.skip {
                next_inline_col = None;
                continue;
            }
            let position = (next_inline_col != Some(col)).then_some((col, row));
            write_cell(
                bytes,
                position,
                cell,
                &mut last_sgr,
                &mut active_hyperlink,
                hyperlinks,
            );
            let cell_w = cell_width(cell);
            next_inline_col =
                (cell.symbol.is_ascii() && cell_w == 1).then_some(col.saturating_add(1));
            to_skip = cell_w.saturating_sub(1);
        }
    }
    if active_hyperlink.is_some() {
        bytes.extend_from_slice(b"\x1b]8;;\x1b\\");
    }
    bytes.extend_from_slice(b"\x1b[0m");
}

fn write_changed_cells(
    bytes: &mut Vec<u8>,
    cells: &[CellData],
    width: u16,
    prev_cells: &[CellData],
    hyperlinks: &[String],
    prev_hyperlinks: &[String],
) {
    let height = (cells.len() / usize::from(width).max(1)) as u16;
    let mut last_sgr = String::new();
    let mut active_hyperlink: Option<String> = None;
    for row in 0..height {
        let mut invalidated = 0usize;
        let mut to_skip = 0usize;
        let mut next_inline_col = None;
        for col in 0..width {
            let idx = usize::from(row) * usize::from(width) + usize::from(col);
            let (Some(cell), Some(prev_cell)) = (cells.get(idx), prev_cells.get(idx)) else {
                continue;
            };
            if !cell.skip
                && (!cells_visually_equal(cell, prev_cell, hyperlinks, prev_hyperlinks)
                    || invalidated > 0)
                && to_skip == 0
            {
                let position =
                    (next_inline_col != Some(col) || invalidated > 0).then_some((col, row));
                write_cell(
                    bytes,
                    position,
                    cell,
                    &mut last_sgr,
                    &mut active_hyperlink,
                    hyperlinks,
                );
                next_inline_col =
                    (cell.symbol.is_ascii() && cell_width(cell) == 1).then_some(col + 1);
            }
            to_skip = cell_width(cell).saturating_sub(1);
            let affected_width = cmp::max(cell_width(cell), cell_width(prev_cell));
            invalidated = cmp::max(affected_width, invalidated).saturating_sub(1);
        }
    }
    if active_hyperlink.is_some() {
        bytes.extend_from_slice(b"\x1b]8;;\x1b\\");
    }
    if !last_sgr.is_empty() {
        bytes.extend_from_slice(b"\x1b[0m");
    }
}

fn cells_visually_equal(
    cell: &CellData,
    prev: &CellData,
    hyperlinks: &[String],
    prev_hyperlinks: &[String],
) -> bool {
    cell.symbol == prev.symbol
        && cell.fg == prev.fg
        && cell.bg == prev.bg
        && cell.modifier == prev.modifier
        && cell_hyperlink(cell, hyperlinks) == cell_hyperlink(prev, prev_hyperlinks)
}

fn cell_hyperlink<'a>(cell: &CellData, hyperlinks: &'a [String]) -> Option<&'a str> {
    hyperlinks.get(cell.hyperlink? as usize).map(String::as_str)
}

fn sanitized_uri(uri: &str) -> Option<String> {
    let sanitized: String = uri
        .chars()
        .filter(|ch| *ch != '\x1b' && *ch != '\x07' && !ch.is_control())
        .collect();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn write_cell(
    bytes: &mut Vec<u8>,
    cursor_position: Option<(u16, u16)>,
    cell: &CellData,
    last_sgr: &mut String,
    active_hyperlink: &mut Option<String>,
    hyperlinks: &[String],
) {
    if let Some(position) = cursor_position {
        write_cursor_position(bytes, position);
    }
    let sgr = build_sgr(cell.fg, cell.bg, cell.modifier);
    if sgr != *last_sgr {
        bytes.extend_from_slice(sgr.as_bytes());
        *last_sgr = sgr;
    }
    let requested = cell_hyperlink(cell, hyperlinks).and_then(sanitized_uri);
    if active_hyperlink.as_deref() != requested.as_deref() {
        if active_hyperlink.is_some() {
            bytes.extend_from_slice(b"\x1b]8;;\x1b\\");
        }
        if let Some(uri) = &requested {
            bytes.extend_from_slice(b"\x1b]8;;");
            bytes.extend_from_slice(uri.as_bytes());
            bytes.extend_from_slice(b"\x1b\\");
        }
        *active_hyperlink = requested;
    }
    bytes.extend_from_slice(cell.symbol.as_bytes());
}

fn cell_width(cell: &CellData) -> usize {
    if is_halfwidth_katakana_voiced_grapheme(&cell.symbol) {
        return 2;
    }
    cell.symbol.width()
}

fn is_halfwidth_katakana_voiced_grapheme(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(base) = chars.next() else {
        return false;
    };
    let Some(mark) = chars.next() else {
        return false;
    };
    chars.next().is_none()
        && ('\u{ff66}'..='\u{ff9d}').contains(&base)
        && matches!(mark, '\u{ff9e}' | '\u{ff9f}')
}

fn color_to_sgr_fg(val: u32) -> String {
    match val >> 24 {
        0x00 => match val & 0xFF {
            0x00 => "39".to_owned(),
            0x01 => "30".to_owned(),
            0x02 => "31".to_owned(),
            0x03 => "32".to_owned(),
            0x04 => "33".to_owned(),
            0x05 => "34".to_owned(),
            0x06 => "35".to_owned(),
            0x07 => "36".to_owned(),
            0x08 => "37".to_owned(),
            0x09 => "90".to_owned(),
            0x0A => "91".to_owned(),
            0x0B => "92".to_owned(),
            0x0C => "93".to_owned(),
            0x0D => "94".to_owned(),
            0x0E => "95".to_owned(),
            0x0F => "96".to_owned(),
            0x10 => "97".to_owned(),
            _ => "39".to_owned(),
        },
        0x01 => format!("38;5;{}", val & 0xFF),
        0x02 => format!(
            "38;2;{};{};{}",
            (val >> 16) & 0xFF,
            (val >> 8) & 0xFF,
            val & 0xFF
        ),
        _ => "39".to_owned(),
    }
}

fn color_to_sgr_bg(val: u32) -> String {
    match val >> 24 {
        0x00 => match val & 0xFF {
            0x00 => "49".to_owned(),
            0x01 => "40".to_owned(),
            0x02 => "41".to_owned(),
            0x03 => "42".to_owned(),
            0x04 => "43".to_owned(),
            0x05 => "44".to_owned(),
            0x06 => "45".to_owned(),
            0x07 => "46".to_owned(),
            0x08 => "47".to_owned(),
            0x09 => "100".to_owned(),
            0x0A => "101".to_owned(),
            0x0B => "102".to_owned(),
            0x0C => "103".to_owned(),
            0x0D => "104".to_owned(),
            0x0E => "105".to_owned(),
            0x0F => "106".to_owned(),
            0x10 => "107".to_owned(),
            _ => "49".to_owned(),
        },
        0x01 => format!("48;5;{}", val & 0xFF),
        0x02 => format!(
            "48;2;{};{};{}",
            (val >> 16) & 0xFF,
            (val >> 8) & 0xFF,
            val & 0xFF
        ),
        _ => "49".to_owned(),
    }
}

fn underline_style_from_modifier(modifier: u16) -> u8 {
    ((modifier & UNDERLINE_STYLE_MASK) >> UNDERLINE_STYLE_SHIFT) as u8
}

fn modifier_to_sgr_parts(val: u16) -> Vec<&'static str> {
    const BOLD: u16 = 1 << 0;
    const DIM: u16 = 1 << 1;
    const ITALIC: u16 = 1 << 2;
    const UNDERLINED: u16 = 1 << 3;
    const SLOW_BLINK: u16 = 1 << 4;
    const RAPID_BLINK: u16 = 1 << 5;
    const HIDDEN: u16 = 1 << 7;
    const CROSSED_OUT: u16 = 1 << 8;
    let mut parts = Vec::new();
    if val & BOLD != 0 {
        parts.push("1");
    }
    if val & DIM != 0 {
        parts.push("2");
    }
    if val & ITALIC != 0 {
        parts.push("3");
    }
    if val & UNDERLINED != 0 {
        parts.push(match underline_style_from_modifier(val) {
            2 => "4:2",
            3 => "4:3",
            4 => "4:4",
            5 => "4:5",
            _ => "4",
        });
    }
    if val & SLOW_BLINK != 0 {
        parts.push("5");
    }
    if val & RAPID_BLINK != 0 {
        parts.push("6");
    }
    if val & REVERSED_MODIFIER != 0 {
        parts.push("7");
    }
    if val & HIDDEN != 0 {
        parts.push("8");
    }
    if val & CROSSED_OUT != 0 {
        parts.push("9");
    }
    parts
}

fn build_sgr(fg: u32, bg: u32, modifier: u16) -> String {
    let mut parts = vec!["0".to_owned()];
    parts.extend(
        modifier_to_sgr_parts(modifier)
            .into_iter()
            .map(str::to_owned),
    );
    parts.push(color_to_sgr_fg(fg));
    parts.push(color_to_sgr_bg(bg));
    format!("\x1b[{}m", parts.join(";"))
}
