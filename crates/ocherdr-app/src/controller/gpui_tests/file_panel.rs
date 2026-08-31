use super::*;
use ocherdr_core::WorkspaceWorktreeInfo;
use ocherdr_files::{BackendKind, BackendSpec, EntryKind, FileEntry, FileService};

#[gpui::test]
fn file_panel_docks_wide_and_overlays_the_terminal_when_narrow(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let file_path = root.join("notes.txt");
    std::fs::write(&file_path, b"hello\nworld\n").unwrap();
    let nested = root.join("src");
    std::fs::create_dir(&nested).unwrap();

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
    let wide_main = cx.debug_bounds("tab-bar").expect("tab bar");
    let wide_panel = cx.debug_bounds("file-panel").expect("file panel");
    assert_eq!(
        wide_panel.size.width,
        gpui::px(crate::FILE_PANEL_DEFAULT_WIDTH)
    );
    assert_eq!(
        wide_main.origin.x + wide_main.size.width,
        wide_panel.origin.x,
        "a wide window reserves layout width for the docked panel"
    );
    let file_tree = cx.debug_bounds("file-tree-scroll").expect("file tree");
    assert_eq!(
        file_tree.origin.y + file_tree.size.height,
        wide_panel.origin.y + wide_panel.size.height,
        "the file tree reaches the bottom without a persistent status subtitle"
    );
    assert!(cx.debug_bounds("file-selected-actions").is_none());

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

    view.update(cx, |this, cx| this.close_context_menu(cx));
    cx.run_until_parked();

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
        let event = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    platform: true,
                    ..Default::default()
                },
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
    let narrow_main = cx.debug_bounds("tab-bar").expect("tab bar");
    let narrow_panel = cx.debug_bounds("file-panel").expect("file panel");
    let main_right = narrow_main.origin.x + narrow_main.size.width;
    let panel_right = narrow_panel.origin.x + narrow_panel.size.width;
    assert_eq!(main_right, panel_right);
    assert!(
        narrow_panel.origin.x < main_right,
        "a narrow window overlays the terminal instead of resizing it"
    );
}
