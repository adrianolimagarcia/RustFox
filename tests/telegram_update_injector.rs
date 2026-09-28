//! Telegram Update injector harness — no live Bot API / no Desktop.
//!
//! ```bash
//! cargo test --test telegram_update_injector
//! ```
//!
//! See `docs/telegram-update-injector.md`.

use rustfox::platform::telegram::{notify_shutdown, notify_startup};
use rustfox::platform::{
    AllowlistDecision, HandlerRoute, InjectedKind, UpdateInjector, DEFAULT_BOT_ID,
};
use serde_json::json;
use teloxide::prelude::*;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/telegram");
const ALLOWED: u64 = 111_001;
const TOKEN: &str = "000000000:QA-INJECTOR-MOCK-TOKEN";

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(FIXTURE_DIR).join(name)
}

fn injector() -> UpdateInjector {
    UpdateInjector::new([ALLOWED]).with_bot_id("main")
}

/// Minimal Message JSON accepted as `sendMessage` result by teloxide.
fn ok_message_result(chat_id: i64, text: &str) -> serde_json::Value {
    json!({
        "ok": true,
        "result": {
            "message_id": 1,
            "date": 1700000000,
            "chat": { "id": chat_id, "type": "private", "first_name": "QA" },
            "text": text
        }
    })
}

/// Wiremock stand-in for `https://api.telegram.org` (teloxide `Bot::set_api_url`).
struct MockTelegramApi {
    server: MockServer,
    token: String,
}

impl MockTelegramApi {
    async fn start() -> Self {
        Self {
            server: MockServer::start().await,
            token: TOKEN.to_string(),
        }
    }

    fn bot(&self) -> Bot {
        let url = reqwest::Url::parse(&format!("{}/", self.server.uri()))
            .expect("mock server URI is a valid URL");
        Bot::new(&self.token).set_api_url(url)
    }

    async fn stub_send_message(&self) {
        // teloxide posts to `/bot{token}/SendMessage` (PascalCase method name).
        Mock::given(method("POST"))
            .and(path_regex(r"^/bot[^/]+/SendMessage$"))
            .respond_with(|req: &Request| {
                let body: serde_json::Value =
                    serde_json::from_slice(&req.body).unwrap_or_else(|_| json!({}));
                let chat_id = body
                    .get("chat_id")
                    .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
                    .unwrap_or(0);
                let text = body
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                ResponseTemplate::new(200).set_body_json(ok_message_result(chat_id, &text))
            })
            .expect(1..)
            .mount(&self.server)
            .await;
    }

    async fn received_send_message_bodies(&self) -> Vec<serde_json::Value> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.url.path().to_ascii_lowercase().contains("sendmessage"))
            .filter_map(|r| serde_json::from_slice(&r.body).ok())
            .collect()
    }
}

#[test]
fn fixture_text_message_accepted() {
    let upd = UpdateInjector::parse_update_file(fixture("text_message.json")).unwrap();
    assert_eq!(UpdateInjector::classify(&upd), InjectedKind::TextMessage);
    let inj = injector();
    assert_eq!(inj.route(&upd), HandlerRoute::Message);
    let incoming = inj.extract_incoming(&upd).unwrap();
    assert_eq!(incoming.bot_id, "main");
    assert_eq!(incoming.user_id, ALLOWED.to_string());
    assert_eq!(incoming.command, Some(("start".into(), "".into())));
    assert_eq!(incoming.platform, "telegram");
}

#[test]
fn fixture_text_message_rejected_off_allowlist() {
    let upd = UpdateInjector::parse_update_file(fixture("text_message_rejected.json")).unwrap();
    let inj = injector();
    assert_eq!(inj.route(&upd), HandlerRoute::RejectedAllowlist);
    assert!(matches!(
        inj.allowlist_decision(&upd),
        AllowlistDecision::Reject {
            user_id: Some(999_999)
        }
    ));
    assert!(inj.extract_incoming(&upd).is_none());
}

#[test]
fn fixture_photo_media_handler_shape() {
    let upd = UpdateInjector::parse_update_file(fixture("photo_caption.json")).unwrap();
    assert_eq!(
        UpdateInjector::classify(&upd),
        InjectedKind::Media {
            has_photo: true,
            has_document: false
        }
    );
    let incoming = injector().extract_incoming(&upd).unwrap();
    assert!(incoming.has_photo);
    assert!(!incoming.has_document);
    assert_eq!(incoming.text, "look at this image");
}

#[test]
fn fixture_document_media_handler_shape() {
    let upd = UpdateInjector::parse_update_file(fixture("document_pdf.json")).unwrap();
    assert_eq!(
        UpdateInjector::classify(&upd),
        InjectedKind::Media {
            has_photo: false,
            has_document: true
        }
    );
    let incoming = injector().extract_incoming(&upd).unwrap();
    assert!(incoming.has_document);
    assert_eq!(incoming.document_file_name.as_deref(), Some("report.pdf"));
    assert_eq!(incoming.text, "quarterly report");
}

#[test]
fn fixture_callback_model_accepted() {
    let upd = UpdateInjector::parse_update_file(fixture("callback_model.json")).unwrap();
    assert_eq!(UpdateInjector::classify(&upd), InjectedKind::CallbackQuery);
    let inj = injector();
    assert_eq!(inj.route(&upd), HandlerRoute::ModelCallback);
    let cb = inj.extract_callback(&upd).unwrap();
    assert!(!cb.is_loop_callback);
    assert_eq!(cb.data.as_deref(), Some("model:openai/gpt-4o-mini"));
}

#[test]
fn fixture_callback_loop_route() {
    let upd = UpdateInjector::parse_update_file(fixture("callback_loop.json")).unwrap();
    let inj = injector();
    assert_eq!(inj.route(&upd), HandlerRoute::LoopCallback);
    assert!(inj.extract_callback(&upd).unwrap().is_loop_callback);
}

#[test]
fn allowlist_isolation_across_bots() {
    let upd = UpdateInjector::parse_update_file(fixture("text_message.json")).unwrap();
    let main = UpdateInjector::new([ALLOWED]).with_bot_id("main");
    let researcher = UpdateInjector::new([222u64]).with_bot_id("researcher");
    assert_eq!(main.route(&upd), HandlerRoute::Message);
    assert_eq!(researcher.route(&upd), HandlerRoute::RejectedAllowlist);
}

#[test]
fn default_bot_id_when_empty() {
    let inj = UpdateInjector::new([1u64]).with_bot_id("  ");
    assert_eq!(inj.bot_id(), DEFAULT_BOT_ID);
}

#[tokio::test]
async fn mock_bot_api_outbound_send_assert() {
    let api = MockTelegramApi::start().await;
    api.stub_send_message().await;
    let bot = api.bot();

    notify_startup(&bot, &[ALLOWED], "test-model", 0, 0, false).await;

    let bodies = api.received_send_message_bodies().await;
    assert!(
        !bodies.is_empty(),
        "expected at least one SendMessage to mock Bot API"
    );
    let chat_id = bodies[0]
        .get("chat_id")
        .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)))
        .unwrap();
    assert_eq!(chat_id, ALLOWED as i64);
    let text = bodies[0].get("text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        text.contains("RustFox is online"),
        "unexpected startup text: {text}"
    );

    notify_shutdown(&bot, &[ALLOWED]).await;
    let bodies = api.received_send_message_bodies().await;
    assert!(
        bodies.iter().any(|b| {
            b.get("text")
                .and_then(|v| v.as_str())
                .is_some_and(|t| t.contains("going offline"))
        }),
        "expected shutdown sendMessage; got {bodies:?}"
    );
}
