use super::*;
use std::fs;
use std::io::Read as _;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
enum CommandPaste {
    PassThrough,
    Text(String),
    RemoteImage(RemoteClipboardImage),
    RemoteImageError(RemoteImagePasteError),
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteClipboardImage {
    Bytes { extension: String, bytes: Vec<u8> },
    File { extension: String, path: PathBuf },
}

#[derive(Debug, PartialEq, Eq)]
enum RemoteImagePasteError {
    UnsupportedFormat(String),
    TooLarge { bytes: usize, max: usize },
    InvalidImage,
    FileRead { path: String, error: String },
}

impl RemoteImagePasteError {
    fn detail(&self, i18n: I18n) -> String {
        match self {
            Self::UnsupportedFormat(format) => i18n.clipboard_image_format_detail(format),
            Self::TooLarge { bytes, max } => i18n.clipboard_image_too_large_detail(*bytes, *max),
            Self::InvalidImage => i18n
                .text(k::NOTIFY_DETAIL_CLIPBOARD_IMAGE_INVALID)
                .to_owned(),
            Self::FileRead { path, error } => i18n.clipboard_image_read_detail(path, error),
        }
    }
}

fn command_paste(item: Option<ClipboardItem>, remote: bool) -> CommandPaste {
    let Some(item) = item else {
        return CommandPaste::PassThrough;
    };
    let explicit_text = explicit_clipboard_text(&item);
    let fallback_text = item.text();
    let entries = item.into_entries().collect::<Vec<_>>();
    let file_image = clipboard_image_file(&entries);
    if let Some(text) = explicit_text
        && !file_image
            .as_ref()
            .is_some_and(|(path, _)| text == path.display().to_string())
    {
        return CommandPaste::Text(text);
    }
    if !remote {
        if file_image.is_some()
            || entries
                .iter()
                .any(|entry| matches!(entry, ClipboardEntry::Image(_)))
        {
            return CommandPaste::PassThrough;
        }
        return fallback_text
            .map(CommandPaste::Text)
            .unwrap_or(CommandPaste::PassThrough);
    }
    if let Some(image) = entries.into_iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image),
        _ => None,
    }) {
        let extension = image.format.extension();
        return match validate_remote_image_bytes(extension, &image.bytes) {
            Ok(extension) => CommandPaste::RemoteImage(RemoteClipboardImage::Bytes {
                extension: extension.to_owned(),
                bytes: image.bytes,
            }),
            Err(error) => CommandPaste::RemoteImageError(error),
        };
    }
    if let Some((path, extension)) = file_image {
        return CommandPaste::RemoteImage(RemoteClipboardImage::File { extension, path });
    }
    fallback_text
        .map(CommandPaste::Text)
        .unwrap_or(CommandPaste::PassThrough)
}

fn explicit_clipboard_text(item: &ClipboardItem) -> Option<String> {
    let mut text = String::new();
    for entry in item.entries() {
        if let ClipboardEntry::String(value) = entry {
            text.push_str(value.text());
        }
    }
    (!text.is_empty()).then_some(text)
}

fn clipboard_image_file(entries: &[ClipboardEntry]) -> Option<(PathBuf, String)> {
    let mut path = None;
    for entry in entries {
        let ClipboardEntry::ExternalPaths(paths) = entry else {
            continue;
        };
        for candidate in paths.paths() {
            if path.replace(candidate).is_some() {
                return None;
            }
        }
    }
    let path = path?;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(normalize_image_extension)?;
    Some((path.clone(), extension.to_owned()))
}

fn normalize_image_extension(extension: &str) -> Option<&'static str> {
    if extension.eq_ignore_ascii_case("png") {
        Some("png")
    } else if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        Some("jpg")
    } else if extension.eq_ignore_ascii_case("gif") {
        Some("gif")
    } else if extension.eq_ignore_ascii_case("webp") {
        Some("webp")
    } else if extension.eq_ignore_ascii_case("bmp") {
        Some("bmp")
    } else {
        None
    }
}

fn validate_remote_image_bytes(
    extension: &str,
    bytes: &[u8],
) -> Result<&'static str, RemoteImagePasteError> {
    let Some(extension) = normalize_image_extension(extension) else {
        return Err(RemoteImagePasteError::UnsupportedFormat(
            extension.to_owned(),
        ));
    };
    if bytes.is_empty() || !image_bytes_match_signature(extension, bytes) {
        return Err(RemoteImagePasteError::InvalidImage);
    }
    let max = MAX_REMOTE_CLIPBOARD_IMAGE_BYTES;
    if bytes.len() > max {
        return Err(RemoteImagePasteError::TooLarge {
            bytes: bytes.len(),
            max,
        });
    }
    Ok(extension)
}

fn read_remote_clipboard_image_file(
    path: PathBuf,
    extension: String,
) -> Result<(String, Vec<u8>), RemoteImagePasteError> {
    let metadata =
        fs::metadata(&path).map_err(|error| remote_image_file_read_error(&path, error))?;
    if !metadata.is_file() {
        return Err(remote_image_file_read_error(
            &path,
            "the clipboard path is not a regular file",
        ));
    }
    let max = MAX_REMOTE_CLIPBOARD_IMAGE_BYTES;
    if metadata.len() > max as u64 {
        return Err(RemoteImagePasteError::TooLarge {
            bytes: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            max,
        });
    }
    let file = fs::File::open(&path).map_err(|error| remote_image_file_read_error(&path, error))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| remote_image_file_read_error(&path, error))?;
    let extension = validate_remote_image_bytes(&extension, &bytes)?.to_owned();
    Ok((extension, bytes))
}

fn remote_image_file_read_error(
    path: &Path,
    error: impl std::fmt::Display,
) -> RemoteImagePasteError {
    RemoteImagePasteError::FileRead {
        path: path.display().to_string(),
        error: error.to_string(),
    }
}

#[derive(Debug)]
enum RemoteImageUploadError {
    Clipboard(RemoteImagePasteError),
    Upload(HerdrError),
}

impl RemoteClipboardImage {
    fn into_bytes(self) -> Result<(String, Vec<u8>), RemoteImagePasteError> {
        match self {
            Self::Bytes { extension, bytes } => Ok((extension, bytes)),
            Self::File { extension, path } => read_remote_clipboard_image_file(path, extension),
        }
    }
}

fn image_bytes_match_signature(extension: &str, bytes: &[u8]) -> bool {
    match extension {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes[8..12] == *b"WEBP",
        "bmp" => {
            if bytes.len() < 26 || !bytes.starts_with(b"BM") {
                return false;
            }
            let offset = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
            (26..=bytes.len()).contains(&offset)
        }
        _ => false,
    }
}

impl OcHerdrView {
    pub(crate) fn create_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(workspace_id) = self.selection.workspace_id.clone() {
            self.invoke_with_response(
                "tab.create",
                json!({ "workspace_id": workspace_id, "focus": true, "env": {} }),
                Self::follow_created_tab,
                cx,
            );
        }
    }

    pub(crate) fn cycle_tab(&mut self, offset: isize, cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(workspace_id) = self.selection.workspace_id.as_deref() else {
            return;
        };
        let hidden = self.hidden_tab_ids();
        let tab_ids = snapshot
            .tabs_for(workspace_id)
            .filter(|tab| !hidden.contains(&tab.tab_id))
            .map(|tab| tab.tab_id.clone())
            .collect::<Vec<_>>();
        if tab_ids.is_empty() {
            return;
        }
        let current = self
            .selection
            .tab_id
            .as_ref()
            .and_then(|tab_id| tab_ids.iter().position(|candidate| candidate == tab_id))
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(tab_ids.len() as isize) as usize;
        self.select_tab(tab_ids[next].clone(), cx);
    }

    pub(crate) fn select_tab_number(&mut self, number: usize, cx: &mut Context<Self>) {
        let tab_id = self.snapshot.as_ref().and_then(|snapshot| {
            self.selection
                .workspace_id
                .as_deref()
                .and_then(|workspace_id| {
                    let hidden = self.hidden_tab_ids();
                    tab_id_for_shortcut(
                        snapshot
                            .tabs_for(workspace_id)
                            .filter(|tab| !hidden.contains(&tab.tab_id)),
                        number,
                    )
                })
        });
        if let Some(tab_id) = tab_id {
            self.select_tab(tab_id, cx);
        }
    }

    pub(crate) fn focus_pane_direction(&mut self, direction: &'static str, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.selection.pane_id.clone() {
            self.invoke(
                "pane.focus_direction",
                json!({ "pane_id": pane_id, "direction": direction }),
                cx,
            );
        }
    }

    pub(crate) fn handle_prefix_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prefix_pending = false;
        let key = event.keystroke.key.as_str();
        let shift = event.keystroke.modifiers.shift;
        match (key, shift) {
            ("escape", _) => {}
            ("s", false) => self.open_native_tui(cx),
            ("c", false) => self.create_tab(cx),
            ("n", true) => self.create_workspace(cx),
            ("n", false) => self.cycle_tab(1, cx),
            ("p", false) => self.cycle_tab(-1, cx),
            ("w", true) => {
                if let Some(target) = self.selected_workspace_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("d", true) => {
                if let Some(target) = self.selected_workspace_target() {
                    self.request_close(target, cx);
                }
            }
            ("t", true) => {
                if let Some(target) = self.selected_tab_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("x", true) => {
                if let Some(target) = self.selected_tab_target() {
                    self.request_close(target, cx);
                }
            }
            ("p", true) => {
                if let Some(target) = self.selected_pane_target() {
                    self.open_rename(target, window, cx);
                }
            }
            ("m", false) => self.enter_keyboard_pane_move(cx),
            ("h", false) => self.focus_pane_direction("left", cx),
            ("j", false) => self.focus_pane_direction("down", cx),
            ("k", false) => self.focus_pane_direction("up", cx),
            ("l", false) => self.focus_pane_direction("right", cx),
            ("j" | "down", true) => self.move_selected_workspace(1, cx),
            ("k" | "up", true) => self.move_selected_workspace(-1, cx),
            _ => {
                if let Some(number) =
                    tab_index_from_keystroke(key, event.keystroke.key_char.as_deref())
                {
                    self.select_tab_number(number, cx);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn handle_app_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.control && !modifiers.platform && !modifiers.alt && key == "b" {
            self.prefix_pending = true;
            if matches!(self.overlay, Overlay::ContextMenu(_)) {
                self.set_overlay(Overlay::None, cx);
            }
            cx.notify();
            return true;
        }
        if self.prefix_pending {
            self.handle_prefix_key(event, window, cx);
            return true;
        }
        if self.pane_keyboard_move.is_some() && self.handle_keyboard_pane_move_key(event, cx) {
            return true;
        }
        if key == "escape" {
            if matches!(self.surface_drag, SurfaceDrag::Pane(_)) {
                self.cancel_pane_drag();
                cx.notify();
                return true;
            }
            if matches!(self.overlay, Overlay::Appearance) {
                self.close_appearance(window, cx);
                return true;
            }
            if matches!(self.overlay, Overlay::AgentPanel { .. }) {
                self.close_agent_panel(window, cx);
                return true;
            }
            if matches!(
                self.overlay,
                Overlay::ContextMenu(_) | Overlay::NodeManager | Overlay::HostSwitcher
            ) {
                self.set_overlay(Overlay::None, cx);
                self.focus.focus(window, cx);
                return true;
            }
            if matches!(
                self.overlay,
                Overlay::ConfirmClose(_)
                    | Overlay::ConfirmRemoveWorktree { .. }
                    | Overlay::WorktreeCreate { .. }
                    | Overlay::WorktreeOpen(_)
            ) {
                self.set_overlay(Overlay::None, cx);
                self.focus.focus(window, cx);
                return true;
            }
        }
        if modifiers.platform && !modifiers.alt && !modifiers.control {
            if let Some(number) = tab_index_from_keystroke(key, event.keystroke.key_char.as_deref())
            {
                self.select_tab_number(number, cx);
                return true;
            }
            let handled = match (key, modifiers.shift) {
                ("t", false) => {
                    self.create_tab(cx);
                    true
                }
                ("w", true) => {
                    if let Some(target) = self.selected_workspace_target() {
                        self.request_close(target, cx);
                    }
                    true
                }
                ("w", false) => {
                    if let Some(target) = self.cmd_w_close_target() {
                        self.request_close(target, cx);
                    }
                    true
                }
                ("n", true) => {
                    self.create_workspace(cx);
                    true
                }
                (",", false) => {
                    self.open_native_tui(cx);
                    true
                }
                ("c", false) => {
                    self.copy_selection(cx);
                    true
                }
                ("a", false) => {
                    self.select_all_visible(cx);
                    true
                }
                ("[", false) => {
                    self.cycle_tab(-1, cx);
                    true
                }
                ("]", false) => {
                    self.cycle_tab(1, cx);
                    true
                }
                _ => false,
            };
            if handled {
                return true;
            }
        }
        if modifiers.control && key == "tab" {
            self.cycle_tab(if modifiers.shift { -1 } else { 1 }, cx);
            return true;
        }
        if key == "f2" && !modifiers.platform && !modifiers.control && !modifiers.alt {
            let target = self
                .selected_tab_target()
                .or_else(|| self.selected_workspace_target());
            if let Some(target) = target {
                self.open_rename(target, window, cx);
            }
            return true;
        }
        false
    }

    pub(crate) fn send_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.handle_app_shortcut(event, window, cx) {
            // The matching key-up must not reach the terminal either.
            self.suppress_key_release = true;
            cx.stop_propagation();
            return;
        }
        if self.ime_marked.is_some() {
            return;
        }
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        self.take_terminal_control(pane_id.clone(), cx);
        let key = &event.keystroke;
        let paste = (key.modifiers.platform && key.key == "v").then(|| {
            command_paste(
                cx.read_from_clipboard(),
                matches!(self.current_profile(), ConnectionProfile::Ssh { .. }),
            )
        });
        let mut paste_error = None;
        let mut remote_image = None;
        let mut suppress_key_release = false;
        let stream_closed = {
            let Some(runtime) = self.pane_mut(&pane_id) else {
                return;
            };
            if !runtime.mode.is_controlled() {
                return;
            }
            match paste.unwrap_or(CommandPaste::PassThrough) {
                CommandPaste::Text(text) => {
                    runtime.terminal.paste(&text);
                    suppress_key_release = true;
                    cx.stop_propagation();
                    drain_terminal_input(runtime)
                }
                CommandPaste::RemoteImage(image) => {
                    suppress_key_release = true;
                    cx.stop_propagation();
                    remote_image = Some(image);
                    false
                }
                CommandPaste::RemoteImageError(error) => {
                    suppress_key_release = true;
                    paste_error = Some(error);
                    cx.stop_propagation();
                    false
                }
                CommandPaste::PassThrough => {
                    // Match Ghostty's performable paste binding: an image-only
                    // clipboard on a local profile leaves the original Cmd+V
                    // intact, so a local agent can read the OS clipboard.
                    let action = if event.is_held {
                        KeyAction::Repeat
                    } else {
                        KeyAction::Press
                    };
                    if !runtime.terminal.send_key(
                        action,
                        &key.key,
                        key.key_char.as_deref(),
                        gpui_key_modifiers(key.modifiers),
                    ) {
                        return;
                    }
                    cx.stop_propagation();
                    drain_terminal_input(runtime)
                }
            }
        };
        if suppress_key_release {
            self.suppress_key_release = true;
        }
        if let Some(error) = paste_error {
            let detail = error.detail(self.i18n);
            self.notify_failure(FailureKind::ClipboardImagePaste, detail, cx);
        }
        if let Some(image) = remote_image {
            self.upload_and_paste_remote_clipboard_image(pane_id, image, cx);
        }
        if stream_closed {
            self.resync_snapshot(self.event_epoch, cx);
        }
    }

    fn upload_and_paste_remote_clipboard_image(
        &mut self,
        pane_id: String,
        image: RemoteClipboardImage,
        cx: &mut Context<Self>,
    ) {
        let profile = self.current_profile();
        let profile_id = profile.id().to_owned();
        let session_name = self.current_session().map(|session| session.name.clone());
        let event_epoch = self.event_epoch;
        let upload = self.remote_clipboard_image_upload;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let (extension, bytes) = image
                        .into_bytes()
                        .map_err(RemoteImageUploadError::Clipboard)?;
                    upload(profile, extension, bytes).map_err(RemoteImageUploadError::Upload)
                })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(path) => {
                    let target_is_current = this.event_epoch == event_epoch
                        && this.current_profile().id() == profile_id
                        && this.current_session().map(|session| &session.name)
                            == session_name.as_ref();
                    if !target_is_current {
                        this.notify_failure(
                            FailureKind::ClipboardImagePaste,
                            this.i18n
                                .text(k::NOTIFY_DETAIL_CLIPBOARD_IMAGE_TARGET_CHANGED),
                            cx,
                        );
                        return;
                    }
                    let Some(stream_closed) =
                        this.paste_uploaded_remote_clipboard_image(&pane_id, &path)
                    else {
                        this.notify_failure(
                            FailureKind::ClipboardImagePaste,
                            this.i18n
                                .text(k::NOTIFY_DETAIL_CLIPBOARD_IMAGE_TARGET_CHANGED),
                            cx,
                        );
                        return;
                    };
                    if stream_closed {
                        this.notify_failure(
                            FailureKind::TerminalStream,
                            HerdrError::TerminalClosed("terminal worker stopped".into()),
                            cx,
                        );
                        this.resync_snapshot(this.event_epoch, cx);
                    }
                }
                Err(RemoteImageUploadError::Clipboard(error)) => {
                    let detail = error.detail(this.i18n);
                    this.notify_failure(FailureKind::ClipboardImagePaste, detail, cx);
                }
                Err(RemoteImageUploadError::Upload(error)) => {
                    this.notify_failure(FailureKind::ClipboardImagePaste, error, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn paste_uploaded_remote_clipboard_image(&mut self, pane_id: &str, path: &str) -> Option<bool> {
        let runtime = self.pane_mut(pane_id)?;
        if !runtime.mode.is_controlled() {
            return None;
        }
        runtime.terminal.paste(path);
        Some(drain_terminal_input(runtime))
    }

    /// Key releases matter only to applications that asked the kitty
    /// keyboard protocol to report them; Ghostty decides.
    pub(crate) fn send_key_release(
        &mut self,
        event: &ochub_ui::gpui::KeyUpEvent,
        cx: &mut Context<Self>,
    ) {
        if std::mem::take(&mut self.suppress_key_release)
            || self.ime_marked.is_some()
            || self.prefix_pending
            || self.pane_keyboard_move.is_some()
        {
            return;
        }
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        let key = &event.keystroke;
        let stream_closed = {
            let Some(runtime) = self.pane_mut(&pane_id) else {
                return;
            };
            if !runtime.terminal.send_key(
                KeyAction::Release,
                &key.key,
                None,
                gpui_key_modifiers(key.modifiers),
            ) {
                return;
            }
            drain_terminal_input(runtime)
        };
        if stream_closed {
            self.resync_snapshot(self.event_epoch, cx);
        }
    }

    /// Forward whatever Ghostty has queued for every pane's pty. Tests call
    /// this in place of the frame and event polls that do it in production.
    #[cfg(test)]
    pub(crate) fn pump_terminal_input(&mut self) {
        if let Some(session) = self.session_panes.as_mut() {
            for runtime in session.panes.values_mut() {
                flush_pane_surface(runtime);
            }
        }
    }

    pub(crate) fn pane_mouse_down(
        &mut self,
        pane_id: String,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.surface_drag,
            SurfaceDrag::Split(_) | SurfaceDrag::Reorder(_) | SurfaceDrag::Pane(_)
        ) {
            return;
        }
        self.end_text_drag_unless_pane(&pane_id);
        self.select_pane(pane_id.clone(), window, cx);
        self.take_terminal_control(pane_id.clone(), cx);
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        let mouse = mouse_point(event.position);
        if !point_in_rect(mouse, runtime.body_bounds) {
            self.surface_drag = SurfaceDrag::Idle;
            return;
        }
        let Some(surface) = map_mouse_to_surface(
            mouse,
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        ) else {
            self.surface_drag = SurfaceDrag::Idle;
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        let Some(runtime) = self.pane_mut(&pane_id) else {
            return;
        };
        let captured = runtime
            .terminal
            .begin_text_selection(surface.0, surface.1, modifiers);
        flush_pane_surface(runtime);
        self.surface_drag = SurfaceDrag::Text {
            pane_id: pane_id.clone(),
            captured,
        };
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn pane_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.update_split_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.update_reorder_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.update_pane_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        let SurfaceDrag::Text { pane_id, .. } = &self.surface_drag else {
            return;
        };
        let pane_id = pane_id.clone();
        let Some(runtime) = self.pane(&pane_id) else {
            return;
        };
        let Some(surface) = map_mouse_to_surface(
            mouse_point(event.position),
            runtime.body_bounds,
            runtime.pixel_size,
            window.scale_factor(),
        ) else {
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        if let Some(runtime) = self.pane_mut(&pane_id) {
            runtime
                .terminal
                .update_text_selection(surface.0, surface.1, modifiers);
            flush_pane_surface(runtime);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn pane_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.finish_split_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.finish_reorder_drag(mouse_point(event.position), cx) {
            cx.stop_propagation();
            return;
        }
        if self.finish_pane_drag(mouse_point(event.position), window, cx) {
            cx.stop_propagation();
            return;
        }
        let SurfaceDrag::Text { pane_id, captured } =
            std::mem::replace(&mut self.surface_drag, SurfaceDrag::Idle)
        else {
            return;
        };
        let modifiers = gpui_key_modifiers(event.modifiers);
        if let Some(runtime) = self.pane_mut(&pane_id) {
            let point = map_mouse_to_surface(
                mouse_point(event.position),
                runtime.body_bounds,
                runtime.pixel_size,
                window.scale_factor(),
            );
            runtime.terminal.end_text_selection(point, modifiers);
            flush_pane_surface(runtime);
            if !captured {
                copy_terminal_selection(runtime, cx);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ochub_ui::gpui::{ClipboardString, ExternalPaths, Image, ImageFormat};

    fn png(bytes: Vec<u8>) -> ClipboardItem {
        ClipboardItem::new_image(&Image {
            format: ImageFormat::Png,
            bytes,
            id: 1,
        })
    }

    fn external_path(path: PathBuf) -> ClipboardItem {
        ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                vec![path].into(),
            ))],
        }
    }

    #[test]
    fn local_image_paste_keeps_the_original_command_v() {
        assert_eq!(
            command_paste(Some(png(b"\x89PNG\r\n\x1a\nrest".to_vec())), false),
            CommandPaste::PassThrough
        );
    }

    #[test]
    fn remote_image_paste_validates_content() {
        assert_eq!(
            command_paste(Some(png(b"not png".to_vec())), true),
            CommandPaste::RemoteImageError(RemoteImagePasteError::InvalidImage)
        );
        assert_eq!(
            command_paste(
                Some(ClipboardItem::new_image(&Image {
                    format: ImageFormat::Svg,
                    bytes: b"<svg/>".to_vec(),
                    id: 2,
                })),
                true,
            ),
            CommandPaste::RemoteImageError(RemoteImagePasteError::UnsupportedFormat("svg".into()))
        );
        let mut too_large = vec![0; MAX_REMOTE_CLIPBOARD_IMAGE_BYTES + 1];
        too_large[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            command_paste(Some(png(too_large)), true),
            CommandPaste::RemoteImageError(RemoteImagePasteError::TooLarge {
                bytes: MAX_REMOTE_CLIPBOARD_IMAGE_BYTES + 1,
                max: MAX_REMOTE_CLIPBOARD_IMAGE_BYTES,
            })
        );
    }

    #[test]
    fn remote_png_becomes_an_upload_request() {
        let bytes = b"\x89PNG\r\n\x1a\nrest".to_vec();
        assert_eq!(
            command_paste(Some(png(bytes.clone())), true),
            CommandPaste::RemoteImage(RemoteClipboardImage::Bytes {
                extension: "png".into(),
                bytes,
            })
        );
    }

    #[test]
    fn pixpin_file_backed_png_becomes_an_upload_request() {
        let path = PathBuf::from("/tmp/PixPin Screenshot.PNG");
        assert_eq!(
            command_paste(Some(external_path(path.clone())), true),
            CommandPaste::RemoteImage(RemoteClipboardImage::File {
                extension: "png".into(),
                path,
            })
        );
        assert_eq!(
            command_paste(
                Some(external_path(PathBuf::from("/tmp/PixPin Screenshot.PNG"))),
                false,
            ),
            CommandPaste::PassThrough,
            "a local agent must receive the original Cmd+V"
        );
    }

    #[test]
    fn file_backed_image_is_read_and_revalidated_before_upload() {
        let dir = tempfile::TempDir::new().unwrap();
        let image = dir.path().join("capture.jpeg");
        let bytes = b"\xff\xd8\xffremote".to_vec();
        fs::write(&image, &bytes).unwrap();
        assert_eq!(
            read_remote_clipboard_image_file(image, "jpg".into()).unwrap(),
            ("jpg".into(), bytes)
        );

        let invalid = dir.path().join("invalid.png");
        fs::write(&invalid, b"not png").unwrap();
        assert_eq!(
            read_remote_clipboard_image_file(invalid, "png".into()).unwrap_err(),
            RemoteImagePasteError::InvalidImage
        );

        let oversized = dir.path().join("oversized.png");
        fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX_REMOTE_CLIPBOARD_IMAGE_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(
            read_remote_clipboard_image_file(oversized, "png".into()).unwrap_err(),
            RemoteImagePasteError::TooLarge {
                bytes: MAX_REMOTE_CLIPBOARD_IMAGE_BYTES + 1,
                max: MAX_REMOTE_CLIPBOARD_IMAGE_BYTES,
            }
        );
    }

    #[test]
    fn clipboard_text_keeps_precedence_for_remote_profiles() {
        assert_eq!(
            command_paste(Some(ClipboardItem::new_string("hello".into())), true),
            CommandPaste::Text("hello".into())
        );

        let item = ClipboardItem {
            // macOS presents copied files in this order. A real string must
            // still win; ExternalPaths alone is what PixPin supplies.
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(
                    vec![PathBuf::from("/tmp/capture.png")].into(),
                )),
                ClipboardEntry::String(ClipboardString::new("caption".into())),
            ],
        };
        assert_eq!(
            command_paste(Some(item), true),
            CommandPaste::Text("caption".into())
        );

        let path = PathBuf::from("/tmp/capture.png");
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::ExternalPaths(ExternalPaths(vec![path.clone()].into())),
                ClipboardEntry::String(ClipboardString::new(path.display().to_string())),
            ],
        };
        assert_eq!(
            command_paste(Some(item), true),
            CommandPaste::RemoteImage(RemoteClipboardImage::File {
                extension: "png".into(),
                path,
            }),
            "a file provider's redundant path string is not user-authored text"
        );
    }
}
