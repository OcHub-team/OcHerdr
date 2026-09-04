use super::*;

#[gpui::test]
fn terminal_paints_reuse_chrome_but_model_changes_invalidate_it(cx: &mut TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_path_buf();
    let (view, cx) = open_view(cx);
    view.update(cx, |this, cx| {
        this.headless_terminals = true;
        let mut snapshot = two_pane_snapshot();
        let pane = snapshot
            .panes
            .iter_mut()
            .find(|pane| pane.pane_id == "p-left")
            .unwrap();
        pane.cwd = Some(root.to_string_lossy().into_owned());
        pane.foreground_cwd = None;
        this.snapshot = Some(snapshot);
        this.selection.workspace_id = Some("w".into());
        this.selection.tab_id = Some("t-a".into());
        this.selection.pane_id = Some("p-left".into());
        this.file_panel.open = true;
        // This is a paint benchmark, not an async directory-listing test.
        // Supply an already-loaded empty listing so no real worker can wake a
        // test executor after the benchmark has finished.
        this.file_panel.source = Some(crate::FilePanelSource {
            profile_id: this.current_profile().id().to_owned(),
            suggested_root: root.clone(),
        });
        this.file_panel.service =
            Some(ocherdr_files::FileService::new(ocherdr_files::BackendSpec::Local).unwrap());
        this.file_panel.root = Some(root.clone());
        this.file_panel.children.insert(root.clone(), Vec::new());
        cx.notify();
    });
    // Settle initial layout measurement and subscription installation.
    for _ in 0..3 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let before = view.read_with(cx, |this, cx| this.render_cache.render_counts(cx));
    assert!(
        before.iter().all(|count| *count > 0),
        "all chrome was painted: {before:?}"
    );
    // Reproduce the previous frame path (root notification) for a measured
    // baseline using the same window, layout and number of paints.
    for _ in 0..120 {
        view.update(cx, |_, cx| cx.notify());
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let baseline = view.read_with(cx, |this, cx| this.render_cache.render_counts(cx));
    assert!(baseline.iter().zip(before).all(|(a, b)| *a - b == 120));
    for _ in 0..120 {
        view.update(cx, |this, cx| this.render_cache.notify_terminal(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let after = view.read_with(cx, |this, cx| this.render_cache.render_counts(cx));
    assert_eq!(
        baseline, after,
        "120 terminal-only paints must not rebuild chrome"
    );
    // The ordinary notification path still updates cached labels, selection,
    // toolbar state and file listing. It must never get a stale paint cache.
    view.update(cx, |_, cx| cx.notify());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let changed = view.read_with(cx, |this, cx| this.render_cache.render_counts(cx));
    assert!(changed.iter().zip(after).all(|(a, b)| *a > b));
    eprintln!(
        "120 paints: old notification rebuilt each chrome 120 times, new notification 0 times; model invalidation {:?}",
        changed
    );
}

#[gpui::test]
fn retina_wheel_handler_converts_logical_pixels_before_sending(cx: &mut TestAppContext) {
    let fake = FakeHerdr::snapshot_with_live_events(two_pane_snapshot());
    let (view, cx) = open_view(cx);
    cx.executor().allow_parking();
    connect_view_to_fake_and_resync(&view, &fake, cx);
    view.update_in(cx, |this, window, cx| {
        this.sync_measured_pane_body(
            "p-left",
            gpui::Bounds {
                origin: gpui::point(gpui::px(0.), gpui::px(0.)),
                size: gpui::size(gpui::px(400.), gpui::px(480.)),
            },
            window,
            cx,
        );
        this.ensure_session_terminals(cx);
        let runtime = this.pane_mut("p-left").unwrap();
        runtime.size.1 = 30;
        runtime.body_bounds.3 = 480.;
        runtime.pixel_size.1 = 960;
        runtime.scroll_px = 0.;
        this.scroll_pane(
            "p-left",
            &gpui::ScrollWheelEvent {
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(24.))),
                ..Default::default()
            },
            cx,
        );
        assert_eq!(this.pane("p-left").unwrap().scroll_px, 8.);
    });
}
