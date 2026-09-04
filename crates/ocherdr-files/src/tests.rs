use super::*;

#[test]
fn names_are_single_safe_path_components() {
    for invalid in ["", ".", "..", "a/b", "bad\0name"] {
        assert!(validate_name(invalid).is_err(), "{invalid:?}");
    }
    for valid in ["main.rs", ".env", "资料"] {
        assert!(validate_name(valid).is_ok(), "{valid:?}");
    }
}

#[test]
fn local_listing_sorts_directories_first_and_filters_hidden() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("z.txt"), b"z").unwrap();
    fs::write(temp.path().join("A.txt"), b"a").unwrap();
    fs::write(temp.path().join(".secret"), b"x").unwrap();
    fs::create_dir(temp.path().join("folder")).unwrap();

    let entries = list_local_dir(temp.path(), false)
        .map(|mut entries| {
            sort_entries(&mut entries);
            entries
        })
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["folder", "A.txt", "z.txt"]
    );
    assert_eq!(entries[0].kind, EntryKind::Directory);
    let entries = list_local_dir(temp.path(), true).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == ".secret" && entry.hidden)
    );
}

#[test]
fn local_service_handles_repeated_navigation_requests() {
    let temp = tempfile::tempdir().unwrap();
    let nested = temp.path().join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("file.txt"), b"contents").unwrap();
    let service = FileService::new(BackendSpec::Local).unwrap();

    let first = futures::executor::block_on(service.canonicalize(temp.path().into())).unwrap();
    assert_eq!(
        futures::executor::block_on(service.list_dir(first, false))
            .unwrap()
            .len(),
        1
    );
    let second = futures::executor::block_on(service.canonicalize(nested)).unwrap();
    assert_eq!(
        futures::executor::block_on(service.list_dir(second, false))
            .unwrap()
            .first()
            .map(|entry| entry.name.as_str()),
        Some("file.txt")
    );
}

#[test]
fn local_copy_preserves_a_directory_tree() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/data.txt"), b"payload").unwrap();
    let mut summary = TransferSummary {
        files: 0,
        directories: 0,
        bytes: 0,
    };
    copy_local_recursive(&source, &destination, &mut summary).unwrap();
    assert_eq!(
        fs::read(destination.join("nested/data.txt")).unwrap(),
        b"payload"
    );
    assert_eq!(summary.files, 1);
    assert_eq!(summary.directories, 2);
    assert_eq!(summary.bytes, 7);
}

#[test]
fn local_copy_rejects_source_and_descendant_destinations() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("child")).unwrap();

    assert!(reject_recursive_local_copy(&source, &source).is_err());
    assert!(reject_recursive_local_copy(&source, &source.join("child/copy")).is_err());
    assert!(reject_recursive_local_copy(&source, &temp.path().join("copy")).is_ok());
}

#[test]
fn tracked_local_transfer_reports_exact_progress() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("one.txt"), b"one").unwrap();
    fs::write(source.join("nested/two.txt"), b"second").unwrap();
    let service = FileService::new(BackendSpec::Local).unwrap();
    let monitor = TransferMonitor::new();

    let summary = futures::executor::block_on(service.upload_tracked(
        vec![source],
        destination.clone(),
        monitor.clone(),
    ))
    .unwrap();

    assert_eq!(summary.files, 2);
    assert_eq!(summary.directories, 2);
    assert_eq!(summary.bytes, 9);
    assert_eq!(
        fs::read(destination.join("source/nested/two.txt")).unwrap(),
        b"second"
    );
    assert_eq!(
        monitor.snapshot(),
        TransferProgress {
            bytes_transferred: 9,
            total_bytes: Some(9),
            files_transferred: 2,
            total_files: Some(2),
            directories_transferred: 2,
            total_directories: Some(2),
            current_path: None,
        }
    );
    assert!(monitor.is_finished());
}

#[test]
fn cancelled_transfer_does_not_create_a_destination() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.txt");
    let destination = temp.path().join("destination");
    fs::write(&source, b"payload").unwrap();
    fs::create_dir(&destination).unwrap();
    let service = FileService::new(BackendSpec::Local).unwrap();
    let monitor = TransferMonitor::new();
    monitor.cancel();

    let error = futures::executor::block_on(service.upload_tracked(
        vec![source],
        destination.clone(),
        monitor.clone(),
    ))
    .unwrap_err();

    assert!(matches!(error, FileError::Cancelled));
    assert!(!destination.join("source.txt").exists());
    assert!(monitor.is_finished());
}

#[test]
fn editor_sync_refuses_stale_versions_and_replaces_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let local_edit = temp.path().join("edit.txt");
    let destination = temp.path().join("remote.txt");
    fs::write(&local_edit, b"edited contents").unwrap();
    fs::write(&destination, b"original").unwrap();
    let service = FileService::new(BackendSpec::Local).unwrap();
    let observed = futures::executor::block_on(service.version(destination.clone())).unwrap();
    fs::write(&destination, b"changed elsewhere").unwrap();

    let conflict = futures::executor::block_on(service.sync_file(
        local_edit.clone(),
        destination.clone(),
        Some(observed),
        TransferMonitor::new(),
    ))
    .unwrap_err();
    assert!(matches!(conflict, FileError::Conflict { .. }));
    assert_eq!(fs::read(&destination).unwrap(), b"changed elsewhere");

    let current = futures::executor::block_on(service.version(destination.clone())).unwrap();
    let monitor = TransferMonitor::new();
    let updated = futures::executor::block_on(service.sync_file(
        local_edit,
        destination.clone(),
        Some(current),
        monitor.clone(),
    ))
    .unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"edited contents");
    assert_eq!(updated.size, Some(15));
    assert_eq!(monitor.snapshot().bytes_transferred, 15);
    assert!(monitor.is_finished());
}
