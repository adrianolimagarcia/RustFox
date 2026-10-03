//! Multi-bot §7.7 automated gate (no live BotFather / no Update injector).
//!
//! Covers the checklist items that can be asserted without Telegram I/O:
//! conversation `bot_id` isolation, peer `via` attribution, depth reject,
//! and per-bot allowlist independence. Live checklist: `docs/multi-bot-e2e.md`.

use rustfox::agents_edit::append_bot_binding;
use rustfox::config::Config;
use rustfox::peer_invoke::{
    format_via_attribution, guard_peer_invoke, resolve_invoke_source, InvokeSource, MAX_PEER_DEPTH,
};
use rustfox::platform::{normalize_bot_id, user_on_allowlist, DEFAULT_BOT_ID};
use rustfox::setup::wizard::merge_wizard_save;
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

#[test]
fn e2e_gate_wizard_add_another_bot_bak_and_materialize() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, minimal_legacy_toml()).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    let store = rustfox::secret_store::FakeSecretStore::new();
    let r = append_bot_binding(
        &path,
        "researcher",
        "222222222:AANewTokenSecretValueYYYYYY",
        42,
        &store,
    )
    .unwrap();
    assert_eq!(r.id, "researcher");
    assert_eq!(r.persona, "researcher");
    assert!(r.bak_path.exists(), "must create config.toml.bak");
    assert_eq!(std::fs::read_to_string(&r.bak_path).unwrap(), before);

    let after = std::fs::read_to_string(&path).unwrap();
    let mut cfg: Config = toml::from_str(&after).unwrap();
    cfg.normalize_bots().unwrap();
    assert_eq!(cfg.bots.len(), 2, "legacy default + researcher");
    assert!(cfg.bots.iter().any(|b| b.id == "default"));
    assert!(cfg.bots.iter().any(|b| b.id == "researcher"));
}

/// Full wizard save must preserve secondary tools/model/persona/allowlist
/// when `[[bots]]` already exists (TL HOLD #74).
#[test]
fn e2e_gate_wizard_full_save_preserves_secondary_bot_fields() {
    let existing = r#"
[[bots]]
id = "main"
bot_token = "111111111:AAMainTokenSecretValueXXXX"
allowed_user_ids = [42]
persona = "main"

[[bots]]
id = "researcher"
bot_token = "222222222:AAResearcherTokenSecretYY"
allowed_user_ids = [42, 99]
persona = "researcher"
model = "moonshotai/kimi-k2.6"
tools = ["read_file", "list_files", "web_search", "invoke_agent"]

[openrouter]
api_key = "sk-old"
model = "old-model"

[sandbox]
allowed_directory = "/tmp"
"#;
    // generateToml-style body: [telegram] + sections, no [[bots]].
    let wizard = r#"
[telegram]
bot_token = "111111111:AAMainTokenSecretValueXXXX"
allowed_user_ids = [42]

[openrouter]
api_key = "sk-new"
model = "new-model"
"#;

    let merged = merge_wizard_save(existing, wizard).expect("merge");
    let mut cfg: Config = toml::from_str(&merged).unwrap();
    cfg.normalize_bots().unwrap();

    let research = cfg
        .bots
        .iter()
        .find(|b| b.id == "researcher")
        .expect("researcher must survive full wizard save");
    assert_eq!(research.persona, "researcher");
    assert_eq!(research.allowed_user_ids, vec![42, 99]);
    assert_eq!(research.model.as_deref(), Some("moonshotai/kimi-k2.6"));
    assert_eq!(
        research.tools,
        Some(vec![
            "read_file".into(),
            "list_files".into(),
            "web_search".into(),
            "invoke_agent".into(),
        ])
    );
    assert_eq!(cfg.openrouter.api_key, "sk-new");
}

#[test]
fn e2e_gate_conversation_bot_id_isolation_keys() {
    // Same human on two bots must not share a conversation key.
    let platform = "telegram";
    let user = "42";
    let key_main = (platform, normalize_bot_id("main"), user);
    let key_research = (platform, normalize_bot_id("researcher"), user);
    assert_ne!(
        key_main, key_research,
        "bot_id must isolate (platform, bot_id, user_id)"
    );
    assert_eq!(normalize_bot_id(""), DEFAULT_BOT_ID);
}

#[tokio::test]
async fn e2e_gate_memory_bot_id_isolates_open_conversations() {
    let mem = rustfox::memory::MemoryStore::open_in_memory().unwrap();
    let a = mem
        .get_or_create_conversation("telegram", "main", "42")
        .await
        .unwrap();
    let b = mem
        .get_or_create_conversation("telegram", "researcher", "42")
        .await
        .unwrap();
    assert_ne!(
        a, b,
        "same user on different bots must get distinct conversations"
    );
}

#[test]
fn e2e_gate_peer_via_attribution() {
    let body = format_via_attribution("researcher", "top findings");
    assert!(
        body.starts_with("via researcher:\n"),
        "peer summary must prepend via marker: {body}"
    );
    assert!(body.contains("top findings"));
}

#[test]
fn e2e_gate_peer_depth_reject() {
    assert_eq!(MAX_PEER_DEPTH, 2);
    assert!(guard_peer_invoke(&["main".into()], "researcher").is_ok());
    assert!(guard_peer_invoke(&["main".into(), "researcher".into()], "verifier").is_ok());
    let err = guard_peer_invoke(
        &["main".into(), "researcher".into(), "verifier".into()],
        "other",
    )
    .unwrap_err();
    assert!(
        err.contains("depth limit") && err.contains("max_peer_depth=2"),
        "{err}"
    );
}

#[test]
fn e2e_gate_peer_resolves_bot_persona() {
    let bots = vec![rustfox::config::BotConfig {
        id: "researcher".into(),
        bot_token: "t".into(),
        allowed_user_ids: vec![1],
        persona: "researcher".into(),
        system_prompt_file: None,
        model: None,
        tools: None,
        fully_silent: false,
    }];
    assert_eq!(
        resolve_invoke_source("researcher", false, false, &bots),
        Some(InvokeSource::BotPersona {
            bot_id: "researcher".into(),
            persona: "researcher".into(),
        })
    );
}

#[test]
fn e2e_gate_allowlist_isolation() {
    let main = vec![111u64];
    let researcher = vec![222u64];
    assert!(user_on_allowlist(&main, 111));
    assert!(!user_on_allowlist(&main, 222));
    assert!(user_on_allowlist(&researcher, 222));
    assert!(!user_on_allowlist(&researcher, 111));
}
