//! Shell commands only. Editing belongs to ochub-ui and unhandled input stays
//! with the terminal. This table also supplies labels for discovery surfaces.
use ochub_ui::{gpui::Keystroke, shortcuts::Shortcut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    NewTab,
    ClosePane,
    CloseWorkspace,
    NewWorkspace,
    ToggleFiles,
    FileAddress,
    Appearance,
    Copy,
    SelectAll,
    PreviousTab,
    NextTab,
    Rename,
    Help,
}

pub(crate) const COMMANDS: &[(Command, Shortcut)] = &[
    (Command::NewTab, Shortcut::new("cmd-t", "ctrl-shift-t")),
    (Command::ClosePane, Shortcut::new("cmd-w", "ctrl-shift-w")),
    (
        Command::CloseWorkspace,
        Shortcut::new("cmd-shift-w", "ctrl-alt-shift-w"),
    ),
    (
        Command::NewWorkspace,
        Shortcut::new("cmd-shift-n", "ctrl-shift-n"),
    ),
    (
        Command::ToggleFiles,
        Shortcut::new("cmd-shift-e", "ctrl-shift-e"),
    ),
    (Command::FileAddress, Shortcut::new("cmd-l", "ctrl-shift-l")),
    (Command::Appearance, Shortcut::new("cmd-,", "ctrl-,")),
    (Command::Copy, Shortcut::new("cmd-c", "ctrl-shift-c")),
    (Command::SelectAll, Shortcut::new("cmd-a", "ctrl-shift-a")),
    (Command::PreviousTab, Shortcut::new("cmd-[", "ctrl-shift-[")),
    (Command::NextTab, Shortcut::new("cmd-]", "ctrl-shift-]")),
    (
        Command::PreviousTab,
        Shortcut::new("ctrl-shift-tab", "ctrl-shift-tab"),
    ),
    (Command::NextTab, Shortcut::new("ctrl-tab", "ctrl-tab")),
    (Command::Rename, Shortcut::new("f2", "f2")),
    (Command::Help, Shortcut::new("cmd-/", "ctrl-shift-/")),
];

pub(crate) fn command_for(key: &Keystroke) -> Option<Command> {
    COMMANDS
        .iter()
        .find(|(_, shortcut)| shortcut.matches(key))
        .map(|(command, _)| *command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_no_collisions_and_keeps_terminal_control_sequences() {
        let mut keys = std::collections::HashSet::new();
        for (_, shortcut) in COMMANDS {
            assert!(keys.insert(shortcut.stroke()));
            assert!(command_for(&Keystroke::parse(shortcut.stroke()).unwrap()).is_some());
        }
        for key in [
            "ctrl-c",
            "ctrl-w",
            "ctrl-l",
            "alt-left",
            "ctrl-alt-tab",
            "shift-f2",
        ] {
            assert_eq!(command_for(&Keystroke::parse(key).unwrap()), None, "{key}");
        }
    }
}
