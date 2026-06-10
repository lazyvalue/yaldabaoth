# Tiles and Apps — Content Model

**Status:** ACTIVE
**Last updated:** 2026-06-10
**Owner:** content model (`WindowContent` → `App`)

## Builds On

- **`docs/decisions/0019-tiles-contain-apps.md`** — the committed decision this
  spec implements. WHY: it fixes the vocabulary and the two-level model (Tile →
  App → Buffer|Agent) and the Cmd+O scoping; HOW: this spec turns that decision
  into the target types, the migration mapping, and the call-site contract.
- **`docs/decisions/0005-shared-content-pool.md`** /
  **`docs/decisions/0007-doc-edit-shared-rope.md`** — Doc and Edit are already
  two views of one pooled `SharedCore`. WHY: that pooling is what makes
  `Viewing ⇄ Editing` a *mode toggle* rather than a content swap; HOW: this spec
  leaves the pool untouched and only re-nests the view states under `BufferApp`.
- **`docs/specs/spec-agent-window.md`** — defines `AgentRing` / `AgentSlot` /
  `AgentState`. WHY: `Agent` is the second App variant; HOW: this spec leaves the
  ring's internals alone and only retypes its `underlying` stash.
- **`docs/specs/spec-menu-scopes.md`** — Tile-scope vs workspace-scope command
  dispatch. WHY: Cmd+O moves from a global action to a Buffer-app-scoped one;
  HOW: this spec specifies the scoping and the inert-on-Agent behavior.

## Overview

A **Tile** (`Window<App>`, code name `Window`) holds exactly one **App**. There
are two App kinds:

- **`App::Buffer(BufferApp)`** — a view onto the file-buffer pool. It is always
  in exactly one **`BufferMode`**: `Picking` (file/buffer browser, no file chosen
  yet), `Viewing` (rendered markdown), or `Editing` (raw markdown). `Viewing ⇄
  Editing` toggle over the same pooled `SharedCore`.
- **`App::Agent(AgentRing)`** — the ACP multi-session ring, unchanged in
  substance (`spec-agent-window.md`).

This replaces the current flat four-variant content enum
`WindowContent { Doc, Edit, Agent, Browser }` (main.rs:1109). The four variants
re-nest into two levels; **Browser stops being a peer content type** and becomes
`BufferMode::Picking`. The split tree (`Workspace<C>`, `Tab`, `Layout`,
`Window`) is generic over the content type and is **unchanged** except that the
type parameter `C` goes from `WindowContent` to `App`.

Named entities introduced here and referenced below: **`App`**, **`BufferApp`**,
**`BufferMode`** (`Picking` / `Viewing` / `Editing`), and the existing
**`BrowserWindow`** / **`DocState`** / **`EditState`** / **`AgentRing`** payloads.

## Behaviors

- **B1. Buffer is always in one mode. [DRAFT]** A `BufferApp` is `Picking`,
  `Viewing`, or `Editing` — never none, never two. A newly created Buffer tile
  opens in `Picking`.

- **B2. Cmd+O is Buffer-app-scoped. [DRAFT]** `open-browser` (Cmd+O) means
  "active Buffer app: show your picker." On a `Buffer` tile it transitions the
  `BufferApp` to `Picking`, stashing the prior mode for restore (B4). On an
  `Agent` tile it is **inert** — a transient status hint ("no buffer here"), no
  tile mutation. This is the spec's enforcement point for *browser-over-Agent
  removal*: the picker is a state of the Buffer app, not something laid over an
  arbitrary tile.

- **B3. New Buffer tile vs in-place pick. [DRAFT]** Two distinct commands:
  - `new-buffer-tile` (formerly `new-browser-tile`) splits/creates a **new**
    Tile holding `App::Buffer` in `Picking` with no restore target.
  - `inplace-buffer-pick` (formerly `inplace-browser-tile`) and Cmd+O (B2) reset
    the **focused** Buffer app to `Picking` with the prior mode as restore.

- **B4. Picker outcomes. [DRAFT]** From `Picking`:
  - *Pick a file* → `Viewing(DocState)` (or `Editing`) bound to the pooled core
    for that path. Any stashed restore target is discarded (the user chose a new
    file).
  - *Cancel (Esc)* with a restore target → return to the stashed `Viewing` /
    `Editing`. *Cancel* with no restore target (a fresh `new-buffer-tile`) →
    close the Tile, **inheriting the existing sole-tile floor**: closing the only
    Tile in the only Tab is a no-op, not an app quit (`browser_close`,
    browser_ui.rs:141-157). The rewrite must preserve that guard.

- **B5. Viewing ⇄ Editing toggle. [DRAFT]** Unchanged in feel: the existing
  enter-edit / back-to-rendered toggle now flips `BufferMode` between `Viewing`
  and `Editing` over the same `SharedCore`. No stash, no content swap (per
  ADR-0007).

- **B6. Agent over Buffer; back to Buffer. [DRAFT]** Opening an Agent (Ctrl-K)
  over a Buffer tile stashes that `BufferApp` in `AgentRing.underlying`; the
  agent's back-to-buffer path (Ctrl-V) restores it. When there is **no** stash
  (a standalone agent tile, or a session that closed with nothing behind it),
  back-to-buffer and session-close fall back to a **fresh `BufferApp::Picking`**
  — they never close the tile (a closed session must leave a usable buffer
  behind, not vanish). This mirrors today's "fall back to a fresh Browser"
  behavior in `back_to_doc` (edit_ui.rs:200-205) and `reconcile_session_closed`
  (agent_ui.rs:1406-1410), and is **distinct from B4's close-on-cancel** — the
  Agent side restores/replaces, it does not close. Because the stash is typed
  `BufferApp` (D3), an Agent can only ever be backed by a Buffer, never by
  another Agent.

- **B7. Persisted layout falls back cleanly via the existing discard. [DRAFT]**
  The persisted layout (`workspace.json`) stores a **flat** per-tile kind tag
  with no nesting (`PersistedKind`, persist.rs:366-383); `underlying` stashes are
  never persisted, so no on-disk layout can encode an Agent-behind-picker. The
  tag set changes (`{doc, edit, browser, claude}` → `{buffer{mode}, agent}`).
  **No version field is added** — the existing load path already discards an
  entry that fails to deserialize (`serde_json::from_value(...).ok()`,
  persist.rs:794) and falls back to the default workspace, so a stale
  `workspace.json` from an older build silently re-opens at defaults. Durable
  agent sessions (WAL / ACP session list, ADR-0009/0018) are keyed independently
  and are **not** affected — only the remembered tile arrangement is lost on
  first run of the new build.

## Data Model

**D1. `App` (replaces `WindowContent`).** [DRAFT]
```rust
enum App {
    Buffer(BufferApp),
    Agent(AgentRing),
}
```
`Workspace<App>`, `Tab<App>`, `Layout<App>`, `Window<App>`, `Box<App>` — the
generic parameter is renamed throughout; the tree code is otherwise untouched.

**D2. `BufferApp` / `BufferMode`.** [DRAFT]
```rust
enum BufferApp {
    Picking(BrowserWindow),   // restore target lives in BrowserWindow (D3)
    Viewing(DocState),
    Editing(EditState),
}
```
`BufferMode` is the three-way tag; `BufferApp` carries the per-mode payload.
`DocState` and `EditState` are unchanged (they already own their pooled core
binding). The `Picking` payload is the existing `BrowserWindow`.
`BufferApp::Viewing` **must tolerate a source-less `DocState`** (`source:
None`): transient placeholder Docs are constructed and immediately swapped today
(`open_agent_inner` agent_ui.rs:34, `back_to_doc` edit_ui.rs:161), and the
restructure preserves that swap-dance rather than forbidding the transient.

**D3. Retyped stashes.** [DRAFT] Both `underlying` fields narrow from
`Option<Box<WindowContent>>` to `Option<Box<BufferApp>>`:
- `BrowserWindow.underlying: Option<Box<BufferApp>>` — the mode to restore on
  cancel (B4). Invariant: never `Picking`.
- `AgentRing.underlying: Option<Box<BufferApp>>` — the Buffer to restore on
  back-to-buffer (B6).

The narrowing from `WindowContent` to `BufferApp` is what makes
browser-over-Agent and agent-over-agent *unrepresentable* rather than merely
discouraged.

**D4. Migration mapping (four flat variants → two levels).** [DRAFT]

| old `WindowContent` | new `App` |
|---|---|
| `Doc(DocState)` | `Buffer(BufferApp::Viewing(DocState))` |
| `Edit(EditState)` | `Buffer(BufferApp::Editing(EditState))` |
| `Browser(BrowserWindow)` | `Buffer(BufferApp::Picking(BrowserWindow))` |
| `Agent(AgentRing)` | `Agent(AgentRing)` |

Every `match` on the old enum (≈61 match arms, ~126 `WindowContent::` sites
across 8 files; main.rs holds ~57) becomes a two-level match: outer on
`App::{Buffer, Agent}`, inner on `BufferMode` where the old code distinguished
Doc/Edit/Browser. Sites that only cared "is this an Agent?" simplify; sites that
touched all of Doc/Edit/Browser gain one level of nesting.

Non-`match` reach-through sites adapt too: the theme-restyle pass that reaches a
stashed Doc via `Browser.underlying` (`re_render_layout_docs`,
render_blocks.rs:1412-1417) becomes `BufferApp::Picking(b).underlying →
Viewing/Editing`. A missed site here re-introduces the stale-themed-blocks bug
that pass exists to prevent.

## Interfaces

Accessor helpers on `SketchGpuiView` adapt to the nesting (names indicative):

- **`buffer_mut() -> Option<&mut BufferApp>`** / **`agent_mut() -> Option<&mut
  AgentRing>`** — replace ad-hoc `doc_mut()` / matches that reached a single old
  variant. `doc_mut()` / `edit_mut()` become thin wrappers that match
  `Buffer(Viewing(_))` / `Buffer(Editing(_))`. [DRAFT]
- **`set_buffer_mode(BufferMode)` / picker transitions** — the focused Buffer
  app's `Picking ↔ Viewing ↔ Editing` transitions (B2–B5) flow through one place
  rather than open-coded `replace_focused_content` calls. [DRAFT]
- **Persistence (`persist.rs`)** — the serialized content tag set changes from
  `{doc, edit, agent, browser}` to `{buffer{mode}, agent}`; a workspace-layout
  schema version gates discard-on-mismatch (B7). External contract: none beyond
  the on-disk `workspace.json` shape. [DRAFT]
- **Commands** — `new-browser-tile` → `new-buffer-tile`, `inplace-browser-tile`
  → `inplace-buffer-pick`; `open-browser` (Cmd+O) gains Buffer-scope semantics
  (B2). `new-agent-tile` / `inplace-agent-tile` unchanged in name. [DRAFT]

## State Machine

`BufferApp` mode transitions (one Buffer tile):

```
        new-buffer-tile / Cmd+O(B2)
   ┌──────────────► Picking ◄──────────────┐
   │                 │  │                   │ Cmd+O (stash mode)
   │   pick file     │  │  cancel + restore │
   │                 ▼  └───────────────────┤
   │              Viewing ◄────────────────► Editing
   │                 (enter-edit / back-to-rendered, B5)
   └─ cancel + no restore ⇒ close Tile
```

`Picking` with no restore target and a cancel closes the Tile (B4); with a
restore target it returns to the stashed `Viewing`/`Editing`.

## Constraints

- **C1.** The split tree (`workspace.rs`) must not gain App-kind knowledge — it
  stays generic over the content type. Only the type parameter name changes.
- **C2.** `Viewing ⇄ Editing` must remain a zero-copy mode flip over the shared
  pooled core (ADR-0007); the restructure must not reintroduce a stash or
  re-parse on that toggle.
- **C3.** No new behavior beyond the decision: this is a re-nesting plus the
  Cmd+O scoping and browser-over-Agent removal. Agent internals, the buffer
  pool, rendering, and the desktop/split layout are out of scope.
- **C4.** Restore stashes are typed `BufferApp`, never `App` — the type system,
  not a runtime check, forbids backing an Agent with a picker or an agent.

## Revision History

- 2026-06-10 — Initial DRAFT. Implements ADR-0019's content-model restructure:
  `WindowContent → App`, `Browser` folded into `BufferMode::Picking`, Cmd+O
  Buffer-scoped, stashes narrowed to `BufferApp`. Migration mapping and
  ~126-site impact captured for the implementation pass.
- 2026-06-10 — **Implemented and ACTIVE.** Landed in two commits (mechanical
  re-nest + behavioral pass): `WindowContent → App`, 125 sites migrated, stashes
  narrowed to `BufferApp`, Cmd+O Buffer-scoped, no-stash fallbacks, persist tag
  shape changed (no version field). Both bins build clean; 136+64 bin/lib tests
  pass (the 2 pre-existing `snapshot_test` failures are unrelated). Human runtime
  smoke passed (Cmd+O scoping, agent-inert, back-to-buffer, session-close
  fallback, Doc↔Edit toggle). All 7 behaviors verified at file:line.
- 2026-06-10 — Adversarial-review pass (verdict REVISE, all items folded in):
  B7 corrected to describe the existing `serde_json::...ok()` discard rather
  than a nonexistent version field; B6 now covers the Agent-side no-stash
  fallback to a fresh `Picking` (distinct from B4's close-on-cancel); B4 carries
  the sole-tile close floor; D2 notes source-less `Viewing` tolerance; D4 calls
  out the `re_render_layout_docs` theme-restyle reach-through. Verified against
  code: ~125 sites / `underlying` never persisted / Agent-behind-Browser is a
  real in-memory path (screens.rs:2160) the restructure removes.
