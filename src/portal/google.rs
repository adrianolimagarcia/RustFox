//! Portal control for the one built-in Google connector.
//!
//! `POST /api/connectors/google` (signed in) returns Google's authorize URL.
//! `GET /api/connectors/google/callback` (public; `state` must match the tap)
//! exchanges the code, stores the token in SecretStore, and starts the HTTP
//! MCP client. Not part of the setup wizard.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::PortalState;
use crate::google_mcp::{
    self, authorize_url, finish_google_login, parse_token_response, redirect_uri, GoogleToken,
};

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    code: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    error: String,
}

fn oauth_client() -> Result<(String, String), (StatusCode, Json<Value>)> {
    let id = std::env::var("RUSTFOX_GOOGLE_OAUTH_CLIENT_ID").unwrap_or_default();
    let secret = std::env::var("RUSTFOX_GOOGLE_OAUTH_CLIENT_SECRET").unwrap_or_default();
    if id.trim().is_empty() || secret.trim().is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": {
                    "code": "google_not_configured",
                    "message": "Google sign-in is not configured"
                }
            })),
        ));
    }
    Ok((id.trim().to_string(), secret.trim().to_string()))
}

/// One tap. The body is only the service label and the authorize URL.
pub async fn start(
    State(st): State<PortalState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (client_id, _secret) = oauth_client()?;
    let state = uuid::Uuid::new_v4().simple().to_string();
    let url = authorize_url(&client_id, &redirect_uri(st.config.port), &state).map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": {
                    "code": "google_not_configured",
                    "message": "Google sign-in is not configured"
                }
            })),
        )
    })?;
    st.google_oauth_states
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(state);
    Ok(Json(json!({
        "service": google_mcp::SERVICE_LABEL,
        "authorizeUrl": url,
    })))
}

async fn exchange_code(
    code: &str,
    client_id: &str,
    client_secret: &str,
    redirect: &str,
) -> anyhow::Result<GoogleToken> {
    let http = reqwest::Client::new();
    let response = http
        .post(google_mcp::TOKEN_URL)
        .form(&[
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("Google token endpoint returned {status}");
    }
    parse_token_response(&body)
}

pub async fn callback(
    State(st): State<PortalState>,
    Query(query): Query<CallbackQuery>,
) -> Result<Html<&'static str>, (StatusCode, String)> {
    if !query.error.is_empty() || query.code.is_empty() || query.state.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Google sign-in failed".into()));
    }
    let known = st
        .google_oauth_states
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&query.state);
    if !known {
        return Err((StatusCode::BAD_REQUEST, "Google sign-in failed".into()));
    }
    let (client_id, client_secret) = match oauth_client() {
        Ok(pair) => pair,
        Err(_) => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Google sign-in is not configured".into(),
            ))
        }
    };
    let redirect = redirect_uri(st.config.port);
    let token = exchange_code(&query.code, &client_id, &client_secret, &redirect)
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Google sign-in failed".into()))?;
    let runtime = finish_google_login(
        st.secret_store.as_ref(),
        st.config_path.as_path(),
        &token,
        &mut |_| Ok(()),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Google sign-in failed".into(),
        )
    })?;
    st.agent
        .start_google_connector(runtime)
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Google sign-in failed".into()))?;
    Ok(Html(
        "<!doctype html><title>Google</title><p>Google connected.</p>",
    ))
}
