use super::*;

impl OcHerdrView {
    pub(crate) fn open_agent_panel(
        &mut self,
        pane_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_pane(pane_id.clone(), window, cx);
        let same =
            matches!(&self.overlay, Overlay::AgentPanel { pane_id: open } if open == &pane_id);
        if !same {
            self.reset_agent_panel_state();
            self.agent_name_input
                .update(cx, |input, cx| input.set_content("", cx));
            self.set_overlay(Overlay::AgentPanel { pane_id }, cx);
        }
        self.fetch_agent_name(cx);
        self.fetch_agent_output(cx);
        self.agent_prompt_input
            .update(cx, |input, cx| input.set_content("", cx));
        self.agent_prompt_input
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    pub(crate) fn close_agent_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::AgentPanel { .. }) {
            self.set_overlay(Overlay::None, cx);
        }
        self.focus.focus(window, cx);
    }

    pub(crate) fn submit_agent_prompt(&mut self, cx: &mut Context<Self>) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        let pane_id = pane_id.clone();
        if matches!(
            self.agent_prompts.get(&pane_id),
            Some(AgentPromptPhase::Sending { .. })
        ) {
            return;
        }
        let raw = self.agent_prompt_input.read(cx).content();
        let Some(text) = agent_prompt_text_to_send(raw.as_ref()) else {
            self.post_notice(
                FailureNotice {
                    level: ochub_ui::notifications::NotificationLevel::Warning,
                    title: self.i18n.text(k::AGENT_PROMPT_SEND).to_owned(),
                    message: self.i18n.text(k::AGENT_PROMPT_EMPTY).to_owned(),
                },
                cx,
            );
            return;
        };
        let Some(connection) = &self.connection else {
            return;
        };
        let socket = connection.socket_path().to_owned();
        let sent_text = text.clone();
        let params = json!({ "target": pane_id, "text": text });
        let target = pane_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "agent.prompt", params) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        this.agent_prompts
                            .insert(target.clone(), AgentPromptPhase::Sent);
                        if agent_panel_pane(&this.overlay) == Some(target.as_str())
                            && this.agent_prompt_input.read(cx).content().as_ref() == sent_text
                        {
                            this.agent_prompt_input
                                .update(cx, |input, cx| input.set_content("", cx));
                        }
                        if agent_panel_pane(&this.overlay) == Some(target.as_str()) {
                            this.fetch_agent_output(cx);
                        }
                    }
                    Err(HerdrError::Api { code, message }) if code == "agent_blocked" => {
                        this.agent_prompts.insert(
                            target.clone(),
                            AgentPromptPhase::Blocked {
                                message: message.clone(),
                            },
                        );
                        this.notify_failure(
                            FailureKind::AgentBlocked,
                            this.i18n.text(k::AGENT_BLOCKED_DETAIL),
                            cx,
                        );
                    }
                    Err(error) => {
                        let message = error.to_string();
                        this.agent_prompts.insert(
                            target.clone(),
                            AgentPromptPhase::Failed {
                                message: message.clone(),
                            },
                        );
                        this.notify_command_failure("agent.prompt", message, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.agent_prompts
            .insert(pane_id, AgentPromptPhase::Sending { _task: task });
        cx.notify();
    }

    pub(crate) fn refresh_agent_output(&mut self, cx: &mut Context<Self>) {
        self.fetch_agent_output(cx);
    }

    pub(crate) fn submit_agent_rename(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        if self.agent_renames.contains_key(pane_id)
            || !matches!(self.agent_name, AgentNameState::Ready)
        {
            return;
        }
        let pane_id = pane_id.clone();
        let raw = self.agent_name_input.read(cx).content();
        match parse_agent_name(raw.as_ref()) {
            Err(error) => {
                self.agent_name_error = Some(error);
                cx.notify();
            }
            Ok(name) => {
                self.agent_name_error = None;
                let Some(connection) = &self.connection else {
                    return;
                };
                let socket = connection.socket_path().to_owned();
                let params = match name {
                    Some(name) => json!({ "target": pane_id, "name": name }),
                    None => json!({ "target": pane_id }),
                };
                let target = pane_id.clone();
                let task = cx.spawn(async move |this, cx| {
                    let result = cx
                        .background_spawn(
                            async move { request_socket(&socket, "agent.rename", params) },
                        )
                        .await;
                    this.update(cx, |this, cx| {
                        this.agent_renames.remove(&target);
                        match result {
                            Ok(value) => match parse_agent_info_result(value) {
                                Ok(agent) => {
                                    if agent_panel_pane(&this.overlay) == Some(target.as_str()) {
                                        this.agent_name = AgentNameState::Ready;
                                        this.agent_name_input.update(cx, |input, cx| {
                                            input.set_content(
                                                agent.name.as_deref().unwrap_or(""),
                                                cx,
                                            )
                                        });
                                    }
                                    this.resync_snapshot(this.event_epoch, cx);
                                }
                                Err(message) => {
                                    this.notify_command_failure("agent.rename", message, cx)
                                }
                            },
                            Err(error) => this.notify_command_failure("agent.rename", error, cx),
                        }
                        cx.notify();
                    })
                    .ok();
                });
                self.agent_renames.insert(pane_id, task);
                cx.notify();
            }
        }
    }

    pub(crate) fn send_agent_keys(
        &mut self,
        keys: &'static [&'static str],
        cx: &mut Context<Self>,
    ) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        if self.agent_keys.contains_key(pane_id) {
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        let pane_id = pane_id.clone();
        let socket = connection.socket_path().to_owned();
        let params = json!({ "target": pane_id, "keys": keys });
        let target = pane_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "agent.send_keys", params) })
                .await;
            this.update(cx, |this, cx| {
                this.agent_keys.remove(&target);
                match result {
                    Ok(_) if agent_panel_pane(&this.overlay) == Some(target.as_str()) => {
                        this.fetch_agent_output(cx)
                    }
                    Ok(_) => {}
                    Err(error) => this.notify_command_failure("agent.send_keys", error, cx),
                }
                cx.notify();
            })
            .ok();
        });
        self.agent_keys.insert(pane_id, task);
        cx.notify();
    }

    pub(super) fn fetch_agent_name(&mut self, cx: &mut Context<Self>) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        let Some(connection) = &self.connection else {
            return;
        };
        let pane_id = pane_id.clone();
        let socket = connection.socket_path().to_owned();
        let params = json!({ "target": pane_id });
        let target = pane_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "agent.get", params) })
                .await;
            this.update(cx, |this, cx| {
                if agent_panel_pane(&this.overlay) != Some(target.as_str()) {
                    return;
                }
                match result {
                    Ok(value) => match parse_agent_info_result(value) {
                        Ok(agent) => {
                            this.agent_name = AgentNameState::Ready;
                            this.agent_name_input.update(cx, |input, cx| {
                                input.set_content(agent.name.as_deref().unwrap_or(""), cx)
                            });
                        }
                        Err(message) => {
                            this.agent_name = AgentNameState::Failed(message.clone());
                            this.notify_command_failure("agent.get", message, cx);
                        }
                    },
                    Err(error) => {
                        let message = error.to_string();
                        this.agent_name = AgentNameState::Failed(message.clone());
                        this.notify_command_failure("agent.get", message, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.agent_name = AgentNameState::Loading { _task: task };
        cx.notify();
    }

    pub(super) fn fetch_agent_output(&mut self, cx: &mut Context<Self>) {
        let Overlay::AgentPanel { pane_id } = &self.overlay else {
            return;
        };
        let Some(connection) = &self.connection else {
            return;
        };
        let pane_id = pane_id.clone();
        let socket = connection.socket_path().to_owned();
        let params = json!({
            "target": pane_id,
            "source": AGENT_OUTPUT_SOURCE,
            "format": "text",
        });
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { request_socket(&socket, "agent.read", params) })
                .await;
            this.update(cx, |this, cx| {
                if agent_panel_pane(&this.overlay) != Some(pane_id.as_str()) {
                    return;
                }
                this.agent_output = match result {
                    Ok(value) => match parse_agent_read_result(&value) {
                        Ok((text, truncated)) => AgentOutputState::Ready { text, truncated },
                        Err(message) => AgentOutputState::Failed { message },
                    },
                    Err(error) => {
                        this.notify_command_failure("agent.read", &error, cx);
                        AgentOutputState::Failed {
                            message: error.to_string(),
                        }
                    }
                };
                cx.notify();
            })
            .ok();
        });
        self.agent_output = AgentOutputState::Loading { _task: task };
        cx.notify();
    }

    pub(super) fn reset_agent_panel_state(&mut self) {
        self.agent_name = AgentNameState::Idle;
        self.agent_output = AgentOutputState::Idle;
        self.agent_prompts
            .retain(|_, phase| matches!(phase, AgentPromptPhase::Sending { .. }));
        self.agent_name_error = None;
    }

    pub(super) fn reconcile_open_agent_panel(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if agent_panel_target_missing(&self.overlay, self.snapshot.as_ref()) {
            self.set_overlay(Overlay::None, cx);
            return;
        }
        if refresh {
            self.fetch_agent_output(cx);
        }
    }
}
