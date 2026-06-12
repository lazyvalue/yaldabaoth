//! Render-skip embed helper — the thin wrapper around GPUI's one render-skip
//! lever, plus the perf counters every later ticket asserts against.
//!
//! GPUI re-renders the **root** view every frame ([`gpui` 0.2.2]
//! `window.rs` `draw_roots`, called unconditionally from `draw`). Any child
//! embedded as a plain `Entity<V>` element therefore re-runs its `render()`
//! every frame too, even when nothing it reads has changed. The single GPUI
//! lever that breaks this coupling is [`AnyView::cached`] (`view.rs:103`): a
//! cached view's prepaint is reused — and `render()` SKIPPED — iff its entity
//! is **not** in `window.dirty_views` AND the layout cache key (bounds /
//! content-mask / text-style) is unchanged (`view.rs:209-212`).
//!
//! # The invalidation model (corrected — read this before using the helper)
//!
//! Rev 1 (ticket 020) gated the cached child on a fingerprint polled from
//! inside the parent's `render()` and notified the child mid-draw. That is
//! unsound. The corrected facts, verified against the gpui 0.2.2 source:
//!
//! 1. **Notify dirties the entity AND its ancestor views — up only.**
//!    `mark_view_dirty` (`window.rs:1304`) walks `view_path(view_id)` and
//!    inserts each ancestor into `dirty_views`. Propagation is **upward**:
//!    notifying the parent/root never dirties a cached *child*, so the child's
//!    `render()` is skipped. (Rev 1's "marks only that entity" was wrong, but
//!    the conclusion — parent notify skips the child — still holds, for the
//!    right reason.) A cached child invalidates by being notified *itself*.
//!
//! 2. **The timing law: a `cx.notify` issued DURING a draw is parked.**
//!    `invalidate_view` (`window.rs:116`) only pushes `Effect::Notify` and
//!    sets the window dirty flag when `draw_phase == None`; under a draw it
//!    drops into the invalidator's pending set instead. Consequences:
//!    - it cannot affect the current frame — `dirty_views` was drained at draw
//!      start (`window.rs:1926`, `self.dirty_views.clear()` after
//!      `draw_roots`);
//!    - it does not schedule a next frame — the frame loop only draws when
//!      `is_dirty()` (`window.rs:128` / the `on_request_frame` loop ~`1018`),
//!      so the stale frame persists until an *unrelated* event;
//!    - observers are skipped.
//!    ⇒ **NEVER call `cx.notify()` from inside a `render()` / `Render` impl.**
//!    This is the exact bug rev 1 had. Notify only from event handlers,
//!    `cx.observe` callbacks, timers, or `cx.defer` — all run at
//!    `draw_phase == None` and land in `dirty_views` for the very frame their
//!    triggering event scheduled (zero frames late).
//!
//! 3. **Observation is the canonical cache-busting path.** `cx.observe`
//!    (`app.rs:780`) callbacks fire in effect flush (`apply_notify_effect`,
//!    `app.rs:1301`) — outside the draw — so a cached view busts its own cache
//!    by doing `cx.observe(&model, |view, _model, cx| { …; cx.notify() })`,
//!    filtered by monotonic version counters so it only self-notifies when its
//!    own slice actually moved. Alternatively, mutators that own the state
//!    notify at the **mutation site** (`model.update(cx, |m, cx| { …;
//!    cx.notify() })`) — also outside the draw, also timing-correct.
//!
//! 4. **Accessed-entity tracking schedules redraws but does not bust caches.**
//!    A cached view's render records the entities it read; notifying a read
//!    entity schedules a redraw, but only a notify on the **view entity
//!    itself** lands in `dirty_views`. Hence rule 3: cached views invalidate
//!    themselves via observation.
//!
//! # Size-from-style requirement (load-bearing)
//!
//! When `cached_style` is set, `AnyView`'s `request_layout` sizes the element
//! **from the style**, not from content (`view.rs:170-176`): it refines a
//! `Style::default()` with the passed `StyleRefinement` and requests layout
//! from that. A default/empty style has zero size, so the cached panel would
//! COLLAPSE. [`cached_child`] bakes in `size_full` so the common case is
//! misuse-proof; [`cached_child_styled`] takes an explicit style for non-fill
//! cases — and that style MUST carry a size (`flex_1` / explicit dimensions).
//!
//! # Bound on the guarantee
//!
//! `cached()`'s reuse key also compares bounds / content-mask / text-style
//! (`view.rs:209-211`). A panel whose container resizes every frame, or whose
//! ambient text-style changes, re-renders regardless — that is inherent to
//! gpui, not a helper bug.

use gpui::{AnyElement, AnyView, IntoElement, StyleRefinement, Styled};

/// Embed `view` as a **cached** element that fills its parent (`size_full`).
///
/// This is the misuse-proof default: the size is baked in, so the cached slot
/// can never collapse for want of a sizing style. Invalidation is the view's
/// own job — it must `cx.observe` its model (or be notified at a mutation
/// site); embedding it here only opts it into render-skip, it does not poll or
/// notify anything.
///
/// (No non-test consumer yet — ticket 025/021 wire the first ones; the proof
/// test in `verify_harness.rs` exercises it today.)
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn cached_child(view: impl Into<AnyView>) -> AnyElement {
    cached_child_styled(view, size_full_style())
}

/// Embed `view` as a **cached** element sized from `style`.
///
/// The caller MUST pass a style carrying a size (see the module's
/// "Size-from-style requirement"); a sizeless style collapses the panel
/// because cached `AnyView` layout is computed from the style, not the
/// content. Prefer [`cached_child`] unless you genuinely need a non-fill size.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn cached_child_styled(view: impl Into<AnyView>, style: StyleRefinement) -> AnyElement {
    // `AnyView::cached(style)` sets `cached_style` so prepaint is reused unless
    // the entity is in `dirty_views` (`view.rs:103` / `:209-212`).
    // `IntoElement for AnyView` turns it into a paintable element.
    view.into().cached(style).into_any_element()
}

/// A `size_full` style refinement — the canonical size-carrying style for a
/// cached panel that should fill its parent. `StyleRefinement` implements
/// `Styled`, so the builder methods apply directly.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn size_full_style() -> StyleRefinement {
    StyleRefinement::default().size_full()
}

// ---- YALDA_PERF render / cache-hit-miss counters -------------------------
//
// The instrumentation the responsiveness refactor verifies "fast" against.
//
// HONEST INFERENCE LIMITS: gpui does not expose its cache-check result, so we
// CANNOT directly observe a cached child's hit/miss or gpui's reuse-key
// comparison (bounds / content-mask / text-style). What we CAN observe is:
//   * how many times each instrumented view's `render()` actually ran
//     ([`record_render`], called from the view's `render`), and
//   * the last invalidation reason we recorded at OUR OWN notify sites
//     ([`record_notify`], called from `cx.observe` callbacks / mutation
//     sites).
// A "hit" is therefore *inferred*: a frame in which the parent rendered but an
// instrumented cached child's render count did NOT advance. A "miss reason" is
// whatever we last stamped at our notify site (dirtied / bounds / text-style /
// refresh) — `bounds`/`text-style` misses originate INSIDE gpui's reuse key,
// so we can only label them when we deliberately drive them (e.g. a resize
// handler stamping `Bounds`); we never see gpui deciding them on its own.
// Headless tests close the gap by asserting render-count deltas directly (the
// `PROBE_RENDERS` idiom), which is ground truth for skip/no-skip.

/// Why a cached panel's render cache was (or is inferred to have been)
/// invalidated. Recorded at our notify sites; gpui-internal reasons
/// (`Bounds` / `TextStyle`) can only be stamped when WE drive them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum MissReason {
    /// The view's own slice moved (a version counter advanced) and its
    /// `cx.observe` callback / a mutation site notified it. The common path.
    Dirtied,
    /// The container resized — gpui's reuse key `bounds` differs. Only
    /// stampable from a resize handler we own.
    Bounds,
    /// The ambient text-style changed — gpui's reuse key `text_style` differs.
    /// Only stampable when we drive a text-style change (e.g. zoom).
    TextStyle,
    /// A forced full refresh (theme swap, window reactivation, etc.).
    Refresh,
}

#[cfg(any(test, debug_assertions))]
#[cfg_attr(not(test), allow(dead_code))]
mod perf {
    use super::MissReason;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        /// Per-label render count: how many times an instrumented view's
        /// `render()` actually ran. Ground truth for skip/no-skip.
        static RENDER_COUNTS: RefCell<HashMap<&'static str, u64>> =
            RefCell::new(HashMap::new());
        /// Per-label last-recorded notify reason — what WE stamped at our
        /// invalidation site. Inferred, not read from gpui (see module note).
        static LAST_NOTIFY: RefCell<HashMap<&'static str, MissReason>> =
            RefCell::new(HashMap::new());
    }

    /// Is the `YALDA_PERF` counter scaffolding live? (env var set, like the
    /// other perf gates.) Counters are always recorded in test/debug builds so
    /// headless assertions work regardless of the env var.
    pub(super) fn perf_enabled() -> bool {
        std::env::var_os("YALDA_PERF").is_some()
    }

    pub(super) fn record_render(label: &'static str) {
        RENDER_COUNTS.with(|m| {
            *m.borrow_mut().entry(label).or_insert(0) += 1;
        });
        if perf_enabled() {
            let n = RENDER_COUNTS.with(|m| *m.borrow().get(label).unwrap_or(&0));
            eprintln!("[perf] cached-panel render label={label} count={n}");
        }
    }

    pub(super) fn record_notify(label: &'static str, reason: MissReason) {
        LAST_NOTIFY.with(|m| {
            m.borrow_mut().insert(label, reason);
        });
        if perf_enabled() {
            eprintln!("[perf] cached-panel notify  label={label} reason={reason:?}");
        }
    }

    pub(super) fn render_count(label: &'static str) -> u64 {
        RENDER_COUNTS.with(|m| *m.borrow().get(label).unwrap_or(&0))
    }

    pub(super) fn last_notify(label: &'static str) -> Option<MissReason> {
        LAST_NOTIFY.with(|m| m.borrow().get(label).copied())
    }

    pub(super) fn reset(label: &'static str) {
        RENDER_COUNTS.with(|m| {
            m.borrow_mut().remove(label);
        });
        LAST_NOTIFY.with(|m| {
            m.borrow_mut().remove(label);
        });
    }
}

/// Record that an instrumented cached view's `render()` actually ran. Call
/// this at the top of a cached panel's `render()`; a frame where the count does
/// NOT advance is an inferred cache hit (render-skip). No-op in release builds
/// unless the counter module is compiled in.
#[cfg_attr(not(any(test, debug_assertions)), allow(unused_variables))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn record_render(label: &'static str) {
    #[cfg(any(test, debug_assertions))]
    perf::record_render(label);
}

/// Record the reason a cached view was invalidated, at OUR notify site (the
/// `cx.observe` callback or mutation site that calls `cx.notify`). This is the
/// inferred miss reason — gpui-internal reasons can only be stamped when we
/// drive them. No-op in release builds unless the counter module is compiled
/// in.
#[cfg_attr(not(any(test, debug_assertions)), allow(unused_variables))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn record_notify(label: &'static str, reason: MissReason) {
    #[cfg(any(test, debug_assertions))]
    perf::record_notify(label, reason);
}

/// Current render count for `label` (test/debug only). The render-skip oracle:
/// a parent re-render that leaves this flat proves the child was skipped.
#[cfg(any(test, debug_assertions))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn perf_render_count(label: &'static str) -> u64 {
    perf::render_count(label)
}

/// Last recorded notify reason for `label` (test/debug only).
#[cfg(any(test, debug_assertions))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn perf_last_notify(label: &'static str) -> Option<MissReason> {
    perf::last_notify(label)
}

/// Reset a label's counters (test/debug only). Tests call this to isolate a
/// measurement window.
#[cfg(any(test, debug_assertions))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn perf_reset(label: &'static str) {
    perf::reset(label);
}
