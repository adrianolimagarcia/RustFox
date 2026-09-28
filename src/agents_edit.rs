//! Telegram `/agents` create + bot-token bind helpers.
//!
//! Pure logic (no Telegram types): create persona packs under `agents/<id>/`,
//! list/show without echoing tokens, append `[[bots]]` via the shared
//! [`crate::config_edit::write_config_validated`] bak path.
//!
//! Token values are never returned in list/show strings — replies use
//! `bot_token=***` only (TL lock).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::{BotConfig, Config};
use crate::config_edit::{validate_config_str, write_config_validated};

/// Pending-token memory key prefix (`settings` category). Value = agent id only —
/// never the BotFather token.
pub fn token_pending_key(user_id: u64) -> String {
    format!("agents_token_pending_{user_id}")
}

/// Validate a new agent / bot id (same charset as skill/agent portal names).
pub fn validate_agent_id(id: &str) -> Result<()> {
    let id = id.trim();
    if id.is_empty() {
        bail!("agent id cannot be empty");
    }
    if id.len() > 64 {
        bail!("agent id too long (max 64 chars)");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("agent id must be ascii alphanumeric, '-' or '_'");
    }
    Ok(())
}

/// Default `AGENT.md` for a newly created persona pack.
pub fn default_agent_md(id: &str) -> String {
    format!(
        "---\n\
         name: {id}\n\
         description: \"Telegram bot persona `{id}`\"\n\
         ---\n\
         You are the `{id}` persona for RustFox on Telegram. Be helpful, concise, \
         and use tools when they improve the answer.\n"
    )
}

/// Default `SOUL.md` overlay for a newly created persona pack.
pub fn default_soul_md(id: &str) -> String {
    format!(
        "# Soul — {id}\n\
         \n\
         ## Who I Am\n\
         I am the `{id}` RustFox persona on Telegram.\n\
         \n\
         ## My Values\n\
         - Be genuinely helpful, not performatively helpful\n\
         - Earn trust through competence\n\
         \n\
         ## My Boundaries\n\
         - Private things stay private\n\
         - Never send half-baked replies\n"
    )
}

/// Result of creating a persona directory.
#[derive(Debug, Clone)]
pub struct CreatePersonaResult {
    pub id: String,
    pub dir: PathBuf,
    pub agent_md: PathBuf,
    pub soul_md: PathBuf,
}

/// Create `agents/<id>/AGENT.md` (+ default SOUL.md). Rejects if the directory
/// or `AGENT.md` already exists.
pub fn create_persona_pack(agents_dir: &Path, id: &str) -> Result<CreatePersonaResult> {
    validate_agent_id(id)?;
    let id = id.trim().to_string();
    let dir = agents_dir.join(&id);
    let agent_md = dir.join("AGENT.md");
    let soul_md = dir.join("SOUL.md");

    if agent_md.exists() || dir.exists() {
        bail!("persona pack `agents/{id}/` already exists");
    }

    std::fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;

    if let Err(e) = std::fs::write(&agent_md, default_agent_md(&id)) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e).with_context(|| format!("Failed to write {}", agent_md.display()));
    }
    if let Err(e) = std::fs::write(&soul_md, default_soul_md(&id)) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e).with_context(|| format!("Failed to write {}", soul_md.display()));
    }

    Ok(CreatePersonaResult {
        id,
        dir,
        agent_md,
        soul_md,
    })
}

/// List bots with ids + personas; never include raw tokens.
pub fn format_agents_list(cfg: &Config) -> String {
    let mut lines = Vec::new();
    lines.push("**Agents / bots** (tokens redacted)".to_string());
    lines.push(String::new());
    if cfg.bots.is_empty() {
        lines.push("_No bots configured._".to_string());
    } else {
        for b in &cfg.bots {
            lines.push(format!(
                "- `{}` — persona=`{}` allowlist={:?} bot_token=***",
                b.id, b.persona, b.allowed_user_ids
            ));
        }
    }
    lines.push(String::new());
    lines.push(
        "Create: `/agents create <id>` then send the BotFather token as your **next** message (one-shot; deleted)."
            .to_string(),
    );
    lines.push("Cancel pending token: `/agents cancel`".to_string());
    lines.join("\n")
}

/// Show one bot; `bot_token=***` only.
pub fn format_agent_show(cfg: &Config, id: &str) -> Result<String> {
    let id = id.trim();
    let bot = cfg
        .bots
        .iter()
        .find(|b| b.id.trim() == id)
        .with_context(|| format!("no bot with id `{id}`"))?;
    Ok(format!(
        "**Bot `{id}`**\n\
         - persona=`{}`\n\
         - bot_token=***\n\
         - allowed_user_ids={:?}\n",
        bot.persona, bot.allowed_user_ids
    ))
}

/// Slash help for `/agents`.
pub fn slash_help_markdown() -> String {
    "**`/agents` slash map**\n\n\
     - `/agents` — list bot ids + personas (`bot_token=***`)\n\
     - `/agents show <id>` — one bot (token redacted)\n\
     - `/agents create <id>` — create `agents/<id>/` then bind BotFather token\n\
     - `/agents cancel` — abort pending token bind\n\n\
     After create, send the token as the **next** message only. It is deleted and never logged.\n\
     New `[[bots]]` entry: `{ id, bot_token, persona=<id>, allowed_user_ids=[caller] }` → `.bak` → restart.\n"
        .to_string()
}

/// Lightweight BotFather token shape check (digits:secret). Does not call Telegram.
pub fn looks_like_bot_token(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return false;
    }
    let Some((id_part, secret)) = t.split_once(':') else {
        return false;
    };
    !id_part.is_empty()
        && id_part.chars().all(|c| c.is_ascii_digit())
        && secret.len() >= 20
        && secret
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Result of appending a `[[bots]]` entry.
#[derive(Debug, Clone)]
pub struct BindBotResult {
    pub id: String,
    pub persona: String,
    pub bak_path: PathBuf,
    pub allowed_user_ids: Vec<u64>,
}

/// Append a new `[[bots]]` row (materializing legacy `[telegram]` into `[[bots]]`
/// when needed). Uses validate → bak → atomic write.
///
/// Duplicate `id` or `bot_token` is rejected. `allowed_user_ids` must be non-empty
/// (caller id). The raw token is never returned in [`BindBotResult`].
pub fn append_bot_binding(
    config_path: &Path,
    id: &str,
    bot_token: &str,
    caller_user_id: u64,
) -> Result<BindBotResult> {
    validate_agent_id(id)?;
    let id = id.trim().to_string();
    let token = bot_token.trim();
    if token.is_empty() {
        bail!("bot_token cannot be empty");
    }
    if !looks_like_bot_token(token) {
        bail!("bot_token does not look like a BotFather token (expected <digits>:<secret>)");
    }
    if caller_user_id == 0 {
        bail!("caller user id cannot be empty");
    }

    let content = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;

    let mut cfg: Config =
        toml::from_str(&content).context("Failed to parse config.toml before bot bind")?;
    cfg.normalize_bots()
        .context("bots / telegram validation failed before bind")?;

    if cfg.bots.iter().any(|b| b.id.trim() == id) {
        bail!("duplicate bots[].id \"{id}\"");
    }
    if cfg.bots.iter().any(|b| b.bot_token.trim() == token) {
        bail!("duplicate bots[].bot_token");
    }

    let allowed = vec![caller_user_id];
    let new_bot = BotConfig {
        id: id.clone(),
        bot_token: token.to_string(),
        allowed_user_ids: allowed.clone(),
        persona: id.clone(),
        system_prompt_file: None,
        model: None,
        tools: None,
    };

    // Materialize full list (includes synthesized legacy default when needed).
    let mut bots = cfg.bots;
    bots.push(new_bot);

    let mut doc: toml::Value =
        toml::from_str(&content).context("Failed to parse config.toml as Value")?;
    let bots_val = bots_to_toml_array(&bots);
    if let Some(table) = doc.as_table_mut() {
        table.insert("bots".to_string(), bots_val);
    } else {
        bail!("config.toml root is not a table");
    }

    let new_content =
        toml::to_string_pretty(&doc).context("Failed to serialize config after bot bind")?;
    // Extra safety: validate before write_config_validated (it also validates).
    validate_config_str(&new_content).context("Validation failed; config.toml not modified")?;

    let bak = write_config_validated(config_path, &new_content)?;
    let persona = id.clone(); // persona == id by TL lock

    Ok(BindBotResult {
        id,
        persona,
        bak_path: bak,
        allowed_user_ids: allowed,
    })
}

fn bots_to_toml_array(bots: &[BotConfig]) -> toml::Value {
    let arr: Vec<toml::Value> = bots
        .iter()
        .map(|b| {
            let mut t = toml::map::Map::new();
            t.insert("id".into(), toml::Value::String(b.id.clone()));
            t.insert("bot_token".into(), toml::Value::String(b.bot_token.clone()));
            t.insert(
                "allowed_user_ids".into(),
                toml::Value::Array(
                    b.allowed_user_ids
                        .iter()
                        .map(|&u| toml::Value::Integer(u as i64))
                        .collect(),
                ),
            );
            t.insert("persona".into(), toml::Value::String(b.persona.clone()));
            if let Some(ref p) = b.system_prompt_file {
                t.insert(
                    "system_prompt_file".into(),
                    toml::Value::String(p.display().to_string()),
                );
            }
            if let Some(ref m) = b.model {
                t.insert("model".into(), toml::Value::String(m.clone()));
            }
            if let Some(ref tools) = b.tools {
                t.insert(
                    "tools".into(),
                    toml::Value::Array(
                        tools
                            .iter()
                            .map(|s| toml::Value::String(s.clone()))
                            .collect(),
                    ),
                );
            }
            toml::Value::Table(t)
        })
        .collect();
    toml::Value::Array(arr)
}

/// Redact a string for logs when it might contain a token (never echo raw).
pub fn redact_possible_token(text: &str) -> String {
    let t = text.trim();
    if looks_like_bot_token(t) {
        return "bot_token=***".to_string();
    }
    // Truncate long messages for logs without leaking token-like mid-string.
    if t.len() > 80 {
        format!("{}…", &t[..40])
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn minimal_legacy_toml() -> String {
        r#"
[telegram]
bot_token = "111111111:AALegacyTokenSecretValueXX"
allowed_user_ids = [42]

[openrouter]
api_key = "sk-test"
model = "test-model"

[sandbox]
allowed_directory = "/tmp"
"#
        .to_string()
    }

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

    fn write_cfg(dir: &TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn validate_agent_id_accepts_safe_names() {
        assert!(validate_agent_id("researcher").is_ok());
        assert!(validate_agent_id("bot_2").is_ok());
        assert!(validate_agent_id("a-b").is_ok());
    }

    #[test]
    fn validate_agent_id_rejects_bad() {
        assert!(validate_agent_id("").is_err());
        assert!(validate_agent_id("../x").is_err());
        assert!(validate_agent_id("has space").is_err());
        assert!(validate_agent_id(&"x".repeat(65)).is_err());
    }

    #[test]
    fn create_persona_writes_agent_and_soul() {
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let r = create_persona_pack(&agents, "researcher").unwrap();
        assert!(r.agent_md.exists());
        assert!(r.soul_md.exists());
        let body = std::fs::read_to_string(&r.agent_md).unwrap();
        assert!(body.contains("name: researcher"));
        assert!(std::fs::read_to_string(&r.soul_md)
            .unwrap()
            .contains("researcher"));
    }

    #[test]
    fn create_persona_rejects_duplicate_dir() {
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        create_persona_pack(&agents, "dup").unwrap();
        let err = create_persona_pack(&agents, "dup").unwrap_err().to_string();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn format_list_and_show_never_include_raw_token() {
        let mut cfg: Config = toml::from_str(&minimal_bots_toml()).unwrap();
        cfg.normalize_bots().unwrap();
        let list = format_agents_list(&cfg);
        assert!(list.contains("bot_token=***"));
        assert!(!list.contains("AAMainToken"));
        assert!(list.contains("`main`"));
        let show = format_agent_show(&cfg, "main").unwrap();
        assert!(show.contains("bot_token=***"));
        assert!(!show.contains("AAMainToken"));
    }

    #[test]
    fn append_bot_writes_bak_and_rejects_duplicate_id() {
        let dir = TempDir::new().unwrap();
        let path = write_cfg(&dir, &minimal_bots_toml());
        let before = std::fs::read_to_string(&path).unwrap();

        let r = append_bot_binding(
            &path,
            "researcher",
            "222222222:AANewTokenSecretValueYYYYYY",
            99,
        )
        .unwrap();
        assert_eq!(r.id, "researcher");
        assert_eq!(r.persona, "researcher");
        assert_eq!(r.allowed_user_ids, vec![99]);
        assert!(r.bak_path.exists());
        let bak = std::fs::read_to_string(&r.bak_path).unwrap();
        assert_eq!(bak, before);

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("researcher"));
        assert!(after.contains("222222222:AANewTokenSecretValueYYYYYY"));
        // show/list helpers still redact
        let mut cfg: Config = toml::from_str(&after).unwrap();
        cfg.normalize_bots().unwrap();
        assert_eq!(cfg.bots.len(), 2);
        assert!(!format_agents_list(&cfg).contains("AANewToken"));

        let err = append_bot_binding(
            &path,
            "researcher",
            "333333333:AAOtherTokenSecretValueZZZZ",
            99,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn append_bot_rejects_duplicate_token() {
        let dir = TempDir::new().unwrap();
        let path = write_cfg(&dir, &minimal_bots_toml());
        let err = append_bot_binding(&path, "other", "111111111:AAMainTokenSecretValueXXXX", 99)
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate bots[].bot_token"), "{err}");
    }

    #[test]
    fn append_bot_materializes_legacy_telegram() {
        let dir = TempDir::new().unwrap();
        let path = write_cfg(&dir, &minimal_legacy_toml());
        let r = append_bot_binding(
            &path,
            "researcher",
            "222222222:AANewTokenSecretValueYYYYYY",
            7,
        )
        .unwrap();
        assert_eq!(r.id, "researcher");
        let after = std::fs::read_to_string(&path).unwrap();
        let mut cfg: Config = toml::from_str(&after).unwrap();
        cfg.normalize_bots().unwrap();
        // legacy default + new
        assert_eq!(cfg.bots.len(), 2);
        assert!(cfg.bots.iter().any(|b| b.id == "default"));
        assert!(cfg.bots.iter().any(|b| b.id == "researcher"));
        let research = cfg.bots.iter().find(|b| b.id == "researcher").unwrap();
        assert_eq!(research.allowed_user_ids, vec![7]);
        assert_eq!(research.persona, "researcher");
    }

    #[test]
    fn looks_like_bot_token_gate() {
        assert!(looks_like_bot_token(
            "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw"
        ));
        assert!(!looks_like_bot_token("not-a-token"));
        assert!(!looks_like_bot_token("123:short"));
        assert!(!looks_like_bot_token(""));
    }

    #[test]
    fn redact_possible_token_hides_token() {
        let raw = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";
        assert_eq!(redact_possible_token(raw), "bot_token=***");
        assert!(!redact_possible_token(raw).contains("AAHdq"));
    }
}
