//! Portal pending-secret claim API (secure-store Slice 2).
//!
//! Claim routes are reachable with the opaque one-shot token alone (magic
//! link from Telegram) — no portal session required. Creating / listing
//! pending claims requires auth.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::error::PortalError;
use super::PortalState;
use crate::secret_store::secret_request_claim_url;

#[derive(Debug, Deserialize)]
pub struct CreatePendingBody {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SubmitClaimBody {
    pub value: String,
}

/// POST /api/secrets/pending — auth required. Creates a pending claim.
pub async fn create_pending(
    State(state): State<PortalState>,
    Json(body): Json<CreatePendingBody>,
) -> Result<Json<Value>, PortalError> {
    let created = state
        .pending_secrets
        .request(body.name.trim())
        .map_err(|e| PortalError::bad_request("invalid_secret_name", e.to_string()))?;

    let base = portal_base_url(&state);
    let claim_url = secret_request_claim_url(&base, &created.claim_token);

    // Return claimToken once to the authenticated portal client so it can
    // build links / tests. Telegram notify must use format helpers that keep
    // the raw token out of message *text*.
    Ok(Json(json!({
        "id": created.id,
        "name": created.name,
        "expiresAt": created.expires_at_unix,
        "claimUrl": claim_url,
        "claimToken": created.claim_token,
    })))
}

/// GET /api/secrets/pending — auth required. List pending (no tokens).
pub async fn list_pending(State(state): State<PortalState>) -> Result<Json<Value>, PortalError> {
    let items: Vec<Value> = state
        .pending_secrets
        .list()
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "expiresAt": p.expires_at_unix,
            })
        })
        .collect();
    Ok(Json(json!(items)))
}

/// DELETE /api/secrets/pending/{id} — auth required. Cancel by id.
pub async fn cancel_pending_by_id(
    State(state): State<PortalState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, PortalError> {
    let removed = state.pending_secrets.cancel_by_id(&id);
    if !removed {
        return Err(PortalError::not_found("pending secret"));
    }
    Ok(Json(json!({ "cancelled": true })))
}

/// GET /api/secrets/claim/{token} — public (token is the credential).
pub async fn get_claim(
    State(state): State<PortalState>,
    Path(token): Path<String>,
) -> Result<Json<Value>, PortalError> {
    let view = state
        .pending_secrets
        .peek(&token)
        .map_err(|_| PortalError::not_found("pending claim"))?;
    Ok(Json(json!({
        "id": view.id,
        "name": view.name,
        "expiresAt": view.expires_at_unix,
        "status": "pending",
    })))
}

/// POST /api/secrets/claim/{token} — public. Body `{ value }` → SecretStore::set.
pub async fn submit_claim(
    State(state): State<PortalState>,
    Path(token): Path<String>,
    Json(body): Json<SubmitClaimBody>,
) -> Result<Json<Value>, PortalError> {
    let value = body.value;
    if value.is_empty() {
        return Err(PortalError::bad_request(
            "empty_value",
            "secret value must not be empty",
        ));
    }
    // Peek first for a friendly name in the response / errors.
    let view = state
        .pending_secrets
        .peek(&token)
        .map_err(|_| PortalError::not_found("pending claim"))?;
    state
        .pending_secrets
        .submit(&token, &value, state.secret_store.as_ref())
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("expired") {
                PortalError::not_found("pending claim")
            } else {
                PortalError::internal(e)
            }
        })?;
    Ok(Json(json!({
        "ok": true,
        "name": view.name,
        "stored": true,
    })))
}

/// POST /api/secrets/claim/{token}/cancel — public (holder of token may cancel).
pub async fn cancel_claim(
    State(state): State<PortalState>,
    Path(token): Path<String>,
) -> Result<Json<Value>, PortalError> {
    let removed = state.pending_secrets.cancel(&token);
    if !removed {
        return Err(PortalError::not_found("pending claim"));
    }
    Ok(Json(json!({ "cancelled": true })))
}

fn portal_base_url(state: &PortalState) -> String {
    let bind = state.config.bind.as_str();
    let host = if bind.is_empty()
        || bind == "0.0.0.0"
        || bind == "::"
        || bind == "*"
        || bind == "127.0.0.1"
        || bind.eq_ignore_ascii_case("localhost")
    {
        "127.0.0.1"
    } else {
        bind
    };
    format!("http://{}:{}/", host, state.config.port)
}
