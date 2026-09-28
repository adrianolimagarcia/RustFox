//! Slice 3: missing → pending+notify, env-only inject, value redaction.
//!
//! Tools/MCP/sandbox resolve named secrets through [`SecretBridge`]. Values
//! are injected into child-process env maps only — never into chat, LLM
//! context, tool args, or logs. Missing required secrets create a pending
//! claim and invoke the notify callback (Telegram link; name only).

use super::notify::{format_secret_request_notify, secret_request_claim_url};
use super::pending::{PendingCreate, PendingSecretRegistry};
use super::{validate_name, SecretStore, SecretValue};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};

/// Prefix for MCP/config env values that resolve from [`SecretStore`].
/// Example: `API_KEY = "secret:OPENROUTER_API_KEY"`.
pub const SECRET_REF_PREFIX: &str = "secret:";

/// Safe error when a required secret is missing (pending created + notify).
#[derive(Clone)]
pub struct MissingSecret {
    pub name: String,
    pub claim_url: String,
    pub notify_message: String,
}

impl fmt::Display for MissingSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Secret `{}` is required — check Telegram for a portal link to enter it (masked).",
            self.name
        )
    }
}

impl fmt::Debug for MissingSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MissingSecret")
            .field("name", &self.name)
            .field("claim_url", &self.claim_url)
            .field("notify_message", &"[see claim_url]")
            .finish()
    }
}

impl std::error::Error for MissingSecret {}

/// Callback invoked when a missing secret creates a pending claim.
/// Arguments: secret name, claim URL (opaque token may appear only in the URL).
pub type SecretNotifyFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Host-side secret orchestration for spawn env + missing-secret UX.
pub struct SecretBridge {
    store: Arc<dyn SecretStore>,
    pending: Arc<PendingSecretRegistry>,
    portal_base: Mutex<String>,
    notify: Mutex<Option<SecretNotifyFn>>,
    /// Values seen via resolve/require — used for redaction hooks.
    known_values: Mutex<HashSet<String>>,
}

impl SecretBridge {
    pub fn new(store: Arc<dyn SecretStore>, pending: Arc<PendingSecretRegistry>) -> Self {
        Self {
            store,
            pending,
            portal_base: Mutex::new("http://127.0.0.1:8090/".into()),
            notify: Mutex::new(None),
            known_values: Mutex::new(HashSet::new()),
        }
    }

    pub fn store(&self) -> Arc<dyn SecretStore> {
        Arc::clone(&self.store)
    }

    pub fn pending(&self) -> Arc<PendingSecretRegistry> {
        Arc::clone(&self.pending)
    }

    pub fn set_portal_base(&self, base: impl Into<String>) {
        *self.portal_base.lock().expect("portal_base lock") = base.into();
    }

    pub fn set_notify(&self, notify: SecretNotifyFn) {
        *self.notify.lock().expect("notify lock") = Some(notify);
    }

    /// Look up a secret without creating pending. Remembers value for redaction.
    pub fn get(&self, name: &str) -> Result<Option<SecretValue>> {
        let v = self.store.get(name)?;
        if let Some(ref sv) = v {
            self.remember(sv.expose());
        }
        Ok(v)
    }

    /// Require a named secret. Missing → pending + notify + [`MissingSecret`].
    pub fn require(&self, name: &str) -> Result<SecretValue, MissingSecretError> {
        validate_name(name).map_err(|e| MissingSecretError::InvalidName(e.to_string()))?;
        match self.store.get(name) {
            Ok(Some(v)) => {
                self.remember(v.expose());
                Ok(v)
            }
            Ok(None) => Err(MissingSecretError::Missing(self.request_missing(name)?)),
            Err(e) => Err(MissingSecretError::Store(e.to_string())),
        }
    }

    /// Create pending + notify for a missing secret (idempotent-ish: always new claim).
    pub fn request_missing(&self, name: &str) -> Result<MissingSecret, MissingSecretError> {
        validate_name(name).map_err(|e| MissingSecretError::InvalidName(e.to_string()))?;
        let created: PendingCreate = self
            .pending
            .request(name)
            .map_err(|e| MissingSecretError::Store(e.to_string()))?;
        let base = self.portal_base.lock().expect("portal_base lock").clone();
        let claim_url = secret_request_claim_url(&base, &created.claim_token);
        let notify_message = format_secret_request_notify(name, &claim_url);
        if let Some(cb) = self.notify.lock().expect("notify lock").as_ref() {
            cb(name, &claim_url);
        }
        Ok(MissingSecret {
            name: name.to_string(),
            claim_url,
            notify_message,
        })
    }

    /// Resolve a configured env map. Values with [`SECRET_REF_PREFIX`] are
    /// looked up in the store (required). Plain values pass through.
    pub fn resolve_env_map(
        &self,
        configured: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, MissingSecretError> {
        let mut out = HashMap::new();
        for (key, value) in configured {
            if let Some(secret_name) = value.strip_prefix(SECRET_REF_PREFIX) {
                let secret_name = secret_name.trim();
                let sv = self.require(secret_name)?;
                out.insert(key.clone(), sv.expose().to_string());
            } else {
                out.insert(key.clone(), value.clone());
            }
        }
        Ok(out)
    }

    /// Build env inject map for the given secret names (all required).
    pub fn env_for_names(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, String>, MissingSecretError> {
        let mut out = HashMap::new();
        for name in names {
            let sv = self.require(name)?;
            out.insert(name.clone(), sv.expose().to_string());
        }
        Ok(out)
    }

    /// Redact known secret values (exact) plus credential-style patterns.
    pub fn redact(&self, text: &str) -> String {
        let mut s = text.to_string();
        let values: Vec<String> = self
            .known_values
            .lock()
            .expect("known_values lock")
            .iter()
            .filter(|v| v.len() >= 4)
            .cloned()
            .collect();
        // Longer values first to avoid partial overlaps.
        let mut values = values;
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        for v in values {
            if s.contains(&v) {
                s = s.replace(&v, "***");
            }
        }
        crate::supervisor::redact::redact(&s)
    }

    fn remember(&self, value: &str) {
        if value.is_empty() {
            return;
        }
        self.known_values
            .lock()
            .expect("known_values lock")
            .insert(value.to_string());
    }
}

/// Error surface for require / resolve (never includes secret values).
#[derive(Debug)]
pub enum MissingSecretError {
    Missing(MissingSecret),
    InvalidName(String),
    Store(String),
}

impl fmt::Display for MissingSecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(m) => write!(f, "{m}"),
            Self::InvalidName(s) => write!(f, "invalid secret name: {s}"),
            Self::Store(s) => write!(f, "secret store error: {s}"),
        }
    }
}

impl std::error::Error for MissingSecretError {}

impl MissingSecretError {
    pub fn missing(&self) -> Option<&MissingSecret> {
        match self {
            Self::Missing(m) => Some(m),
            _ => None,
        }
    }
}

// Manual Debug so we never dump known_values.
impl fmt::Debug for SecretBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretBridge")
            .field("portal_base", &self.portal_base)
            .field("known_values", &"[REDACTED_SET]")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::FakeSecretStore;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn bridge_with(store: FakeSecretStore) -> (Arc<SecretBridge>, Arc<PendingSecretRegistry>) {
        let pending = Arc::new(PendingSecretRegistry::default());
        let bridge = Arc::new(SecretBridge::new(Arc::new(store), Arc::clone(&pending)));
        bridge.set_portal_base("http://127.0.0.1:8090/");
        (bridge, pending)
    }

    #[test]
    fn missing_secret_creates_pending_and_notifies_without_value() {
        let (bridge, pending) = bridge_with(FakeSecretStore::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = Arc::clone(&calls);
        let last_url = Arc::new(Mutex::new(String::new()));
        let last_url2 = Arc::clone(&last_url);
        bridge.set_notify(Arc::new(move |name, url| {
            assert_eq!(name, "NEED_ME");
            assert!(!url.is_empty());
            calls2.fetch_add(1, Ordering::SeqCst);
            *last_url2.lock().unwrap() = url.to_string();
        }));

        let err = bridge.require("NEED_ME").unwrap_err();
        let missing = err.missing().unwrap();
        assert_eq!(missing.name, "NEED_ME");
        assert!(missing.notify_message.contains("NEED_ME"));
        assert!(!missing.notify_message.contains("sk-"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!pending.list().is_empty());
        let body = crate::secret_store::secret_request_notify_text("NEED_ME");
        let token = last_url
            .lock()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        assert!(!token.is_empty());
        assert!(
            !body.contains(&token),
            "notify text must not echo raw claim token"
        );
        assert!(!missing.notify_message.contains("sk-live"));
    }

    #[test]
    fn env_inject_uses_store_values_not_in_debug() {
        let store = FakeSecretStore::with_secrets([("API_TOKEN", "sk-inject-SECRET-xyz")]).unwrap();
        let (bridge, _) = bridge_with(store);
        let env = bridge
            .env_for_names(&["API_TOKEN".into()])
            .expect("resolve");
        assert_eq!(
            env.get("API_TOKEN").map(String::as_str),
            Some("sk-inject-SECRET-xyz")
        );
        // Debug of bridge / missing errors must not show the value.
        assert!(!format!("{bridge:?}").contains("sk-inject"));
        let redacted = bridge.redact("using sk-inject-SECRET-xyz in output");
        assert!(!redacted.contains("sk-inject-SECRET-xyz"));
        assert!(redacted.contains("***"));
    }

    #[test]
    fn resolve_env_map_secret_prefix() {
        let store = FakeSecretStore::with_secrets([("GH_TOKEN", "ghs_secret_VALUE_99")]).unwrap();
        let (bridge, _) = bridge_with(store);
        let mut cfg = HashMap::new();
        cfg.insert("GITHUB_TOKEN".into(), "secret:GH_TOKEN".into());
        cfg.insert("PLAIN".into(), "hello".into());
        let resolved = bridge.resolve_env_map(&cfg).unwrap();
        assert_eq!(resolved.get("GITHUB_TOKEN").unwrap(), "ghs_secret_VALUE_99");
        assert_eq!(resolved.get("PLAIN").unwrap(), "hello");
        assert!(!bridge
            .redact(&format!("{resolved:?}"))
            .contains("ghs_secret_VALUE_99"));
    }

    #[test]
    fn redact_strips_known_values_from_tool_echo() {
        let store = FakeSecretStore::with_secrets([("K", "unique-secret-value-ZZ9")]).unwrap();
        let (bridge, _) = bridge_with(store);
        let _ = bridge.require("K").unwrap();
        let echo = "tool said unique-secret-value-ZZ9 aloud";
        let safe = bridge.redact(echo);
        assert!(!safe.contains("unique-secret-value-ZZ9"));
        assert!(!format!("{safe:?}").contains("unique-secret-value-ZZ9"));
    }
}
