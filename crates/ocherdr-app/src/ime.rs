use super::*;
use std::ops::Range;

impl EntityInputHandler for OcHerdrView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let marked = self.ime_marked.as_deref().unwrap_or("");
        let utf16: Vec<u16> = marked.encode_utf16().collect();
        let start = range.start.min(utf16.len());
        let end = range.end.min(utf16.len()).max(start);
        *adjusted_range = Some(start..end);
        Some(String::from_utf16_lossy(&utf16[start..end]))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if !self.accepts_ime() {
            return None;
        }
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime_marked
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_ime_preedit(window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_ime() {
            return;
        }
        self.commit_ime_text(text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_ime() {
            return;
        }
        self.set_ime_preedit(new_text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<ochub_ui::gpui::Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<ochub_ui::gpui::Pixels>> {
        self.ime_cursor_bounds(window).or(Some(Bounds {
            origin: element_bounds.origin,
            size: size(px(1.), px(16.)),
        }))
    }

    fn character_index_for_point(
        &mut self,
        _point: ochub_ui::gpui::Point<ochub_ui::gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.accepts_ime()
    }
}
