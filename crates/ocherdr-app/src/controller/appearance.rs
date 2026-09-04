use super::*;

impl OcHerdrView {
    pub(crate) fn open_node_manager(&mut self, cx: &mut Context<Self>) {
        let profile_index = self.profile_index;
        self.set_overlay(Overlay::NodeManager, cx);
        self.host_center
            .update(cx, |center, cx| center.open(profile_index, cx));
    }

    pub(crate) fn open_appearance(&mut self, cx: &mut Context<Self>) {
        self.open_select = None;
        self.appearance_ui = Default::default();
        self.set_overlay(Overlay::Appearance, cx);
    }

    pub(crate) fn close_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.appearance_ui.dismiss_sheet() {
            cx.notify();
            return;
        }
        self.open_select = None;
        self.appearance_ui = Default::default();
        self.set_overlay(Overlay::None, cx);
        self.focus.focus(window, cx);
    }

    pub(crate) fn cancel_bulk_remove(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::ConfirmBulkRemove) {
            self.set_overlay(Overlay::NodeManager, cx);
        }
    }

    pub(crate) fn confirm_bulk_remove(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::ConfirmBulkRemove) {
            return;
        }
        self.set_overlay(Overlay::NodeManager, cx);
        self.host_center
            .update(cx, |center, cx| center.confirm_bulk_remove(cx));
    }

    pub(crate) fn request_choose_node(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.profiles.len() {
            return;
        }
        self.apply_profile(index, cx);
    }

    pub(crate) fn toggle_host_switcher(&mut self, cx: &mut Context<Self>) {
        self.set_overlay(
            if matches!(self.overlay, Overlay::HostSwitcher) {
                Overlay::None
            } else {
                Overlay::HostSwitcher
            },
            cx,
        );
    }

    pub(crate) fn close_host_switcher(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::HostSwitcher) {
            self.set_overlay(Overlay::None, cx);
        }
    }

    pub(super) fn apply_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        self.set_overlay(Overlay::None, cx);
        if index == self.profile_index {
            self.remember_current_host(cx);
            let profile_id = self.current_profile().id().to_owned();
            match self.host_connection_state(&profile_id) {
                HostConnectionState::Connected | HostConnectionState::Connecting => cx.notify(),
                HostConnectionState::Disconnected => self.reload(None, cx),
                HostConnectionState::Degraded => {
                    let preferred = self.current_session().map(|session| session.name.clone());
                    self.disconnect_host(&profile_id, cx);
                    self.reload(preferred, cx);
                }
            }
            return;
        }
        self.select_profile(index, cx);
    }

    pub(crate) fn apply_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep the configured family name even when the file is missing, so a
        // temporarily absent theme can come back on the next launch.
        install_appearance(&self.appearance, window.appearance());
        theme::apply_window_background(window);
        let palette = current_terminal_palette(&self.appearance);
        let mut palette_error = None;
        for runtime in self
            .session_panes
            .iter_mut()
            .flat_map(|session| session.panes.values_mut())
        {
            if let Err(error) = runtime.terminal.apply_palette(&palette) {
                palette_error = Some(error);
            }
            runtime.color_scheme_dark = palette.dark;
            runtime.palette_signature = palette.signature();
        }
        if let Some(error) = palette_error {
            self.notify_failure(FailureKind::ApplyPalette, error, cx);
        }
        self.persist_settings(FailureKind::SaveAppearance, cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub(crate) fn set_theme_family(
        &mut self,
        family_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set(
            "theme",
            &crate::config::values::ThemeRef::Name(family_id.clone()).to_config(),
        );
        self.appearance.theme_family = family_id;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_appearance_mode(
        &mut self,
        mode: AppearanceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set("appearance-mode", mode.as_config());
        self.appearance.mode = mode;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_backdrop_mode(
        &mut self,
        backdrop: BackdropMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set("window-backdrop", backdrop.as_config());
        self.appearance.backdrop = backdrop;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_background_opacity(
        &mut self,
        opacity: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set(
            "background-opacity",
            &crate::config::format_opacity(opacity),
        );
        self.appearance.background_opacity = opacity;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_agent_notifications(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.config.set(
            "agent-notifications",
            if enabled { "true" } else { "false" },
        );
        self.agent_notifications = enabled;
        self.persist_settings(FailureKind::SaveAppearance, cx);
        cx.notify();
    }

    pub(crate) fn set_status_indicators(
        &mut self,
        style: StatusIndicatorStyle,
        cx: &mut Context<Self>,
    ) {
        self.config.set("status-indicators", style.as_config());
        self.status_indicators = style;
        self.persist_settings(FailureKind::SaveAppearance, cx);
        cx.notify();
    }

    pub(crate) fn set_terminal_theme(
        &mut self,
        theme: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match theme.as_deref() {
            None => self.config.set("terminal-theme", ""),
            Some(id) => self.config.set("terminal-theme", id),
        }
        self.appearance.terminal_theme = theme;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_window_padding(
        &mut self,
        horizontal: bool,
        value: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if horizontal {
            self.config.set("window-padding-x", &value.to_string());
            self.appearance.window_padding_x = value;
        } else {
            self.config.set("window-padding-y", &value.to_string());
            self.appearance.window_padding_y = value;
        }
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_font_thicken_strength(
        &mut self,
        strength: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config
            .set("font-thicken-strength", &strength.to_string());
        self.appearance.font.thicken_strength = strength;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_palette_slot(
        &mut self,
        slot: u8,
        color: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance.palette[slot as usize] = color;
        let values = palette_config_values(&self.appearance.palette);
        self.config.set_repeatable("palette", &values);
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_font_family(
        &mut self,
        family: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if family.is_empty() {
            self.config.set_repeatable("font-family", &[]);
        } else {
            self.config
                .set_repeatable("font-family", std::slice::from_ref(&family));
        }
        self.appearance.font.family = family;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_font_size(&mut self, size: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.config
            .set("font-size", &crate::config::format_font_size(size));
        self.appearance.font.size = size;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_font_features(
        &mut self,
        features: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set_repeatable("font-feature", &features);
        self.appearance.font.features = features;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_font_thicken(
        &mut self,
        thicken: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config
            .set("font-thicken", if thicken { "true" } else { "false" });
        self.appearance.font.thicken = thicken;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_cell_width(
        &mut self,
        metric: Option<crate::config::values::MetricModifier>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set(
            "adjust-cell-width",
            &metric
                .map(crate::config::values::MetricModifier::to_config)
                .unwrap_or_default(),
        );
        self.appearance.font.cell_width = metric;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_cell_height(
        &mut self,
        metric: Option<crate::config::values::MetricModifier>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.config.set(
            "adjust-cell-height",
            &metric
                .map(crate::config::values::MetricModifier::to_config)
                .unwrap_or_default(),
        );
        self.appearance.font.cell_height = metric;
        self.apply_appearance(window, cx);
    }

    pub(crate) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.config.set("language", language.as_config());
        self.i18n.set_preference(language);
        theme::reload_registry();
        let i18n = self.i18n;
        self.host_center
            .update(cx, |center, cx| center.apply_language(i18n, cx));
        self.rename_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::COMMON_NAME), cx)
        });
        self.worktree_label_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::WORKTREE_FIELD_LABEL_HINT), cx)
        });
        self.worktree_branch_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::WORKTREE_FIELD_BRANCH_HINT), cx)
        });
        self.worktree_base_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::WORKTREE_FIELD_BASE_HINT), cx)
        });
        self.worktree_path_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::WORKTREE_FIELD_PATH_HINT), cx)
        });
        self.agent_name_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::COMMON_NAME), cx)
        });
        self.agent_prompt_input.update(cx, |input, cx| {
            input.set_placeholder(self.i18n.text(k::AGENT_PROMPT_PLACEHOLDER), cx)
        });
        self.persist_settings(FailureKind::SaveLanguage, cx);
        cx.notify();
    }
}
