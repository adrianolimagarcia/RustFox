//! Portal control for the one built-in Google connector.
//!
//! `GET /api/connectors/google` says whether the button is shown.
//! `POST /api/connectors/google` starts public PKCE when a client id is
//! available (baked at compile time, or an advanced setting). Neither path
//! asks the user to create an OAuth client, and neither returns a
//! "not configured" error. The callback stores the token and starts the
//! HTTP MCP client. Not part of the setup wizard.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::PortalState;
use crate::google_mcp::{
    self, authorize_url, client_id_for_store, code_challenge_s256, finish_google_login,
    new_code_verifier, parse_token_response, redirect_uri, store_advanced_client_id, tap_body,
    token_form, GoogleToken, PendingLogin,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIdBody {
    #[serde(default)]
    client_id: String,
}

fn lock_states(
    st: &PortalState,
) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, PendingLogin>> {
    st.google_oauth_states
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether the Google button is shown. Does not echo the client id.
pub async fn status(State(st): State<PortalState>) -> Json<Value> {
    let offered = client_id_for_store(st.secret_store.as_ref())
        .ok()
        .flatten()
        .is_some();
    Json(json!({ "offered": offered }))
}

/// One tap. Hidden clients get `{"offered": false}` and no authorize URL.
pub async fn start(State(st): State<PortalState>) -> Json<Value> {
    let Some(client_id) = client_id_for_store(st.secret_store.as_ref()).ok().flatten() else {
        return Json(tap_body(false, None));
    };
    let state = uuid::Uuid::new_v4().simple().to_string();
    let verifier = new_code_verifier();
    let challenge = code_challenge_s256(&verifier);
    let url = match authorize_url(
        &client_id,
        &redirect_uri(st.config.port),
        &state,
        &challenge,
    ) {
        Ok(url) => url,
        Err(_) => return Json(tap_body(false, None)),
    };
    lock_states(&st).insert(
        state,
        PendingLogin {
            client_id,
            verifier,
        },
    );
    Json(tap_body(true, Some(&url)))
}

/// Advanced setting. Stores a desktop client id in SecretStore. Empty clears it.
/// The response does not repeat the id.
pub async fn set_client_id(
    State(st): State<PortalState>,
    Json(body): Json<ClientIdBody>,
) -> Result<Json<Value>, StatusCode> {
    store_advanced_client_id(st.secret_store.as_ref(), &body.client_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let offered = client_id_for_store(st.secret_store.as_ref())
        .ok()
        .flatten()
        .is_some();
    Ok(Json(json!({ "offered": offered })))
}

async fn exchange_code(
    code: &str,
    client_id: &str,
    redirect: &str,
    verifier: &str,
) -> anyhow::Result<GoogleToken> {
    let form = token_form(code, client_id, redirect, verifier);
    let http = reqwest::Client::new();
    let response = http.post(google_mcp::TOKEN_URL).form(&form).send().await?;
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
) -> Result<Html<&'static str>, (StatusCode, &'static str)> {
    if !query.error.is_empty() || query.code.is_empty() || query.state.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Google sign-in failed"));
    }
    let pending = lock_states(&st).remove(&query.state);
    let Some(pending) = pending else {
        return Err((StatusCode::BAD_REQUEST, "Google sign-in failed"));
    };
    let redirect = redirect_uri(st.config.port);
    let token = exchange_code(
        &query.code,
        &pending.client_id,
        &redirect,
        &pending.verifier,
    )
    .await
    .map_err(|_| (StatusCode::BAD_GATEWAY, "Google sign-in failed"))?;
    let runtime = finish_google_login(
        st.secret_store.as_ref(),
        st.config_path.as_path(),
        &token,
        &mut |_| Ok(()),
    )
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Google sign-in failed"))?;
    st.agent
        .start_google_connector(runtime)
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, "Google sign-in failed"))?;
    Ok(Html(
        "<!doctype html><title>Google</title><p>Google connected.</p>",
    ))
}
