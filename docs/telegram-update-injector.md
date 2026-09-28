# Telegram Update injector (QA harness)

In-process fixture harness for Telegram transport **without** Telegram Desktop, TDLib/userbot, live BotFather, or live OpenRouter on the agent box.

PO approach: Update JSON injector → real `handle_message` + stub/fixture LLM + existing portal `e2e-verify.mjs`. Live Bot API spot-check stays manual / secret-request (parallel wait-Kan).

## What it covers

| Assert | How |
|--------|-----|
| Allowlist accept / reject | `UpdateInjector::route` / `allowlist_decision` |
| Text message → message handler shape | fixtures + `extract_incoming` |
| Callback (model / loop) | fixtures + `extract_callback` / `HandlerRoute` |
| Media (photo / document) | fixtures classify + caption/file_name without download |
| Outbound `sendMessage` | wiremock mock of Bot API (`Bot::set_api_url`) |
| **Full `handle_message` path** | `UpdateInjector::drive_handle_message` + real Agent |
| **Stub/fixture LLM (offline chat)** | `FixtureLlm` — deterministic reply, no OpenRouter |

**Not covered here:** client bubble UX, real Telegram-originated taps, production identity, TDLib, live BotFather tokens, media download bytes (shape only).

## Run

```bash
# Injector unit + integration (fixtures + mock Bot API + handle_message + FixtureLlm)
cargo test --test telegram_update_injector
cargo test -p rustfox telegram_injector

# Keep green (existing gates)
cargo test
# portal (when preview is up): node e2e-verify.mjs
```

## Injector API (shape)

```rust
use rustfox::platform::{UpdateInjector, HandlerRoute, InjectedKind, FixtureLlm};

let inj = UpdateInjector::new([111_001u64]).with_bot_id("main");
let upd = UpdateInjector::parse_update_file("tests/fixtures/telegram/text_message.json")?;
// or: UpdateInjector::parse_update(json_str)? / parse_update_value(&value)?

assert_eq!(inj.route(&upd), HandlerRoute::Message);
assert_eq!(UpdateInjector::classify(&upd), InjectedKind::TextMessage);

let incoming = inj.extract_incoming(&upd).unwrap();
// platform, bot_id, user_id, chat_id, text, command, has_photo/document, …

// Full handle_message path (Agent + mock Bot + FixtureLlm):
// let registry = FixtureLlm::new("deterministic reply").into_registry();
// … build Agent with that registry …
// inj.drive_handle_message(&upd, bot, agent).await?;
```

Routes mirror `src/platform/telegram.rs` dispatcher branches:

- `HandlerRoute::Message` — allowlisted message → `drive_handle_message` calls real `handle_message`
- `HandlerRoute::ModelCallback` — allowlisted non-loop callback
- `HandlerRoute::LoopCallback` — `"type":"loop"` callback (no allowlist in live dispatcher)
- `HandlerRoute::RejectedAllowlist` — filtered out (driver no-ops)
- `HandlerRoute::Unhandled` — other Update kinds

Shared predicates used by both live `run()` and the injector:

- `telegram_injector::message_passes_allowlist`
- `telegram_injector::callback_passes_allowlist`
- `telegram_injector::is_loop_callback`

### Stub LLM

`FixtureLlm` implements `Provider` and returns a fixed assistant string (no tool calls). Register via `FixtureLlm::new(reply).into_registry()` as the Agent's `ProviderRegistry`. CI needs **no** OpenRouter key.

## Fixtures

JSON files under `tests/fixtures/telegram/`:

| File | Purpose |
|------|---------|
| `text_message.json` | Allowlisted `/start` |
| `text_message_rejected.json` | Off-allowlist text |
| `slash_clear.json` | Allowlisted `/clear` |
| `slash_tools.json` | Allowlisted `/tools` |
| `slash_verbose.json` | Allowlisted `/verbose` |
| `chat_hello.json` | Free-text chat → full `handle_message` + FixtureLlm |
| `photo_caption.json` | Photo + caption (shape; no download assert) |
| `document_pdf.json` | Document + caption (shape) |
| `callback_model.json` | Model-picker callback |
| `callback_loop.json` | Loop-detector callback |

Add new cases as Bot API `Update` JSON (same shape as `getUpdates` / webhook bodies). Deserialize via teloxide’s `Update` type.

## Optional mock Bot API (outbound)

Point a teloxide `Bot` at wiremock instead of `https://api.telegram.org`:

```rust
let server = wiremock::MockServer::start().await;
let bot = teloxide::Bot::new("000:TOKEN")
    .set_api_url(reqwest::Url::parse(&format!("{}/", server.uri())).unwrap());
// Mount POST /bot{token}/SendMessage → {"ok":true,"result":{…Message…}}
// Also DeleteMessage / EditMessageText for handle_message streaming + silent UI.
// (teloxide uses PascalCase method names in the URL path)
```

See `HandleMessageHarness` / `drive_handle_message_*` in `tests/telegram_update_injector.rs`.


## Offline notes (CI)

- Point teloxide `Bot` at wiremock via `Bot::set_api_url`.
- Prefer per-user `message_format=markdown` in the harness: default `auto`/`rich` uses `sendRichMessage` against hard-coded `https://api.telegram.org` (bypasses `set_api_url`). Entities path stays on the mock Bot.
- `FixtureLlm` covers chat turns; slash commands (`/start`, `/clear`, `/tools`, …) do not call the LLM.

## Related

- Research / AC: Notion QA harness follow-up (deepen #75)
- Portal-only E2E: `e2e-verify.mjs`, `web/e2e/smoke.mjs`
- Multi-bot gate (no injector): `docs/multi-bot-e2e.md`, `cargo test --test multi_bot_e2e_gate`
