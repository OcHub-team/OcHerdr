use super::*;

#[test]
fn terminal_palette_follows_the_gui_light_and_dark_theme() {
    let family = ochub_ui::theme::ochub_family();
    let appearance = AppearanceSettings::default();
    let overlay = crate::theme_ansi::overlay_for(Some(&family));
    let light = terminal_palette_from_theme(family.light, false, overlay, &appearance);
    let dark = terminal_palette_from_theme(family.dark, true, overlay, &appearance);
    assert!(!light.dark);
    assert!(dark.dark);
    assert_eq!(light.background, family.light.bg.0);
    assert_eq!(light.foreground, family.light.text.0);
    assert_eq!(dark.background, family.dark.bg.0);
    assert_ne!(light.background, dark.background);
    assert_ne!(light.background, 0x1E1E1E);
    assert_ne!(light.signature(), dark.signature());
    assert_eq!(light.font_size, 13);
    assert!(light.font_features.is_empty());
    assert!(light.font_family.is_empty());
}

#[test]
fn terminal_font_settings_change_the_ghostty_signature() {
    let family = ochub_ui::theme::ochub_family();
    let default = AppearanceSettings::default();
    let menlo = AppearanceSettings {
        font: TerminalFontSettings {
            family: "Menlo".into(),
            size: 16.0,
            features: crate::config::values::no_ligature_features(),
            thicken: true,
            thicken_strength: 80,
            cell_width: CellWidthChoice::Tight.metric(),
            cell_height: CellHeightChoice::Relaxed.metric(),
        },
        window_padding_x: 2,
        ..AppearanceSettings::default()
    };
    let overlay = crate::theme_ansi::overlay_for(Some(&family));
    let left = terminal_palette_from_theme(family.light, false, overlay, &default);
    let right = terminal_palette_from_theme(family.light, false, overlay, &menlo);
    assert_eq!(right.font_family, "Menlo");
    assert_eq!(right.font_size, 16);
    assert_eq!(
        right.font_features,
        crate::config::values::no_ligature_features()
    );
    assert!(right.thicken);
    assert_eq!(right.thicken_strength, 80);
    assert_eq!(right.cell_width.as_deref(), Some("-10%"));
    assert_eq!(right.padding_x, 2);
    assert_ne!(left.signature(), right.signature());
}

#[test]
fn current_terminal_palette_applies_config_slot_overrides() {
    let probe = 0xc0ffee;
    let family = ochub_ui::theme::ochub_family();
    let overlay = crate::theme_ansi::overlay_for(Some(&family));
    let base = terminal_ansi(overlay, &family.dark, theme::is_dark());
    assert_ne!(
        base[3], probe,
        "fixture color must not already be the theme slot"
    );
    let mut appearance = AppearanceSettings::default();
    appearance.palette[3] = Some(probe);
    let palette = current_terminal_palette(&appearance);
    assert_eq!(palette.ansi[3], probe);
    assert_eq!(palette.ansi[0], base[0]);
}

#[test]
fn terminal_ansi_bright_slots_are_distinct_and_lighter_than_normal() {
    let ochub = ochub_ui::theme::ochub_family();
    let ember = ochub_ui::theme::ember_family();
    let custom = custom_theme_family("scarlet");
    let custom_overlay = crate::theme_ansi::overlay_for(Some(&custom));
    assert!(
        custom_overlay.colors(true).is_none(),
        "fixture must omit ansi so derivation is what we measure"
    );
    for (label, overlay, palette, dark) in [
        (
            "ochub-dark",
            crate::theme_ansi::overlay_for(Some(&ochub)),
            ochub.dark,
            true,
        ),
        (
            "ochub-light",
            crate::theme_ansi::overlay_for(Some(&ochub)),
            ochub.light,
            false,
        ),
        (
            "ember-dark",
            crate::theme_ansi::overlay_for(Some(&ember)),
            ember.dark,
            true,
        ),
        (
            "ember-light",
            crate::theme_ansi::overlay_for(Some(&ember)),
            ember.light,
            false,
        ),
        ("custom-dark", custom_overlay, custom.dark, true),
    ] {
        let ansi = terminal_ansi(overlay, &palette, dark);
        let unique: HashSet<u32> = ansi.iter().copied().collect();
        assert_eq!(unique.len(), 16, "{label}");
        for slot in 0..8 {
            assert_ne!(ansi[slot], ansi[slot + 8], "{label} slot {slot}");
            assert!(
                crate::theme_ansi::ansi_luma(ansi[slot + 8])
                    > crate::theme_ansi::ansi_luma(ansi[slot]),
                "{label} slot {slot}"
            );
        }
    }
}

#[test]
fn switching_theme_family_changes_the_terminal_ansi_palette() {
    let appearance = AppearanceSettings::default();
    let ochub = ochub_ui::theme::ochub_family();
    let ember = ochub_ui::theme::ember_family();
    let ochub_dark = terminal_palette_from_theme(
        ochub.dark,
        true,
        crate::theme_ansi::overlay_for(Some(&ochub)),
        &appearance,
    );
    let ember_dark = terminal_palette_from_theme(
        ember.dark,
        true,
        crate::theme_ansi::overlay_for(Some(&ember)),
        &appearance,
    );
    assert_ne!(ochub_dark.ansi, ember_dark.ansi);
    assert_eq!(ochub_dark.background, ochub.dark.bg.0);
    assert_eq!(ember_dark.background, ember.dark.bg.0);
    assert_eq!(ochub_dark.foreground, ochub.dark.text.0);
    assert_eq!(ember_dark.cursor, ember.dark.accent.0);
}

#[test]
fn a_valid_custom_theme_family_gets_its_own_ansi_palette() {
    let ochub = ochub_ui::theme::ochub_family();
    let custom = custom_theme_family("scarlet");
    let ochub_overlay = crate::theme_ansi::overlay_for(Some(&ochub));
    let custom_overlay = crate::theme_ansi::overlay_for(Some(&custom));
    assert!(
        custom_overlay.colors(true).is_none(),
        "fixture must omit ansi so derivation is what we measure"
    );
    let ochub_ansi = terminal_ansi(ochub_overlay, &ochub.dark, true);
    let custom_ansi = terminal_ansi(custom_overlay, &custom.dark, true);
    let derived = crate::theme_ansi::ansi_from_theme(&custom.dark, true);
    assert_eq!(ochub_ansi, ochub_overlay.colors(true).expect("ochub ansi"));
    assert_ne!(
        ochub_ansi,
        crate::theme_ansi::ansi_from_theme(&ochub.dark, true)
    );
    assert_eq!(custom_ansi, derived);
    assert_ne!(custom_ansi, ochub_ansi);
    let missing = terminal_ansi(crate::theme_ansi::overlay_for(None), &ochub.dark, true);
    assert_eq!(missing, ochub_ansi);
}

#[test]
fn explicit_ansi_is_used_instead_of_deriving_from_tokens() {
    let custom = custom_theme_family("scarlet");
    let derived = crate::theme_ansi::ansi_from_theme(&custom.dark, true);
    let explicit = [
        0x101010, 0xB00000, 0x00B000, 0xB0B000, 0x0000B0, 0xB000B0, 0x00B0B0, 0xB0B0B0, 0x404040,
        0xFF4040, 0x40FF40, 0xFFFF40, 0x4040FF, 0xFF40FF, 0x40FFFF, 0xFFFFFF,
    ];
    assert_ne!(
        explicit, derived,
        "fixture must differ from derivation or the test cannot catch a fallback"
    );
    let overlay = crate::theme_ansi::ThemeAnsi {
        dark: crate::theme_ansi::ThemeAnsiPalette {
            ansi: Some(explicit.map(theme::ThemeColor::new)),
        },
        ..crate::theme_ansi::ThemeAnsi::default()
    };
    assert_eq!(terminal_ansi(overlay, &custom.dark, true), explicit);
    assert_ne!(terminal_ansi(overlay, &custom.dark, true), derived);
}

fn custom_theme_family(id: &str) -> theme::ThemeFamily {
    let mut dark = theme::OCHUB_DARK;
    dark.red = theme::ThemeColor::new(0xE23D48);
    dark.green = theme::ThemeColor::new(0x2FBF71);
    dark.yellow = theme::ThemeColor::new(0xF0C400);
    dark.accent = theme::ThemeColor::new(0x3D8BFF);
    dark.mauve = theme::ThemeColor::new(0xC45CFF);
    dark.teal = theme::ThemeColor::new(0x1EC8B8);
    theme::ThemeFamily {
        schema_version: theme::THEME_SCHEMA_VERSION,
        id: id.into(),
        name: id.into(),
        author: String::new(),
        description: String::new(),
        light: theme::OCHUB_LIGHT,
        dark,
    }
}

#[test]
fn overlay_enter_confirms_and_escape_cancels() {
    let enter = KeyDownEvent {
        keystroke: ochub_ui::gpui::Keystroke {
            key: "enter".into(),
            key_char: Some("\n".into()),
            modifiers: ochub_ui::gpui::Modifiers::default(),
        },
        is_held: false,
        prefer_character_input: false,
    };
    let held = KeyDownEvent {
        is_held: true,
        ..enter.clone()
    };
    let escape = KeyDownEvent {
        keystroke: ochub_ui::gpui::Keystroke {
            key: "escape".into(),
            key_char: None,
            modifiers: ochub_ui::gpui::Modifiers::default(),
        },
        is_held: false,
        prefer_character_input: false,
    };
    assert_eq!(overlay_confirm_or_cancel(&enter), Some(true));
    assert_eq!(overlay_confirm_or_cancel(&escape), Some(false));
    assert_eq!(overlay_confirm_or_cancel(&held), None);
}

#[test]
fn tab_index_reads_plain_digit_and_named_keys() {
    assert_eq!(tab_index_from_keystroke("1", None), Some(1));
    assert_eq!(tab_index_from_keystroke("9", Some("9")), Some(9));
    assert_eq!(tab_index_from_keystroke("0", None), Some(0));
    assert_eq!(tab_index_from_keystroke("digit3", None), Some(3));
    assert_eq!(tab_index_from_keystroke("numpad8", None), Some(8));
    assert_eq!(tab_index_from_keystroke("t", None), None);
    assert_eq!(tab_index_from_keystroke("w", Some("w")), None);
    assert_eq!(tab_index_from_keystroke("１", None), Some(1));
}

#[test]
fn tab_shortcut_uses_visual_order_despite_stable_numbers_and_zero_for_last() {
    let tabs = [
        ocherdr_core::TabInfo {
            tab_id: "first".into(),
            workspace_id: "w".into(),
            number: 1,
            label: "one".into(),
            focused: true,
            pane_count: 1,
            agent_status: AgentStatus::Idle,
        },
        ocherdr_core::TabInfo {
            tab_id: "second".into(),
            workspace_id: "w".into(),
            number: 3,
            label: "two".into(),
            focused: false,
            pane_count: 1,
            agent_status: AgentStatus::Idle,
        },
        ocherdr_core::TabInfo {
            tab_id: "third".into(),
            workspace_id: "w".into(),
            number: 14,
            label: "three".into(),
            focused: false,
            pane_count: 1,
            agent_status: AgentStatus::Idle,
        },
    ];
    assert_eq!(
        tab_id_for_shortcut(tabs.iter(), 1).as_deref(),
        Some("first")
    );
    assert_eq!(
        tab_id_for_shortcut(tabs.iter(), 2).as_deref(),
        Some("second")
    );
    assert_eq!(
        tab_id_for_shortcut(tabs.iter(), 3).as_deref(),
        Some("third")
    );
    assert_eq!(
        tab_id_for_shortcut(tabs.iter(), 0).as_deref(),
        Some("third")
    );
    assert_eq!(tab_id_for_shortcut(tabs.iter(), 8).as_deref(), None);
}

#[test]
fn surface_rect_mapping_inverts_mouse_mapping() {
    let body = (100., 50., 800., 400.);
    let pixel_size = (1600, 800);
    let mouse = map_mouse_to_surface((500., 250.), body, pixel_size, 2.).unwrap();
    let rect =
        map_surface_rect_to_window((mouse.0, mouse.1, 10., 16.), body, pixel_size, 2.).unwrap();
    assert!((rect.0 - 500.).abs() < 0.02);
    assert!((rect.1 - 250.).abs() < 0.02);
    assert!((rect.2 - 10.).abs() < 0.02);
    assert!((rect.3 - 16.).abs() < 0.02);
}

#[test]
fn mouse_to_surface_fills_a_matching_retina_framebuffer() {
    let body = (100., 50., 800., 400.);
    let pixel_size = (1600, 800);
    assert_eq!(
        map_mouse_to_surface((100., 50.), body, pixel_size, 2.),
        Some((0., 0.))
    );
    let bottom_right = map_mouse_to_surface((900., 450.), body, pixel_size, 2.).unwrap();
    assert!((bottom_right.0 - 800.).abs() < 0.01);
    assert!((bottom_right.1 - 400.).abs() < 0.01);
    assert_eq!(map_mouse_to_surface((100., 50.), body, (0, 0), 2.), None);
}

#[test]
fn mouse_to_surface_accounts_for_contain_letterboxing() {
    let body = (0., 0., 1000., 400.);
    let mapped = map_mouse_to_surface((100., 0.), body, (1600, 800), 2.).unwrap();
    assert!(mapped.0.abs() < 0.01);
    assert!(mapped.1.abs() < 0.01);
    let mapped = map_mouse_to_surface((900., 400.), body, (1600, 800), 2.).unwrap();
    assert!((mapped.0 - 800.).abs() < 0.01);
    assert!((mapped.1 - 400.).abs() < 0.01);
}

#[test]
fn mouse_to_surface_uses_measured_window_bounds_without_chrome_offsets() {
    let body = (300., 80., 800., 400.);
    assert_eq!(
        map_mouse_to_surface((300., 80.), body, (1600, 800), 2.),
        Some((0., 0.))
    );
    let bottom_right = map_mouse_to_surface((1100., 480.), body, (1600, 800), 2.).unwrap();
    assert!((bottom_right.0 - 800.).abs() < 0.01);
    assert!((bottom_right.1 - 400.).abs() < 0.01);
}

#[test]
fn pointer_along_split_uses_the_layout_area_origin() {
    let area = LayoutRect {
        x: 10,
        y: 20,
        width: 80,
        height: 40,
    };
    let surface = (100., 50., 400., 200.);
    assert_eq!(
        pointer_along_split(SplitDirection::Right, area, surface, (100., 50.)),
        Some(10.)
    );
    assert_eq!(
        pointer_along_split(SplitDirection::Right, area, surface, (300., 50.)),
        Some(50.)
    );
    assert_eq!(
        pointer_along_split(SplitDirection::Down, area, surface, (100., 150.)),
        Some(40.)
    );
}

fn split_area() -> LayoutRect {
    LayoutRect {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    }
}

fn layout_snapshot(splits: &[(&str, f32)], panes: &[(&str, LayoutRect)]) -> HierarchySnapshot {
    let area = split_area();
    HierarchySnapshot {
        panes: panes.iter().map(|(id, _)| test_pane(id, "t1")).collect(),
        layouts: vec![ocherdr_core::PaneLayout {
            workspace_id: "w".into(),
            tab_id: "t1".into(),
            zoomed: false,
            area,
            focused_pane_id: panes[0].0.into(),
            panes: panes
                .iter()
                .map(|(id, rect)| ocherdr_core::LayoutPane {
                    pane_id: (*id).into(),
                    focused: false,
                    rect: *rect,
                })
                .collect(),
            splits: splits
                .iter()
                .map(|(id, ratio)| LayoutSplit {
                    id: (*id).into(),
                    direction: SplitDirection::Right,
                    ratio: *ratio,
                    rect: area,
                })
                .collect(),
        }],
        ..Default::default()
    }
}

fn split_drag_on(snapshot: &HierarchySnapshot) -> SplitDrag {
    split_drag_at(snapshot, 0)
}

fn split_drag_at(snapshot: &HierarchySnapshot, split_index: usize) -> SplitDrag {
    let layout = &snapshot.layouts[0];
    let split = &layout.splits[split_index];
    SplitDrag {
        workspace_id: layout.workspace_id.clone(),
        tab_id: layout.tab_id.clone(),
        path: split.path().expect("test split ids encode a path"),
        layout: split_layout_fingerprint(layout),
        direction: split.direction,
        rect: split.rect,
        grab_offset: 0.,
        preview_ratio: split.ratio,
        start_ratio: split.ratio,
    }
}

fn nested_layout(root_ratio: f32) -> HierarchySnapshot {
    let area = split_area();
    let left_w = (f32::from(area.width) * root_ratio).round() as u16;
    let right_w = area.width - left_w;
    let nested = LayoutRect {
        x: area.x,
        y: area.y,
        width: left_w,
        height: area.height,
    };
    let top_h = nested.height / 2;
    HierarchySnapshot {
        panes: ["p-top", "p-bot", "p-right"]
            .into_iter()
            .map(|id| test_pane(id, "t1"))
            .collect(),
        layouts: vec![ocherdr_core::PaneLayout {
            workspace_id: "w".into(),
            tab_id: "t1".into(),
            zoomed: false,
            area,
            focused_pane_id: "p-top".into(),
            panes: vec![
                ocherdr_core::LayoutPane {
                    pane_id: "p-top".into(),
                    focused: false,
                    rect: LayoutRect {
                        x: nested.x,
                        y: nested.y,
                        width: left_w,
                        height: top_h,
                    },
                },
                ocherdr_core::LayoutPane {
                    pane_id: "p-bot".into(),
                    focused: false,
                    rect: LayoutRect {
                        x: nested.x,
                        y: nested.y + top_h,
                        width: left_w,
                        height: nested.height - top_h,
                    },
                },
                ocherdr_core::LayoutPane {
                    pane_id: "p-right".into(),
                    focused: false,
                    rect: LayoutRect {
                        x: nested.x + left_w,
                        y: area.y,
                        width: right_w,
                        height: area.height,
                    },
                },
            ],
            splits: vec![
                LayoutSplit {
                    id: "split_0_root".into(),
                    direction: SplitDirection::Right,
                    ratio: root_ratio,
                    rect: area,
                },
                LayoutSplit {
                    id: "split_1_0".into(),
                    direction: SplitDirection::Down,
                    ratio: 0.5,
                    rect: nested,
                },
            ],
        }],
        ..Default::default()
    }
}

#[test]
fn reconciling_a_split_drag_stays_split_when_only_the_ratio_changes() {
    let left = LayoutRect {
        x: 0,
        y: 0,
        width: 50,
        height: 50,
    };
    let right = LayoutRect {
        x: 50,
        y: 0,
        width: 50,
        height: 50,
    };
    let before = layout_snapshot(
        &[("split_0_root", 0.5)],
        &[("p-left", left), ("p-right", right)],
    );
    let after = layout_snapshot(
        &[("split_0_root", 0.7)],
        &[
            (
                "p-left",
                LayoutRect {
                    x: 0,
                    y: 0,
                    width: 70,
                    height: 50,
                },
            ),
            (
                "p-right",
                LayoutRect {
                    x: 70,
                    y: 0,
                    width: 30,
                    height: 50,
                },
            ),
        ],
    );
    assert_eq!(
        split_layout_fingerprint(&before.layouts[0]),
        split_layout_fingerprint(&after.layouts[0])
    );
    assert!(matches!(
        reconcile_split_drag_state(split_drag_on(&before), Some(&after)),
        SurfaceDrag::Split(_)
    ));
}

#[test]
fn reconciling_a_split_drag_goes_idle_when_a_pane_is_replaced() {
    let left = LayoutRect {
        x: 0,
        y: 0,
        width: 50,
        height: 50,
    };
    let right = LayoutRect {
        x: 50,
        y: 0,
        width: 50,
        height: 50,
    };
    let before = layout_snapshot(
        &[("split_0_root", 0.5)],
        &[("p-left", left), ("p-right", right)],
    );
    let after = layout_snapshot(
        &[("split_0_root", 0.5)],
        &[("p-left", left), ("p-other", right)],
    );
    assert!(matches!(
        reconcile_split_drag_state(split_drag_on(&before), Some(&after)),
        SurfaceDrag::Idle
    ));
}

#[test]
fn reconciling_a_split_drag_goes_idle_when_a_pane_is_added_before_layout_updated() {
    let left = LayoutRect {
        x: 0,
        y: 0,
        width: 50,
        height: 50,
    };
    let right = LayoutRect {
        x: 50,
        y: 0,
        width: 50,
        height: 50,
    };
    let before = layout_snapshot(
        &[("split_0_root", 0.5)],
        &[("p-left", left), ("p-right", right)],
    );
    let mut after = before.clone();
    after.panes.push(test_pane("p-new", "t1"));
    assert!(matches!(
        reconcile_split_drag_state(split_drag_on(&before), Some(&after)),
        SurfaceDrag::Idle
    ));
}

#[test]
fn selecting_a_pane_voids_a_split_drag_only_when_leaving_its_tab_or_workspace() {
    let left = LayoutRect {
        x: 0,
        y: 0,
        width: 50,
        height: 50,
    };
    let right = LayoutRect {
        x: 50,
        y: 0,
        width: 50,
        height: 50,
    };
    let drag = split_drag_on(&layout_snapshot(
        &[("split_0_root", 0.5)],
        &[("p-left", left), ("p-right", right)],
    ));
    assert!(!split_drag_voided_by_pane(&drag, Some("w"), Some("t1")));
    assert!(split_drag_voided_by_pane(&drag, Some("w"), Some("t-other")));
    assert!(split_drag_voided_by_pane(
        &drag,
        Some("w-other"),
        Some("t1")
    ));
    assert!(split_drag_voided_by_pane(&drag, None, None));
}

#[test]
fn reconciling_a_split_drag_stays_split_when_an_ancestor_ratio_moves_a_nested_rect() {
    let before = nested_layout(0.5);
    let after = nested_layout(0.7);
    assert_ne!(
        before.layouts[0].splits[1].rect,
        after.layouts[0].splits[1].rect
    );
    assert_eq!(
        split_layout_fingerprint(&before.layouts[0]),
        split_layout_fingerprint(&after.layouts[0])
    );
    assert!(matches!(
        reconcile_split_drag_state(split_drag_at(&before, 1), Some(&after)),
        SurfaceDrag::Split(_)
    ));
}

fn agent_panel(pane_id: &str) -> Overlay {
    Overlay::AgentPanel {
        pane_id: pane_id.into(),
    }
}

fn snapshot_with_agent(pane_id: &str, agent: Option<&str>) -> HierarchySnapshot {
    let mut snapshot = two_tab_snapshot();
    snapshot.panes[0].pane_id = pane_id.into();
    snapshot.panes[0].agent = agent.map(str::to_owned);
    snapshot.panes[0].display_agent = agent.map(str::to_owned);
    snapshot
}

#[test]
fn agent_panel_closes_when_the_pane_or_agent_is_gone() {
    let overlay = agent_panel("p-a");
    assert!(!agent_panel_target_missing(
        &overlay,
        Some(&snapshot_with_agent("p-a", Some("grok"))),
    ));
    assert!(agent_panel_target_missing(
        &overlay,
        Some(&snapshot_with_agent("p-a", None)),
    ));
    assert!(agent_panel_target_missing(
        &overlay,
        Some(&snapshot_with_agent("p-b", Some("grok"))),
    ));
    assert!(agent_panel_target_missing(&overlay, None));
    assert!(!agent_panel_target_missing(&Overlay::Appearance, None));
}

#[test]
fn agent_prompt_preserves_the_user_text_and_rejects_only_exact_empty() {
    assert_eq!(
        agent_prompt_text_to_send("  hello  "),
        Some("  hello  ".into())
    );
    assert_eq!(agent_prompt_text_to_send("   "), Some("   ".into()));
    assert_eq!(agent_prompt_text_to_send(""), None);
}

#[test]
fn agent_read_parses_text_and_truncated() {
    let value = json!({
        "type": "pane_read",
        "read": { "text": "hello\n", "truncated": true }
    });
    assert_eq!(
        parse_agent_read_result(&value).unwrap(),
        ("hello\n".into(), true)
    );
    assert!(parse_agent_read_result(&json!({ "ok": true })).is_err());
    assert!(parse_agent_read_result(&json!({ "read": { "text": "hello" } })).is_err());
    assert!(
        parse_agent_read_result(&json!({ "read": { "text": "hello", "truncated": "false" } }))
            .is_err()
    );
}

#[test]
fn agent_info_parses_the_custom_name_instead_of_display_metadata() {
    let agent = parse_agent_info_result(json!({
        "agent": {
            "pane_id": "p-a",
            "name": "reviewer",
            "agent": "claude",
            "display_agent": "Claude Code"
        }
    }))
    .unwrap();
    assert_eq!(agent.pane_id, "p-a");
    assert_eq!(agent.name.as_deref(), Some("reviewer"));
}

#[test]
fn agent_panel_refreshes_output_from_that_pane_status_events() {
    let overlay = agent_panel("p-a");
    let status = HerdrEvent::PaneAgentStatusChanged {
        pane_id: "p-a".into(),
        workspace_id: "w".into(),
        agent_status: AgentStatus::Working,
        agent: Some("grok".into()),
        title: None,
        display_agent: Some("grok".into()),
        state_labels: HashMap::new(),
    };
    let other = HerdrEvent::PaneAgentStatusChanged {
        pane_id: "p-b".into(),
        workspace_id: "w".into(),
        agent_status: AgentStatus::Done,
        agent: Some("grok".into()),
        title: None,
        display_agent: Some("grok".into()),
        state_labels: HashMap::new(),
    };
    assert!(agent_panel_refresh_from_batch(
        &overlay,
        Some(&[Ok(status.clone())]),
    ));
    assert!(!agent_panel_refresh_from_batch(
        &overlay,
        Some(&[Ok(other)]),
    ));
    assert!(!agent_panel_refresh_from_batch(
        &Overlay::Appearance,
        Some(&[Ok(status)])
    ));
}

fn workspace_snapshot(ids: &[&str], labels: &[&str]) -> HierarchySnapshot {
    HierarchySnapshot {
        workspaces: ids
            .iter()
            .zip(labels)
            .map(|(id, label)| {
                let mut workspace = sample_workspace(id, None);
                workspace.label = (*label).into();
                workspace
            })
            .collect(),
        ..Default::default()
    }
}

fn workspace_reorder_on(snapshot: &HierarchySnapshot, source: usize) -> ReorderDrag {
    ReorderDrag {
        list: ReorderList::Workspaces,
        source_index: source,
        order: snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.clone())
            .collect(),
        previous_hover: ReorderHover::AfterLast,
        hover: ReorderHover::AfterLast,
        origin: (0., 0.),
        pointer: (0., 0.),
        grab_offset: (0., 0.),
        source_rect: (0., 0., 0., 0.),
    }
}

#[test]
fn reconciling_a_reorder_drag_goes_idle_when_list_order_changes() {
    let before = workspace_snapshot(&["w1", "w2", "w3"], &["a", "b", "c"]);
    let after = workspace_snapshot(&["w2", "w1", "w3"], &["b", "a", "c"]);
    assert!(matches!(
        reconcile_reorder_drag_state(workspace_reorder_on(&before, 0), Some(&after)),
        SurfaceDrag::Idle
    ));
}

#[test]
fn reconciling_a_reorder_drag_stays_when_only_a_label_changes() {
    let before = workspace_snapshot(&["w1", "w2"], &["a", "b"]);
    let after = workspace_snapshot(&["w1", "w2"], &["a", "renamed"]);
    assert!(matches!(
        reconcile_reorder_drag_state(workspace_reorder_on(&before, 0), Some(&after)),
        SurfaceDrag::Reorder(_)
    ));
}
