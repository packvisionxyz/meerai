#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaletteAction {
    Slash(&'static str),
    CopyReply,
    NextTab,
    PreviousTab,
    ToggleThinking,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PaletteCommand {
    pub(crate) label: &'static str,
    pub(crate) hint: &'static str,
    pub(crate) keywords: &'static str,
    pub(crate) action: PaletteAction,
}

pub(crate) const PALETTE_COMMANDS: &[PaletteCommand] = &[
    PaletteCommand {
        label: "New session tab",
        hint: "/new",
        keywords: "create tab session",
        action: PaletteAction::Slash("/new"),
    },
    PaletteCommand {
        label: "Switch session",
        hint: "/sessions",
        keywords: "open resume tab list",
        action: PaletteAction::Slash("/sessions"),
    },
    PaletteCommand {
        label: "Close tab",
        hint: "Ctrl-w",
        keywords: "close tab session",
        action: PaletteAction::Slash("/close"),
    },
    PaletteCommand {
        label: "Next tab",
        hint: "Alt-Right / ]",
        keywords: "switch tab next",
        action: PaletteAction::NextTab,
    },
    PaletteCommand {
        label: "Previous tab",
        hint: "Alt-Left / [",
        keywords: "switch tab previous",
        action: PaletteAction::PreviousTab,
    },
    PaletteCommand {
        label: "Connect provider",
        hint: "/connect",
        keywords: "provider api key login auth",
        action: PaletteAction::Slash("/connect"),
    },
    PaletteCommand {
        label: "Set model",
        hint: "/models",
        keywords: "model picker provider",
        action: PaletteAction::Slash("/models"),
    },
    PaletteCommand {
        label: "Approval mode: plan",
        hint: "/mode plan",
        keywords: "mode read only permissions",
        action: PaletteAction::Slash("/mode plan"),
    },
    PaletteCommand {
        label: "Approval mode: ask",
        hint: "/mode ask",
        keywords: "mode prompt permissions",
        action: PaletteAction::Slash("/mode ask"),
    },
    PaletteCommand {
        label: "Approval mode: auto-edit",
        hint: "/mode auto-edit",
        keywords: "mode automatic edit permissions",
        action: PaletteAction::Slash("/mode auto-edit"),
    },
    PaletteCommand {
        label: "Approval mode: yolo",
        hint: "/mode yolo",
        keywords: "mode allow all permissions",
        action: PaletteAction::Slash("/mode yolo"),
    },
    PaletteCommand {
        label: "Search files",
        hint: "Ctrl-f",
        keywords: "find open search",
        action: PaletteAction::Slash("/search"),
    },
    PaletteCommand {
        label: "Search file contents",
        hint: "/grep",
        keywords: "grep content search ripgrep",
        action: PaletteAction::Slash("/grep"),
    },
    PaletteCommand {
        label: "Review changed files",
        hint: "/diff",
        keywords: "diff changes git review",
        action: PaletteAction::Slash("/diff"),
    },
    PaletteCommand {
        label: "Toggle sidebar",
        hint: "Ctrl-b",
        keywords: "sidebar panel layout",
        action: PaletteAction::Slash("/sidebar"),
    },
    PaletteCommand {
        label: "Toggle latest thinking",
        hint: "Ctrl-t",
        keywords: "reasoning thinking collapse expand",
        action: PaletteAction::ToggleThinking,
    },
    PaletteCommand {
        label: "Compact context",
        hint: "/compact",
        keywords: "compaction summarize context tokens",
        action: PaletteAction::Slash("/compact"),
    },
    PaletteCommand {
        label: "Reset conversation",
        hint: "/reset",
        keywords: "clear reset history",
        action: PaletteAction::Slash("/reset"),
    },
    PaletteCommand {
        label: "Cancel running turn",
        hint: "Ctrl-c",
        keywords: "cancel stop abort turn",
        action: PaletteAction::Slash("/cancel"),
    },
    PaletteCommand {
        label: "Copy last reply",
        hint: "y",
        keywords: "clipboard yank copy",
        action: PaletteAction::CopyReply,
    },
    PaletteCommand {
        label: "Show help",
        hint: "/help",
        keywords: "help commands shortcuts",
        action: PaletteAction::Slash("/help"),
    },
    PaletteCommand {
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
