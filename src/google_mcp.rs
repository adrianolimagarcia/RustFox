//! Built-in Google connector.
//!
//! One service, labeled "Google". The remote endpoint is Google's official
//! Gmail MCP server (`https://gmailmcp.googleapis.com/mcp/v1`, Streamable HTTP),
//! documented at
//! <https://developers.google.com/workspace/gmail/api/guides/configure-mcp-server>.
//! Google publishes product MCP URLs, not one umbrella host; this is the
//! documented remote server this process can speak without Node. Scopes are
//! the two listed on that page.
//!
//! Sign-in uses Google's OAuth authorization endpoint
//! (`https://accounts.google.com/o/oauth2/v2/auth`) and token endpoint
//! (`https://oauth2.googleapis.com/token`), from
//! <https://developers.google.com/identity/protocols/oauth2/web-server>.
//! The access token is stored in SecretStore. `config.toml` keeps
//! `secret:google.mcp.token` only. The connector is the existing HTTP MCP
//! client: a URL and a bearer, no command and no argv.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

use crate::config::McpServerConfig;
use crate::config_edit::write_config_validated;
use crate::secret_store::{parse_secret_ref, SecretStore, SECRET_REF_PREFIX};

/// User-facing service name. The portal offers this and nothing else.
pub const SERVICE_LABEL: &str = "Google";

/// Internal MCP server id. Not typed by the user.
pub const SERVER_NAME: &str = "google";

/// SecretStore name for the Google access token.
pub const SECRET_NAME: &str = "google.mcp.token";

/// Official Gmail remote MCP endpoint (Streamable HTTP).
pub const MCP_URL: &str = "https://gmailmcp.googleapis.com/mcp/v1";

/// Official Google OAuth authorization endpoint.
pub const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Official Google OAuth token endpoint.
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Scopes named by the Gmail MCP configuration guide.
pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/gmail.compose",
];

/// The only built-in connector. Fit and every other service stay out.
pub fn built_in_services() -> &'static [&'static str] {
    &[SERVICE_LABEL]
}

pub fn secret_ref() -> String {
    format!("{SECRET_REF_PREFIX}{SECRET_NAME}")
}

pub fn redirect_uri(portal_port: u16) -> String {
    format!("http://127.0.0.1:{portal_port}/api/connectors/google/callback")
}

/// Browser login URL. No server name, MCP URL, or command is a query the user types.
pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str) -> Result<String> {
    let client_id = client_id.trim();
    if client_id.is_empty() {
        bail!("Google sign-in is not configured");
    }
    if state.trim().is_empty() {
        bail!("OAuth state is empty");
    }
    let mut url = reqwest::Url::parse(AUTHORIZE_URL).context("authorize URL")?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", &SCOPES.join(" "))
            .append_pair("state", state)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
    }
    Ok(url.into())
}

/// Access token returned by Google's token endpoint. Refresh tokens, when
/// present, stay in SecretStore under a separate name and never go to chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

pub fn parse_token_response(body: &str) -> Result<GoogleToken> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("token response is not JSON")?;
    let access = value
        .get("access_token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("token response missing access_token")?
        .to_string();
    let refresh = value
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Ok(GoogleToken {
        access_token: access,
        refresh_token: refresh,
    })
}

/// HTTP MCP config for the live client. Bearer is the stored token, not a
/// command line. `command` stays unset.
pub fn runtime_config(access_token: &str) -> Result<McpServerConfig> {
    let access_token = access_token.trim();
    if access_token.is_empty() {
        bail!("Google access token is empty");
    }
    Ok(McpServerConfig {
        name: SERVER_NAME.to_string(),
        enabled: true,
        command: None,
        args: Vec::new(),
        env: HashMap::new(),
        url: Some(MCP_URL.to_string()),
        auth_token: Some(access_token.to_string()),
        refresh_token: None,
        token_expires_at: None,
        token_endpoint: None,
        oauth_client_id: None,
        oauth_client_secret: None,
    })
}

fn upsert_google_server(doc: &mut toml::Value) -> Result<()> {
    let root = doc
        .as_table_mut()
        .context("config.toml root is not a table")?;
    let servers = root
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    let Some(servers) = servers.as_array_mut() else {
        bail!("mcp_servers is not an array");
    };
    let mut table = toml::map::Map::new();
    table.insert("name".into(), toml::Value::String(SERVER_NAME.into()));
    table.insert("enabled".into(), toml::Value::Boolean(true));
    table.insert("url".into(), toml::Value::String(MCP_URL.into()));
    table.insert("auth_token".into(), toml::Value::String(secret_ref()));
    let replacement = toml::Value::Table(table);
    if let Some(existing) = servers.iter_mut().find(|server| {
        server
            .get("name")
            .and_then(|v| v.as_str())
            .is_some_and(|name| name == SERVER_NAME)
    }) {
        *existing = replacement;
    } else {
        servers.push(replacement);
    }
    Ok(())
}

/// Write `secret:google.mcp.token` onto the Google MCP row. Does not write
/// the access token, a command, or argv.
pub fn persist_secret_ref(config_path: &Path) -> Result<()> {
    let original = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let mut doc: toml::Value = toml::from_str(&original).context("parse config.toml")?;
    upsert_google_server(&mut doc)?;
    let text = toml::to_string_pretty(&doc).context("serialize config.toml")?;
    write_config_validated(config_path, &text)?;
    Ok(())
}

/// Callback handoff: SecretStore, then config ref, then the HTTP connector.
/// `start` is `McpManager::connect` in the portal and a fake in tests.
pub fn finish_google_login(
    store: &dyn SecretStore,
    config_path: &Path,
    token: &GoogleToken,
    start: &mut dyn FnMut(&McpServerConfig) -> Result<()>,
) -> Result<McpServerConfig> {
    let access = token.access_token.trim();
    if access.is_empty() {
        bail!("Google access token is empty");
    }
    store.set(SECRET_NAME, access)?;
    if let Some(refresh) = token.refresh_token.as_deref() {
        store.set("google.mcp.refresh", refresh)?;
    }
    persist_secret_ref(config_path)?;
    let runtime = runtime_config(access)?;
    debug_assert!(runtime.command.is_none());
    debug_assert!(runtime.args.is_empty());
    start(&runtime)?;
    Ok(runtime)
}

/// True when the on-disk Google row points at the official URL via SecretStore
/// and does not carry a process command.
pub fn google_row_is_http_secret(config_toml: &str, plaintext_token: &str) -> Result<bool> {
    if config_toml.contains(plaintext_token) {
        return Ok(false);
    }
    let doc: toml::Value = toml::from_str(config_toml).context("parse config")?;
    let Some(servers) = doc.get("mcp_servers").and_then(|v| v.as_array()) else {
        return Ok(false);
    };
    let Some(row) = servers
        .iter()
        .find(|server| server.get("name").and_then(|v| v.as_str()) == Some(SERVER_NAME))
    else {
        return Ok(false);
    };
    let url_ok = row.get("url").and_then(|v| v.as_str()) == Some(MCP_URL);
    let auth = row.get("auth_token").and_then(|v| v.as_str()).unwrap_or("");
    let secret_ok = parse_secret_ref(auth) == Some(SECRET_NAME);
    let no_command = row.get("command").and_then(|v| v.as_str()).is_none();
    let no_args = match row.get("args") {
        None => true,
        Some(toml::Value::Array(items)) => items.is_empty(),
        Some(_) => false,
    };
    Ok(url_ok && secret_ok && no_command && no_args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::FakeSecretStore;
    use crate::setup::thin::{wizard_fields, ThinProvider};
    use std::sync::{Arc, Mutex};

    fn minimal_config() -> String {
        r#"
[telegram]
bot_token = "123:abc"
allowed_user_ids = [42]

[openrouter]
api_key = "k"
model = "moonshotai/kimi-k2.6"
"#
        .to_string()
    }

    #[test]
    fn only_google_is_offered() {
        assert_eq!(built_in_services(), &["Google"]);
        let joined = built_in_services().join(" ").to_ascii_lowercase();
        assert!(!joined.contains("fit"));
        assert!(!joined.contains("drive"));
        assert!(!joined.contains("calendar"));
    }

    #[test]
    fn wizard_fields_do_not_ask_about_mcp() {
        for provider in [ThinProvider::OpenRouter, ThinProvider::Ollama] {
            let fields = wizard_fields(provider);
            assert!(
                fields
                    .iter()
                    .all(|field| !field.to_ascii_lowercase().contains("mcp")),
                "{fields:?}"
            );
            assert!(!fields.contains(&"command"));
            assert!(!fields.contains(&"url"));
        }
    }

    #[test]
    fn authorize_url_is_the_official_google_endpoint() {
        let url = authorize_url(
            "test-client.apps.googleusercontent.com",
            &redirect_uri(8090),
            "state-1",
        )
        .unwrap();
        assert!(url.starts_with(AUTHORIZE_URL), "{url}");
        assert!(url.contains("client_id=test-client.apps.googleusercontent.com"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("gmail.readonly"));
        assert!(url.contains("gmail.compose"));
        assert!(url.contains("redirect_uri="));
        assert!(!url.contains("npx"));
        assert!(!url.contains("uvx"));
        assert!(authorize_url("", "http://127.0.0.1/cb", "s").is_err());
    }

    #[test]
    fn callback_stores_secret_and_starts_http_connector_without_a_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, minimal_config()).unwrap();
        let store = FakeSecretStore::new();
        let seen: Arc<Mutex<Option<McpServerConfig>>> = Arc::new(Mutex::new(None));
        let seen_start = Arc::clone(&seen);
        let token = parse_token_response(
            r#"{"access_token":"ya29.test-token","refresh_token":"1//refresh","token_type":"Bearer"}"#,
        )
        .unwrap();
        let runtime = finish_google_login(&store, &path, &token, &mut |cfg| {
            *seen_start.lock().unwrap() = Some(cfg.clone());
            Ok(())
        })
        .unwrap();

        let stored = store.get(SECRET_NAME).unwrap().unwrap();
        assert_eq!(stored.expose(), "ya29.test-token");
        assert_eq!(
            store.get("google.mcp.refresh").unwrap().unwrap().expose(),
            "1//refresh"
        );
        assert!(runtime.command.is_none());
        assert!(runtime.args.is_empty());
        assert_eq!(runtime.url.as_deref(), Some(MCP_URL));
        assert_eq!(runtime.auth_token.as_deref(), Some("ya29.test-token"));
        assert!(runtime.env.is_empty());

        let started = seen.lock().unwrap().clone().unwrap();
        assert!(started.command.is_none());
        assert!(started.args.is_empty());
        assert_eq!(started.url.as_deref(), Some(MCP_URL));

        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(google_row_is_http_secret(&disk, "ya29.test-token").unwrap());
        assert!(!disk.contains("ya29.test-token"));
        assert!(!disk.contains("1//refresh"));
        assert!(!disk.contains("uvx"));
        assert!(!disk.contains("npx"));
    }
}
