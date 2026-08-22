use ochub_ui::notifications::{NotificationLevel, NotificationRequest};

use crate::i18n::I18n;

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

    fn title_key(self) -> &'static str {
        match self {
            Self::DiscoverSessions => "Could not discover Herdr sessions",
            Self::RefreshSnapshot => "Could not refresh the session",
            Self::ApplyLiveUpdate => "Could not apply a live update",
            Self::UpdateFavorites => "Could not update favorites",
            Self::ApplyOrganization => "Could not apply organization",
            Self::RemoveHosts => "Could not remove hosts",
            Self::SaveAppearance => "Could not save appearance",
            Self::SaveLanguage => "Could not save language",
            Self::RemoveHost => "Could not remove host",
            Self::SaveHost => "Could not save host",
            Self::OpenTerminal => "Could not open Terminal",
            Self::ApplyPalette => "Could not apply the terminal theme",
            Self::SpawnTerminal => "Could not open a terminal pane",
            Self::ResizeTerminal => "Could not resize the terminal",
            Self::RenderTerminal => "Could not render the terminal",
            Self::TerminalStream => "Terminal stream failed",
            Self::TerminalRuntime => "Terminal runtime failed",
            Self::CannotEditThisMac => "Cannot edit this Mac",
            Self::NeedGroupOrTag => "Cannot apply organization",
            Self::CannotRemoveActiveHost => "Cannot remove host",
            Self::EmptyWorkspaceOrTabName => "Cannot rename",
            Self::SshDestinationRequired => "Invalid SSH destination",
            Self::SshPortInvalid => "Invalid SSH port",
            Self::NoSessionSelected => "Cannot open Terminal",
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

fn command_title_key(method: &str) -> &'static str {
    match method {
        "workspace.create" => "Could not create a workspace",
        "workspace.close" => "Could not close the workspace",
        "workspace.rename" => "Could not rename the workspace",
        "tab.create" => "Could not create a tab",
        "tab.close" => "Could not close the tab",
        "tab.rename" => "Could not rename the tab",
        "pane.close" => "Could not close the pane",
        "pane.rename" => "Could not rename the pane",
        "pane.split" => "Could not split the pane",
        "pane.zoom" => "Could not zoom the pane",
        "pane.focus_direction" => "Could not move pane focus",
        _ => "Could not run the Herdr command",
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
