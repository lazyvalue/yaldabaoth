# ADR-0019: Tiles contain Apps; Apps are Buffer or Agent

**Status:** Accepted
**Date:** 2026-06-10
**Related:** ADR-0002 (workspaces model), ADR-0005 (shared content pool),
ADR-0007 (doc/edit shared rope), `spec-tabs-and-splits.md`,
`spec-desktop-mode.md`, `spec-menu-scopes.md`, `docs/UX.md`

## Context

The GPUI workspace is a tree of split leaves, each holding one of four
content variants:

```rust
enum WindowContent { Doc, Edit, Agent, Browser }
```

Two problems with how this is named and modeled:

**1. "Pane" is three colliding concepts.** The codebase uses "pane" for the
split-tree leaf (~160 refs: `MovePane`, "close pane", `new-agent-pane`), but
also "Panel" for desktop-mode grid cells (~94 refs: `slot_origin`, drag state,
`spec-desktop-mode.md`) and "Sidepane" for the Tasklist/Subagents chrome
(~12 refs: `pane_bg`, `pane_border`, `pane_header`). Three words, blurred
boundaries, and "Panel" even shares a prefix with "Pane" so naive renames
corrupt it.

**2. The four content variants misrepresent the architecture.** The code
already groups them two-and-two underneath the naming:

- **Doc and Edit are already two views of one pooled buffer core** (ADR-0005,
  ADR-0007). They bind to the same `SharedCore` via the `FileBuffer` pool;
  an edit shows live in the doc view, undo is unified, nothing is stashed
  between them. They are one thing with two view modes, named as two.
- **Browser is already a transient overlay, not a peer.** Cmd+O replaces a
  leaf's content in place and stashes the prior content in
  `BrowserWindow.underlying`, restoring it on Esc. It is modeled as a content
  *type* but behaves as a *mode* — and it can currently overlay *any* leaf,
  including an Agent, which is a category error.
- **Agent (`AgentRing`) is the genuinely separate app** — a multi-session ACP
  container with its own lifecycle.

## Decision

**Vocabulary: the container is a Tile.**

- **Pane → Tile.** A Tile is the universal container for one App instance.
  The split tree and the desktop grid are two *layout modes* that arrange
  Tiles; a desktop "Panel" is just a Tile on the grid, so **Panel → Tile**
  too. One word for the container, everywhere.
- **Sidepane → Sidebar.** The Tasklist/Subagents strips are attached chrome,
  not Tiles. Theme fields `pane_bg/pane_border/pane_header` → `sidebar_*`.
- **"tool body pane" → "tool body."** Sub-regions of a rendered tool card are
  not Tiles.

**Model: Tiles contain Apps; an App is a Buffer or an Agent.**

```rust
enum App { Buffer(BufferApp), Agent(AgentRing) }
```

- A **Buffer app** is a view onto a pooled file buffer. Its view mode is one
  of: *picking* (file browser / buffer browser), *viewing* (rendered doc),
  or *editing* (raw markdown) — `viewing ⇄ editing` over the same shared core.
- The **file browser and buffer browser are states of a Buffer app**, not
  peer content types. You don't create a "new file browser"; you create a new
  Buffer app, which opens in its picker state.
- An **Agent app** is the ACP session ring, unchanged in substance.

**Cmd+O is a Buffer-app-scoped command.** It means "active Buffer app: show
your file picker." When an Agent Tile is focused it is out of scope (no
browser-over-Agent). A separate "new Buffer app" command spawns a fresh Tile
in picker state. This removes the ability to stash an Agent behind a Browser
overlay — an intentional simplification.

## Rationale

The reframing names what the architecture already does: Doc/Edit are one
buffer wearing two hats, Browser is that buffer's empty state, Agent is the
other app. Collapsing `WindowContent { Doc, Edit, Agent, Browser }` to
`App { Buffer, Agent }` turns four match arms into two at every content-access
site and deletes the Browser-as-peer category error. The Tile/Sidebar split
gives chrome a name distinct from the container, and unifying Panel into Tile
means the desktop grid and the split tree stop being described in different
words for the same object.

Scoping Cmd+O to the Buffer app makes the picker a property of the thing it
picks *into*, which is why "browser over an Agent" stops being expressible —
the awkward path was a symptom of Browser being a top-level peer.

## Consequences

- **Sequenced, not atomic.** The vocabulary rename (Pane/Panel→Tile,
  Sidepane→Sidebar) lands first as a mostly-mechanical pass so the model
  restructure arrives in already-correct words. The `WindowContent` → `App`
  restructure (folding Browser into BufferApp's picker state) is a separate,
  spec'd change; the split tree itself (`Workspace<C>`) is untouched — only
  the content type `C` and the content-access match arms change.
- **Behavior change:** opening the file browser over an Agent Tile goes away.
  Cmd+O is inert on an Agent Tile; opening a file targets a Buffer Tile.
- Specs carrying the old vocabulary (`spec-desktop-mode.md`,
  `spec-agent-window.md`, `spec-menu-scopes.md`, `spec-agent-presentation.md`,
  `docs/UX.md`) are updated to Tile/Sidebar/App language.
- User-facing menu strings and command ids (`new-agent-pane`,
  `inplace-browser-pane`, "close pane", …) rename to Tile.
