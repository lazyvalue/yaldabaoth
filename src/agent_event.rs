//! Canonical agent-fact vocabulary (spec-event-stream §1/§2/§8).
//!
//! Phase 8, Stage A (PRODUCER COLLAPSE, additive). There is exactly **one**
//! agent-fact type — [`AgentEvent`] — sourced once at the worker, forwarded
//! verbatim (worker → server → GUI), and never re-inferred downstream. It folds
//! the 11 [`ReplyEvent`](crate::acp_channel::ReplyEvent) arms plus the duplicate
//! `Notification::{ReplyEvent, TurnEnded, UserPrompt}` lifecycle variants into a
//! single self-attributing envelope.
//!
//! ## Additive rollout (spec §9)
//!
//! This vocabulary ships ALONGSIDE the existing `ReplyEvent` / inference path —
//! nothing is deleted this pass. The server forwards `Notification::Agent { event }`
//! next to the legacy `Notification::{ReplyEvent, TurnEnded, UserPrompt}`; the
//! idempotent `(generation, turn)` finalize is the backstop that neutralises the
//! duplicate `TurnEnded`. Deleting the legacy path is a follow-up after real-
//! session soak.
//!
//! ## Identity envelope (spec §2)
//!
//! Every fact carries `(session_id, generation, turn, seq)`:
//! - `session_id` — total attribution; a consumer routes a raw [`AgentEvent`]
//!   with zero out-of-band state.
//! - `generation` — channel-respawn token; the single uniform rebaseline signal
//!   (spec §4). Rides the FIRST event of every (re)spawned channel
//!   ([`AgentEventKind::ChannelOpened`]).
//! - `turn` — the authoritative `k` (spec §5), forwarded verbatim from the log.
//! - `seq` — monotonic per `(session_id, generation)`; the ordering key AND the
//!   compaction-safe cursor base. A **logical offset**, never a `Vec` index.
//!   The worker stamps a local ordering seq; the server's `record()` chokepoint
//!   assigns the authoritative durable seq under its lock (spec §3).
//!
//! ## Evolution (spec §8) — the load-bearing part
//!
//! The durable WAL is forwarded across GUI versions (candidate/promote co-attach
//! a new + an old GUI to one server session), so an old decoder MUST round-trip a
//! newer variant **byte-faithfully**. Stock `#[serde(tag = "kind")]` errors on an
//! unknown tag and `#[serde(other)]` is unit-only (drops the payload) — both
//! unacceptable. [`AgentEventKind`] therefore HAND-WRITES both `Serialize` and
//! `Deserialize`: an unknown `kind` lands in [`AgentEventKind::Unknown`]
//! preserving the whole object, and on the way out it is re-emitted under its
//! ORIGINAL `kind` tag (NOT `{"kind":"unknown"}`).

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::acp_channel::{
    Plan, ReplyEvent, SessionModeId, ToolCall, ToolCallUpdate, UsageSnapshot,
};
use crate::session_proto::ServerSessionId;

/// Canonical, durably-logged, forwarded-verbatim agent fact (spec §1/§2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentEvent {
    /// Sourced at the worker, NOT grafted at the server hop (spec §2).
    pub session_id: ServerSessionId,
    /// Channel-respawn token; rides the FIRST event of a channel (spec §4).
    pub generation: u64,
    /// Authoritative `k` (spec §5); forwarded verbatim from the durable log.
    pub turn: u64,
    /// Monotonic per `(session, generation)`; ordering + cursor base (spec §3).
    /// LOGICAL offset, never a `Vec` index.
    pub seq: u64,
    /// Flattened into the envelope (spec §2): `{..,"kind":"chunk","text":..}`
    /// so the `kind` discriminator and its payload sit alongside the identity
    /// fields. This is also what makes the `Unknown` byte-preservation (spec §8)
    /// work — an unknown variant's extra fields land at the envelope level.
    ///
    /// MINOR (6) — FRAGILITY UNDER NON-SELF-DESCRIBING FORMATS: `#[serde(flatten)]`
    /// makes serde buffer the whole struct into an internal `Content` map and
    /// re-deserialize fields from it; combined with the custom `AgentEventKind`
    /// `Deserialize` (which round-trips through `serde_json::Value`, see below),
    /// the envelope's `u64` fields (`generation`/`turn`/`seq`) are decoded from
    /// that intermediate, not directly from the wire. This is sound for
    /// SELF-DESCRIBING formats (JSON — our only wire/WAL format today) because the
    /// buffered value carries its own number type. It is NOT robust for a
    /// non-self-describing format (bincode/postcard/MessagePack-in-compact-mode),
    /// where flatten + `Value` can lose the integer width or fail outright. GUARD:
    /// keep `AgentEvent` JSON-only. If a binary codec is ever introduced for this
    /// vocabulary, hand-roll the envelope `Deserialize` (read the four scalar
    /// fields directly, then the kind) rather than relying on flatten + `Value`.
    #[serde(flatten)]
    pub kind: AgentEventKind,
}

impl AgentEvent {
    /// Build an envelope around a kind. The worker stamps its local ordering
    /// seq; the server's `record()` reassigns the durable seq under its lock.
    pub fn new(
        session_id: ServerSessionId,
        generation: u64,
        turn: u64,
        seq: u64,
        kind: AgentEventKind,
    ) -> Self {
        Self {
            session_id,
            generation,
            turn,
            seq,
            kind,
        }
    }
}

/// Role of a streamed [`AgentEventKind::Chunk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkRole {
    /// Normal assistant message text (today's `ReplyEvent::Chunk`).
    Message,
    /// Reasoning / thought text (un-parks the parked `AgentThoughtChunk`).
    Thought,
}

/// Kind of a transient [`AgentEventKind::Notice`] — informational ONLY. A
/// terminal failure is NOT a notice; it is [`TurnOutcome::Failed`] (spec §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// A retryable API error is being retried (overloaded / rate-limit / …).
    Retry,
    /// Generic informational status.
    Info,
}

/// How a turn ended (spec §1). Carries the verbatim ACP `PromptResponse.stopReason`
/// semantics for live turns; `ReplayEnd` subsumes the old `ReplayComplete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    MaxTokens,
    Refusal,
    /// Retry-exhausted / agent error — a boundary, not a `Notice` string.
    Failed {
        msg: String,
    },
    /// End of the replayed history prefix (spec §5) — old `ReplayComplete`.
    ReplayEnd,
}

/// The canonical agent-fact vocabulary (spec §1). Custom (de)serialize for the
/// byte-preserving [`AgentEventKind::Unknown`] catch-all (spec §8).
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEventKind {
    /// FIRST event of every (re)spawned channel (spec §4). `resumed` ==
    /// `resume_session_id.is_some()`.
    ChannelOpened {
        resumed: bool,
    },
    /// Streamed text. `role` distinguishes assistant message from thought.
    Chunk {
        text: String,
        role: ChunkRole,
    },
    ToolCallStarted(ToolCall),
    ToolCallUpdated(ToolCallUpdate),
    PlanUpdated(Plan),
    ModeChanged(SessionModeId),
    UsageUpdated(UsageSnapshot),
    /// Transient status ONLY (spec §1) — terminal failure is `TurnEnded`.
    Notice {
        kind: NoticeKind,
        msg: String,
    },
    /// Live submit + replay echo, unified; dedup by identity (spec §2/§5).
    UserMessage {
        text: String,
    },
    /// Turn boundary. Subsumes `ReplayComplete` (→ `ReplayEnd`) and terminal
    /// failure (→ `Failed`).
    TurnEnded {
        outcome: TurnOutcome,
    },
    /// Ringbuffer-trim marker (spec §6/§7) — NOT a silent drop. (Stage B emits.)
    CompactedSummary {
        through_turn: u64,
        summary: String,
    },
    /// Forward-compat catch-all (spec §8): an older decoder lands an unknown
    /// `kind` HERE preserving the WHOLE object so a forwarding node round-trips
    /// it verbatim through the durable log. NEVER constructed by current code —
    /// only by [`Deserialize`] on an unrecognised tag.
    Unknown {
        tag: String,
        raw: serde_json::Value,
    },
}

// ── Serde ──────────────────────────────────────────────────────────────────
//
// The known variants are encoded via an internal derive-backed mirror enum that
// uses the stock `#[serde(tag = "kind")]` representation. `Unknown` is handled
// by hand so its bytes round-trip under the ORIGINAL tag.

/// Internal derive mirror for the KNOWN variants only. Its wire shape IS the
/// canonical `{"kind": "...", ..payload}` representation (spec §1) — the public
/// `AgentEventKind` (de)serialize delegates to this for everything but `Unknown`.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum KnownKind {
    ChannelOpened {
        resumed: bool,
    },
    Chunk {
        text: String,
        role: ChunkRole,
    },
    ToolCallStarted(ToolCall),
    ToolCallUpdated(ToolCallUpdate),
    PlanUpdated(Plan),
    ModeChanged(SessionModeId),
    UsageUpdated(UsageSnapshot),
    // `kind` is the internal enum tag, so the Notice severity rides the wire as
    // `level` to avoid colliding with it.
    Notice {
        #[serde(rename = "level")]
        kind: NoticeKind,
        msg: String,
    },
    UserMessage {
        text: String,
    },
    TurnEnded {
        outcome: TurnOutcome,
    },
    CompactedSummary {
        through_turn: u64,
        summary: String,
    },
}

impl AgentEventKind {
    /// Map a known variant to the derive mirror, or `None` for `Unknown`.
    fn as_known(&self) -> Option<KnownKind> {
        Some(match self {
            AgentEventKind::ChannelOpened { resumed } => {
                KnownKind::ChannelOpened { resumed: *resumed }
            }
            AgentEventKind::Chunk { text, role } => KnownKind::Chunk {
                text: text.clone(),
                role: *role,
            },
            AgentEventKind::ToolCallStarted(tc) => KnownKind::ToolCallStarted(tc.clone()),
            AgentEventKind::ToolCallUpdated(u) => KnownKind::ToolCallUpdated(u.clone()),
            AgentEventKind::PlanUpdated(p) => KnownKind::PlanUpdated(p.clone()),
            AgentEventKind::ModeChanged(m) => KnownKind::ModeChanged(m.clone()),
            AgentEventKind::UsageUpdated(s) => KnownKind::UsageUpdated(s.clone()),
            AgentEventKind::Notice { kind, msg } => KnownKind::Notice {
                kind: *kind,
                msg: msg.clone(),
            },
            AgentEventKind::UserMessage { text } => KnownKind::UserMessage { text: text.clone() },
            AgentEventKind::TurnEnded { outcome } => KnownKind::TurnEnded {
                outcome: outcome.clone(),
            },
            AgentEventKind::CompactedSummary {
                through_turn,
                summary,
            } => KnownKind::CompactedSummary {
                through_turn: *through_turn,
                summary: summary.clone(),
            },
            AgentEventKind::Unknown { .. } => return None,
        })
    }

    fn from_known(k: KnownKind) -> Self {
        match k {
            KnownKind::ChannelOpened { resumed } => AgentEventKind::ChannelOpened { resumed },
            KnownKind::Chunk { text, role } => AgentEventKind::Chunk { text, role },
            KnownKind::ToolCallStarted(tc) => AgentEventKind::ToolCallStarted(tc),
            KnownKind::ToolCallUpdated(u) => AgentEventKind::ToolCallUpdated(u),
            KnownKind::PlanUpdated(p) => AgentEventKind::PlanUpdated(p),
            KnownKind::ModeChanged(m) => AgentEventKind::ModeChanged(m),
            KnownKind::UsageUpdated(s) => AgentEventKind::UsageUpdated(s),
            KnownKind::Notice { kind, msg } => AgentEventKind::Notice { kind, msg },
            KnownKind::UserMessage { text } => AgentEventKind::UserMessage { text },
            KnownKind::TurnEnded { outcome } => AgentEventKind::TurnEnded { outcome },
            KnownKind::CompactedSummary {
                through_turn,
                summary,
            } => AgentEventKind::CompactedSummary {
                through_turn,
                summary,
            },
        }
    }
}

impl Serialize for AgentEventKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            AgentEventKind::Unknown { tag, raw } => {
                // Re-emit `raw`'s fields under the ORIGINAL tag (spec §8): NOT
                // `{"kind":"unknown"}`. `raw` is the whole original object,
                // which may or may not still carry its `kind` key — we force a
                // `kind` == `tag` and splice the remaining fields, so a node
                // that can't render a newer variant still round-trips it.
                let obj = raw.as_object();
                let extra = obj.map(|o| o.len()).unwrap_or(0);
                let mut map = serializer.serialize_map(Some(extra + 1))?;
                map.serialize_entry("kind", tag)?;
                if let Some(obj) = obj {
                    for (k, v) in obj {
                        if k == "kind" {
                            continue; // already written as `tag`
                        }
                        map.serialize_entry(k, v)?;
                    }
                }
                map.end()
            }
            other => {
                // Safe: `as_known` only returns `None` for `Unknown`, handled above.
                let known = other.as_known().expect("non-Unknown variant is known");
                known.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for AgentEventKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Buffer into a Value first so an unknown tag preserves its bytes.
        let value = serde_json::Value::deserialize(deserializer)?;
        let tag = value
            .get("kind")
            .and_then(|k| k.as_str())
            .ok_or_else(|| de::Error::custom("AgentEventKind missing string `kind`"))?
            .to_string();

        // Try the known mirror. If serde rejects the tag (unknown variant) OR
        // the payload doesn't fit, fall into Unknown preserving the bytes.
        match serde_json::from_value::<KnownKind>(value.clone()) {
            Ok(known) => Ok(AgentEventKind::from_known(known)),
            Err(_) => Ok(AgentEventKind::Unknown { tag, raw: value }),
        }
    }
}

// A standalone Visitor isn't needed (we delegate through Value), but keep the
// import surface honest for future hand-rolled paths.
#[allow(dead_code)]
fn _visitor_marker<'de, V: Visitor<'de>>(_: V) {}
#[allow(dead_code)]
fn _map_marker<'de, M: MapAccess<'de>>(_: M) {}

// ── Additive producer: ReplyEvent → AgentEventKind (spec §9) ────────────────

/// Convert a legacy [`ReplyEvent`] into the canonical [`AgentEventKind`]. This is
/// the additive producer seam (spec §9): the worker emits the `AgentEvent` stream
/// ALONGSIDE the existing `ReplyEvent`, and the forwarder/reducer agreement tests
/// assert the two streams describe the same facts.
///
/// Returns `None` for `ReplyEvent` variants whose fact identity lives in the
/// envelope rather than the payload — `ReplyEvent::TurnEnded { count }` and
/// `ReplyEvent::ReplayComplete` both map to `TurnEnded`, but the authoritative
/// `turn` is the envelope's, so callers stamp the envelope and choose the
/// [`TurnOutcome`] explicitly via [`turn_ended_kind`] / [`replay_end_kind`].
pub fn agent_kind_from_reply(reply: &ReplyEvent) -> Option<AgentEventKind> {
    Some(match reply {
        ReplyEvent::Chunk(text) => AgentEventKind::Chunk {
            text: text.clone(),
            role: ChunkRole::Message,
        },
        ReplyEvent::ToolCallStarted(tc) => AgentEventKind::ToolCallStarted(tc.clone()),
        ReplyEvent::ToolCallUpdated(u) => AgentEventKind::ToolCallUpdated(u.clone()),
        ReplyEvent::PlanUpdated(p) => AgentEventKind::PlanUpdated(p.clone()),
        ReplyEvent::ModeChanged(m) => AgentEventKind::ModeChanged(m.clone()),
        ReplyEvent::UsageUpdated(s) => AgentEventKind::UsageUpdated(s.clone()),
        ReplyEvent::Notice(msg) => AgentEventKind::Notice {
            // Legacy Notice conflates retry-status with terminal failure; during
            // additive rollout we classify by content best-effort. Terminal
            // failure properly belongs to TurnOutcome::Failed (a follow-up once
            // the driver loop emits it directly).
            kind: if msg.contains("retry") || msg.contains("retrying") {
                NoticeKind::Retry
            } else {
                NoticeKind::Info
            },
            msg: msg.clone(),
        },
        ReplyEvent::UserMessage(text) => AgentEventKind::UserMessage { text: text.clone() },
        // Envelope-authoritative: caller supplies the outcome + turn.
        ReplyEvent::ReplayComplete => return None,
        ReplyEvent::TurnEnded { .. } => return None,
    })
}

/// The `TurnEnded` kind for a settled LIVE turn (spec §1/§5).
pub fn turn_ended_kind(outcome: TurnOutcome) -> AgentEventKind {
    AgentEventKind::TurnEnded { outcome }
}

/// The `TurnEnded { ReplayEnd }` kind — old `ReplayComplete` (spec §5).
pub fn replay_end_kind() -> AgentEventKind {
    AgentEventKind::TurnEnded {
        outcome: TurnOutcome::ReplayEnd,
    }
}

/// The `ChannelOpened` first-event kind (spec §4).
pub fn channel_opened_kind(resumed: bool) -> AgentEventKind {
    AgentEventKind::ChannelOpened { resumed }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: AgentEventKind) -> AgentEvent {
        AgentEvent::new("s1".into(), 3, 7, 42, kind)
    }

    #[test]
    fn envelope_round_trips() {
        let e = ev(AgentEventKind::Chunk {
            text: "hello".into(),
            role: ChunkRole::Message,
        });
        let json = serde_json::to_string(&e).unwrap();
        // Envelope fields are present and the kind is flattened in.
        assert!(json.contains("\"session_id\":\"s1\""));
        assert!(json.contains("\"generation\":3"));
        assert!(json.contains("\"turn\":7"));
        assert!(json.contains("\"seq\":42"));
        assert!(json.contains("\"kind\":\"chunk\""));
        let back: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn channel_opened_round_trips() {
        let e = ev(channel_opened_kind(true));
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"kind\":\"channel_opened\""));
        assert!(json.contains("\"resumed\":true"));
        let back: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn turn_ended_outcomes_round_trip() {
        for outcome in [
            TurnOutcome::Completed,
            TurnOutcome::Cancelled,
            TurnOutcome::MaxTokens,
            TurnOutcome::Refusal,
            TurnOutcome::Failed { msg: "boom".into() },
            TurnOutcome::ReplayEnd,
        ] {
            let e = ev(turn_ended_kind(outcome.clone()));
            let json = serde_json::to_string(&e).unwrap();
            let back: AgentEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, e, "outcome {outcome:?} must round-trip");
        }
    }

    /// The load-bearing spec §8 guarantee: an older decoder lands an unknown
    /// `kind` in `Unknown` AND re-emits it under its ORIGINAL tag, byte-faithful.
    #[test]
    fn unknown_kind_round_trips_under_original_tag() {
        // A future variant this decoder doesn't know about.
        let future = r#"{"session_id":"s1","generation":3,"turn":7,"seq":42,"kind":"speculative_decode","tokens":128,"nested":{"a":1}}"#;
        let back: AgentEvent = serde_json::from_str(future).unwrap();
        match &back.kind {
            AgentEventKind::Unknown { tag, raw } => {
                assert_eq!(tag, "speculative_decode");
                // The whole inner object is preserved.
                assert_eq!(raw.get("tokens").and_then(|v| v.as_u64()), Some(128));
                assert_eq!(
                    raw.get("nested")
                        .and_then(|v| v.get("a"))
                        .and_then(|v| v.as_u64()),
                    Some(1)
                );
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
        // Re-serialize: the kind tag is the ORIGINAL, not "unknown", and the
        // payload fields survive.
        let reser = serde_json::to_string(&back).unwrap();
        assert!(
            reser.contains("\"kind\":\"speculative_decode\""),
            "must re-emit original tag, got {reser}"
        );
        assert!(!reser.contains("\"kind\":\"unknown\""));
        assert!(reser.contains("\"tokens\":128"));

        // Round-trip-stable: decoding the re-serialized form yields the same kind.
        let again: AgentEvent = serde_json::from_str(&reser).unwrap();
        assert_eq!(again.kind, back.kind);
    }

    #[test]
    fn notice_classifies_retry_vs_info() {
        let retry =
            agent_kind_from_reply(&ReplyEvent::Notice("API error — retrying 1/5 in 1s".into()))
                .unwrap();
        assert!(matches!(
            retry,
            AgentEventKind::Notice {
                kind: NoticeKind::Retry,
                ..
            }
        ));
        let info = agent_kind_from_reply(&ReplyEvent::Notice("heads up".into())).unwrap();
        assert!(matches!(
            info,
            AgentEventKind::Notice {
                kind: NoticeKind::Info,
                ..
            }
        ));
    }

    #[test]
    fn reply_chunk_maps_to_message_chunk() {
        let k = agent_kind_from_reply(&ReplyEvent::Chunk("hi".into())).unwrap();
        match k {
            AgentEventKind::Chunk { text, role } => {
                assert_eq!(text, "hi");
                assert_eq!(role, ChunkRole::Message);
            }
            other => panic!("expected Chunk, got {other:?}"),
        }
    }

    #[test]
    fn turn_ended_and_replay_complete_are_envelope_authoritative() {
        assert!(agent_kind_from_reply(&ReplyEvent::TurnEnded { count: 3 }).is_none());
        assert!(agent_kind_from_reply(&ReplyEvent::ReplayComplete).is_none());
    }
}
