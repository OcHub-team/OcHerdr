use super::{
    StatusIndicatorKind, TabStripPress, pane_fractions, status_indicator_kind,
    status_indicator_symbol, tab_key_equivalent, tab_strip_press,
};
use ocherdr_core::{AgentStatus, LayoutPane, LayoutRect, PaneLayout};

#[test]
fn agent_dots_match_herdrs_five_state_shapes() {
    assert_eq!(
        status_indicator_kind(AgentStatus::Blocked),
        StatusIndicatorKind::Filled
    );
    assert_eq!(
        status_indicator_kind(AgentStatus::Working),
        StatusIndicatorKind::Filled
    );
    assert_eq!(
        status_indicator_kind(AgentStatus::Done),
        StatusIndicatorKind::Filled
    );
    assert_eq!(
        status_indicator_kind(AgentStatus::Idle),
        StatusIndicatorKind::Ring
    );
    assert_eq!(
        status_indicator_kind(AgentStatus::Unknown),
        StatusIndicatorKind::Point
    );
}

#[test]
fn distinct_agent_symbols_match_herdrs_five_states() {
    assert_eq!(status_indicator_symbol(AgentStatus::Blocked), "×");
    assert_eq!(status_indicator_symbol(AgentStatus::Working), "◐");
    assert_eq!(status_indicator_symbol(AgentStatus::Done), "✓");
    assert_eq!(status_indicator_symbol(AgentStatus::Idle), "○");
    assert_eq!(status_indicator_symbol(AgentStatus::Unknown), "·");
}

#[test]
fn empty_strip_press_moves_the_window_and_a_double_click_zooms() {
    assert_eq!(tab_strip_press(1), TabStripPress::MoveWindow);
    assert_eq!(tab_strip_press(2), TabStripPress::TitlebarDoubleClick);
    // A triple click already started a move on click one and ran the
    // titlebar action on click two; do not run it again.
    assert_eq!(tab_strip_press(3), TabStripPress::MoveWindow);
}

fn layout_rect(x: u16, y: u16, width: u16, height: u16) -> LayoutRect {
    LayoutRect {
        x,
        y,
        width,
        height,
    }
}

fn pane_layout(area: LayoutRect, panes: &[(&str, LayoutRect)]) -> PaneLayout {
    PaneLayout {
        workspace_id: "w".into(),
        tab_id: "t".into(),
        zoomed: false,
        area,
        focused_pane_id: panes[0].0.into(),
        panes: panes
            .iter()
            .map(|(id, rect)| LayoutPane {
                pane_id: (*id).into(),
                focused: false,
                rect: *rect,
            })
            .collect(),
        splits: Vec::new(),
    }
}

#[test]
fn ghostty_style_tab_hints_use_command_glyph() {
    #[cfg(target_os = "macos")]
    let prefix = "⌘";
    #[cfg(not(target_os = "macos"))]
    let prefix = "Ctrl+";
    assert_eq!(tab_key_equivalent(0, 1), None);
    assert_eq!(tab_key_equivalent(0, 2), Some(format!("{prefix}1")));
    assert_eq!(tab_key_equivalent(1, 2), Some(format!("{prefix}2")));
    assert_eq!(tab_key_equivalent(8, 9), Some(format!("{prefix}9")));
    assert_eq!(tab_key_equivalent(9, 10), None);
}

#[test]
fn pane_fractions_split_left_and_right_in_half() {
    let layout = pane_layout(
        layout_rect(0, 0, 100, 50),
        &[
            ("left", layout_rect(0, 0, 50, 50)),
            ("right", layout_rect(50, 0, 50, 50)),
        ],
    );
    assert_eq!(pane_fractions(&layout, "left"), Some((0.0, 0.0, 0.5, 1.0)));
    assert_eq!(pane_fractions(&layout, "right"), Some((0.5, 0.0, 0.5, 1.0)));
}

#[test]
fn pane_fractions_split_top_and_bottom_in_half() {
    let layout = pane_layout(
        layout_rect(0, 0, 100, 80),
        &[
            ("top", layout_rect(0, 0, 100, 40)),
            ("bottom", layout_rect(0, 40, 100, 40)),
        ],
    );
    assert_eq!(pane_fractions(&layout, "top"), Some((0.0, 0.0, 1.0, 0.5)));
    assert_eq!(
        pane_fractions(&layout, "bottom"),
        Some((0.0, 0.5, 1.0, 0.5))
    );
}

#[test]
fn pane_fractions_nested_split_keeps_child_ratios() {
    let layout = pane_layout(
        layout_rect(0, 0, 100, 100),
        &[
            ("left", layout_rect(0, 0, 50, 100)),
            ("right-top", layout_rect(50, 0, 50, 50)),
            ("right-bottom", layout_rect(50, 50, 50, 50)),
        ],
    );
    assert_eq!(pane_fractions(&layout, "left"), Some((0.0, 0.0, 0.5, 1.0)));
    assert_eq!(
        pane_fractions(&layout, "right-top"),
        Some((0.5, 0.0, 0.5, 0.5))
    );
    assert_eq!(
        pane_fractions(&layout, "right-bottom"),
        Some((0.5, 0.5, 0.5, 0.5))
    );
}

#[test]
fn pane_fractions_are_relative_to_a_non_zero_area_origin() {
    let layout = pane_layout(
        layout_rect(10, 20, 80, 40),
        &[
            ("left", layout_rect(10, 20, 40, 40)),
            ("right", layout_rect(50, 20, 40, 40)),
        ],
    );
    assert_eq!(pane_fractions(&layout, "left"), Some((0.0, 0.0, 0.5, 1.0)));
    assert_eq!(pane_fractions(&layout, "right"), Some((0.5, 0.0, 0.5, 1.0)));
}

#[test]
fn zoomed_layout_only_exposes_the_focused_pane_at_full_size() {
    let mut layout = pane_layout(
        layout_rect(0, 0, 100, 50),
        &[
            ("left", layout_rect(0, 0, 50, 50)),
            ("right", layout_rect(50, 0, 50, 50)),
        ],
    );
    layout.zoomed = true;
    layout.focused_pane_id = "right".into();

    assert_eq!(pane_fractions(&layout, "left"), None);
    assert_eq!(pane_fractions(&layout, "right"), Some((0.0, 0.0, 1.0, 1.0)));
}
