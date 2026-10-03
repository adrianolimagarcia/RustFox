//! Thin setup: four fields, safe defaults.
//!
//! Provider is OpenRouter (the default) or Ollama. OpenRouter asks for an
//! API key and keeps the schema's default model. Ollama never asks for a
//! base URL or a typed model id: a running daemon's tags are the choices,
//! and a model that is not already local is picked from one library list
//! and pulled once. Bot token and one system-prompt sentence are the other
//! two fields. Tools and MCP are not questions. Sandbox-safe tools stay on
//! because the written config does not set a tool whitelist.

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Fixed local daemon. Not a wizard question.
pub const OLLAMA_TAGS_URL: &str = "http://127.0.0.1:11434/api/tags";
pub const OLLAMA_PULL_URL: &str = "http://127.0.0.1:11434/api/pull";
/// OpenAI-compatible base the existing `[[provider]]` schema expects.
pub const OLLAMA_PROVIDER_BASE: &str = "http://127.0.0.1:11434/v1";

pub const OLLAMA_NOT_RUNNING: &str = "Ollama is not running.";

/// Featured library names (default tag). One list: no sizes, no quantizations,
/// no repo browser. Each name is a model on the Ollama library.
pub const OLLAMA_LIBRARY: &[&str] = &[
    "llama3.2",
    "llama3.1",
    "qwen2.5",
    "qwen3",
    "gemma3",
    "mistral",
    "phi3",
    "deepseek-r1",
];

/// Sandbox-safe builtins. A missing whitelist keeps these on (install default).
pub const SANDBOX_SAFE_TOOLS: &[&str] =
    &["read_file", "write_file", "list_files", "execute_command"];

/// Telegram user ids start at 1, so `0` matches nobody. Fresh thin setup
/// boots closed instead of asking for an allowlist (empty is a hard error).
pub const CLOSED_ALLOWLIST_USER: u64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinProvider {
    OpenRouter,
    Ollama,
}

impl ThinProvider {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "openrouter" => Ok(Self::OpenRouter),
            "ollama" => Ok(Self::Ollama),
            other => bail!("provider must be OpenRouter or Ollama, not {other}"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::Ollama => "ollama",
        }
    }
}

/// The only questions the wizard asks, in order.
pub fn wizard_fields(provider: ThinProvider) -> [&'static str; 4] {
    match provider {
        ThinProvider::OpenRouter => [
            "provider",
            "openrouter_api_key",
            "bot_token",
            "system_prompt",
        ],
        ThinProvider::Ollama => ["provider", "ollama_model", "bot_token", "system_prompt"],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaDetect {
    pub running: bool,
    pub models: Vec<String>,
    /// Set only when Ollama is not running. One line, no trailing newline.
    pub message: Option<&'static str>,
}

/// `reached_ok` is a successful tags response. Anything else is "not running".
pub fn detect_from_http(reached_ok: bool, body: &str) -> OllamaDetect {
    if !reached_ok {
        return OllamaDetect {
            running: false,
            models: Vec::new(),
            message: Some(OLLAMA_NOT_RUNNING),
        };
    }
    OllamaDetect {
        running: true,
        models: model_names_from_tags(body),
        message: None,
    }
}

pub fn model_names_from_tags(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(models) = value.get("models").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for model in models {
        let name = model
            .get("name")
            .and_then(|n| n.as_str())
            .or_else(|| model.get("model").and_then(|n| n.as_str()))
            .unwrap_or("")
            .trim();
        if name.is_empty() || names.iter().any(|n| n == name) {
            continue;
        }
        names.push(name.to_string());
    }
    names
}

fn model_base(name: &str) -> &str {
    name.split(':').next().unwrap_or(name).trim()
}

/// Library names that are not already present locally. One list, no tags.
pub fn library_choices(local: &[String]) -> Vec<&'static str> {
    let have: Vec<&str> = local.iter().map(|n| model_base(n)).collect();
    OLLAMA_LIBRARY
        .iter()
        .copied()
        .filter(|name| !have.iter().any(|h| h == name))
        .collect()
}

/// Rejects a typed id, a tag/quantization, and anything outside the one list.
/// Does not pull.
pub fn validate_library_pick(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name.contains(':') || name.contains('/') || name.contains(' ') {
        bail!("pick a model from the Ollama library list");
    }
    if !OLLAMA_LIBRARY.contains(&name) {
        bail!("pick a model from the Ollama library list");
    }
    Ok(())
}

pub fn pull_request_body(name: &str) -> Result<Value> {
    validate_library_pick(name)?;
    Ok(serde_json::json!({ "name": name.trim(), "stream": false }))
}

/// A detected local name, or a library name after it has been pulled
/// (the caller only passes models the tags endpoint just returned).
pub fn ollama_choice_allowed(chosen: &str, local: &[String]) -> bool {
    let chosen = chosen.trim();
    !chosen.is_empty() && local.iter().any(|name| name == chosen)
}

#[derive(Debug, Clone)]
pub struct ThinAnswers {
    pub provider: ThinProvider,
    pub openrouter_api_key: String,
    pub ollama_model: String,
    pub bot_token: String,
    pub system_prompt: String,
}

pub fn render_config(answers: &ThinAnswers) -> Result<String> {
    let token = answers.bot_token.trim();
    if token.is_empty() {
        bail!("bot token is required");
    }
    let sentence = answers.system_prompt.trim();
    if sentence.is_empty() {
        bail!("system prompt sentence is required");
    }

    let mut root = toml::map::Map::new();

    let mut telegram = toml::map::Map::new();
    telegram.insert("bot_token".into(), toml::Value::String(token.to_string()));
    telegram.insert(
        "allowed_user_ids".into(),
        toml::Value::Array(vec![toml::Value::Integer(CLOSED_ALLOWLIST_USER as i64)]),
    );
    root.insert("telegram".into(), toml::Value::Table(telegram));

    let mut openrouter = toml::map::Map::new();
    match answers.provider {
        ThinProvider::OpenRouter => {
            let key = answers.openrouter_api_key.trim();
            if key.is_empty() {
                bail!("OpenRouter API key is required");
            }
            openrouter.insert("api_key".into(), toml::Value::String(key.to_string()));
        }
        ThinProvider::Ollama => {
            let model = answers.ollama_model.trim();
            if model.is_empty() || model.contains(' ') || model.contains('/') {
                bail!("pick a detected Ollama model");
            }
            let mut provider = toml::map::Map::new();
            provider.insert("name".into(), toml::Value::String("ollama".into()));
            provider.insert("type".into(), toml::Value::String("ollama".into()));
            provider.insert(
                "base_url".into(),
                toml::Value::String(OLLAMA_PROVIDER_BASE.into()),
            );
            provider.insert("model".into(), toml::Value::String(model.to_string()));
            provider.insert("discover_models".into(), toml::Value::Boolean(true));
            root.insert(
                "provider".into(),
                toml::Value::Array(vec![toml::Value::Table(provider)]),
            );
            // Schema requires [openrouter]. The key is unused on this path.
            openrouter.insert("api_key".into(), toml::Value::String(String::new()));
        }
    }
    openrouter.insert(
        "system_prompt".into(),
        toml::Value::String(sentence.to_string()),
    );
    root.insert("openrouter".into(), toml::Value::Table(openrouter));

    let mut memory = toml::map::Map::new();
    memory.insert(
        "database_path".into(),
        toml::Value::String("rustfox.db".into()),
    );
    root.insert("memory".into(), toml::Value::Table(memory));

    toml::to_string(&toml::Value::Table(root)).context("serialize thin config")
}

/// Re-runs keep a real allowlist. A fresh file stays closed (`[0]`).
pub fn keep_existing_allowlist(existing: &str, wizard: &str) -> Result<String> {
    let existing_doc: toml::Value = toml::from_str(existing).context("existing config")?;
    let Some(ids) = allowlist_value(&existing_doc) else {
        return Ok(wizard.to_string());
    };
    let mut wizard_doc: toml::Value = toml::from_str(wizard).context("wizard config")?;
    let table = wizard_doc
        .as_table_mut()
        .context("wizard config root is not a table")?;
    let telegram = table
        .entry("telegram")
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let telegram = telegram
        .as_table_mut()
        .context("wizard [telegram] is not a table")?;
    telegram.insert("allowed_user_ids".into(), ids);
    toml::to_string(&wizard_doc).context("serialize wizard config")
}

fn allowlist_value(doc: &toml::Value) -> Option<toml::Value> {
    let from_telegram = doc
        .get("telegram")
        .and_then(|t| t.get("allowed_user_ids"))
        .and_then(|v| v.as_array())
        .filter(|ids| !ids.is_empty())
        .cloned();
    if let Some(ids) = from_telegram {
        return Some(toml::Value::Array(ids));
    }
    doc.get("bots")
        .and_then(|b| b.as_array())
        .and_then(|bots| bots.first())
        .and_then(|bot| bot.get("allowed_user_ids"))
        .and_then(|v| v.as_array())
        .filter(|ids| !ids.is_empty())
        .cloned()
        .map(toml::Value::Array)
}

/// `None` (no whitelist) keeps sandbox-safe tools on. An explicit list keeps
/// them on only when every sandbox-safe name is still present.
pub fn sandbox_safe_tools_stay_on(whitelist: Option<&[String]>) -> bool {
    match whitelist {
        None => true,
        Some(list) => SANDBOX_SAFE_TOOLS
            .iter()
            .all(|name| list.iter().any(|item| item == name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn openrouter_answers() -> ThinAnswers {
        ThinAnswers {
            provider: ThinProvider::OpenRouter,
            openrouter_api_key: "sk-or-test".into(),
            ollama_model: String::new(),
            bot_token: "123:abc".into(),
            system_prompt: "Be brief and kind.".into(),
        }
    }

    fn parse(toml_text: &str) -> Config {
        let mut cfg: Config = toml::from_str(toml_text).unwrap();
        cfg.normalize_bots().unwrap();
        cfg
    }

    #[test]
    fn wizard_asks_only_the_four_fields() {
        for provider in [ThinProvider::OpenRouter, ThinProvider::Ollama] {
            let fields = wizard_fields(provider);
            assert_eq!(fields.len(), 4);
            assert_eq!(fields[0], "provider");
            assert!(fields.contains(&"bot_token"));
            assert!(fields.contains(&"system_prompt"));
            let joined = fields.join(" ");
            for banned in [
                "tools",
                "mcp",
                "base_url",
                "model_id",
                "quantization",
                "huggingface",
                "langsmith",
                "allowed_user",
            ] {
                assert!(
                    !joined.contains(banned),
                    "{provider:?} field list contains {banned}: {joined}"
                );
            }
        }
        assert!(wizard_fields(ThinProvider::OpenRouter).contains(&"openrouter_api_key"));
        assert!(!wizard_fields(ThinProvider::OpenRouter).contains(&"ollama_model"));
        assert!(wizard_fields(ThinProvider::Ollama).contains(&"ollama_model"));
        assert!(!wizard_fields(ThinProvider::Ollama).contains(&"openrouter_api_key"));
        assert!(ThinProvider::parse("").unwrap() == ThinProvider::OpenRouter);
        assert!(ThinProvider::parse("lmstudio").is_err());
        assert!(ThinProvider::parse("huggingface").is_err());
    }

    #[test]
    fn openrouter_is_default_and_stores_the_sentence() {
        let text = render_config(&openrouter_answers()).unwrap();
        assert!(!text.contains("[[provider]]") && !text.contains("name = \"ollama\""));
        assert!(!text.contains("mcp"));
        assert!(!text.contains("tools"));
        assert!(!text.contains("langsmith"));
        let cfg = parse(&text);
        assert_eq!(cfg.openrouter.api_key, "sk-or-test");
        assert_eq!(cfg.openrouter.system_prompt, "Be brief and kind.");
        // Model omitted so the schema default stays the OpenRouter model.
        assert_eq!(cfg.openrouter.model, "moonshotai/kimi-k2.6");
        assert_eq!(cfg.openrouter.base_url, "https://openrouter.ai/api/v1");
        assert!(cfg.provider.is_empty());
        assert!(cfg.mcp_servers.is_empty());
        assert!(cfg.bots[0].tools.is_none());
        assert!(sandbox_safe_tools_stay_on(cfg.bots[0].tools.as_deref()));
        assert_eq!(cfg.bots[0].allowed_user_ids, vec![CLOSED_ALLOWLIST_USER]);
        let (providers, default_name, _) = cfg.build_providers();
        assert_eq!(default_name, "openrouter");
        assert_eq!(
            providers[0].provider_type,
            crate::config::ProviderType::OpenRouter
        );
    }

    #[test]
    fn ollama_config_uses_detected_model_and_fixed_base() {
        let answers = ThinAnswers {
            provider: ThinProvider::Ollama,
            openrouter_api_key: String::new(),
            ollama_model: "llama3.2:latest".into(),
            bot_token: "123:abc".into(),
            system_prompt: "Speak plainly.".into(),
        };
        let text = render_config(&answers).unwrap();
        assert!(!text.contains("mcp"));
        assert!(!text.contains("\ntools"));
        let cfg = parse(&text);
        assert_eq!(cfg.openrouter.system_prompt, "Speak plainly.");
        assert!(cfg.openrouter.api_key.is_empty());
        assert_eq!(cfg.provider.len(), 1);
        assert_eq!(cfg.provider[0].name, "ollama");
        assert_eq!(
            cfg.provider[0].provider_type,
            crate::config::ProviderType::Ollama
        );
        assert_eq!(cfg.provider[0].base_url, OLLAMA_PROVIDER_BASE);
        assert_eq!(cfg.provider[0].model, "llama3.2:latest");
        assert!(cfg.bots[0].tools.is_none());
        assert!(sandbox_safe_tools_stay_on(None));
        assert!(!sandbox_safe_tools_stay_on(Some(
            &["web_search".into()][..]
        )));
        let (_providers, default_name, _) = cfg.build_providers();
        assert_eq!(default_name, "ollama");
    }

    #[test]
    fn ollama_down_is_one_line_and_library_pull_waits_for_a_pick() {
        let down = detect_from_http(false, "");
        assert!(!down.running);
        assert!(down.models.is_empty());
        assert_eq!(down.message, Some(OLLAMA_NOT_RUNNING));
        assert!(!OLLAMA_NOT_RUNNING.contains('\n'));

        let body = r#"{"models":[{"name":"llama3.2:latest"},{"name":"qwen2.5:7b"}]}"#;
        let up = detect_from_http(true, body);
        assert!(up.running);
        assert!(up.message.is_none());
        assert_eq!(
            up.models,
            vec!["llama3.2:latest".to_string(), "qwen2.5:7b".to_string()]
        );
        assert!(ollama_choice_allowed("llama3.2:latest", &up.models));
        assert!(!ollama_choice_allowed("typed-by-hand", &up.models));

        let library = library_choices(&up.models);
        assert!(!library.contains(&"llama3.2"));
        assert!(library.contains(&"mistral"));
        for name in &library {
            assert!(!name.contains(':'));
            assert!(!name.contains('/'));
        }
        assert!(validate_library_pick("llama3.2:q4_K_M").is_err());
        assert!(validate_library_pick("huggingface/foo").is_err());
        assert!(validate_library_pick("not-a-library-model").is_err());
        let body = pull_request_body("mistral").unwrap();
        assert_eq!(body["name"], "mistral");
        assert_eq!(body["stream"], false);
        // Detecting tags does not build a pull. Pull exists only after a pick.
        assert!(pull_request_body("").is_err());
    }

    #[test]
    fn rerun_keeps_an_existing_allowlist() {
        let existing = r#"
            [telegram]
            bot_token = "old"
            allowed_user_ids = [42, 7]
            [openrouter]
            api_key = "k"
        "#;
        let wizard = render_config(&openrouter_answers()).unwrap();
        let kept = keep_existing_allowlist(existing, &wizard).unwrap();
        let cfg = parse(&kept);
        assert_eq!(cfg.telegram.allowed_user_ids, vec![42, 7]);
        assert_eq!(cfg.openrouter.system_prompt, "Be brief and kind.");
    }

    #[test]
    fn readme_locks_openrouter_default_and_ollama_library() {
        let readme = include_str!("../../README.md");
        assert!(!readme.contains("Self-hosted, no cloud dependency"));
        let lower = readme.to_ascii_lowercase();
        assert!(!lower.contains("inference stays on the machine"));
        assert!(!lower.contains("no cloud"));
        assert!(readme.contains("OpenRouter"));
        assert!(readme.contains("Ollama is the local option"));
        assert!(readme.contains("pulled from the Ollama library"));
    }

    #[test]
    fn wizard_page_has_no_tool_or_mcp_questions() {
        let html = include_str!("../../setup/index.html");
        assert!(html.contains("id=\"f-provider-openrouter\""));
        assert!(html.contains("id=\"f-provider-ollama\""));
        assert!(html.contains("id=\"f-telegram-token\""));
        assert!(html.contains("id=\"f-system-prompt\""));
        assert!(html.contains("id=\"f-openrouter-key\""));
        assert!(html.contains("<select id=\"f-ollama-model\""));
        assert!(html.contains("<select id=\"f-ollama-library\""));
        assert!(html.contains("Ollama is not running."));
        assert!(html.contains("Use OpenRouter"));
        assert!(!html.contains("id=\"f-allowed-ids\""));
        assert!(!html.contains("id=\"f-model\""));
        assert!(!html.contains("id=\"f-base-url\""));
        assert!(!html.contains("Add another bot"));
        assert!(!html.contains("id=\"mcp-catalog\""));
        assert!(!html.contains("id=\"step-3\""));
        assert!(!html.contains("Show all settings"));
    }
}
