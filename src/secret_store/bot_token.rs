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

/// Seal BotFather-shaped plaintext `bot_token` values in a config TOML string.
///
/// Writes each token into [`SecretStore`] under `bot.<id>.token` and replaces the
/// config value with `secret:bot.<id>.token`. Placeholders and existing
/// `secret:NAME` refs are left alone. Does **not** materialize `[[bots]]` from
/// legacy `[telegram]` — only scrubs in place (wizard first-save stays
/// `[telegram]`-shaped).
///
/// Returns `(sealed_toml, tokens_stored_or_scrubbed)`.
pub fn seal_plaintext_bot_tokens_in_config(
    content: &str,
    store: &dyn SecretStore,
) -> Result<(String, usize)> {
    let mut doc: toml::Value =
        toml::from_str(content).context("Failed to parse config.toml before bot-token seal")?;
    let table = doc
        .as_table_mut()
        .context("config.toml root is not a table")?;

    let mut changed = 0usize;

    // Scrub [[bots]] rows first so [telegram] can reuse the shim ref.
    if let Some(arr) = table.get("bots").and_then(|v| v.as_array()).cloned() {
        let mut new_arr = Vec::with_capacity(arr.len());
        for bot_val in arr {
            let mut bot_val = bot_val;
            if let Some(bot) = bot_val.as_table_mut() {
                let id = bot
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let token = bot
                    .get("bot_token")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !id.is_empty() && looks_like_bot_token(&token) {
                    let secret_ref = store_bot_token(store, &id, &token)?;
                    bot.insert("bot_token".into(), toml::Value::String(secret_ref));
                    changed += 1;
                }
            }
            new_arr.push(bot_val);
        }
        table.insert("bots".into(), toml::Value::Array(new_arr));
    }

    let telegram_token = table
        .get("telegram")
        .and_then(|v| v.as_table())
        .and_then(|tg| tg.get("bot_token"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    if let Some(token) = telegram_token {
        if looks_like_bot_token(&token) {
            let shim_id = shim_bot_id_from_table(table).unwrap_or_else(|| "default".to_string());
            let secret_ref = match bot_token_field_for_id(table, &shim_id) {
                Some(existing) if is_secret_ref(&existing) => existing,
                _ => store_bot_token(store, &shim_id, &token)?,
            };
            if let Some(tg) = table.get_mut("telegram").and_then(|v| v.as_table_mut()) {
                tg.insert("bot_token".into(), toml::Value::String(secret_ref));
                changed += 1;
            }
        }
    }

    if changed == 0 {
        return Ok((content.to_string(), 0));
    }
    let sealed =
        toml::to_string_pretty(&doc).context("Failed to serialize config after bot-token seal")?;
    Ok((sealed, changed))
}

fn shim_bot_id_from_table(table: &toml::map::Map<String, toml::Value>) -> Option<String> {
    let arr = table.get("bots")?.as_array()?;
    let mut ids: Vec<String> = Vec::new();
    for bot in arr {
        if let Some(id) = bot
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
        {
            if !id.is_empty() {
                ids.push(id);
            }
        }
    }
    if ids.is_empty() {
        return None;
    }
    if let Some(main) = ids.iter().find(|id| *id == "main") {
        return Some(main.clone());
    }
    if let Some(default) = ids.iter().find(|id| *id == "default") {
        return Some(default.clone());
    }
    Some(ids[0].clone())
}

fn bot_token_field_for_id(table: &toml::map::Map<String, toml::Value>, id: &str) -> Option<String> {
    let arr = table.get("bots")?.as_array()?;
    for bot in arr {
        let bot_id = bot.get("id").and_then(|v| v.as_str()).unwrap_or("").trim();
        if bot_id == id {
            return bot
                .get("bot_token")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

/// One-time migrate: BotFather-shaped plaintext in `[[bots]]` / `[telegram]` →
/// SecretStore + scrub on disk (safety net after wizard/bind seal).
///
/// Returns the number of tokens scrubbed. No-op when everything is already a
/// `secret:NAME` ref or a non-token placeholder.
pub fn migrate_plaintext_bot_tokens(config_path: &Path, store: &dyn SecretStore) -> Result<usize> {
    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    // Validate shape early so we do not seal a broken file onto disk.
    let mut cfg: Config =
        toml::from_str(&content).context("Failed to parse config.toml before bot-token migrate")?;
    cfg.normalize_bots()
        .context("bots / telegram validation failed before bot-token migrate")?;

    let (sealed, changed) = seal_plaintext_bot_tokens_in_config(&content, store)?;
    if changed == 0 {
        return Ok(0);
    }
    write_config_validated(config_path, &sealed)
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

    #[test]
    fn seal_scrubs_telegram_only_first_save() {
        let store = FakeSecretStore::new();
        let raw = r#"
[telegram]
bot_token = "111111111:AAWizardFirstSaveTokenXXXX"
allowed_user_ids = [42]

[openrouter]
api_key = "sk-test"
model = "test-model"
"#;
        let (sealed, n) = seal_plaintext_bot_tokens_in_config(raw, &store).unwrap();
        assert_eq!(n, 1);
        assert!(!sealed.contains("AAWizardFirstSaveTokenXXXX"));
        assert!(sealed.contains("secret:bot.default.token"));
        assert!(!sealed.contains("[[bots]]"));
        assert_eq!(
            store.get("bot.default.token").unwrap().unwrap().expose(),
            "111111111:AAWizardFirstSaveTokenXXXX"
        );
        // Idempotent
        let (_, n2) = seal_plaintext_bot_tokens_in_config(&sealed, &store).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn seal_leaves_placeholder_alone() {
        let store = FakeSecretStore::new();
        let raw = r#"
[telegram]
bot_token = "YOUR_TELEGRAM_BOT_TOKEN"
allowed_user_ids = [1]
"#;
        let (sealed, n) = seal_plaintext_bot_tokens_in_config(raw, &store).unwrap();
        assert_eq!(n, 0);
        assert!(sealed.contains("YOUR_TELEGRAM_BOT_TOKEN"));
    }
}
