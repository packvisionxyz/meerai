use std::collections::BTreeMap;
use std::sync::Arc;

use meerai_mcp::client::{EnvResolver, SpawnPlan, spawn_plan};
use meerai_mcp::config::{McpTransport, parse_config};

const OPENCODE: &str = r#"{
  "mcpServers": {
    "everything": {
      "type": "local",
      "command": ["npx", "-y", "@modelcontextprotocol/server-everything"],
      "cwd": ".",
      "environment": {
        "LOG_LEVEL": "info",
        "MCP_API_KEY": "{env:MCP_API_KEY}"
      }
    },
    "remote-docs": {
      "type": "remote",
      "url": "https://mcp.example.test/v1",
      "headers": { "Authorization": "Bearer {env:DOCS_TOKEN}" }
    }
  }
}"#;

fn resolver(pairs: &[(&str, &str)]) -> EnvResolver {
    let owned: BTreeMap<String, String> = pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    Arc::new(move |name: &str| owned.get(name).cloned())
}

fn stdio_server(name: &str, raw: &str) -> meerai_mcp::McpServerConfig {
    parse_config(raw.as_bytes())
        .expect("parse")
        .server(name)
        .cloned()
        .expect("server")
}

#[test]
fn parses_the_opencode_shape_with_a_command_array_and_environment() {
    let config = parse_config(OPENCODE.as_bytes()).unwrap();
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);

    let everything = config.server("everything").unwrap();
    assert_eq!(everything.kind(), "stdio");
    let McpTransport::Stdio {
        command,
        args,
        env,
        cwd,
    } = &everything.transport
    else {
        panic!("expected stdio, got {:?}", everything.transport);
    };
    assert_eq!(command, "npx");
    assert_eq!(
        args,
        &vec![
            "-y".to_string(),
            "@modelcontextprotocol/server-everything".to_string()
        ]
    );
    assert_eq!(cwd.as_deref(), Some("."));
    assert_eq!(env.get("LOG_LEVEL").map(String::as_str), Some("info"));
    assert_eq!(
        env.get("MCP_API_KEY").map(String::as_str),
        Some("{env:MCP_API_KEY}"),
        "the template must stay unexpanded in the parsed config"
    );

    let remote = config.server("remote-docs").unwrap();
    assert_eq!(remote.kind(), "http");
    let McpTransport::Http { url, headers } = &remote.transport else {
        panic!("expected http, got {:?}", remote.transport);
    };
    assert_eq!(url, "https://mcp.example.test/v1");
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer {env:DOCS_TOKEN}")
    );
}

#[test]
fn a_string_command_still_works() {
    let server = stdio_server(
        "docs",
        r#"{"mcpServers": {"docs": {"command": "docs-server", "args": ["--stdio"]}}}"#,
    );
    let McpTransport::Stdio { command, args, .. } = &server.transport else {
        panic!("expected stdio");
    };
    assert_eq!(command, "docs-server");
    assert_eq!(args, &vec!["--stdio".to_string()]);
}

#[test]
fn explicit_args_are_appended_to_the_command_array_tail() {
    let server = stdio_server(
        "mix",
        r#"{"mcpServers": {"mix": {"command": ["npx", "-y", "srv"], "args": ["--verbose"]}}}"#,
    );
    let McpTransport::Stdio { args, .. } = &server.transport else {
        panic!("expected stdio");
    };
    assert_eq!(
        args,
        &vec!["-y".to_string(), "srv".to_string(), "--verbose".to_string()]
    );
}

#[test]
fn a_local_server_without_a_command_is_rejected() {
    let config = parse_config(br#"{"mcpServers": {"x": {"type": "local"}}}"#).unwrap();
    assert!(config.servers.is_empty());
    assert_eq!(config.warnings.len(), 1);
    assert!(
        config.warnings[0].contains("declares no 'command'"),
        "{:?}",
        config.warnings
    );
}

#[test]
fn a_remote_server_without_a_url_is_rejected() {
    let config = parse_config(br#"{"mcpServers": {"x": {"type": "remote"}}}"#).unwrap();
    assert!(config.servers.is_empty());
    assert_eq!(config.warnings.len(), 1);
    assert!(
        config.warnings[0].contains("declares no 'url'"),
        "{:?}",
        config.warnings
    );
}

#[test]
fn expansion_happens_at_spawn_time_not_parse_time() {
    let server = stdio_server("everything", OPENCODE);
    let env = resolver(&[("MCP_API_KEY", "sk-secret")]);

    let plan: SpawnPlan = spawn_plan(&server, &env, None).unwrap();

    assert_eq!(plan.program, "npx");
    assert_eq!(
        plan.env.get("MCP_API_KEY").map(String::as_str),
        Some("sk-secret")
    );
    assert_eq!(plan.env.get("LOG_LEVEL").map(String::as_str), Some("info"));

    let McpTransport::Stdio { env: raw_env, .. } = &server.transport else {
        panic!("expected stdio");
    };
    assert_eq!(
        raw_env.get("MCP_API_KEY").map(String::as_str),
        Some("{env:MCP_API_KEY}"),
        "the parsed config must never hold the secret"
    );
    assert!(!format!("{server:?}").contains("sk-secret"));
}

#[test]
fn expansion_reaches_command_and_args() {
    let server = stdio_server(
        "wrapped",
        r#"{"mcpServers": {"wrapped": {
            "command": ["${MCP_BIN}", "--token", "${MCP_TOKEN}"],
            "env": {"PATH_DIR": "${MCP_BIN|/usr/bin}"}
        }}}"#,
    );
    let env = resolver(&[("MCP_BIN", "/opt/mcp"), ("MCP_TOKEN", "tok-123")]);

    let plan = spawn_plan(&server, &env, None).unwrap();
    assert_eq!(plan.program, "/opt/mcp");
    assert_eq!(
        plan.args,
        vec!["--token".to_string(), "tok-123".to_string()]
    );
    assert_eq!(
        plan.env.get("PATH_DIR").map(String::as_str),
        Some("/opt/mcp")
    );
}

#[test]
fn a_missing_command_variable_fails_clearly() {
    let server = stdio_server(
        "missing",
        r#"{"mcpServers": {"missing": {"command": ["${NOT_SET_BIN}"]}}}"#,
    );
    let error = spawn_plan(&server, &resolver(&[]), None).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("command is empty after environment expansion"),
        "{message}"
    );
}

#[test]
fn cwd_is_expanded_and_resolved_against_the_base_dir() {
    let server = stdio_server(
        "withcwd",
        r#"{"mcpServers": {"withcwd": {"command": "srv", "cwd": "${SUB_DIR|.}/sub"}}}"#,
    );
    let base = std::path::Path::new("/tmp/project");
    let plan = spawn_plan(&server, &resolver(&[]), Some(base)).unwrap();
    assert_eq!(plan.cwd, Some(base.join(".").join("sub")));

    let absolute = stdio_server(
        "abs",
        r#"{"mcpServers": {"abs": {"command": "srv", "cwd": "/var/tmp"}}}"#,
    );
    let plan = spawn_plan(&absolute, &resolver(&[]), Some(base)).unwrap();
    assert_eq!(plan.cwd, Some(std::path::PathBuf::from("/var/tmp")));
}

#[test]
fn no_cwd_falls_back_to_the_base_dir() {
    let server = stdio_server("plain", r#"{"mcpServers": {"plain": {"command": "srv"}}}"#);
    let base = std::path::Path::new("/tmp/project");
    let plan = spawn_plan(&server, &resolver(&[]), Some(base)).unwrap();
    assert_eq!(plan.cwd, Some(base.to_path_buf()));
    assert_eq!(plan.args, Vec::<String>::new());
}

#[test]
fn spawn_plan_rejects_a_non_stdio_server() {
    let remote = stdio_server(
        "remote",
        r#"{"mcpServers": {"remote": {"type": "remote", "url": "https://x.test"}}}"#,
    );
    let error = spawn_plan(&remote, &resolver(&[]), None).unwrap_err();
    assert!(error.to_string().contains("not a stdio server"), "{error}");
}
