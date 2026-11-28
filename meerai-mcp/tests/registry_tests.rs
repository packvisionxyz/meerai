use meerai_mcp::config::parse_config;
use meerai_mcp::registry::McpRegistry;

fn servers(raw: &str) -> Vec<meerai_mcp::McpServerConfig> {
    parse_config(raw.as_bytes()).unwrap().servers
}

const RAW: &str = r#"{"mcpServers": {
  "docs": {"command": "docs-server", "read_only": true},
  "writer": {"command": "writer-server"},
  "legacy": {"type": "sse", "url": "https://x.test/sse"},
  "off": {"command": "off-server", "enabled": false}
}}"#;

#[test]
fn a_registry_orders_servers_by_name() {
    let registry = McpRegistry::new(servers(RAW), "/tmp/work");
    let names: Vec<&str> = registry.servers().iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["docs", "legacy", "off", "writer"]);
    assert!(!registry.is_empty());
    assert_eq!(registry.workspace(), std::path::Path::new("/tmp/work"));
}

#[test]
fn an_empty_registry_reports_no_servers() {
    let registry = McpRegistry::empty("/tmp/work");
    assert!(registry.is_empty());
    assert!(registry.servers().is_empty());
    assert!(registry.reports().is_empty());
}

#[test]
fn reports_classify_disabled_and_unsupported_servers() {
    let registry = McpRegistry::new(servers(RAW), "/tmp/work");
    let reports = registry.reports();
    assert_eq!(reports.len(), 4);

    let by_name = |name: &str| {
        reports
            .iter()
            .find(|report| report.name == name)
            .unwrap_or_else(|| panic!("missing report for {name}"))
    };

    assert_eq!(by_name("docs").status, meerai_mcp::ServerStatus::Configured);
    assert_eq!(
        by_name("writer").status,
        meerai_mcp::ServerStatus::Configured
    );
    assert_eq!(
        by_name("legacy").status,
        meerai_mcp::ServerStatus::Unsupported
    );
    assert_eq!(by_name("off").status, meerai_mcp::ServerStatus::Disabled);
    assert!(!by_name("off").enabled);
    assert_eq!(by_name("legacy").kind, "sse");
}

#[test]
fn nothing_is_cached_before_a_connection() {
    let registry = McpRegistry::new(servers(RAW), "/tmp/work");
    for name in ["docs", "writer", "legacy", "off"] {
        assert!(registry.cached(name).is_none(), "{name}");
        assert!(registry.failure(name).is_none(), "{name}");
    }
}

#[test]
fn a_failed_connection_is_recorded_as_a_failure() {
    let registry = McpRegistry::new(servers(RAW), "/tmp/work");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let connected = runtime.block_on(registry.toolsets());
    assert!(connected.is_empty());
    let reports = registry.reports();
    let by_name = |name: &str| {
        reports
            .iter()
            .find(|report| report.name == name)
            .unwrap_or_else(|| panic!("missing report for {name}"))
    };
    assert_eq!(
        by_name("docs").status,
        meerai_mcp::ServerStatus::Failed,
        "a connectable server that cannot connect must report failed, not configured"
    );
    assert!(by_name("docs").detail.is_some());
}

#[test]
fn a_clone_shares_the_same_state() {
    let registry = McpRegistry::new(servers(RAW), "/tmp/work");
    let clone = registry.clone();
    assert_eq!(clone.servers().len(), registry.servers().len());
    assert_eq!(clone.workspace(), registry.workspace());
}

#[test]
fn plan_mode_filters_to_read_only_servers() {
    let all = servers(RAW);
    let read_only: Vec<_> = all.into_iter().filter(|server| server.read_only).collect();
    assert_eq!(read_only.len(), 1);
    assert_eq!(read_only[0].name, "docs");
}

#[test]
fn unknown_servers_are_rejected_by_lookup() {
    let registry = McpRegistry::new(servers(RAW), "/tmp/work");
    assert!(registry.server("docs").is_some());
    assert!(registry.server("nope").is_none());
}
