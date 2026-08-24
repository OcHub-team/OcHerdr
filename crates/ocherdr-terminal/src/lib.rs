//! Native Ghostty Metal surfaces for embedding terminal frames in GPUI.

#![allow(
    deprecated,
    reason = "core-video 0.5 requires the matching io-surface wrapper"
)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock, Weak};
use std::task::{Context, Poll};

use futures::Stream;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};

use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, TCFType as _};
use core_foundation::data::CFData;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
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

const MOUSE_REPORTING_RESET: &[u8] =
    b"\x1b[?1006l\x1b[?1016l\x1b[?1015l\x1b[?1005l\x1b[?1003l\x1b[?1002l\x1b[?1000l";
const MOUSE_REPORTING_ENABLE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1015h\x1b[?1006h";
const PIXEL_MOUSE_REPORTING_ENABLE: &[u8] = b"\x1b[?1016h";

fn mouse_capture_sequence(enabled: bool, sgr_pixels: bool) -> Vec<u8> {
    let mut sequence = MOUSE_REPORTING_RESET.to_vec();
    if enabled {
        sequence.extend_from_slice(MOUSE_REPORTING_ENABLE);
        if sgr_pixels {
            sequence.extend_from_slice(PIXEL_MOUSE_REPORTING_ENABLE);
        }
    }
    sequence
}

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
pub enum SurfaceMouseButton {
    Left,
    Right,
    Middle,
}

/// Same family Ghostty embeds as the default Latin face. Configured
/// `font-family-bold` entries are discovered *before* that embedded Bold
/// (SharedGridSet.collection at 3da10da).
const JETBRAINS_MONO: &str = "JetBrains Mono";

/// macOS CJK families with a bold trait (Semibold / W6 / Bold). Order is the
/// fallback used when no preferred language tag selects a script.
const CJK_FALLBACK_FAMILIES: &[&str] = &[
    "PingFang SC",
    "PingFang TC",
    "Hiragino Sans",
    "Apple SD Gothic Neo",
];

const CT_FONT_MANAGER_SCOPE_PROCESS: u32 = 1;

/// Colors and type settings for the embedded Ghostty surface.
/// Color values are `0xRRGGBB`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalPalette {
    pub dark: bool,
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub selection: u32,
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

    fn config_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "background = {}", hex_color(self.background));
        let _ = writeln!(out, "foreground = {}", hex_color(self.foreground));
        let _ = writeln!(out, "cursor-color = {}", hex_color(self.cursor));
        let _ = writeln!(out, "cursor-text = {}", hex_color(self.background));
        let _ = writeln!(out, "selection-background = {}", hex_color(self.selection));
        let _ = writeln!(out, "selection-foreground = {}", hex_color(self.foreground));
        for (index, color) in self.ansi.iter().copied().enumerate() {
            let _ = writeln!(out, "palette = {index}={}", hex_color(color));
        }
        write_font_families(&mut out, self.font_family.trim());
        let _ = writeln!(out, "font-size = {}", self.font_size.clamp(8, 32));
        for feature in &self.font_features {
            let _ = writeln!(out, "font-feature = {feature}");
        }
        if self.thicken {
            let _ = writeln!(out, "font-thicken = true");
            let _ = writeln!(out, "font-thicken-strength = {}", self.thicken_strength);
        }
        if let Some(value) = &self.cell_width {
            let _ = writeln!(out, "adjust-cell-width = {value}");
        }
        if let Some(value) = &self.cell_height {
            let _ = writeln!(out, "adjust-cell-height = {value}");
        }
        if self.padding_x != 0 {
            let _ = writeln!(out, "window-padding-x = {}", self.padding_x);
        }
        if self.padding_y != 0 {
            let _ = writeln!(out, "window-padding-y = {}", self.padding_y);
        }
        out
    }
}

fn write_font_families(out: &mut String, primary: &str) {
    let tags = preferred_language_tags();
    let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
    let cjk = cjk_families_for_languages(&tags);
    if primary.is_empty() {
        ensure_jetbrains_mono_bold_registered();
        if !jetbrains_mono_family_available() {
            // CJK faces include Latin. Without a JetBrains Mono bold head they
            // would become Latin bold (CodepointResolver.getIndex walks the
            // bold collection in insertion order).
            return;
        }
    }
    write_font_families_with(out, primary, &cjk);
}

fn write_font_families_with(out: &mut String, primary: &str, cjk: &[&str]) {
    if primary.is_empty() {
        // CodepointResolver.getIndex (3da10da): look up the requested style
        // first; missing bold CJK recurses to regular; only the regular
        // branch runs discoverFallback. CJK bold therefore needs a bold CJK
        // face already on the bold list.
        //
        // SharedGridSet.collection discovers configured font-family-bold
        // *before* completeStyles and *before* the embedded JetBrains Mono
        // Bold (fallback=true). PingFang/Hiragino include Latin, so they
        // cannot be first: Latin 'A' would resolve to a proportional CJK
        // face. Anchor with JetBrains Mono so Latin bold stays in the same
        // family as the embedded regular face, then append locale-sorted
        // CJK. Do not set font-family (regular) here — that list stays
        // empty until Ghostty appends embedded JetBrains Mono.
        let _ = writeln!(out, "font-family-bold = {}", ghostty_quoted(JETBRAINS_MONO));
        for family in cjk {
            if *family == JETBRAINS_MONO {
                continue;
            }
            let _ = writeln!(out, "font-family-bold = {}", ghostty_quoted(family));
        }
        return;
    }
    let _ = writeln!(out, "font-family = {}", ghostty_quoted(primary));
    let _ = writeln!(out, "font-family-bold = {}", ghostty_quoted(primary));
    for family in cjk {
        if *family == primary {
            continue;
        }
        let _ = writeln!(out, "font-family = {}", ghostty_quoted(family));
        let _ = writeln!(out, "font-family-bold = {}", ghostty_quoted(family));
    }
}

fn ensure_jetbrains_mono_bold_registered() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        let bytes: &[u8] = include_bytes!("../fonts/JetBrainsMono-Bold.ttf");
        let path = std::env::temp_dir().join("ocherdr-jetbrains-mono-bold.ttf");
        if std::fs::write(&path, bytes).is_err() {
            return;
        }
        let Some(url) = CFURL::from_path(&path, false) else {
            return;
        };
        unsafe {
            CTFontManagerRegisterFontsForURL(
                url.as_concrete_TypeRef().cast(),
                CT_FONT_MANAGER_SCOPE_PROCESS,
                std::ptr::null_mut(),
            );
        }
    });
}

fn jetbrains_mono_family_available() -> bool {
    unsafe {
        let name = CFString::new(JETBRAINS_MONO);
        let font = CTFontCreateWithName(name.as_concrete_TypeRef().cast(), 12.0, std::ptr::null());
        if font.is_null() {
            return false;
        }
        let family_ref = CTFontCopyFamilyName(font);
        CFRelease(font);
        if family_ref.is_null() {
            return false;
        }
        let family = CFString::wrap_under_create_rule(family_ref.cast());
        family == JETBRAINS_MONO
    }
}

fn cjk_families_for_languages(tags: &[&str]) -> Vec<&'static str> {
    let mut families = Vec::new();
    for tag in tags {
        let Some(family) = cjk_family_for_language(tag) else {
            continue;
        };
        if !families.contains(&family) {
            families.push(family);
        }
    }
    for family in CJK_FALLBACK_FAMILIES {
        if !families.contains(family) {
            families.push(*family);
        }
    }
    families
}

fn cjk_family_for_language(tag: &str) -> Option<&'static str> {
    let tag = tag.to_ascii_lowercase();
    if tag == "ja" || tag.starts_with("ja-") {
        return Some("Hiragino Sans");
    }
    if tag == "ko" || tag.starts_with("ko-") {
        return Some("Apple SD Gothic Neo");
    }
    if tag.contains("hant")
        || tag.starts_with("zh-tw")
        || tag.starts_with("zh-hk")
        || tag.starts_with("zh-mo")
    {
        return Some("PingFang TC");
    }
    if tag == "zh" || tag.starts_with("zh-") {
        return Some("PingFang SC");
    }
    None
}

fn preferred_language_tags() -> Vec<String> {
    unsafe {
        let array_ref = CFLocaleCopyPreferredLanguages();
        if array_ref.is_null() {
            return Vec::new();
        }
        let array = CFArray::<CFString>::wrap_under_create_rule(array_ref.cast());
        array
            .iter()
            .map(|tag| tag.to_string())
            .filter(|tag| !tag.is_empty())
            .collect()
    }
}

unsafe extern "C" {
    fn CFLocaleCopyPreferredLanguages() -> *const c_void;
    fn CTFontManagerRegisterFontsForURL(
        font_url: *const c_void,
        scope: u32,
        error: *mut *mut c_void,
    ) -> u8;
    fn CTFontCreateWithName(name: *const c_void, size: f64, matrix: *const c_void) -> *mut c_void;
    fn CTFontCopyFamilyName(font: *mut c_void) -> *mut c_void;
}

fn ghostty_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn hex_color(color: u32) -> String {
    format!("#{color:06X}")
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
    frames: UnboundedSender<PendingFrame>,
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
            // Do not load the user's Ghostty config. That file is almost always
            // a dark terminal theme and would pin the embedded surface away
            // from OcHerdr's light/dark appearance.
            ffi::ghostty_config_finalize(config);

            // Ghostty's embedded runtime treats clipboard callbacks as required
            // function pointers. Leaving them None jumps to address 0 on
            // copy-on-select (mouse-up after a drag selection).
            let runtime = ffi::ghostty_runtime_config_s {
                userdata: std::ptr::null_mut(),
                supports_selection_clipboard: false,
                wakeup_cb: Some(runtime_wakeup),
                action_cb: Some(runtime_action),
                read_clipboard_cb: Some(runtime_read_clipboard),
                confirm_read_clipboard_cb: Some(runtime_confirm_read_clipboard),
                write_clipboard_cb: Some(runtime_write_clipboard),
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

    fn apply_palette(&self, palette: &TerminalPalette) -> Result<(), TerminalError> {
        with_palette_config(palette, |config| unsafe {
            ffi::ghostty_app_update_config(self.app.as_ptr(), config);
        })
    }
}

fn with_palette_config<T>(
    palette: &TerminalPalette,
    apply: impl FnOnce(ffi::ghostty_config_t) -> T,
) -> Result<T, TerminalError> {
    let path = std::env::temp_dir().join(format!("ocherdr-ghostty-{}.conf", std::process::id()));
    std::fs::write(&path, palette.config_text())
        .map_err(|error| TerminalError::Initialization(error.to_string()))?;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|error| TerminalError::Initialization(error.to_string()))?;
    unsafe {
        let config = ffi::ghostty_config_new();
        if config.is_null() {
            return Err(TerminalError::Initialization(
                "ghostty_config_new returned null".into(),
            ));
        }
        ffi::ghostty_config_load_file(config, c_path.as_ptr());
        ffi::ghostty_config_finalize(config);
        let value = apply(config);
        ffi::ghostty_config_free(config);
        Ok(value)
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

unsafe extern "C" fn runtime_read_clipboard(
    _userdata: *mut c_void,
    _clipboard: ffi::ghostty_clipboard_e,
    _state: *mut c_void,
) -> bool {
    false
}

unsafe extern "C" fn runtime_confirm_read_clipboard(
    _userdata: *mut c_void,
    _text: *const c_char,
    _state: *mut c_void,
    _request: ffi::ghostty_clipboard_request_e,
) {
}

unsafe extern "C" fn runtime_write_clipboard(
    _userdata: *mut c_void,
    _clipboard: ffi::ghostty_clipboard_e,
    contents: *const ffi::ghostty_clipboard_content_s,
    count: usize,
    confirm: bool,
) {
    if confirm {
        return;
    }
    if let Some(text) = clipboard_text_from_contents(contents, count) {
        write_macos_clipboard(&text);
    }
}

fn clipboard_text_from_contents(
    contents: *const ffi::ghostty_clipboard_content_s,
    count: usize,
) -> Option<String> {
    if contents.is_null() || count == 0 {
        return None;
    }
    // SAFETY: Ghostty keeps this array alive for the write callback.
    clipboard_text_from_items(unsafe { std::slice::from_raw_parts(contents, count) })
}

fn clipboard_text_from_items(items: &[ffi::ghostty_clipboard_content_s]) -> Option<String> {
    let preferred = items.iter().find(|item| mime_is_plain_text(item.mime));
    let item = preferred.or(items.first())?;
    c_ptr_to_string(item.data).filter(|text| !text.is_empty())
}

fn mime_is_plain_text(ptr: *const c_char) -> bool {
    let Some(bytes) = c_ptr_to_bytes(ptr) else {
        return false;
    };
    bytes == b"text/plain" || bytes.starts_with(b"text/plain;")
}

fn c_ptr_to_bytes(ptr: *const c_char) -> Option<&'static [u8]> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: Ghostty clipboard payloads are NUL-terminated for the callback.
    Some(unsafe { CStr::from_ptr(ptr) }.to_bytes())
}

fn c_ptr_to_string(ptr: *const c_char) -> Option<String> {
    Some(String::from_utf8_lossy(c_ptr_to_bytes(ptr)?).into_owned())
}

unsafe extern "C" {
    fn PasteboardCreate(name: *const c_void, pasteboard: *mut *mut c_void) -> i32;
    fn PasteboardClear(pasteboard: *mut c_void) -> i32;
    fn PasteboardPutItemFlavor(
        pasteboard: *mut c_void,
        item_id: *mut c_void,
        flavor_type: *const c_void,
        data: *const c_void,
        flags: u32,
    ) -> i32;
}

fn write_macos_clipboard(text: &str) {
    if text.is_empty() {
        return;
    }
    let name = CFString::from_static_string("com.apple.pasteboard.clipboard");
    let flavor = CFString::from_static_string("public.utf8-plain-text");
    let data = CFData::from_buffer(text.as_bytes());
    unsafe {
        let mut pasteboard = std::ptr::null_mut();
        if PasteboardCreate(name.as_CFTypeRef(), &mut pasteboard) != 0 || pasteboard.is_null() {
            return;
        }
        let _ = PasteboardClear(pasteboard);
        let _ = PasteboardPutItemFlavor(
            pasteboard,
            std::ptr::without_provenance_mut(1),
            flavor.as_CFTypeRef(),
            data.as_CFTypeRef(),
            0,
        );
        CFRelease(pasteboard.cast());
    }
}

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
    match callback.frames.unbounded_send(pending) {
        Ok(()) => {
            ffi::ghostty_metal_external_frame_disposition_e_GHOSTTY_METAL_EXTERNAL_FRAME_ACQUIRE
        }
        Err(error) => {
            error
                .into_inner()
                .lifetime
                .token
                .store(0, Ordering::Release);
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
    frames: UnboundedReceiver<PendingFrame>,
    input: Receiver<Vec<u8>>,
}

impl Terminal {
    pub fn new(
        cols: u16,
        rows: u16,
        scrollback: usize,
        palette: &TerminalPalette,
    ) -> Result<Self, TerminalError> {
        let runtime = GhosttyRuntime::shared()?;
        runtime.apply_palette(palette)?;
        runtime.set_color_scheme(palette.dark);
        let (frame_sender, frames) = futures::channel::mpsc::unbounded();
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
        terminal.apply_palette(palette)?;
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

    pub fn surface_size(&self) -> SurfaceSize {
        // SAFETY: the surface is live and queried on GPUI's application thread.
        unsafe { ffi::ghostty_surface_size(self.raw()).into() }
    }

    pub fn try_frame(&mut self) -> Result<Option<RenderedFrame>, TerminalError> {
        let newest = match self.frames.try_recv() {
            Ok(frame) => frame,
            Err(_) => return Ok(None),
        };
        self.rendered_newest(newest).map(Some)
    }

    pub fn poll_frame(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<RenderedFrame, TerminalError>>> {
        match Pin::new(&mut self.frames).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(first)) => Poll::Ready(Some(self.rendered_newest(first))),
        }
    }

    fn rendered_newest(
        &mut self,
        mut newest: PendingFrame,
    ) -> Result<RenderedFrame, TerminalError> {
        while let Ok(frame) = self.frames.try_recv() {
            newest = frame;
        }
        // SAFETY: an acquired frame keeps the borrowed IOSurface alive until the
        // lifetime guard is dropped. Both wrappers retain their underlying object.
        let iosurface = unsafe { IOSurface::wrap_under_get_rule(newest.iosurface as IOSurfaceRef) };
        let pixel_buffer = CVPixelBuffer::from_io_surface(&iosurface, None)
            .map_err(TerminalError::FrameConversion)?;
        Ok(RenderedFrame {
            pixel_buffer,
            width_px: newest.width_px,
            height_px: newest.height_px,
            host_context: newest.host_context,
            lifetime: newest.lifetime,
        })
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
            if !modifiers.control
                && !modifiers.alt
                && let Some(text) = text.filter(|text| !text.is_empty())
            {
                self.send_committed_text(text);
                return true;
            }
            return false;
        }
        // Ctrl/Alt chords must be encoded from the key+mods, not from the
        // printable `key_char` ("c" + Ctrl would type `c` instead of `^C`).
        let text = if modifiers.control || modifiers.alt {
            None
        } else {
            text.and_then(|text| CString::new(text).ok())
        };
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

    pub fn set_preedit(&self, text: Option<&str>) {
        let (ptr, len) = text
            .filter(|text| !text.is_empty())
            .map(|text| (text.as_ptr().cast::<c_char>(), text.len()))
            .unwrap_or((std::ptr::null(), 0));
        // SAFETY: Ghostty borrows the UTF-8 bytes only for this call; empty
        // length clears the composition underline.
        unsafe { ffi::ghostty_surface_preedit(self.raw(), ptr, len) };
        self.refresh();
    }

    pub fn ime_point(&self) -> (f64, f64, f64, f64) {
        let mut x = 0.0;
        let mut y = 0.0;
        let mut width = 0.0;
        let mut height = 0.0;
        // SAFETY: the surface is live; Ghostty writes the four out-params.
        unsafe {
            ffi::ghostty_surface_ime_point(self.raw(), &mut x, &mut y, &mut width, &mut height);
        }
        (x, y, width, height)
    }

    pub fn mouse_pos(&self, x: f64, y: f64, modifiers: KeyModifiers) {
        // SAFETY: the surface is live and called on GPUI's application thread.
        unsafe {
            ffi::ghostty_surface_mouse_pos(self.raw(), x, y, ghostty_modifiers(modifiers));
        }
    }

    pub fn mouse_button(
        &self,
        pressed: bool,
        button: SurfaceMouseButton,
        modifiers: KeyModifiers,
    ) -> bool {
        let state = if pressed {
            ffi::ghostty_input_mouse_state_e_GHOSTTY_MOUSE_PRESS
        } else {
            ffi::ghostty_input_mouse_state_e_GHOSTTY_MOUSE_RELEASE
        };
        let button = match button {
            SurfaceMouseButton::Left => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_LEFT,
            SurfaceMouseButton::Right => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_RIGHT,
            SurfaceMouseButton::Middle => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_MIDDLE,
        };
        // SAFETY: the surface is live and called on GPUI's application thread.
        unsafe {
            ffi::ghostty_surface_mouse_button(
                self.raw(),
                state,
                button,
                ghostty_modifiers(modifiers),
            )
        }
    }

    pub fn set_mouse_capture(&self, enabled: bool, sgr_pixels: bool) {
        self.apply_frame(&mouse_capture_sequence(enabled, sgr_pixels), false);
    }

    pub fn has_selection(&self) -> bool {
        // SAFETY: the surface is live and queried on GPUI's application thread.
        unsafe { ffi::ghostty_surface_has_selection(self.raw()) }
    }

    pub fn read_selection(&self) -> Option<String> {
        let mut result = empty_ghostty_text();
        // SAFETY: the surface is live; Ghostty fills `result` until free_text.
        let ok = unsafe { ffi::ghostty_surface_read_selection(self.raw(), &mut result) };
        if !ok {
            return None;
        }
        Some(take_ghostty_text(self.raw(), &mut result))
    }

    pub fn select_all_visible(&self) -> bool {
        let size = self.surface_size();
        if size.rows == 0 {
            return false;
        }
        // SAFETY: the surface is live and called on GPUI's application thread.
        let ok = unsafe {
            ffi::ghostty_surface_select_viewport_rows(self.raw(), 0, size.rows.saturating_sub(1))
        };
        if ok {
            self.refresh();
        }
        ok
    }

    pub fn begin_text_selection(&self, x: f64, y: f64, modifiers: KeyModifiers) -> bool {
        self.mouse_pos(x, y, modifiers);
        let captured = self.mouse_button(true, SurfaceMouseButton::Left, modifiers);
        self.refresh();
        captured
    }

    pub fn update_text_selection(&self, x: f64, y: f64, modifiers: KeyModifiers) {
        self.mouse_pos(x, y, modifiers);
        self.refresh();
    }

    pub fn end_text_selection(&self, point: Option<(f64, f64)>, modifiers: KeyModifiers) {
        if let Some((x, y)) = point {
            self.update_text_selection(x, y, modifiers);
        }
        let _ = self.mouse_button(false, SurfaceMouseButton::Left, modifiers);
        self.refresh();
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

    pub fn apply_palette(&self, palette: &TerminalPalette) -> Result<(), TerminalError> {
        let runtime = GhosttyRuntime::shared()?;
        runtime.apply_palette(palette)?;
        runtime.set_color_scheme(palette.dark);
        with_palette_config(palette, |config| unsafe {
            ffi::ghostty_surface_update_config(self.raw(), config);
        })?;
        self.set_color_scheme(palette.dark);
        Ok(())
    }

    pub fn refresh(&self) {
        // SAFETY: the surface is live and called on GPUI's application thread.
        unsafe { ffi::ghostty_surface_refresh(self.raw()) };
    }

    /// Visible viewport text for assistive technology. `None` if Ghostty cannot
    /// produce a selection; an empty string means the screen is blank.
    pub fn read_visible_text(&self) -> Option<String> {
        let mut result = empty_ghostty_text();
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
        Some(take_ghostty_text(self.raw(), &mut result))
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

fn empty_ghostty_text() -> ffi::ghostty_text_s {
    ffi::ghostty_text_s {
        tl_px_x: 0.0,
        tl_px_y: 0.0,
        offset_start: 0,
        offset_len: 0,
        text: std::ptr::null(),
        text_len: 0,
    }
}

fn take_ghostty_text(raw: ffi::ghostty_surface_t, result: &mut ffi::ghostty_text_s) -> String {
    let text = if result.text.is_null() || result.text_len == 0 {
        String::new()
    } else {
        // SAFETY: `text_len` is the length Ghostty reported for this buffer.
        let bytes =
            unsafe { std::slice::from_raw_parts(result.text.cast::<u8>(), result.text_len) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    if !result.text.is_null() {
        unsafe { ffi::ghostty_surface_free_text(raw, result) };
    }
    text
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

    #[test]
    fn mouse_capture_sequences_reset_modes_before_enabling_requested_encoding() {
        let disabled = mouse_capture_sequence(false, true);
        assert_eq!(disabled, MOUSE_REPORTING_RESET);

        let enabled = mouse_capture_sequence(true, false);
        assert!(enabled.starts_with(MOUSE_REPORTING_RESET));
        assert!(enabled.ends_with(b"\x1b[?1006h"));
        assert!(!enabled.ends_with(b"\x1b[?1016h"));

        let pixels = mouse_capture_sequence(true, true);
        assert!(pixels.starts_with(MOUSE_REPORTING_RESET));
        assert!(pixels.ends_with(b"\x1b[?1016h"));
    }

    #[test]
    fn palette_config_uses_light_background_and_ansi_slots() {
        let palette = TerminalPalette {
            dark: false,
            background: 0xEFF1F5,
            foreground: 0x4C4F69,
            cursor: 0x8839EF,
            selection: 0xCCD0DA,
            ansi: [
                0x5C5F77, 0xD20F39, 0x40A02B, 0xDF8E1D, 0x1E66F5, 0xEA76CB, 0x179299, 0xACB0BE,
                0x6C6F85, 0xD20F39, 0x40A02B, 0xDF8E1D, 0x1E66F5, 0xEA76CB, 0x179299, 0x4C4F69,
            ],
            font_family: "SF Mono".into(),
            font_size: 14,
            font_features: vec!["-calt".into(), "-liga".into(), "-dlig".into()],
            thicken: true,
            thicken_strength: 80,
            cell_width: Some("-8%".into()),
            cell_height: Some("12%".into()),
            padding_x: 2,
            padding_y: 4,
        };
        let config = palette.config_text();
        assert!(config.contains("background = #EFF1F5"));
        assert!(config.contains("foreground = #4C4F69"));
        assert!(config.contains("palette = 0=#5C5F77"));
        assert!(config.contains("palette = 15=#4C4F69"));
        assert!(config.contains("font-family = \"SF Mono\""));
        assert!(config.contains("font-family-bold = \"SF Mono\""));
        assert!(config.contains("font-family-bold = \"PingFang SC\""));
        assert!(config.contains("font-size = 14"));
        assert!(config.contains("font-feature = -calt"));
        assert!(config.contains("font-thicken = true"));
        assert!(config.contains("font-thicken-strength = 80"));
        assert!(config.contains("adjust-cell-width = -8%"));
        assert!(config.contains("adjust-cell-height = 12%"));
        assert!(config.contains("window-padding-x = 2"));
        assert!(config.contains("window-padding-y = 4"));
        let builtin = TerminalPalette {
            font_family: String::new(),
            font_features: Vec::new(),
            thicken: false,
            thicken_strength: 255,
            cell_width: None,
            cell_height: None,
            padding_x: 0,
            padding_y: 0,
            ..palette.clone()
        };
        let builtin_config = builtin.config_text();
        assert!(
            !builtin_config.contains("font-family = "),
            "{builtin_config}"
        );
        assert!(builtin_config.contains("font-family-bold = \"JetBrains Mono\""));
        assert!(builtin_config.contains("font-family-bold = \"PingFang SC\""));
        assert!(!builtin_config.contains("Menlo"));
        let jbm = builtin_config
            .find("font-family-bold = \"JetBrains Mono\"")
            .unwrap();
        let cjk = builtin_config
            .find("font-family-bold = \"PingFang SC\"")
            .unwrap();
        assert!(jbm < cjk);
        assert!(!builtin_config.contains("font-thicken"));
        assert!(!builtin_config.contains("adjust-cell-width"));
        assert_ne!(palette.signature(), builtin.signature());
    }

    #[test]
    fn default_ghostty_config_anchors_latin_bold_before_cjk() {
        let mut out = String::new();
        write_font_families_with(&mut out, "", &["PingFang SC", "Hiragino Sans"]);
        assert!(
            !out.contains("font-family = "),
            "regular list stays empty so embedded JetBrains Mono remains Latin regular\n{out}"
        );
        let jbm = out.find("font-family-bold = \"JetBrains Mono\"").unwrap();
        let cjk = out.find("font-family-bold = \"PingFang SC\"").unwrap();
        assert!(jbm < cjk);
        assert!(!out.contains("Menlo"));
    }

    #[test]
    fn cjk_fallback_order_follows_preferred_language_tags() {
        assert_eq!(
            cjk_families_for_languages(&["zh-Hant-TW"])[0],
            "PingFang TC"
        );
        assert_eq!(cjk_families_for_languages(&["ja-JP"])[0], "Hiragino Sans");
        assert_eq!(
            cjk_families_for_languages(&["ko-KR"])[0],
            "Apple SD Gothic Neo"
        );
        assert_eq!(
            cjk_families_for_languages(&["zh-Hans-CN"])[0],
            "PingFang SC"
        );
        assert_eq!(cjk_families_for_languages(&["en-US"])[0], "PingFang SC");
    }

    #[test]
    fn ghostty_config_puts_the_primary_bold_face_ahead_of_cjk_fallbacks() {
        let mut config = String::new();
        write_font_families_with(
            &mut config,
            "Menlo",
            &cjk_families_for_languages(&["ja-JP"]),
        );
        let primary_bold = config.find("font-family-bold = \"Menlo\"").unwrap();
        let cjk_bold = config.find("font-family-bold = \"Hiragino Sans\"").unwrap();
        assert!(primary_bold < cjk_bold);
        let primary = config.find("font-family = \"Menlo\"").unwrap();
        let cjk = config.find("font-family = \"Hiragino Sans\"").unwrap();
        assert!(primary < cjk);
    }

    #[test]
    fn clipboard_write_prefers_plain_text_payload() {
        let mime_html = CString::new("text/html").unwrap();
        let html = CString::new("<b>x</b>").unwrap();
        let mime_plain = CString::new("text/plain").unwrap();
        let plain = CString::new("hello\nworld").unwrap();
        let contents = [
            ffi::ghostty_clipboard_content_s {
                mime: mime_html.as_ptr(),
                data: html.as_ptr(),
            },
            ffi::ghostty_clipboard_content_s {
                mime: mime_plain.as_ptr(),
                data: plain.as_ptr(),
            },
        ];
        assert_eq!(
            clipboard_text_from_items(&contents).as_deref(),
            Some("hello\nworld")
        );
    }

    #[test]
    fn clipboard_write_skips_empty_payloads() {
        let mime = CString::new("text/plain").unwrap();
        let data = CString::new("").unwrap();
        let contents = [ffi::ghostty_clipboard_content_s {
            mime: mime.as_ptr(),
            data: data.as_ptr(),
        }];
        assert_eq!(clipboard_text_from_items(&contents), None);
        assert_eq!(clipboard_text_from_contents(std::ptr::null(), 2), None);
    }
}
