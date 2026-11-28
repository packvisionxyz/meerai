use meerai_mcp::config::parse_config;
use meerai_mcp::registry::McpRegistry;

const NOISY: &str = r#"{"mcpServers": {
  "noisy": {"command": "/bin/sh", "args": ["-c", "echo noisy-launcher-line >&2; exit 3"]}
}}"#;

#[test]
fn a_failed_stdio_server_reports_its_stderr_instead_of_writing_to_the_terminal() {
    let servers = parse_config(NOISY.as_bytes()).unwrap().servers;
    let registry = McpRegistry::new(servers, std::env::temp_dir());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let connected = runtime.block_on(registry.toolsets());
    assert!(connected.is_empty());
    let report = registry
        .reports()
        .into_iter()
        .find(|report| report.name == "noisy")
        .expect("report");
    let detail = report.detail.expect("detail");
    assert!(detail.contains("noisy-launcher-line"), "{detail}");
}
