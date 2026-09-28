//! In-memory store for injector / harness tests (no live keyring / BotFather).

use super::{validate_name, SecretStore, SecretValue};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Mutex;

/// Process-local `SecretStore` for unit tests and the Telegram Update injector harness.
#[derive(Debug, Default)]
pub struct FakeSecretStore {
    inner: Mutex<HashMap<String, String>>,
}

impl FakeSecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-seed secrets without going through `set` validation (test helper).
    pub fn with_secrets<I, K, V>(entries: I) -> Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let store = Self::new();
        {
            let mut map = store.inner.lock().expect("fake secret store lock");
            for (k, v) in entries {
                let name = k.into();
                validate_name(&name)?;
                map.insert(name, v.into());
            }
        }
        Ok(store)
    }
}

impl SecretStore for FakeSecretStore {
    fn get(&self, name: &str) -> Result<Option<SecretValue>> {
        validate_name(name)?;
        let map = self.inner.lock().expect("fake secret store lock");
        Ok(map.get(name).map(|v| SecretValue::new(v.clone())))
    }

    fn set(&self, name: &str, value: &str) -> Result<()> {
        validate_name(name)?;
        let mut map = self.inner.lock().expect("fake secret store lock");
        map.insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let mut map = self.inner.lock().expect("fake secret store lock");
        map.remove(name);
        Ok(())
    }

    fn exists(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let map = self.inner.lock().expect("fake secret store lock");
        Ok(map.contains_key(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_round_trip_and_delete() {
        let store = FakeSecretStore::new();
        assert!(!store.exists("API_KEY").unwrap());
        store.set("API_KEY", "sk-test-ROUNDTRIP-never-log").unwrap();
        assert!(store.exists("API_KEY").unwrap());
        let v = store.get("API_KEY").unwrap().unwrap();
        assert_eq!(v.expose(), "sk-test-ROUNDTRIP-never-log");
        assert!(!format!("{v:?}").contains("sk-test"));
        store.delete("API_KEY").unwrap();
        assert!(store.get("API_KEY").unwrap().is_none());
        assert!(!store.exists("API_KEY").unwrap());
    }

    #[test]
    fn fake_preseed_for_harness() {
        let store =
            FakeSecretStore::with_secrets([("HARNESS_TOKEN", "injector-fake-no-botfather")])
                .unwrap();
        assert_eq!(
            store.get("HARNESS_TOKEN").unwrap().unwrap().expose(),
            "injector-fake-no-botfather"
        );
    }
}
