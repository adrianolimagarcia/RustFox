//! Host-side secret store.
//!
//! Prefer the OS keyring; when Secret Service / Keychain / Credential Manager
//! is unavailable, fall back to an encrypted file under the RustFox home.
//! Values are never logged (`SecretValue` redacts `Debug` / `Display`).
//!
//! Slice 2: pending claims + Telegram notify + portal claim form.
//! Slice 3: missing→notify, sandbox/tool env inject, redaction hooks.

mod bot_token;
mod bridge;
mod fake;
mod file;
mod keyring_backend;
mod notify;
mod pending;
mod value;

pub use bot_token::{
    bot_token_secret_name, bot_token_secret_ref, is_secret_ref, migrate_plaintext_bot_tokens,
    parse_secret_ref, resolve_bot_token, store_bot_token,
};
pub use bridge::{
    MissingSecret, MissingSecretError, SecretBridge, SecretNotifyFn, SECRET_REF_PREFIX,
};
pub use fake::FakeSecretStore;
pub use file::EncryptedFileSecretStore;
pub use keyring_backend::KeyringSecretStore;
pub use notify::{
    format_secret_request_notify, secret_request_claim_url, secret_request_notify_text,
};
pub use pending::{PendingCreate, PendingSecretRegistry, PendingView, DEFAULT_CLAIM_TTL};
pub use value::SecretValue;

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Which concrete backend [`open`] selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStoreBackend {
    /// OS credential store (`keyring` crate).
    Keyring,
    /// AES-GCM encrypted vault under the RustFox home.
    EncryptedFile,
}

/// Named secret persistence used by the host (and later by portal / sandbox).
pub trait SecretStore: Send + Sync {
    /// Return the secret if present; `Ok(None)` when the name is unknown.
    fn get(&self, name: &str) -> Result<Option<SecretValue>>;

    /// Create or overwrite a secret.
    fn set(&self, name: &str, value: &str) -> Result<()>;

    /// Remove a secret; no-op / `Ok(())` if it did not exist.
    fn delete(&self, name: &str) -> Result<()>;

    /// Whether a secret with this name is stored.
    fn exists(&self, name: &str) -> Result<bool>;
}

/// Validate a secret name (stable identifier; not a free-form path).
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("secret name must not be empty");
    }
    if name.len() > 128 {
        anyhow::bail!("secret name too long (max 128)");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        anyhow::bail!("secret name must be alphanumeric / `_` / `-` / `.`");
    }
    if name.starts_with('.') || name.ends_with('.') {
        anyhow::bail!("secret name must not start or end with '.'");
    }
    Ok(())
}

/// Open the preferred store: OS keyring first; encrypted-file fallback.
///
/// `home` is the RustFox home root (typically `~/.rustfox`). The file vault
/// lives at `<home>/secrets/vault` with master key `<home>/secrets/vault.key`.
pub fn open(home: &Path) -> Result<(Box<dyn SecretStore>, SecretStoreBackend)> {
    match KeyringSecretStore::try_probe_and_open() {
        Ok(store) => {
            tracing::debug!("secret store: using OS keyring");
            Ok((Box::new(store), SecretStoreBackend::Keyring))
        }
        Err(err) => {
            tracing::info!(
                error = %err,
                "OS keyring unavailable; using encrypted-file secret store"
            );
            let vault = default_vault_path(home);
            let store = EncryptedFileSecretStore::open(&vault)
                .with_context(|| format!("open encrypted secret vault at {}", vault.display()))?;
            Ok((Box::new(store), SecretStoreBackend::EncryptedFile))
        }
    }
}

/// Default encrypted vault path under the RustFox home.
pub fn default_vault_path(home: &Path) -> PathBuf {
    home.join("secrets").join("vault")
}

/// Default plaintext master-key path beside the vault (file-fallback only).
pub fn default_vault_key_path(home: &Path) -> PathBuf {
    home.join("secrets").join("vault.key")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn validate_name_accepts_safe_identifiers() {
        assert!(validate_name("OPENROUTER_API_KEY").is_ok());
        assert!(validate_name("bot.token-1").is_ok());
    }

    #[test]
    fn validate_name_rejects_empty_and_path_like() {
        assert!(validate_name("").is_err());
        assert!(validate_name("../etc/passwd").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name(".hidden").is_err());
    }

    #[test]
    fn open_selects_a_usable_backend() {
        let dir = tempdir().unwrap();
        let (store, backend) = open(dir.path()).expect("open secret store");
        // On typical headless CI this is EncryptedFile; either is fine.
        assert!(matches!(
            backend,
            SecretStoreBackend::Keyring | SecretStoreBackend::EncryptedFile
        ));
        store
            .set("SLICE1_PROBE", "round-trip-value-xyz")
            .expect("set");
        assert!(store.exists("SLICE1_PROBE").unwrap());
        let got = store.get("SLICE1_PROBE").unwrap().unwrap();
        assert_eq!(got.expose(), "round-trip-value-xyz");
        store.delete("SLICE1_PROBE").unwrap();
        assert!(!store.exists("SLICE1_PROBE").unwrap());
    }
}
