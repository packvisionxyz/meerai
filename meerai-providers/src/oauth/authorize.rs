use crate::oauth::{AUTHORIZE_ENDPOINT, RESOURCE, SCOPE};

pub struct AuthorizeParams<'a> {
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub nonce: &'a str,
    pub code_challenge: &'a str,
    pub host_id: &'a str,
    pub agent_name_hint: Option<&'a str>,
}

pub fn authorize_url(params: &AuthorizeParams<'_>) -> String {
    let mut fields = vec![
        ("client_id", params.client_id),
        ("response_type", "code"),
        ("redirect_uri", params.redirect_uri),
        ("resource", RESOURCE),
        ("scope", SCOPE),
        ("state", params.state),
        ("nonce", params.nonce),
        ("code_challenge", params.code_challenge),
        ("code_challenge_method", "S256"),
    ];
    if let Some(hint) = params.agent_name_hint {
        fields.push(("agent_name_hint", hint));
    }
    fields.push(("ext_agent_host_id", params.host_id));

    let query = fields
        .into_iter()
        .map(|(key, value)| format!("{}={}", encode_component(key), encode_component(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{AUTHORIZE_ENDPOINT}?{query}")
}

fn encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::{AGENT_NAME_HINT, DYNAMIC_CLIENT_ID, PREFERRED_CALLBACK_PORT};

    fn params<'a>(client_id: &'a str, redirect_uri: &'a str) -> AuthorizeParams<'a> {
        AuthorizeParams {
            client_id,
            redirect_uri,
            state: "state-token",
            nonce: "nonce-token",
            code_challenge: "challenge-token",
            host_id: "urn:uuid:00000000-0000-4000-8000-000000000000",
            agent_name_hint: Some(AGENT_NAME_HINT),
        }
    }

    #[test]
    fn new_registration_url_carries_every_required_parameter() {
        let redirect_uri = crate::oauth::callback_redirect_uri(PREFERRED_CALLBACK_PORT);
        let url = authorize_url(&params(DYNAMIC_CLIENT_ID, &redirect_uri));

        assert!(url.starts_with("https://auth.openai.com/api/accounts/authorize?"));
        assert!(url.contains("client_id=dynamic_agent_client"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback"));
        assert!(url.contains("resource=https%3A%2F%2Fapi.openai.com%2Fv1"));
        assert!(url.contains("scope=openid%20profile%20email%20offline_access%20resource.invoke%20chatgpt.tokens.use.direct"));
        assert!(url.contains("state=state-token"));
        assert!(url.contains("nonce=nonce-token"));
        assert!(url.contains("code_challenge=challenge-token"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("agent_name_hint=meerai"));
        assert!(
            url.contains("ext_agent_host_id=urn%3Auuid%3A00000000-0000-4000-8000-000000000000")
        );
    }

    #[test]
    fn the_redirect_uri_uses_the_actual_bound_port() {
        let redirect_uri = crate::oauth::callback_redirect_uri(51234);
        let url = authorize_url(&params(DYNAMIC_CLIENT_ID, &redirect_uri));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A51234%2Fauth%2Fcallback"));
    }

    #[test]
    fn existing_registration_reuses_the_client_id_without_the_agent_hint() {
        let redirect_uri = crate::oauth::callback_redirect_uri(PREFERRED_CALLBACK_PORT);
        let mut existing = params("oaiapp_existing", &redirect_uri);
        existing.agent_name_hint = None;
        let url = authorize_url(&existing);

        assert!(url.contains("client_id=oaiapp_existing"));
        assert!(!url.contains("agent_name_hint"));
    }
}
