# Workspaces & Tagging (tiling-WM paradigm for the workspace model)

**Status:** RESEARCH
**Last updated:** 2026-06-02
**Builds on:** `spec-tabs-and-splits.md` (workspace / tab / layout-tree model), `spec-rail.md` (per-tab chrome)

## Overview

This doc investigates whether the **tagging tiling-window-manager** paradigm
(dwm, awesome, wmii, xmonad, river) is a good fit for sketch, and how it would
map onto sketch's existing `Workspace<C>` → `Tab<C>` → `Layout<C>` → `WindowId`
model in `src/bin/sketch-gpui/workspace.rs`.

The user's framing:

> DWM / tagging windowed tile managers — this might be a useful paradigm for
> us. Perhaps sketch has different workspaces / desktops, and I tag panels to
> belong to them. Panels could live in multiple workspaces because they have
> multiple tags. Check if that is insane and for prior art.

**Verdict up front:** the proposal is *not* insane — it has solid prior art in
both WMs *and* editors (vim/neovim tabpages, Emacs `perspective.el`). But the
literal "a panel has N tags and appears in N workspaces" form collides with one
hard fact about sketch: a sketch panel is **not** an independent object the way
an X11 window is. The shareable thing in sketch is the *buffer* (`EditorCore` /
`FileBuffer`), already pooled by reference. The *panel* (`Window<C>` leaf in a
`Layout`) is per-tab geometry. The clean design tags the **buffer**, not the
panel, and lets each workspace own its own split-tree of *views* onto the
shared buffer pool — which is exactly what vim tabpages and perspective.el do,
and exactly the seam sketch already has. See **Feasibility verdict** and
**Recommended design**.

## How tagging WMs work

### Tags are not workspaces

The canonical statement, from w0ng's "dwm tags are not workspaces"
([source](https://github.com/w0ng/wongdev.com/blob/master/content/dwm-tags-are-not-workspaces.md)):

> A **tag** is simply a label placed on windows. A window may have one or more
> tags. A **workspace** is the arrangement of windows from one or more tags.

So in dwm there is no first-class "workspace" object at all. There is a set of
tag labels (by default `1`–`9`), each window carries a *bitmask* of tags, and
the "current view" is itself a tag bitmask. A window is visible iff
`window.tags & view.tags != 0`. A "workspace," in the everyday sense, is just
*"the view where bit 3 is set"* — it's a query, not a container.

### The three core operations

dwm (and awesome, which copied the model — "like dwm and wmii, awesome uses
tags instead of workspaces … windows can be assigned to several tags, and
multiple tags can be selected at the same time"
[[awesome wiki / Wikipedia](https://en.wikipedia.org/wiki/Awesome_(window_manager))])
expose three verbs over the tag bitmask:

| Operation | Default binding | Effect on bitmasks |
|---|---|---|
| **View tag** | `MOD+[1-9]` (dwm default) | `view.tags = {n}` — replace the view, show *only* windows tagged `n` |
| **Toggle tag into view** | `MOD+Ctrl+[1-9]` | `view.tags ^= {n}` — show the **union** of the current view *and* tag `n` |
| **Send window to tag** | `MOD+Shift+[1-9]` | `focused.tags = {n}` — move the focused window so it carries only tag `n` |
| **Toggle window's tag** | `MOD+Ctrl+Shift+[1-9]` | `focused.tags ^= {n}` — add/remove tag `n` on the focused window (this is what gives a window *multiple* tags) |

(w0ng rebinds these so `MOD+[1-9]` *toggles into view* rather than replaces;
the defaults above are stock dwm. The exact bindings don't matter — the four
verbs do.)

The two ideas that make this more than "renamed workspaces":

1. **A window can carry multiple tags.** "A single window can have multiple
   tags, thus allowing finer control over your task set" — the same physical
   window shows up in view `2`, view `4`, and view `2|4` without being
   duplicated or moved.

2. **The view is a union of tags.** "dwm allows you to look at two tags at the
   same time … all the windows in tag 2 and 4 appear together on a single
   screen." Selecting `{2,4}` composes the two tag-sets into one transient
   layout.

### Per-tag layout memory

The other property worth lifting: "users are able to dynamically arrange all
windows in different ways and it'll always be remembered, regardless of what
windows or what tag numbers are visible in the current view." dwm remembers the
tile arrangement *per tag-view*, so flipping between views restores each view's
geometry. awesome goes further — "as a dynamic window manager, awesome can
switch between different layouts for each tag" — each tag has its own layout
algorithm (tile / float / max / etc.)
([Wikipedia](https://en.wikipedia.org/wiki/Awesome_(window_manager))).

### The xmonad / river contrast

xmonad and river lean back toward **workspaces** (a window lives on exactly one
workspace; you *move* it between them), but with strong per-workspace layout
state and good multi-monitor handling. A community library, *Charitable*, was
written specifically to bolt xmonad-style tag management onto awesome
([fREW Schmidt](https://blog.afoolishmanifesto.com/posts/introducing-charitable-awesome-xmonad-tag-management/)),
which tells you the two camps are genuinely different ergonomics, not just
spelling. The dividing question is always the same: **can one window be in two
places at once?** dwm/awesome: yes (multi-tag). xmonad/river/GNOME: no (one
workspace).

## Prior art in editors

This is the section that actually decides feasibility, because editors — unlike
WMs — have to answer "is this the *same buffer* or a *clone*?" The WM question
("is it the same window?") is easy because an X11 window is a single OS object;
the editor question is the one sketch has to answer.

### vim / neovim tabpages — the closest match

A neovim **tabpage is not a tab in the browser sense — it is a named window
layout.** "A tabpage holds one or more windows (not buffers) … it works as if
you create a new window on the same buffer." Critically:

- **Buffers are global; windows/tabpages are views onto them.** "A buffer can
  exist in multiple tabpages simultaneously — you can have the same buffer
  displayed in windows across different tabpages."
- **Each tabpage owns its own window layout.** "Most commands work only in the
  current tabpage, including the CTRL-W commands … each tabpage maintains its
  own independent window layout."
- **Stable identity.** "Each tabpage has a unique identifier which will not
  change, even after rearranging tabs."

([Neovim tabpage docs](https://neovim.io/doc/user/tabpage/))

This is *exactly* the factoring sketch already has: a global buffer pool
(`Workspace.file_buffers`, keyed by canonical path) + per-tab geometry trees
(`Tab.layout`). vim simply doesn't expose a *multi-tag* affordance on top — a
buffer "appears in tab N" only because some window in tab N's layout happens to
be viewing it. There is no `buffer.tags` bitmask; membership is derived from
the layout trees. That's a real design choice and it's the cheap one.

### Emacs `perspective.el` — the explicit multi-membership match

`perspective.el` provides "multiple named workspaces (or 'perspectives') …
similar to multiple desktops in window managers like Awesome and XMonad. Each
perspective has its own buffer list and its own window layout"
([perspective-el](https://github.com/nex3/perspective-el)). It is the one piece
of prior art that implements the user's literal proposal:

- **A buffer can belong to multiple perspectives.** `persp-add-buffer` "adds a
  buffer to the current perspective" (additive — it can now be in several);
  `persp-set-buffer` "adds buffer to current perspective and removes it from all
  others" (exclusive). The two commands are precisely dwm's "toggle tag on
  window" vs "send to tag."
- **Shared by reference, not cloned.** "When you use `persp-add-buffer`, you're
  making the same buffer instance available in multiple perspective contexts."
  Edit it in one perspective, it's edited in all — same as vim, same as
  sketch's `EditorCore` pool.
- **Each perspective stores its own window layout** and restores it on switch.

The related packages clarify the design space ([perspective-el README](https://github.com/nex3/perspective-el)):

- **`eyebrowse`** — "supports window layouts but **not** buffer lists." Pure
  geometry workspaces; no membership model.
- **Built-in `tab-bar`** — "maintains window layouts (with optional names)."
  Same: geometry-only named layouts (this is the neovim-tabpage model).
- **`tab-line`** — per-window list of buffers opened in that window. Orthogonal,
  browser-tab style.

Tellingly, the perspective.el authors warn that "using Perspective and Tab Bar
at the same time is not recommended … the tab list is global … likely to cause
confusion." That is a direct warning about the failure mode sketch would walk
into if it stacks a tag layer *on top of* the existing tab strip without
deciding which one owns layout. (See **Mapping**, option (b).)

### VS Code — workspaces ≠ membership; editor groups own layout

VS Code splits the concept cleanly but does *not* do multi-membership:

- A **workspace** (`.code-workspace`) is a project-scope concept: a set of root
  folders + settings, "a clear distinction between 'a window' and 'a
  workspace'" ([Zed migration doc](https://zed.dev/docs/migrate/vs-code),
  [VS Code UI docs](https://code.visualstudio.com/docs/getstarted/userinterface)).
  It is not a layout container and a file does not have "workspace tags."
- **Editor groups** own layout — "you can open as many editors as you like side
  by side … split the active editor with `Ctrl+\`." The same file *can* be open
  in two groups (two editor tabs onto one underlying `TextDocument`/model,
  shared by reference). That's the vim factoring again: shared document, N
  view-tabs.
- There is a long-standing request to support **multiple sets of editor
  groups** ([microsoft/vscode#69968](https://github.com/microsoft/vscode/issues/69968))
  — i.e. swappable named whole-layouts. It has *not* shipped, which is a
  data point on how much demand vs. complexity this carries even in a
  flagship editor.

### Zed — flatter, no swappable layouts, no multi-membership

Zed's hierarchy is **Window → Pane(s) → Item(s)**; a "workspace" is essentially
"the window's contents for a project"
([Zed discussion #9763](https://github.com/zed-industries/zed/discussions/9763)).
"Zed treats every folder as its own project … removes an entire layer of
project-management complexity"
([Zed migration doc](https://zed.dev/docs/migrate/vs-code)). One buffer can be
open in multiple panes (shared model), but Zed deliberately has **no** named
swappable layouts and **no** tag-membership. It's the "don't build this" data
point.

### tmux — the WM-shaped editor-adjacent case

tmux **windows** within a **session** are the cleanest "switch the whole
layout" model in a terminal: a session has many windows, each window has its own
pane layout, and `link-window` can make *the same window appear in multiple
sessions* — genuine multi-membership of a layout object. But a tmux pane wraps a
*process*, not a shared document, so "the same content in two places" is the
same PTY, not a shared editor buffer. It validates the *layout-swapping* UX, not
the buffer-sharing question.

### Prior-art summary

| Tool | Swappable named layouts? | One buffer in many "spaces"? | Shared by ref? | Membership mechanism |
|---|---|---|---|---|
| dwm / awesome | yes (per-tag view) | **yes** (multi-tag) | n/a (X window) | tag bitmask on window |
| xmonad / river | yes (per-workspace) | no (move only) | n/a | one workspace per window |
| **vim/nvim tabpages** | **yes** | **yes** | **yes** | *derived* — a window in the tab views it |
| **Emacs perspective.el** | **yes** | **yes** | **yes** | explicit `persp-add-buffer` (≈ tag) |
| Emacs eyebrowse / tab-bar | yes | n/a (geometry only) | — | none |
| VS Code | no (1 layout/window) | yes (across groups) | yes | derived |
| Zed | no | yes (across panes) | yes | derived |
| tmux | yes (`link-window`) | yes (layout, not buffer) | n/a (PTY) | window in many sessions |

The pattern is overwhelming: **everything that works shares the document by
reference and gives each space its own geometry tree.** The only axis of
disagreement is whether membership is *explicit* (a tag, à la perspective.el) or
*derived* (a space owns geometry, and a buffer is "in" the space because some
view in that geometry points at it, à la vim/VS Code/Zed). Sketch already has
the shared-by-reference half built.

## The user's multi-tag proposal, evaluated

Restating it precisely: *workspaces/desktops exist; I tag **panels** to belong
to them; a panel lives in multiple workspaces because it has multiple tags.*

### Where it's coherent

If "panel" means **buffer** (the file content), this is exactly perspective.el
and it is coherent and proven. `Workspace.file_buffers` is already a
shared-by-reference pool; adding a tag set per buffer and deriving "which
workspace shows it" from that tag set is a small, well-trodden step.

### Where it's sharp — "panel" is the wrong unit to tag

A sketch "panel" as the user sees it is a `Window<C>` leaf — but that leaf is
**three things fused**: (1) a reference to shared content (an `EditorView` →
`FileBufferId` → pooled `EditorCore`), (2) **per-view cursor/scroll/selection
state** (the `EditorView`, `scroll_handle`, `cursor_block`), and (3) **a
position in one specific `Layout` tree** (its slot in `Tab.layout`). Items (1)
is sharable; items (2) and (3) are *not* — they are inherently per-occurrence.

So "the same panel in two workspaces" forces a fork in the road, and every
branch has a cost:

**(a) Same panel = same `Window<C>` leaf node, by reference, in two layout
trees.** This breaks the data model. `Tab.layout` is an *owning* `Layout<C>`
tree (`Vec<(f32, Layout<C>)>`); a leaf has one parent, one weight, one set of
neighbors. A node can't be in two trees with two different weights / two
different neighbor sets / two different split directions without becoming
`Rc<RefCell<…>>` and inventing a per-(node,tree) weight side-table. And cursor
state: is the cursor shared between the two appearances? If yes, scrolling in
workspace A scrolls workspace B — almost never what you want. If no, then it
isn't really "the same panel," it's two panels sharing a buffer — which is
branch (b).

**(b) Same buffer, *different* `Window<C>`/`EditorView` per workspace.** This is
the vim/perspective.el answer and it's the right one. The buffer is shared by
reference (already true); each workspace's layout has its own view leaf with its
own cursor/scroll. "The panel is in two workspaces" becomes "two views onto one
buffer, one in each workspace's layout." Closing the view in workspace A doesn't
touch workspace B; the buffer survives via refcount (already implemented:
`buffer_retain` / `buffer_release`). Sketch's Behaviors 9–11 in
`spec-tabs-and-splits.md` (per-view cursor, shared edit propagation, shared
undo) already specify exactly the cross-view semantics this needs.

**(c) Geometry-only workspaces (eyebrowse model).** Workspaces are named layout
trees you swap between; "membership" is purely derived (a buffer is "in" a
workspace iff a view in that workspace's tree shows it). No buffer-tag field at
all. This is the *cheapest* and is essentially "sketch's existing `Tab` but
relabeled and made swappable."

The sharp edges the user intuited, made concrete:

- **Focus:** focus is per-tab today (`Tab.focused: WindowId`). A workspace must
  own its own focused-window pointer. Trivial under (b)/(c) (each workspace has
  its own tree, so its own `focused`); incoherent under (a) (which leaf is
  focused when the node is shared?).
- **Closing:** under (b)/(c), close = prune the view from *this* workspace's
  tree and `buffer_release` (drop only when refcount hits 0 and clean — already
  the rule). Under (a), "close" is ambiguous: this occurrence or the node
  everywhere?
- **Layout geometry conflict:** the same buffer naturally has *different
  geometry* in different workspaces (wide in a writing workspace, a thin sliver
  next to an agent in a review workspace). (b)/(c) get this for free — different
  view leaf, different weight. (a) cannot represent it.
- **Agent/Browser panels:** `AgentWindow` and `BrowserWindow` own their content
  *exclusively* and die with the window (`spec-tabs-and-splits.md` Overview, and
  the `with_initial`/content model). They are **not** poolable and therefore
  **cannot** be genuinely multi-membership. A Claude session is one subprocess;
  showing it in two workspaces would mean two views onto one `AcpChannelClient`
  — which is the *multi-subscriber mirror* idea from the self-hosting work, a
  much bigger lift, and out of scope here. So even in the best design, only
  **file-backed Doc/Edit panels** can be "in multiple workspaces"; Agent/Browser
  panels are single-home.

## Mapping onto sketch's tab / split model

Sketch today: `Workspace<C>` owns `tabs: Vec<Tab<C>>`; each `Tab` owns a
`Layout<C>` tree of `Window<C>` leaves (each leaf a `WindowId` + content), a
`focused: WindowId`, and (per `spec-rail.md`) an optional `rail`. File content
is shared via the `file_buffers` pool; Agent/Browser content is window-owned.

Three ways to introduce tags/workspaces:

### Option A — tags *replace* tabs

A `Tab` becomes a saved tag-view: a name (= the tag) + a layout + focus. The tab
strip becomes a tag strip. Switching "tabs" = `view tag`. This is the smallest
conceptual change — it's literally renaming `Tab` to `Workspace`/tag and the
strip to a tag bar — but on its own it gives you *nothing new* over today's
tabs unless you also add (1) buffer-tag membership and (2) union views. Without
those it's a rename.

### Option B — tags as a layer *above* tabs

Keep tabs; add workspaces that each select a *subset of tabs* (a tab carries a
tag bitmask; a workspace shows tabs whose mask intersects). This is the
configuration perspective.el explicitly warns against ("tab list is global …
likely to cause confusion"). Two stacked grouping layers (workspace-of-tabs,
tab-of-splits) is a lot of nesting for a markdown editor and the mental model
gets muddy fast (which layer owns focus? which owns the rail?). **Reject.**

### Option C — tags apply to *buffers*, workspaces own layout trees

The perspective.el / vim mapping, and the recommended one:

- A **Workspace** (new) ≈ today's `Tab`: a name, a `Layout<C>` tree of view
  leaves, a `focused: WindowId`, an optional rail. Rename or keep `Tab`.
- The **buffer pool is unchanged** (`file_buffers`, shared by reference,
  refcounted). This is the shared substrate.
- **Membership is derived** from the layout trees (option C-derived, the vim
  way): "buffer X is in workspace W" iff some leaf in `W.layout` is an
  `EditorView` onto X. No tag field needed for the v1 experience.
- **Optional explicit tags** (option C-explicit, the perspective.el way) only if
  a real workflow demands "this buffer should *belong* to workspaces 2 and 4
  even when not currently shown": add `tags: TagSet` to `FileBuffer` and a
  `:tag`/`:untag` command, then a "send these tagged buffers into the current
  workspace as a default layout" action. This is a strict superset of
  C-derived and can be added later without touching the layout trees.

**The hard problem — one buffer, N workspaces, different geometry — resolves
cleanly under C:** each workspace stores its own `Layout<C>` tree; the buffer is
shared by reference through the pool; the *view leaf* (with its own weight,
neighbors, cursor, scroll) is per-workspace. There is no shared layout node and
no `Rc` graph. Splitting, resizing, closing, focus motion — every method in
`workspace.rs` (`split_focused`, `close_focused`, `resize_focused`,
`focus_motion`, `only`, `equalize_focused`) operates on *one* tab's tree and is
already correct for "one tab = one workspace." The refcount machinery
(`buffer_retain`/`buffer_release`) already does the right thing when the last
view of a buffer across all workspaces closes.

What sketch is *missing* for the full dwm experience, and what each costs:

| dwm capability | sketch today | gap |
|---|---|---|
| swappable named layouts | `Tab` + tab strip + persistence | **already shipped** — this is the bulk of it |
| per-space focus memory | `Tab.focused` | already shipped |
| per-space rail/chrome | `Tab.rail` (spec-rail) | already specced |
| one buffer, many spaces | `file_buffers` pool, refcounted | **already shipped** (just not surfaced as a feature) |
| **union view (show tags 2∪4)** | — | **not present.** Requires merging two layout trees into a transient combined tree. Genuinely new, genuinely awkward (how do two independent split trees compose into one screen?). |
| **explicit buffer tags** | — | not present; optional add (FileBuffer.tags) |

## Feasibility verdict

**The proposal is feasible and ~80% already built — but the literal "tag the
panel" framing should be reframed as "tag/share the buffer; each workspace owns
its own layout of views."** That reframe is not a compromise; it is what every
successful implementation (vim tabpages, perspective.el, VS Code groups, Zed
panes) actually does, because a "panel" is fused geometry+cursor+content and
only the content is sharable.

**Not insane.** The one genuinely insane sub-idea is option (a) — the *same
layout node* living in two trees. That requires `Rc<RefCell>` layout graphs,
per-(node,tree) weight side-tables, and an unanswerable "is the cursor shared?"
question. Don't do it. Everything else is sound.

**The one dwm feature that doesn't transfer well** is the **union view** ("show
tags 2 and 4 at once"). In a WM, windows are free-floating rectangles that a
tiler re-packs on the fly, so unioning two tag-sets is just "re-tile this larger
set." In sketch, each workspace is a *hand-arranged split tree*; there is no
canonical way to merge two arbitrary split trees into one screen (concatenate as
siblings of a new root split? whose weights win?). Union view is the part to
**drop or heavily simplify** — and notably, even vim/perspective.el/VS Code
don't offer it. It's the most dwm-distinctive feature and the worst fit.

## Recommended design

A two-phase plan. Phase 1 is almost entirely a relabel + small additions on top
of the shipped tab/split model and delivers most of the value. Phase 2 is
optional and only if a concrete workflow demands explicit membership.

### Phase 1 — "Workspaces" (option C-derived). Low cost, high value.

1. **Rename the concept, not (much) the code.** Present today's `Tab` as a
   **Workspace**: a named, swappable layout of windows with its own focus and
   rail. The tab strip becomes the workspace strip. No data-model change beyond
   naming; `Workspace<C>`, `Tab<C>`, `Layout<C>`, `WindowId`, the pool, and all
   mutation methods stay exactly as in `workspace.rs`. (Note the unfortunate
   name collision: the *container* is already called `Workspace<C>`; the
   user-facing "workspace" is today's `Tab<C>`. Pick distinct names — e.g. keep
   `Workspace<C>` as the container and rename `Tab<C>` → `Space<C>` — before
   this confuses everyone.)

2. **Surface buffer sharing as a first-class verb.** Add
   `:send-to-space {n}` / `:also-in-space {n}`: take the focused **file-backed**
   view and open a view onto the *same pooled buffer* in space `n`'s layout
   (`buffer_retain` + insert a `Leaf` into `spaces[n].layout`). This is the
   user's "a panel lives in multiple workspaces," done correctly — same buffer,
   independent view/geometry per space. Reject the action for Agent/Browser
   panels (single-home) with a footer message.

3. **Persist per-space.** `workspace.json` already serializes per-tab layouts
   (Behavior 23). Each space's layout already records the buffer's canonical
   path per leaf — so "the same buffer in two spaces" round-trips for free (two
   leaves, same path, two spaces). Nothing new.

4. **Keybindings, dwm-flavored but sketch-native.** `Cmd-[1-9]` = view space N
   (replace), reusing the existing tab-switch path. `Ctrl-W m` then `[1-9]` =
   send focused view to space N. Keep `Ctrl-Tab`/`Cmd-T` working as today.

This phase ships the user's intent ("different desktops; the same doc can live
in several") with essentially no new data structures — it's naming + two
commands + two keybindings over already-shipped machinery.

### Phase 2 — explicit buffer tags (option C-explicit). Only if needed.

Add `tags: TagSet` to `FileBuffer`, `:tag {name}` / `:untag {name}`, and a
"populate this space with all buffers tagged X" action that builds a default
layout from the tagged set. This is the perspective.el `persp-add-buffer` /
`persp-set-buffer` pair. It's additive and touches only `FileBuffer` + commands,
not the layout trees. Defer until a workflow actually needs membership that
outlives a buffer being shown.

### Explicitly out of scope / rejected

- **Union view** (show space 2 ∪ space 4 simultaneously) — no clean tree-merge;
  not offered by any editor prior art; drop it.
- **Shared layout nodes** (option (a)) — `Rc<RefCell>` layout graph; rejected.
- **Multi-membership of Agent/Browser panels** — needs the multi-subscriber ACP
  mirror; separate, larger effort.
- **Workspaces-of-tabs nesting** (option B) — two grouping layers; rejected per
  the perspective.el confusion warning.

### Why this is the right cut

It matches what every editor that tried this converged on (share the document,
per-space geometry), it reuses ~all of `workspace.rs` unchanged, it gives the
user the headline feature ("the same doc in multiple desktops"), and it avoids
the two things that are genuinely hard or genuinely wrong (union views; shared
layout nodes). The 80/20: **Phase 1 is ~90% relabeling + 2 commands and
delivers the whole "tag a doc into several desktops" experience**, because
sketch already shipped the hard part (a refcounted shared buffer pool with
per-tab layout trees) for an unrelated reason.

## Open questions

1. **Naming collision.** `Workspace<C>` is taken (the container). If the
   user-facing concept is "Workspace," the container needs a new name
   (`App`/`Session`/`WorkspaceSet`) or the per-space type needs one (`Space`,
   `Desk`, `Tag`). Decide before writing UI copy.

2. **Is derived membership enough, or is explicit tagging actually wanted?**
   The user said "tag panels to belong to them" — does a doc need to *belong* to
   a space when it isn't currently shown there (Phase 2), or is "it's there
   because I put a view of it there" (Phase 1) sufficient? Likely Phase 1
   suffices; confirm with a real workflow.

3. **Cursor/scroll independence across spaces — desired or surprising?** Under
   the recommended design, the same doc in space 2 and space 4 has *independent*
   cursors and scroll (per-`EditorView`), but *shared* text and undo
   (per-`EditorCore`, Behaviors 9–11). Confirm that's the wanted feel (it's the
   vim/VS Code behavior, and almost certainly yes).

4. **Does the user actually want union views despite the cost?** If a
   composite "show me my writing doc *and* its outline space together" view is
   the real goal, that might be better served by the existing **rail**
   (`spec-rail.md`) or by ad-hoc splits, not by tree-merging two spaces.

5. **Persistence of empty/Phase-2 tags.** If explicit tags land, do tags on a
   buffer with refcount 0 (closed everywhere) survive in `workspace.json`?
   perspective.el keeps perspective→buffer membership independent of display;
   sketch would need a tag side-table that outlives the pool entry.

## Revision history

- 2026-06-02 — Initial research draft.
