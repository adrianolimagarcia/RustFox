//! OS keyring backend via the `keyring` crate.

use super::{validate_name, SecretStore, SecretValue};
use anyhow::{anyhow, Context, Result};
use keyring::Entry;

const SERVICE: &str = "rustfox";

/// `SecretStore` backed by the platform credential store.
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    /// Probe whether the OS keyring is usable, then return a store handle.
    ///
    /// Uses a short-lived probe entry. Failures (no Secret Service, locked
    /// session, missing D-Bus, etc.) surface as `Err` so [`super::open`] can
    /// fall back to the encrypted-file vault.
    pub fn try_probe_and_open() -> Result<Self> {
        let probe_name = "__rustfox_keyring_probe__";
        let entry = Entry::new(SERVICE, probe_name)
            .map_err(|e| anyhow!("keyring Entry::new failed: {e}"))?;
        entry
            .set_password("probe")
            .map_err(|e| anyhow!("OS keyring set_password failed: {e}"))?;
        // Best-effort cleanup; ignore delete errors after a successful set.
        let _ = entry.delete_credential();
        Ok(Self)
    }

    fn entry(name: &str) -> Result<Entry> {
        Entry::new(SERVICE, name).with_context(|| format!("keyring entry for {name}"))
    }
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, name: &str) -> Result<Option<SecretValue>> {
        validate_name(name)?;
        let entry = Self::entry(name)?;
        match entry.get_password() {
            Ok(pw) => Ok(Some(SecretValue::new(pw))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow!("keyring get_password: {e}")),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<()> {
        validate_name(name)?;
        let entry = Self::entry(name)?;
        entry
            .set_password(value)
            .map_err(|e| anyhow!("keyring set_password: {e}"))
    }

    fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let entry = Self::entry(name)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow!("keyring delete_credential: {e}")),
        }
    }

    fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.get(name)?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_round_trip_when_available() {
        let store = match KeyringSecretStore::try_probe_and_open() {
            Ok(s) => s,
            Err(_) => {
                eprintln!("skipping keyring round-trip: OS keyring unavailable");
                return;
            }
        };
        let name = "RUSTFOX_SLICE1_KEYRING_TEST";
        let value = "keyring-roundtrip-VALUE-do-not-log";
        store.set(name, value).expect("keyring set");
        assert!(store.exists(name).unwrap());
        let got = store.get(name).unwrap().unwrap();
        assert_eq!(got.expose(), value);
        assert!(!format!("{got:?}").contains("keyring-roundtrip-VALUE"));
        store.delete(name).unwrap();
        assert!(!store.exists(name).unwrap());
    }
}
