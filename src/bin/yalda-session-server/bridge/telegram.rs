//! `TelegramTransport` — the real [`ChatTransport`] over the Telegram Bot API.
//!
//! Talks HTTP with the blocking `ureq` client (already used by the Linear app),
//! wrapped in `spawn_blocking` so it doesn't stall the async runtime. Inbound is
//! long-poll `getUpdates` with a monotonic `offset`; outbound is
//! `sendMessage`/`editMessageText` with `message_thread_id`, plus the forum
//! topic ops (`create`/`close`/`reopen`/`edit`ForumTopic).
//!
//! Setup (spec §4a): a forum-enabled supergroup, the bot an admin with the
//! Manage Topics right. `chat_id` is that group.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::json;

use super::transport::{ChatTransport, InboundMsg, MessageId, ThreadId, TransportResult};

/// Long-poll timeout handed to `getUpdates` (seconds). The request blocks
/// server-side up to this long when idle; because polling runs on its own task
/// this doesn't stall the bridge.
const LONG_POLL_SECS: u64 = 25;

#[derive(Clone)]
pub struct TelegramTransport {
    inner: Arc<Inner>,
}

struct Inner {
    token: String,
    chat_id: i64,
    /// `getUpdates` cursor: one past the highest processed `update_id`.
    offset: AtomicI64,
}

impl TelegramTransport {
    pub fn new(token: String, chat_id: i64) -> Self {
        Self {
            inner: Arc::new(Inner {
                token,
                chat_id,
                offset: AtomicI64::new(0),
            }),
        }
    }

    fn base(&self) -> String {
        format!("https://api.telegram.org/bot{}", self.inner.token)
    }

    /// POST a Bot API method with a JSON body, returning its `result` value.
    /// Runs the blocking `ureq` call on the blocking pool.
    async fn call(&self, method: &str, body: serde_json::Value) -> TransportResult<serde_json::Value> {
        let url = format!("{}/{method}", self.base());
        let method = method.to_string();
        tokio::task::spawn_blocking(move || call_blocking(&url, &method, body))
            .await
            .map_err(|e| format!("telegram call join error: {e}"))?
    }
}

/// The blocking HTTP call + Bot API envelope handling.
fn call_blocking(url: &str, method: &str, body: serde_json::Value) -> TransportResult<serde_json::Value> {
    let resp = match ureq::post(url)
        .set("Content-Type", "application/json")
        .send_json(body)
    {
        Ok(r) => r,
        // Telegram returns 4xx with a JSON `description` for API-level errors;
        // surface that rather than a bare status.
        Err(ureq::Error::Status(code, r)) => {
            let detail = r.into_string().unwrap_or_default();
            return Err(format!("telegram {method} HTTP {code}: {}", detail.trim()));
        }
        Err(e) => return Err(format!("telegram {method} request failed: {e}")),
    };
    let value: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("telegram {method} decode failed: {e}"))?;
    if value.get("ok").and_then(|b| b.as_bool()) != Some(true) {
        let desc = value
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("unknown error");
        return Err(format!("telegram {method} not ok: {desc}"));
    }
    Ok(value.get("result").cloned().unwrap_or(serde_json::Value::Null))
}

impl ChatTransport for TelegramTransport {
    async fn poll_inbound(&self) -> TransportResult<Vec<InboundMsg>> {
        let offset = self.inner.offset.load(Ordering::Acquire);
        let body = json!({
            "offset": offset,
            "timeout": LONG_POLL_SECS,
            "allowed_updates": ["message"],
        });
        let result = self.call("getUpdates", body).await?;
        let updates = result.as_array().cloned().unwrap_or_default();

        let mut out = Vec::new();
        let mut max_update_id = offset - 1;
        for upd in &updates {
            if let Some(id) = upd.get("update_id").and_then(|v| v.as_i64()) {
                max_update_id = max_update_id.max(id);
            }
            let Some(message) = upd.get("message") else {
                continue;
            };
            // Only messages from our configured group, that carry text.
            let chat_id = message
                .get("chat")
                .and_then(|c| c.get("id"))
                .and_then(|v| v.as_i64());
            if chat_id != Some(self.inner.chat_id) {
                continue;
            }
            let Some(text) = message.get("text").and_then(|v| v.as_str()) else {
                continue;
            };
            let from_user = message
                .get("from")
                .and_then(|f| f.get("id"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            // `message_thread_id` present ⇒ a forum topic; absent ⇒ General.
            let thread = message
                .get("message_thread_id")
                .and_then(|v| v.as_i64())
                .map(ThreadId)
                .unwrap_or(ThreadId::GENERAL);
            out.push(InboundMsg {
                thread,
                from_user,
                text: text.to_string(),
            });
        }
        // Advance the cursor past everything we saw (even filtered updates, so
        // we don't re-fetch them).
        if max_update_id >= offset {
            self.inner
                .offset
                .store(max_update_id + 1, Ordering::Release);
        }
        Ok(out)
    }

    async fn send(&self, thread: ThreadId, text: &str) -> TransportResult<MessageId> {
        let mut body = json!({
            "chat_id": self.inner.chat_id,
            "text": text,
        });
        if thread != ThreadId::GENERAL {
            body["message_thread_id"] = json!(thread.0);
        }
        let result = self.call("sendMessage", body).await?;
        let id = result
            .get("message_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| "sendMessage: no message_id".to_string())?;
        Ok(MessageId(id))
    }

    async fn edit(&self, _thread: ThreadId, message: MessageId, text: &str) -> TransportResult<()> {
        let body = json!({
            "chat_id": self.inner.chat_id,
            "message_id": message.0,
            "text": text,
        });
        self.call("editMessageText", body).await?;
        Ok(())
    }

    async fn open_thread(&self, name: &str) -> TransportResult<ThreadId> {
        let body = json!({ "chat_id": self.inner.chat_id, "name": name });
        let result = self.call("createForumTopic", body).await?;
        let id = result
            .get("message_thread_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| "createForumTopic: no message_thread_id".to_string())?;
        Ok(ThreadId(id))
    }

    async fn close_thread(&self, thread: ThreadId) -> TransportResult<()> {
        let body = json!({ "chat_id": self.inner.chat_id, "message_thread_id": thread.0 });
        self.call("closeForumTopic", body).await?;
        Ok(())
    }

    async fn reopen_thread(&self, thread: ThreadId) -> TransportResult<()> {
        let body = json!({ "chat_id": self.inner.chat_id, "message_thread_id": thread.0 });
        self.call("reopenForumTopic", body).await?;
        Ok(())
    }

    async fn rename_thread(&self, thread: ThreadId, name: &str) -> TransportResult<()> {
        let body = json!({
            "chat_id": self.inner.chat_id,
            "message_thread_id": thread.0,
            "name": name,
        });
        self.call("editForumTopic", body).await?;
        Ok(())
    }
}
