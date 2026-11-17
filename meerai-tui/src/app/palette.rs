#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaletteAction {
    Slash(&'static str),
    CopyReply,
    NextTab,
    PreviousTab,
    MoveTabLeft,
    MoveTabRight,
    CycleApproval,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PaletteCommand {
    pub(crate) section: &'static str,
    pub(crate) label: &'static str,
    pub(crate) hint: &'static str,
    pub(crate) keywords: &'static str,
    pub(crate) action: PaletteAction,
}

impl PaletteCommand {
    pub(crate) fn display_hint(&self) -> String {
        self.hint.to_string()
    }
}

pub(crate) const PALETTE_COMMANDS: &[PaletteCommand] = &[
    PaletteCommand {
        section: "Session",
        label: "New session tab",
        hint: "ctrl+n",
        keywords: "create tab session new",
        action: PaletteAction::Slash("/new"),
    },
    PaletteCommand {
        section: "Session",
        label: "Switch session",
        hint: "ctrl+s",
        keywords: "open resume tab list sessions",
        action: PaletteAction::Slash("/sessions"),
    },
    PaletteCommand {
        section: "Session",
        label: "Close tab",
        hint: "ctrl+w",
        keywords: "close tab session",
        action: PaletteAction::Slash("/close"),
    },
    PaletteCommand {
        section: "Session",
        label: "Next tab",
        hint: "alt+right / ]",
        keywords: "switch tab next",
        action: PaletteAction::NextTab,
    },
    PaletteCommand {
        section: "Session",
        label: "Previous tab",
        hint: "alt+left / [",
        keywords: "switch tab previous",
        action: PaletteAction::PreviousTab,
    },
    PaletteCommand {
        section: "Session",
        label: "Move tab left",
        hint: "alt+shift+left",
        keywords: "move tab reorder left",
        action: PaletteAction::MoveTabLeft,
    },
    PaletteCommand {
        section: "Session",
        label: "Move tab right",
        hint: "alt+shift+right",
        keywords: "move tab reorder right",
        action: PaletteAction::MoveTabRight,
    },
    PaletteCommand {
        section: "Model",
        label: "Connect provider",
        hint: "ctrl+a",
        keywords: "provider api key login auth connect",
        action: PaletteAction::Slash("/connect"),
    },
    PaletteCommand {
        section: "Model",
        label: "Set model",
        hint: "ctrl+o",
        keywords: "model picker provider set",
        action: PaletteAction::Slash("/models"),
    },
    PaletteCommand {
        section: "Approval",
        label: "Cycle approval mode",
        hint: "shift+tab",
        keywords: "mode plan ask auto cycle permissions",
        action: PaletteAction::CycleApproval,
    },
    PaletteCommand {
        section: "Approval",
        label: "Approval mode: plan",
        hint: "/mode plan",
        keywords: "mode read only permissions plan",
        action: PaletteAction::Slash("/mode plan"),
    },
    PaletteCommand {
        section: "Approval",
        label: "Approval mode: ask",
        hint: "/mode ask",
        keywords: "mode prompt permissions ask",
        action: PaletteAction::Slash("/mode ask"),
    },
    PaletteCommand {
        section: "Approval",
        label: "Approval mode: auto",
        hint: "/mode auto",
        keywords: "mode automatic allow permissions auto",
        action: PaletteAction::Slash("/mode auto"),
    },
    PaletteCommand {
        section: "View",
        label: "Search files",
        hint: "ctrl+f",
        keywords: "find open search files",
        action: PaletteAction::Slash("/search"),
    },
    PaletteCommand {
        section: "View",
        label: "Search file contents",
        hint: "ctrl+shift+f",
        keywords: "grep content search ripgrep contents",
        action: PaletteAction::Slash("/grep"),
    },
    PaletteCommand {
        section: "View",
        label: "Review changed files",
        hint: "ctrl+e",
        keywords: "diff changes git review",
        action: PaletteAction::Slash("/diff"),
    },
    PaletteCommand {
        section: "View",
        label: "Toggle sidebar",
        hint: "ctrl+b",
        keywords: "sidebar panel layout toggle",
        action: PaletteAction::Slash("/sidebar"),
    },
    PaletteCommand {
        section: "Turn",
        label: "Compact context",
        hint: "ctrl+k",
        keywords: "compaction summarize context tokens compact",
        action: PaletteAction::Slash("/compact"),
    },
    PaletteCommand {
        section: "Turn",
        label: "Reset conversation",
        hint: "ctrl+shift+r",
        keywords: "clear reset history conversation",
        action: PaletteAction::Slash("/reset"),
    },
    PaletteCommand {
        section: "Turn",
        label: "Cancel running turn",
        hint: "ctrl+c",
        keywords: "cancel stop abort turn",
        action: PaletteAction::Slash("/cancel"),
    },
    PaletteCommand {
        section: "App",
        label: "Copy last reply",
        hint: "y",
        keywords: "clipboard yank copy reply",
        action: PaletteAction::CopyReply,
    },
    PaletteCommand {
        section: "App",
        label: "Show help",
        hint: "f1",
        keywords: "help commands shortcuts",
        action: PaletteAction::Slash("/help"),
    },
    PaletteCommand {
        section: "App",
        label: "Quit meerai",
        hint: "ctrl+q",
        keywords: "exit quit close",
        action: PaletteAction::Slash("/quit"),
    },
];

pub(crate) fn filtered_commands(filter: &str) -> Vec<&'static PaletteCommand> {
    let needle = filter.trim().to_ascii_lowercase();
    PALETTE_COMMANDS
        .iter()
        .filter(|command| {
            needle.is_empty()
                || command.label.to_ascii_lowercase().contains(&needle)
                || command.hint.to_ascii_lowercase().contains(&needle)
                || command.keywords.contains(&needle)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_label_hint_and_keyword() {
        assert_eq!(filtered_commands("").len(), PALETTE_COMMANDS.len());
        assert!(
            filtered_commands("sidebar")
                .iter()
                .any(|command| command.label == "Toggle sidebar")
        );
        assert!(
            filtered_commands("ctrl+w")
                .iter()
                .any(|command| command.label == "Close tab")
        );
        assert!(
            filtered_commands("grep")
                .iter()
                .any(|command| command.label == "Search file contents")
        );
        assert!(filtered_commands("no-such-command").is_empty());
    }

    #[test]
    fn every_command_has_a_shortcut_hint() {
        for command in PALETTE_COMMANDS {
            assert!(!command.hint.is_empty(), "{} has no hint", command.label);
        }
    }
}
