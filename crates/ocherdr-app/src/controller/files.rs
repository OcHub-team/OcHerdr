use super::*;
use ocherdr_files::{BackendSpec, FileError};
use std::path::Path;

const TRANSFER_POLL_INTERVAL: Duration = Duration::from_millis(80);
const EDITOR_WATCH_INTERVAL: Duration = Duration::from_millis(350);
const EDITOR_SAVE_DEBOUNCE: Duration = Duration::from_millis(550);
const MAX_VISIBLE_TRANSFERS: usize = 40;

enum EditorWatchAction {
    Wait,
    Stop,
    Sync {
        local_path: PathBuf,
        remote_path: PathBuf,
        expected: FileVersion,
        revision: LocalFileRevision,
        transfer_id: u64,
        monitor: TransferMonitor,
    },
}

impl OcHerdrView {
    pub(crate) fn sync_file_panel_source(&mut self, cx: &mut Context<Self>) {
        if !self.file_panel.open {
            return;
        }
        let desired = self.desired_file_panel_source();
        if self.file_panel.source.as_ref() == Some(&desired) && self.file_panel.service.is_some() {
            return;
        }
        if self
            .file_panel
            .source
            .as_ref()
            .is_some_and(|source| source.profile_id == desired.profile_id)
            && self.file_panel.service.is_some()
        {
            let root = desired.suggested_root.clone();
            self.file_panel.source = Some(desired);
            self.begin_file_panel_root(root, cx);
            return;
        }
        let profile = self.current_profile();
        match FileService::new(BackendSpec::from_profile(&profile)) {
            Ok(service) => {
                let root = desired.suggested_root.clone();
                self.file_panel.reset_for_source(desired, service);
                self.begin_file_panel_root(root, cx);
            }
            Err(error) => {
                self.file_panel.error = Some(error.to_string());
                self.file_panel.service = None;
                cx.notify();
            }
        }
    }

    fn desired_file_panel_source(&self) -> FilePanelSource {
        let profile = self.current_profile();
        let workspace_root = self.snapshot.as_ref().and_then(|snapshot| {
            let workspace_id = self.selection.workspace_id.as_deref()?;
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
                .and_then(|workspace| workspace.worktree.as_ref())
                .map(|worktree| worktree.checkout_path.as_str())
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        });
        let pane_root = self.snapshot.as_ref().and_then(|snapshot| {
            self.selection
                .pane_id
                .as_deref()
                .and_then(|pane_id| snapshot.pane(pane_id))
                .and_then(|pane| pane.foreground_cwd.as_deref().or(pane.cwd.as_deref()))
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        });
        let fallback = match profile {
            ConnectionProfile::Local { .. } => {
                dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
            }
            ConnectionProfile::Ssh { .. } => PathBuf::from("."),
        };
        FilePanelSource {
            profile_id: profile.id().to_owned(),
            suggested_root: workspace_root.or(pane_root).unwrap_or(fallback),
        }
    }

    fn begin_file_panel_root(&mut self, requested: PathBuf, cx: &mut Context<Self>) {
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        self.file_panel.generation = self.file_panel.generation.wrapping_add(1);
        let generation = self.file_panel.generation;
        self.file_panel.root = None;
        self.file_panel.children.clear();
        self.file_panel.expanded.clear();
        self.file_panel.loading.clear();
        self.file_panel.selected = None;
        self.file_panel.error = None;
        self.file_panel.status = None;
        self.file_panel.address_editing = false;
        self.file_panel.address_error = None;
        self.file_panel.prompt = FilePanelPrompt::None;
        self.file_panel.prompt_error = None;
        self.file_panel.directory_tasks.clear();
        self.file_panel.address_task = None;
        self.file_panel.root_task = Some(cx.spawn(async move |this, cx| {
            let result = service.canonicalize(requested).await;
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.root_task = None;
                match result {
                    Ok(root) => {
                        this.file_panel.root = Some(root.clone());
                        this.file_panel.expanded.insert(root.clone());
                        this.load_file_panel_directory(root, true, cx);
                    }
                    Err(error) => this.file_panel.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn toggle_file_panel(&mut self, cx: &mut Context<Self>) {
        self.file_panel.open = !self.file_panel.open;
        self.file_panel.resize = None;
        self.config.set(
            "file-panel-open",
            if self.file_panel.open {
                "true"
            } else {
                "false"
            },
        );
        self.persist_settings(FailureKind::FileOperation, cx);
        if self.file_panel.open {
            self.sync_file_panel_source(cx);
        }
        cx.notify();
    }

    pub(crate) fn toggle_file_panel_hidden(&mut self, cx: &mut Context<Self>) {
        self.file_panel.show_hidden = !self.file_panel.show_hidden;
        self.config.set(
            "file-panel-show-hidden",
            if self.file_panel.show_hidden {
                "true"
            } else {
                "false"
            },
        );
        self.persist_settings(FailureKind::FileOperation, cx);
        let directories = self.file_panel.expanded.iter().cloned().collect::<Vec<_>>();
        for directory in directories {
            self.load_file_panel_directory(directory, true, cx);
        }
        cx.notify();
    }

    pub(crate) fn refresh_file_panel(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.file_panel.root.clone() else {
            self.file_panel.source = None;
            self.sync_file_panel_source(cx);
            return;
        };
        let profile = self.current_profile();
        match FileService::new(BackendSpec::from_profile(&profile)) {
            Ok(service) => {
                self.file_panel.backend_kind = Some(service.kind());
                self.file_panel.service = Some(service);
                self.begin_file_panel_root(root, cx);
            }
            Err(error) => {
                self.file_panel.error = Some(error.to_string());
                cx.notify();
            }
        }
    }

    pub(crate) fn navigate_file_panel(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.begin_file_panel_root(path, cx);
    }

    pub(crate) fn navigate_file_panel_up(&mut self, cx: &mut Context<Self>) {
        let parent = self
            .file_panel
            .root
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        if let Some(parent) = parent {
            self.begin_file_panel_root(parent, cx);
        }
    }

    pub(crate) fn open_file_panel_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.file_panel.open || self.file_panel.address_task.is_some() {
            return;
        }
        if matches!(
            self.overlay,
            Overlay::ContextMenu(_) | Overlay::FileContextMenu(_)
        ) {
            self.set_overlay(Overlay::None, cx);
        }
        let value = self
            .file_panel
            .root
            .as_ref()
            .or_else(|| {
                self.file_panel
                    .source
                    .as_ref()
                    .map(|source| &source.suggested_root)
            })
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.file_path_input
            .update(cx, |input, cx| input.set_content(value, cx));
        self.file_panel.address_editing = true;
        self.file_panel.address_error = None;
        self.file_panel.prompt = FilePanelPrompt::None;
        self.file_panel.prompt_error = None;
        self.file_path_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(crate) fn cancel_file_panel_address(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_panel.address_task.is_some() {
            return;
        }
        self.file_panel.address_editing = false;
        self.file_panel.address_error = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn submit_file_panel_address(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.file_panel.address_editing || self.file_panel.address_task.is_some() {
            return;
        }
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        let value = self.file_path_input.read(cx).content().trim().to_owned();
        let backend = self
            .file_panel
            .backend_kind
            .unwrap_or(FileBackendKind::Local);
        let current = self.file_panel.root.as_deref().unwrap_or(Path::new("."));
        let requested =
            match resolve_file_panel_address(&value, backend, current, dirs::home_dir().as_deref())
            {
                Ok(path) => path,
                Err(FileAddressError::Empty) => {
                    self.file_panel.address_error =
                        Some(self.i18n.text(k::FILES_PATH_EMPTY).to_owned());
                    cx.notify();
                    return;
                }
                Err(FileAddressError::HomeUnavailable) => {
                    self.file_panel.address_error =
                        Some(self.i18n.text(k::FILES_PATH_HOME_UNAVAILABLE).to_owned());
                    cx.notify();
                    return;
                }
                Err(FileAddressError::UnsupportedTilde) => {
                    self.file_panel.address_error =
                        Some(self.i18n.text(k::FILES_PATH_UNSUPPORTED_TILDE).to_owned());
                    cx.notify();
                    return;
                }
            };
        self.file_panel.generation = self.file_panel.generation.wrapping_add(1);
        let generation = self.file_panel.generation;
        let show_hidden = self.file_panel.show_hidden;
        self.file_panel.address_error = None;
        self.file_panel.status = None;
        self.file_panel.directory_tasks.clear();
        self.file_panel.address_task = Some(cx.spawn(async move |this, cx| {
            let result = async {
                let root = service.canonicalize(requested).await?;
                let entries = service.list_dir(root.clone(), show_hidden).await?;
                Ok::<_, ocherdr_files::FileError>((root, entries))
            }
            .await;
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.address_task = None;
                match result {
                    Ok((root, entries)) => {
                        this.file_panel.root = Some(root.clone());
                        this.file_panel.children.clear();
                        this.file_panel.children.insert(root.clone(), entries);
                        this.file_panel.expanded.clear();
                        this.file_panel.expanded.insert(root);
                        this.file_panel.loading.clear();
                        this.file_panel.selected = None;
                        this.file_panel.error = None;
                        this.file_panel.address_editing = false;
                        this.file_panel.address_error = None;
                        this.pending_focus = Some(PendingFocus::Surface);
                    }
                    Err(error) => {
                        this.file_panel.address_error = Some(error.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn load_file_panel_directory(&mut self, path: PathBuf, force: bool, cx: &mut Context<Self>) {
        if self.file_panel.directory_tasks.contains_key(&path)
            || (!force && self.file_panel.children.contains_key(&path))
        {
            return;
        }
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        let generation = self.file_panel.generation;
        let show_hidden = self.file_panel.show_hidden;
        let is_root = self.file_panel.root.as_ref() == Some(&path);
        self.file_panel.loading.insert(path.clone());
        if is_root {
            self.file_panel.error = None;
        }
        let task_path = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = service.list_dir(task_path.clone(), show_hidden).await;
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.directory_tasks.remove(&task_path);
                this.file_panel.loading.remove(&task_path);
                match result {
                    Ok(entries) => {
                        this.file_panel.children.insert(task_path, entries);
                        if is_root {
                            this.file_panel.error = None;
                        }
                    }
                    Err(error) if is_root => {
                        this.file_panel.error = Some(error.to_string());
                    }
                    Err(error) => {
                        this.file_panel.expanded.remove(&task_path);
                        this.file_panel.status = Some(error.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.file_panel.directory_tasks.insert(path, task);
        cx.notify();
    }

    pub(crate) fn activate_file_entry(
        &mut self,
        entry: FileEntry,
        double_click: bool,
        cx: &mut Context<Self>,
    ) {
        self.file_panel.selected = Some(entry.clone());
        self.file_panel.prompt = FilePanelPrompt::None;
        self.file_panel.prompt_error = None;
        if entry.kind.is_directory() {
            if double_click {
                self.navigate_file_panel(entry.path, cx);
                return;
            }
            if self.file_panel.expanded.remove(&entry.path) {
                cx.notify();
                return;
            }
            self.file_panel.expanded.insert(entry.path.clone());
            self.load_file_panel_directory(entry.path, false, cx);
        } else if double_click {
            self.open_file_panel_entry(cx);
        }
        cx.notify();
    }

    pub(crate) fn open_file_context_menu(
        &mut self,
        entry: FileEntry,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.file_panel.selected = Some(entry.clone());
        if self.file_panel.busy.is_none() {
            self.file_panel.prompt = FilePanelPrompt::None;
            self.file_panel.prompt_error = None;
            self.file_panel.address_editing = false;
            self.file_panel.address_error = None;
        }
        let viewport = window.viewport_size();
        self.set_overlay(
            Overlay::FileContextMenu(FileContextMenu {
                entry,
                x: f32::from(event.position.x)
                    .min((f32::from(viewport.width) - 220.).max(8.))
                    .max(8.),
                y: f32::from(event.position.y)
                    .min((f32::from(viewport.height) - 340.).max(8.))
                    .max(8.),
            }),
            cx,
        );
        cx.stop_propagation();
    }

    pub(crate) fn open_selected_file_panel_directory(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.file_panel.selected.clone() else {
            return;
        };
        if entry.kind.is_directory() {
            self.set_overlay(Overlay::None, cx);
            self.navigate_file_panel(entry.path, cx);
        }
    }

    pub(crate) fn open_create_file_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.file_panel.selected_directory() else {
            return;
        };
        self.file_name_input
            .update(cx, |input, cx| input.set_content("", cx));
        self.file_panel.prompt = FilePanelPrompt::CreateFile { parent };
        self.file_panel.prompt_error = None;
        self.file_name_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_create_directory_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(parent) = self.file_panel.selected_directory() else {
            return;
        };
        self.file_name_input
            .update(cx, |input, cx| input.set_content("", cx));
        self.file_panel.prompt = FilePanelPrompt::CreateDirectory { parent };
        self.file_panel.prompt_error = None;
        self.file_name_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_file_rename_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.file_panel.selected.clone() else {
            return;
        };
        self.file_name_input
            .update(cx, |input, cx| input.set_content(entry.name, cx));
        self.file_panel.prompt = FilePanelPrompt::Rename { path: entry.path };
        self.file_panel.prompt_error = None;
        self.file_name_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(crate) fn request_file_delete(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.file_panel.selected.clone() else {
            return;
        };
        self.file_panel.prompt = FilePanelPrompt::ConfirmDelete { entry };
        self.file_panel.prompt_error = None;
        cx.notify();
    }

    pub(crate) fn cancel_file_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_panel.busy.is_none() {
            self.file_panel.prompt = FilePanelPrompt::None;
            self.file_panel.prompt_error = None;
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn submit_file_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_panel.busy.is_some() || self.file_panel.operation_task.is_some() {
            return;
        }
        let prompt = self.file_panel.prompt.clone();
        let name = self.file_name_input.read(cx).content().trim().to_owned();
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        let (parent, busy) = match &prompt {
            FilePanelPrompt::CreateFile { parent }
            | FilePanelPrompt::CreateDirectory { parent } => {
                (parent.clone(), FileBusyKind::Creating)
            }
            FilePanelPrompt::Rename { path } => (
                path.parent().unwrap_or(path).to_path_buf(),
                FileBusyKind::Renaming,
            ),
            FilePanelPrompt::None | FilePanelPrompt::ConfirmDelete { .. } => return,
        };
        if let Err(error) = ocherdr_files::validate_name(&name) {
            self.file_panel.prompt_error = Some(error.to_string());
            cx.notify();
            return;
        }
        let generation = self.file_panel.generation;
        self.file_panel.busy = Some(busy);
        self.file_panel.prompt_error = None;
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result = match prompt {
                FilePanelPrompt::CreateFile { parent } => service.create_file(parent, name).await,
                FilePanelPrompt::CreateDirectory { parent } => {
                    service.create_dir(parent, name).await
                }
                FilePanelPrompt::Rename { path } => service.rename(path, name).await,
                FilePanelPrompt::None | FilePanelPrompt::ConfirmDelete { .. } => return,
            };
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.operation_task = None;
                this.file_panel.busy = None;
                match result {
                    Ok(_) => {
                        this.file_panel.prompt = FilePanelPrompt::None;
                        this.file_panel.selected = None;
                        this.file_panel.status =
                            Some(this.i18n.text(k::FILES_STATUS_SAVED).to_owned());
                        this.load_file_panel_directory(parent, true, cx);
                    }
                    Err(error) => {
                        this.file_panel.prompt_error = Some(error.to_string());
                        this.notify_failure(FailureKind::FileOperation, error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn confirm_file_delete(&mut self, cx: &mut Context<Self>) {
        if self.file_panel.busy.is_some() {
            return;
        }
        let FilePanelPrompt::ConfirmDelete { entry } = self.file_panel.prompt.clone() else {
            return;
        };
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        let parent = entry
            .path
            .parent()
            .map(std::path::Path::to_path_buf)
            .or_else(|| self.file_panel.root.clone());
        let generation = self.file_panel.generation;
        self.file_panel.busy = Some(FileBusyKind::Removing);
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result = service.remove(entry.path).await;
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.operation_task = None;
                this.file_panel.busy = None;
                match result {
                    Ok(()) => {
                        this.file_panel.prompt = FilePanelPrompt::None;
                        this.file_panel.selected = None;
                        this.file_panel.status =
                            Some(this.i18n.text(k::FILES_STATUS_REMOVED).to_owned());
                        if let Some(parent) = parent {
                            this.load_file_panel_directory(parent, true, cx);
                        }
                    }
                    Err(error) => {
                        this.file_panel.prompt_error = Some(error.to_string());
                        this.notify_failure(FailureKind::FileOperation, error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn choose_file_panel_upload(&mut self, cx: &mut Context<Self>) {
        if self.file_panel.operation_task.is_some() {
            return;
        }
        let Some(destination) = self.file_panel.selected_directory() else {
            return;
        };
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(self.i18n.text(k::FILES_UPLOAD).into()),
        });
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result = receiver.await;
            this.update(cx, |this, cx| {
                this.file_panel.operation_task = None;
                match result {
                    Ok(Ok(Some(paths))) => this.file_panel_upload_paths_to(paths, destination, cx),
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.notify_failure(FailureKind::FileOperation, error, cx),
                    Err(error) => this.notify_failure(FailureKind::FileOperation, error, cx),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn file_panel_upload_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(destination) = self.file_panel.root.clone() else {
            return;
        };
        self.file_panel_upload_paths_to(paths, destination, cx);
    }

    pub(crate) fn file_panel_upload_paths_to(
        &mut self,
        paths: Vec<PathBuf>,
        destination: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        };
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        let name = transfer_sources_label(&paths, self.i18n);
        let detail = destination.to_string_lossy().into_owned();
        let (transfer_id, monitor) =
            self.begin_file_transfer(FileTransferKind::Upload, name, detail, None, cx);
        let source_id = self
            .file_panel
            .source
            .as_ref()
            .map(|source| source.profile_id.clone());
        cx.spawn(async move |this, cx| {
            let result = service
                .upload_tracked(paths, destination.clone(), monitor)
                .await;
            this.update(cx, |this, cx| {
                let succeeded = result.is_ok();
                let failure = result
                    .as_ref()
                    .err()
                    .filter(|error| !matches!(error, FileError::Cancelled))
                    .map(ToString::to_string);
                this.finish_file_transfer(transfer_id, result.map(|_| ()), cx);
                if let Some(failure) = failure {
                    this.notify_failure(FailureKind::FileOperation, failure, cx);
                }
                if succeeded
                    && this
                        .file_panel
                        .source
                        .as_ref()
                        .map(|source| &source.profile_id)
                        == source_id.as_ref()
                    && this.file_panel.children.contains_key(&destination)
                {
                    this.load_file_panel_directory(destination, true, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn choose_file_panel_download(&mut self, cx: &mut Context<Self>) {
        if self.file_panel.operation_task.is_some() {
            return;
        }
        let Some(entry) = self.file_panel.selected.clone() else {
            return;
        };
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        let receiver = cx.prompt_for_new_path(&directory, Some(&entry.name));
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result = receiver.await;
            this.update(cx, |this, cx| {
                this.file_panel.operation_task = None;
                match result {
                    Ok(Ok(Some(destination))) => {
                        this.start_file_panel_download(entry, destination, service, cx)
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.notify_failure(FailureKind::FileOperation, error, cx),
                    Err(error) => this.notify_failure(FailureKind::FileOperation, error, cx),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn start_file_panel_download(
        &mut self,
        entry: FileEntry,
        destination: PathBuf,
        service: FileService,
        cx: &mut Context<Self>,
    ) {
        let (transfer_id, monitor) = self.begin_file_transfer(
            FileTransferKind::Download,
            entry.name,
            destination.to_string_lossy().into_owned(),
            Some(destination.clone()),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = service
                .download_tracked(entry.path, destination, monitor)
                .await;
            this.update(cx, |this, cx| {
                let failure = result
                    .as_ref()
                    .err()
                    .filter(|error| !matches!(error, FileError::Cancelled))
                    .map(ToString::to_string);
                this.finish_file_transfer(transfer_id, result.map(|_| ()), cx);
                if let Some(failure) = failure {
                    this.notify_failure(FailureKind::FileOperation, failure, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn begin_file_transfer(
        &mut self,
        kind: FileTransferKind,
        name: String,
        detail: String,
        reveal_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> (u64, TransferMonitor) {
        self.file_panel.next_transfer_id = self.file_panel.next_transfer_id.wrapping_add(1);
        let id = self.file_panel.next_transfer_id;
        let monitor = TransferMonitor::new();
        if kind != FileTransferKind::EditorSync {
            self.file_panel.transfers_open = true;
        }
        if self.file_panel.transfers.len() >= MAX_VISIBLE_TRANSFERS
            && let Some(index) = self
                .file_panel
                .transfers
                .iter()
                .position(|transfer| !transfer.running())
        {
            self.file_panel.transfers.remove(index);
        }
        self.file_panel.transfers.push(FileTransfer {
            id,
            kind,
            name,
            detail,
            progress: monitor.snapshot(),
            monitor: monitor.clone(),
            state: FileTransferState::Running,
            reveal_path,
        });
        self.poll_file_transfer(id, monitor.clone(), cx);
        cx.notify();
        (id, monitor)
    }

    fn poll_file_transfer(
        &self,
        transfer_id: u64,
        monitor: TransferMonitor,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TRANSFER_POLL_INTERVAL).await;
                let progress = monitor.snapshot();
                let finished = monitor.is_finished();
                let keep_polling = this
                    .update(cx, |this, cx| {
                        let Some(transfer) = this
                            .file_panel
                            .transfers
                            .iter_mut()
                            .find(|transfer| transfer.id == transfer_id)
                        else {
                            return false;
                        };
                        transfer.progress = progress;
                        cx.notify();
                        !finished
                    })
                    .unwrap_or(false);
                if !keep_polling {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_file_transfer(
        &mut self,
        transfer_id: u64,
        result: Result<(), FileError>,
        cx: &mut Context<Self>,
    ) {
        let Some(transfer) = self
            .file_panel
            .transfers
            .iter_mut()
            .find(|transfer| transfer.id == transfer_id)
        else {
            return;
        };
        transfer.progress = transfer.monitor.snapshot();
        transfer.state = match result {
            Ok(()) => FileTransferState::Completed,
            Err(FileError::Cancelled) => FileTransferState::Cancelled,
            Err(FileError::Conflict { .. }) => FileTransferState::Conflict,
            Err(error) => FileTransferState::Failed(error.to_string()),
        };
        cx.notify();
    }

    pub(crate) fn toggle_file_transfers(&mut self, cx: &mut Context<Self>) {
        self.file_panel.transfers_open = !self.file_panel.transfers_open;
        cx.notify();
    }

    pub(crate) fn cancel_file_transfer(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(transfer) = self
            .file_panel
            .transfers
            .iter()
            .find(|transfer| transfer.id == id && transfer.running())
        {
            transfer.monitor.cancel();
            cx.notify();
        }
    }

    pub(crate) fn clear_finished_file_transfers(&mut self, cx: &mut Context<Self>) {
        self.file_panel.transfers.retain(FileTransfer::running);
        cx.notify();
    }

    pub(crate) fn reveal_file_transfer(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(path) = self
            .file_panel
            .transfers
            .iter()
            .find(|transfer| transfer.id == id)
            .and_then(|transfer| transfer.reveal_path.as_ref())
        else {
            return;
        };
        cx.open_with_system(path.parent().unwrap_or(path));
    }

    pub(crate) fn copy_file_panel_path(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .file_panel
            .selected
            .as_ref()
            .map(|entry| entry.path.clone())
            .or_else(|| self.file_panel.root.clone())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(
            path.to_string_lossy().into_owned(),
        ));
        self.file_panel.status = Some(self.i18n.text(k::FILES_STATUS_PATH_COPIED).to_owned());
        cx.notify();
    }

    pub(crate) fn insert_file_panel_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .file_panel
            .selected
            .as_ref()
            .map(|entry| entry.path.clone())
        else {
            return;
        };
        let Some(pane_id) = self.selection.pane_id.clone() else {
            return;
        };
        self.take_terminal_control(pane_id.clone(), cx);
        let stream_closed = {
            let Some(runtime) = self.pane_mut(&pane_id) else {
                return;
            };
            runtime.terminal.paste(&shell_quote_path(&path));
            drain_terminal_input(runtime)
        };
        if stream_closed {
            self.resync_snapshot(self.event_epoch, cx);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_file_panel_entry(&mut self, cx: &mut Context<Self>) {
        self.open_file_panel_entry_with(self.file_panel.editor.clone(), cx);
    }

    fn open_file_panel_entry_with(&mut self, editor: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.file_panel.busy.is_some() {
            return;
        }
        let Some(entry) = self.file_panel.selected.clone() else {
            return;
        };
        if !entry.kind.is_file() {
            return;
        }
        if self.file_panel.backend_kind == Some(FileBackendKind::Local) {
            match launch_file_editor(&entry.path, editor.as_deref(), self.i18n, cx) {
                Ok(()) => {
                    self.file_panel.status = Some(crate::tf!(
                        self.i18n,
                        k::FILES_STATUS_OPENED,
                        name = entry.name
                    ));
                }
                Err(error) => self.notify_failure(FailureKind::FileOperation, error, cx),
            }
            cx.notify();
            return;
        }
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        if self.file_panel.editor_temp_dir.is_none() {
            match tempfile::Builder::new().prefix("ocherdr-editor-").tempdir() {
                Ok(directory) => self.file_panel.editor_temp_dir = Some(directory),
                Err(error) => {
                    self.notify_failure(FailureKind::FileOperation, error, cx);
                    return;
                }
            }
        }
        self.file_panel.editor_open_serial = self.file_panel.editor_open_serial.wrapping_add(1);
        let destination = self
            .file_panel
            .editor_temp_dir
            .as_ref()
            .expect("editor temp directory was initialized")
            .path()
            .join(format!("{:016x}", self.file_panel.editor_open_serial))
            .join(&entry.name);
        let session_id = self.file_panel.editor_open_serial;
        let (transfer_id, monitor) = self.begin_file_transfer(
            FileTransferKind::Download,
            entry.name.clone(),
            destination.to_string_lossy().into_owned(),
            None,
            cx,
        );
        self.file_panel.busy = Some(FileBusyKind::Opening);
        cx.spawn(async move |this, cx| {
            let result = async {
                let before = service.version(entry.path.clone()).await?;
                service
                    .download_tracked(entry.path.clone(), destination.clone(), monitor)
                    .await?;
                let after = service.version(entry.path.clone()).await?;
                if before != after {
                    return Err(FileError::Conflict {
                        path: entry.path.to_string_lossy().into_owned(),
                    });
                }
                Ok(after)
            }
            .await;
            this.update(cx, |this, cx| {
                this.file_panel.busy = None;
                match result {
                    Ok(version) => {
                        this.finish_file_transfer(transfer_id, Ok(()), cx);
                        match local_file_revision(&destination).and_then(|revision| {
                            launch_file_editor(&destination, editor.as_deref(), this.i18n, cx)?;
                            Ok(revision)
                        }) {
                            Ok(revision) => {
                                this.file_panel.editor_sessions.insert(
                                    session_id,
                                    RemoteEditSession {
                                        name: entry.name.clone(),
                                        remote_path: entry.path.clone(),
                                        local_path: destination.clone(),
                                        expected_remote: version,
                                        synced_revision: revision,
                                        pending_revision: None,
                                        pending_since: None,
                                        syncing: false,
                                        conflict: false,
                                    },
                                );
                                this.watch_remote_editor(session_id, service.clone(), cx);
                                this.file_panel.status = Some(crate::tf!(
                                    this.i18n,
                                    k::FILES_STATUS_OPENED_REMOTE,
                                    name = entry.name
                                ));
                            }
                            Err(error) => {
                                this.notify_failure(FailureKind::FileOperation, error, cx)
                            }
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        this.finish_file_transfer(transfer_id, Err(error), cx);
                        this.file_panel.status = Some(message.clone());
                        this.notify_failure(FailureKind::FileOperation, message, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn watch_remote_editor(
        &mut self,
        session_id: u64,
        service: FileService,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(EDITOR_WATCH_INTERVAL).await;
                let request = this
                    .update(cx, |this, cx| {
                        let request = {
                            let Some(session) =
                                this.file_panel.editor_sessions.get_mut(&session_id)
                            else {
                                return EditorWatchAction::Stop;
                            };
                            if session.conflict {
                                return EditorWatchAction::Stop;
                            }
                            let Ok(revision) = local_file_revision(&session.local_path) else {
                                return EditorWatchAction::Wait;
                            };
                            if revision == session.synced_revision {
                                session.pending_revision = None;
                                session.pending_since = None;
                                return EditorWatchAction::Wait;
                            }
                            if session.pending_revision != Some(revision) {
                                session.pending_revision = Some(revision);
                                session.pending_since = Some(Instant::now());
                                return EditorWatchAction::Wait;
                            }
                            if session.syncing
                                || session
                                    .pending_since
                                    .is_none_or(|since| since.elapsed() < EDITOR_SAVE_DEBOUNCE)
                            {
                                return EditorWatchAction::Wait;
                            }
                            session.syncing = true;
                            session.pending_revision = None;
                            session.pending_since = None;
                            (
                                session.name.clone(),
                                session.local_path.clone(),
                                session.remote_path.clone(),
                                session.expected_remote,
                                revision,
                            )
                        };
                        let (name, local_path, remote_path, expected, revision) = request;
                        let (transfer_id, monitor) = this.begin_file_transfer(
                            FileTransferKind::EditorSync,
                            name,
                            remote_path.to_string_lossy().into_owned(),
                            None,
                            cx,
                        );
                        EditorWatchAction::Sync {
                            local_path,
                            remote_path,
                            expected,
                            revision,
                            transfer_id,
                            monitor,
                        }
                    })
                    .unwrap_or(EditorWatchAction::Stop);
                let EditorWatchAction::Sync {
                    local_path,
                    remote_path,
                    expected,
                    revision,
                    transfer_id,
                    monitor,
                } = request
                else {
                    if matches!(request, EditorWatchAction::Stop) {
                        break;
                    }
                    continue;
                };

                let result = service
                    .sync_file(local_path, remote_path, Some(expected), monitor)
                    .await;
                let should_stop = this
                    .update(cx, |this, cx| match result {
                        Ok(version) => {
                            this.finish_file_transfer(transfer_id, Ok(()), cx);
                            if let Some(session) =
                                this.file_panel.editor_sessions.get_mut(&session_id)
                            {
                                session.expected_remote = version;
                                session.synced_revision = revision;
                                session.syncing = false;
                            }
                            this.file_panel.status =
                                Some(this.i18n.text(k::FILES_EDITOR_SYNCED).to_owned());
                            false
                        }
                        Err(error) => {
                            let conflict = matches!(&error, FileError::Conflict { .. });
                            let message = error.to_string();
                            this.finish_file_transfer(transfer_id, Err(error), cx);
                            if let Some(session) =
                                this.file_panel.editor_sessions.get_mut(&session_id)
                            {
                                session.syncing = false;
                                session.conflict = true;
                            }
                            this.file_panel.status = Some(if conflict {
                                this.i18n.text(k::FILES_EDITOR_CONFLICT).to_owned()
                            } else {
                                message.clone()
                            });
                            this.notify_failure(FailureKind::FileOperation, message, cx);
                            true
                        }
                    })
                    .unwrap_or(true);
                if should_stop {
                    break;
                }
            }
        });
        self.file_panel.editor_watch_tasks.insert(session_id, task);
    }

    pub(crate) fn choose_file_panel_editor(&mut self, cx: &mut Context<Self>) {
        if self.file_panel.operation_task.is_some() || self.file_panel.busy.is_some() {
            return;
        }
        self.set_overlay(Overlay::None, cx);
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some(self.i18n.text(k::FILES_EDITOR_CHOOSE).into()),
        });
        let invalid_editor = self.i18n.text(k::FILES_EDITOR_INVALID).to_owned();
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<Option<PathBuf>, String> = async {
                let selection = receiver
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
                let Some(mut paths) = selection else {
                    return Ok(None);
                };
                let Some(path) = paths.pop() else {
                    return Ok(None);
                };
                if !is_editor_path(&path) {
                    return Err(invalid_editor);
                }
                Ok(Some(path))
            }
            .await;
            this.update(cx, |this, cx| {
                this.file_panel.operation_task = None;
                match result {
                    Ok(Some(path)) => {
                        this.config.set("file-editor", &path.to_string_lossy());
                        this.file_panel.editor = Some(path);
                        this.persist_settings(FailureKind::FileOperation, cx);
                    }
                    Ok(None) => {}
                    Err(error) => this.notify_failure(FailureKind::FileOperation, error, cx),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn use_system_file_panel_editor(&mut self, cx: &mut Context<Self>) {
        self.set_overlay(Overlay::None, cx);
        self.file_panel.editor = None;
        self.config.set("file-editor", "");
        self.persist_settings(FailureKind::FileOperation, cx);
        cx.notify();
    }

    pub(crate) fn begin_file_panel_resize(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.file_panel.resize = Some(FilePanelResize {
            pointer_x: f32::from(event.position.x),
            width: self.file_panel.width,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn file_panel_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(resize) = self.file_panel.resize else {
            return false;
        };
        self.file_panel.width = (resize.width + resize.pointer_x - f32::from(event.position.x))
            .clamp(FILE_PANEL_MIN_WIDTH, FILE_PANEL_MAX_WIDTH);
        cx.stop_propagation();
        cx.notify();
        true
    }

    pub(crate) fn file_panel_mouse_up(&mut self, cx: &mut Context<Self>) -> bool {
        if self.file_panel.resize.take().is_none() {
            return false;
        }
        self.config.set(
            "file-panel-width",
            &format!("{}", self.file_panel.width.round() as u32),
        );
        self.persist_settings(FailureKind::FileOperation, cx);
        cx.stop_propagation();
        cx.notify();
        true
    }

    pub(crate) fn handle_file_panel_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        if self.file_panel.open
            && event.keystroke.key == "l"
            && only_primary_modifier(modifiers)
            && modifiers.shift == !cfg!(target_os = "macos")
        {
            self.open_file_panel_address(window, cx);
            return true;
        }
        if event.keystroke.key == "escape" && self.file_panel.address_editing {
            self.cancel_file_panel_address(window, cx);
            return true;
        }
        if event.keystroke.key == "escape"
            && !matches!(self.file_panel.prompt, FilePanelPrompt::None)
        {
            self.cancel_file_prompt(window, cx);
            return true;
        }
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileAddressError {
    Empty,
    HomeUnavailable,
    UnsupportedTilde,
}

fn resolve_file_panel_address(
    value: &str,
    backend: FileBackendKind,
    current: &Path,
    local_home: Option<&Path>,
) -> Result<PathBuf, FileAddressError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(FileAddressError::Empty);
    }
    if value == "~" || value.starts_with("~/") {
        let suffix = value.strip_prefix("~/").unwrap_or("");
        let base = match backend {
            FileBackendKind::Local => local_home.ok_or(FileAddressError::HomeUnavailable)?,
            FileBackendKind::Sftp => Path::new("."),
        };
        return Ok(base.join(suffix));
    }
    if value.starts_with('~') {
        return Err(FileAddressError::UnsupportedTilde);
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(current.join(path))
    }
}

fn launch_file_editor(
    path: &Path,
    editor: Option<&Path>,
    i18n: I18n,
    cx: &mut Context<OcHerdrView>,
) -> Result<(), String> {
    let Some(editor) = editor else {
        cx.open_with_system(path);
        return Ok(());
    };
    if !is_editor_path(editor) {
        return Err(i18n.text(k::FILES_EDITOR_INVALID).to_owned());
    }
    let mut command = if editor
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-a").arg(editor).arg(path);
        command
    } else {
        let mut command = std::process::Command::new(editor);
        command.arg(path);
        command
    };
    command.spawn().map(|_| ()).map_err(|error| {
        crate::tf!(
            i18n,
            k::FILES_EDITOR_LAUNCH_FAILED,
            editor = editor.display(),
            error = error
        )
    })
}

fn is_editor_path(path: &Path) -> bool {
    if path.is_dir()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

fn local_file_revision(path: &Path) -> Result<LocalFileRevision, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    Ok(LocalFileRevision {
        len: metadata.len(),
        modified_nanos,
    })
}

fn transfer_sources_label(paths: &[PathBuf], i18n: I18n) -> String {
    if paths.len() == 1 {
        paths[0]
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| paths[0].to_string_lossy().into_owned())
    } else {
        crate::tf!(i18n, k::FILES_TRANSFER_ITEMS, count = paths.len())
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
