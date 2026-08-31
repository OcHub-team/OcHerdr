//! Filesystem access for OcHerdr's file panel.
//!
//! Local and SSH filesystems expose the same typed operations. SSH uses the
//! standard SFTP subsystem over OpenSSH, so host aliases, ProxyJump, agents,
//! ports, and identity files keep the same meaning as the terminal connection.
//! A dedicated worker owns the Tokio runtime and SFTP session; GPUI callers can
//! await requests without blocking the render thread or depending on a Tokio
//! executor themselves.

use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
#[cfg(unix)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use futures::channel::oneshot;
use ocherdr_core::ConnectionProfile;
#[cfg(unix)]
use openssh::{KnownHosts, SessionBuilder};
use openssh_sftp_client::{Sftp, SftpOptions};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::process::Stdio;
use tokio::sync::mpsc;
#[cfg(windows)]
use tokio::{io::AsyncReadExt, process::Command as TokioCommand};

const TRANSFER_CHUNK_BYTES: usize = 256 * 1024;
static TEMP_FILE_SERIAL: AtomicU64 = AtomicU64::new(1);

/// The concrete transport behind a file service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Local,
    Sftp,
}

/// Serializable connection data needed by the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendSpec {
    Local,
    Sftp {
        destination: String,
        port: Option<u16>,
        identity_file: Option<PathBuf>,
    },
}

impl BackendSpec {
    pub fn from_profile(profile: &ConnectionProfile) -> Self {
        match profile {
            ConnectionProfile::Local { .. } => Self::Local,
            ConnectionProfile::Ssh {
                destination,
                port,
                identity_file,
                ..
            } => Self::Sftp {
                destination: destination.clone(),
                port: *port,
                identity_file: identity_file.clone(),
            },
        }
    }

    pub fn kind(&self) -> BackendKind {
        match self {
            Self::Local => BackendKind::Local,
            Self::Sftp { .. } => BackendKind::Sftp,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl EntryKind {
    pub fn is_directory(self) -> bool {
        self == Self::Directory
    }

    pub fn is_file(self) -> bool {
        self == Self::File
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub modified: Option<u64>,
    pub permissions: Option<u32>,
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferSummary {
    pub files: usize,
    pub directories: usize,
    pub bytes: u64,
}

/// A lightweight remote/local version token used to prevent an editor save
/// from silently overwriting a file that changed after it was opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileVersion {
    pub size: Option<u64>,
    pub modified: Option<u64>,
}

/// The latest observable state of a file transfer. Totals are optional because
/// a remote directory may be streamed before its complete size is known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransferProgress {
    pub bytes_transferred: u64,
    pub total_bytes: Option<u64>,
    pub files_transferred: usize,
    pub total_files: Option<usize>,
    pub directories_transferred: usize,
    pub total_directories: Option<usize>,
    pub current_path: Option<PathBuf>,
}

#[derive(Debug)]
struct TransferMonitorInner {
    progress: Mutex<TransferProgress>,
    cancelled: AtomicBool,
    finished: AtomicBool,
}

/// Shared progress and cancellation handle for uploads, downloads, and editor
/// synchronization. Reading a snapshot never blocks the filesystem worker for
/// longer than a small in-memory update.
#[derive(Clone, Debug)]
pub struct TransferMonitor {
    inner: Arc<TransferMonitorInner>,
}

impl Default for TransferMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferMonitor {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(TransferMonitorInner {
                progress: Mutex::new(TransferProgress::default()),
                cancelled: AtomicBool::new(false),
                finished: AtomicBool::new(false),
            }),
        }
    }

    pub fn snapshot(&self) -> TransferProgress {
        self.with_progress(|progress| progress.clone())
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub fn is_finished(&self) -> bool {
        self.inner.finished.load(Ordering::Acquire)
    }

    fn with_progress<T>(&self, f: impl FnOnce(&mut TransferProgress) -> T) -> T {
        let mut progress = self
            .inner
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut progress)
    }

    fn set_totals(&self, bytes: u64, files: usize, directories: usize) {
        self.with_progress(|progress| {
            progress.total_bytes = Some(bytes);
            progress.total_files = Some(files);
            progress.total_directories = Some(directories);
        });
    }

    fn set_current(&self, path: &Path) {
        self.with_progress(|progress| progress.current_path = Some(path.to_path_buf()));
    }

    fn add_bytes(&self, bytes: u64) {
        self.with_progress(|progress| {
            progress.bytes_transferred = progress.bytes_transferred.saturating_add(bytes);
        });
    }

    fn finish_file(&self) {
        self.with_progress(|progress| {
            progress.files_transferred = progress.files_transferred.saturating_add(1);
        });
    }

    fn finish_directory(&self) {
        self.with_progress(|progress| {
            progress.directories_transferred = progress.directories_transferred.saturating_add(1);
        });
    }

    fn ensure_active(&self) -> FileResult<()> {
        if self.is_cancelled() {
            Err(FileError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn finish(&self) {
        self.with_progress(|progress| progress.current_path = None);
        self.inner.finished.store(true, Ordering::Release);
    }
}

impl TransferSummary {
    fn add_file(&mut self, bytes: u64) {
        self.files += 1;
        self.bytes = self.bytes.saturating_add(bytes);
    }

    fn add_directory(&mut self) {
        self.directories += 1;
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FileError {
    #[error("the file service is no longer available")]
    ServiceStopped,
    #[error("invalid file name `{0}`")]
    InvalidName(String),
    #[error("cannot operate on filesystem root `{0}`")]
    RootOperation(String),
    #[error("{operation} `{path}` failed: {message}")]
    Operation {
        operation: &'static str,
        path: String,
        message: String,
    },
    #[error("SSH connection failed: {0}")]
    Connection(String),
    #[error("transfer cancelled")]
    Cancelled,
    #[error("file changed since it was opened: `{path}`")]
    Conflict { path: String },
}

impl FileError {
    fn operation(operation: &'static str, path: &Path, error: impl std::fmt::Display) -> Self {
        Self::Operation {
            operation,
            path: path.to_string_lossy().into_owned(),
            message: error.to_string(),
        }
    }
}

type FileResult<T> = Result<T, FileError>;

/// Cloneable request handle. Dropping the final handle shuts the worker down.
#[derive(Clone)]
pub struct FileService {
    inner: Arc<ServiceInner>,
}

struct ServiceInner {
    kind: BackendKind,
    tx: mpsc::UnboundedSender<Command>,
}

impl FileService {
    pub fn new(spec: BackendSpec) -> FileResult<Self> {
        let kind = spec.kind();
        let (tx, rx) = mpsc::unbounded_channel();
        thread::Builder::new()
            .name("ocherdr-files".to_owned())
            .spawn(move || run_worker(spec, rx))
            .map_err(|error| FileError::Connection(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(ServiceInner { kind, tx }),
        })
    }

    pub fn kind(&self) -> BackendKind {
        self.inner.kind
    }

    pub async fn canonicalize(&self, path: PathBuf) -> FileResult<PathBuf> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Canonicalize { path, tx })?;
        receive(rx).await
    }

    pub async fn list_dir(&self, path: PathBuf, show_hidden: bool) -> FileResult<Vec<FileEntry>> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::ListDir {
            path,
            show_hidden,
            tx,
        })?;
        receive(rx).await
    }

    pub async fn create_file(&self, parent: PathBuf, name: String) -> FileResult<PathBuf> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::CreateFile { parent, name, tx })?;
        receive(rx).await
    }

    pub async fn create_dir(&self, parent: PathBuf, name: String) -> FileResult<PathBuf> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::CreateDir { parent, name, tx })?;
        receive(rx).await
    }

    pub async fn rename(&self, path: PathBuf, name: String) -> FileResult<PathBuf> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Rename { path, name, tx })?;
        receive(rx).await
    }

    /// Local entries are moved to Trash. Remote entries are permanently
    /// removed because SFTP has no portable trash protocol.
    pub async fn remove(&self, path: PathBuf) -> FileResult<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Remove { path, tx })?;
        receive(rx).await
    }

    /// Copy local paths into a directory managed by this service.
    pub async fn upload(
        &self,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
    ) -> FileResult<TransferSummary> {
        self.upload_tracked(sources, destination_dir, TransferMonitor::new())
            .await
    }

    pub async fn upload_tracked(
        &self,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        monitor: TransferMonitor,
    ) -> FileResult<TransferSummary> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Upload {
            sources,
            destination_dir,
            monitor,
            tx,
        })?;
        receive(rx).await
    }

    /// Copy a service path to a local destination path. Directories are copied
    /// recursively; a directory destination is created when absent.
    pub async fn download(
        &self,
        source: PathBuf,
        destination: PathBuf,
    ) -> FileResult<TransferSummary> {
        self.download_tracked(source, destination, TransferMonitor::new())
            .await
    }

    pub async fn download_tracked(
        &self,
        source: PathBuf,
        destination: PathBuf,
        monitor: TransferMonitor,
    ) -> FileResult<TransferSummary> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Download {
            source,
            destination,
            monitor,
            tx,
        })?;
        receive(rx).await
    }

    pub async fn version(&self, path: PathBuf) -> FileResult<FileVersion> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Version { path, tx })?;
        receive(rx).await
    }

    /// Replace one service file from a local source after checking the version
    /// observed when the editor session began. `None` explicitly requests an
    /// overwrite, which is reserved for a user-confirmed conflict resolution.
    pub async fn sync_file(
        &self,
        source: PathBuf,
        destination: PathBuf,
        expected: Option<FileVersion>,
        monitor: TransferMonitor,
    ) -> FileResult<FileVersion> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::SyncFile {
            source,
            destination,
            expected,
            monitor,
            tx,
        })?;
        receive(rx).await
    }

    fn send(&self, command: Command) -> FileResult<()> {
        self.inner
            .tx
            .send(command)
            .map_err(|_| FileError::ServiceStopped)
    }
}

async fn receive<T>(rx: oneshot::Receiver<FileResult<T>>) -> FileResult<T> {
    rx.await.map_err(|_| FileError::ServiceStopped)?
}

enum Command {
    Canonicalize {
        path: PathBuf,
        tx: oneshot::Sender<FileResult<PathBuf>>,
    },
    ListDir {
        path: PathBuf,
        show_hidden: bool,
        tx: oneshot::Sender<FileResult<Vec<FileEntry>>>,
    },
    CreateFile {
        parent: PathBuf,
        name: String,
        tx: oneshot::Sender<FileResult<PathBuf>>,
    },
    CreateDir {
        parent: PathBuf,
        name: String,
        tx: oneshot::Sender<FileResult<PathBuf>>,
    },
    Rename {
        path: PathBuf,
        name: String,
        tx: oneshot::Sender<FileResult<PathBuf>>,
    },
    Remove {
        path: PathBuf,
        tx: oneshot::Sender<FileResult<()>>,
    },
    Upload {
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        monitor: TransferMonitor,
        tx: oneshot::Sender<FileResult<TransferSummary>>,
    },
    Download {
        source: PathBuf,
        destination: PathBuf,
        monitor: TransferMonitor,
        tx: oneshot::Sender<FileResult<TransferSummary>>,
    },
    Version {
        path: PathBuf,
        tx: oneshot::Sender<FileResult<FileVersion>>,
    },
    SyncFile {
        source: PathBuf,
        destination: PathBuf,
        expected: Option<FileVersion>,
        monitor: TransferMonitor,
        tx: oneshot::Sender<FileResult<FileVersion>>,
    },
}

fn run_worker(spec: BackendSpec, rx: mpsc::UnboundedReceiver<Command>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return,
    };
    runtime.block_on(async move {
        let mut worker = Worker::new(spec);
        worker.run(rx).await;
    });
}

enum Worker {
    Local,
    Sftp(RemoteWorker),
}

impl Worker {
    fn new(spec: BackendSpec) -> Self {
        match spec {
            BackendSpec::Local => Self::Local,
            BackendSpec::Sftp {
                destination,
                port,
                identity_file,
            } => Self::Sftp(RemoteWorker {
                destination,
                port,
                identity_file,
                session: None,
                #[cfg(windows)]
                ssh_child: None,
            }),
        }
    }

    async fn run(&mut self, mut rx: mpsc::UnboundedReceiver<Command>) {
        while let Some(command) = rx.recv().await {
            match command {
                Command::Canonicalize { path, tx } => {
                    let _ = tx.send(self.canonicalize(path).await);
                }
                Command::ListDir {
                    path,
                    show_hidden,
                    tx,
                } => {
                    let _ = tx.send(self.list_dir(path, show_hidden).await);
                }
                Command::CreateFile { parent, name, tx } => {
                    let _ = tx.send(self.create_file(parent, name).await);
                }
                Command::CreateDir { parent, name, tx } => {
                    let _ = tx.send(self.create_dir(parent, name).await);
                }
                Command::Rename { path, name, tx } => {
                    let _ = tx.send(self.rename(path, name).await);
                }
                Command::Remove { path, tx } => {
                    let _ = tx.send(self.remove(path).await);
                }
                Command::Upload {
                    sources,
                    destination_dir,
                    monitor,
                    tx,
                } => {
                    let result = self.upload(sources, destination_dir, &monitor).await;
                    monitor.finish();
                    let _ = tx.send(result);
                }
                Command::Download {
                    source,
                    destination,
                    monitor,
                    tx,
                } => {
                    let result = self.download(source, destination, &monitor).await;
                    monitor.finish();
                    let _ = tx.send(result);
                }
                Command::Version { path, tx } => {
                    let _ = tx.send(self.version(&path).await);
                }
                Command::SyncFile {
                    source,
                    destination,
                    expected,
                    monitor,
                    tx,
                } => {
                    let result = self
                        .sync_file(&source, &destination, expected, &monitor)
                        .await;
                    monitor.finish();
                    let _ = tx.send(result);
                }
            }
        }
    }

    async fn canonicalize(&mut self, path: PathBuf) -> FileResult<PathBuf> {
        match self {
            Self::Local => fs::canonicalize(&path)
                .map_err(|error| FileError::operation("canonicalize", &path, error)),
            Self::Sftp(remote) => remote.canonicalize(&path).await,
        }
    }

    async fn list_dir(&mut self, path: PathBuf, show_hidden: bool) -> FileResult<Vec<FileEntry>> {
        let mut entries = match self {
            Self::Local => list_local_dir(&path, show_hidden),
            Self::Sftp(remote) => remote.list_dir(&path, show_hidden).await,
        }?;
        sort_entries(&mut entries);
        Ok(entries)
    }

    async fn create_file(&mut self, parent: PathBuf, name: String) -> FileResult<PathBuf> {
        let path = child_path(&parent, &name)?;
        match self {
            Self::Local => fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map(|_| ())
                .map_err(|error| FileError::operation("create file", &path, error)),
            Self::Sftp(remote) => remote.create_file(&path).await,
        }?;
        Ok(path)
    }

    async fn create_dir(&mut self, parent: PathBuf, name: String) -> FileResult<PathBuf> {
        let path = child_path(&parent, &name)?;
        match self {
            Self::Local => fs::create_dir(&path)
                .map_err(|error| FileError::operation("create directory", &path, error)),
            Self::Sftp(remote) => remote.create_dir(&path).await,
        }?;
        Ok(path)
    }

    async fn rename(&mut self, path: PathBuf, name: String) -> FileResult<PathBuf> {
        let parent = path
            .parent()
            .ok_or_else(|| FileError::RootOperation(path.to_string_lossy().into_owned()))?;
        let destination = child_path(parent, &name)?;
        match self {
            Self::Local => fs::rename(&path, &destination)
                .map_err(|error| FileError::operation("rename", &path, error)),
            Self::Sftp(remote) => remote.rename(&path, &destination).await,
        }?;
        Ok(destination)
    }

    async fn remove(&mut self, path: PathBuf) -> FileResult<()> {
        reject_root(&path)?;
        match self {
            Self::Local => trash::delete(&path)
                .map_err(|error| FileError::operation("move to Trash", &path, error)),
            Self::Sftp(remote) => remote.remove_recursive(&path).await,
        }
    }

    async fn upload(
        &mut self,
        sources: Vec<PathBuf>,
        destination_dir: PathBuf,
        monitor: &TransferMonitor,
    ) -> FileResult<TransferSummary> {
        monitor.ensure_active()?;
        let totals = measure_local_paths(&sources)?;
        monitor.set_totals(totals.bytes, totals.files, totals.directories);
        let mut summary = TransferSummary {
            files: 0,
            directories: 0,
            bytes: 0,
        };
        for source in sources {
            let name = source.file_name().ok_or_else(|| {
                FileError::operation("upload", &source, "source has no file name")
            })?;
            let destination = destination_dir.join(name);
            match self {
                Self::Local => {
                    reject_recursive_local_copy(&source, &destination)?;
                    copy_local_recursive_tracked(&source, &destination, &mut summary, monitor)?
                }
                Self::Sftp(remote) => {
                    remote
                        .upload_recursive(&source, &destination, &mut summary, monitor)
                        .await?
                }
            }
        }
        Ok(summary)
    }

    async fn download(
        &mut self,
        source: PathBuf,
        destination: PathBuf,
        monitor: &TransferMonitor,
    ) -> FileResult<TransferSummary> {
        monitor.ensure_active()?;
        let mut summary = TransferSummary {
            files: 0,
            directories: 0,
            bytes: 0,
        };
        match self {
            Self::Local => {
                let totals = measure_local_paths(std::slice::from_ref(&source))?;
                monitor.set_totals(totals.bytes, totals.files, totals.directories);
                reject_recursive_local_copy(&source, &destination)?;
                copy_local_recursive_tracked(&source, &destination, &mut summary, monitor)?
            }
            Self::Sftp(remote) => {
                remote
                    .download_recursive(&source, &destination, &mut summary, monitor)
                    .await?
            }
        }
        Ok(summary)
    }

    async fn version(&mut self, path: &Path) -> FileResult<FileVersion> {
        match self {
            Self::Local => fs::symlink_metadata(path)
                .map(|metadata| file_version(&metadata))
                .map_err(|error| FileError::operation("read metadata", path, error)),
            Self::Sftp(remote) => remote.version(path).await,
        }
    }

    async fn sync_file(
        &mut self,
        source: &Path,
        destination: &Path,
        expected: Option<FileVersion>,
        monitor: &TransferMonitor,
    ) -> FileResult<FileVersion> {
        monitor.ensure_active()?;
        let actual = self.version(destination).await?;
        if expected.is_some_and(|expected| expected != actual) {
            return Err(FileError::Conflict {
                path: destination.to_string_lossy().into_owned(),
            });
        }
        let source_metadata = fs::symlink_metadata(source)
            .map_err(|error| FileError::operation("read upload source", source, error))?;
        if !source_metadata.is_file() {
            return Err(FileError::operation(
                "sync file",
                source,
                "editor source is not a regular file",
            ));
        }
        monitor.set_totals(source_metadata.len(), 1, 0);
        monitor.set_current(destination);
        match self {
            Self::Local => sync_local_file(source, destination, monitor)?,
            Self::Sftp(remote) => remote.sync_file(source, destination, monitor).await?,
        }
        monitor.finish_file();
        self.version(destination).await
    }
}

struct RemoteWorker {
    destination: String,
    port: Option<u16>,
    identity_file: Option<PathBuf>,
    session: Option<Sftp>,
    #[cfg(windows)]
    ssh_child: Option<tokio::process::Child>,
}

impl RemoteWorker {
    async fn connect(&mut self) -> FileResult<&Sftp> {
        if self.session.is_none() {
            #[cfg(unix)]
            let sftp = {
                let mut builder = SessionBuilder::default();
                builder
                    .known_hosts_check(KnownHosts::Strict)
                    .connect_timeout(Duration::from_secs(12))
                    .server_alive_interval(Duration::from_secs(20));
                if let Some(port) = self.port {
                    builder.port(port);
                }
                if let Some(identity_file) = &self.identity_file {
                    builder.keyfile(identity_file);
                }
                let ssh = builder
                    .connect(&self.destination)
                    .await
                    .map_err(|error| FileError::Connection(error.to_string()))?;
                Sftp::from_session(ssh, SftpOptions::default())
                    .await
                    .map_err(|error| FileError::Connection(error.to_string()))?
            };

            #[cfg(windows)]
            let sftp = {
                let mut command = TokioCommand::new("ssh.exe");
                command
                    .arg("-o")
                    .arg("BatchMode=yes")
                    .arg("-o")
                    .arg("StrictHostKeyChecking=yes")
                    .arg("-o")
                    .arg("ConnectTimeout=12")
                    .arg("-o")
                    .arg("ServerAliveInterval=20");
                if let Some(port) = self.port {
                    command.arg("-p").arg(port.to_string());
                }
                if let Some(identity_file) = &self.identity_file {
                    command.arg("-i").arg(identity_file);
                }
                command
                    .arg("-s")
                    .arg(&self.destination)
                    .arg("sftp")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                command.as_std_mut().creation_flags(0x0800_0000);
                let mut child = command.spawn().map_err(|error| {
                    FileError::Connection(format!("could not start ssh.exe: {error}"))
                })?;
                let stdin = child.stdin.take().ok_or_else(|| {
                    FileError::Connection("ssh.exe did not expose its standard input".into())
                })?;
                let stdout = child.stdout.take().ok_or_else(|| {
                    FileError::Connection("ssh.exe did not expose its standard output".into())
                })?;
                match Sftp::new(stdin, stdout, SftpOptions::default()).await {
                    Ok(sftp) => {
                        if let Some(mut stderr) = child.stderr.take() {
                            tokio::spawn(async move {
                                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
                            });
                        }
                        self.ssh_child = Some(child);
                        sftp
                    }
                    Err(error) => {
                        let _ = child.kill().await;
                        let mut stderr = String::new();
                        if let Some(mut pipe) = child.stderr.take() {
                            let _ = pipe.read_to_string(&mut stderr).await;
                        }
                        let detail = stderr.trim();
                        return Err(FileError::Connection(if detail.is_empty() {
                            error.to_string()
                        } else {
                            format!("{error}: {detail}")
                        }));
                    }
                }
            };
            self.session = Some(sftp);
        }
        self.session.as_ref().ok_or(FileError::ServiceStopped)
    }

    async fn canonicalize(&mut self, path: &Path) -> FileResult<PathBuf> {
        let mut fs = self.connect().await?.fs();
        fs.canonicalize(path)
            .await
            .map_err(|error| FileError::operation("canonicalize", path, error))
    }

    async fn version(&mut self, path: &Path) -> FileResult<FileVersion> {
        let mut fs = self.connect().await?.fs();
        fs.symlink_metadata(path)
            .await
            .map(|metadata| remote_file_version(&metadata))
            .map_err(|error| FileError::operation("read metadata", path, error))
    }

    async fn list_dir(&mut self, path: &Path, show_hidden: bool) -> FileResult<Vec<FileEntry>> {
        let mut fs = self.connect().await?.fs();
        let dir = fs
            .open_dir(path)
            .await
            .map_err(|error| FileError::operation("list directory", path, error))?;
        let stream = dir.read_dir();
        futures::pin_mut!(stream);
        let mut entries = Vec::new();
        while let Some(entry) = stream.next().await {
            let entry =
                entry.map_err(|error| FileError::operation("list directory", path, error))?;
            let filename = entry.filename();
            if filename == OsStr::new(".") || filename == OsStr::new("..") {
                continue;
            }
            let name = filename.to_string_lossy().into_owned();
            let hidden = is_hidden_name(&name);
            if hidden && !show_hidden {
                continue;
            }
            let metadata = entry.metadata();
            entries.push(FileEntry {
                path: path.join(filename),
                name,
                kind: remote_entry_kind(metadata.file_type()),
                size: metadata.len(),
                modified: metadata.modified().map(|time| u64::from(time.into_raw())),
                permissions: metadata.permissions().map(remote_permissions_mode),
                hidden,
            });
        }
        Ok(entries)
    }

    async fn create_file(&mut self, path: &Path) -> FileResult<()> {
        let sftp = self.connect().await?;
        let file = sftp
            .options()
            .write(true)
            .create_new(true)
            .open(path)
            .await
            .map_err(|error| FileError::operation("create file", path, error))?;
        file.close()
            .await
            .map_err(|error| FileError::operation("close file", path, error))
    }

    async fn create_dir(&mut self, path: &Path) -> FileResult<()> {
        let mut fs = self.connect().await?.fs();
        fs.create_dir(path)
            .await
            .map_err(|error| FileError::operation("create directory", path, error))
    }

    async fn rename(&mut self, path: &Path, destination: &Path) -> FileResult<()> {
        let mut fs = self.connect().await?.fs();
        fs.rename(path, destination)
            .await
            .map_err(|error| FileError::operation("rename", path, error))
    }

    async fn remove_recursive(&mut self, path: &Path) -> FileResult<()> {
        let mut pending = vec![(path.to_path_buf(), false)];
        while let Some((current, visited)) = pending.pop() {
            let mut fs = self.connect().await?.fs();
            let metadata = fs
                .symlink_metadata(&current)
                .await
                .map_err(|error| FileError::operation("read metadata", &current, error))?;
            let directory = metadata.file_type().is_some_and(|kind| kind.is_dir());
            if directory && !visited {
                pending.push((current.clone(), true));
                let children = self.list_dir(&current, true).await?;
                for child in children.into_iter().rev() {
                    pending.push((child.path, false));
                }
            } else if directory {
                fs.remove_dir(&current)
                    .await
                    .map_err(|error| FileError::operation("remove directory", &current, error))?;
            } else {
                fs.remove_file(&current)
                    .await
                    .map_err(|error| FileError::operation("remove file", &current, error))?;
            }
        }
        Ok(())
    }

    async fn upload_recursive(
        &mut self,
        source: &Path,
        destination: &Path,
        summary: &mut TransferSummary,
        monitor: &TransferMonitor,
    ) -> FileResult<()> {
        let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
        while let Some((local, remote)) = pending.pop() {
            monitor.ensure_active()?;
            monitor.set_current(&remote);
            let metadata = fs::symlink_metadata(&local)
                .map_err(|error| FileError::operation("read upload source", &local, error))?;
            if metadata.is_dir() {
                let mut remote_fs = self.connect().await?.fs();
                if let Err(error) = remote_fs.create_dir(&remote).await {
                    let existing = remote_fs.metadata(&remote).await.ok();
                    if !existing
                        .and_then(|metadata| metadata.file_type())
                        .is_some_and(|kind| kind.is_dir())
                    {
                        return Err(FileError::operation("create directory", &remote, error));
                    }
                }
                summary.add_directory();
                monitor.finish_directory();
                let mut children = fs::read_dir(&local)
                    .map_err(|error| FileError::operation("read upload directory", &local, error))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        FileError::operation("read upload directory", &local, error)
                    })?;
                children.sort_by_key(|entry| entry.file_name());
                for child in children.into_iter().rev() {
                    pending.push((child.path(), remote.join(child.file_name())));
                }
            } else if metadata.is_file() {
                self.upload_file(&local, &remote, monitor).await?;
                summary.add_file(metadata.len());
                monitor.finish_file();
            } else {
                return Err(FileError::operation(
                    "upload",
                    &local,
                    "symbolic links and special files are not uploaded",
                ));
            }
        }
        Ok(())
    }

    async fn upload_file(
        &mut self,
        source: &Path,
        destination: &Path,
        monitor: &TransferMonitor,
    ) -> FileResult<()> {
        let mut input = fs::File::open(source)
            .map_err(|error| FileError::operation("open upload source", source, error))?;
        let temporary = transfer_temp_path(destination)?;
        let existing_permissions = {
            let mut remote_fs = self.connect().await?.fs();
            remote_fs
                .symlink_metadata(destination)
                .await
                .ok()
                .and_then(|metadata| metadata.permissions())
        };
        let mut output = self
            .connect()
            .await?
            .create(&temporary)
            .await
            .map_err(|error| FileError::operation("create remote file", &temporary, error))?;
        let write_result = async {
            let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
            loop {
                monitor.ensure_active()?;
                let read = input
                    .read(&mut buffer)
                    .map_err(|error| FileError::operation("read upload source", source, error))?;
                if read == 0 {
                    break;
                }
                output.write_all(&buffer[..read]).await.map_err(|error| {
                    FileError::operation("write remote file", &temporary, error)
                })?;
                monitor.add_bytes(read as u64);
            }
            output
                .close()
                .await
                .map_err(|error| FileError::operation("close remote file", &temporary, error))?;
            monitor.ensure_active()
        }
        .await;
        if let Err(error) = write_result {
            let mut remote_fs = self.connect().await?.fs();
            let _ = remote_fs.remove_file(&temporary).await;
            return Err(error);
        }

        let mut remote_fs = self.connect().await?.fs();
        if let Some(permissions) = existing_permissions
            && let Err(error) = remote_fs.set_permissions(&temporary, permissions).await
        {
            let _ = remote_fs.remove_file(&temporary).await;
            return Err(FileError::operation(
                "preserve remote permissions",
                &temporary,
                error,
            ));
        }
        if let Err(error) = remote_fs.rename(&temporary, destination).await {
            let _ = remote_fs.remove_file(&temporary).await;
            return Err(FileError::operation(
                "replace remote file",
                destination,
                error,
            ));
        }
        Ok(())
    }

    async fn sync_file(
        &mut self,
        source: &Path,
        destination: &Path,
        monitor: &TransferMonitor,
    ) -> FileResult<()> {
        self.upload_file(source, destination, monitor).await
    }

    async fn download_recursive(
        &mut self,
        source: &Path,
        destination: &Path,
        summary: &mut TransferSummary,
        monitor: &TransferMonitor,
    ) -> FileResult<()> {
        let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
        while let Some((remote, local)) = pending.pop() {
            monitor.ensure_active()?;
            monitor.set_current(&remote);
            let mut remote_fs = self.connect().await?.fs();
            let metadata = remote_fs
                .symlink_metadata(&remote)
                .await
                .map_err(|error| FileError::operation("read metadata", &remote, error))?;
            let kind = remote_entry_kind(metadata.file_type());
            if kind == EntryKind::Directory {
                fs::create_dir_all(&local).map_err(|error| {
                    FileError::operation("create download directory", &local, error)
                })?;
                summary.add_directory();
                monitor.finish_directory();
                let children = self.list_dir(&remote, true).await?;
                for child in children.into_iter().rev() {
                    pending.push((child.path, local.join(child.name)));
                }
            } else if kind == EntryKind::File {
                self.download_file(&remote, &local, monitor).await?;
                summary.add_file(metadata.len().unwrap_or(0));
                monitor.finish_file();
            } else {
                return Err(FileError::operation(
                    "download",
                    &remote,
                    "symbolic links and special files are not downloaded",
                ));
            }
        }
        Ok(())
    }

    async fn download_file(
        &mut self,
        source: &Path,
        destination: &Path,
        monitor: &TransferMonitor,
    ) -> FileResult<()> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                FileError::operation("create download directory", parent, error)
            })?;
        }
        let temporary = transfer_temp_path(destination)?;
        let mut input = self
            .connect()
            .await?
            .open(source)
            .await
            .map_err(|error| FileError::operation("open remote file", source, error))?;
        let mut output = fs::File::create(&temporary)
            .map_err(|error| FileError::operation("create download file", &temporary, error))?;
        let copy_result = async {
            loop {
                monitor.ensure_active()?;
                let Some(bytes) = input
                    .read(TRANSFER_CHUNK_BYTES as u32, Default::default())
                    .await
                    .map_err(|error| FileError::operation("read remote file", source, error))?
                else {
                    break;
                };
                output.write_all(&bytes).map_err(|error| {
                    FileError::operation("write download file", &temporary, error)
                })?;
                monitor.add_bytes(bytes.len() as u64);
            }
            input
                .close()
                .await
                .map_err(|error| FileError::operation("close remote file", source, error))?;
            monitor.ensure_active()
        }
        .await;
        if let Err(error) = copy_result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temporary, destination) {
            let _ = fs::remove_file(&temporary);
            return Err(FileError::operation(
                "replace download file",
                destination,
                error,
            ));
        }
        Ok(())
    }
}

fn list_local_dir(path: &Path, show_hidden: bool) -> FileResult<Vec<FileEntry>> {
    let read =
        fs::read_dir(path).map_err(|error| FileError::operation("list directory", path, error))?;
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.map_err(|error| FileError::operation("list directory", path, error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let hidden = is_hidden_name(&name);
        if hidden && !show_hidden {
            continue;
        }
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path)
            .map_err(|error| FileError::operation("read metadata", &entry_path, error))?;
        entries.push(local_file_entry(entry_path, name, hidden, metadata));
    }
    Ok(entries)
}

fn local_file_entry(
    path: PathBuf,
    name: String,
    hidden: bool,
    metadata: fs::Metadata,
) -> FileEntry {
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode() & 0o7777)
    };
    #[cfg(windows)]
    let permissions = None;
    let file_type = metadata.file_type();
    FileEntry {
        path,
        name,
        kind: if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        },
        size: metadata.is_file().then_some(metadata.len()),
        modified: metadata.modified().ok().and_then(system_time_seconds),
        permissions,
        hidden,
    }
}

#[cfg(test)]
fn copy_local_recursive(
    source: &Path,
    destination: &Path,
    summary: &mut TransferSummary,
) -> FileResult<()> {
    copy_local_recursive_tracked(source, destination, summary, &TransferMonitor::new())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TransferTotals {
    bytes: u64,
    files: usize,
    directories: usize,
}

fn measure_local_paths(paths: &[PathBuf]) -> FileResult<TransferTotals> {
    let mut totals = TransferTotals::default();
    let mut pending = paths.to_vec();
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| FileError::operation("read source metadata", &path, error))?;
        if metadata.is_dir() {
            totals.directories = totals.directories.saturating_add(1);
            let entries = fs::read_dir(&path)
                .map_err(|error| FileError::operation("read source directory", &path, error))?;
            for entry in entries {
                pending.push(
                    entry
                        .map_err(|error| {
                            FileError::operation("read source directory", &path, error)
                        })?
                        .path(),
                );
            }
        } else if metadata.is_file() {
            totals.files = totals.files.saturating_add(1);
            totals.bytes = totals.bytes.saturating_add(metadata.len());
        } else {
            return Err(FileError::operation(
                "transfer",
                &path,
                "symbolic links and special files are not transferred",
            ));
        }
    }
    Ok(totals)
}

fn copy_local_recursive_tracked(
    source: &Path,
    destination: &Path,
    summary: &mut TransferSummary,
    monitor: &TransferMonitor,
) -> FileResult<()> {
    monitor.ensure_active()?;
    monitor.set_current(destination);
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| FileError::operation("read source metadata", source, error))?;
    if metadata.is_dir() {
        fs::create_dir_all(destination).map_err(|error| {
            FileError::operation("create destination directory", destination, error)
        })?;
        summary.add_directory();
        monitor.finish_directory();
        let read = fs::read_dir(source)
            .map_err(|error| FileError::operation("read source directory", source, error))?;
        for entry in read {
            let entry = entry
                .map_err(|error| FileError::operation("read source directory", source, error))?;
            copy_local_recursive_tracked(
                &entry.path(),
                &destination.join(entry.file_name()),
                summary,
                monitor,
            )?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                FileError::operation("create destination directory", parent, error)
            })?;
        }
        let bytes = copy_local_file_tracked(source, destination, monitor)?;
        summary.add_file(bytes);
        monitor.finish_file();
    } else {
        return Err(FileError::operation(
            "copy",
            source,
            "symbolic links and special files are not copied",
        ));
    }
    Ok(())
}

fn copy_local_file_tracked(
    source: &Path,
    destination: &Path,
    monitor: &TransferMonitor,
) -> FileResult<u64> {
    let mut input = fs::File::open(source)
        .map_err(|error| FileError::operation("open copy source", source, error))?;
    let temporary = transfer_temp_path(destination)?;
    let mut output = fs::File::create(&temporary)
        .map_err(|error| FileError::operation("create copy destination", &temporary, error))?;
    let result = (|| {
        let mut copied = 0_u64;
        let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
        loop {
            monitor.ensure_active()?;
            let read = input
                .read(&mut buffer)
                .map_err(|error| FileError::operation("read copy source", source, error))?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read]).map_err(|error| {
                FileError::operation("write copy destination", &temporary, error)
            })?;
            copied = copied.saturating_add(read as u64);
            monitor.add_bytes(read as u64);
        }
        output
            .flush()
            .map_err(|error| FileError::operation("flush copy destination", &temporary, error))?;
        monitor.ensure_active()?;
        Ok(copied)
    })();
    let copied = match result {
        Ok(copied) => copied,
        Err(error) => {
            drop(output);
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };

    let permissions = fs::symlink_metadata(source)
        .map_err(|error| FileError::operation("read source metadata", source, error))?
        .permissions();
    if let Err(error) = fs::set_permissions(&temporary, permissions) {
        drop(output);
        let _ = fs::remove_file(&temporary);
        return Err(FileError::operation(
            "preserve file permissions",
            &temporary,
            error,
        ));
    }
    drop(output);
    if let Err(error) = fs::rename(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(FileError::operation(
            "replace copied file",
            destination,
            error,
        ));
    }
    Ok(copied)
}

fn sync_local_file(source: &Path, destination: &Path, monitor: &TransferMonitor) -> FileResult<()> {
    let destination_permissions = fs::symlink_metadata(destination)
        .map_err(|error| FileError::operation("read destination metadata", destination, error))?
        .permissions();
    copy_local_file_tracked(source, destination, monitor)?;
    fs::set_permissions(destination, destination_permissions).map_err(|error| {
        FileError::operation("preserve destination permissions", destination, error)
    })
}

fn transfer_temp_path(destination: &Path) -> FileResult<PathBuf> {
    let name = destination.file_name().ok_or_else(|| {
        FileError::operation(
            "create transfer temporary file",
            destination,
            "destination has no file name",
        )
    })?;
    let serial = TEMP_FILE_SERIAL.fetch_add(1, Ordering::Relaxed);
    Ok(destination.with_file_name(format!(
        ".{}.ocherdr-transfer-{}-{serial}",
        name.to_string_lossy(),
        std::process::id()
    )))
}

fn file_version(metadata: &fs::Metadata) -> FileVersion {
    FileVersion {
        size: metadata.is_file().then_some(metadata.len()),
        modified: metadata.modified().ok().and_then(system_time_seconds),
    }
}

fn remote_file_version(metadata: &openssh_sftp_client::metadata::MetaData) -> FileVersion {
    FileVersion {
        size: metadata.len(),
        modified: metadata.modified().map(|time| u64::from(time.into_raw())),
    }
}

fn reject_recursive_local_copy(source: &Path, destination: &Path) -> FileResult<()> {
    let canonical_source = fs::canonicalize(source)
        .map_err(|error| FileError::operation("canonicalize copy source", source, error))?;
    let canonical_destination = if destination.exists() {
        fs::canonicalize(destination).map_err(|error| {
            FileError::operation("canonicalize copy destination", destination, error)
        })?
    } else {
        let parent = destination.parent().ok_or_else(|| {
            FileError::operation("copy", destination, "destination has no parent directory")
        })?;
        let canonical_parent = fs::canonicalize(parent).map_err(|error| {
            FileError::operation("canonicalize copy destination", parent, error)
        })?;
        canonical_parent.join(destination.file_name().ok_or_else(|| {
            FileError::operation("copy", destination, "destination has no file name")
        })?)
    };
    let source_is_directory = fs::symlink_metadata(&canonical_source)
        .map_err(|error| FileError::operation("read source metadata", source, error))?
        .is_dir();
    if canonical_destination == canonical_source
        || (source_is_directory && canonical_destination.starts_with(&canonical_source))
    {
        return Err(FileError::operation(
            "copy",
            source,
            "destination is the source or one of its descendants",
        ));
    }
    Ok(())
}

fn child_path(parent: &Path, name: &str) -> FileResult<PathBuf> {
    validate_name(name)?;
    Ok(parent.join(name))
}

pub fn validate_name(name: &str) -> FileResult<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\0')
        || Path::new(name)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(FileError::InvalidName(name.to_owned()));
    }
    Ok(())
}

fn reject_root(path: &Path) -> FileResult<()> {
    if path.parent().is_none() {
        Err(FileError::RootOperation(
            path.to_string_lossy().into_owned(),
        ))
    } else {
        Ok(())
    }
}

fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".."
}

fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|left, right| {
        right
            .kind
            .is_directory()
            .cmp(&left.kind.is_directory())
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn system_time_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs())
}

fn remote_entry_kind(file_type: Option<openssh_sftp_client::metadata::FileType>) -> EntryKind {
    match file_type {
        Some(kind) if kind.is_dir() => EntryKind::Directory,
        Some(kind) if kind.is_file() => EntryKind::File,
        Some(kind) if kind.is_symlink() => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

fn remote_permissions_mode(permissions: openssh_sftp_client::metadata::Permissions) -> u32 {
    let mut mode = 0;
    let bits = [
        (permissions.read_by_owner(), 0o400),
        (permissions.write_by_owner(), 0o200),
        (permissions.execute_by_owner(), 0o100),
        (permissions.read_by_group(), 0o040),
        (permissions.write_by_group(), 0o020),
        (permissions.execute_by_group(), 0o010),
        (permissions.read_by_other(), 0o004),
        (permissions.write_by_other(), 0o002),
        (permissions.execute_by_other(), 0o001),
        (permissions.suid(), 0o4000),
        (permissions.sgid(), 0o2000),
        (permissions.svtx(), 0o1000),
    ];
    for (set, bit) in bits {
        if set {
            mode |= bit;
        }
    }
    mode
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
