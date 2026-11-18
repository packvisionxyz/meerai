pub mod authorize;
mod callback;
pub mod host;
pub mod login;
pub(crate) mod pkce;
pub mod refresh;
pub(crate) mod token;

pub use authorize::{AuthorizeParams, authorize_url};
pub use callback::{
    CallbackQuery, bind_listener, callback_redirect_uri, parse_callback_input, verify_state,
};
pub use login::{LoginInteraction, login_chatgpt};
pub use refresh::ensure_fresh_credential;
pub use token::revoke_chatgpt_credential;

use std::time::{SystemTime, UNIX_EPOCH};

pub const AUTHORIZE_ENDPOINT: &str = "https://auth.openai.com/api/accounts/authorize";
pub const TOKEN_ENDPOINT: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub const REVOKE_ENDPOINT: &str = "https://auth.openai.com/api/accounts/oauth/revoke";
pub const RESOURCE: &str = "https://api.openai.com/v1";
pub const SCOPE: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
pub const AGENT_NAME_HINT: &str = "meerai";
pub const CALLBACK_PATH: &str = "/auth/callback";
pub const PREFERRED_CALLBACK_PORT: u16 = 1455;

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn error_code(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(code) = value.get("error").and_then(|error| error.as_str()) {
        return Some(code.to_string());
    }
    if let Some(code) = value.pointer("/error/code").and_then(|code| code.as_str()) {
        return Some(code.to_string());
    }
    value
        .get("code")
        .and_then(|code| code.as_str())
        .map(str::to_string)
}

pub(crate) fn error_description(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    for pointer in ["/error_description", "/error/message", "/message"] {
        if let Some(description) = value.pointer(pointer).and_then(|text| text.as_str()) {
            return Some(description.to_string());
        }
    }
    None
}
