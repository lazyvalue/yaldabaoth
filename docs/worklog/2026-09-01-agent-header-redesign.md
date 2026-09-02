# Worklog: agent-header-redesign

**Date:** 2026-09-01
**Branches touched:** agent-header-redesign (6ad6939, merged to main at e1aa51f, branch deleted)

## Cog execution evidence

- Graph id: `y8m`

### Initial render

```text
graph agent-header-redesign (frontiers)
frontier 0: implement-header [open]
frontier 1: tests-and-spec [open]
frontier 2: verify-and-merge [open]
frontier 3: omega [open] (omega)
```

### Node execution

- `jehl` `implement-header`: claimed → closed; output: header rewritten as two
  decks in `screens.rs::render_agent` (identity deck: haloed activity dot,
  truncating semibold label, quiet model chip, exception-based amber permission
  chip; activity deck: fixed 52px state word, turn/timer with turn-0
  suppression, filled stop chip, compact usage meter with pct + nearly-full
  tokens-left + cost, right-aligned location without CWD label; activity-tinted
  bottom hairline). `cargo check --bin yalda-gpui` clean.
- `uwui` `tests-and-spec`: claimed → closed; output: vocabulary test pins the
  word-only activity copy + exception-based permission labels; status-pill test
  pins `AGENT_ACTIVITY_STATE_WIDTH` (52px); layout test proves identity-deck →
  activity-deck order with meter + location contained on the activity deck; new
  `agent_permission_chip_paints_only_when_restricted` with negative control
  observed RED (always-show reverted) then restored green. UXI-AgentTile-28/31
  reconciled in `docs/components/agent-tile/transcript.md` + README index.
- `migy` `verify-and-merge`: claimed → closed; output: branch committed
  (6ad6939), merged no-ff to main (e1aa51f); `cargo test --bin yalda-gpui` on
  main: 764 passed, 1 pre-existing unrelated failure; worktree removed; release
  rebuild started; running GUI/session-server untouched.
- `o8ew` `omega`: claimed → closed; output: redesign shipped to main with
  reconciled UXIs and green guards + NC evidence.

### Notes

- graph, seq 11, topic `pre-existing-failure`:
  `verify_harness::archived_waiting_session_is_removed_from_the_painted_waiting_tab`
  fails on main (15d780c) before this branch — stale after the jump-panel
  segmented-widget removal (graph `k2z`). Not addressed in `y8m`.

### Final status

- Status: `complete`

```text
graph agent-header-redesign (frontiers)
frontier 0: implement-header [done]
frontier 1: tests-and-spec [done]
frontier 2: verify-and-merge [done]
frontier 3: omega [done] (omega)
```

## Built (with status)

- **Agent tile header redesign** (`screens.rs::render_agent` + helpers) — the
  three heavy bordered rows (~81px) became two compact decks (~45px):
  - Identity deck: live-state **dot** (halo, green ready / orange working),
    truncating session label, quiet model chip (`▾`, still opens the picker,
    probe `agent-model-badge` kept), **exception-based permission chip** — the
    Yolo default renders nothing; `read-only`/`auto-edit`/`ask-each` wear an
    amber chip (`agent_header_permission_label`).
  - Activity deck: fixed-slot colored state word (`working`/`ready`,
    `AGENT_ACTIVITY_STATE_WIDTH` = 52px), `turn N · M:SS` with `turn 0`
    suppressed on virgin sessions, filled `■ stop ⌘.` chip, transient compose
    state, compact context meter (slim bar + percent; `NNk left` joins only at
    ≥85%; `$X.XX` session cost when reported), right-aligned location — `in
    <worktree>` emphasized, else shortened cwd, no `CWD` label.
  - Header bottom hairline tinted with the live state color. Whole clusters
    wrap on narrow tiles; label/location truncate with ellipses.
- Spec reconciled: `UXI-AgentTile-28` (dot + fixed-slot state word) and
  `UXI-AgentTile-31` (two-deck order, exception-based permission chip) in
  `docs/components/agent-tile/transcript.md`; README index row updated.
- Guards: updated `agent_header_uses_compact_activity_and_transient_editor_vocabulary`,
  `agent_tile_paints_a_status_pill_while_working`,
  `agent_usage_paints_on_the_activity_header_line`; new
  `agent_permission_chip_paints_only_when_restricted` (NC observed RED with the
  exception logic reverted, restored green).

## Open / unresolved

- Pre-existing failing test on main:
  `archived_waiting_session_is_removed_from_the_painted_waiting_tab` (stale
  after graph `k2z`'s jump-panel segmented-widget removal) — backlogged.
- Activation: release binary rebuilt; the running `yalda-gpui` was NOT
  restarted (prohibition) — Scott restarts when ready.

## Decisions

- Exception-based permission display (chip only when restricted; the Yolo
  default is silent) — recorded in `UXI-AgentTile-31`'s statement rather than a
  standalone ADR (single-surface presentation choice).

## Verification status

- Headless: 764/765 harness tests green on main post-merge; the 1 failure is
  pre-existing and unrelated (see above). Layout probes cover deck order,
  state-word slot stability, permission-chip presence/absence, meter + location
  containment.
- `NEEDS-RUNTIME` (gap 1, pixels/colors): the actual look — dot halo, hairline
  tint, chip colors, truncation feel at narrow widths — needs Scott's eye after
  restart.
- `scripts/check-cog-worklog.sh docs/worklog/2026-09-01-agent-header-redesign.md` passes.

## Next

- Scott: restart the GUI when convenient (`./dev-gui.sh`; release binary is
  built) and judge the pixels; tweaks welcome.
- Fix or retire the stale archived-waiting-tab test (follow-up to `k2z`).

## Follow-up: polish pass (Cog graph g10, complete)

Scott's runtime verdict on the first pass: "OK but nothing special" — stop
button unwanted; the left/right split alignment reads badly on wide screens.
Changes (commit `f86957f`, merged `19bb51f`):

- **Stop chip removed.** Esc / ⌘. / the space menu remain the stop
  affordances; the fixed-slot state word now says `stopping` (same orange)
  while a requested stop winds down. Slot widened 52→60px to fit.
- **Single left flow.** Both decks drop their `flex_1` spacers — no
  right-aligned cluster; deck 2 reads state · turn/timer · edit status ·
  meter · location as one group.
- Specs reconciled (`UXI-AgentTile-28/31`), vocabulary guard extended
  (`stopping`, and the unreachable `(working=false, stop_requested=true)`
  degrades to `ready`). 764/765 green on main; same pre-existing
  archived-waiting failure only.
