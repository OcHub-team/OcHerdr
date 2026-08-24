use super::super::*;
use super::hierarchy::status_color;
use crate::a11y::apply_dialog;
use ochub_ui::gpui::{AnyElement, Div};
use ochub_ui::layout::section_header;
use ochub_ui::scrollbar::{VerticalScrollbar, contain_vertical_scroll};

impl OcHerdrView {
    pub(super) fn render_agent_panel(
        &mut self,
        pane_id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = self.i18n;
        let pane = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pane(pane_id));
        let title = pane
            .and_then(|pane| pane.display_agent.as_deref().or(pane.agent.as_deref()))
            .unwrap_or(pane_id)
            .to_owned();
        let kind = pane.and_then(|pane| pane.agent.clone());
        let status = pane.map(|pane| pane.agent_status);
        let location = self.snapshot.as_ref().zip(pane).map(|(snapshot, pane)| {
            let workspace = snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == pane.workspace_id)
                .map(|workspace| workspace.label.as_str())
                .unwrap_or(pane.workspace_id.as_str());
            let tab = snapshot
                .tabs
                .iter()
                .find(|tab| tab.tab_id == pane.tab_id)
                .map(|tab| tab.label.as_str())
                .unwrap_or(pane.tab_id.as_str());
            i18n.agent_location(workspace, tab, &pane.pane_id)
        });
        let sending = matches!(self.agent_prompt, AgentPromptPhase::Sending);
        let renaming = self.agent_rename_task.is_some();
        let keys_busy = self.agent_keys_task.is_some();
        let name_error = self
            .agent_name_error
            .map(|error| i18n.agent_name_error(error));
        let output = self.agent_output.clone();
        let prompt_phase = self.agent_prompt.clone();
        let output_scroll = self.agent_output_scroll.clone();

        let close = icon_only_button_tone(
            "close-agent-panel",
            i18n.text(k::COMMON_CLOSE),
            IconName::Close,
            ButtonTone::Ghost,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, window, cx| this.close_agent_panel(window, cx)));

        let save_name = if renaming {
            busy_button(
                "save-agent-name",
                i18n.text(k::COMMON_SAVE),
                ButtonTone::Primary,
                ButtonSize::Sm,
                true,
            )
            .into_any_element()
        } else {
            button(
                "save-agent-name",
                i18n.text(k::COMMON_SAVE),
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, window, cx| this.submit_agent_rename(window, cx)))
            .into_any_element()
        };

        let refresh = icon_only_button_tone(
            "refresh-agent-output",
            i18n.text(k::COMMON_REFRESH),
            IconName::Refresh,
            ButtonTone::Ghost,
            ButtonSize::Sm,
        )
        .on_click(cx.listener(|this, _, _window, cx| this.refresh_agent_output(cx)));

        let esc = key_button(
            "agent-key-esc",
            i18n.text(k::AGENT_KEYS_ESC),
            ButtonTone::Neutral,
            keys_busy,
            cx,
            |this, cx| this.send_agent_keys(&["esc"], cx),
        );
        let enter = key_button(
            "agent-key-enter",
            i18n.text(k::AGENT_KEYS_ENTER),
            ButtonTone::Neutral,
            keys_busy,
            cx,
            |this, cx| this.send_agent_keys(&["enter"], cx),
        );
        let ctrl_c = key_button(
            "agent-key-ctrl-c",
            i18n.text(k::AGENT_KEYS_CTRL_C),
            ButtonTone::Danger,
            keys_busy,
            cx,
            |this, cx| this.send_agent_keys(&["ctrl+c"], cx),
        );

        let send = if sending {
            busy_button(
                "send-agent-prompt",
                i18n.text(k::AGENT_PROMPT_SENDING),
                ButtonTone::Primary,
                ButtonSize::Sm,
                true,
            )
            .into_any_element()
        } else {
            button(
                "send-agent-prompt",
                i18n.text(k::AGENT_PROMPT_SEND),
                ButtonTone::Primary,
                ButtonSize::Sm,
            )
            .on_click(cx.listener(|this, _, _window, cx| this.submit_agent_prompt(cx)))
            .into_any_element()
        };

        let feedback = match &prompt_phase {
            AgentPromptPhase::Idle | AgentPromptPhase::Sending => None,
            AgentPromptPhase::Sent => {
                Some((theme::muted(), i18n.text(k::AGENT_PROMPT_SENT).to_owned()))
            }
            AgentPromptPhase::Failed { blocked: true, .. } => Some((
                theme::yellow(),
                i18n.text(k::AGENT_BLOCKED_DETAIL).to_owned(),
            )),
            AgentPromptPhase::Failed {
                blocked: false,
                message,
            } => Some((theme::red(), message.clone())),
        };

        let card = apply_dialog(
            modal_card(),
            "agent-panel-dialog",
            i18n.text(k::AGENT_PANEL_TITLE),
        )
        .w(px(560.))
        .h(px(660.))
        .rounded(px(CORNER_MODAL))
        .child(modal_header(title).child(close))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .gap_3()
                .px_5()
                .pb_4()
                .child(agent_meta_row(
                    i18n.text(k::AGENT_STATUS),
                    status.map(|status| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(status_dot(status_color(status)))
                            .child(i18n.agent_status(status))
                            .into_any_element()
                    }),
                ))
                .children(kind.map(|kind| {
                    agent_meta_row(
                        i18n.text(k::AGENT_KIND),
                        Some(div().child(kind).into_any_element()),
                    )
                }))
                .children(
                    location
                        .map(|location| div().text_xs().text_color(theme::muted()).child(location)),
                )
                .child(field_with_error(
                    i18n.text(k::COMMON_NAME),
                    false,
                    Some(i18n.text(k::AGENT_NAME_HINT).into()),
                    name_error.map(SharedString::from),
                    self.agent_name_input.clone(),
                ))
                .child(div().flex().justify_end().child(save_name))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(section_header(i18n.text(k::AGENT_OUTPUT), None))
                        .child(refresh),
                )
                .child(render_agent_output(&output, output_scroll, i18n))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(i18n.text(k::AGENT_KEYS)),
                        )
                        .child(esc)
                        .child(enter)
                        .child(ctrl_c),
                )
                .child(field(
                    i18n.text(k::AGENT_PROMPT_PLACEHOLDER),
                    false,
                    None,
                    self.agent_prompt_input.clone(),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(match feedback {
                            None => div().flex_1().into_any_element(),
                            Some((color, text)) => div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(color)
                                .child(text)
                                .into_any_element(),
                        })
                        .child(send),
                ),
        );

        modal_overlay(card)
            .top_0()
            .left_0()
            .on_key_down(cx.listener(|this, event, window, cx| {
                this.handle_overlay_key(event, window, cx);
            }))
    }
}

fn agent_meta_row(label: &'static str, value: Option<AnyElement>) -> Div {
    let mut row = div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .child(div().text_xs().text_color(theme::muted()).child(label));
    if let Some(value) = value {
        row = row.child(value);
    }
    row
}

fn render_agent_output(output: &AgentOutputState, scroll: ScrollHandle, i18n: I18n) -> AnyElement {
    let body = match output {
        AgentOutputState::Idle | AgentOutputState::Loading => div()
            .flex()
            .items_center()
            .gap_2()
            .p_3()
            .child(spinner(theme::muted(), 14.))
            .child(
                div()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(i18n.text(k::AGENT_OUTPUT_LOADING)),
            )
            .into_any_element(),
        AgentOutputState::Failed { message } => div()
            .p_3()
            .text_sm()
            .text_color(theme::red())
            .child(message.clone())
            .into_any_element(),
        AgentOutputState::Ready { text, .. } if text.is_empty() => div()
            .p_3()
            .text_sm()
            .text_color(theme::muted())
            .child(i18n.text(k::AGENT_OUTPUT_EMPTY))
            .into_any_element(),
        AgentOutputState::Ready { text, truncated } => {
            let lines = text.lines().map(|line| {
                div().w_full().child(if line.is_empty() {
                    " ".to_owned()
                } else {
                    line.to_owned()
                })
            });
            let mut column = div()
                .id("agent-output-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .on_scroll_wheel(contain_vertical_scroll(scroll.clone()))
                .p_3()
                .text_xs()
                .font_family("JetBrains Mono")
                .children(lines);
            if *truncated {
                column = column.child(
                    div()
                        .pt_2()
                        .text_color(theme::muted())
                        .child(i18n.text(k::AGENT_OUTPUT_TRUNCATED)),
                );
            }
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(column)
                .child(VerticalScrollbar::new(
                    ochub_ui::gpui::ElementId::Name("agent-output-scrollbar".into()),
                    scroll,
                ))
                .into_any_element()
        }
    };
    div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .rounded(px(CORNER_CONTROL))
        .border_1()
        .border_color(theme::border())
        .bg(theme::inset())
        .child(body)
        .into_any_element()
}

fn key_button(
    id: &'static str,
    label: &'static str,
    tone: ButtonTone,
    busy: bool,
    cx: &mut Context<OcHerdrView>,
    on_click: impl Fn(&mut OcHerdrView, &mut Context<OcHerdrView>) + 'static,
) -> AnyElement {
    if busy {
        disabled_button(id, label, tone, ButtonSize::Sm, true).into_any_element()
    } else {
        button(id, label, tone, ButtonSize::Sm)
            .on_click(cx.listener(move |this, _, _window, cx| on_click(this, cx)))
            .into_any_element()
    }
}
