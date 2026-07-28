# Agent Tile — Naming

Facet of the [Agent Tile](README.md) component. Owns `UXI-AgentTile-27`.

## Description

Every agent session is born with a placeholder label (`claude-N`, allocated by
`next_agent_label`). That label carries no information: a jump panel holding six
sessions reads `claude-3 · claude-4 · claude-7`, and the only way to get a
meaningful name has been to run the rename command by hand on each one.

**Autonaming** closes that gap. When a session's **first** agent turn completes,
the opening exchange (the user's first message + the agent's first reply) is sent
to a cheap model — Haiku, over a single plain-HTTP `/v1/messages` call — which
returns a two-to-three-word label and a compact topic summary of what the session
is about. The label replaces `claude-N` everywhere the session is listed; the
summary renders under it in tiny italics in the jump panel.

It fires **once per session, ever** — never re-derived as the conversation
drifts, and never retroactively applied to a session restored from a previous
launch. An **explicit rename always wins**: renaming a session latches its name
origin to `User`, after which autonaming can never fire and a late-arriving
autoname result is dropped on the floor.

The naming call is deliberately **not** the recap facet's throwaway ACP
subprocess ([`UXI-AgentTile-15`](recap.md)). A recap is a multi-paragraph
summary that earns a full agent subprocess; two words and a compact topic line
do not.
The direct Haiku call is ~1s instead of several, and the request is
`ANTHROPIC_API_KEY`-authenticated from the environment (or a gitignored `.env`
loaded at startup) rather than riding the Claude Code login.

## References

- [`recap.md`](recap.md) — `UXI-AgentTile-15`, the prior art for running an LLM
  over a session without touching the visible transcript reducer.
- [`session-binding.md`](session-binding.md) — session identity and the rename
  path that autonaming must never fight.
- [`../jump-panel.md`](../jump-panel.md) — the surface the summary renders on.
- `docs/specs/spec-agent-session-ownership.md` — `AgentSession` is the owner of
  `label`; the store enforces 1:1 binding.

## UX invariants

### UXI-AgentTile-27 — A session names and summarizes itself once, and an explicit rename wins forever

**Statement.** When a session's **first** agent turn completes, yalda derives a
short name and a short summary for it from the opening exchange and installs them
without the user asking. Five hard properties:

1. **One shot, on the first turn.** The derivation is triggered exactly once per
   session, at the completion of turn 1 (`finalize_agent_turn_idem` returning
   `true` for the session's first turn). It is never re-run as the conversation
   continues, and a session restored from a previous launch — which has already
   had turns — is never retro-named.
2. **Shape.** The name is 2–3 lowercase space-separated words, hard-capped at 28
  characters (`payments refactor`, `flaky test hunt`); the summary describes only
  the session's enduring topic or goal, never progress, actions, outcomes, or
  blockers. It prefers one sentence, permits two when needed, and is capped at
  140 characters. Both are sanitized client-side, so a
   model that ignores the instruction and returns a preamble, quotes, a code
   fence, or a paragraph still yields a well-formed label or nothing at all.
   Names are **not** deduplicated — two sessions may legitimately be about the
   same thing, and sessions are keyed by `SessionId`, never by label.
3. **An explicit rename wins, permanently.** `AgentSession` carries a typed
   `NameOrigin` (`Auto` | `User`). The rename command latches it to `User`;
   autonaming only ever fires while it is `Auto`, and an in-flight autoname
   result that lands after a rename is **dropped**, not applied. This replaces
   the string-sniffing `is_auto_claude_label` heuristic, which cannot tell an
   autoname (`payments refactor`) from a name the user typed.
4. **Fast, visible, reliable fallback.** The first accepted user turn installs a
   deterministic compact topic excerpt immediately; the model result may refine
   it after the first reply. If no useful excerpt exists while the request is in
   flight, the jump row says `summarizing topic…`. The direct request is bounded
   to eight seconds. No API key, timeout/network/non-2xx failure, refusal, or an
   omitted summary settles with the opening-topic fallback and persists it
   through the normal summary sidecar. The session may retain its placeholder
   name when naming fails, but its summary never remains permanently blank or
   stuck in `Requested`. No error banner interrupts the user.
5. **Placement.** The name is the session's `label` and therefore appears
   everywhere a session is listed (jump panel, tile selector, tab strip). The
   summary renders **only** in the jump panel, on its own line under the label,
   in a smaller italic cool-prose style (`AgentTheme::agent_tint`, never the warm
   gold accent or low-contrast structural dim) — chrome-class, so document zoom
   does not scale it.
6. **Arming is by IDENTITY, not provenance (bug-0021 — amends property 1).** A
   session is armed for its one-shot at the moment it becomes **bound to a sid**
   (create resolution, attach resolution, restore) when all of: its label is still
   an auto-generated `claude-N`, `name_origin` is not the `User` latch, no call is
   in flight, and **the one-shot has not already been spent for that sid**
   (recorded durably — see 7). The original "armed only at genuinely-fresh
   creation points" rule excluded three whole classes of nameless session
   (jump-panel-created free sessions, `/clear`, and anything restored), which
   could therefore never be named at all. The "one shot, ever" guarantee is
   unchanged — it is now keyed on the session's identity instead of on which
   constructor ran, which is strictly stronger (it survives restarts).

   A session that is armed while still empty becomes **due** at its replay
   boundary (`finish_replay`, when the transcript is non-empty) — an attached or
   restored session gets named as soon as its content arrives, without waiting for
   another turn. An empty transcript at drain time re-arms rather than settling:
   nothing to name from yet is not the same as unnameable.
7. **Durability (bug-0020, bug-0021).** Because the call is one-shot and never re-run, the
   summary must survive a GUI restart **unconditionally** — including for a
   session no tile is bound to. Its durable home is the **id-keyed sidecar**
   `~/.yalda/session_summaries.json` (`{server session id → summary}`), written at
   `finish_autoname` and read at construction into
   `YaldaGpuiView.session_summaries`; the live `AgentState::summary` wins whenever
   the session is open here, and the sidecar is the fallback everywhere a summary
   is shown or restored. (The cwd-keyed `acp_sessions.json` `summary` key still
   round-trips for tile-bound sessions, but it CANNOT carry a free session — which
   is what made the line vanish on reload.) The NAME needs none of this: it is
   pushed to the session server, whose WAL is its durable home.

   The same entry doubles as the record that the one-shot is **spent** for that
   sid: an **empty-string** value means "the call ran, nothing usable came back".
   That is what keeps property 6's identity-keyed arm from re-asking Haiku about
   the same session on every launch.

**Applies to.** `agent.rs` — `NameOrigin`, `AgentSession::{name_origin, summary,
autoname_state}`; `agent_naming.rs` — the pure `build_naming_prompt` /
`parse_naming_reply` / `sanitize_name` / `sanitize_summary` /
`fallback_topic_summary` and the blocking
`request_session_name` HTTP call (`claude-haiku-4-5`, `POST
https://api.anthropic.com/v1/messages`, `anthropic-version: 2023-06-01`);
`agent_ui.rs` — `maybe_autoname_session` (fired from the turn-finalize
chokepoint) / `spawn_autoname_worker` / `apply_autoname_result`;
`main.rs::commit_rename_overlay` (`RenameTarget::AgentSession`) — the latch;
`persist.rs` — `dotenv` load at startup, the `acp_sessions.json` `summary` key, and
the id-keyed summary sidecar (`session_summaries_path` / `load_session_summaries` /
`save_session_summary` / `forget_session_summaries`, bug-0020);
`jump_panel_view.rs` — the italic summary line (+ its sidecar fallback).

**Why.** A list of `claude-N` placeholders is unnavigable, and hand-renaming
every session is friction the user should not have to pay. Property 3 is the
load-bearing one: the moment autonames exist, "is this name auto-generated?"
stops being answerable by looking at the string, so it must become a typed field
or the feature will silently eat names the user typed (the bug-0016 class).
Property 4 keeps a cosmetic nicety from ever becoming a failure the user has to
deal with, and property 1 keeps the cost bounded at one cheap call per session.

**Status.** `implemented` (headless for the arming, the latch, and the apply
path; the live Haiku HTTP call is the sole `NEEDS-RUNTIME` gap — dev-system
§ Verification harness gap 2).

**Enforcement.** `verify_harness.rs`:
`autoname_fires_once_on_first_turn_completion` (drives the REAL turn-finalize
path — `apply_server_batch` → `ServerNotification::TurnEnded` →
`finalize_agent_turn_idem` → `drain_autoname_requests` — and asserts the request
is armed on turn 1 and NOT re-armed on turn 2),
`autoname_result_renames_the_session`,
`attached_unnamed_session_is_armed_and_named` + `a_spent_autoname_is_never_re_armed`
(property 6 / bug-0021 — the REAL `Attached` resolution arms a still-`claude-N`
session and replay makes it due; a spent one-shot is never re-armed after a
relaunch; both NCs observed RED),
`autoname_summary_survives_a_restart` + `autoname_summary_survives_a_gui_reload`
(property 6 / bug-0020 — the settle path's write round-trips, and a SECOND view
that knows the session only from the roster still shows its summary; NC observed
RED: drop `save_session_summary` → the reloaded row's summary is `None`),
`rename_latches_origin_and_blocks_autoname` (drives the REAL
`open_rename_overlay` → `commit_rename_overlay` entry point), and
`late_autoname_result_never_clobbers_a_user_rename`. `tests.rs`:
`sanitize_name_enforces_shape_and_cap`,
`sanitize_summary_keeps_two_sentences_and_flattens`,
`naming_summary_is_topic_only_short_and_has_a_user_turn_fallback`,
`autoname_topic_appears_on_first_user_turn`,
`autoname_without_api_key_settles_with_persisted_topic`,
`parse_naming_reply_tolerates_real_model_output`,
`parse_dotenv_reads_keys_and_ignores_noise`. Four negative controls observed RED
(finalize arm removed → `Pending` not `Requested`; rename latch removed → origin
`Auto`, and the late result overwrites the typed name; name-install removed →
label stays `claude-3`). The worker is suppressed under `cfg(test)` exactly as
`spawn_recap_worker` is, and tests feed `finish_autoname` directly.

## Deviations from plan

Three, all discovered while wiring it up:

1. **`name_origin` / `summary` / `autoname` live on `AgentState`, not
   `AgentSession`.** The spec named `AgentSession` because that is where `label`
   lives. In practice `AgentSession` is built by ~30 struct literals (mostly
   test fixtures) while `AgentState` has exactly two constructors, and
   `AgentSession` derefs to `AgentState` — so the fields read identically
   (`session.name_origin`) at a fraction of the churn. The turn-finalize
   chokepoint is also on `AgentState`, so the arming flag had to live there
   regardless.
2. **Arming is opt-in, not derived from `resume_id`.** The plan implied a
   session's freshness could be read off its construction. It cannot: the picker's
   "attach to an existing session" path builds a session with `resume_id: None`
   just like a fresh create. So `AutonameState` defaults to `Done` (never name)
   and the six genuinely-fresh creation points call `armed_for_autoname()`
   explicitly. A missed autoname is invisible; a wrong one overwrites a name.
3. **The latch is checked twice, not once.** The spec described dropping a late
   result (`finish_autoname`). Implementation also needed the early half in
   `drain_autoname_requests` — a rename that lands *before* the first turn ends
   leaves `autoname` at `Pending`, so without that check the session would still
   fire a pointless call and burn a request on a name it can never install.

Not built: `name_origin` is **not** persisted across restarts. It doesn't need to
be — a restored session is never armed, so nothing can clobber its name — but it
means `recover_labels_from_roster` still uses the old `is_auto_claude_label`
string sniff. That is safe today only because an installed autoname is pushed to
the server, so the roster and the local label agree.
