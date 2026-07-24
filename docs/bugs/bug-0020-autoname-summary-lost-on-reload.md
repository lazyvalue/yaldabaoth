# bug-0020: autoname-summary-lost-on-reload

**Status:** FIXED
**First seen:** 2026-07-24
**Component:** `docs/components/agent-tile/naming.md` (`UXI-AgentTile-27`),
`docs/components/jump-panel.md`

## Symptom

The little explainer text under an agent session in the jump panel — the
Haiku-generated two-sentence summary (`UXI-AgentTile-27`) — is there while the app
runs, and **gone after the GUI is reloaded**. The session's NAME survives; only the
summary line disappears.

## Context / root cause

The summary had exactly one durable home: the `summary` key of a slot in
`~/.yalda/acp_sessions.json`. That file is the wrong home for it:

- It is keyed by **cwd**, and
- `save_agent_ring` only writes sessions that are **bound to a tile** at save time
  ("free sessions (no tile) are not persisted — they only live for the running
  process").

The jump panel, however, lists **every** session on the server (the universal
roster), bound or not. So a summary only survived a restart for a session that
happened to be open in a tile in the restored layout; for every other session the
row came back with `summary: None` and the line vanished. `jump_panel_agent_rows`
compounded it — it read the summary **only** off a live in-store session entity
(`opened.and_then(|e| e.read(cx).state.summary)`), so a roster-only row could never
show one.

Note the naming call is **one-shot per session and never re-run**, so a lost summary
is lost forever — there is no path that regenerates it.

Verified BEFORE fixing that the save→load round trip for a *bound* session already
worked (`autoname_summary_survives_a_restart`, added here, passes on the pre-fix
code): the write path was not the hole; the storage *model* was.

## Planned solution

Give the summary a home that matches its lifetime — **id-keyed and independent of
tile binding**, the same durability the LABEL already gets from the session server's
WAL (the server has no summary concept, so a local sidecar it is):

- `~/.yalda/session_summaries.json` — a flat `{server session id → summary}` map
  (`persist.rs`: `session_summaries_path` + `load_session_summaries` /
  `save_session_summary` / `forget_session_summaries`, with the same `cfg(test)`
  fail-safe + `with_session_summaries_path` override as every other persisted path).
- Loaded once into `YaldaGpuiView.session_summaries` at construction.
- Written at `finish_autoname` (where the summary is installed).
- Read as a **fallback** wherever a summary is shown: both jump-panel row builders
  and the restore path (`restore_agent_leaves`), with the live session state still
  authoritative when the session is open here.
- Dead sids scrubbed alongside `forget_persisted_acp_session_ids`.

The `acp_sessions.json` `summary` key stays (harmless, and it keeps working for
bound sessions) — the sidecar is what makes the guarantee unconditional.

## Approaches already tried (do NOT repeat)

- **Persisting the summary only in `acp_sessions.json`** (the original
  `UXI-AgentTile-27` implementation). Round-trips correctly for a tile-bound
  session, but structurally cannot cover a free session — which is most of the
  jump panel. Don't "fix" this by writing free sessions into the cwd-keyed slot
  file; that file means "what to restore into tiles", and overloading it is how the
  restore path got its duplicate-bind bugs.

---

## Log

### 2026-07-24 — id-keyed summary sidecar

**Changed**

- `persist.rs`: new `session_summaries_path()` (+`SUMMARIES_PATH_OVERRIDE` /
  `with_session_summaries_path` test seam, `None` under `cfg(test)` so tests never
  touch `~/.yalda` — the bug-0016 rule), `load_session_summaries`,
  `save_session_summary`, `forget_session_summaries`.
- `main.rs`: `YaldaGpuiView.session_summaries: HashMap<String, String>`, loaded in
  both constructors; `restore_agent_leaves` falls back to the sidecar when the
  persisted slot has no summary.
- `agent_ui.rs`: `finish_autoname` records the summary into the map + the sidecar;
  the dead-sid scrub drops them from both.
- `jump_panel_view.rs`: both row builders fall back to the sidecar when the session
  isn't open here.

**Verified**

- `autoname_summary_survives_a_restart` — the settle path's write round-trips
  through `acp_sessions.json` (passes pre-fix too; documents that half).
- `autoname_summary_survives_a_gui_reload` — the real guard: view 1 settles the
  autoname through `finish_autoname`, a SECOND view boots (the reload) knowing the
  session only from the roster, and `jump_panel_agent_rows` still carries the
  summary. **Negative control observed RED**: with `save_session_summary` removed
  from `finish_autoname`, the reloaded row's summary is `None` (`left: None`).
- Full `cargo test --bin yalda-gpui`: 471 passed, 0 failed.

**Outcome** — fixed. Runtime-unverified in the live GUI (the summary text as
*pixels* is harness gap #1); the data path is guarded end to end.
