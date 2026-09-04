use super::*;
use gpui::{AnyElement, StyleRefinement, Subscription};

/// Separate data invalidation from terminal-only paint invalidation. GPUI still
/// walks the root/layout ancestors of a dirty view; cached sibling chrome can
/// reuse its layout/paint instead of rebuilding every workspace, tab and file.
pub(crate) struct RenderCache {
    pub(super) terminal_signal: Entity<TerminalSignal>,
    sidebar: Entity<ChromePart>,
    tabs: Entity<ChromePart>,
    files: Entity<ChromePart>,
}

impl RenderCache {
    pub(crate) fn new(cx: &mut Context<OcHerdrView>) -> Self {
        let parent = cx.entity();
        Self {
            terminal_signal: cx.new(|_| TerminalSignal),
            sidebar: cx.new(|cx| ChromePart::new(&parent, Part::Sidebar, cx)),
            tabs: cx.new(|cx| ChromePart::new(&parent, Part::Tabs, cx)),
            files: cx.new(|cx| ChromePart::new(&parent, Part::Files, cx)),
        }
    }

    pub(crate) fn notify_terminal(&self, cx: &mut App) {
        cx.notify(self.terminal_signal.entity_id());
    }

    pub(super) fn sidebar(&self) -> AnyElement {
        self.sidebar
            .clone()
            .cached(
                StyleRefinement::default()
                    .w(px(SIDEBAR_WIDTH))
                    .h_full()
                    .flex_none(),
            )
            .into_any_element()
    }

    pub(super) fn tabs(&self) -> AnyElement {
        self.tabs
            .clone()
            .cached(
                StyleRefinement::default()
                    .w_full()
                    .h(px(HEADER_HEIGHT))
                    .flex_none(),
            )
            .into_any_element()
    }

    pub(super) fn files(&self, width: f32, overlay: bool) -> AnyElement {
        let mut style = StyleRefinement::default().w(px(width)).h_full().flex_none();
        if overlay {
            style = style.absolute().right_0().top_0().bottom_0();
        }
        self.files.clone().cached(style).into_any_element()
    }

    #[cfg(all(test, not(target_os = "windows")))]
    pub(crate) fn render_counts(&self, cx: &App) -> [usize; 3] {
        [&self.sidebar, &self.tabs, &self.files].map(|part| part.read(cx).render_count)
    }
}

pub(super) struct TerminalSignal;

impl Render for TerminalSignal {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().absolute().size_0()
    }
}

#[derive(Clone, Copy)]
enum Part {
    Sidebar,
    Tabs,
    Files,
}

struct ChromePart {
    parent: WeakEntity<OcHerdrView>,
    part: Part,
    _subscription: Subscription,
    #[cfg(all(test, not(target_os = "windows")))]
    render_count: usize,
}

impl ChromePart {
    fn new(parent: &Entity<OcHerdrView>, part: Part, cx: &mut Context<Self>) -> Self {
        Self {
            parent: parent.downgrade(),
            part,
            // Real model/input changes invalidate chrome. A terminal signal
            // dirties the ancestor for traversal but does not notify its model.
            _subscription: cx.observe(parent, |_, _, cx| cx.notify()),
            #[cfg(all(test, not(target_os = "windows")))]
            render_count: 0,
        }
    }
}

impl Render for ChromePart {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(test, not(target_os = "windows")))]
        {
            self.render_count += 1;
        }
        self.parent
            .update(cx, |parent, cx| -> AnyElement {
                match self.part {
                    Part::Sidebar => {
                        let chrome = parent.chrome_a11y();
                        parent.render_sidebar(&chrome, cx).into_any_element()
                    }
                    Part::Tabs => {
                        let chrome = parent.chrome_a11y();
                        parent
                            .render_tab_bar(&chrome, window, cx)
                            .into_any_element()
                    }
                    Part::Files => parent.render_file_panel(
                        f32::from(window.viewport_size().width) < FILE_PANEL_OVERLAY_BREAKPOINT,
                        cx,
                    ),
                }
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}
