# Backlog

Open, deferred, and flagged work. Higher fidelity than git: each item says what,
why it's here, and its status. Updated at session end (`/worklog`) and as items
move. Past work lives in `docs/worklog/`; the *why* of choices lives in
`docs/decisions/`.

Status legend: `IN-FLIGHT` (agent/branch active) · `READY` (scoped, not started)
· `DEFERRED` (deliberately not now, reason given) · `NEEDS-DECISION` (waiting on
the user) · `NEEDS-RUNTIME` (built + state-tested headlessly; awaiting human
confirmation of *pixels / timing / OS-behavior* specifically — not "no test was
possible." State-level behavior is testable headlessly via `verify_harness.rs`).

---

- **Close a workspace without closing its sessions or quitting the app** —
  `DONE` (2026-08-11 via `/new-ux`; `UXI-Workspace-13`; Cog graph `ubr`).
  Captured verbatim: *"Add a workspace command to close the workspace. Shouldn't
  close any actual sessions, just the tiles they're bound to"* and *"Closing a
  workspace should never quit the app"*. The `.` workspace menu uses uppercase
  `X` for **close workspace**, preserving lowercase `x` as **close tile**. Closing
  a non-last workspace drops all of its tiles, so any sessions they referenced
  remain alive in the session store and become free. Closing the sole workspace
  is a no-op; it never exits Yalda. Both command entry points share a
  no-`Context` helper, so workspace closure cannot request application quit.
  `workspace_menu_uppercase_x_selects_close_workspace` pins the literal menu
  keys; `closing_workspace_frees_sessions_and_never_quits` drives the real menu
  dispatch and `Cmd-Shift-W`, proving the workspace/tile disappears while its
  server session remains store-owned and free. Removing the sole-workspace floor
  produced the expected RED on the real render path; all three mutations of the
  close helper were caught. Full GPUI suite: 549 passed, 1 ignored; non-test GUI
  check green. No runtime gap.

- **Worksheet reply: select-to-quote, older-turn replies, replied-to marker** —
  `NEEDS-RUNTIME` for the shipped parts (branch `worksheet-reply-select`, via
  `/new-ux`; project `docs/projects/worksheet-reply-select/`). Captured verbatim:
  (1) *"When moving my cursor around in agent worksheet mode I want to be able to
  select agent text: either a whole line or part of a line, with V and v. If I have
  text selected that is what is entered into the reply text when I do 'r'."*
  (2) *"I want to be able to reply to agent text earlier than the most recent agent
  turn. The reply text is sent in my next (current) turn."*
  (3) *"I want the text that is being replied to to be more obvious in the
  transcript. Add > when not editing."*
  **DONE (ticket 001):** `V` = whole-line visual (+ extend-mode so `V`/`j`/`k`
  grow), `v` = char-wise visual, both through the real keymap; an active selection
  is the `r` quote (sentence-count ignored; multi-line ⇒ `>`-per-line); `r` replies
  across the turn boundary and lands in the current turn at the tail
  (**UXI-AgentTile-34/-35/-36**). 6 new guards + unit test; 3 observed-RED negative
  controls; suite green. Runtime gap: the selection highlight **color/contrast**
  ("beautiful, clearly visible") is gap #1 — tune `AgentTheme::selection_bg` by eye
  once a live `V` selection is triggerable.
  **DONE (ticket 002):** the `>` replied-to source marker (**UXI-AgentTile-37**) —
  a pending reply's quoted source lines show a `>` gutter glyph + blockquote-colored
  left bar in the transcript when NOT typing in the block; clears on submit/abandon.
  Threaded through `TranscriptSeqs`/`TranscriptPrep`; paint-tap guard +2 observed-RED
  NCs. Suite **534** green. Gaps: exact glyph/bar **hue** (gap #1); text italic on
  the source line deferred.

- **Session tags (jump-panel folders)** — `NEEDS-RUNTIME` (built + headless-guarded;
  branch `session-tags`, via `/new-ux`). Spec'd as **UXI-AgentTile-33** (in-tile
  `tag`/`untag` prompt, `<space> t/T`, mirrors the confirm-kill pattern),
  **UXI-JumpPanel-20** (per-project tag folders in each tab: category-filtered,
  multi-appearance, untagged flat below, All drops activity headings + sorts by
  label) and **UXI-JumpPanel-21** (per-project drag order + fold, persisted). Tags
  live in the id-keyed `session_tags.json` sidecar; order/fold in
  `Preferences::{jump_tag_order, jump_folded_tags}`. 6 new guards + 4 observed-RED
  negative controls; suite 523 (531 w/ test-support) green. Runtime gaps: the folder
  glyph/indent **pixels** (gap #1) and the mouse-drag folder-reorder **gesture**
  (gap #2).

- **Direct session visits are detached from workspace placement** — `DONE`
  (2026-08-02 via `/new-ux`; `UXI-JumpPanel-19`). Captured verbatim: *"Visiting
  a session directly (via the jump panel or the p-menu) should be different from
  visiting it in a workspace. Going directly to it should show it detatched from
  a workspace. However, it hsould still present in workspace when visited."*
  Both entry surfaces now open a bare ephemeral workspace whose ordinary
  `AgentTile::Bound` tile references the project session. The original tile
  remains the unique durable placement, switch-away removes only the direct
  reference, and placement/free/persistence projections exclude ephemeral
  workspaces. Detachment is derived from the viewport's containing workspace;
  it is not duplicated as tile state.
  Close and `/clear` from the bare view preserve valid workspace placement.
  `direct_session_visits_add_a_reference_and_keep_workspace_placement` drives the
  jump-panel dispatcher plus real `Cmd-P` activation; restoring the former
  focus-owner branch produced the expected RED. GPUI harness (518 passed, 1
  ignored) and the non-test binary check are green.

- **Archiving a session tore down the whole GUI connection** — `NEEDS-RUNTIME`
  (2026-08-04 via `/bug`; `bug-0028`). Reported as *"when I unarchive a session
  it has trouble starting up again."* Root cause is at archive time, not
  unarchive: `do_set_archived` released the session's forwarder with
  `ForwarderHandle::evicted`, but that is the **high-water kill flag** whose
  handler shuts down the **per-connection** write half — so archiving one
  session dropped every attached session and forced a from-base reconnect, and
  the respawned session only looked broken. Likely the source of the
  long-standing reconnect storm too. Fixed with a distinct `released` flag plus
  a pure `forwarder_stop_action` mapping; guard
  `archiving_one_session_stops_its_forwarder_without_killing_the_connection`
  observed RED as `ShutdownConnection`. **Runtime-pending: server-side, so the
  running daemon must be restarted.** The socket teardown itself is reasoned
  from code, not observed live — a real two-session client repro is the
  follow-up if the symptom survives a restart.

- **Sessions stuck showing "Unavailable"** — `NEEDS-RUNTIME` (2026-07-31 via
  `/bug`; `bug-0027`). Reported as *"frequently sessions are listed
  (temporarily) as 'unavailable'"*, then sharpened to *"this session is listed
  as 'unavailable' even though you are very clearly available"*. Root cause: the
  exact residue of bug-0022 — that fix made `busy` roster-wide via a broadcast
  but left `connected` with **no live source at all**, so the flag froze at
  whatever the last full `list_sessions` seed said. Fixed server-side with
  `Notification::SessionConnected` + `broadcast_connected` (PublishChannel /
  AgentDisconnected / SpawnFailed), `AgentRoster::set_connected`, and a reducer
  arm; guard `agent_coming_online_clears_the_unavailable_row` observed RED
  without it. **Runtime-pending for one specific reason (not a test gap): the
  session server outlives the GUI, so the running daemon is still the old binary
  that never broadcasts. Restart `yalda-session-server` to pick this up** —
  same caveat bug-0022 carried.

- **Archive action announces itself in the transcript and system console** —
  `DONE` (2026-07-31 via `/new-ux`; `UXI-JumpPanel-18`). Captured verbatim:
  *"The archive agent action should write a system message to the agent
  transcript. It should also write a log message to the system console with the
  name of the agent."* Shipped at the single durable-flag mutator
  `jump_panel_view.rs::set_session_archived`, so the `<space>` command and the
  row context menu announce identically. Decisions taken (user delegated):
  unarchive announces symmetrically; a roster-only session gets the console line
  only, since it has no in-memory transcript; a no-op toggle stays silent.
  Console line is `archived/unarchived agent session "<label>"` at
  `ConsoleLevel::Info` via `append_system_console` (the live view path, not the
  pre-GPUI `record_system_message`); transcript notice is `session
  archived`/`session unarchived` on the `TurnId::System` lane. Guard
  `archive_toggle_announces_in_console_and_transcript` covers all four cases;
  removing the announcement call produced the expected RED. GPUI harness (513),
  library suite (161), and all-targets check are green.

- **Hide archived sessions from the Agent Tile picker** — `DONE`
  (2026-07-29 via `/new-ux`; `UXI-AgentTile-32`). Captured verbatim:
  *"The agent tile picker should not show archived sessions."* Scoped to the
  unbound Agent Tile's existing-session projection: archived sessions are absent
  whether free or bound/in use. The New Claude and New Codex rows, project
  scoping, 1:1 binding, and the Jump Panel's Archived tab remain unchanged.
  Shipped at the shared `picker_projection` seam, so render, navigation, row
  count, and activation consume the same filtered list. Guard
  `agent_tile_picker_excludes_free_and_bound_archived_sessions` proves both free
  and bound archived sessions are excluded while equivalent unarchived sessions
  remain; the unchanged projection produced the expected RED. The full GPUI
  harness, library suite, and all-bin check are green.

- **Remove redundant status words from Jump Panel session rows** — `DONE`
  (2026-07-28 via `/new-ux`; revises `UXI-JumpPanel-10`). Captured verbatim:
  *"with the different tabs and headers we no longer need the words 'your turn'
  or 'working' in the jump pane."* This removes only the right-edge per-session
  status words; the Waiting / Working tab labels and All-tab section headers
  remain. Glyph shape, semantic color, row wash, summaries, ordering, counts,
  and selection behavior stay unchanged. A real paint guard proves both live
  rows paint while neither paints a status-word element; the unchanged renderer
  produced the expected RED on the Working row. The full suite and all-bin build
  are green.

- **Lay out Jump Panel agent tabs in two rows** — `DONE` (2026-07-28 via
  `/new-ux`; extends `UXI-JumpPanel-15`). Captured verbatim: *"Tabs look
  crowded. Let's put Waiting and Working on one line, All and Archived on
  another line."* Every expanded project renders one bounded 2×2 tab control:
  Waiting / Working on the first row and All / Archived on the second. Tab
  behavior, counts, per-project selection, colors, and spacing from workspaces
  remain unchanged. Shipped with vertical separators inside each row and a
  horizontal hairline between them. Painted geometry proves the requested row
  pairing, aligned equal columns, and that all four targets remain inside the
  control. The unchanged one-row renderer produced the expected RED; full suite
  and all-bin build are green.

- **Keep Waiting order stable when viewing an agent** — `DONE` (2026-07-28 via
  `/new-ux`; extends `UXI-JumpPanel-14`). Captured verbatim: *"The ordering of
  the waiting tab shouldn't change ON VIEW. The ordering should be based on when
  the agent ENTERED the waiting state. Selecting an agent current moves it to
  the bottom of the list, which is NOT correct behavior."* Selecting or viewing
  a session is not an operational state transition and must preserve its
  Waiting entry timestamp and list position. Only a real
  Waiting→Working→Waiting cycle can move it to the newest (bottom) position.
  Shipped by keeping the roster's identity-stable state-entry time when local
  and server activity agree; if local activity is ahead of the server
  broadcast, its real transition time leads temporarily. Guard
  `viewing_a_waiting_agent_does_not_change_waiting_order` drives the real
  roster-row `jump_to_agent` attach path, observed the reported `z-old,a-new` →
  `a-new,z-old` RED, proves selection preserves order after the fix, and proves
  a real Working→Waiting cycle does move the row to the bottom. Full suite
  green.

- **Show live totals on Waiting and Working tabs** — `NEEDS-RUNTIME` (built on branch
  `jump-tab-counts`, 2026-07-28 via `/new-ux`; `UXI-JumpPanel-17`). Captured
  verbatim: *"I want the Waiting and Working tabs to have a number indicator
  showing how many sessions are in each."* Scoped per expanded project: both
  indicators are always visible, including `0`, and count non-archived sessions
  admitted by the corresponding live-activity tab. All and Archived remain
  unnumbered. The indicator uses the tab's green/orange activity color while
  selected-tab chrome remains neutral gray. Derived totals, activity changes,
  archive/unavailable exclusion, zero-state paint, and indicator containment
  are headless-verified; final visual balance in Folio and Nightfox remains
  runtime gap #1.

- **Group the All tab by live activity** — `NEEDS-RUNTIME` (built on branch
  `jump-all-state-groups`, 2026-07-28 via `/new-ux`; extends
  `UXI-JumpPanel-14`). Captured verbatim: *"In the 'all' tab sessions should be
  organized by 'working' and 'waiting'. Working is first. Each has an
  appropriate header."* Resolved with the user: All renders Working first,
  Waiting second, then a subdued Unavailable group only when disconnected or
  connecting rows exist. The durable custom order remains stable within each
  group. Real rendered ordering, headings, row order, and empty-section removal
  are headless-verified; final visual density/contrast remains runtime gap #1.

- **Archive agent sessions from the jump panel** — `NEEDS-RUNTIME` (built on branch
  `jump-session-archive`, 2026-07-28 via `/new-ux`; `UXI-JumpPanel-16`).
  Captured verbatim: *"I also want to add an 'archived' state for sessions.
  These sessions don't show up in all, and they don't show up in the p-menu.
  There should be a tab for archived. I should be able to flag a session as
  archived from the <space> command menu. If I am on an archived session, I
  should be able to flag it as unarchived. Right clicking on a session should in
  the jump panel should present a context menu. This should allow me to
  archive/unarchive."* Implemented as a durable server-owned cold lifecycle
  orthogonal to Waiting/Working: archived sessions appear only in Archived,
  retain their custom All slot, close their ACP transport and WAL descriptor,
  and return to the correct live tab after resume. Legacy preference flags are
  migrated into WAL state. State, persistence, resource release, menu dispatch,
  and painted-row interaction are headless-verified; only final visual judgment
  of the four-tab strip and cursor popup remains runtime gap #1.

- **Closing a free session lands in the SAME project** — `NEEDS-RUNTIME` (branch
  `free-close-same-project`, 2026-07-24; amends `UXI-Workspace-9` clause 2).
  Reported: *"when I close a free agent session it drops me in a different project
  sometimes."* The first cut returned to the workspace you jumped FROM, which is a
  foreign project whenever you jumped across projects. Now the landing stays in the
  **closed session's** project: origin-if-same-project → that project's first
  workspace → (project has no workspace) **another session in it**, in its own bare
  agent view → origin/last. Guard
  `closing_a_free_session_lands_in_the_same_project` (three projects; both NCs
  observed RED, one reproducing the reported bug exactly). Also recorded the
  **terminology** the user asked for — "free" = a session no tile binds — in
  `docs/components/README.md` § Terminology, cross-referenced from the jump-panel
  and session-binding specs.

- **Cmd-P fuzzy jump palette (workspaces + agent sessions)** — `NEEDS-RUNTIME`
  (branch `cmd-p-jump-palette`, 2026-07-24 via `/new-ux`; `UXI-JumpPanel-9`).
  Captured verbatim: *"I want a Cmd-P jump command. When I hit Cmd-P a dialog
  appears that I can type into. It is doing a fuzzy match on both workspace names
  and agent session names. It shows a list of possible matches below. I can use
  arrow keys to select one directly. Or if I hit `<enter>` it jumps to the current
  top match."* **Built.** `Cmd-P` opens a centered palette over every non-ephemeral
  workspace + every agent session (`Local` ∪ `Roster`), projects excluded (a
  container, not a view target). Empty query = the full list in panel order; typing
  filters by subsequence and orders by score (exact > prefix > word-start >
  contiguous, shorter wins ties, panel order as tiebreak); arrows move the highlight
  without navigating; `Enter` activates the highlight (= the top match unless you
  moved); no-match `Enter` is a no-op that stays open; `Esc` closes; a second `Cmd-P`
  (or `Cmd-P` over another overlay) is a no-op. Candidates come from
  `jump_panel_sections` and activation goes through `select_workspace` /
  `jump_to_agent`, so the palette can't drift from the sidebar or grow its own jump
  semantics. New `jump_palette.rs`; 9 headless guards, each observed RED under a
  reverted-fix mutation; full suite green (466 + 157). **Remaining:** a human look
  at the popup's glyphs/colors/placement (harness gap #1) — the geometry is pinned
  by a layout probe, the pixels aren't.


- **Autonaming of agent sessions + jump-panel summary** — `NEEDS-RUNTIME`
  (branch `session-autonaming`, merged to `main` 2026-07-24 via `/new-ux`;
  `UXI-AgentTile-27`, spec `docs/components/agent-tile/naming.md`). Captured
  verbatim: *"I want autonaming of sessions based on the first couple of
  interactions. Possibly via Haiku. Should be a couple of words. When I use the
  rename command it should override."* plus, mid-build, *"also having a short
  summary (2 sentences) would be great"* and *"we could put the summary
  underneath in tiny italics on the jump panel."* When a session's FIRST agent
  turn completes, the opening exchange goes to `claude-haiku-4-5` over one plain
  HTTP `/v1/messages` call (no SDK exists for Rust; built on the `ureq` client
  Linear + Telegram already use) and comes back as a 2–3 word name plus a
  two-sentence summary. The name replaces `claude-N` everywhere; the summary
  renders under it in small italics in the jump panel and persists in
  `acp_sessions.json`. One shot per session, ever — restored sessions are never
  retro-named. An explicit rename latches a typed `NameOrigin::User`, which
  permanently blocks autonaming and drops an in-flight result rather than
  applying it; this replaces the `is_auto_claude_label` string sniff, which
  cannot tell an autoname from a name the user typed. Shape is enforced
  client-side (28-char name cap, 2-sentence summary cap, tolerant JSON/fence/
  bare-text parser), and every failure mode is silent — no key, no network, a
  refusal, or junk leaves the session as `claude-N`. `.env` is now gitignored and
  loaded privately at startup for `ANTHROPIC_API_KEY` (real env vars win; the
  key is not forwarded to ACP/MCP children). Guards:
  `autoname_fires_once_on_first_turn_completion`,
  `autoname_result_renames_the_session`,
  `rename_latches_origin_and_blocks_autoname`,
  `late_autoname_result_never_clobbers_a_user_rename`, plus unit coverage for the
  sanitizers / reply parser / `.env` parser; **four negative controls observed
  RED**; 457 gpui tests + full suite green. Remaining human check: the live Haiku
  round-trip end-to-end (harness gap #2 — the worker is `cfg(test)`-suppressed),
  and the italic summary line's exact size/color under folio + nightfox (gap #1).

- **Contextual "New Agent" + one-gesture close in the bare agent view** —
  `NEEDS-RUNTIME` (branch `contextual-new-agent`, 2026-07-24 via `/new-ux`;
  `UXI-Workspace-8`, `UXI-Workspace-9`, `UXI-AgentTile-23`). Two user
  requirements, both scoped to the **ephemeral virtual workspace** (what the user
  calls the "just agent view"):
  1. *"New Agent command should be contextual based on whether I have a workspace
     open. If workspace — open a new tile. If not, just open a new agent
     session."* → `.` → `n` → `a` still adds a tile in a real workspace, but in a
     bare agent view it swaps that single tile **in place** (no split) to the
     picker. The session it was showing is **freed, not killed** — still running,
     re-pickable from the picker that just opened. Both branches land on the same
     picker (the user reversed an earlier "immediately fresh" choice for
     consistency).
  2. *"closing an agent session is a pain in the ass … `<space>` `x` insert-mode
     `yes` … then `.` `x`."* → arming the close confirm now also drops you into
     insert **when the compose is empty** (a draft suppresses it entirely: `yes`
     appended to a draft would silently cancel, and clearing it would destroy the
     user's work), and answering `yes` in a bare agent view **dismisses the view**,
     returning to the origin workspace. Real workspaces are unchanged (tile stays,
     becomes the unbound selector).
  This **amends `UXI-AgentTile-22` rule 1** — its "no focus move" half now applies
  only to the draft case. Guards:
  `new_agent_splits_in_a_workspace_and_swaps_in_place_in_a_bare_agent_view`,
  `closing_the_session_in_a_bare_agent_view_dismisses_it`,
  `arming_close_drops_into_insert_unless_a_draft_is_at_risk`; **four negative
  controls observed RED**; 455 gpui tests + full suite green. Remaining human
  check: the live feel of the in-place swap and the dismissal, and the picker's
  server round-trip (harness gaps #1/#2 — needs the daemon).

- **Command panel (leader menu) aesthetic redesign** — `NEEDS-RUNTIME` (branch
  `main`, 2026-07-23 via `/new-ux`, autonomous, Fable advising UX + aesthetics +
  architecture; `UXI-Menu-1..5`, spec `docs/components/common/menu.md`, rationale
  `ADR-0029`). The old
  full-width (`.w_full().top_0()`) opaque drop-down bar is replaced by **"The Sigil
  Card"**: a floating, content-sized card (`[340, 720]px` band) in the workspace
  region right of the jump panel, horizontally centered, pinned 48px below the top.
  Each leader wears a scope hue on a 2px left accent bar + a header sigil
  (`space`→cyan `✦`/`▣`/…, `.`→`overlay.key` `⊞`, `?`→`jump_header` `◉`); the header
  breadcrumb is the literal **keystroke trail** you typed as key chips; entries are
  mono key-chip + label rows on a 26px grid; the footer collapses to an `esc` chip.
  Guards in `verify_harness.rs`: `menu_panel_floats_in_content_region`,
  `menu_panel_top_stable_across_descent`, `menu_panel_rows_and_sections_paint`; unit
  `tests.rs::menu_trail_crumbs_tracks_descent`. All four observed RED under reverted
  negative controls; 445 gpui tests + full suite green. Remaining human check: the
  exact chip colors / accent-bar hue / glyph legibility on folio + nightfox (the
  documented pixels-beyond-bounds gap).

- **Jump panel redesign (declutter + context menu + shading)** — `NEEDS-RUNTIME`
  (branch `jump-panel-redesign`, 2026-07-23 via `/new-ux`, autonomous, Fable
  advising UX + color; `UXI-JumpPanel-7`, `-8`). The panel loses its inline
  create/delete chrome: no CWD subtext line, no ＋New project / ＋New workspace /
  ＋New agent session rows, no ✕ close-project glyph. Project creation moved to the
  GLOBAL menu ("new project"); the per-project create + delete actions moved to a
  **context menu** opened by clicking the project NAME (New workspace / New agent
  session / Delete project, anchored at the cursor, Esc / click-away to close).
  Aesthetics: an inter-section **hairline rule above** each project header
  (replacing the underline); **icons** — `⊞` workspaces / status-colored `✦`
  agent sessions (the ●/○ dot is folded into the star's color; the ctrl-<n> number
  moves to a dim right-edge hint); **SEMIBOLD** row labels; and a **slightly
  darker panel background** derived per-theme (`jump_panel_bg`: fixed ΔL with a
  lighten-flip on near-black themes, hue+sat preserved). Guards in
  `verify_harness.rs`: `jump_panel_bg_shades_by_theme_and_preserves_hue`,
  `project_menu_opens_on_name_click_and_actions_dispatch`,
  `new_project_relocated_to_global_menu` (NCs noted). **Runtime check:** the
  literal glyphs / colors / popup placement are harness gap #1 (human eye).

- **Subagent-pane markdown wrapping + Task-output pretty-print** — `NEEDS-RUNTIME`
  (built 2026-07-23, autonomous; `UXI-AgentTile-26`, on `worktree-agent-a11a3fd6ea26222bc`).
  Two live-screenshot defects in the UXI-AgentTile-25 tool-body sections: (1) a
  markdown bullet list of long unbroken file paths rendered as a **vertical column
  of one glyph per line** — root cause: `render_markdown_column` never gave each
  block a definite full width, so a list item's `flex_1().min_w_0()` content column
  got 0px and wrapped char-by-char; fixed by wrapping every block in a `w_full()`
  row + `flex_1().min_w_0()` inner (mirroring the doc view's `block_element`), plus
  `w_full()`/`flex_none()` on `list_item_element`. (2) A subagent's Task output — a
  **bare** content-block array `[{type:"text",text:"…"}]` — dumped as escaped JSON;
  fixed by extending `extract_output_text` with a `Value::Array` arm so it renders
  as the readable markdown report. Guards: `subagent_markdown_list_wraps_at_pane_width`
  (layout probe, NC RED) in `verify_harness.rs`; `extract_output_text_handles_bare_content_block_array`
  + `plan_tool_sections_bare_array_output_is_markdown_not_json` (NC RED) in `tests.rs`.
  **Runtime check:** exact painted glyphs/theme colors are gap #1 (human eye).

- **Beautiful subagent / tool-call content rendering** — `NEEDS-RUNTIME` (built
  2026-07-23 via `/new-ux`, autonomous, Fable advising; `UXI-AgentTile-25`). Tool
  inputs/outputs now render as typed semantic sections instead of raw JSON: a
  subagent's prompt + report as **markdown**, Bash as a **command code** block,
  edits as **diffs**, params as **chips**; terminal output stays monospace;
  content/output dedup; JSON only for unknown shapes; theme-driven + zoom-aware.
  New `tool_body.rs` (pure `plan_tool_sections` + render layer) replaces
  `append_tool_body`/`tool_body_free`; wired into the subagent view (`screens.rs`)
  and both transcript tool paths (`transcript_view.rs`). Guards: 6 pure tests in
  `tests.rs` (NC observed RED). **Runtime check:** the actual rendered
  fonts/colors/markdown layout is gap #1 (human eye); a `sample` on a huge report
  would confirm no jank (the parse cache was deferred — see the deviation note).

- **Abandon-reply gesture in the worksheet You-block** — DONE (2026-07-23 via
  `/new-ux`, `UXI-AgentTile-24`). `<esc>u` in an open worksheet You-block backs
  the reply out undo-style: normal undo of any typing, then `u` pops the block
  (clear draft + close active block, parked untouched) → transcript Normal nav.
  In the common `r → Esc → u` case (`r`-seed is a committed baseline) the first
  `u` pops. Guard `worksheet_esc_u_backs_out_reply_block` (real keystrokes; NC
  observed RED). Headless — no runtime gap beyond painted glyphs (#1).

- **Projects as the top-level organizational primitive** — `NEEDS-RUNTIME`
  (branch `projects-primitive`, **all 7 tickets landed + green**, 2026-07-23 via
  `/new-ux` + a Fable-advised workflow; **not yet merged to `main`**). All of
  `UXI-Project-1..8` implemented; 426 tests green; adversarial review caught one
  last-project-delete bug, fixed + guarded (`e25a43b`). Runtime-unverified in the
  live GUI (the daemon session-close/create round-trips + pixels are harness
  gaps #1/#2). **Next step: merge to `main` + a human launch.** Model + tickets:
  ADR-0028 + `docs/components/project.md` (`UXI-Project-1..8`) + `/plan`
  `docs/projects/projects-primitive/`.
  **Landed + verified (branch `projects-primitive`, 421 tests green, 4 commits):**
  T001 `Projects` store (name+cwd unique, `Membership`, `ensure_at_cwd`); T002
  `projects.json` persistence + migration + `boot_projects`; **T003** the FK swap —
  the workspace holds a required-private `ProjectId`, cwd is **derived** at the
  point of use and **never cached** (`workspace_and_session_cwd_derive_from_project`,
  NC RED); T004 core — jump-panel session headers show the owning **project name**.
  Fable-advised durable model: `ProjectId` foreign key (denormalized cache rejected
  by name in the ADR); `AgentSession` keeps its immutable spawn cwd (corrected §3).
  **Remaining (well-scoped, each owes a UXI guard):** T004 tail (per-project
  workspaces sublist + per-project ＋New rows + top-level ＋New project); T005
  create/delete-project overlays + cascade + remove the global cwd overlay; T006
  intra-project bind gate + `active_project()`; T007 the `tab`→`workspace` /
  `Workspace`→`Frame` eradication (supersedes ADR-0002's deferral). Not yet merged
  to `main`. Original ask: "Change our
  organizational primitive vocabulary and hierarchy. Projects are at the top.
  Projects have a CWD, and a map of other parameters. Workspaces belong to projects;
  Tiles are in workspaces. Agent sessions belong to projects; they can also be bound
  to tiles. A Project can have no workspaces and all agent sessions, or no agent
  sessions and a bunch of workspaces with tiles (all Linear or Buffer). The jump
  panel should represent this hierarchy. As part of the migration, change all active
  CWDs `ws/yaldabaoth` and `ws/fulcrum` to projects Yaldabaoth and Fulcrum
  respectively. This will allow us to set other configurations on projects as well."
  Big architectural change: introduces `Project` above the current cwd-keyed grouping;
  today the cwd IS the implicit project axis (jump panel groups sessions by cwd; each
  Tab carries a `WorkspaceCwd`). Likely spawns `/spec` + `/decision` + `/plan`.
  Owning components: `JumpPanel`, `Workspace`, `AgentTile` (session ownership),
  plus a new `Project` component. Status stays `NEEDS-DECISION` until interrogation
  resolves the model.

- **Extra vertical spacing between paragraphs / blocks / bullets in text tiles** —
  `NEEDS-RUNTIME` (built 2026-07-23 via `/new-ux`; `UXI-ParagraphSpacing-1`). User:
  "a few extra pixels between every newline … to break up paragraphs" + "space
  between bullet points." Resolved to option **B** (gap *between* blocks/paragraphs
  and *between* list items; within-paragraph leading unchanged), on Doc view + agent
  transcript + WP edit (Code/RAW + compose + chrome excluded), scaled with zoom.
  Shared `PARAGRAPH_GAP_PX`/`paragraph_gap` in `render_blocks.rs`; applied as
  **padding** (a `gpui::list` ignores item margins — the old `mb_2`/`mt/mb` were dead
  spacing) on Doc blocks + transcript blocks, flex `gap` on list items, scaled
  `top_pad`/blank-row height in WP. **Transcript PROSE paragraphs** also covered
  (added after a user screenshot showed them tight): the blank-collapse pass drops
  the blank `FlatItem::Line`, so paragraphs render adjacent — fixed by paragraph-start
  top-padding detected over `lines_snap` (frozen-only, so draft/compose excluded).
  Guards: `paragraph_gap_between_doc_blocks_exceeds_within_paragraph_leading` +
  `transcript_paragraph_start_row_is_taller_than_within_paragraph_row` (both NC
  observed RED at 0px delta). **Runtime check:** exact feel per surface (WP +
  transcript pixels are gap #1, human eye) — `PARAGRAPH_GAP_PX` tunable.

- **Agent transcript: faint-blue background on the user's own turns** —
  `NEEDS-RUNTIME` (built 2026-07-22 via `/new-ux`; `UXI-AgentTile-23`, ADR-0027).
  "I want the background color of my responses in an agent session to be a slightly
  different color. Possibly a faint blue." User turns (`TurnId::User`) now render on
  a faint blue band (`AgentTheme::user_turn_bg`, retuned from warm-green to blue in
  all 8 themes); agent/tool/system turns stay on the plain tile background. Reversed
  `UXI-AgentTile-4` for user turns only (ADR-0027). Pure selector
  `committed_row_bg`; nav-focus cursor-row highlight still overrides on its row.
  Guard: `user_turn_gets_tint_agent_turn_does_not` (NC observed RED). **Runtime
  check:** the exact blue shade per theme — confirm it reads as "faint/pleasant" on
  each theme (gap #1, human eye); shades are tunable in `src/theme.rs`.

- **Jump panel: red bounding box on the active screen UX element** —
  `NEEDS-RUNTIME` (built 2026-07-22 via `/new-ux`; `UXI-JumpPanel-5`). "The active
  screen UX element (either a workspace or an agent session) should have a red
  bounding box in the jump panel." A 1px red box (`DetailStyle.err` = `0xff6b6b`,
  rounded) marks the active-workspace row (additive over its accent label +
  selection tint) and, when the focused tile is a bound agent, that session's row
  — 0/1/2 boxes. Ephemeral (free-session) workspaces aren't listed, so only the
  session boxes in that state; a buffer / unbound-agent tile boxes no session.
  Pure predicate `jump_target_is_active` over `jump_active_session()`
  (focused-tile bound session). Guard:
  `jump_active_box_marks_focused_workspace_and_session` (NC observed RED). The
  literal red pixels are harness gap #1 — human eye.

- **Free agent session: choose its CWD at create** — `NEEDS-RUNTIME` (built
  2026-07-22 via `/new-ux`; `UXI-JumpPanel-4`). "When creating a new agent session
  outside of a workspace, need a way to specify what CWD." Both free-session entry
  points (jump-panel ＋ row, `?`-menu "new agent session") now open a path-input
  overlay ("NEW AGENT SESSION AT…") pre-filled with `agent_base_cwd()` (the active
  workspace's cwd, else process cwd) — Enter accepts the default, or type/edit a
  path. Commit resolves via `resolve_agent_cwd_arg` (tilde/canonicalize/validate);
  a bad path surfaces a transient error and creates nothing; a good path routes to
  `spawn_free_agent_session_at(cwd)`. Reuses the existing `RenameOverlay` machinery
  (new `RenameTarget::FreeAgentSessionCwd`). Guards: overlay opens pre-filled, both
  entry points open it, commit routes valid→spawn / invalid→error. The session
  actually created AT that cwd needs the daemon (harness gap #2).

- **Create a free (tile-less) agent session from the jump panel** — `NEEDS-RUNTIME`
  (built 2026-07-22 via `/new-ux`; `UXI-JumpPanel-3`). "I want to create new agents
  that aren't attached to a tile in a workspace." Capability already existed
  (`spawn_free_agent_session`, `?`-menu "new agent session") but was checkpointed
  WIP — un-spec'd, unguarded, and undiscoverable. This makes it a first-class,
  discoverable action: a **＋ New agent session** row at the top of the jump panel's
  "Agent sessions" section calls the same `spawn_free_agent_session`; the created
  session lands in the universal roster as an unbound (○) row, never auto-bound,
  bindable later by selecting it. Guards: `free_agent_session_no_server_is_graceful_noop`
  (real method, no-server contract, NC observed), `free_agent_row_is_unbound_and_bindable`
  (roster session surfaces as free ○, then binds via `jump_to_agent`). Runtime check:
  the live server `create_session` round-trip needs the daemon (harness gap #2) and
  the ＋ row's click paint (gap #1).

- **Blockquoted text is italic everywhere** — `NEEDS-RUNTIME` (built 2026-07-21,
  branch `quote-parser-blockquote-italic`, NOT yet merged; `UXI-Blockquote-1`,
  new spec `docs/components/common/blockquote.md`). `>`-quoted text renders italic
  on ALL six render paths. Three already did (rendered doc block, transcript
  parsed block, WP view); three did not and now do: the `md_highlight` per-line
  path (agent transcript source-highlighted lines + RAW edit view) via
  `Modifier::ITALIC` on **every** segment of the quote — so nested `**bold**` /
  `` `code` `` spans don't punch upright holes — and the compose / virtualized
  compose / inline You-block via `.italic()` in `build_chatbox_wrapped_line`
  (single choke point for all three). Only a line-leading `>` counts (`a > b`
  stays upright). Guards: `blockquote_segments_are_italic` (lib, NC observed) +
  `is_blockquote_line_matches_leading_marker_only`. Runtime check: the italic
  actually shows in the live compose/RAW view (harness gap #1 — paint, not state).

- **Worksheet `r` = reply-with-quotation** — `NEEDS-RUNTIME` (built via
  `/new-ux`; `UXI-AgentTile-21`). In an idle worksheet, transcript-focused, with
  the caret on an agent line at a legal insertion point, `r` opens a You-block
  like `o` but **seeded** `re\n> <first N sentences of the caret's line>\n`, caret
  on the trailing blank line. `N` is the shared vim `pending_count` (default 1,
  clamps to available); `3r` quotes three sentences. Sentence = `.`/`!`/`?` +
  whitespace/EOT, with abbreviation/decimal special-casing (`first_n_sentences`).
  Same idle/legality gate as `o`; blank/no-sentence line → no-op.
  `agent.rs::reply_quote_at_cursor` + `first_n_sentences`, `agent_ui.rs` `r`
  branch. Guards: `worksheet_r_seeds_reply_quote_from_agent_line`,
  `worksheet_count_r_quotes_n_sentences`, `worksheet_r_noop_on_blank_line`,
  `first_n_sentences_splits_and_respects_abbrevs` (386 pass, NC observed). Runtime
  gap: the inline caret tint over the reply is a human-eye check (harness gap #1).
  - **Bold-breaks-the-parser FIXED** (2026-07-21, branch
    `quote-parser-blockquote-italic`): `*this sentence is bold.*` put a `*` between
    the `.` and the space, so the terminator+whitespace rule missed the boundary
    and the sentence ran on into the next one. `first_n_sentences` now consumes a
    run of closing markup (`*_`` ` ``~)]}"'»”’`) after the terminator and keeps it
    inside the sentence, so quoted markup stays balanced. Guard
    `first_n_sentences_terminates_through_closing_markup`; NC reproduced the exact
    symptom (`"*this sentence is bold.* Next one."`).

- **Plane view pans slightly on tile drag/resize** — `NEEDS-RUNTIME` (built this
  session via `/new-ux`; `UXI-Workspace-8`). Committing a tile drag or edge-resize now
  snaps the camera pan to whole slot units (`DesktopState::snap_camera_to_slots`,
  called from `chrome.rs::desktop_drop` + the reveal block), so the view rests
  cell-aligned like the tile instead of the fractional edge-auto-pan drift. Guards:
  `tile_drag_rests_view_cell_aligned` + `snap_camera_to_slots_rounds_and_preserves_slots`,
  both NC-verified. Awaiting human confirmation that the drift is gone / feel is right.

- **No way to hide the agent sidepanel** — `NEEDS-RUNTIME` (built this session via
  `/new-ux`; `UXI-AgentTile-20`). `Cmd-B` (AgentView-scoped, shadowing the global rail
  toggle) force-hides the whole right sidepanel via `AgentState::sidepanel_hidden`;
  stays hidden even with plan/subagent content; `Cmd-0` un-hides+focuses; persists per
  session. Guard: `cmd_b_hides_and_cmd_0_reshows_the_sidepanel` (NC-verified) + persist
  round-trip. Awaiting human check of the real `Cmd-B` chord (rule-4 gap).

- **Subagent row rendering in the sidepanel** — `NEEDS-RUNTIME` (built 2026-07-13
  via `/new-ux`; `UXI-AgentTile-17`). The old one-line glyph+label+prompt row read
  as two cramped mismatched-color columns in the 280px sidepanel. Now a **two-line
  stacked row**: line 1 = glyph + label (foreground; warm accent when focused);
  line 2 = the prompt snippet, dimmed + indented, single-line ellipsized (rows stay
  short). Headless PAINT guard `subagent_row_stacks_label_over_prompt`
  (negative-controlled RED); full suite 357 green. Human check (gap 1): the exact
  indent/dim look in the live sidepanel.

- **Infinite-plane workspace** — `NEEDS-RUNTIME` (2026-07-12, merged to `main`).
  "each workspace is actually infinite — an
  unbounded grid of slots in all directions; tiles can span multiple slots;
  zoom in/out; pan around; reset the view to origin, where all workspaces start."
  A workspace is now one infinite signed-coordinate plane with a pan +
  discrete-semantic-zoom (`Full`/`Card`/`Minimap`) camera + reset-to-origin;
  the layout-mode / master-stack / split-resize / equalize surface is retired
  (`SplitH`/`SplitV` remain as the plane's new-tile mechanism). Built in 4 staged
  subagent passes (engine → persistence → Detail render → surface-retirement +
  binding reflow). `UXI-Workspace-2..7` all `implemented` + headless-guarded (12
  new tests, each negative-control-verified RED-then-green); design doc
  `docs/specs/spec-infinite-plane-workspace.md`. Build clean; full suite green
  (gpui bin **362 pass**, lib **154 pass**). **Runtime checks pending:** the
  `Ctrl-W 0/-/=` chord firing (macOS post-leader-digit key gap, rule 4), and the
  pan/zoom scroll *feel* + Card/Minimap pixels. Follow-up: rewrite the
  `workspace.md` Description prose around the plane.

- **Event-log O(n²) append stall FIXED** — `NEEDS-RUNTIME` (2026-07-11, on
  `main` `bf6bbe8`; `docs/worklog/2026-07-11-eventlog-on2-stall.md`). The
  per-session event log was `Arc<Vec>`; publishing a snapshot every append left
  an outstanding ref so the next push's `make_mut` deep-cloned the whole log —
  O(n²) over a session, stalling the actor for seconds on a big session (FoF,
  28k events) and making messages spool/release in bursts. Now backed by
  `imbl::Vector` (O(1) clone, O(log n) append). Diagnosed live over the server
  socket; full suite green + negative-controlled perf guard. Human check:
  `restart all`, then confirm a long session streams smoothly with no bursty
  stalls. Follow-up worth considering: an ADR on the persistent-vector choice.

---

## Features

- **Render mermaid diagrams inline** — `NEEDS-RUNTIME` (built 2026-08-11 via
  `/new-ux`, Cog graph `sve`; renderer swapped to native merman 2026-08-12, Cog
  graph `zwh`, ADR-0031; `UXI-Diagram-1`; on `main`). A ` ```mermaid ` fence renders
  as its diagram inline on the agent transcript AND the buffer Viewing surface
  (shared `block_inner`); Editing shows raw source. Mechanism: **in-process native
  `merman`** (parse→layout→SVG→resvg→PNG) — no `mmdc`, no Node/Chromium, no `PATH`.
  Painted via `img()`, cached by `hash(source+theme)`; placeholder + fallback = raw
  highlighted source (+ note) when merman can't render (unsupported type / parse
  err) — never blank; theme-matched. No zoom/click v1; the image opts out of
  per-line hit-testing. Headless-guarded: `diagram_001/002/003` (3 NCs observed RED)
  + `merman_tests::real_merman_renders_flowchart_to_png` (real engine, valid PNG) +
  `unrenderable_source_errors_without_panic`; 556 bin + 167 lib green. merman covers
  flowchart/sequence/class/ER/XY (other types → fallback). Deviations: width dropped
  from cache key (paint can't know layout width); nested-in-list mermaid falls back
  to source; no cache eviction on theme switch; merman 0.6.2 pinned (0.7 needs rustc
  1.95) with `roughr-merman` pinned to 0.12.0 in `Cargo.lock`. Runtime gap (only
  `NEEDS-RUNTIME`): gap 1 the painted PNG pixels/theme colors (a human eye); the old
  live-subprocess gap is gone.
- **Close session needs a confirm** — `NEEDS-RUNTIME` (built 2026-07-21 via
  `/new-ux`; `UXI-AgentTile-22`). The agent space-menu `x` no longer closes: it
  appends `> <Yaldabaoth System>: Confirm close session (yes or any key for no)?`
  to the session's own transcript (never sent to the agent) and arms a gate that
  swallows the next submit on either surface. Trimmed `yes` → real close; anything
  else cancels, sends nothing, and leaves the draft untouched. Arms regardless of
  turn state; no focus move / no You-block opened (user's call); the prompt line
  stays as a permanent record and a second `x` appends another. Guard
  `close_session_requires_typed_yes_confirmation` (channel-level "nothing sent"
  assert), 2 negative controls observed RED; 397 bin + 156 lib green. Runtime
  check (gap 1): the `>`-quoted line renders as intended in the live transcript.
- **Image paste into a session** — `NEEDS-RUNTIME` (built 2026-07-09, branch
  `image-paste`, NOT yet merged; INV-UX-21). Cmd+V of a clipboard image stages it
  as a pending attachment (chip above the compose), sent on submit as an ACP
  `ContentBlock::Image` (both submit paths) with a `🖼 image N (EXT)` transcript
  marker; attachments clear after send and are ephemeral (not persisted). Wire
  carries `Request::Prompt.images` additively. Headless-tested end-to-end (paste
  staging, mixed content-block build, wire round-trip, real worksheet submit; 2
  negative controls RED). Human check (harness gap 2, live loop): the
  `claude-agent-acp` adapter actually advertises the `image` prompt capability +
  reads the pasted image — NOT gated on the capability yet, so verify it doesn't
  error; gap 1 for the chip glyphs. See `docs/worklog/2026-07-09-image-paste.md`.
- **Session recap panel** — `NEEDS-RUNTIME` (built 2026-07-09, on `main`
  `36bdc8a`; INV-UX-20). Agent space-menu `R` ("recap this session") generates an
  LLM prose summary of the focused session on a THROWAWAY isolated
  `AcpChannelClient` subprocess and pins it at the top of the jump panel, above
  the session list; re-runnable (`⟳`), dismissed (`✕`), pinned until dismissed.
  Reducer + panel + token-guard supersession are headless-tested (7 `recap_*`, 2
  negative controls RED). Human check (harness gap 2, live subprocess): with the
  agent on PATH, `R` streams a summary in, `⟳` re-runs, `✕` dismisses, and the
  throwaway worker EXITS (no lingering `claude-agent-acp`); gap 1 for the panel's
  exact look. The `spawn_recap_worker`→pump wiring is the only untested seam
  (`cfg(test)`-skipped).
- **Agent model switcher (per session, live)** — `NEEDS-RUNTIME` (built
  2026-07-09, merged to `main`; INV-UX-22, `docs/projects/agent-model-switch/`).
  Switch a tile's model (Opus / Fable / Sonnet / …) live from the agent's
  advertised picklist via ACP `session/set_config_option` — `space M` submenu or
  the clickable `model ▾` status-strip badge. Full suite green + 3 new headless
  tests (each negative-controlled) + `#[ignore]` live round-trip. Rebuild +
  restart to pick it up in the running binary. Follow-up: the `effort` option
  (low..max) could get the same treatment. Human check: the badge shows `▾` +
  opens the menu, picking a model flips the badge live and the next turn uses it.

- **Jump panel (root-level navigator)** — `NEEDS-RUNTIME` (built 2026-06-22,
  merged `e3fa254`/`720b7a0`; spec `spec-jump-panel.md`, ADR-0021). Always-visible
  left sidebar (Pinned placeholder · Workspaces · Agent sessions), `cmd-j`/`?`
  toggle (persisted), free-session select → ephemeral virtual workspace. Inline
  render (cheap; a root-reading cached child double-leases). Human check: visible
  across workspace switches, active workspace highlighted, free-session
  open-then-vanish on jump-away.
- **Universal agent roster** — `NEEDS-RUNTIME` (built 2026-06-22, merged
  `4ec7a62`; spec `spec-universal-agent-list.md`, ADR-0022). One `AgentRoster`
  (all server sessions, live on Created/Closed/Renamed broadcasts, seeded at
  boot); jump panel + tile selector both project from it. Human check: a session
  created/renamed/closed elsewhere updates both surfaces live; selecting one
  moves it free↔bound in both.
- **Workspace cwd is a required typed field** — `NEEDS-RUNTIME` (built
  2026-06-22, merged `1329898`/`e942960`; ADR-0023). `Tab.cwd: WorkspaceCwd`
  (private, required); a new agent inherits the LIVE active-workspace cwd; Set
  CWD persists across restart. Human check: Set CWD → new agent runs in that dir;
  survives relaunch. NOTE: pre-existing `~/.yalda/workspace.json` entries have no
  stored cwd → the first Set CWD per workspace populates it going forward.
- **Desktop mode** — `NEEDS-RUNTIME` (built 2026-06-10, spec
  `spec-desktop-mode.md`, engine `1f7c269^..1f7c269` on master). Fifth
  per-tab LayoutMode (`Ctrl-W Space` cycle, sigil `[#]`): fixed-size tiles
  (global `{cols}x{rows}` via `Ctrl-W p`, default 120×40) on a pannable slot
  grid; drag tiles by title bar (insert-and-shift, right-click cancels);
  spatial focus via the usual `Ctrl-W h/j/k/l`. Human checklist: drag feel +
  drop targeting, scroll-pan + edge auto-pan, typing/keys inside each tile
  kind (Doc/Edit/Browser/Agent), focus-offscreen recovery (focus a panned-out
  tile → auto-reveal), mode round-trips (Manual ↔ Desktop preserves both
  arrangements), restart persistence. Deferred polish, in spec but not v1:
  Esc-to-cancel drag at canvas root (global escape binding would shadow
  per-screen escape; needs a careful dispatch design); measured mono cell
  size (currently 0.6em/1.4em approximation in `desktop_tile_px`).

- **Auto-resume the same session in each agent tile on restart (identity-based)** —
  `NEEDS-RUNTIME` (Part 1 built 2026-07-13 via `/new-ux`; `UXI-AgentTile-18`,
  ADR-0025, `docs/components/agent-tile/session-binding.md`). Requirement (verbatim,
  07-13): "Yalda should remember what agent tiles are occupied by what sessions. When
  I restart the system it should automatically resume those agent connections IN
  those tiles. I do not want to need to select from a picker again." Root cause was
  **positional index-zip** (`workspace.json` wrote `Agent { session_id: None }` and
  `restore_agent_leaves` zipped sessions to leaves by index → picker whenever the zip
  broke). **Fixed by identity:** each agent leaf now persists its bound session's
  server id in the layout (`resume_sid` cached on the tile by `save_agent_ring`,
  written by `snapshot_content`); `restore_agent_leaves` rebinds each tile to its OWN
  id — no positional zip, no picker on restart. Headless guard
  `agent_tile_persists_session_identity_not_index` (negative-controlled RED); 358
  suite green. **Human check (harness gap #2):** restart yalda (GUI-only AND full
  reboot) → each tile reconnects to the session it held, no picker. **Part 2 BUILT
  (`UXI-AgentTile-19`):** a genuinely unresumable session (permanent "no such
  session" on the resuming attach) shows an inline "session unavailable — start
  fresh" notice, never the picker — `spawn_attach_sessions` gained a `resuming` flag
  routing it to `reconcile_session_unavailable`; identity kept for a later
  re-attempt; "Start fresh" opens a new session in the tile. Guard
  `unresumable_session_shows_inline_notice_not_picker` (state + PAINT,
  negative-controlled RED); 359 suite green. Human check (gap #2): the live "session
  gone" attach result actually reaches this path on a real dead session.
- **Click anywhere on a tile focuses it** — **DONE** (2026-07-20), spec'd as
  `UXI-Workspace-9`. A left press in an unfocused Full tile's **body** focuses it and
  is **consumed** (click-to-focus: first click only focuses, second click interacts) —
  capture-phase `capture_any_mouse_down` + `stop_propagation` on the tile-body div in
  `chrome.rs render_desktop`, plus `desktop_focus_click`. Carve-outs: title bar /
  resize bands keep focus-and-drag in one gesture; Card/Minimap placeholders out of
  scope. Guards `click_in_unfocused_tile_body_focuses_and_is_consumed` +
  `title_bar_press_on_unfocused_tile_still_focuses_and_arms_drag`, both
  negative-controlled RED; 389 suite green. Runtime-unverified (feel of the
  two-click interaction is a human check).
  <br>*(stale) 2026-07-12 note:* "already implemented for splits — chrome.rs:830
  `on_mouse_down(Left) → focus_window_by_click`" — that code was deleted by the
  infinite-plane refactor (5fe623c); the plane had no click-to-focus at all.

## Bugs

- **Worksheet resume: cursor lost / undo erased the buffer / tool calls at the
  bottom** — `FIXED` + `NEEDS-RUNTIME` (2026-06-22, merged `1560db7`/`a7beb83`;
  worksheet-frozen-blocks ticket 001). Data was always safe (server WAL). Three
  fixes, headless-tested: (F2) `programmatic_insert` didn't shift the view caret
  → `Editor::splice_insert/_delete`; (C3) `undo` reset line anchors → now SHIFTS
  them; (THE repro) `begin_insert` opens one undo group and agent chunks streamed
  mid-insert recorded into it → agent/programmatic splices are now non-undoable
  (`*_no_undo` + `shift_recorded_splices`). Human check: reopen a multiturn
  worksheet session, type, let it stream, undo — your edits revert, the
  transcript stays; caret findable; `G` reaches the bottom.
- **Worksheet caret rendered below the visible buffer (on entry / nav)** —
  `FIXED` + `NEEDS-RUNTIME` (2026-06-22, ticket-001 fingerprint item).
  `view_model_fingerprint` folded in neither the input surface nor the worksheet
  caret line, so entering Worksheet mode (or moving the caret onto a collapsible
  blank) reused a flat list that stripped the trailing editable tail → caret on
  a roomless line. Fix: fold `InputSurface::Worksheet` + the worksheet caret line
  into the fingerprint (option 1, worksheet-scoped — chatbox typing stays
  render-flat); `finish_replay` snaps the caret to the editable tail on reopen.
  Human check: a `--release` `sample` holding `j` in a huge worksheet to confirm
  the per-nav S1 rebuild is imperceptible.
- **Worksheet ticket-001 remaining (deferred deep)** — `SUPERSEDED by Model C`
  (2026-06-24, ADR-0024). The **floor-only-EOF** edge case no longer exists: the
  user draft lives in a separate `Compose` buffer, never in the transcript, so
  there's no mid-document draft for a stream to overwrite; `agent_tail_floor_char`
  always returns EOF and the `append_llm_chunk_floored` path is inert. Pinned by
  `inv_order_*`. Ticket closed.
- **Mid-turn message drops (lease gate + invisible rejection)** — `FIXED`
  (2026-06-09, `b7bdcde` on master); `NEEDS-RUNTIME` for the GUI
  PromptRejected surfacing (notice + chatbox restore — headless tests cover
  the server half only). Root cause was two-part: `prompt()` is
  fire-and-forget so a server rejection had no waiter (the optimistic echo
  made it look sent), and `do_prompt` demanded a LIVE lease — an App-Napped
  window's lease lapses during a long turn, so the first post-wake message
  raced the 5s heartbeat reclaim and silently lost. Fix:
  `acquire_or_renew_lease` (action-as-liveness, shared with Owner attach) on
  prompt/cancel/mode/restart + `Notification::PromptRejected` to the
  submitter with the text restored into the chatbox. Tests 3b/3c/3d in
  `session_transcript_test.rs` (red on old gate, green now).
- **Agent transcript typing lag (worksheet + while-streaming)** — `FIXED`
  (2026-06-09, `8af1d4c` merged to master); `NEEDS-RUNTIME` (worksheet typing
  feel + typing-while-streaming on the real resumed session). Both shared one
  hot path: every `edit_seq` bump (worksheet keystroke; every streamed chunk)
  misses the S1 view-model cache, and the rebuild (a) deep-cloned EVERY
  parsed `RenderedBlock` into per-rebuild lookup maps, and (b) on streaming,
  re-parsed (pulldown-cmark + syntect) the WHOLE frozen transcript per chunk
  because the block cache was keyed by `(start,end)` and chunk inserts shift
  every range. Fix: S1 rebuild extracted to `rebuild_agent_view_model()`
  (headlessly testable — first real seam into GPUI render cost, progresses
  the verification-harness goal); `FlatItem::Block(Rc<RenderedBlock>)` +
  `resolved_blocks` (Rc bumps, no clones); content-hash block-cache keys
  (parses survive range shifts); metadata-view hoist in the cursor-reveal
  loop. Probe: 3,151 lines / 50 code blocks → ~135µs per keystroke rebuild
  (debug). Identity/INV-10/probe tests in the gpui tests mod.
  Left open (minor): tag-bar `all_tags()` walk + per-leaf `mark_for_window()`
  scan per frame (new in 09e266b, small constants).
- **Theme switch leaves agent transcript caches stale** — `FIXED` (2026-06-12,
  `91a6885`; re-confirmed 2026-06-25). `set_theme` calls
  `AgentViewModel::invalidate_theme()` (clears `block_cache` +
  `block_cache_frozen_fp` + `view_model_fp`) for every live session, rebuilds the
  edit-view syntect highlighter, and busts every transcript view via
  `notify_transcript_views`. The READY entry was stale; the fix landed right after
  it was filed.

- **Resume hang (replay fence never cleared)** — `FIXED` (2026-06-09,
  `9112188` on master). After a server restart, a recovered session's pump
  fence waited for the channel turn counter to reach the restored count — but
  the counter restarts at 0 every spawn and `092c218` removed the post-load
  bump, so the fence never cleared and EVERY post-resume event (replay, marker,
  live turns) was silently discarded: prompts looked hung while the agent
  worked invisibly (a queued "integrate" actually ran + folded a branch to
  master unseen). Fix: marker-based fence (`src/replay_fence.rs`), worker emits
  `ReplayComplete` on every resume attempt incl. fallbacks, pump reports
  session-absolute TurnCounts (`turn_base +`), restart-with-resume arms the
  fence (kills the restart double-record). Regression test:
  `recovered_session_is_drivable_after_resume` (red pre-fix, green post-fix).
  Residual hazard noted in code: a timed-out `session/load`'s late replay
  notifications can record as live events (bounded duplication, not a wedge).
- **Leaked `claude-code-acp` adapter processes** — `FIXED` (2026-06-25,
  `fd858d7`). Graceful exits already reap via `kill_on_drop`; the leak was the
  crash/SIGKILL/panic path where Drop never runs and the adapter reparents to
  PID 1. Fix: a startup reaper in both binaries' `main()`
  (`acp_channel::reap_orphaned_adapters`) SIGKILLs adapter processes with
  `ppid == 1` (definitively orphaned — can't hit a live session's adapter) whose
  command matches an adapter needle. Pure parser `orphaned_adapter_pids` is
  unit-tested. (A deeper per-close pump-join was considered unnecessary — the
  graceful path already reaps; the reaper covers the rest.)
- **Reconnect bursts at GUI launch** — `NEEDS-RUNTIME` (probable root cause
  fixed 2026-06-10, `3f85365`). The shared server pump was stored in an agent
  SLOT; every slot-state replacement during startup (restore → re-bootstrap →
  set_screen) cancelled the pump, dropped the notification receiver, killed
  the connection, and triggered a reconnect — hence ~25 conns per launch and,
  once timing shifted, hard "attach failed: session server disconnected" for
  new sessions. Pump is now a view-lifetime singleton (like the lease
  heartbeat). Verify the burst is gone in the server log after a few
  launches.

- **Edit-view typing crash + latency** — `FIXED` (2026-06-06). `reparse` fed
  tree-sitter a stale (never-`edit()`'d) tree → nondeterministic SIGSEGV
  (`d32edf9`); then full-parse-per-keystroke was slow → proper incremental
  reparse, fuzz-guarded, 10–20× faster (`413da19`). See worklog
  `2026-06-06-reparse-segfault-and-incremental.md`.
- **Reparse may be wasted work** — `CONFIRMED + READY` (verified 2026-06-25).
  The tree-sitter tree (`tree_state` / `block_boundaries`) is consumed ONLY
  inside `tree.rs` + `editor.rs` + tests — nothing in the GPUI render path reads
  it. So the per-edit `reparse` (editor.rs:1029/1065/1136/1169/1197) maintains a
  tree the live app never renders from. Making it lazy/skippable would remove
  that cost. NOT done here: it touches the editor hot path and the incremental
  reparse already cut the cost 10–20×, so low priority — but the premise is now
  confirmed, not speculative.

- **Session-server reconnect storm** — `ROOT-CAUSED + FIXED on branch
  session-resilience` (2026-06-07); `NEEDS-RUNTIME` for the GUI reconnect path.
  **Root cause:** `SessionServerClient` had no socket shutdown on drop. Its
  reader thread is detached and blocks forever on `lines()`, so dropping the
  client (notably `reconnect()`'s `*self = fresh`) leaked the thread AND kept
  the socket fd open — the **server never saw the disconnect, so it never
  released session ownership**. Every in-place `reconnect()` orphaned a zombie
  owner; the next re-attach was rejected with "another GUI already owns this
  session", and the connection only truly closed at process exit. That is the
  489-reconnects / few-closes pattern and the `close/create … disconnected`
  round-trip failures. **Fix (4 files):** (1) `Drop` now `shutdown(Both)`s the
  socket so the reader unblocks and the server releases ownership at once;
  (2) reconnect re-attach moved off the paint thread via the existing
  `spawn_attach_sessions` (Owner-reclaim retry) instead of raw inline blocking
  attaches that also froze rendering; (3) `attach_owner_with_retry` added to the
  client lib for the residual teardown-vs-reattach race; (4) single-instance
  guard — a 2nd server on a live socket exits instead of stealing it and
  orphaning sessions; (5) `pid_file_path`/persist path now follow
  `YALDA_SESSION_SOCKET` (enables isolated instances + the guard). **Verified:**
  new headless harness `tests/session_resilience_test.rs` drives the REAL server
  binary (no agent needed) — reproduces the storm without the fix; with it, 30
  sequential restarts + in-place reconnect + duplicate-server guard all pass,
  every connection closes (no zombies). **Still owed:** human runtime check that
  the GPUI app reconnects seamlessly after the server reader thread sees EOF
  (GPUI can't be driven headlessly). Was suspected to be the attach-replay
  broadcast lag — that path was already self-healing; the real cause was the
  missing shutdown.

## Session-server hardening + actor extraction (all MERGED to `master`)

Phase-3 + phase-7 of `spec-session-server-actor.md`. All landed on `master`
(`bd796d4`,`1e2c881`,`03e8d10`,`23747a0`,`a70ef74`); branches deleted. Headlessly
verified via the resilience+transcript harness. Worklogs:
`2026-06-07-session-server-hardening.md`, `2026-06-07-actor-extraction-and-perm-ux.md`.

- **Permission default + `0600` socket** — `DONE` (ADR-0014 + addendum). Now
  **Yolo, config-driven** (`default-permission-mode` in config.kdl; server loads
  config in `create_session`); `DEFAULT_PERMISSION_MODE` is the no-config
  fallback. Socket `0600` (TOCTOU-closed via `umask`-around-`bind`). Owner-gated
  escalation. Runtime-confirmed by user.
- **Permission mode visible + cyclable in the server model** — `DONE` /
  runtime-confirmed. `AgentState.permission_mode` from `SessionInfo`; status-strip
  badge always renders; `<space> c m` cycles via the `SetPermissionMode` wire verb.
- **Structured tracing + `admin_status` verb** — `DONE`. Runtime-confirmed.
- **Actor extraction (phase 3, ADR-0012)** — `DONE` (`23747a0`). `Mutex<HashMap>`
  → single `run_manager` task (mpsc `Command` + oneshot); lock-free watch-based
  forwarder; pump owns the channel + forwards generation-stamped Commands.
  Behavior-preserving (conn_id ownership kept, no wire change); harness green 5×,
  test files unchanged, two adversarial reviews SOLID. Kills the shared-mutex race
  class + poison-tolerant lock.
- **Slow-subscriber disconnect** — `DONE` (`a70ef74`). All server→client writes
  bounded by a timeout (60s default; `YALDA_SLOW_SUB_TIMEOUT_MS`); stuck peer
  dropped → reconnects + replays; owner never gapped.
- **Headless start-work verb** — `DONE` (`f3585b0`, ADR-0015). `Request::AdminPrompt`
  + `yalda-session-server prompt <sid> <text>` CLI + `SessionServerClient::
  {admin_prompt,connect_existing}`; ungated `enqueue_prompt` core shared with the
  owner-gated `do_prompt`. Headless prompt takes no lease; WAL-durable; runs under
  the session's stored permission mode. Test: `admin_prompt_drives_turn_without_owner`.
- **Cursor reconnect (phase 5, additive)** — `DONE` (`a3650a4`). Optional
  `cursor:(generation,index)` on `Request::Attach`; forwarder tails `[index..]` on
  generation-match+in-range, else full replay (additive; GUI untouched). Test:
  `cursor_reconnect_streams_only_tail`. **GUI cursor-wiring is NEEDS-RUNTIME** (have
  the GUI send its last cursor on reconnect; the transcript reconciler must be
  checked under tail-only streams — GPUI not headless-drivable).
- **Lease ownership (phase 4)** — `DONE` — runtime-verified, merging to master
  (branch `phase4-lease` → `ba12d5d`, 2026-06-08). `owner: conn_id` → `Lease{
  client_id, expires_at: Instant}` + 5s client heartbeat / 15s TTL; dual-clock
  (actor owns monotonic `Instant`, wire carries display-only millis); stable
  per-install `client_id` (`~/.cache/yalda/client_id`, `YALDA_CLIENT_ID` override
  for blue-green candidates); `attach_owner_with_retry`→`attach_for_role`
  (deterministic same-`client_id` reclaim, retry/observer-fallback retired); wire
  `OwnerChanged→LeaseChanged`; WAL 1→2 with **discard** of v1. STAGED, not bundled
  with the eventlog collapse. **Verification:** workflow `wf_c45c440b-aac` (build +
  15/8 headless) → race review found 2 BLOCKING client races (owner-gap after
  promote; observer heartbeat steal/churn) → fixed (unconditional beater +
  per-tick `is_driver` self-gate; `is_driver` persisted on `AgentSlot`) → indep.
  re-review `MINOR`, both closed, found a leaked-beater → fixed (singleton
  `_lease_heartbeat` Task). Final: build clean, **17 + 8 headless pass**. **Runtime-verified
  (2026-06-08):** clean v2 daemon spawn + live v1-WAL discard; heartbeats accepted
  (no `bad frame`); idle-then-prompt holds the lease (no false expiry); `:promote`
  self-hosting handoff textbook (candidate observer-attach → original close →
  candidate promote → drives past >15s — the bug-1 owner-gap fix, confirmed in-app
  via daemon log + user drive). **Known limitation (App Nap):** two windows of the
  *same* install on one Mac — the backgrounded owner's heartbeat (collect step on
  GPUI's foreground executor) is throttled by macOS App Nap, so its lease lapses
  (~15s) and ownership follows focus. Fails safe (no double-drive / corruption).
  Acceptable per user — the self-hosting / blue-green loop is the real case and
  works; same-machine multi-window is the edge. Follow-up only if that matters
  (heartbeat off an App-Nap-immune timer / disable App Nap / longer TTL).
- **`AgentTransport` seam (phase 6)** — `DONE` / merging to master (branch
  `phase6-transport` → `b0375e9` + `1f80296`, 2026-06-08). `AgentTransport` trait
  (object-safe, sync, pump-facing) + `AgentSpawner` factory + `RealAgentSpawner`;
  `FakeTransport`/`FakeAgentControls`/`FakeAgentSpawner` in-process fake (gated
  `feature = "test-support"`); new `tests/agent_transport_fake_test.rs`. Real
  subprocess path byte-identical; crash/WAL/socket/back-pressure tests kept
  subprocess-backed. Workflow `wf_6ead8955-d04` → review `MINOR` (fake's
  `complete_turn` wrongly emitted a `TurnEnded` record the default worker doesn't)
  → fixed: `complete_turn` is counter-only, opt-in `emit_turn_ended_event` covers
  `YALDA_EMIT_TURN_ENDED=1`. Build + full suite + 8/8 fake tests green.
  Behavior-preserving → foldable after build-check. Overlaps
  `tests/session_resilience_test.rs` with phase 4 at integrate (kept additive).
  Unblocks the phase-8 eventlog reducer/forwarder headless tests.
- **GUI projection + full eventlog end-to-end (phase 8)** — `MERGED` to master
  (`f0710fc`, 2026-06-08; v3 WAL cutover landed). Post-merge runtime confirms still
  owed (see end of entry) but non-blocking — headless + reviews are green.
  Producer collapse `Notification::{ReplyEvent,TurnEnded,UserPrompt}` +
  `WorkerEvent::Reply` → one `AgentEvent` (`src/agent_event.rs`, byte-preserving
  `Unknown{tag,raw}`); emit chokepoint (worker stamps gen/turn, server `record()`
  assigns durable seq); generation-on-`ChannelOpened`; WAL 2→3 **discard**;
  **ringbuffer compaction** (`log_base` logical offset, §6 epoch predicate wired
  into phase-5 cursor seq-space, `CompactedSummary` trim marker, on-disk WAL
  append-only); GUI **total reducer** over `AgentEventKind` + idempotent finalize,
  **additive** per §9 (old inference kept behind a gate — deleting it is a
  post-soak follow-up). **Verification:** workflow `wf_73656668-97f` (build + all
  suites green: new `event_log`/`agent_event_stream`/`agent_reducer_*`/ringbuffer
  tests) → adversarial review `BLOCKING` (live forwarder gapped the owner across a
  trim; marker prepend shifted seq +1; finalize ledger keyed 0-vs-1-based so dedup
  never fired) → **fixed** (log_base-aware live forwarder + owner hard-ceiling;
  prepend decrements `log_base`; aligned finalize keys) with fail-before/pass-after
  tests → re-review `SOLID`, found a MAJOR (no high-water bound → an App-Napped
  owner pins in-memory growth) → **fixed** (spec §6 disconnect-before-gap:
  `enforce_high_water` evicts the slowest forwarder — owner included, lease-safe —
  before the trim) → eviction race self-checked clean (immutable `LogSnapshot` +
  evicted-check-first). Final: build clean, **full `--features test-support` suite
  green**. **Runtime check (2026-06-08, isolated v3 sandbox):** replay idempotency
  passed (a daemon-bounce full replay caused NO visible re-render — the reducer
  refolded identical state); WAL v3 reload + re-adopt clean. **Found + FIXED a real
  §9 bug:** a live prompt AFTER a resume stuck in "thinking" forever — `ReplayEnd`'s
  server-stamped envelope `turn` (`self.turns`) aliases the next live turn's
  finalize key (`completed_turn = turns-1`), so routing `ReplayEnd` through the
  per-turn idempotency ledger pre-occupied the live turn's `(gen,turn)` slot →
  live `TurnEnded` no-op'd finalize → `turn_phase` never returned to `Idle`. Fix
  (`e19b9d7`): `ReplayEnd` is a replay-PREFIX marker, routed through a one-shot
  `replay_prefix_finalized` (re-armed on `reset_for_replay`), never taking a
  per-turn slot. Reproduced + fixed headlessly (verify_harness), independently
  re-reviewed `SOLID` (multi-resume re-arm + no-other-aliasing-pair + no §9
  regression confirmed). **Post-merge confirms (NEEDS-RUNTIME, non-blocking):** (1) one-shot GPUI paint confirm —
  the now-`Idle` spinner visibly clears on screen (fold/`turn_phase` proven correct
  headlessly; only the paint is unverifiable without a GPUI run); (2) App-Nap-paused-owner
  high-water eviction → clean reconnect + lease reclaim in the live app; (3) the
  merge is the **v2→v3 WAL cutover** (discards v2 sessions — do at a quiet moment).
  **Deferred follow-ups:** delete the §9 gated old-inference after real-session
  soak; latent `event.seq` vs `seq_of` divergence (only bites when phase-5 cursor
  is wired client-side — commented).
- **GUI stale-session robustness** — `DONE` (`b0f1eb2`) / NEEDS-RUNTIME. GUI drops
  the slot + scrubs the persisted id (by id, across all cwd keys) on a permanent
  `no such session` attach error; transient errors keep the recoverable status.
  Compile-verified; runtime check owed (silent drop, no recur next launch,
  transient survives, last-slot restores underlying, multi-tab/tile).
- **In-app rebuild + reconnect-badge** — `NEEDS-RUNTIME`. `dev_rebuild_restart_gui`
  (`<space> c g`) and the permission badge after a sid-only reconnect (shows
  default until re-synced) need a human runtime check.

## Top priority

- **State-first architecture overhaul** — `PHASE-A-DONE / PHASE-B-GATED`
  (updated 2026-06-08). Root-cause fix for the constant-regression class (30% of
  state is hand-synced caches/copies). Full state→owner map (162 items),
  20-module state-first decomposition, 6 gating decisions, and a phased plan in
  `docs/specs/spec-state-architecture.md` (+ Appendix A inventory).
  - **Phase A (pure extractions) — essentially complete.** Landed: `replay_turns`
    field-ownership (`6168157`), `overlay` 5-Options→`ActiveOverlay` enum
    (`e5be921`), `settings`/text-zoom persist (`e66a54c`), canonical cwd key
    (`c46f023`), `tool_calls`→owner (`f10486e`), `agent_view_model`→owner
    (`9253139`), additive `TurnEnded{generation}` (`8cdbdd1`), server `record()`
    fusion + `apply_channel_state` unify (`74c4f73`), `InputSurface` enum
    (`761dfe6`), dead-code removal (`15fe390`), `reset_for_replay` delegation
    (`eca7759`). Deferred-on-purpose: `buffer_pool` (5a, folded into D2) and
    `DocState` auto-derive (5b, memo half already done).
  - **Decisions D1–D6 — written.** ADRs 0006–0011 cover them (0007 doc/edit rope
    = D2, 0008 reconnect semantics = D3, 0009 durability = D4, 0010 cwd = D5,
    0006/0011 turn-end + crate boundary ≈ D1/D6). No longer a blocker on the user.
  - **Stop-the-bleeding — done:** CI gate ✅, keymap extraction + headless action
    smokes ✅, worksheet double-render ✅, `clippy -D warnings` + `fmt --check`
    quality CI gate ✅ (2026-06-08).
  - **Phase B (behavior-changing, GPUI-runtime-gated) — HELD, by design.** Not
    blocked on a decision; blocked on the **verification harness** (GPUI can't be
    driven headlessly) and on stabilizing the active reconnect path. Status
    (updated 2026-06-08):
    - `5c` Doc/Edit single pooled rope — ✅ **LANDED**. The foundation was already
      live (`DocState.source`/`DocSource`/`SharedEditor`/`open_and_retain` dedup +
      `refresh_blocks`); open/split/restore bind the pooled core, so Doc+Edit and
      splits share a rope with unified undo. Final fix: theme-switch re-render
      (`re_render_one_doc`) sources the live core instead of disk (was silently
      reverting unsaved edits). Headless tests added (pool sharing + unified undo
      + live-core re-render). ⚠️ cross-tile *paint* owes a GPUI eyeball.
    - `8b` delete turn-end inference — ⏸️ **architectural goal already met by the
      phase-8 `AgentEvent` stream** (sourced-once + total reducer + exactly-once
      ledger; agreement pinned by `agent_stream_agrees_*`). The remaining legacy-
      inference deletion is the content-application cutover (double-render risk the
      §9 gate prevents) and would inject `TurnEnded` into the durable WAL — runtime
      +soak-gated, **held by design**, not by an open decision.
    - `10` reconnect — ✅ **decided ADR-0008 scope DONE** (re-attach failures
      surfaced via `spawn_attach_sessions`). The `Arc<Core>` swap-in-place is an
      explicit **ADR-0008 deferral** (HIGH risk, rare path, trigger not fired) —
      a recorded non-goal, not unfinished work.
    - `ChannelAttachState` faithful enum — still held (refactors the live
      reconnect path; stabilize that first).

- **CI gate** — `DONE` (2026-06-08). Minimal `build --bins + test` on push/PR
  (`.github/workflows/ci.yml`) landed; the `quality` job (`clippy -D warnings` +
  `fmt --all --check`) is now enabled too — the whole tree is clippy-clean and
  fmt-clean. Turns the human from the only oracle into the fallback.

- **Verification harness** — `PARTIAL`. The original premise ("agents can't
  drive the GPUI app") is **stale**: `verify_harness.rs` (~40 `#[gpui::test]`s)
  drives the real view headlessly — constructs it, presses real keys, streams
  events through the real reducer, asserts state. The scripted-input driver is
  done. Three gaps remain, in leverage order: (1) **full GUI↔server↔agent loop
  in one process** — wire the GUI's real `SessionServerClient` to an in-process
  fake server+agent (server-side fakes already exist); retires the most
  `NEEDS-RUNTIME` flags. (2) **golden render output** — snapshot the element
  tree / layout bounds from `run_until_parked` for the pixels/geometry class.
  (3) **wall-clock perf gate** — `--release` criterion bench at realistic
  transcript size (render-count proxy is already in CI). See
  `docs/dev-system.md` § Verification harness. `NEEDS-RUNTIME` items below now
  mean "owes a pixels/timing eyeball," not "untestable."

## State (2026-06-02)

`master` fast-forwarded `f282130` → `8036ccf` (= `integration`): base ACP + rail
+ perf + workspaces are now on `master`. Rail is **runtime-confirmed by the
user**; the rest is `NEEDS-RUNTIME`. Follow-ups below are off `integration`,
**not yet folded**.

## Follow-ups (branches off `integration`)

- **`ff-buffer-pool`** — `DONE` (folded into 5c, 2026-06-08). The buffer pool is
  wired into the live app: `open_and_retain` dedups by canonical path and
  `gc_buffers` (strong-count liveness) backs every file-backed view, so docs are
  shared by reference across views (Doc/Edit/splits of one file share a rope +
  unified undo). See ADR-0005 / ADR-0007 and spec §6 step 5c. ⚠️ cross-tile paint
  owes a GPUI runtime eyeball.
- **`ff-ui-threading`** — `DONE` (`c7b138f`). Move `open_agent`/`attach`/`close`
  socket round-trips off the paint thread (tachyon S4); open is now instant.
  Removes the last ~30s freeze path. Behavior-changing → **runtime review before fold**.
- **`ff-editor-perf`** — `DONE` (`42b4507`). Delta-based undo (refactor #4) + O(1)
  LLM insertion-point cache (#9), +10 tests. Behavior-preserving → foldable after build check.
- **`ff-server-perf`** — `DONE` (`7a352ea`). `Arc` event_log snapshots (#6).
  Behavior-preserving → foldable. `#7` (lock sharding) deferred below.

## Ready

- **Fold the perf/cleanup follow-ups into `integration`** after build-check:
  `ff-server-perf` (done), `ff-editor-perf` (when done). Hold the behavior-
  changing ones (`ff-buffer-pool`, `ff-ui-threading`) for runtime review.
- **Retarget `/refactor` to yalda** — `READY`. Its `workflow.js` PHILOSOPHY
  preamble is Fulcrum-specific (Python/PyO3/pytest/EARS). Replace with a Rust /
  GPUI philosophy (Result-typed errors, newtypes for invariants, `#[test]` /
  `debug_assert!` as enforcement hooks, no migration framing).

## Deferred (with reason)

- **`/refactor` net-new findings not yet taken** — see
  `docs/research/refactor-review-perf-hot-path.md`. `#4`/`#9` are being done in
  `ff-editor-perf`; `#6` in `ff-server-perf`. Remaining: nothing critical.
- **Server lock sharding / forwarder-consumes-broadcast (refactor #7)** —
  `DEFERRED` (needs-human). The "event_log is the single source of truth,
  broadcast is only a wake signal" design is load-bearing (fixes `Lagged`
  merge artifacts). Changing it risks ordering/dup regressions. `#6` already
  removed the dominant cost (whole-log clone). Revisit only if profiling shows
  the per-event global lock is a real bottleneck under many sessions.
- **event_log compaction/capping** — `DEFERRED`. Interacts with the resumable-
  tail `sent`-index replay protocol; risky. `Arc` snapshots (`#6`) bought the
  cheap win without it.
- **Tachyon R1/R2 (speculative pre-tokenize, frame-budgeted replay)** —
  `DEFERRED`. Marginal after the memoization (S1) landed; measure before building.
- **tool_calls deep-clone per frame → `Rc<HashMap>`** — `DEFERRED`. Touches
  ~5 mutation sites with `Rc::make_mut`; outside the memoized boundary, so
  orthogonal. Low risk but not yet worth the churn.

## Worksheet frozen-block model (branch `main`, this session)

- **Worksheet insert/render fixes** — `NEEDS-RUNTIME`. Five fixes from a
  4-personality subagent sweep (`docs/projects/worksheet-frozen-blocks/`):
  (1) atomic structural blocks (code/table) can no longer be split by an insert —
  the "butchers Claude text" bug, guarded in `can_insert_char_at` via a new
  `EditorCore::atomic_blocks` seeded from the render-time block detector;
  (2) blank lines are no longer frozen as empty "You" turns on submit;
  (3) the phantom "You" header scan is bounded to the current editable run;
  (4) each frozen prose line is its own nav stop (insert between any two);
  (5) `snap_nav_stop` no longer strands the caret on a block-interior line.
  Builds + 217 gpui tests + full suite green; needs human runtime check (GPUI
  can't run headless).
- **Worksheet deep bugs (deferred)** — `READY`. `001-ticket-deferred-deep-bugs`:
  streaming cursor-drift (cursor not shifted on `programmatic_insert`),
  floor-only-EOF (`agent_tail_floor_char` misses mid-transcript drafts), undo
  wipes `TurnId` metadata, `view_model_fingerprint` excludes cursor/content.
  Real, higher-scope, NOT the reported repro — each needs runtime repro + a
  separately-tested fix.

## Needs decision (you)

- **Workspaces multi-membership for agents** — `NEEDS-DECISION`. Needs the
  multi-subscriber session core/view split (see `spec-workspaces-tagging.md`).
  Bigger lift; confirm it's wanted before building.
- **Merge order to `master`/`main`** — `NEEDS-DECISION`. `integration` is the
  combined buildable branch; none of it is runtime-verified yet.

## Needs runtime verification

- **System console self-relaunch (`feature/system-console`)** — run `r` and `R`
  from the console; confirm live Cargo streaming, process replacement with the
  console reopened, and agent-session reattachment.

All 2026-06-02 branches: `rail-fixes` (placement/contrast/chords), `perf` /
`perf-tachyon` (feels-fast + tokens/tool-expand/thinking-indicator correct),
`workspaces` (Ctrl-W m/M chords, dot, focus after move), `integration` (all of
the above together).
