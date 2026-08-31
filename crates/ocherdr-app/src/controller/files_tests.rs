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

#[test]
fn local_revisions_change_after_a_save() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("remote.txt");
    std::fs::write(&path, b"remote contents").unwrap();
    let before = local_file_revision(&path).unwrap();
    std::fs::write(&path, b"edited remote contents").unwrap();
    let after = local_file_revision(&path).unwrap();
    assert_ne!(before, after);
}
