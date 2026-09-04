//! macOS editing semantics, operating directly on the component's selection.
use super::*;
use gpui::KeyDownEvent;

#[derive(Clone, Copy)]
enum Motion {
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
    WordLeft,
    WordRight,
    Left,
    Right,
    Up,
    Down,
}

enum Edit {
    Move(Motion, bool),
    Delete(Motion),
    KillLine,
    Transpose,
}

fn edit(event: &KeyDownEvent) -> Option<Edit> {
    use Motion::*;
    let key = event.keystroke.key.as_str();
    let m = event.keystroke.modifiers;
    let (motion, delete) = if m.platform && !m.control && !m.alt {
        match key {
            "left" => (LineStart, false),
            "right" => (LineEnd, false),
            "up" => (DocumentStart, false),
            "down" => (DocumentEnd, false),
            "backspace" if !m.shift => (LineStart, true),
            "delete" if !m.shift => (LineEnd, true),
            _ => return None,
        }
    } else if m.alt && !m.platform && !m.control {
        match key {
            "left" => (WordLeft, false),
            "right" => (WordRight, false),
            "backspace" if !m.shift => (WordLeft, true),
            "delete" if !m.shift => (WordRight, true),
            _ => return None,
        }
    } else if m.control && !m.platform && !m.alt {
        match key {
            "a" => (LineStart, false),
            "e" => (LineEnd, false),
            "b" => (Left, false),
            "f" => (Right, false),
            "p" => (Up, false),
            "n" => (Down, false),
            "h" if !m.shift => (Left, true),
            "d" if !m.shift => (Right, true),
            "k" if !m.shift => return Some(Edit::KillLine),
            "t" if !m.shift => return Some(Edit::Transpose),
            _ => return None,
        }
    } else if m.shift && !m.platform && !m.control && !m.alt {
        match key {
            "up" => (Up, false),
            "down" => (Down, false),
            _ => return None,
        }
    } else {
        return None;
    };
    Some(if delete {
        Edit::Delete(motion)
    } else {
        Edit::Move(motion, m.shift)
    })
}

fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor..].find('\n').map_or(text.len(), |i| cursor + i)
}

// Unicode word boundaries, not ASCII bytes: skip separators in the movement
// direction, then stop at the far edge of the next word (or symbol cluster).
fn word_left(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .split_word_bound_indices()
        .rev()
        .find(|(_, segment)| !segment.chars().all(char::is_whitespace))
        .map_or(0, |(start, _)| start)
}

fn word_right(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .split_word_bound_indices()
        .find(|(_, segment)| !segment.chars().all(char::is_whitespace))
        .map_or(text.len(), |(start, segment)| {
            cursor + start + segment.len()
        })
}

impl TextInput {
    pub(crate) fn is_macos_editing_key(event: &KeyDownEvent) -> bool {
        edit(event).is_some()
    }

    fn motion_offset(&self, motion: Motion) -> usize {
        let cursor = self.cursor_offset();
        match motion {
            Motion::LineStart => line_start(&self.content, cursor),
            Motion::LineEnd => line_end(&self.content, cursor),
            Motion::DocumentStart => 0,
            Motion::DocumentEnd => self.content.len(),
            // Password fields navigate as a single concealed value.
            Motion::WordLeft if self.masked => 0,
            Motion::WordRight if self.masked => self.content.len(),
            Motion::WordLeft => word_left(&self.content, cursor),
            Motion::WordRight => word_right(&self.content, cursor),
            Motion::Left => self.previous_boundary(cursor),
            Motion::Right => self.next_boundary(cursor),
            Motion::Up | Motion::Down => {
                let start = line_start(&self.content, cursor);
                let column = self.content[start..cursor].graphemes(true).count();
                let target_start = if matches!(motion, Motion::Up) {
                    if start == 0 {
                        return 0;
                    }
                    line_start(&self.content, start - 1)
                } else {
                    let end = line_end(&self.content, cursor);
                    if end == self.content.len() {
                        return end;
                    }
                    end + 1
                };
                let target_end = line_end(&self.content, target_start);
                self.content[target_start..target_end]
                    .grapheme_indices(true)
                    .nth(column)
                    .map_or(target_end, |(offset, _)| target_start + offset)
            }
        }
    }

    pub(crate) fn handle_macos_editing_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(edit) = edit(event) else {
            return false;
        };
        // The IME owns its preedit. Never delete committed text underneath it.
        if self.marked_range.is_some() {
            return true;
        }
        match edit {
            Edit::Move(motion, extend) => {
                let target = self.motion_offset(motion);
                if extend {
                    self.select_to(target, cx);
                } else {
                    self.move_to(target, cx);
                }
                self.scroll_selection_into_view();
            }
            Edit::Delete(motion) => {
                let cursor = self.cursor_offset();
                let target = self.motion_offset(motion);
                let range = if self.selected_range.is_empty() {
                    cursor.min(target)..cursor.max(target)
                } else {
                    self.selected_range.clone()
                };
                self.delete_bytes(range, window, cx);
            }
            Edit::KillLine => {
                let cursor = self.cursor_offset();
                let end = line_end(&self.content, cursor);
                let range = if !self.selected_range.is_empty() {
                    self.selected_range.clone()
                } else if cursor == end && end < self.content.len() {
                    cursor..end + 1
                } else {
                    cursor..end
                };
                self.delete_bytes(range, window, cx);
            }
            Edit::Transpose => {
                if !self.selected_range.is_empty() {
                    return true;
                }
                let cursor = self.cursor_offset();
                let end = if cursor == self.content.len() {
                    cursor
                } else {
                    self.next_boundary(cursor)
                };
                let middle = self.previous_boundary(end);
                let start = self.previous_boundary(middle);
                if start < middle && middle < end && !self.content[start..end].contains('\n') {
                    let replacement = format!(
                        "{}{}",
                        &self.content[middle..end],
                        &self.content[start..middle]
                    );
                    let range = self.offset_to_utf16(start)..self.offset_to_utf16(end);
                    self.replace_text_in_range(Some(range), &replacement, window, cx);
                }
            }
        }
        true
    }

    fn delete_bytes(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if !range.is_empty() {
            let range = self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end);
            // Let the component create exactly one undo snapshot and Changed event.
            self.replace_text_in_range(Some(range), "", window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn macos_navigation_and_selection_use_unicode_boundaries(cx: &mut TestAppContext) {
        cx.update(init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            let input = TextInput::new(cx, "editor").multiline(true);
            input.focus_handle.focus(window, cx);
            input
        });
        let text = "one two\n中文😀 three";
        let start = text.find('中').unwrap();
        let emoji = text.find('😀').unwrap();
        let word = text.find("three").unwrap();
        for (keys, from, expected) in [
            ("cmd-left", word + 2, start),
            ("cmd-right", start, text.len()),
            ("cmd-up", word, 0),
            ("cmd-down", 0, text.len()),
            ("alt-left", text.len(), word),
            ("alt-right", word, text.len()),
            ("alt-left", 7, 4),
            ("alt-right", 3, 7),
            ("ctrl-a", word, start),
            ("ctrl-e", start, text.len()),
            ("ctrl-b", emoji + '😀'.len_utf8(), emoji),
            ("ctrl-f", emoji, emoji + '😀'.len_utf8()),
            ("ctrl-p", start, 0),
            ("ctrl-n", 0, start),
        ] {
            input.update(cx, |input, cx| {
                input.set_content(text, cx);
                input.move_to(from, cx);
            });
            cx.simulate_keystrokes(keys);
            input.read_with(cx, |input, _| {
                assert_eq!(input.selected_range, expected..expected, "{keys}");
                assert_eq!(input.content.as_ref(), text);
            });
        }
        input.update(cx, |input, cx| input.move_to(word, cx));
        cx.simulate_keystrokes("cmd-shift-left");
        input.read_with(cx, |input, _| {
            assert_eq!(input.selected_range, start..word);
            assert!(input.selection_reversed);
        });
        // Cross the original anchor without losing it.
        cx.simulate_keystrokes("cmd-shift-right");
        input.read_with(cx, |input, _| {
            assert_eq!(input.selected_range, word..text.len());
            assert!(!input.selection_reversed);
        });
        cx.simulate_keystrokes("cmd-shift-up");
        input.read_with(cx, |input, _| assert_eq!(input.selected_range, 0..word));
        cx.simulate_keystrokes("cmd-shift-down");
        input.read_with(cx, |input, _| {
            assert_eq!(input.selected_range, word..text.len())
        });
        input.update(cx, |input, cx| input.move_to(word, cx));
        cx.simulate_keystrokes("alt-shift-right");
        input.read_with(cx, |input, _| {
            assert_eq!(input.selected_range, word..text.len())
        });
        cx.simulate_keystrokes("alt-shift-left");
        input.read_with(cx, |input, _| assert_eq!(input.selected_range, word..word));
        input.update(cx, |input, cx| input.move_to(start, cx));
        cx.simulate_keystrokes("shift-up shift-down");
        input.read_with(cx, |input, _| {
            assert_eq!(input.selected_range, start..start)
        });
    }

    #[gpui::test]
    fn macos_deletion_is_atomic_undoable_and_keeps_graphemes_intact(cx: &mut TestAppContext) {
        cx.update(init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            let input = TextInput::new(cx, "editor").multiline(true);
            input.focus_handle.focus(window, cx);
            input
        });
        for (text, keys, expected) in [
            ("first\n中文😀", "cmd-backspace", "first\n"),
            ("one two", "alt-backspace", "one "),
            ("one two", "cmd-up alt-delete", " two"),
            ("first\nsecond", "cmd-up cmd-delete", "\nsecond"),
            ("first\nsecond", "cmd-up ctrl-k ctrl-k", "second"),
            ("x👩‍💻", "ctrl-h", "x"),
            ("👩‍💻x", "cmd-up ctrl-d", "x"),
            ("e\u{301}x", "ctrl-t", "xe\u{301}"),
        ] {
            input.update(cx, |input, cx| input.set_content(text, cx));
            cx.simulate_keystrokes(keys);
            input.read_with(cx, |input, _| {
                assert_eq!(input.content.as_ref(), expected, "{keys}")
            });
            let undo = if keys == "cmd-up ctrl-k ctrl-k" {
                "cmd-z cmd-z"
            } else {
                "cmd-z"
            };
            cx.simulate_keystrokes(undo);
            input.read_with(cx, |input, _| {
                assert_eq!(input.content.as_ref(), text, "undo {keys}")
            });
        }
        input.update(cx, |input, cx| input.set_content("one two", cx));
        cx.simulate_keystrokes("cmd-up alt-shift-right alt-backspace");
        input.read_with(cx, |input, _| assert_eq!(input.content.as_ref(), " two"));
        cx.simulate_keystrokes("cmd-z cmd-shift-z");
        input.read_with(cx, |input, _| assert_eq!(input.content.as_ref(), " two"));
    }

    #[gpui::test]
    fn macos_clipboard_find_and_composition_remain_native(cx: &mut TestAppContext) {
        cx.update(init);
        let (input, cx) = cx.add_window_view(|window, cx| {
            let input = TextInput::new(cx, "editor")
                .multiline(true)
                .with_content("中文😀 one");
            input.focus_handle.focus(window, cx);
            input
        });
        cx.simulate_keystrokes("cmd-a cmd-c cmd-x cmd-shift-v");
        input.read_with(cx, |input, _| {
            assert_eq!(input.content.as_ref(), "中文😀 one")
        });
        cx.simulate_keystrokes("cmd-f");
        let search = input.read_with(cx, |input, _| input.find_input.clone().unwrap());
        cx.simulate_input("one two");
        cx.simulate_keystrokes("cmd-left alt-shift-right");
        search.read_with(cx, |search, _| assert_eq!(search.selected_range, 0..3));
        cx.simulate_keystrokes("escape");
        input.update_in(cx, |input, window, cx| {
            assert!(input.focus_handle.is_focused(window));
            input.replace_and_mark_text_in_range(None, "候选", None, window, cx);
        });
        let before = input.read_with(cx, |input, _| {
            (
                input.content.clone(),
                input.selected_range.clone(),
                input.marked_range.clone(),
            )
        });
        cx.simulate_keystrokes("cmd-left cmd-backspace alt-delete ctrl-k");
        input.read_with(cx, |input, _| {
            assert_eq!(
                (
                    input.content.clone(),
                    input.selected_range.clone(),
                    input.marked_range.clone()
                ),
                before
            );
        });
    }
}
