//! LIVE integration test for the Telegram external-chat bridge
//! (spec-external-chat-bridge.md). Ignored by default — it needs real Telegram
//! credentials and a forum-enabled supergroup.
//!
//! ## What it asserts
//!
//! A round-trip against the REAL Telegram Bot API: create a forum topic
//! (`createForumTopic`), post a message into it (`sendMessage` with
//! `message_thread_id`) and edit that message (`editMessageText`) — the exact
//! outbound calls the bridge's `TelegramTransport` fold path makes — then close
//! the topic (`closeForumTopic`). This exercises the same HTTP surface the
//! production `TelegramTransport` drives (`bridge/telegram.rs`), so a green run
//! proves the token, chat, bot admin/Manage-Topics rights, and the forum-topic
//! API all line up end to end.
//!
//! NOTE ON SCOPE: `TelegramTransport` and the bridge loop live INSIDE the
//! `yalda-session-server` binary crate, so an external integration test cannot
//! name those types. This test therefore re-issues the identical Bot API calls
//! directly (same `ureq` client + JSON bodies as `bridge/telegram.rs`). It is
//! the spec's documented minimum: "assert the TelegramTransport can open+close a
//! forum topic against the live API." A full
//! GUI↔server↔agent↔Telegram loop is verification-harness gap #2 (the live
//! subprocess worker) and is out of scope for a headless test.
//!
//! ## Credentials (all REQUIRED; the test is a no-op skip if any is missing)
//!
//! - `YALDA_TELEGRAM_TOKEN`     — the bot token from @BotFather.
//! - `YALDA_TELEGRAM_CHAT_ID`   — the forum-enabled supergroup id (negative int).
//!   The bot must be an admin with the "Manage Topics" right.
//! - `YALDA_TELEGRAM_ALLOWED_IDS` — the allow-list (spec §7). Not consulted for
//!   the outbound round-trip, but required so the test fails loudly if the live
//!   bridge would refuse the operator; parsed and asserted non-empty.
//!
//! These are the SAME env vars `BridgeConfig::load` reads, so a passing run means
//! the live bridge would boot with this config.
//!
//! ## Run
//!
//!     YALDA_TELEGRAM_TOKEN=… YALDA_TELEGRAM_CHAT_ID=-100… \
//!     YALDA_TELEGRAM_ALLOWED_IDS=12345 \
//!     cargo test --test telegram_bridge_live -- --ignored --nocapture

use serde_json::json;

const API: &str = "https://api.telegram.org";

/// POST a Bot API method, returning its `result` value. Mirrors
/// `bridge/telegram.rs::call_blocking` (envelope handling + error surfacing).
fn call(token: &str, method: &str, body: serde_json::Value) -> serde_json::Value {
    let url = format!("{API}/bot{token}/{method}");
    let resp = match ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(body)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            panic!("telegram {method} HTTP {code}: {}", detail.trim());
        }
        Err(e) => panic!("telegram {method} request failed: {e}"),
    };
    let value: serde_json::Value = resp
        .into_json()
        .unwrap_or_else(|e| panic!("telegram {method} decode failed: {e}"));
    assert_eq!(
        value.get("ok").and_then(|b| b.as_bool()),
        Some(true),
        "telegram {method} not ok: {value}"
    );
    value.get("result").cloned().unwrap_or(serde_json::Value::Null)
}

#[test]
#[ignore = "live: needs YALDA_TELEGRAM_{TOKEN,CHAT_ID,ALLOWED_IDS} + a forum supergroup + the bot as admin"]
fn telegram_bridge_topic_roundtrip_live() {
    // Skip cleanly (not fail) when creds are absent, so `--ignored` in CI without
    // secrets doesn't red. A real run supplies all three.
    let (token, chat_id, allowed) = match (
        std::env::var("YALDA_TELEGRAM_TOKEN").ok().filter(|s| !s.trim().is_empty()),
        std::env::var("YALDA_TELEGRAM_CHAT_ID").ok().and_then(|s| s.trim().parse::<i64>().ok()),
        std::env::var("YALDA_TELEGRAM_ALLOWED_IDS").ok().filter(|s| !s.trim().is_empty()),
    ) {
        (Some(t), Some(c), Some(a)) => (t, c, a),
        _ => {
            eprintln!(
                "SKIP telegram_bridge_topic_roundtrip_live: set YALDA_TELEGRAM_TOKEN, \
                 YALDA_TELEGRAM_CHAT_ID, and YALDA_TELEGRAM_ALLOWED_IDS to run it."
            );
            return;
        }
    };

    // The allow-list must be non-empty and parse — the bridge refuses an empty
    // one (spec §7 / `config_refuses_empty_allowlist`). Assert the same here so a
    // misconfigured operator list fails the live check too.
    let ids: Vec<i64> = allowed
        .split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect();
    assert!(!ids.is_empty(), "YALDA_TELEGRAM_ALLOWED_IDS parsed to nothing: {allowed:?}");

    // 1) Open a forum topic — the per-session gesture (`createForumTopic`).
    let topic_name = format!("yalda-bridge-live-{}", std::process::id());
    let created = call(&token, "createForumTopic", json!({ "chat_id": chat_id, "name": topic_name }));
    let thread_id = created
        .get("message_thread_id")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| panic!("createForumTopic: no message_thread_id in {created}"));
    assert!(thread_id > 0, "forum topic id should be positive: {thread_id}");

    // 2) Post into the topic, then edit it — the outbound fold path
    //    (`sendMessage` + `editMessageText`). Wrapped so a failure still closes
    //    the topic below (no orphaned test topics).
    let roundtrip = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sent = call(
            &token,
            "sendMessage",
            json!({ "chat_id": chat_id, "message_thread_id": thread_id, "text": "bridge live: hello" }),
        );
        let message_id = sent
            .get("message_id")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| panic!("sendMessage: no message_id in {sent}"));

        call(
            &token,
            "editMessageText",
            json!({ "chat_id": chat_id, "message_id": message_id, "text": "bridge live: edited" }),
        );
    }));

    // 3) Always close the topic (`closeForumTopic`) so a run leaves no residue.
    call(&token, "closeForumTopic", json!({ "chat_id": chat_id, "message_thread_id": thread_id }));

    if let Err(e) = roundtrip {
        std::panic::resume_unwind(e);
    }
}
