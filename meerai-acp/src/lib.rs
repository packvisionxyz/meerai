//! meerai's ACP client bridge.
//!
//! The crate spawns one external ACP agent from a preset, performs the
//! `initialize` and `session/new` handshake, and exposes a typed event stream
//! plus permission replies. Every `agent-client-protocol` type stays inside
//! this crate.
//!
//! There is no CLI, TUI, settings, or agent-loop integration yet, and no claim
//! that meerai's permission rules, path guards, redaction, or sandbox applies
//! to the external agent process.

mod client;
mod events;

pub use client::{AcpSession, connect};
pub use events::{
    AcpEvent, AcpPermissionKind, AcpPermissionOption, AcpPermissionRequest, AcpPlanStep,
    AcpToolCall, AcpToolCallUpdate,
};

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
