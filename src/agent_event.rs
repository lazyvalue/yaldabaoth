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
//!
//! The `Deserialize` reads the kind object as an ORDERED, duplicate-preserving
//! entry list (a map visitor) rather than via `serde_json::Value`. This is
//! load-bearing for BACKWARD compat: tool-call payloads (`ToolCall` /
//! `ToolCallUpdate`) carry their own `kind` (the ACP tool category), and a
//! legacy build serialized that FLAT next to the event tag, so old WAL records
//! hold TWO `kind` keys. `Value` collapses duplicates to the last, which would
//! destroy the event tag and drop the event as `Unknown`; the ordered read keeps
//! the first `kind` as the tag and the second as the tool kind. New records nest
//! the tool payload under `tool_call`/`tool_call_update` (no collision); the
//! reader accepts BOTH shapes, so every record ever written round-trips.

use serde::de::{MapAccess, Visitor};
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
    /// `Deserialize` (an ordered map read whose per-field values are buffered as
    /// `serde_json::Value`, see below), the envelope's `u64` fields
    /// (`generation`/`turn`/`seq`) are decoded from that intermediate, not
    /// directly from the wire. This is sound for
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
    // Nest the payload under a named key instead of letting the newtype variant
    // FLATTEN it: both `ToolCall` and `ToolCallUpdate` carry their own `kind`
    // (the ACP tool category — read/execute/think/search/…), which flattened
    // would collide with this enum's `#[serde(tag = "kind")]` and emit a
    // DUPLICATE `kind` key. serde_json keeps the LAST on read-back, so the
    // variant tag ("tool_call_started") loses to the tool kind ("read") and the
    // event is misparsed as `Unknown` and silently dropped by the reducer.
    // Nesting keeps the tool kind safely under `tool_call`/`tool_call_update`.
    // (Same collision the `Notice`/`level` rename below avoids.)
    ToolCallStarted {
        tool_call: ToolCall,
    },
    ToolCallUpdated {
        tool_call_update: ToolCallUpdate,
    },
    PlanUpdated(Plan),
    // `SessionModeId` is a newtype around a STRING, and an internally-tagged
    // (`#[serde(tag = "kind")]`) newtype variant wrapping a non-map value cannot
    // serialize — serde errors with "cannot serialize tagged newtype variant
    // ... containing a string", which made every mode change fail to record to
    // the WAL. Nest it under a named field so the value rides as `"mode":"..."`.
    // (Plan/UsageSnapshot wrap MAPS, so their newtype flatten is fine.)
    ModeChanged {
        mode: SessionModeId,
    },
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
            AgentEventKind::ToolCallStarted(tc) => KnownKind::ToolCallStarted {
                tool_call: tc.clone(),
            },
            AgentEventKind::ToolCallUpdated(u) => KnownKind::ToolCallUpdated {
                tool_call_update: u.clone(),
            },
            AgentEventKind::PlanUpdated(p) => KnownKind::PlanUpdated(p.clone()),
            AgentEventKind::ModeChanged(m) => KnownKind::ModeChanged { mode: m.clone() },
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
            KnownKind::ToolCallStarted { tool_call } => AgentEventKind::ToolCallStarted(tool_call),
            KnownKind::ToolCallUpdated { tool_call_update } => {
                AgentEventKind::ToolCallUpdated(tool_call_update)
            }
            KnownKind::PlanUpdated(p) => AgentEventKind::PlanUpdated(p),
            KnownKind::ModeChanged { mode } => AgentEventKind::ModeChanged(mode),
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
        // Read the kind object as an ORDERED entry list, preserving duplicate
        // keys — we must NOT route through `serde_json::Value` here. Legacy flat
        // tool-call records (written before tool payloads were nested) carry TWO
        // `kind` keys: the event tag, then the ACP tool's own kind. `Value`
        // collapses duplicates to the LAST, which destroys the event tag and
        // misparses the whole event as `Unknown` (dropping it). An ordered read
        // keeps both, so both the new nested shape and every legacy flat record
        // round-trip. (`flatten` on `AgentEvent` preserves the duplicate entries
        // in its buffer, so this visitor sees them even via the envelope.)
        deserializer.deserialize_map(AgentEventKindVisitor)
    }
}

struct AgentEventKindVisitor;

impl<'de> Visitor<'de> for AgentEventKindVisitor {
    type Value = AgentEventKind;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an AgentEventKind object carrying a `kind` tag")
    }

    fn visit_map<A>(self, mut access: A) -> Result<AgentEventKind, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut entries: Vec<(String, serde_json::Value)> = Vec::new();
        while let Some((k, v)) = access.next_entry::<String, serde_json::Value>()? {
            entries.push((k, v));
        }
        Ok(agent_event_kind_from_entries(entries))
    }
}

/// Build an `AgentEventKind` from the ordered (duplicate-preserving) entries of
/// the kind object. Handles every on-wire shape this vocabulary has produced:
///   - NEW nested tool calls — payload under `tool_call` / `tool_call_update`.
///   - LEGACY flat tool calls — tool fields beside the event tag, possibly with
///     a SECOND `kind` (the ACP tool category) recovered by dropping ONLY the
///     first `kind` (the event tag).
///   - all other kinds — the derive mirror (`KnownKind`), or byte-preserving
///     `Unknown` for a tag this build doesn't recognize (forward-compat, §8).
fn agent_event_kind_from_entries(entries: Vec<(String, serde_json::Value)>) -> AgentEventKind {
    // Event tag = the FIRST `kind` entry (ordered read keeps it even when a
    // legacy flat record also carries the tool's own `kind` later).
    let tag = entries
        .iter()
        .find(|(k, _)| k == "kind")
        .and_then(|(_, v)| v.as_str())
        .unwrap_or_default()
        .to_string();

    match tag.as_str() {
        "tool_call_started" => {
            if let Some(tc) = tool_payload::<ToolCall>(&entries, "tool_call") {
                return AgentEventKind::ToolCallStarted(tc);
            }
        }
        "tool_call_updated" => {
            if let Some(u) = tool_payload::<ToolCallUpdate>(&entries, "tool_call_update") {
                return AgentEventKind::ToolCallUpdated(u);
            }
        }
        _ => {}
    }

    // Non-tool kinds never carry a colliding `kind`, so a deduped map is
    // faithful; reuse it for the known-mirror parse and the `Unknown` raw.
    let value = serde_json::Value::Object(entries.into_iter().collect());
    match serde_json::from_value::<KnownKind>(value.clone()) {
        Ok(known) => AgentEventKind::from_known(known),
        Err(_) => AgentEventKind::Unknown { tag, raw: value },
    }
}

/// Reconstruct a tool-call payload of type `T` from the ordered entries.
/// New nested shape: the sub-object under `nested_key`. Legacy flat shape: every
/// field EXCEPT the first `kind` (the event tag); a later `kind` (the tool's own
/// category) survives, so default- and explicit-kind legacy records both
/// recover. Returns `None` if neither shape parses (caller falls back).
fn tool_payload<T: serde::de::DeserializeOwned>(
    entries: &[(String, serde_json::Value)],
    nested_key: &str,
) -> Option<T> {
    if let Some((_, v)) = entries.iter().find(|(k, _)| k == nested_key) {
        return serde_json::from_value::<T>(v.clone()).ok();
    }
    let mut map = serde_json::Map::new();
    let mut dropped_tag = false;
    for (k, v) in entries {
        if k == "kind" && !dropped_tag {
            dropped_tag = true; // drop ONLY the event tag; keep a later tool kind
            continue;
        }
        map.insert(k.clone(), v.clone());
    }
    serde_json::from_value::<T>(serde_json::Value::Object(map)).ok()
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

    /// Regression: a tool call carries its OWN `kind` (the ACP tool category —
    /// read/execute/think/…). Flattening the newtype variant emitted that next
    /// to the event's `#[serde(tag = "kind")]`, producing a DUPLICATE `kind`;
    /// serde_json keeps the last on read-back, so the variant tag lost to the
    /// tool kind and EVERY tool call deserialized as `Unknown` and was dropped
    /// by the reducer (resume/replay + the authoritative live AgentEvent stream).
    /// Nesting under `tool_call` fixes it: one event tag, tool kind preserved.
    #[test]
    fn tool_call_kind_does_not_collide_with_event_tag() {
        let tc: ToolCall =
            serde_json::from_str(r#"{"toolCallId":"t1","title":"Read File","kind":"read"}"#)
                .unwrap();
        let e = ev(AgentEventKind::ToolCallStarted(tc));
        let json = serde_json::to_string(&e).unwrap();

        // Exactly ONE top-level event tag, and it's the variant (not the tool
        // kind) — i.e. no duplicate `kind` key.
        assert_eq!(
            json.matches(r#""kind":"tool_call_started""#).count(),
            1,
            "event tag present exactly once: {json}"
        );

        // Round-trips back to ToolCallStarted (NOT Unknown), tool kind intact.
        let back: AgentEvent = serde_json::from_str(&json).unwrap();
        match &back.kind {
            AgentEventKind::ToolCallStarted(tc) => {
                assert_eq!(tc.kind, crate::acp_channel::ToolKind::Read);
                assert_eq!(tc.title, "Read File");
            }
            other => panic!("expected ToolCallStarted, got {other:?}"),
        }
        assert_eq!(back, e, "tool-call event must round-trip");
    }

    /// Backward-compat: a LEGACY FLAT tool-call record (the broken on-disk shape
    /// with a DUPLICATE `kind` — event tag THEN the ACP tool kind) must still be
    /// recovered, not dropped as `Unknown`. This is a verbatim record shape from
    /// a real pre-fix WAL (toolName "Glob" → kind "search"). The ordered reader
    /// keeps the first `kind` as the tag and the second as the tool's kind.
    #[test]
    fn legacy_flat_tool_call_started_with_dup_kind_recovers() {
        let wire = r#"{"session_id":"479446f2","generation":0,"turn":0,"seq":5,"kind":"tool_call_started","toolCallId":"toolu_01EfFgJpFRVft5yZGvzzX6bm","title":"Find","kind":"search","rawInput":{},"_meta":{"claudeCode":{"toolName":"Glob"}}}"#;
        let back: AgentEvent = serde_json::from_str(wire).unwrap();
        match &back.kind {
            AgentEventKind::ToolCallStarted(tc) => {
                assert_eq!(tc.kind, crate::acp_channel::ToolKind::Search);
                assert_eq!(tc.title, "Find");
            }
            other => panic!("legacy flat dup-kind must recover, got {other:?}"),
        }
        // Envelope still decodes around the recovered kind.
        assert_eq!(back.seq, 5);
        assert_eq!(back.turn, 0);
    }

    /// Backward-compat: a legacy FLAT `tool_call_updated` (no nested key, no tool
    /// kind on this one) recovers to `ToolCallUpdated`, not `Unknown`.
    #[test]
    fn legacy_flat_tool_call_updated_recovers() {
        let wire = r#"{"session_id":"s1","generation":0,"turn":0,"seq":7,"kind":"tool_call_updated","toolCallId":"toolu_x","status":"completed"}"#;
        let back: AgentEvent = serde_json::from_str(wire).unwrap();
        assert!(
            matches!(back.kind, AgentEventKind::ToolCallUpdated(_)),
            "legacy flat tool_call_updated must recover, got {:?}",
            back.kind
        );
    }

    /// New nested tool-call records (post-fix shape) round-trip through the
    /// ordered reader exactly as the legacy ones do.
    #[test]
    fn nested_tool_call_round_trips_through_ordered_reader() {
        let tc: ToolCall =
            serde_json::from_str(r#"{"toolCallId":"t9","title":"Run","kind":"execute"}"#).unwrap();
        let e = ev(AgentEventKind::ToolCallStarted(tc));
        let wire = serde_json::to_string(&e).unwrap();
        assert!(wire.contains(r#""tool_call":{"#), "writes nested: {wire}");
        let back: AgentEvent = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, e);
    }

    /// 100%-certainty check against the user's REAL WAL(s): every record tagged
    /// as a tool call must recover (never land in `Unknown`). Opt-in (reads
    /// `~/.sketch/wal`) so CI / other machines skip it; run with
    /// `SKETCH_WAL_RECOVER_CHECK=1 cargo test`.
    #[test]
    fn real_wal_tool_calls_all_recover_when_present() {
        if std::env::var("SKETCH_WAL_RECOVER_CHECK").as_deref() != Ok("1") {
            return;
        }
        let Some(home) = dirs::home_dir() else { return };
        let wal_dir = home.join(".sketch").join("wal");
        let Ok(entries) = std::fs::read_dir(&wal_dir) else {
            return;
        };
        let mut checked = 0usize;
        let mut dropped = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("log") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in content.lines() {
                // Tool-call records carry the tag textually (legacy lines have a
                // DUPLICATE `kind`, so a Value-based check would miss them).
                if !(line.contains(r#""kind":"tool_call_started""#)
                    || line.contains(r#""kind":"tool_call_updated""#))
                {
                    continue;
                }
                // The WAL line wraps the event: {"t":..,"type":..,"event":<OBJ>}.
                // Parse <OBJ> from the RAW bytes (NOT via serde_json::Value,
                // which would collapse the duplicate `kind` and defeat the test).
                // "event" is the last key, so <OBJ> runs to the final `}`.
                let marker = "\"event\":";
                let Some(pos) = line.find(marker) else {
                    continue;
                };
                let obj = line[pos + marker.len()..].trim_end();
                let obj = obj.strip_suffix('}').unwrap_or(obj); // drop wrapper close
                checked += 1;
                match serde_json::from_str::<AgentEvent>(obj) {
                    Ok(ev) if matches!(ev.kind, AgentEventKind::Unknown { .. }) => dropped += 1,
                    Ok(_) => {}
                    Err(_) => dropped += 1,
                }
            }
        }
        assert_eq!(
            dropped, 0,
            "all {checked} real tool-call WAL records must recover; {dropped} still dropped"
        );
        eprintln!("[wal-recover-check] {checked} tool-call records, all recovered");
    }

    /// Regression: `ModeChanged` wraps a `SessionModeId` (a newtype around a
    /// STRING). As a `#[serde(tag="kind")]` newtype variant it could NOT
    /// serialize ("cannot serialize tagged newtype variant ... containing a
    /// string"), so every agent mode change failed to record to the WAL. Nesting
    /// under `mode` fixes serialize + round-trip.
    #[test]
    fn mode_changed_serializes_and_round_trips() {
        let mode: crate::acp_channel::SessionModeId = serde_json::from_str("\"plan\"").unwrap();
        let e = ev(AgentEventKind::ModeChanged(mode));
        // Previously this `unwrap` panicked (serialize error).
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""kind":"mode_changed""#), "{json}");
        assert!(json.contains(r#""mode":"plan""#), "{json}");
        let back: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
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
