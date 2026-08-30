use super::*;
use std::path::Path;

pub(crate) const FILE_PANEL_DEFAULT_WIDTH: f32 = 320.;
pub(crate) const FILE_PANEL_MIN_WIDTH: f32 = 240.;
pub(crate) const FILE_PANEL_MAX_WIDTH: f32 = 560.;
pub(crate) const FILE_PANEL_OVERLAY_BREAKPOINT: f32 = 940.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FilePanelSource {
    pub profile_id: String,
    pub suggested_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileTreeRow {
    pub depth: usize,
    pub entry: FileEntry,
    pub expanded: bool,
    pub loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FilePanelPrompt {
    None,
    CreateFile { parent: PathBuf },
    CreateDirectory { parent: PathBuf },
    Rename { path: PathBuf },
    ConfirmDelete { entry: FileEntry },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileBusyKind {
    Creating,
    Opening,
    Renaming,
    Removing,
    Uploading,
    Downloading,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FilePanelResize {
    pub pointer_x: f32,
    pub width: f32,
}

pub(crate) struct FilePanelState {
    pub open: bool,
    pub width: f32,
    pub show_hidden: bool,
    pub pinned: bool,
    pub source: Option<FilePanelSource>,
    pub service: Option<FileService>,
    pub backend_kind: Option<FileBackendKind>,
    pub root: Option<PathBuf>,
    pub children: HashMap<PathBuf, Vec<FileEntry>>,
    pub expanded: HashSet<PathBuf>,
    pub loading: HashSet<PathBuf>,
    pub selected: Option<FileEntry>,
    pub error: Option<String>,
    pub status: Option<String>,
    pub address_editing: bool,
    pub address_error: Option<String>,
    pub prompt: FilePanelPrompt,
    pub prompt_error: Option<String>,
    pub busy: Option<FileBusyKind>,
    pub generation: u64,
    pub root_task: Option<Task<()>>,
    pub directory_tasks: HashMap<PathBuf, Task<()>>,
    pub address_task: Option<Task<()>>,
    pub operation_task: Option<Task<()>>,
    pub tree_scroll: ScrollHandle,
    pub resize: Option<FilePanelResize>,
    pub editor: Option<PathBuf>,
    pub editor_temp_dir: Option<tempfile::TempDir>,
    pub editor_open_serial: u64,
}

impl FilePanelState {
    pub fn new(open: bool, width: f32, show_hidden: bool, editor: Option<PathBuf>) -> Self {
        Self {
            open,
            width: width.clamp(FILE_PANEL_MIN_WIDTH, FILE_PANEL_MAX_WIDTH),
            show_hidden,
            pinned: false,
            source: None,
            service: None,
            backend_kind: None,
            root: None,
            children: HashMap::new(),
            expanded: HashSet::new(),
            loading: HashSet::new(),
            selected: None,
            error: None,
            status: None,
            address_editing: false,
            address_error: None,
            prompt: FilePanelPrompt::None,
            prompt_error: None,
            busy: None,
            generation: 0,
            root_task: None,
            directory_tasks: HashMap::new(),
            address_task: None,
            operation_task: None,
            tree_scroll: ScrollHandle::new(),
            resize: None,
            editor,
            editor_temp_dir: None,
            editor_open_serial: 0,
        }
    }

    pub fn selected_directory(&self) -> Option<PathBuf> {
        self.selected
            .as_ref()
            .filter(|entry| entry.kind.is_directory())
            .map(|entry| entry.path.clone())
            .or_else(|| self.root.clone())
    }

    pub fn rows(&self) -> Vec<FileTreeRow> {
        let Some(root) = self.root.as_ref() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let mut visited = HashSet::new();
        self.collect_rows(root, 0, &mut visited, &mut rows);
        rows
    }

    fn collect_rows(
        &self,
        directory: &PathBuf,
        depth: usize,
        visited: &mut HashSet<PathBuf>,
        rows: &mut Vec<FileTreeRow>,
    ) {
        if !visited.insert(directory.clone()) {
            return;
        }
        let Some(children) = self.children.get(directory) else {
            return;
        };
        for entry in children {
            let expanded = entry.kind.is_directory() && self.expanded.contains(&entry.path);
            rows.push(FileTreeRow {
                depth,
                entry: entry.clone(),
                expanded,
                loading: self.loading.contains(&entry.path),
            });
            if expanded {
                self.collect_rows(&entry.path, depth + 1, visited, rows);
            }
        }
    }

    pub fn reset_for_source(&mut self, source: FilePanelSource, service: FileService) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.source = Some(source);
        self.backend_kind = Some(service.kind());
        self.service = Some(service);
        self.root = None;
        self.children.clear();
        self.expanded.clear();
        self.loading.clear();
        self.selected = None;
        self.error = None;
        self.status = None;
        self.address_editing = false;
        self.address_error = None;
        self.prompt = FilePanelPrompt::None;
        self.prompt_error = None;
        self.busy = None;
        self.root_task = None;
        self.directory_tasks.clear();
        self.address_task = None;
        self.operation_task = None;
        self.generation
    }
}

pub(crate) fn shell_quote_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        return value.into_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn human_file_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} {}", UNITS[unit])
    } else if value >= 10. {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(crate) fn breadcrumb_paths(path: &Path) -> Vec<(String, PathBuf)> {
    let mut result = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let label = match component {
            std::path::Component::RootDir => "/".to_owned(),
            _ => component.as_os_str().to_string_lossy().into_owned(),
        };
        result.push((label, current.clone()));
    }
    if result.is_empty() {
        result.push((path.to_string_lossy().into_owned(), path.to_path_buf()));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ocherdr_files::EntryKind;

    fn entry(path: &str, kind: EntryKind) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            kind,
            size: None,
            modified: None,
            permissions: None,
            hidden: false,
        }
    }

    #[test]
    fn rows_flatten_only_expanded_directories() {
        let mut panel = FilePanelState::new(true, 320., false, None);
        panel.root = Some("/repo".into());
        panel.children.insert(
            "/repo".into(),
            vec![
                entry("/repo/src", EntryKind::Directory),
                entry("/repo/README.md", EntryKind::File),
            ],
        );
        panel.children.insert(
            "/repo/src".into(),
            vec![entry("/repo/src/main.rs", EntryKind::File)],
        );
        assert_eq!(panel.rows().len(), 2);
        panel.expanded.insert("/repo/src".into());
        let rows = panel.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn shell_paths_are_posix_quoted() {
        assert_eq!(shell_quote_path(Path::new("/tmp/a.txt")), "/tmp/a.txt");
        assert_eq!(
            shell_quote_path(Path::new("/tmp/a b.txt")),
            "'/tmp/a b.txt'"
        );
        assert_eq!(shell_quote_path(Path::new("it's.txt")), "'it'\\''s.txt'");
    }

    #[test]
    fn file_sizes_stay_compact() {
        assert_eq!(human_file_size(42), "42 B");
        assert_eq!(human_file_size(1536), "1.5 KB");
        assert_eq!(human_file_size(12 * 1024), "12 KB");
    }
}
