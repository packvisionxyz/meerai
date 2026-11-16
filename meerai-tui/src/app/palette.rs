#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaletteAction {
    Slash(&'static str),
    CopyReply,
    NextTab,
    PreviousTab,
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
        let shortcut = match self.action {
            PaletteAction::Slash("/sessions") => Some('s'),
            PaletteAction::Slash("/models") => Some('o'),
            PaletteAction::Slash("/connect") => Some('a'),
            PaletteAction::Slash("/diff") => Some('e'),
            _ => None,
        };
        match shortcut {
            Some(key) => format!("{}  {}", crate::app::ctrl_label(key), self.hint),
            None => self.hint.to_string(),
        }
    }
}

pub(crate) const PALETTE_COMMANDS: &[PaletteCommand] = &[
    PaletteCommand {
        section: "Session",
        label: "New session tab",
        hint: "/new",
        keywords: "create tab session",
        action: PaletteAction::Slash("/new"),
    },
    PaletteCommand {
        section: "Session",
        label: "Switch session",
        hint: "/sessions",
        keywords: "open resume tab list",
        action: PaletteAction::Slash("/sessions"),
    },
    PaletteCommand {
        section: "Session",
        label: "Close tab",
        hint: "Ctrl-w",
        keywords: "close tab session",
        action: PaletteAction::Slash("/close"),
    },
    PaletteCommand {
        section: "Session",
        label: "Next tab",
        hint: "Alt-Right / ]",
        keywords: "switch tab next",
        action: PaletteAction::NextTab,
    },
    PaletteCommand {
        section: "Session",
        label: "Previous tab",
        hint: "Alt-Left / [",
        keywords: "switch tab previous",
        action: PaletteAction::PreviousTab,
    },
    PaletteCommand {
        section: "Model",
        label: "Connect provider",
        hint: "/connect",
        keywords: "provider api key login auth",
        action: PaletteAction::Slash("/connect"),
    },
    PaletteCommand {
        section: "Model",
        label: "Set model",
        hint: "/models",
        keywords: "model picker provider",
        action: PaletteAction::Slash("/models"),
    },
    PaletteCommand {
        section: "Approval",
        label: "Approval mode: plan",
        hint: "/mode plan",
        keywords: "mode read only permissions",
        action: PaletteAction::Slash("/mode plan"),
    },
    PaletteCommand {
        section: "Approval",
        label: "Approval mode: ask",
        hint: "/mode ask",
        keywords: "mode prompt permissions",
        action: PaletteAction::Slash("/mode ask"),
    },
    PaletteCommand {
        section: "Approval",
        label: "Approval mode: auto-edit",
        hint: "/mode auto-edit",
        keywords: "mode automatic edit permissions",
        action: PaletteAction::Slash("/mode auto-edit"),
    },
    PaletteCommand {
        section: "Approval",
        label: "Approval mode: auto-approve",
        hint: "/mode auto-approve",
        keywords: "mode automatic approve allow permissions",
        action: PaletteAction::Slash("/mode auto-approve"),
    },
    PaletteCommand {
        section: "Approval",
        label: "Approval mode: yolo",
        hint: "/mode yolo",
        keywords: "mode allow all permissions",
        action: PaletteAction::Slash("/mode yolo"),
    },
    PaletteCommand {
        section: "View",
        label: "Search files",
        hint: "Ctrl-f",
        keywords: "find open search",
        action: PaletteAction::Slash("/search"),
    },
    PaletteCommand {
        section: "View",
        label: "Search file contents",
        hint: "/grep",
        keywords: "grep content search ripgrep",
        action: PaletteAction::Slash("/grep"),
    },
    PaletteCommand {
        section: "View",
        label: "Review changed files",
        hint: "/diff",
        keywords: "diff changes git review",
        action: PaletteAction::Slash("/diff"),
    },
    PaletteCommand {
        section: "View",
        label: "Toggle sidebar",
        hint: "Ctrl-b",
        keywords: "sidebar panel layout",
        action: PaletteAction::Slash("/sidebar"),
    },
    PaletteCommand {
        section: "Turn",
        label: "Compact context",
        hint: "/compact",
        keywords: "compaction summarize context tokens",
        action: PaletteAction::Slash("/compact"),
    },
    PaletteCommand {
        section: "Turn",
        label: "Reset conversation",
        hint: "/reset",
        keywords: "clear reset history",
        action: PaletteAction::Slash("/reset"),
    },
    PaletteCommand {
        section: "Turn",
        label: "Cancel running turn",
        hint: "Ctrl-c",
        keywords: "cancel stop abort turn",
        action: PaletteAction::Slash("/cancel"),
    },
    PaletteCommand {
        section: "App",
        label: "Copy last reply",
        hint: "y",
        keywords: "clipboard yank copy",
        action: PaletteAction::CopyReply,
    },
    PaletteCommand {
        section: "App",
        label: "Show help",
        hint: "/help",
        keywords: "help commands shortcuts",
        action: PaletteAction::Slash("/help"),
    },
    PaletteCommand {
        section: "App",
        label: "Quit meerai",
        hint: "/quit",
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
            filtered_commands("ctrl-w")
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
}
