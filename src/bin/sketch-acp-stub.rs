//! `sketch-acp-stub` — a **test-support** stub ACP agent.
//!
//! This binary is NOT a product surface. It exists so the headless session-
//! server harness (`tests/session_resilience_test.rs`,
//! `tests/session_transcript_test.rs`) can drive a *real* ACP transcript
//! through the real `AcpChannelClient` (the client half, in
//! `src/acp_channel.rs`) and the real `sketch-session-server`, without
//! depending on a live Claude agent.
//!
//! It speaks the **agent** side of the Agent Client Protocol over stdio. The
//! framing is newline-delimited JSON (one JSON-RPC object per line) — the exact
//! framing the `agent-client-protocol` crate's `ByteStreams` connection uses,
//! and the same framing the existing hand-rolled Python fake agent in
//! `acp_channel.rs`'s unit tests relies on.
//!
//! Only the subset of methods the sketch client actually sends is handled:
//!   - `initialize`     → `{protocolVersion, agentCapabilities:{loadSession:true}}`
//!   - `session/new`    → `{sessionId}`
//!   - `session/load`   → replays the prior transcript as `session/update`
//!     notifications, then returns (used on resume)
//!   - `session/prompt` → streams `STUB_CHUNKS` `agent_message_chunk` updates,
//!     then returns `{stopReason:"end_turn"}`
//!   - `session/cancel` → (notification) ends the in-flight turn early
//!
//! Anything else (other notifications, unknown methods, `_meta`, the
//! system-prompt append) is ignored.
//!
//! ## Knobs (environment variables)
//!
//! - `STUB_CHUNKS=N`      — number of `agent_message_chunk` updates to stream
//!   per `session/prompt` (default 2). Set high (e.g.
//!   800) to force a LARGE event_log / replay.
//! - `STUB_CHUNK_TEXT=s`  — text emitted per chunk (default `"chunk "`). The
//!   chunk index is appended so chunks are distinct.
//! - `STUB_DELAY_MS=ms`   — delay between streamed chunks (default 0). Use a
//!   nonzero value to keep a turn streaming long enough
//!   for a mid-turn reconnect test.
//! - `STUB_REPLAY_USER=s` — if set, `session/load` emits one
//!   `user_message_chunk` with this text before its
//!   agent chunks (mirrors how a real agent re-emits the
//!   user's own prior turn on resume). Default: unset.
//!
//! The stub is intentionally single-threaded and synchronous: it reads one
//! request line, fully handles it (including streaming all chunks), then reads
//! the next. That's enough for the client, which awaits the `session/prompt`
//! response after consuming the streamed notifications.

use std::io::{BufRead, Write};

/// Read an env var as the given type, falling back to `default` when unset or
/// unparseable.
fn env_parsed<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    // Lock once; we own stdio for the process lifetime.
    let mut out = stdout.lock();

    let chunks: usize = env_parsed("STUB_CHUNKS", 2usize);
    let chunk_text = std::env::var("STUB_CHUNK_TEXT").unwrap_or_else(|_| "chunk ".to_string());
    let delay_ms: u64 = env_parsed("STUB_DELAY_MS", 0u64);
    let replay_user = std::env::var("STUB_REPLAY_USER").ok();

    // The session id the stub hands back from session/new. On session/load the
    // client supplies its own id (the one it's resuming); we honor whatever it
    // sends so the resumed transcript stays attributed to the same session.
    let mut session_id = "stub-session-1".to_string();

    fn emit(out: &mut dyn Write, obj: &serde_json::Value) {
        let mut line = serde_json::to_string(obj).expect("serialize");
        line.push('\n');
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }

    // Stream the agent's reply for one turn: optional leading user echo, then
    // `chunks` agent_message_chunk updates. Shared by session/prompt and the
    // session/load replay so both produce a transcript of the same shape.
    let stream_turn = |out: &mut dyn Write, sid: &str, user_echo: Option<&str>| {
        if let Some(text) = user_echo {
            emit(
                out,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": sid,
                        "update": {
                            "sessionUpdate": "user_message_chunk",
                            "content": {"type": "text", "text": text}
                        }
                    }
                }),
            );
        }
        for i in 0..chunks {
            emit(
                out,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": sid,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": format!("{chunk_text}{i}")}
                        }
                    }
                }),
            );
            if delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
        }
    };

    let reader = stdin.lock();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();

        match method {
            "initialize" => {
                // Advertise loadSession so the client will exercise
                // `session/load` on resume rather than silently falling back to
                // session/new.
                emit(
                    &mut out,
                    &serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": 1,
                            "agentCapabilities": {"loadSession": true}
                        }
                    }),
                );
            }
            "session/new" => {
                emit(
                    &mut out,
                    &serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"sessionId": session_id}
                    }),
                );
            }
            "session/load" => {
                // Adopt the id the client is resuming so replayed updates carry
                // the right sessionId.
                if let Some(sid) = msg
                    .get("params")
                    .and_then(|p| p.get("sessionId"))
                    .and_then(|s| s.as_str())
                {
                    session_id = sid.to_string();
                }
                // Replay one prior turn (user echo + agent chunks) as
                // notifications, THEN return the response — exactly the ordering
                // the client's worker relies on to emit ReplayComplete after the
                // last replayed chunk.
                stream_turn(&mut out, &session_id, replay_user.as_deref());
                emit(
                    &mut out,
                    &serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {}
                    }),
                );
            }
            "session/prompt" => {
                let sid = msg
                    .get("params")
                    .and_then(|p| p.get("sessionId"))
                    .and_then(|s| s.as_str())
                    .unwrap_or(&session_id)
                    .to_string();
                stream_turn(&mut out, &sid, None);
                emit(
                    &mut out,
                    &serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"stopReason": "end_turn"}
                    }),
                );
            }
            // Notifications and anything else: ignore. (session/cancel arrives
            // as a notification with no id; since we stream synchronously the
            // turn is already done by the time we'd see it, so dropping it is
            // correct — there is nothing in flight to cancel.)
            _ => {}
        }
    }
}
