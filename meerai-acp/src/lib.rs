//! meerai's ACP client bridge. It spawns an external agent and adapts it to
//! `AgentCommand`/`AgentEvent`. meerai's permission rules, path guards,
//! redaction, and sandbox do not apply to the external process.

mod client;
mod events;
mod worker;

pub use client::{AcpHandshake, AcpSession, AcpStart, connect, connect_with};
pub use events::{
    AcpEvent, AcpPermissionKind, AcpPermissionOption, AcpPermissionRequest, AcpPlanStep,
    AcpToolCall, AcpToolCallUpdate,
};
pub use worker::{AcpWorkerConfig, spawn_acp_worker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpPreset {
    pub id: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
}

pub const AGY: AcpPreset = AcpPreset {
    id: "agy",
    command: "agy",
    args: &["acp"],
};

pub const PRESETS: &[AcpPreset] = &[AGY];

pub fn presets() -> &'static [AcpPreset] {
    PRESETS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn presets_have_unique_ids_and_non_empty_commands() {
        let mut ids = HashSet::new();
        for preset in presets() {
            assert!(!preset.id.is_empty());
            assert!(!preset.command.is_empty());
            assert!(ids.insert(preset.id), "duplicate preset id: {}", preset.id);
        }
    }

    #[test]
    fn catalog_holds_only_agy() {
        assert_eq!(presets(), &[AGY]);
    }

    #[test]
    fn agy_runs_the_acp_subcommand() {
        assert_eq!(AGY.id, "agy");
        assert_eq!(AGY.command, "agy");
        assert_eq!(AGY.args, &["acp"]);
    }
}
