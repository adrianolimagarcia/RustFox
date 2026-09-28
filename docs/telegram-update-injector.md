# Telegram Update injector (QA harness)

In-process fixture harness for Telegram transport **without** Telegram Desktop, TDLib/userbot, or a live BotFather network path on the agent box.

PO approach: Update JSON injector + existing portal `e2e-verify.mjs`. Live Bot API spot-check stays manual / secret-request.

## What it covers

| Assert | How |
|--------|-----|
| Allowlist accept / reject | `UpdateInjector::route` / `allowlist_decision` |
| Text message → message handler shape | fixtures + `extract_incoming` |
| Callback (model / loop) | fixtures + `extract_callback` / `HandlerRoute` |
| Media (photo / document) | fixtures classify + caption/file_name without download |
| Outbound `sendMessage` | optional wiremock mock of Bot API (`Bot::set_api_url`) |

**Not covered here:** client bubble UX, real Telegram-originated taps, production identity, TDLib.

## Run

```bash
# Injector unit + integration (fixtures + mock Bot API)
cargo test --test telegram_update_injector
cargo test -p rustfox telegram_injector

# Keep green (existing gates)
cargo test
# portal (when preview is up): node e2e-verify.mjs
```

## Injector API (shape)

```rust
use rustfox::platform::{UpdateInjector, HandlerRoute, InjectedKind};

let inj = UpdateInjector::new([111_001u64]).with_bot_id("main");
let upd = UpdateInjector::parse_update_file("tests/fixtures/telegram/text_message.json")?;
// or: UpdateInjector::parse_update(json_str)? / parse_update_value(&value)?

assert_eq!(inj.route(&upd), HandlerRoute::Message);
assert_eq!(UpdateInjector::classify(&upd), InjectedKind::TextMessage);

let incoming = inj.extract_incoming(&upd).unwrap();
// platform, bot_id, user_id, chat_id, text, command, has_photo/document, …
```

Routes mirror `src/platform/telegram.rs` dispatcher branches:

- `HandlerRoute::Message` — allowlisted message
- `HandlerRoute::ModelCallback` — allowlisted non-loop callback
- `HandlerRoute::LoopCallback` — `"type":"loop"` callback (no allowlist in live dispatcher)
- `HandlerRoute::RejectedAllowlist` — filtered out
- `HandlerRoute::Unhandled` — other Update kinds

Shared predicates used by both live `run()` and the injector:

- `telegram_injector::message_passes_allowlist`
- `telegram_injector::callback_passes_allowlist`
- `telegram_injector::is_loop_callback`

## Fixtures

JSON files under `tests/fixtures/telegram/`:

| File | Purpose |
|------|---------|
| `text_message.json` | Allowlisted `/start` |
| `text_message_rejected.json` | Off-allowlist text |
| `photo_caption.json` | Photo + caption |
| `document_pdf.json` | Document + caption |
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
// (teloxide uses PascalCase method names in the URL path)
```

See `mock_bot_api_outbound_send_assert` in `tests/telegram_update_injector.rs` (uses `notify_startup` / `notify_shutdown`).

## Related

- Research / AC: Notion QA harness task
- Portal-only E2E: `e2e-verify.mjs`, `web/e2e/smoke.mjs`
- Multi-bot gate (no injector): `docs/multi-bot-e2e.md`, `cargo test --test multi_bot_e2e_gate`
