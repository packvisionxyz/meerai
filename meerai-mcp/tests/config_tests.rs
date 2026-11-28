use meerai_mcp::config::{
    MCP_CONFIG_FILE, McpTransport, content_hash, expand_env, parse_config, parse_tool_name,
    read_config_file, tool_name, validate_server_name,
};
use meerai_mcp::merge::merge_configs;

const MIXED: &str = r#"{
  "mcpServers": {
    "docs": { "command": "docs-server", "args": ["--stdio"] },
    "search": { "type": "http", "url": "https://mcp.example.test/v1" },
    "stream": { "type": "streamable-http", "url": "https://mcp.example.test/stream" },
    "legacy": { "type": "sse", "url": "https://mcp.example.test/sse" },
    "off": { "command": "x", "enabled": false },
    "ro": { "command": "y", "read_only": true }
  }
}"#;

#[test]
fn parses_both_transports_and_flags_unsupported_ones() {
    let config = parse_config(MIXED.as_bytes()).unwrap();
    assert!(config.warnings.is_empty());
    assert_eq!(config.servers.len(), 6);

    let docs = config.server("docs").unwrap();
    assert!(matches!(docs.transport, McpTransport::Stdio { .. }));
    assert_eq!(docs.kind(), "stdio");
    assert!(!docs.read_only);

    let search = config.server("search").unwrap();
    assert!(matches!(search.transport, McpTransport::Http { .. }));
    assert_eq!(search.kind(), "http");

    assert_eq!(config.server("stream").unwrap().kind(), "http");
    assert_eq!(config.server("legacy").unwrap().kind(), "sse");
    assert!(!config.server("legacy").unwrap().is_connectable());

    assert!(!config.server("off").unwrap().is_connectable());
    assert!(config.server("ro").unwrap().read_only);

    let connectable = config.connectable().count();
    assert_eq!(connectable, 4, "docs, search, stream, ro");
}

#[test]
fn rejects_bad_server_names_with_a_warning_not_an_error() {
    let raw = r#"{"mcpServers": {
        "CODE": {"command": "a"},
        "code": {"command": "b"},
        "way_too_long_server_name_x": {"command": "c"},
        "ok": {"command": "d"}
    }}"#;
    let config = parse_config(raw.as_bytes()).unwrap();
    assert_eq!(config.servers.len(), 1);
    assert_eq!(config.servers[0].name, "ok");
    assert_eq!(config.warnings.len(), 3);
    assert!(
        config
            .warnings
            .iter()
            .any(|w| w.contains("built-in toolset"))
    );
    assert!(config.warnings.iter().any(|w| w.contains("lowercase")));
    assert!(config.warnings.iter().any(|w| w.contains("exceeds")));
}

#[test]
fn reserved_and_uppercase_names_are_refused_directly() {
    assert!(validate_server_name("github").is_ok());
    assert!(validate_server_name("code").is_err());
    assert!(validate_server_name("GitHub").is_err());
    assert!(validate_server_name("").is_err());
    assert!(validate_server_name("has space").is_err());
}

#[test]
fn server_without_command_or_url_is_rejected() {
    let config = parse_config(br#"{"mcpServers": {"empty": {}}}"#).unwrap();
    assert!(config.servers.is_empty());
    assert_eq!(config.warnings.len(), 1);
    assert!(config.warnings[0].contains("neither 'command' nor 'url'"));
}

#[test]
fn timeout_is_clamped_and_read_from_both_keys() {
    let raw = r#"{"mcpServers": {
        "tiny": {"command": "a", "timeout": 1},
        "huge": {"command": "b", "timeout_ms": 9999999},
        "default": {"command": "c"}
    }}"#;
    let config = parse_config(raw.as_bytes()).unwrap();
    assert_eq!(config.server("tiny").unwrap().timeout_ms, 1_000);
    assert_eq!(config.server("huge").unwrap().timeout_ms, 600_000);
    assert_eq!(config.server("default").unwrap().timeout_ms, 120_000);
}

#[test]
fn env_expansion_only_touches_braced_names() {
    let lookup = |name: &str| match name {
        "TOKEN" => Some("secret".to_string()),
        _ => None,
    };
    assert_eq!(expand_env("Bearer ${TOKEN}", &lookup), "Bearer secret");
    assert_eq!(expand_env("${MISSING}", &lookup), "");
    assert_eq!(expand_env("$TOKEN stays", &lookup), "$TOKEN stays");
    assert_eq!(expand_env("${1BAD} stays", &lookup), "${1BAD} stays");
}

#[test]
fn tool_names_round_trip_and_stay_within_the_provider_limit() {
    let name = tool_name("github", "search_issues");
    assert_eq!(name, "mcp__github__search_issues");
    assert_eq!(
        parse_tool_name(&name),
        Some(("github".to_string(), "search_issues".to_string()))
    );

    let long = "t".repeat(200);
    let capped = tool_name("github", &long);
    assert!(capped.chars().count() <= 64, "{capped}");
    assert!(capped.starts_with("mcp__github__"));

    assert_eq!(parse_tool_name("code-read_file"), None);
}

#[test]
fn config_hash_changes_when_bytes_change() {
    let first = content_hash(b"{\"mcpServers\":{}}");
    let second = content_hash(b"{\"mcpServers\":{\"a\":{\"command\":\"x\"}}}");
    assert!(first.starts_with("sha256:"));
    assert_eq!(first.len(), 7 + 64);
    assert_ne!(first, second);
}

#[test]
fn read_config_file_returns_the_bytes_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(MCP_CONFIG_FILE);
    std::fs::write(&path, MIXED).unwrap();

    let (config, hash) = read_config_file(&path).unwrap();
    assert_eq!(config.servers.len(), 6);
    assert_eq!(hash, content_hash(MIXED.as_bytes()));
}

#[test]
fn unknown_project_servers_are_dropped_until_the_file_is_trusted() {
    let user = parse_config(MIXED.as_bytes()).unwrap();
    let project =
        parse_config(br#"{"mcpServers": {"secret": {"command": "curl", "args": ["evil"]}}}"#)
            .unwrap();

    let untrusted = merge_configs(user.clone(), Some(project.clone()), Some("sha256:a"), None);
    assert!(untrusted.project_untrusted);
    assert_eq!(untrusted.config.servers.len(), 6);
    assert!(untrusted.config.server("secret").is_none());
    assert!(
        untrusted
            .warnings
            .iter()
            .any(|warning| warning.contains("/mcp trust"))
    );

    let stale = merge_configs(
        user.clone(),
        Some(project.clone()),
        Some("sha256:b"),
        Some("sha256:a"),
    );
    assert!(stale.project_untrusted, "a changed file drops trust");
    assert!(stale.config.server("secret").is_none());

    let trusted = merge_configs(user, Some(project), Some("sha256:a"), Some("sha256:a"));
    assert!(!trusted.project_untrusted);
    assert!(trusted.config.server("secret").is_some());
}

#[test]
fn a_trusted_project_file_can_disable_but_not_enable() {
    let user = parse_config(MIXED.as_bytes()).unwrap();
    let project = parse_config(
        br#"{"mcpServers": {
            "docs": {"command": "docs-server", "enabled": false},
            "off": {"command": "x", "enabled": true}
        }}"#,
    )
    .unwrap();

    let outcome = merge_configs(user, Some(project), Some("sha256:a"), Some("sha256:a"));

    assert!(!outcome.config.server("docs").unwrap().enabled);
    assert!(
        !outcome.config.server("off").unwrap().enabled,
        "a project file must not enable a server the user disabled"
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("cannot enable"))
    );
}

#[test]
fn a_trusted_project_file_can_only_narrow_the_tool_allowlist() {
    let user = parse_config(
        br#"{"mcpServers": {"docs": {"command": "d", "tools": {"allow": ["search_*", "read_*"]}}}}"#,
    )
    .unwrap();
    let project = parse_config(
        br#"{"mcpServers": {"docs": {"command": "d", "tools": {"allow": ["search_issues", "write_*"]}}}}"#,
    )
    .unwrap();

    let outcome = merge_configs(user, Some(project), Some("sha256:a"), Some("sha256:a"));
    let tools = outcome
        .config
        .server("docs")
        .unwrap()
        .tools
        .clone()
        .unwrap();
    assert_eq!(tools.patterns(), ["search_issues".to_string()]);

    let allows = |tool: &str| tools.allows(tool);
    assert!(allows("search_issues"));
    assert!(!allows("read_file"), "user grant is narrowed away");
    assert!(!allows("write_x"), "project cannot widen past the user");
}

#[test]
fn an_empty_narrowed_allowlist_allows_nothing() {
    let user = parse_config(
        br#"{"mcpServers": {"docs": {"command": "d", "tools": {"allow": ["search_*"]}}}}"#,
    )
    .unwrap();
    let project = parse_config(
        br#"{"mcpServers": {"docs": {"command": "d", "tools": {"allow": ["write_*"]}}}}"#,
    )
    .unwrap();

    let outcome = merge_configs(user, Some(project), Some("sha256:a"), Some("sha256:a"));
    let tools = outcome
        .config
        .server("docs")
        .unwrap()
        .tools
        .clone()
        .unwrap();
    assert_eq!(tools, meerai_mcp::ToolFilter::Only(Vec::new()));
    assert!(
        !tools.allows("search_issues"),
        "an empty allowlist must reject every tool, not allow them all"
    );
    assert!(!tools.allows("anything"));
}

#[test]
fn a_project_file_cannot_widen_read_only() {
    let user =
        parse_config(br#"{"mcpServers": {"ro": {"command": "d", "read_only": true}}}"#).unwrap();
    let project =
        parse_config(br#"{"mcpServers": {"ro": {"command": "d", "read_only": false}}}"#).unwrap();

    let outcome = merge_configs(user, Some(project), Some("sha256:a"), Some("sha256:a"));
    assert!(outcome.config.server("ro").unwrap().read_only);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("cannot widen access"))
    );
}

#[test]
fn trust_store_is_hash_bound() {
    let mut settings = meerai_common::settings::McpSettings::default();
    let root = std::path::Path::new("/tmp/project");

    assert_eq!(settings.trusted_hash(root), None);
    assert!(settings.trust_project(root, "sha256:a"));
    assert_eq!(settings.trusted_hash(root), Some("sha256:a"));

    assert!(
        !settings.trust_project(root, "sha256:a"),
        "repeat is a no-op"
    );

    assert!(settings.revoke_project(root));
    assert_eq!(settings.trusted_hash(root), None);
    assert!(!settings.revoke_project(root));
}

#[test]
fn a_changed_file_hash_drops_trust_through_settings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let mut settings = meerai_common::settings::McpSettings::default();
    let (_, first_hash) = meerai_mcp::config::read_config_file(&write(dir.path(), MIXED)).unwrap();
    settings.trust_project(root, &first_hash);

    let user = meerai_mcp::config::parse_config(MIXED.as_bytes()).unwrap();
    let trusted = merge_configs(
        user.clone(),
        Some(meerai_mcp::config::parse_config(MIXED.as_bytes()).unwrap()),
        Some(&first_hash),
        settings.trusted_hash(root),
    );
    assert!(!trusted.project_untrusted);

    let changed = r#"{"mcpServers": {"injected": {"command": "curl"}}}"#;
    let (_, second_hash) =
        meerai_mcp::config::read_config_file(&write(dir.path(), changed)).unwrap();
    assert_ne!(first_hash, second_hash);

    let stale = merge_configs(
        user,
        Some(meerai_mcp::config::parse_config(changed.as_bytes()).unwrap()),
        Some(&second_hash),
        settings.trusted_hash(root),
    );
    assert!(
        stale.project_untrusted,
        "an edited .mcp.json must re-prompt"
    );
    assert!(stale.config.server("injected").is_none());
}

fn write(dir: &std::path::Path, contents: &str) -> std::path::PathBuf {
    let path = dir.join(MCP_CONFIG_FILE);
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn untrusted_project_servers_are_listed_but_not_connectable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, MIXED);
    let (config, hash) = meerai_mcp::config::read_config_file(&root.join(MCP_CONFIG_FILE)).unwrap();

    let report = meerai_mcp::build_setup(None, root, None);
    assert!(report.project_config_present, "the file exists on disk");
    assert!(report.project_untrusted);
    assert!(
        report.configs.iter().all(|server| !config
            .servers
            .iter()
            .any(|declared| declared.name == server.name)),
        "nothing from an untrusted file may be connectable"
    );
    assert_eq!(
        report.servers.len(),
        config.servers.len(),
        "untrusted servers must still be listed so the UI can explain why"
    );
    assert!(
        report
            .servers
            .iter()
            .all(|server| server.status == meerai_mcp::ServerStatus::Untrusted)
    );
    assert!(report.servers.iter().all(|server| {
        server
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("/mcp trust"))
    }));
    assert!(report.servers.iter().any(|server| server.name == "docs"));
    assert!(report.warnings.iter().any(|w| w.contains("/mcp trust")));

    let trusted = meerai_mcp::build_setup(None, root, Some(&hash));
    assert!(!trusted.project_untrusted);
    assert!(!trusted.configs.is_empty());
    assert!(
        trusted
            .servers
            .iter()
            .all(|server| server.status != meerai_mcp::ServerStatus::Untrusted)
    );
}

#[test]
fn an_untrusted_project_server_without_a_file_is_never_reported() {
    let dir = tempfile::tempdir().unwrap();
    let report = meerai_mcp::build_setup(None, dir.path(), None);
    assert!(!report.project_config_present);
    assert!(report.servers.is_empty());
    assert!(report.configs.is_empty());
}

#[test]
fn a_user_server_keeps_its_status_when_the_project_shadows_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        r#"{"mcpServers": {"docs": {"command": "project-docs"}}}"#,
    );

    let user_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        user_dir.path().join(MCP_CONFIG_FILE),
        r#"{"mcpServers": {"docs": {"command": "user-docs"}}}"#,
    )
    .unwrap();

    let report = meerai_mcp::build_setup(Some(user_dir.path()), root, None);
    assert_eq!(
        report.servers.len(),
        1,
        "a shadowed name must be listed once, not twice"
    );
    assert_eq!(
        report.servers[0].status,
        meerai_mcp::ServerStatus::Configured
    );
}

#[test]
fn a_configured_server_is_not_reported_as_connected() {
    let user_dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        user_dir.path().join(MCP_CONFIG_FILE),
        r#"{"mcpServers": {"docs": {"command": "docs-server"}}}"#,
    )
    .unwrap();

    let report = meerai_mcp::build_setup(Some(user_dir.path()), root.path(), None);
    assert_eq!(report.servers[0].status.label(), "configured");
    assert!(!report.servers[0].status.is_connected());
    assert!(report.servers[0].tools.is_empty());
}
