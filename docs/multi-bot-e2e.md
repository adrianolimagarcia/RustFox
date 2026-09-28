# Multi-bot E2E checklist (§7.7)

Automated coverage (no live Telegram / no Update JSON injector):

```bash
cargo test --test multi_bot_e2e_gate
```

Live BotFather spot-check is **not** a merge-blocker when unit/docs land first (confirm with PO). Secrets via env / secret-request only — never chat.

## Test bots (recommended)

| Bot id       | Persona      | Purpose                                      | Env |
|--------------|--------------|----------------------------------------------|-----|
| `main`       | `main`       | Default identity; peer *caller*              | `RUSTFOX_TEST_BOT_TOKEN_MAIN` |
| `researcher` | `researcher` | Second token; peer *callee* via `invoke_agent` | `RUSTFOX_TEST_BOT_TOKEN_RESEARCHER` |

Both may share the same owner `allowed_user_ids` in v1 examples; per-bot allowlists are supported.

## Harness checklist

1. **Config** — `[[bots]]` with two tokens + shared sandbox/skills (wizard **Add another bot** or `/agents create`).
2. **Isolation** — both bots receive / reply independently; conversations keyed by `bot_id` (same human chatting two bots does not merge threads).
3. **Peer `via`** — from main, `invoke_agent` → researcher returns in the **caller** chat with `via researcher:` attribution; no Telegram bot↔bot visibility.
4. **Depth reject** — `max_peer_depth = 2` hard-stops deeper chains with a clear error.
5. **Allowlist** — rejection on one bot’s allowlist does not affect the other dispatcher.
6. **Secrets** — tokens only from env / secret-request (`RUSTFOX_TEST_BOT_TOKEN_*`); never paste into chat logs.

## Out of scope (backlog)

- E2E Update JSON injector harness
- `mcp::update_config_tokens` bak follow-up

## Related

- Design: multi-bot agent binding + peer invoke
- GUIDE: [Multi-bot](GUIDE.md#multi-bot)
