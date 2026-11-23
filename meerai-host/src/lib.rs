pub mod actions;
pub mod diff;
pub mod registry;
pub mod search;
pub mod session;
pub mod syntax;
pub mod theme;
pub mod update;
pub mod workspace;

pub use actions::{
    HostAction, HostActionContext, execute_host_action, parse_host_action, render_context,
    render_definitions, render_memories, render_packs, render_routes,
};
pub use diff::{
    DiffCell, DiffRow, DiffRowKind, FileChangeEntry, FileDiffView, UnifiedLine, UnifiedLineKind,
    build_edit_diff, build_file_diff, current_branch, diff_counts, diff_texts, list_git_changes,
    parse_unified_diff, tool_edit_diff,
};
pub use meerai_store::{
    InstalledPack, ModelRoute, NewProjectMemory, PackSourceKind, ProjectMemory, ProjectMemoryKind,
    RoutingSettings, TaskType,
};
pub use registry::{
    ChatGptLoginStatus, HostError, LiveEntry, LiveSessionDetail, LiveSessionSummary, LiveShellRun,
    RegistryEvent, RegistryEventKind, RegistrySnapshot, SessionRegistry, SessionStatus,
    SubscribeResult, TurnMetricsSnapshot, UsageStatsSnapshot,
};
pub use search::{
    MatchRank, SearchMatch, SearchMode, SearchPreview, build_search_preview,
    discover_workspace_files, local_workspace_files, resolve_preview_path, score_candidate,
    search_content, search_files,
};
pub use session::{
    AcpPresetInfo, Backend, ChatGptLogin, ChatGptLoginOutcome, ForgetModel, HostAgentInfo,
    HostAuthKind, HostDefinitionInfo, HostDefinitionsDiagnostics, HostInfoSnapshot,
    HostMcpServerInfo, HostModelItem, HostProviderItem, HostProvidersSnapshot, HostReloadOutcome,
    HostReloadPayload, HostSettingsUpdate, HostSettingsView, HostShadowedDefinition, HostSkillInfo,
    OpenedSession, ResolvedAgentApplication, ResolvedModelSelection, ResolvedSkillApplication,
    SessionOpener, WebServerHandle, WebServerLauncher, WebServerStart, definition_token_estimate,
};
pub use theme::{HostColor, HostPalette, NamedColor, ResolvedThemeHex};
pub use workspace::{LocalWorkspace, WorkspaceMatch, WorkspaceQuery, local_workspace};
