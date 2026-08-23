use ochub_ui::notifications::{NotificationLevel, NotificationRequest};

use crate::i18n::{I18n, Key, k};

/// What the user tried to do, used as the toast title.
///
/// The concrete reason stays in the message; titles are localized phrases so two
/// different failures remain distinguishable even when their details look similar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    DiscoverSessions,
    RefreshSnapshot,
    ApplyLiveUpdate,
    UpdateFavorites,
    ApplyOrganization,
    RemoveHosts,
    SaveAppearance,
    SaveLanguage,
    RemoveHost,
    SaveHost,
    OpenTerminal,
    ApplyPalette,
    SpawnTerminal,
    ResizeTerminal,
    RenderTerminal,
    TerminalStream,
    TerminalRuntime,
    CannotEditThisMac,
    NeedGroupOrTag,
    CannotRemoveActiveHost,
    EmptyWorkspaceOrTabName,
    SshDestinationRequired,
    SshPortInvalid,
    NoSessionSelected,
}

impl FailureKind {
    fn level(self) -> NotificationLevel {
        match self {
            Self::CannotEditThisMac
            | Self::NeedGroupOrTag
            | Self::CannotRemoveActiveHost
            | Self::EmptyWorkspaceOrTabName
            | Self::SshDestinationRequired
            | Self::SshPortInvalid
            | Self::NoSessionSelected => NotificationLevel::Warning,
            Self::DiscoverSessions
            | Self::RefreshSnapshot
            | Self::ApplyLiveUpdate
            | Self::UpdateFavorites
            | Self::ApplyOrganization
            | Self::RemoveHosts
            | Self::SaveAppearance
            | Self::SaveLanguage
            | Self::RemoveHost
            | Self::SaveHost
            | Self::OpenTerminal
            | Self::ApplyPalette
            | Self::SpawnTerminal
            | Self::ResizeTerminal
            | Self::RenderTerminal
            | Self::TerminalStream
            | Self::TerminalRuntime => NotificationLevel::Error,
        }
    }

    fn title_key(self) -> Key {
        match self {
            Self::DiscoverSessions => k::NOTIFY_DISCOVER_SESSIONS,
            Self::RefreshSnapshot => k::NOTIFY_REFRESH_SESSION,
            Self::ApplyLiveUpdate => k::NOTIFY_APPLY_LIVE_UPDATE,
            Self::UpdateFavorites => k::NOTIFY_UPDATE_FAVORITES,
            Self::ApplyOrganization => k::NOTIFY_APPLY_ORGANIZATION,
            Self::RemoveHosts => k::NOTIFY_REMOVE_HOSTS,
            Self::SaveAppearance => k::NOTIFY_SAVE_APPEARANCE,
            Self::SaveLanguage => k::NOTIFY_SAVE_LANGUAGE,
            Self::RemoveHost => k::NOTIFY_REMOVE_HOST,
            Self::SaveHost => k::NOTIFY_SAVE_HOST,
            Self::OpenTerminal => k::NOTIFY_OPEN_TERMINAL,
            Self::ApplyPalette => k::NOTIFY_APPLY_PALETTE,
            Self::SpawnTerminal => k::NOTIFY_SPAWN_TERMINAL,
            Self::ResizeTerminal => k::NOTIFY_RESIZE_TERMINAL,
            Self::RenderTerminal => k::NOTIFY_RENDER_TERMINAL,
            Self::TerminalStream => k::NOTIFY_TERMINAL_STREAM,
            Self::TerminalRuntime => k::NOTIFY_TERMINAL_RUNTIME,
            Self::CannotEditThisMac => k::NOTIFY_CANNOT_EDIT_THIS_MAC,
            Self::NeedGroupOrTag => k::NOTIFY_CANNOT_APPLY_ORGANIZATION,
            Self::CannotRemoveActiveHost => k::NOTIFY_CANNOT_REMOVE_HOST,
            Self::EmptyWorkspaceOrTabName => k::NOTIFY_CANNOT_RENAME,
            Self::SshDestinationRequired => k::NOTIFY_INVALID_SSH_DESTINATION,
            Self::SshPortInvalid => k::NOTIFY_INVALID_SSH_PORT,
            Self::NoSessionSelected => k::NOTIFY_CANNOT_OPEN_TERMINAL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailureNotice {
    pub level: NotificationLevel,
    pub title: String,
    pub message: String,
}

impl FailureNotice {
    pub fn request(self) -> NotificationRequest {
        NotificationRequest::new(self.level, self.title).message(self.message)
    }
}

pub(crate) fn notification_for(kind: FailureKind, detail: &str, i18n: I18n) -> FailureNotice {
    FailureNotice {
        level: kind.level(),
        title: i18n.text(kind.title_key()).to_owned(),
        message: detail.to_owned(),
    }
}

fn command_title_key(method: &str) -> Key {
    match method {
        "workspace.create" => k::NOTIFY_WORKSPACE_CREATE,
        "workspace.close" => k::NOTIFY_WORKSPACE_CLOSE,
        "workspace.rename" => k::NOTIFY_WORKSPACE_RENAME,
        "tab.create" => k::NOTIFY_TAB_CREATE,
        "tab.close" => k::NOTIFY_TAB_CLOSE,
        "tab.rename" => k::NOTIFY_TAB_RENAME,
        "pane.close" => k::NOTIFY_PANE_CLOSE,
        "pane.rename" => k::NOTIFY_PANE_RENAME,
        "pane.split" => k::NOTIFY_PANE_SPLIT,
        "pane.zoom" => k::NOTIFY_PANE_ZOOM,
        "pane.focus_direction" => k::NOTIFY_PANE_FOCUS,
        _ => k::NOTIFY_HERDR_COMMAND,
    }
}

pub(crate) fn command_notification(method: &str, detail: &str, i18n: I18n) -> FailureNotice {
    FailureNotice {
        level: NotificationLevel::Error,
        title: i18n.text(command_title_key(method)).to_owned(),
        message: format!("{method}: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;

    #[test]
    fn invalid_input_maps_to_warning_and_operation_failure_maps_to_error() {
        let i18n = I18n::new(Language::English);
        let invalid = notification_for(
            FailureKind::SshDestinationRequired,
            "SSH destination is required.",
            i18n,
        );
        let failed = notification_for(FailureKind::DiscoverSessions, "connection refused", i18n);
        assert_eq!(invalid.level, NotificationLevel::Warning);
        assert_eq!(invalid.title, "Invalid SSH destination");
        assert_eq!(invalid.message, "SSH destination is required.");
        assert_eq!(failed.level, NotificationLevel::Error);
        assert_eq!(failed.title, "Could not discover Herdr sessions");
        assert_eq!(failed.message, "connection refused");
    }
}
