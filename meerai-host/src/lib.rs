pub mod diff;
pub mod registry;
pub mod search;
pub mod session;
pub mod syntax;
pub mod theme;
pub mod update;
pub mod workspace;

pub use diff::{
    DiffCell, DiffRow, DiffRowKind, FileChangeEntry, FileDiffView, UnifiedLine, UnifiedLineKind,
    build_edit_diff, build_file_diff, current_branch, diff_counts, diff_texts, list_git_changes,
    parse_unified_diff, tool_edit_diff,
};
pub use registry::{
    ChatGptLoginStatus, HostError, LiveEntry, LiveSessionDetail, LiveSessionSummary, RegistryEvent,
    RegistryEventKind, RegistrySnapshot, SessionRegistry, SessionStatus, SubscribeResult,
    TurnMetricsSnapshot, UsageStatsSnapshot,
};
pub use search::{
    MatchRank, SearchMatch, SearchMode, SearchPreview, build_search_preview,
    discover_workspace_files, local_workspace_files, resolve_preview_path, score_candidate,
    search_content, search_files,
};
pub use session::{
    AcpPresetInfo, Backend, ChatGptLogin, ChatGptLoginOutcome, HostAgentInfo, HostAuthKind,
    HostInfoSnapshot, HostMcpServerInfo, HostModelItem, HostProviderItem, HostProvidersSnapshot,
    HostReloadOutcome, HostReloadPayload, HostSettingsUpdate, HostSettingsView, HostSkillInfo,
    OpenedSession, ResolvedAgentApplication, ResolvedModelSelection, ResolvedSkillApplication,
    SessionOpener, WebServerHandle, WebServerLauncher, WebServerStart,
};
pub use theme::{HostColor, HostPalette, NamedColor, ResolvedThemeHex};
pub use workspace::{LocalWorkspace, WorkspaceMatch, WorkspaceQuery, local_workspace};
