# Codex Feedback: GPUI Responsiveness

## Summary

The project is mostly pointed in the right direction. The core correction is important: splitting compose into its own entity is not enough. In GPUI, notifying a child dirties that child and its ancestors, so the root view still renders. The win comes from putting expensive, stable siblings like the transcript behind `AnyView::cached(...)`, so root render can still happen while transcript render/prepaint/paint are reused.

In short: the diagnosis is mostly correct, the transcript cached-child fix is the right first move, but the abstraction needs sharper contracts before it becomes a broad widget pattern.

## What Looks Correct

The main theory in `project.md` is accurate for GPUI:

- `YaldaGpuiView` being the large root entity means frequent `cx.notify()` causes broad parent rendering.
- A plain child `Entity<V>` does not by itself protect the child's render work.
- `AnyView::cached(style)` is the real GPUI lever for skipping clean child view render/prepaint/paint.
- The transcript is the right first target because typing in compose should not relayout stable transcript rows.
- The "inverse" framing is right: cache the transcript, not merely isolate compose.

The helper in `src/bin/yalda-gpui/cached_panel.rs` is directionally correct too. Wrapping GPUI's caching behavior behind a local abstraction is useful because the raw API is easy to misuse: cached views require stable bounds/style, clean child dirty state, and correct invalidation.

## Accuracy Correction

The comment in `cached_panel.rs` says GPUI notify marks "only that entity dirty." That is incomplete. GPUI marks the notified view and its ancestors dirty.

The property this project relies on is narrower:

- notifying the cached child dirties child and ancestors;
- notifying the parent/root does not dirty the cached child;
- therefore a clean cached child can be reused even while the root rerenders.

That distinction matters because otherwise future readers may think GPUI has finer invalidation than it really does.

## Main API Risk

`CachedPanel` currently has separate operations:

- mutate/update the child snapshot;
- call `notify_if_changed`;
- later return `element(style)`.

That ordering is a footgun. Ticket 021 depends on doing this exactly right: update the child inputs, then compare fingerprint, then notify, then return the cached element in the same render path.

A stronger API would make this atomic, for example:

```rust
panel.update_if_changed(cx, new_fp, |view, cx| {
    view.set_snapshot(snapshot, cx);
});

panel.element(style)
```

or a single method that updates, notifies if needed, and returns the element. The current API is reusable, but not yet hard to misuse.

## Fingerprint Correctness

The whole design rests on this invariant:

> If `render_fp` is unchanged, the rendered pixels and layout-affecting behavior of the child are unchanged.

That is a strong requirement. Missing one input means stale UI.

For transcript, the fingerprint needs to include every visual dependency: transcript edit/version, frozen ranges, expanded tool state, call structure, cursor/selection if rendered there, theme/text scale, and any live indicator state that appears inside the transcript.

Bounds should not be included, because GPUI's cache already invalidates on bounds changes. Visual state must be included. Avoid ad hoc hashes spread across components; prefer typed fingerprint inputs or version counters with debug logging that can explain why a panel notified.

## Widget Engineering Judgment

This direction is sustainable if used selectively.

Responsive UI design means high-frequency input should dirty the smallest expensive subtree possible, and per-keystroke work should be O(changed) or O(visible), not O(total app state). GPUI's model makes cached child entities the right tool for expensive stable siblings.

But this should not become "wrap everything in `CachedPanel`." Every cached entity adds lifecycle, fingerprint, sizing, and invalidation complexity. Use it for expensive widgets with stable inputs: transcript, large desktop leaves, status strips with expensive siblings, maybe browser panes. Do not use it as a blanket substitute for simpler render code.

For reusable components, clearer ownership boundaries would help:

- parent owns canonical app state;
- child owns local UI state like scroll/list/focus;
- parent pushes a typed render snapshot;
- child emits typed events/commands;
- fingerprint semantics are documented and tested;
- sizing style is explicit and part of the component contract.

Right now the project is close, but still more of a performance helper than a mature component API.

## Underweighted Problems

The browser filter issue is not primarily a render-cache issue. If filtering recursively walks the filesystem on every keystroke, caching siblings will not fix responsiveness. That needs debounce, background work, cancellation, and result versioning.

The compose long-draft issue also will not be solved by an `edit_seq` cache during actual typing, because the edit sequence changes every keystroke. That path needs incremental text/line handling or a better text editor model.

Desktop drag is similar: if the tile visually follows the pointer, per-pixel updates cannot simply be suppressed. Resize can be quantized; drag usually needs frame coalescing, not semantic gating.

The project also needs better measurement infrastructure. The `sample` output already indicates paint/prepaint/layout are hot, but the project needs counters for:

- root render count;
- transcript render count;
- cached prepaint reuse vs miss;
- miss reason: dirty child, bounds changed, text style changed, refresh;
- per-panel notify reason/fingerprint diff.

Without that, it will be too easy to finish the refactor while still missing the actual cache path.

## Recommendation

Proceed with ticket 021. The transcript cached child is the right GPUI-native fix for chatbox typing stutter.

Before applying the pattern broadly:

- tighten `CachedPanel` so update + fingerprint + notify ordering is hard to get wrong;
- fix the notify wording;
- add cache-hit/miss instrumentation;
- prove the transcript case with the same `sample` workflow;
- handle browser I/O and long-compose text processing as separate problems, because cached rendering alone will not solve those.
