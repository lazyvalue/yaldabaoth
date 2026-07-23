//! Transport abstraction for the external chat bridge.
//!
//! `ChatTransport` is the seam the bridge task talks through: Telegram is the
//! first implementation (`telegram.rs`); WhatsApp/Signal/Slack are later impls
//! of the same trait (spec-external-chat-bridge.md §2, §6). The trait is
//! **topic-aware** — each agent session maps to one chat thread (a Telegram
//! forum topic), addressed by [`ThreadId`].
//!
//! Methods return `impl Future + Send` (edition-2024 native async-in-trait)
//! rather than using `async fn`, so the bridge task stays `Send` when spawned
//! on the multi-threaded tokio runtime without pulling in `async-trait`.

use std::future::Future;
#[cfg(test)]
use std::sync::Mutex;

/// A chat "thread" — a Telegram forum topic's `message_thread_id`. One per
/// agent session (spec §4). `0` is reserved for the forum's **General** topic
/// (the control channel for `/new` and cross-session listing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadId(pub i64);

impl ThreadId {
    /// The forum's General topic — the control channel, mapped to no session.
    pub const GENERAL: ThreadId = ThreadId(0);
}

/// An outbound message id, so the bridge can `edit` a message it is streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MessageId(pub i64);

/// One inbound chat message, normalized across transports. `thread` is the
/// topic it arrived in (`GENERAL` for the forum root); `from_user` is the
/// sender's platform id, checked against the allowlist before anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMsg {
    pub thread: ThreadId,
    pub from_user: i64,
    pub text: String,
}

/// Transport-agnostic result. Errors are human-readable strings — the bridge
/// logs and continues rather than crashing on a transient send failure.
pub type TransportResult<T> = Result<T, String>;

/// The chat surface the bridge drives. One impl per platform.
pub trait ChatTransport: Send + Sync + 'static {
    /// Drain any inbound messages since the last poll (Telegram long-poll
    /// `getUpdates`; a webhook transport would buffer and return them here).
    fn poll_inbound(&self) -> impl Future<Output = TransportResult<Vec<InboundMsg>>> + Send;

    /// Post a new message into a topic. Returns its id so it can be edited
    /// while a turn streams.
    fn send(&self, thread: ThreadId, text: &str)
    -> impl Future<Output = TransportResult<MessageId>> + Send;

    /// Live-edit a previously-sent message (streaming coalesced prose, §5).
    // Called by the T-004 event fold; part of the trait contract now.
    #[allow(dead_code)]
    fn edit(
        &self,
        thread: ThreadId,
        message: MessageId,
        text: &str,
    ) -> impl Future<Output = TransportResult<()>> + Send;

    /// Create a forum topic for a new session; returns its `ThreadId`.
    fn open_thread(&self, name: &str) -> impl Future<Output = TransportResult<ThreadId>> + Send;

    /// Close a topic (session ended) — history preserved, thread greyed out.
    fn close_thread(&self, thread: ThreadId) -> impl Future<Output = TransportResult<()>> + Send;

    /// Reopen a previously-closed topic (session resumed).
    // Wired when session-resume drives a Reopen; part of the trait contract now.
    #[allow(dead_code)]
    fn reopen_thread(&self, thread: ThreadId) -> impl Future<Output = TransportResult<()>> + Send;

    /// Rename a topic (session label changed).
    fn rename_thread(
        &self,
        thread: ThreadId,
        name: &str,
    ) -> impl Future<Output = TransportResult<()>> + Send;
}

// ── FakeTransport (headless tests) ──────────────────────────────────

/// One captured outbound operation, for test assertions.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeOp {
    Open { thread: ThreadId, name: String },
    Close(ThreadId),
    Reopen(ThreadId),
    Rename { thread: ThreadId, name: String },
    Send { thread: ThreadId, text: String, message: MessageId },
    Edit { thread: ThreadId, message: MessageId, text: String },
}

#[cfg(test)]
#[derive(Default)]
struct FakeState {
    ops: Vec<FakeOp>,
    inbound: std::collections::VecDeque<InboundMsg>,
    next_thread: i64,
    next_message: i64,
}

/// In-memory transport for headless tests: captures every outbound op into an
/// inspectable log and replays scripted inbound messages. Cheap to clone
/// (shared state) so a test keeps a handle to assert on while the bridge task
/// owns another.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct FakeTransport {
    state: std::sync::Arc<Mutex<FakeState>>,
}

#[cfg(test)]
impl FakeTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue an inbound message the next `poll_inbound` will return.
    #[allow(dead_code)] // used by the full-loop inbound test (T-006)
    pub fn push_inbound(&self, msg: InboundMsg) {
        self.state.lock().unwrap().inbound.push_back(msg);
    }

    /// Snapshot of every outbound op captured so far.
    pub fn ops(&self) -> Vec<FakeOp> {
        self.state.lock().unwrap().ops.clone()
    }

    fn record(&self, op: FakeOp) {
        self.state.lock().unwrap().ops.push(op);
    }
}

#[cfg(test)]
impl ChatTransport for FakeTransport {
    async fn poll_inbound(&self) -> TransportResult<Vec<InboundMsg>> {
        let mut st = self.state.lock().unwrap();
        Ok(st.inbound.drain(..).collect())
    }

    async fn send(&self, thread: ThreadId, text: &str) -> TransportResult<MessageId> {
        let message = {
            let mut st = self.state.lock().unwrap();
            st.next_message += 1;
            MessageId(st.next_message)
        };
        self.record(FakeOp::Send {
            thread,
            text: text.to_string(),
            message,
        });
        Ok(message)
    }

    async fn edit(&self, thread: ThreadId, message: MessageId, text: &str) -> TransportResult<()> {
        self.record(FakeOp::Edit {
            thread,
            message,
            text: text.to_string(),
        });
        Ok(())
    }

    async fn open_thread(&self, name: &str) -> TransportResult<ThreadId> {
        let thread = {
            let mut st = self.state.lock().unwrap();
            st.next_thread += 1;
            ThreadId(st.next_thread)
        };
        self.record(FakeOp::Open {
            thread,
            name: name.to_string(),
        });
        Ok(thread)
    }

    async fn close_thread(&self, thread: ThreadId) -> TransportResult<()> {
        self.record(FakeOp::Close(thread));
        Ok(())
    }

    async fn reopen_thread(&self, thread: ThreadId) -> TransportResult<()> {
        self.record(FakeOp::Reopen(thread));
        Ok(())
    }

    async fn rename_thread(&self, thread: ThreadId, name: &str) -> TransportResult<()> {
        self.record(FakeOp::Rename {
            thread,
            name: name.to_string(),
        });
        Ok(())
    }
}
