mod fetch;
mod search;
pub(crate) mod text;

use std::time::Duration;

use meerai_core::env::MEERAI_USER_AGENT;
use meerai_core::{
    EffectClass, PermissionAction, ToolDefinition, ToolError, ToolOutput, ToolPermissionSpec,
    ToolRun, Toolset, async_trait,
};
use reqwest::Client;

use crate::code::parse_args;

use self::fetch::FetchArgs;
use self::search::SearchArgs;

pub const TOOL_NAMES: [&str; 2] = ["fetch", "search"];

const TOOLSET_NAME: &str = "web";
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_OUTPUT_CHARS: usize = 60_000;

const SEARCH_ENDPOINTS: [&str; 2] = [
    "https://html.duckduckgo.com/html/",
    "https://lite.duckduckgo.com/lite/",
];

#[derive(Clone)]
pub struct WebTools {
    client: Option<Client>,
    allow_private_networks: bool,
    search_endpoints: Vec<String>,
}

impl Default for WebTools {
    fn default() -> Self {
        Self::new()
    }
}

impl WebTools {
    pub fn new() -> Self {
        Self {
            client: build_client(),
            allow_private_networks: false,
            search_endpoints: SEARCH_ENDPOINTS
                .iter()
                .map(|endpoint| endpoint.to_string())
                .collect(),
        }
    }

    pub fn with_client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    pub fn with_allow_private_networks(mut self, allow: bool) -> Self {
        self.allow_private_networks = allow;
        self
    }

    pub fn with_search_endpoints(mut self, endpoints: Vec<String>) -> Self {
        if !endpoints.is_empty() {
            self.search_endpoints = endpoints;
        }
        self
    }

    fn client(&self) -> Option<&Client> {
        self.client.as_ref()
    }

    fn allow_private_networks(&self) -> bool {
        self.allow_private_networks
    }

    fn search_endpoints(&self) -> &[String] {
        &self.search_endpoints
    }
}

fn build_client() -> Option<Client> {
    Client::builder()
        .timeout(FETCH_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(MEERAI_USER_AGENT)
        .build()
        .ok()
}

fn runtime_name(tool: &str) -> String {
    format!("{TOOLSET_NAME}-{tool}")
}

fn bare_name(fn_name: &str) -> Option<&str> {
    fn_name.strip_prefix(TOOLSET_NAME)?.strip_prefix('-')
}

#[async_trait]
impl Toolset for WebTools {
    fn name(&self) -> String {
        TOOLSET_NAME.to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![
            fetch::definition(&runtime_name("fetch")),
            search::definition(&runtime_name("search")),
        ]
    }

    fn contain(&self, fn_name: &str) -> bool {
        bare_name(fn_name).is_some_and(|name| TOOL_NAMES.contains(&name))
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::External
    }

    fn permission_spec(&self, fn_name: &str) -> Option<ToolPermissionSpec> {
        match bare_name(fn_name)? {
            "fetch" => Some(ToolPermissionSpec {
                action: PermissionAction::WebFetch,
                resource_arg: Some("url"),
            }),
            "search" => Some(ToolPermissionSpec {
                action: PermissionAction::WebFetch,
                resource_arg: Some("query"),
            }),
            _ => None,
        }
    }

    async fn invoke(&self, fn_name: &str, args: &str) -> Result<ToolOutput, ToolError> {
        Ok(self.invoke_structured(fn_name, args).await?.output)
    }

    async fn invoke_structured(&self, fn_name: &str, args: &str) -> Result<ToolRun, ToolError> {
        match bare_name(fn_name) {
            Some("fetch") => Ok(self.fetch(parse_args::<FetchArgs>(args)?).await),
            Some("search") => Ok(self.search(parse_args::<SearchArgs>(args)?).await),
            _ => Err(ToolError::InvalidFunctionName(fn_name.to_string())),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_server {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    pub(crate) struct TestServer {
        port: u16,
        stop: Arc<AtomicBool>,
        handle: JoinHandle<()>,
    }

    impl TestServer {
        pub(crate) fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }

        pub(crate) fn stop(&self) {
            self.stop.store(true, Ordering::SeqCst);
            self.handle.abort();
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    pub(crate) async fn spawn(responses: Vec<String>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = tokio::spawn(async move {
            for response in responses {
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = vec![0u8; 8192];
                let _ = stream.read(&mut buffer).await;
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        TestServer { port, stop, handle }
    }
}

#[cfg(test)]
mod tests {
    use meerai_core::{ObservationStatus, Toolset};

    use super::{TOOL_NAMES, WebTools, test_server};

    #[test]
    fn toolset_exposes_fetch_and_search_as_external_with_webfetch_permissions() {
        let tools = WebTools::new();
        let names: Vec<String> = tools
            .definition()
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        assert_eq!(names, vec!["web-fetch", "web-search"]);
        assert!(tools.contain("web-fetch"));
        assert!(tools.contain("web-search"));
        assert!(!tools.contain("code-read_file"));
        for name in TOOL_NAMES {
            let fn_name = format!("web-{name}");
            assert_eq!(
                tools.effect_of(&fn_name),
                meerai_core::EffectClass::External
            );
            let spec = tools.permission_spec(&fn_name).expect("permission spec");
            assert_eq!(spec.action, meerai_core::PermissionAction::WebFetch);
        }
        assert_eq!(
            tools
                .permission_spec("web-fetch")
                .and_then(|spec| spec.resource_arg),
            Some("url")
        );
        assert_eq!(
            tools
                .permission_spec("web-search")
                .and_then(|spec| spec.resource_arg),
            Some("query")
        );
    }

    #[test]
    fn unknown_function_names_are_rejected() {
        let tools = WebTools::new();
        assert!(!tools.contain("fetch"));
        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(tools.invoke_structured("web-missing", "{}"))
            .expect_err("unknown tool");
        assert!(matches!(
            error,
            meerai_core::ToolError::InvalidFunctionName(_)
        ));
    }

    #[tokio::test]
    async fn search_reads_a_local_endpoint_and_renders_results() {
        let page = "<html><body><a class=\"result__a\" href=\"https://example.com/doc\">Example docs</a><a class=\"result__snippet\">A useful page.</a></body></html>";
        let server = test_server::spawn(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
            page.len(),
            page
        )])
        .await;
        let tools = WebTools::new()
            .with_allow_private_networks(true)
            .with_search_endpoints(vec![server.url("/search")]);
        let run = tools
            .search(super::search::SearchArgs {
                query: "example".to_string(),
                limit: Some(3),
            })
            .await;
        assert_eq!(run.observation.status, ObservationStatus::Success);
        assert!(run.observation.stdout.contains("1. Example docs"));
        assert!(run.observation.stdout.contains("https://example.com/doc"));
        server.stop();
    }

    #[tokio::test]
    async fn search_reports_a_blocked_provider() {
        let page = "<div class=\"anomaly-modal\">Unfortunately, bots use DuckDuckGo too.</div>";
        let server = test_server::spawn(vec![format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
            page.len(),
            page
        )])
        .await;
        let tools = WebTools::new()
            .with_allow_private_networks(true)
            .with_search_endpoints(vec![server.url("/search")]);
        let run = tools
            .search(super::search::SearchArgs {
                query: "example".to_string(),
                limit: None,
            })
            .await;
        assert_eq!(run.observation.status, ObservationStatus::Error);
        assert!(run.observation.stderr.contains("blocked"));
        server.stop();
    }
}
