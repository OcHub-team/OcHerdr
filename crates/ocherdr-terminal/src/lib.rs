//! Native Ghostty Metal surfaces for embedding terminal frames in GPUI.

#![allow(
    deprecated,
    reason = "core-video 0.5 requires the matching io-surface wrapper"
)]

use std::ffi::{CString, c_char, c_void};
use std::os::unix::ffi::OsStrExt as _;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, OnceLock, Weak};

use core_foundation::base::TCFType as _;
use core_video::pixel_buffer::CVPixelBuffer;
use io_surface::{IOSurface, IOSurfaceRef};
use thiserror::Error;

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/ghostty_bindings.rs"));
}

static RUNTIME: OnceLock<Result<GhosttyRuntime, String>> = OnceLock::new();

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("failed to initialize Ghostty: {0}")]
    Initialization(String),
    #[error("Ghostty could not create a Metal terminal surface")]
    SurfaceCreation,
    #[error("Ghostty rejected the requested terminal grid")]
    InvalidGrid,
    #[error("CoreVideo could not wrap Ghostty's IOSurface (status {0})")]
    FrameConversion(i32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
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

impl From<ffi::ghostty_surface_size_s> for SurfaceSize {
    fn from(size: ffi::ghostty_surface_size_s) -> Self {
        Self {
            columns: size.columns,
            rows: size.rows,
            width_px: size.width_px,
            height_px: size.height_px,
            cell_width_px: size.cell_width_px,
            cell_height_px: size.cell_height_px,
        }
    }
}

#[derive(Clone)]
pub struct RenderedFrame {
    pub pixel_buffer: CVPixelBuffer,
    pub width_px: u32,
    pub height_px: u32,
    pub host_context: u64,
    pub lifetime: Arc<FrameLease>,
}

pub struct FrameLease {
    surface: Arc<SurfaceCore>,
    token: AtomicU64,
}

impl Drop for FrameLease {
    fn drop(&mut self) {
        let token = self.token.swap(0, Ordering::AcqRel);
        let raw = self.surface.raw.load(Ordering::Acquire);
        if token != 0 && !raw.is_null() {
            // SAFETY: leased-frame release is explicitly thread-safe. `surface` keeps
            // the Ghostty surface alive until this call has returned.
            unsafe {
                ffi::ghostty_surface_release_external_frame(raw, token);
            }
        }
    }
}

struct PendingFrame {
    iosurface: usize,
    width_px: u32,
    height_px: u32,
    host_context: u64,
    lifetime: Arc<FrameLease>,
}

struct CallbackState {
    surface: Weak<SurfaceCore>,
    frames: SyncSender<PendingFrame>,
    input: mpsc::Sender<Vec<u8>>,
}

struct SurfaceCore {
    raw: AtomicPtr<c_void>,
    callback_state: AtomicPtr<CallbackState>,
}

// Ghostty documents leased-frame release as thread-safe. All other surface calls
// remain on GPUI's application thread; Send + Sync are needed only so completion
// handlers can own and release a lease.
unsafe impl Send for SurfaceCore {}
unsafe impl Sync for SurfaceCore {}

impl Drop for SurfaceCore {
    fn drop(&mut self) {
        let raw = self.raw.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !raw.is_null() {
            // SAFETY: this is the last strong owner. Frame leases also own this core,
            // so no acquired IOSurface remains when surface teardown begins.
            unsafe {
                ffi::ghostty_surface_request_process_termination(raw);
                ffi::ghostty_surface_free(raw);
            }
        }

        let callback_state = self
            .callback_state
            .swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !callback_state.is_null() {
            // SAFETY: the pointer was created by Arc::into_raw exactly once and is
            // reclaimed only after Ghostty has stopped invoking surface callbacks.
            unsafe {
                drop(Arc::from_raw(callback_state));
            }
        }
    }
}

struct GhosttyRuntime {
    app: NonNull<c_void>,
}

// The runtime is process-global and Ghostty owns its internal synchronization.
// OcHerdr calls app APIs only from GPUI's application thread.
unsafe impl Send for GhosttyRuntime {}
unsafe impl Sync for GhosttyRuntime {}

impl GhosttyRuntime {
    fn shared() -> Result<&'static Self, TerminalError> {
        RUNTIME
            .get_or_init(Self::initialize)
            .as_ref()
            .map_err(|error| TerminalError::Initialization(error.clone()))
    }

    fn initialize() -> Result<Self, String> {
        let mut arguments = std::env::args_os()
            .filter_map(|argument| CString::new(argument.as_bytes()).ok())
            .collect::<Vec<_>>();
        if arguments.is_empty() {
            arguments.push(CString::new("ocherdr").expect("static string has no NUL"));
        }
        let mut argument_pointers = arguments
            .iter_mut()
            .map(|argument| argument.as_ptr().cast_mut())
            .collect::<Vec<*mut c_char>>();
        // SAFETY: the argument strings and pointer array remain alive for the call.
        let result =
            unsafe { ffi::ghostty_init(argument_pointers.len(), argument_pointers.as_mut_ptr()) };
        if result != ffi::GHOSTTY_SUCCESS as i32 {
            return Err(format!("ghostty_init returned {result}"));
        }

        // SAFETY: configuration and runtime calls follow Ghostty's ownership rules.
        unsafe {
            let config = ffi::ghostty_config_new();
            if config.is_null() {
                return Err("ghostty_config_new returned null".into());
            }
            ffi::ghostty_config_load_default_files(config);
            ffi::ghostty_config_load_recursive_files(config);
            ffi::ghostty_config_finalize(config);

            let runtime = ffi::ghostty_runtime_config_s {
                userdata: std::ptr::null_mut(),
                supports_selection_clipboard: false,
                wakeup_cb: Some(runtime_wakeup),
                action_cb: Some(runtime_action),
                read_clipboard_cb: None,
                confirm_read_clipboard_cb: None,
                write_clipboard_cb: None,
                close_surface_cb: Some(runtime_close_surface),
                tmux_control_cb: None,
            };
            let app = ffi::ghostty_app_new(&runtime, config);
            ffi::ghostty_config_free(config);
            let app =
                NonNull::new(app).ok_or_else(|| "ghostty_app_new returned null".to_owned())?;
            ffi::ghostty_app_set_focus(app.as_ptr(), true);
            Ok(Self { app })
        }
    }

    fn tick(&self) {
        // SAFETY: the process-global app remains alive for the duration of OcHerdr.
        unsafe { ffi::ghostty_app_tick(self.app.as_ptr()) };
    }

    fn set_color_scheme(&self, dark: bool) {
        let scheme = if dark {
            ffi::ghostty_color_scheme_e_GHOSTTY_COLOR_SCHEME_DARK
        } else {
            ffi::ghostty_color_scheme_e_GHOSTTY_COLOR_SCHEME_LIGHT
        };
        // SAFETY: the process-global app remains alive for the duration of OcHerdr.
        unsafe { ffi::ghostty_app_set_color_scheme(self.app.as_ptr(), scheme) };
    }
}

unsafe extern "C" fn runtime_wakeup(_userdata: *mut c_void) {}

unsafe extern "C" fn runtime_action(
    _app: ffi::ghostty_app_t,
    _target: ffi::ghostty_target_s,
    _action: ffi::ghostty_action_s,
) -> bool {
    false
}

unsafe extern "C" fn runtime_close_surface(_userdata: *mut c_void, _process_alive: bool) {}

unsafe extern "C" fn present_frame(
    userdata: *mut c_void,
    frame: *const ffi::ghostty_metal_external_frame_s,
) -> ffi::ghostty_metal_external_frame_disposition_e {
    if userdata.is_null() || frame.is_null() {
        return ffi::ghostty_metal_external_frame_disposition_e_GHOSTTY_METAL_EXTERNAL_FRAME_DROP;
    }

    // SAFETY: callback userdata is an Arc kept alive by SurfaceCore until after
    // ghostty_surface_free, and Ghostty guarantees `frame` for this callback.
    let callback = unsafe { &*(userdata.cast::<CallbackState>()) };
    let Some(surface) = callback.surface.upgrade() else {
        return ffi::ghostty_metal_external_frame_disposition_e_GHOSTTY_METAL_EXTERNAL_FRAME_DROP;
    };
    // SAFETY: validated above and borrowed only for this callback.
    let frame = unsafe { &*frame };
    if frame.iosurface.is_null() || frame.frame_token == 0 {
        return ffi::ghostty_metal_external_frame_disposition_e_GHOSTTY_METAL_EXTERNAL_FRAME_DROP;
    }

    let pending = PendingFrame {
        iosurface: frame.iosurface as usize,
        width_px: frame.width_px,
        height_px: frame.height_px,
        host_context: frame.host_context,
        lifetime: Arc::new(FrameLease {
            surface,
            token: AtomicU64::new(frame.frame_token),
        }),
    };
    match callback.frames.try_send(pending) {
        Ok(()) => {
            ffi::ghostty_metal_external_frame_disposition_e_GHOSTTY_METAL_EXTERNAL_FRAME_ACQUIRE
        }
        Err(TrySendError::Full(pending) | TrySendError::Disconnected(pending)) => {
            pending.lifetime.token.store(0, Ordering::Release);
            ffi::ghostty_metal_external_frame_disposition_e_GHOSTTY_METAL_EXTERNAL_FRAME_DROP
        }
    }
}

unsafe extern "C" fn write_input(userdata: *mut c_void, bytes: *const c_char, len: usize) {
    if userdata.is_null() || bytes.is_null() || len == 0 {
        return;
    }
    // SAFETY: the callback state and byte slice are valid for this callback.
    let callback = unsafe { &*(userdata.cast::<CallbackState>()) };
    let bytes = unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), len) };
    let _ = callback.input.send(bytes.to_vec());
}

pub struct Terminal {
    surface: Arc<SurfaceCore>,
    frames: Receiver<PendingFrame>,
    input: Receiver<Vec<u8>>,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, scrollback: usize, dark: bool) -> Result<Self, TerminalError> {
        let runtime = GhosttyRuntime::shared()?;
        runtime.set_color_scheme(dark);
        let (frame_sender, frames) = mpsc::sync_channel(3);
        let (input_sender, input) = mpsc::channel();
        let surface = Arc::new(SurfaceCore {
            raw: AtomicPtr::new(std::ptr::null_mut()),
            callback_state: AtomicPtr::new(std::ptr::null_mut()),
        });
        let callback = Arc::new(CallbackState {
            surface: Arc::downgrade(&surface),
            frames: frame_sender,
            input: input_sender,
        });
        let callback_pointer = Arc::into_raw(callback).cast_mut();
        surface
            .callback_state
            .store(callback_pointer, Ordering::Release);

        // SAFETY: all pointers in the surface config remain live through construction.
        let raw = unsafe {
            let mut config = ffi::ghostty_surface_config_new();
            config.platform_tag = ffi::ghostty_platform_e_GHOSTTY_PLATFORM_METAL_EXTERNAL_LEASED;
            config.platform = ffi::ghostty_platform_u {
                metal_external_leased: ffi::ghostty_platform_metal_external_leased_s {
                    userdata: callback_pointer.cast(),
                    present: Some(present_frame),
                },
            };
            config.userdata = callback_pointer.cast();
            config.scale_factor = 1.0;
            config.font_size = 0.0;
            config.context = ffi::ghostty_surface_context_e_GHOSTTY_SURFACE_CONTEXT_SPLIT;
            config.io_mode = ffi::ghostty_surface_io_mode_e_GHOSTTY_SURFACE_IO_MANUAL_MIRROR;
            config.io_write_cb = Some(write_input);
            config.io_write_userdata = callback_pointer.cast();
            ffi::ghostty_surface_new_with_scrollback_limit(
                runtime.app.as_ptr(),
                &config,
                scrollback.saturating_mul(1024),
            )
        };
        let Some(raw) = NonNull::new(raw) else {
            return Err(TerminalError::SurfaceCreation);
        };
        surface.raw.store(raw.as_ptr(), Ordering::Release);

        let terminal = Self {
            surface,
            frames,
            input,
        };
        terminal.set_grid_size(cols, rows)?;
        terminal.set_color_scheme(dark);
        terminal.set_focus(false);
        terminal.refresh();
        Ok(terminal)
    }

    pub fn tick_runtime() -> Result<(), TerminalError> {
        GhosttyRuntime::shared()?.tick();
        Ok(())
    }

    pub fn apply_frame(&self, bytes: &[u8], _full: bool) {
        if bytes.is_empty() {
            return;
        }
        // SAFETY: the surface is alive and Ghostty borrows the bytes only for the call.
        unsafe {
            ffi::ghostty_surface_process_output(
                self.raw(),
                bytes.as_ptr().cast::<c_char>(),
                bytes.len(),
            );
            ffi::ghostty_surface_refresh(self.raw());
        }
    }

    pub fn set_grid_size(&self, cols: u16, rows: u16) -> Result<SurfaceSize, TerminalError> {
        let mut resolved = ffi::ghostty_surface_size_s {
            columns: 0,
            rows: 0,
            width_px: 0,
            height_px: 0,
            cell_width_px: 0,
            cell_height_px: 0,
        };
        // SAFETY: the surface is live and `resolved` is writable for the call.
        let changed = unsafe {
            ffi::ghostty_surface_set_grid_size(self.raw(), cols.max(1), rows.max(1), &mut resolved)
        };
        if changed {
            Ok(resolved.into())
        } else {
            Err(TerminalError::InvalidGrid)
        }
    }

    pub fn resize_pixels(
        &self,
        width_px: u32,
        height_px: u32,
        scale_factor: f64,
        host_context: u64,
    ) -> SurfaceSize {
        // SAFETY: these mutate the live surface on GPUI's application thread.
        unsafe {
            ffi::ghostty_surface_set_external_frame_context(self.raw(), host_context);
            ffi::ghostty_surface_set_content_scale(
                self.raw(),
                scale_factor.max(1.0),
                scale_factor.max(1.0),
            );
            ffi::ghostty_surface_set_size(self.raw(), width_px.max(1), height_px.max(1));
            ffi::ghostty_surface_size(self.raw()).into()
        }
    }

    pub fn try_frame(&self) -> Result<Option<RenderedFrame>, TerminalError> {
        let mut newest = match self.frames.try_recv() {
            Ok(frame) => frame,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(None),
        };
        while let Ok(frame) = self.frames.try_recv() {
            newest = frame;
        }

        // SAFETY: an acquired frame keeps the borrowed IOSurface alive until the
        // lifetime guard is dropped. Both wrappers retain their underlying object.
        let iosurface = unsafe { IOSurface::wrap_under_get_rule(newest.iosurface as IOSurfaceRef) };
        let pixel_buffer = CVPixelBuffer::from_io_surface(&iosurface, None)
            .map_err(TerminalError::FrameConversion)?;
        Ok(Some(RenderedFrame {
            pixel_buffer,
            width_px: newest.width_px,
            height_px: newest.height_px,
            host_context: newest.host_context,
            lifetime: newest.lifetime,
        }))
    }

    pub fn try_input(&self) -> Option<Vec<u8>> {
        self.input.try_recv().ok()
    }

    pub fn send_key(&self, key: &str, text: Option<&str>, modifiers: KeyModifiers) -> bool {
        if !modifiers.control
            && !modifiers.alt
            && let Some(text) = text.filter(|text| !text.is_empty())
        {
            self.send_committed_text(text);
            return true;
        }

        let keycode = ghostty_key(key);
        if keycode == ffi::ghostty_input_key_e_GHOSTTY_KEY_UNIDENTIFIED {
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                self.send_committed_text(text);
                return true;
            }
            return false;
        }
        let text = text.and_then(|text| CString::new(text).ok());
        let input = ffi::ghostty_input_key_s {
            action: ffi::ghostty_input_action_e_GHOSTTY_ACTION_PRESS,
            mods: ghostty_modifiers(modifiers),
            consumed_mods: ffi::ghostty_input_mods_e_GHOSTTY_MODS_NONE,
            keycode,
            text: text.as_ref().map_or(std::ptr::null(), |text| text.as_ptr()),
            unshifted_codepoint: key
                .chars()
                .next()
                .filter(|_| key.chars().count() == 1)
                .map_or(0, u32::from),
            composing: false,
        };
        // SAFETY: key events are sent synchronously on GPUI's application thread.
        unsafe { ffi::ghostty_surface_key(self.raw(), input) }
    }

    pub fn paste(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        // SAFETY: Ghostty borrows the UTF-8 bytes only for this call.
        unsafe {
            ffi::ghostty_surface_text(self.raw(), text.as_ptr().cast::<c_char>(), text.len())
        };
    }

    pub fn set_focus(&self, focused: bool) {
        // SAFETY: the surface is live and called on GPUI's application thread.
        unsafe { ffi::ghostty_surface_set_focus(self.raw(), focused) };
    }

    pub fn set_color_scheme(&self, dark: bool) {
        let scheme = if dark {
            ffi::ghostty_color_scheme_e_GHOSTTY_COLOR_SCHEME_DARK
        } else {
            ffi::ghostty_color_scheme_e_GHOSTTY_COLOR_SCHEME_LIGHT
        };
        // SAFETY: the surface is live and called on GPUI's application thread.
        unsafe { ffi::ghostty_surface_set_color_scheme(self.raw(), scheme) };
        self.refresh();
    }

    pub fn refresh(&self) {
        // SAFETY: the surface is live and called on GPUI's application thread.
        unsafe { ffi::ghostty_surface_refresh(self.raw()) };
    }

    /// Visible viewport text for assistive technology. `None` if Ghostty cannot
    /// produce a selection; an empty string means the screen is blank.
    pub fn read_visible_text(&self) -> Option<String> {
        let mut result = ffi::ghostty_text_s {
            tl_px_x: 0.0,
            tl_px_y: 0.0,
            offset_start: 0,
            offset_len: 0,
            text: std::ptr::null(),
            text_len: 0,
        };
        let sel = ffi::ghostty_selection_s {
            top_left: ffi::ghostty_point_s {
                tag: ffi::ghostty_point_tag_e_GHOSTTY_POINT_VIEWPORT,
                coord: ffi::ghostty_point_coord_e_GHOSTTY_POINT_COORD_TOP_LEFT,
                x: 0,
                y: 0,
            },
            bottom_right: ffi::ghostty_point_s {
                tag: ffi::ghostty_point_tag_e_GHOSTTY_POINT_VIEWPORT,
                coord: ffi::ghostty_point_coord_e_GHOSTTY_POINT_COORD_BOTTOM_RIGHT,
                x: 0,
                y: 0,
            },
            rectangle: false,
        };
        // SAFETY: the surface is live; Ghostty fills `result` and the text
        // pointer remains valid until `ghostty_surface_free_text`.
        let ok = unsafe { ffi::ghostty_surface_read_text(self.raw(), sel, &mut result) };
        if !ok {
            return None;
        }
        let text = if result.text.is_null() || result.text_len == 0 {
            String::new()
        } else {
            // SAFETY: `text_len` is the length Ghostty reported for this buffer.
            let bytes =
                unsafe { std::slice::from_raw_parts(result.text.cast::<u8>(), result.text_len) };
            String::from_utf8_lossy(bytes).into_owned()
        };
        if !result.text.is_null() {
            unsafe { ffi::ghostty_surface_free_text(self.raw(), &mut result) };
        }
        Some(text)
    }

    fn send_committed_text(&self, text: &str) {
        // SAFETY: Ghostty borrows the UTF-8 bytes only for this call.
        unsafe {
            ffi::ghostty_surface_text_input(self.raw(), text.as_ptr().cast::<c_char>(), text.len())
        };
    }

    fn raw(&self) -> ffi::ghostty_surface_t {
        let raw = self.surface.raw.load(Ordering::Acquire);
        debug_assert!(!raw.is_null());
        raw
    }
}

fn ghostty_modifiers(modifiers: KeyModifiers) -> ffi::ghostty_input_mods_e {
    let mut result = ffi::ghostty_input_mods_e_GHOSTTY_MODS_NONE;
    if modifiers.shift {
        result |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT;
    }
    if modifiers.control {
        result |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL;
    }
    if modifiers.alt {
        result |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_ALT;
    }
    if modifiers.platform {
        result |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_SUPER;
    }
    result
}

fn ghostty_key(key: &str) -> ffi::ghostty_input_key_e {
    match key.to_ascii_lowercase().as_str() {
        "a" => ffi::ghostty_input_key_e_GHOSTTY_KEY_A,
        "b" => ffi::ghostty_input_key_e_GHOSTTY_KEY_B,
        "c" => ffi::ghostty_input_key_e_GHOSTTY_KEY_C,
        "d" => ffi::ghostty_input_key_e_GHOSTTY_KEY_D,
        "e" => ffi::ghostty_input_key_e_GHOSTTY_KEY_E,
        "f" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F,
        "g" => ffi::ghostty_input_key_e_GHOSTTY_KEY_G,
        "h" => ffi::ghostty_input_key_e_GHOSTTY_KEY_H,
        "i" => ffi::ghostty_input_key_e_GHOSTTY_KEY_I,
        "j" => ffi::ghostty_input_key_e_GHOSTTY_KEY_J,
        "k" => ffi::ghostty_input_key_e_GHOSTTY_KEY_K,
        "l" => ffi::ghostty_input_key_e_GHOSTTY_KEY_L,
        "m" => ffi::ghostty_input_key_e_GHOSTTY_KEY_M,
        "n" => ffi::ghostty_input_key_e_GHOSTTY_KEY_N,
        "o" => ffi::ghostty_input_key_e_GHOSTTY_KEY_O,
        "p" => ffi::ghostty_input_key_e_GHOSTTY_KEY_P,
        "q" => ffi::ghostty_input_key_e_GHOSTTY_KEY_Q,
        "r" => ffi::ghostty_input_key_e_GHOSTTY_KEY_R,
        "s" => ffi::ghostty_input_key_e_GHOSTTY_KEY_S,
        "t" => ffi::ghostty_input_key_e_GHOSTTY_KEY_T,
        "u" => ffi::ghostty_input_key_e_GHOSTTY_KEY_U,
        "v" => ffi::ghostty_input_key_e_GHOSTTY_KEY_V,
        "w" => ffi::ghostty_input_key_e_GHOSTTY_KEY_W,
        "x" => ffi::ghostty_input_key_e_GHOSTTY_KEY_X,
        "y" => ffi::ghostty_input_key_e_GHOSTTY_KEY_Y,
        "z" => ffi::ghostty_input_key_e_GHOSTTY_KEY_Z,
        "0" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_0,
        "1" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_1,
        "2" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_2,
        "3" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_3,
        "4" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_4,
        "5" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_5,
        "6" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_6,
        "7" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_7,
        "8" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_8,
        "9" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DIGIT_9,
        "`" => ffi::ghostty_input_key_e_GHOSTTY_KEY_BACKQUOTE,
        "\\" => ffi::ghostty_input_key_e_GHOSTTY_KEY_BACKSLASH,
        "[" => ffi::ghostty_input_key_e_GHOSTTY_KEY_BRACKET_LEFT,
        "]" => ffi::ghostty_input_key_e_GHOSTTY_KEY_BRACKET_RIGHT,
        "," => ffi::ghostty_input_key_e_GHOSTTY_KEY_COMMA,
        "=" => ffi::ghostty_input_key_e_GHOSTTY_KEY_EQUAL,
        "-" => ffi::ghostty_input_key_e_GHOSTTY_KEY_MINUS,
        "." => ffi::ghostty_input_key_e_GHOSTTY_KEY_PERIOD,
        "'" => ffi::ghostty_input_key_e_GHOSTTY_KEY_QUOTE,
        ";" => ffi::ghostty_input_key_e_GHOSTTY_KEY_SEMICOLON,
        "/" => ffi::ghostty_input_key_e_GHOSTTY_KEY_SLASH,
        "space" | " " => ffi::ghostty_input_key_e_GHOSTTY_KEY_SPACE,
        "enter" | "return" => ffi::ghostty_input_key_e_GHOSTTY_KEY_ENTER,
        "tab" => ffi::ghostty_input_key_e_GHOSTTY_KEY_TAB,
        "backspace" => ffi::ghostty_input_key_e_GHOSTTY_KEY_BACKSPACE,
        "escape" => ffi::ghostty_input_key_e_GHOSTTY_KEY_ESCAPE,
        "delete" => ffi::ghostty_input_key_e_GHOSTTY_KEY_DELETE,
        "home" => ffi::ghostty_input_key_e_GHOSTTY_KEY_HOME,
        "end" => ffi::ghostty_input_key_e_GHOSTTY_KEY_END,
        "pageup" => ffi::ghostty_input_key_e_GHOSTTY_KEY_PAGE_UP,
        "pagedown" => ffi::ghostty_input_key_e_GHOSTTY_KEY_PAGE_DOWN,
        "up" => ffi::ghostty_input_key_e_GHOSTTY_KEY_ARROW_UP,
        "down" => ffi::ghostty_input_key_e_GHOSTTY_KEY_ARROW_DOWN,
        "left" => ffi::ghostty_input_key_e_GHOSTTY_KEY_ARROW_LEFT,
        "right" => ffi::ghostty_input_key_e_GHOSTTY_KEY_ARROW_RIGHT,
        "f1" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F1,
        "f2" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F2,
        "f3" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F3,
        "f4" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F4,
        "f5" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F5,
        "f6" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F6,
        "f7" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F7,
        "f8" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F8,
        "f9" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F9,
        "f10" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F10,
        "f11" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F11,
        "f12" => ffi::ghostty_input_key_e_GHOSTTY_KEY_F12,
        _ => ffi::ghostty_input_key_e_GHOSTTY_KEY_UNIDENTIFIED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_navigation_and_printable_keys() {
        assert_eq!(ghostty_key("a"), ffi::ghostty_input_key_e_GHOSTTY_KEY_A);
        assert_eq!(
            ghostty_key("pageup"),
            ffi::ghostty_input_key_e_GHOSTTY_KEY_PAGE_UP
        );
        assert_eq!(
            ghostty_key("return"),
            ffi::ghostty_input_key_e_GHOSTTY_KEY_ENTER
        );
        assert_eq!(
            ghostty_key("unknown"),
            ffi::ghostty_input_key_e_GHOSTTY_KEY_UNIDENTIFIED
        );
    }

    #[test]
    fn maps_modifier_bits() {
        let modifiers = ghostty_modifiers(KeyModifiers {
            control: true,
            alt: true,
            shift: false,
            platform: false,
        });
        assert_ne!(modifiers & ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL, 0);
        assert_ne!(modifiers & ffi::ghostty_input_mods_e_GHOSTTY_MODS_ALT, 0);
    }
}
