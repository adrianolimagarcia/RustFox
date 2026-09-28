//! Bot Telegram tokens in [`SecretStore`].
//!
//! Config holds `secret:NAME` (typically `secret:bot.<id>.token`) — never the
//! plaintext BotFather token after bind/migrate. Runtime resolves via the store.
//! One-time startup migration moves legacy plaintext into the store and scrubs
//! `config.toml` (bak via [`crate::config_edit::write_config_validated`]).

use super::{validate_name, SecretStore, SECRET_REF_PREFIX};
use crate::agents_edit::looks_like_bot_token;
use crate::config::Config;
use crate::config_edit::write_config_validated;
use anyhow::{bail, Context, Result};
use std::path::Path;

/// Canonical secret name for a bot's Telegram token.
pub fn bot_token_secret_name(bot_id: &str) -> String {
    format!("bot.{}.token", bot_id.trim())
}

/// Config value form: `secret:bot.<id>.token`.
pub fn bot_token_secret_ref(bot_id: &str) -> String {
    format!("{}{}", SECRET_REF_PREFIX, bot_token_secret_name(bot_id))
}

/// Whether `value` is a `secret:NAME` reference.
pub fn is_secret_ref(value: &str) -> bool {
    value.trim().starts_with(SECRET_REF_PREFIX)
}

/// Strip `secret:` prefix; `None` if not a ref or empty name.
pub fn parse_secret_ref(value: &str) -> Option<&str> {
    value
        .trim()
        .strip_prefix(SECRET_REF_PREFIX)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Resolve a configured `bot_token` field to the raw BotFather token.
///
/// - `secret:NAME` → store lookup (error if missing)
/// - BotFather-shaped plaintext → returned as-is (pre-migration / tests)
/// - anything else → error
pub fn resolve_bot_token(store: &dyn SecretStore, configured: &str) -> Result<String> {
    let configured = configured.trim();
    if configured.is_empty() {
        bail!("bot_token is empty");
    }
    if let Some(name) = parse_secret_ref(configured) {
        validate_name(name)?;
        match store.get(name)? {
            Some(v) => Ok(v.expose().to_string()),
            None => bail!("secret `{name}` not found in SecretStore (bot token)"),
        }
    } else if looks_like_bot_token(configured) {
        Ok(configured.to_string())
    } else {
        bail!("bot_token is neither a secret:NAME ref nor a BotFather token");
    }
}

/// Persist plaintext token under `bot.<id>.token` and return the `secret:…` config value.
pub fn store_bot_token(store: &dyn SecretStore, bot_id: &str, plaintext: &str) -> Result<String> {
    let name = bot_token_secret_name(bot_id);
    validate_name(&name)?;
    let token = plaintext.trim();
    if token.is_empty() {
        bail!("bot_token cannot be empty");
    }
    store.set(&name, token)?;
    Ok(bot_token_secret_ref(bot_id))
}

/// One-time migrate: BotFather-shaped plaintext in `[[bots]]` (and legacy
/// `[telegram].bot_token` when present) → SecretStore + scrub on disk.
///
/// Returns the number of tokens written to the store. No-op when everything is
/// already a `secret:NAME` ref or a non-token placeholder.
pub fn migrate_plaintext_bot_tokens(config_path: &Path, store: &dyn SecretStore) -> Result<usize> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let mut cfg: Config =
        toml::from_str(&content).context("Failed to parse config.toml before bot-token migrate")?;
    cfg.normalize_bots()
        .context("bots / telegram validation failed before bot-token migrate")?;

    let mut changed = 0usize;
    let mut bots = cfg.bots.clone();
    for bot in &mut bots {
        let raw = bot.bot_token.trim();
        if is_secret_ref(raw) {
            continue;
        }
        if !looks_like_bot_token(raw) {
            // Placeholders like YOUR_TELEGRAM_BOT_TOKEN stay until a real bind.
            continue;
        }
        bot.bot_token = store_bot_token(store, &bot.id, raw)?;
        changed += 1;
    }

    if changed == 0 {
        // Still scrub [telegram] if it alone holds plaintext while bots already refs
        // (unlikely after normalize); check disk telegram without rewrite if nothing
        // to do on bots.
        return Ok(0);
    }

    let mut doc: toml::Value =
        toml::from_str(&content).context("Failed to parse config.toml as Value for migrate")?;
    let table = doc
        .as_table_mut()
        .context("config.toml root is not a table")?;

    table.insert(
        "bots".to_string(),
        crate::agents_edit::bots_to_toml_array(&bots),
    );

    // Scrub legacy [telegram].bot_token when it still looks like a BotFather token.
    if let Some(tg) = table.get_mut("telegram").and_then(|v| v.as_table_mut()) {
        let scrub = tg
            .get("bot_token")
            .and_then(|v| v.as_str())
            .map(looks_like_bot_token)
            .unwrap_or(false);
        if scrub {
            let shim_id = Config::shim_bot(&bots).id.clone();
            // Prefer already-migrated shim ref from bots list.
            let shim_ref = bots
                .iter()
                .find(|b| b.id == shim_id)
                .map(|b| b.bot_token.clone())
                .unwrap_or_else(|| bot_token_secret_ref(&shim_id));
            tg.insert("bot_token".to_string(), toml::Value::String(shim_ref));
        }
    }

    let new_content = toml::to_string_pretty(&doc)
        .context("Failed to serialize config after bot-token migrate")?;
    write_config_validated(config_path, &new_content)
        .context("Failed to write scrubbed config after bot-token migrate")?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::SecretStore;
    use super::*;
    use crate::secret_store::FakeSecretStore;
    use tempfile::TempDir;

    fn minimal_bots_toml() -> String {
        r#"
[[bots]]
id = "main"
bot_token = "111111111:AAMainTokenSecretValueXXXX"
allowed_user_ids = [42]
persona = "main"

[openrouter]
api_key = "sk-test"
model = "test-model"

[sandbox]
allowed_directory = "/tmp"
"#
        .to_string()
    }

    #[test]
    fn naming_and_ref_roundtrip() {
        assert_eq!(bot_token_secret_name("main"), "bot.main.token");
        assert_eq!(bot_token_secret_ref("main"), "secret:bot.main.token");
        assert!(is_secret_ref("secret:bot.main.token"));
        assert_eq!(
            parse_secret_ref("secret:bot.main.token"),
            Some("bot.main.token")
        );
        assert!(!is_secret_ref("111111111:AAxxxx"));
    }

    #[test]
    fn resolve_from_store_and_plaintext() {
        let store = FakeSecretStore::new();
        store
            .set("bot.main.token", "111111111:AAMainTokenSecretValueXXXX")
            .unwrap();
        let got = resolve_bot_token(&store, "secret:bot.main.token").unwrap();
        assert_eq!(got, "111111111:AAMainTokenSecretValueXXXX");
        let plain = resolve_bot_token(&store, "222222222:AAPlainTokenSecretValueYY").unwrap();
        assert_eq!(plain, "222222222:AAPlainTokenSecretValueYY");
        assert!(resolve_bot_token(&store, "secret:missing.token").is_err());
    }

    #[test]
    fn migrate_scrubs_config_and_stores() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, minimal_bots_toml()).unwrap();
        let store = FakeSecretStore::new();
        let n = migrate_plaintext_bot_tokens(&path, &store).unwrap();
        assert_eq!(n, 1);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("AAMainTokenSecretValueXXXX"));
        assert!(after.contains("secret:bot.main.token"));
        assert_eq!(
            store.get("bot.main.token").unwrap().unwrap().expose(),
            "111111111:AAMainTokenSecretValueXXXX"
        );
        // Idempotent
        assert_eq!(migrate_plaintext_bot_tokens(&path, &store).unwrap(), 0);
    }

    #[test]
    fn store_bot_token_writes_ref() {
        let store = FakeSecretStore::new();
        let r = store_bot_token(&store, "researcher", "333333333:AAResearchTokenSecretZZ").unwrap();
        assert_eq!(r, "secret:bot.researcher.token");
        assert!(store.exists("bot.researcher.token").unwrap());
    }
}
