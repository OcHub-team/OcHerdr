use super::*;
use ocherdr_files::{BackendSpec, TransferSummary};
use std::path::Path;

impl OcHerdrView {
    pub(crate) fn sync_file_panel_source(&mut self, cx: &mut Context<Self>) {
        if !self.file_panel.open {
            return;
        }
        let desired = self.desired_file_panel_source();
        if self.file_panel.pinned
            && self
                .file_panel
                .source
                .as_ref()
                .is_some_and(|source| source.profile_id == desired.profile_id)
            && self.file_panel.service.is_some()
        {
            return;
        }
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

    pub(crate) fn close_file_panel(&mut self, cx: &mut Context<Self>) {
        if self.file_panel.open {
            self.toggle_file_panel(cx);
        }
    }

    pub(crate) fn toggle_file_panel_pin(&mut self, cx: &mut Context<Self>) {
        self.file_panel.pinned = !self.file_panel.pinned;
        if !self.file_panel.pinned {
            self.file_panel.source = None;
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
        if self.file_panel.busy.is_some() {
            return;
        }
        let Some(destination) = self.file_panel.selected_directory() else {
            return;
        };
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(self.i18n.text(k::FILES_UPLOAD).into()),
        });
        let generation = self.file_panel.generation;
        self.file_panel.busy = Some(FileBusyKind::Uploading);
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<Option<TransferSummary>, String> = async {
                let selection = receiver
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
                let Some(paths) = selection else {
                    return Ok(None);
                };
                service
                    .upload(paths, destination.clone())
                    .await
                    .map(Some)
                    .map_err(|error| error.to_string())
            }
            .await;
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.operation_task = None;
                this.file_panel.busy = None;
                match result {
                    Ok(Some(summary)) => {
                        this.file_panel.status = Some(transfer_status(
                            summary,
                            this.i18n.text(k::FILES_STATUS_UPLOADED),
                            this.i18n,
                        ));
                        this.load_file_panel_directory(destination, true, cx);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        this.file_panel.status = Some(error.clone());
                        this.notify_failure(FailureKind::FileOperation, error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn file_panel_upload_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() || self.file_panel.busy.is_some() {
            return;
        }
        let Some(destination) = self.file_panel.selected_directory() else {
            return;
        };
        let Some(service) = self.file_panel.service.clone() else {
            return;
        };
        let generation = self.file_panel.generation;
        self.file_panel.busy = Some(FileBusyKind::Uploading);
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result = service.upload(paths, destination.clone()).await;
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.operation_task = None;
                this.file_panel.busy = None;
                match result {
                    Ok(summary) => {
                        this.file_panel.status = Some(transfer_status(
                            summary,
                            this.i18n.text(k::FILES_STATUS_UPLOADED),
                            this.i18n,
                        ));
                        this.load_file_panel_directory(destination, true, cx);
                    }
                    Err(error) => {
                        this.file_panel.status = Some(error.to_string());
                        this.notify_failure(FailureKind::FileOperation, error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn choose_file_panel_download(&mut self, cx: &mut Context<Self>) {
        if self.file_panel.busy.is_some() {
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
        let generation = self.file_panel.generation;
        self.file_panel.busy = Some(FileBusyKind::Downloading);
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<Option<TransferSummary>, String> = async {
                let selection = receiver
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
                let Some(destination) = selection else {
                    return Ok(None);
                };
                service
                    .download(entry.path, destination)
                    .await
                    .map(Some)
                    .map_err(|error| error.to_string())
            }
            .await;
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.operation_task = None;
                this.file_panel.busy = None;
                match result {
                    Ok(Some(summary)) => {
                        this.file_panel.status = Some(transfer_status(
                            summary,
                            this.i18n.text(k::FILES_STATUS_DOWNLOADED),
                            this.i18n,
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        this.file_panel.status = Some(error.clone());
                        this.notify_failure(FailureKind::FileOperation, error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
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
        let generation = self.file_panel.generation;
        self.file_panel.busy = Some(FileBusyKind::Opening);
        self.file_panel.operation_task = Some(cx.spawn(async move |this, cx| {
            let result = service.download(entry.path, destination.clone()).await;
            this.update(cx, |this, cx| {
                if this.file_panel.generation != generation {
                    return;
                }
                this.file_panel.operation_task = None;
                this.file_panel.busy = None;
                match result {
                    Ok(_) => match mark_editor_copy_read_only(&destination, this.i18n) {
                        Ok(()) => {
                            match launch_file_editor(&destination, editor.as_deref(), this.i18n, cx)
                            {
                                Ok(()) => {
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
                        Err(error) => this.notify_failure(FailureKind::FileOperation, error, cx),
                    },
                    Err(error) => {
                        this.file_panel.status = Some(error.to_string());
                        this.notify_failure(FailureKind::FileOperation, error, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
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
            && modifiers.platform
            && !modifiers.alt
            && !modifiers.control
            && !modifiers.shift
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

fn mark_editor_copy_read_only(path: &Path, i18n: I18n) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444)).map_err(|error| {
            crate::tf!(
                i18n,
                k::FILES_EDITOR_READ_ONLY_FAILED,
                path = path.display(),
                error = error
            )
        })
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(path)
            .map_err(|error| error.to_string())?
            .permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions).map_err(|error| error.to_string())
    }
}

fn transfer_status(summary: TransferSummary, verb: &str, i18n: I18n) -> String {
    crate::tf!(
        i18n,
        k::FILES_STATUS_TRANSFER,
        verb = verb,
        files = summary.files,
        folders = summary.directories,
        size = human_file_size(summary.bytes),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ocherdr_files::FileError;

    #[test]
    fn file_errors_keep_operation_context() {
        let error = FileError::InvalidName("../bad".into());
        assert!(error.to_string().contains("../bad"));
    }

    #[test]
    fn addresses_support_absolute_relative_and_home_paths() {
        let current = Path::new("/repo/src");
        let home = Path::new("/Users/tester");
        assert_eq!(
            resolve_file_panel_address("/tmp", FileBackendKind::Local, current, Some(home)),
            Ok(PathBuf::from("/tmp"))
        );
        assert_eq!(
            resolve_file_panel_address("../docs", FileBackendKind::Local, current, Some(home)),
            Ok(PathBuf::from("/repo/src/../docs"))
        );
        assert_eq!(
            resolve_file_panel_address("~/code", FileBackendKind::Local, current, Some(home)),
            Ok(PathBuf::from("/Users/tester/code"))
        );
        assert_eq!(
            resolve_file_panel_address("~/code", FileBackendKind::Sftp, current, None),
            Ok(PathBuf::from("./code"))
        );
    }

    #[test]
    fn addresses_reject_empty_or_named_home_shortcuts() {
        assert_eq!(
            resolve_file_panel_address(" ", FileBackendKind::Local, Path::new("/"), None),
            Err(FileAddressError::Empty)
        );
        assert_eq!(
            resolve_file_panel_address("~other", FileBackendKind::Sftp, Path::new("/"), None),
            Err(FileAddressError::UnsupportedTilde)
        );
    }

    #[test]
    fn editor_paths_accept_apps_and_executables_only() {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("Editor.app");
        std::fs::create_dir(&app).unwrap();
        let executable = directory.path().join("editor");
        std::fs::write(&executable, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let ordinary_directory = directory.path().join("folder");
        std::fs::create_dir(&ordinary_directory).unwrap();
        assert!(is_editor_path(&app));
        assert!(is_editor_path(&executable));
        assert!(!is_editor_path(&ordinary_directory));
    }

    #[cfg(unix)]
    #[test]
    fn remote_editor_copies_are_read_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("remote.txt");
        std::fs::write(&path, b"remote contents").unwrap();
        mark_editor_copy_read_only(&path, I18n::new(Language::English)).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }
}
