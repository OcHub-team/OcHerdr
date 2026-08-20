//! A small, owned Rust surface over the vendored native `libghostty-vt`.

use std::ffi::c_int;
use std::ptr::NonNull;

use thiserror::Error;

#[repr(C)]
struct NativeTerminal {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn ocherdr_terminal_new(
        cols: u16,
        rows: u16,
        scrollback: usize,
        out: *mut *mut NativeTerminal,
    ) -> c_int;
    fn ocherdr_terminal_free(terminal: *mut NativeTerminal);
    fn ocherdr_terminal_reset(terminal: *mut NativeTerminal);
    fn ocherdr_terminal_write(terminal: *mut NativeTerminal, bytes: *const u8, length: usize);
    fn ocherdr_terminal_resize(
        terminal: *mut NativeTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> c_int;
    fn ocherdr_terminal_text(
        terminal: *mut NativeTerminal,
        buffer: *mut u8,
        capacity: usize,
    ) -> usize;
    fn ocherdr_terminal_cols(terminal: *const NativeTerminal) -> u16;
    fn ocherdr_terminal_rows(terminal: *const NativeTerminal) -> u16;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TerminalError {
    #[error("libghostty-vt failed with result {0}")]
    Ghostty(i32),
}

pub struct Terminal {
    raw: NonNull<NativeTerminal>,
}

// The native object is owned and has no callbacks. OcHerdr still mutates it
// behind a single lock; this declaration lets terminal decoding run off the UI thread.
unsafe impl Send for Terminal {}

impl Terminal {
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Result<Self, TerminalError> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: `raw` points to writable storage and the constructor owns its result.
        let result =
            unsafe { ocherdr_terminal_new(cols.max(1), rows.max(1), scrollback, &mut raw) };
        if result != 0 {
            return Err(TerminalError::Ghostty(result));
        }
        Ok(Self {
            raw: NonNull::new(raw).ok_or(TerminalError::Ghostty(-1))?,
        })
    }

    pub fn apply_frame(&mut self, bytes: &[u8], full: bool) {
        // SAFETY: the pointer is valid for `self`; libghostty-vt processes borrowed bytes inline.
        unsafe {
            if full {
                ocherdr_terminal_reset(self.raw.as_ptr());
            }
            ocherdr_terminal_write(self.raw.as_ptr(), bytes.as_ptr(), bytes.len());
        }
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), TerminalError> {
        // SAFETY: the terminal is exclusively borrowed.
        let result = unsafe {
            ocherdr_terminal_resize(
                self.raw.as_ptr(),
                cols.max(1),
                rows.max(1),
                cell_width_px,
                cell_height_px,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(TerminalError::Ghostty(result))
        }
    }

    pub fn text(&self) -> String {
        // SAFETY: a null output buffer is the documented size query.
        let needed = unsafe { ocherdr_terminal_text(self.raw.as_ptr(), std::ptr::null_mut(), 0) };
        if needed == 0 {
            return String::new();
        }
        let mut bytes = vec![0_u8; needed];
        // SAFETY: the buffer has exactly the capacity provided to the C API.
        let written =
            unsafe { ocherdr_terminal_text(self.raw.as_ptr(), bytes.as_mut_ptr(), bytes.len()) };
        bytes.truncate(written.min(bytes.len()));
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn size(&self) -> (u16, u16) {
        // SAFETY: getters only read the live native object.
        unsafe {
            (
                ocherdr_terminal_cols(self.raw.as_ptr()),
                ocherdr_terminal_rows(self.raw.as_ptr()),
            )
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: this is the unique owner and the pointer is never used again.
        unsafe { ocherdr_terminal_free(self.raw.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghostty_tracks_ansi_and_unicode_screen_state() {
        let mut terminal = Terminal::new(20, 4, 200).unwrap();
        terminal.apply_frame(
            b"hello\r\n\x1b[31mwide: \xE9\x97\xAA\xE7\x94\xB5\x1b[0m",
            true,
        );
        let text = terminal.text();
        assert!(text.contains("hello"));
        assert!(text.contains("wide: \u{95ea}\u{7535}"));
        assert_eq!(terminal.size(), (20, 4));
    }

    #[test]
    fn resize_reflows_without_losing_content() {
        let mut terminal = Terminal::new(8, 3, 200).unwrap();
        terminal.apply_frame(b"a wrapped line", true);
        terminal.resize(16, 3, 8, 16).unwrap();
        assert!(terminal.text().contains("a wrapped line"));
    }
}
