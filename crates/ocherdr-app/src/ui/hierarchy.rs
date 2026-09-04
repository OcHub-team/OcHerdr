use super::super::*;
use crate::a11y::{
    ChromeA11y, apply_control, apply_list, apply_region, drop_zone_label, event_stream_lost_copy,
    event_stream_status_copy, keyboard_move_state_text, pane_a11y, pane_drag_handle_name,
    pane_drag_state_text,
};

mod pane_surface;
mod pane_templates;
mod sidebar;
mod support;
mod tabs;
mod terminal;

use pane_surface::*;
use pane_templates::*;
use support::*;
use terminal::{TabStripPress, tab_strip_press};

pub(crate) use pane_surface::status_indicator;

#[cfg(test)]
mod tests;
