# bug-0021: autoname-never-fires-for-most-sessions

**Status:** FIXED
**First seen:** 2026-07-25
**Component:** `docs/components/agent-tile/naming.md` (`UXI-AgentTile-27`)

## Symptom

"I'm not sure the Haiku namer / summarizer is firing." Sessions stay `claude-N`
with no summary line, apparently forever.

## Context / root cause

The autoname arm was keyed on **provenance** — `armed_for_autoname()` had to be
called by the constructor that built the `AgentState`. Three of the four ways a
nameless session actually comes into being never touch such a constructor:

1. **Created free from the jump panel / project menu** (`UXI-JumpPanel-3/-4` →
   `spawn_free_agent_session_at`). This creates the session **server-side only** —
   there is no local `AgentSession` to arm. When you later jump to it, it arrives
   through the **attach** path, which is deliberately never armed.
2. **`/clear`** (`clear_agent_session`): mints a brand-new server session that
   inherits the old label; the fresh state was never armed.
3. **Restored from a previous launch**: property 1 said "a restored session already
   has history and is never retro-named", so a session that was never named on
   launch N could never be named on launch N+1 either.

Only a fresh create on an already-open agent tile was armed. Everything else was
永-`claude-N`.

Two smaller contributors, both found while fixing the above:

- `drain_autoname_requests` settled the one-shot **terminally** when the transcript
  was empty at drain time ("nothing to name from"). For an attach/restore the
  transcript is empty *until replay lands*, so an armed session could burn its
  one-shot on nothing.
- `/clear` did not carry `name_origin` across the reset, so a user-typed name came
  back marked `Auto` (a latent "autoname eats a real name" hole).

The API key and the HTTP layer were fine — verified with a live `claude-haiku-4-5`
probe against `/v1/messages` (HTTP 200) using the repo `.env` key.

## Planned solution

Key the arm on **identity + evidence**, not provenance (`maybe_arm_autoname`),
called at every point a session becomes bound to a sid (create + attach
resolutions, restore):

1. label is still an auto-generated placeholder (`is_auto_claude_label`);
2. `name_origin != User`;
3. the one-shot has not already been **spent for this sid** — recorded durably in
   the bug-0020 summary sidecar (an empty-string entry = "tried, nothing usable"),
   so a relaunch never re-asks;
4. no call already in flight.

Plus: `finish_replay` marks an armed session **due** once it actually has content
(that is the moment an attached/restored session becomes nameable), the empty-
transcript drain branch re-arms instead of settling, and `/clear` carries
`name_origin`.

## Approaches already tried (do NOT repeat)

- **Arming at the constructor** (`armed_for_autoname` at "genuinely-fresh creation
  points"). This is the original design and the bug: the set of creation points is
  not the set of nameless sessions. Don't fix a missed case by adding a 7th call
  site — the arm belongs at the bind, keyed on the session's own state.

---

## Log

### 2026-07-25 — identity-keyed arm

**Changed** — `agent_ui.rs`: `maybe_arm_autoname` (new), called from
`apply_open_agent_resolution` (covers create + attach); drain no longer settles on
an empty transcript; `finish_autoname` records the spend durably; `/clear` carries
`name_origin`. `agent.rs`: `finish_replay` marks an armed session due when the
replayed transcript is non-empty. `main.rs`: restore arms after its bind.
`persist.rs`: `mark_autoname_attempted` / `autoname_already_attempted` (empty-string
entry in `session_summaries.json`).

**Verified**
- `attached_unnamed_session_is_armed_and_named` — the REAL `Attached` resolution
  arms a `claude-7` session; the REAL `finish_replay` + drain move it to
  `Requested`. **NC observed RED** (drop the arm call → `Some(Done)`).
- `a_spent_autoname_is_never_re_armed` — a settle in view 1 makes view 2 (relaunch)
  leave the same sid `Done`. **NC observed RED** (drop the attempted check →
  `Some(Pending)`).
- Full workspace `cargo test` green (473 → 474 in the gpui bin).

**Outcome** — fixed. The live Haiku call itself is verification gap 2 (no network
in tests); the key + endpoint were probed manually.
