use super::*;
use gpui::{ExternalPaths, FileDropEvent};
use ocherdr_core::WorkspaceWorktreeInfo;
use ocherdr_files::{BackendKind, BackendSpec, EntryKind, FileEntry, FileService};

#[gpui::test]
#[cfg(target_os = "macos")]
fn focused_file_path_input_keeps_text_out_of_the_terminal(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_with_live_events(three_tab_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);

    view.update(cx, |this, cx| {
        this.file_panel.open = true;
        this.file_panel.service = Some(FileService::new(BackendSpec::Local).unwrap());
        this.file_panel.backend_kind = Some(BackendKind::Local);
        this.file_panel.root = Some(std::path::PathBuf::from("/tmp"));
        cx.notify();
    });
    cx.run_until_parked();
    view.update_in(cx, |this, window, cx| {
        this.open_file_panel_address(window, cx);
    });
    cx.run_until_parked();
    let initial_path = view.read_with(cx, |this, cx| {
        this.file_path_input.read(cx).content().to_owned()
    });

    cx.simulate_input("/typed-path");
    cx.run_until_parked();
    view.update(cx, |this, _| this.pump_terminal_input());

    view.read_with(cx, |this, cx| {
        assert_eq!(
            this.file_path_input.read(cx).content(),
            format!("{initial_path}/typed-path")
        );
    });
    assert!(
        fake.terminal_inputs("p-a").is_empty(),
        "typing in the file path input must never reach the terminal"
    );
}

#[gpui::test]
fn file_panel_tree_scrolls_when_rows_overflow(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entries = (0..48)
        .map(|index| FileEntry {
            path: root.join(format!("file-{index:02}.txt")),
            name: format!("file-{index:02}.txt"),
            kind: EntryKind::File,
            size: Some(1),
            modified: None,
            permissions: Some(0o644),
            hidden: false,
        })
        .collect();

    let (view, cx) = open_view(cx);
    view.update(cx, |this, cx| {
        let mut snapshot = three_tab_snapshot();
        snapshot.workspaces[0].worktree = Some(WorkspaceWorktreeInfo {
            repo_key: "fixture".into(),
            repo_name: "fixture".into(),
            repo_root: root.to_string_lossy().into_owned(),
            checkout_path: root.to_string_lossy().into_owned(),
            is_linked_worktree: false,
        });
        this.snapshot = Some(snapshot);
        this.selection = Selection {
            connection_id: "local".into(),
            workspace_id: Some("w".into()),
            tab_id: Some("t-a".into()),
            ..Default::default()
        };
        this.file_panel.open = true;
        this.file_panel.source = Some(crate::FilePanelSource {
            profile_id: "local".into(),
            suggested_root: root.clone(),
        });
        this.file_panel.service = Some(FileService::new(BackendSpec::Local).unwrap());
        this.file_panel.backend_kind = Some(BackendKind::Local);
        this.file_panel.root = Some(root.clone());
        this.file_panel.expanded.insert(root.clone());
        this.file_panel.children.insert(root, entries);
        cx.notify();
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(320.)));
    cx.run_until_parked();

    let tree_bounds = cx.debug_bounds("file-tree-scroll").expect("file tree");
    let scroll = view.read_with(cx, |this, _| this.file_panel.tree_scroll.clone());
    assert!(
        scroll.max_offset().y > gpui::px(0.),
        "an overflowing file tree must expose vertical scroll range"
    );
    let before = scroll.offset().y;
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: tree_bounds.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-120.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    assert!(
        scroll.offset().y < before,
        "a wheel gesture over the file tree must reveal later rows"
    );
}

#[gpui::test]
fn file_panel_docks_wide_and_overlays_the_terminal_when_narrow(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let file_path = root.join("notes.txt");
    std::fs::write(&file_path, b"hello\nworld\n").unwrap();
    let nested = root.join("src");
    std::fs::create_dir(&nested).unwrap();
    let upload_source = tempfile::tempdir().unwrap();
    let dropped_file = upload_source.path().join("dropped.txt");
    std::fs::write(&dropped_file, b"dragged contents").unwrap();

    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        let mut snapshot = three_tab_snapshot();
        snapshot.workspaces[0].worktree = Some(WorkspaceWorktreeInfo {
            repo_key: "fixture".into(),
            repo_name: "fixture".into(),
            repo_root: root.to_string_lossy().into_owned(),
            checkout_path: root.to_string_lossy().into_owned(),
            is_linked_worktree: false,
        });
        this.snapshot = Some(snapshot);
        this.selection = Selection {
            connection_id: "local".into(),
            workspace_id: Some("w".into()),
            tab_id: Some("t-a".into()),
            ..Default::default()
        };
        let source = crate::FilePanelSource {
            profile_id: "local".into(),
            suggested_root: root.clone(),
        };
        let service = FileService::new(BackendSpec::Local).unwrap();
        this.file_panel.open = true;
        this.file_panel.source = Some(source);
        this.file_panel.service = Some(service);
        this.file_panel.backend_kind = Some(BackendKind::Local);
        this.file_panel.root = Some(root.clone());
        this.file_panel.expanded.insert(root.clone());
        let entry = FileEntry {
            path: file_path.clone(),
            name: "notes.txt".into(),
            kind: EntryKind::File,
            size: Some(12),
            modified: None,
            permissions: Some(0o644),
            hidden: false,
        };
        this.file_panel
            .children
            .insert(root.clone(), vec![entry.clone()]);
        this.file_panel.selected = Some(entry);
        cx.notify();
    });

    cx.simulate_resize(gpui::size(gpui::px(1200.), gpui::px(700.)));
    cx.run_until_parked();
    let wide_tab_bar = cx.debug_bounds("tab-bar").expect("tab bar");
    let wide_panel = cx.debug_bounds("file-panel").expect("file panel");
    assert_eq!(
        wide_panel.size.width,
        gpui::px(crate::FILE_PANEL_DEFAULT_WIDTH)
    );
    assert_eq!(
        wide_tab_bar.origin.x + wide_tab_bar.size.width,
        wide_panel.origin.x + wide_panel.size.width,
        "the top bar spans the whole content area while the file panel is open"
    );
    assert_eq!(
        wide_tab_bar.origin.y + wide_tab_bar.size.height,
        wide_panel.origin.y,
        "the file panel starts below the stable top bar"
    );
    let toolbar = cx
        .debug_bounds("file-panel-toolbar")
        .expect("file panel toolbar");
    assert_eq!(
        toolbar.origin.y, wide_panel.origin.y,
        "the redundant file-panel title must not reserve vertical space"
    );
    view.update(cx, |this, cx| {
        this.file_panel.open = false;
        cx.notify();
    });
    cx.run_until_parked();
    let closed_tab_bar = cx.debug_bounds("tab-bar").expect("closed tab bar");
    assert_eq!(closed_tab_bar, wide_tab_bar);
    assert!(cx.debug_bounds("file-panel").is_none());
    view.update(cx, |this, cx| {
        this.file_panel.open = true;
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(
        cx.debug_bounds("tab-bar").expect("reopened tab bar"),
        wide_tab_bar,
        "opening or closing the file panel must not reflow the top bar"
    );
    let file_tree = cx.debug_bounds("file-tree-scroll").expect("file tree");
    assert_eq!(
        file_tree.origin.y + file_tree.size.height,
        wide_panel.origin.y + wide_panel.size.height,
        "the file tree reaches the bottom without a persistent status subtitle"
    );
    assert!(cx.debug_bounds("file-selected-actions").is_none());
    assert!(
        cx.debug_bounds("file-panel-pin").is_none(),
        "the title bar no longer exposes pinning"
    );
    assert!(
        cx.debug_bounds("close-file-panel").is_none(),
        "the global file toggle is the only close control"
    );
    assert!(cx.debug_bounds("file-transfers").is_some());

    let file_row = cx.debug_bounds("file-row-0").expect("file row");
    cx.simulate_mouse_down(
        file_row.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(matches!(
            &this.overlay,
            crate::Overlay::FileContextMenu(menu) if menu.entry.name == "notes.txt"
        ));
    });
    assert!(cx.debug_bounds("file-menu-open").is_some());
    assert!(cx.debug_bounds("file-menu-choose-editor").is_some());
    assert!(cx.debug_bounds("file-menu-download").is_some());

    view.update(cx, |this, cx| this.close_context_menu(cx));
    cx.run_until_parked();

    let transfers = cx
        .debug_bounds("file-transfers")
        .expect("transfer history toggle");
    cx.simulate_click(transfers.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("file-transfer-drawer").is_some());
    cx.simulate_click(transfers.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("file-transfer-drawer").is_none());

    let drop_position = cx
        .debug_bounds("file-tree-scroll")
        .expect("file drop surface")
        .center();
    cx.simulate_event(FileDropEvent::Entered {
        position: drop_position,
        paths: ExternalPaths([dropped_file.clone()].into_iter().collect()),
    });
    cx.simulate_event(FileDropEvent::Submit {
        position: drop_position,
    });
    for _ in 0..20 {
        cx.run_until_parked();
        if root.join("dropped.txt").exists() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(
        std::fs::read(root.join("dropped.txt")).unwrap(),
        b"dragged contents"
    );
    view.read_with(cx, |this, _| {
        assert!(this.file_panel.transfers.iter().any(|transfer| {
            transfer.name == "dropped.txt" && transfer.state == crate::FileTransferState::Completed
        }));
    });

    let edit_address = cx
        .debug_bounds("file-address-edit")
        .expect("editable file-panel path");
    assert!(edit_address.size.width > gpui::px(0.));
    let parent = root.parent().unwrap().to_path_buf();
    let up = cx.debug_bounds("file-up").expect("parent-directory button");
    let generation = view.read_with(cx, |this, _| this.file_panel.generation);
    cx.simulate_click(up.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.file_panel.generation > generation,
            "the parent-directory button must navigate up"
        );
    });
    view.update(cx, |this, cx| {
        this.file_panel.root_task = None;
        this.file_panel.root = Some(parent.clone());
        this.file_panel.expanded.insert(parent.clone());
        cx.notify();
    });
    cx.run_until_parked();

    let root_crumb = cx
        .debug_bounds("file-crumb-0")
        .expect("root breadcrumb segment");
    let generation = view.read_with(cx, |this, _| this.file_panel.generation);
    cx.simulate_click(root_crumb.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.file_panel.generation > generation,
            "a breadcrumb click must start navigation"
        );
    });
    view.update(cx, |this, cx| {
        this.file_panel.root_task = None;
        this.file_panel.root = Some(std::path::PathBuf::from("/"));
        cx.notify();
    });
    cx.run_until_parked();

    let edit_address = cx
        .debug_bounds("file-address-edit")
        .expect("editable file-panel path");
    cx.simulate_click(edit_address.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    view.read_with(cx, |this, _| {
        assert!(
            this.file_panel.address_editing,
            "the path edit button must enter address-editing mode"
        );
    });
    view.update_in(cx, |this, window, cx| {
        this.cancel_file_panel_address(window, cx)
    });
    cx.run_until_parked();

    view.update_in(cx, |this, window, cx| {
        let mut modifiers = app_primary_modifiers();
        modifiers.shift = !cfg!(target_os = "macos");
        let event = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers,
                key: "l".into(),
                key_char: Some("l".into()),
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(this.handle_app_shortcut(&event, window, cx));
    });
    cx.run_until_parked();
    view.update(cx, |this, cx| {
        assert!(this.file_panel.address_editing);
        this.file_path_input
            .update(cx, |input, cx| input.set_content("src", cx));
    });
    view.update_in(cx, |this, window, cx| {
        this.submit_file_panel_address(window, cx)
    });
    view.update(cx, |this, cx| {
        assert!(this.file_panel.address_task.is_some());
        this.file_panel.address_task = None;
        this.file_panel.address_editing = false;
        cx.notify();
    });

    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(700.)));
    cx.run_until_parked();
    let narrow_tab_bar = cx.debug_bounds("tab-bar").expect("tab bar");
    let narrow_panel = cx.debug_bounds("file-panel").expect("file panel");
    let main_right = narrow_tab_bar.origin.x + narrow_tab_bar.size.width;
    let panel_right = narrow_panel.origin.x + narrow_panel.size.width;
    assert_eq!(main_right, panel_right);
    assert_eq!(
        narrow_tab_bar.origin.y + narrow_tab_bar.size.height,
        narrow_panel.origin.y
    );
    assert!(
        narrow_panel.origin.x < main_right,
        "a narrow window overlays the terminal instead of resizing it"
    );
}

#[gpui::test]
fn remote_editor_save_is_debounced_and_uploaded(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let remote = temp.path().join("remote.txt");
    let local = temp.path().join("editor-copy.txt");
    std::fs::write(&remote, b"original").unwrap();
    std::fs::write(&local, b"original").unwrap();
    let service = FileService::new(BackendSpec::Local).unwrap();
    let expected = futures::executor::block_on(service.version(remote.clone())).unwrap();
    let initial_metadata = std::fs::metadata(&local).unwrap();
    let initial_revision = crate::LocalFileRevision {
        len: initial_metadata.len(),
        modified_nanos: initial_metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    };
    std::fs::write(&local, b"saved in editor").unwrap();
    let saved_metadata = std::fs::metadata(&local).unwrap();
    let saved_revision = crate::LocalFileRevision {
        len: saved_metadata.len(),
        modified_nanos: saved_metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    };

    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    view.update(cx, |this, cx| {
        this.file_panel.editor_sessions.insert(
            1,
            crate::RemoteEditSession {
                name: "remote.txt".into(),
                remote_path: remote.clone(),
                local_path: local.clone(),
                expected_remote: expected,
                synced_revision: initial_revision,
                pending_revision: Some(saved_revision),
                pending_since: Some(Instant::now() - Duration::from_secs(1)),
                syncing: false,
                conflict: false,
            },
        );
        this.watch_remote_editor(1, service.clone(), cx);
    });

    cx.executor().advance_clock(Duration::from_millis(350));
    for _ in 0..20 {
        cx.run_until_parked();
        if std::fs::read(&remote).is_ok_and(|contents| contents == b"saved in editor") {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&remote).unwrap(), b"saved in editor");
    view.read_with(cx, |this, _| {
        let session = this.file_panel.editor_sessions.get(&1).unwrap();
        assert_eq!(session.synced_revision, saved_revision);
        assert!(!session.syncing);
        assert!(!session.conflict);
        assert!(this.file_panel.transfers.iter().any(|transfer| {
            transfer.kind == crate::FileTransferKind::EditorSync
                && transfer.state == crate::FileTransferState::Completed
        }));
    });
}
