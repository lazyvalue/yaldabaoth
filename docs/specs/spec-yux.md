# Spec: yux (yalda-ux) — a GPUI component layer that makes the wrong thing structurally impossible

**yux** (yalda-ux) is yalda's own thin component layer over GPUI. Status:
**draft** (begins the effort the owner asked for: components wrapped around GPUI
so a future session cannot re-introduce the responsiveness regressions). Lands
as the `yux` module (`yux.rs`, renamed from `cached_panel.rs`), superseding its
"free helpers" framing.

## Problem

GPUI re-renders the root every frame and its only render-skip lever is
`AnyView::cached` (see `docs/projects/gpui-responsiveness/project.md` for the
six verified facts). Today the correct pattern exists but only as **discipline**:
`cached_panel.rs` is free functions, and `TranscriptView` hand-assembles the
model-handle + observe-subscription + fingerprint-diff + cached-embed dance.
Every new surface re-implements that dance and can get any step wrong. The
guards we have are a test (`*_is_render_flat`) and docs (module `CLAUDE.md`) —
catch-after-the-fact and tell-you-not-to. We want **can't-write-it-wrong**.

## Goal

A component layer where the *only* sanctioned way to build an expensive surface
is a trait whose shape forbids the dangerous moves. The author writes what
differs (their element tree + what "changed" means); the framework owns what is
error-prone (observe wiring, fingerprint diff, notify timing, cached embed,
perf accounting).

## Design

### The trait + host

```rust
pub(crate) trait CachedView: 'static {
    type Model: 'static;
    type Fp: PartialEq + Clone;

    /// Pure read of the model → the cheap invalidation key. "What changed?"
    fn fingerprint(model: &Self::Model) -> Self::Fp;

    /// Build the element tree. Handed NO `Context` — cannot call cx.notify().
    /// `BuildCx` exposes element/listener helpers; listeners' closures get a
    /// real event-time cx (the legitimate deferred path).
    fn build(&mut self, model: &Self::Model, w: &mut Window, h: &mut BuildCx<Self>) -> AnyElement;
}

/// The ONLY `impl Render` for a cached surface. Owns the model handle, the
/// `cx.observe(&model)` subscription (registered in `new`), `last_fp`, the
/// diff, `record_*`, and the `cached_child` embed.
pub(crate) struct Cached<V: CachedView> { /* model, view-state V, last_fp, perf_label */ }
```

`render_agent`-style call sites shrink to: `cached_child(cached)` — the
`Cached<V>` host does the rest. (`CachedView` = the trait the author implements;
`Cached<V>` = the framework-owned host entity. The term "Panel" is deliberately
avoided — it connotes a dockable UI region, GPUI/Zed's meaning, whereas this is
a render-skip boundary that also wraps non-region surfaces like the status strip
and thinking indicator.)

### What each guarantee is worth (honest)

- **notify-in-render → FULLY STRUCTURAL.** `build` is never handed a `Context`,
  so `cx.notify()` in the render path does not compile. `BuildCx::listener`
  wraps `cx.listener`, whose closure gets an event-time cx — so click handlers
  still notify correctly. This is the headline win: the rev-1 / stale-tail bug
  class becomes unwritable.
- **observe / cache / size / accounting wiring → STRUCTURAL.** Done once in the
  host; a surface can't forget the subscription, embed uncached, collapse on a
  sizeless style, or skip `record_render`.
- **seq-coverage (read-in-build ⇒ present-in-fingerprint) → STRONGLY GUIDED,
  not type-proven.** Making it structural means `build` reads only a snapshot
  the fingerprint is derived from — but a snapshot holding the transcript lines
  makes the per-keystroke diff O(n), defeating the O(1) invalidation rev-2 was
  built to get. You cannot have O(1) invalidation AND structural coverage for
  free; the fingerprint is a hand-maintained O(1) summary and any summary can
  drift. Mitigations: a `#[derive(Fingerprint)]` on the key struct (adding a
  field updates the diff), plus the annotation/`*_is_render_flat` tests as
  backstops. (Sub-entities do NOT rescue this: notifying a model a cached view
  merely *read* does not put the view in `dirty_views`, so it won't bust the
  child cache — verified against gpui-0.2.2 source. The view must observe and
  self-notify regardless.)

## Open decision (needs owner input before implementation)

**How aggressive is `BuildCx`?** Two ends:

- **(A) Thin** — `BuildCx` is mostly a marker; `build` still reads the model
  directly and the no-notify guarantee comes from simply not passing `cx`. Least
  churn, keeps current ergonomics, ships fastest. notify-in-render still fully
  structural.
- **(B) Snapshot-projected** — `build` reads only a `V::Snapshot` the framework
  built from the model, making coverage structural at an O(snapshot) build cost
  per *actual* render (not per keystroke). Maximal safety, more boilerplate per
  surface, and needs the cheap-key/heavy-data split worked out.

Recommendation: **start at (A)** — it banks the fully-structural notify-in-render
win and the wiring wins immediately with low risk, and migrate the transcript
onto it as the proof. Revisit (B) per-surface if coverage drift actually recurs.

## Migration (one surface at a time; each is a ticket)

1. Build `CachedView` + `Cached<V>` + `BuildCx` + `#[derive(Fingerprint)]` in
   the `yux` module (`yux.rs`, renamed from `cached_panel.rs`; keeps the existing
   `record_*`). Headless: re-prove the render-skip + timing-law + observe-protocol
   tests against `Cached<V>`.
2. Re-express `TranscriptView` as `impl CachedView` (the flagship; behavior-
   identical, all `transcript_021_*` tests stay green). Deletes the hand-rolled
   `new`/observe/diff.
3. Compose box → `CachedView` (ticket 022). 4. Status strip + thinking
   indicator (023). 5. Each split/desktop leaf (030) — at which point hand-rolled
   `impl Render` for an app surface is the exception, not the norm.

## Guards that remain (the framework is necessary, not sufficient)

- CI runs `cargo test` on every push/PR + the pre-push hook; the `*_is_render_flat`
  render-count tests gate regressions on *covered* surfaces.
- Module `src/bin/yalda-gpui/CLAUDE.md` documents the pattern + "don't hand-roll
  `impl Render`; extend the framework."
- Still not covered: a *new* surface that bypasses the framework entirely (no
  type forces adoption — the module CLAUDE.md is the only nudge), and frame-time
  vs render-count (needs a human `sample`). These are accepted, documented gaps.

## Links

`docs/projects/gpui-responsiveness/project.md` (facts + component model),
`cached_panel.rs` (→ `yux.rs`), `transcript_view.rs`, module `CLAUDE.md`.
Warrants an ADR for the framework name (yux) and the (A)/(B) decision once made
(`/decision`).
