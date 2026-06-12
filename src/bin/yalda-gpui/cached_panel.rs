//! Render-skip helper — the keystone of the responsiveness refactor.
//!
//! GPUI re-renders the **root** view every frame. Any child embedded as a plain
//! `Entity<V>` element therefore re-runs its `render()` every frame too, even
//! when nothing it reads has changed. The single GPUI lever that breaks this
//! coupling is [`AnyView::cached`] (gpui 0.2.2 `view.rs:103`): a cached view's
//! prepaint is reused — and `render()` SKIPPED — as long as the view's entity is
//! **not** in `window.dirty_views` (i.e. nobody called `App::notify` on it) and
//! the layout cache key (bounds / content-mask / text-style) is unchanged
//! (`view.rs:202-252`). `App::notify(entity_id)` marks *only* that entity dirty,
//! so notifying the parent does NOT dirty a cached child — its render is skipped.
//!
//! [`CachedPanel`] packages that lever behind a cheap **fingerprint**: the owner
//! holds the child `Entity<V>` plus the last fingerprint it rendered at. Each
//! frame the parent calls [`CachedPanel::notify_if_changed`], which reads the
//! child's [`FingerprintedPanel::render_fp`] (an allocation-free hash of
//! everything its `render()` reads) and notifies the **child** entity only when
//! the fingerprint moved. Unchanged fingerprint => no notify => `cached()` reuses
//! the prepaint => `render()` skipped. That is the O(changed) guarantee.
//!
//! # Call-ordering contract (load-bearing)
//!
//! [`CachedPanel::notify_if_changed`] is a per-frame **pull**: the owner MUST
//! call it every frame, BEFORE the cached child is laid out (i.e. before / as
//! part of building the element that embeds [`CachedPanel::element`]). If the
//! call is skipped or lands after layout, a fingerprint change invalidates one
//! frame late. Adopters: call it in the parent `render()` before emitting the
//! panel element.
//!
//! # Size-from-style requirement (load-bearing)
//!
//! When `cached_style` is set, `AnyView`'s `request_layout` sizes the element
//! **from the style**, not from content (`view.rs:170-176`): it refines a
//! `Style::default()` with the passed `StyleRefinement` and requests layout from
//! that. A default/empty style has zero size, so the cached panel would COLLAPSE.
//! Callers MUST pass a style that carries a size — use [`size_full_style`] (or a
//! `flex_1`/explicit-size refinement). [`CachedPanel::element`] takes the style
//! as an argument precisely to force the caller to make this choice.
//!
//! # Bound on the guarantee
//!
//! `cached()`'s reuse key also compares bounds / content-mask / text-style
//! (`view.rs:209-211`). A panel whose container resizes every frame, or whose
//! ambient text-style changes, re-renders regardless of an unchanged
//! fingerprint — that is inherent to gpui, not a helper bug.

use gpui::{AnyElement, App, Entity, IntoElement, Render, StyleRefinement, Styled};

/// A view that can produce a cheap, allocation-free fingerprint of everything its
/// `render()` reads. Two renders with the same fingerprint must be visually
/// identical; a changed fingerprint must change the rendered output. Used by
/// [`CachedPanel`] to decide whether to invalidate the render cache.
pub(crate) trait FingerprintedPanel: Render {
    /// A cheap hash over every input `render()` consumes. MUST NOT allocate and
    /// MUST be stable for identical render output.
    fn render_fp(&self) -> u64;
}

/// Owns a cached child panel: the child `Entity<V>` plus the fingerprint it was
/// last known to be at. Embed the child each frame via [`CachedPanel::element`]
/// and, before painting, call [`CachedPanel::notify_if_changed`] to invalidate
/// the GPUI render cache only when the child's inputs actually moved.
pub(crate) struct CachedPanel<V: FingerprintedPanel> {
    view: Entity<V>,
    last_fp: u64,
}

impl<V: FingerprintedPanel> CachedPanel<V> {
    /// Wrap a child view. `last_fp` is seeded from the child's current
    /// fingerprint so the first `notify_if_changed` only fires if the child has
    /// already diverged since construction (normal first-frame render still
    /// happens — the cache is empty until the first prepaint populates it).
    pub(crate) fn new(view: Entity<V>, cx: &App) -> Self {
        let last_fp = view.read(cx).render_fp();
        Self { view, last_fp }
    }

    /// The child handle, for callers that need to mutate it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn view(&self) -> &Entity<V> {
        &self.view
    }

    /// Read the child's current fingerprint; if it differs from the last one,
    /// update the stored fingerprint, **notify the child entity** (so it enters
    /// `window.dirty_views` and its cached `render()` is re-run next frame), and
    /// return `true`. If unchanged, do nothing and return `false` — leaving the
    /// child out of `dirty_views` so `cached()` reuses its prepaint and SKIPS
    /// `render()`.
    pub(crate) fn notify_if_changed(&mut self, cx: &mut App) -> bool {
        let fp = self.view.read(cx).render_fp();
        if fp != self.last_fp {
            self.last_fp = fp;
            self.view.update(cx, |_v, cx| cx.notify());
            true
        } else {
            false
        }
    }

    /// Embed the child as a **cached** element sized from `style`.
    ///
    /// The caller MUST pass a style carrying a size (see module docs and
    /// [`size_full_style`]); a sizeless style collapses the panel because cached
    /// `AnyView` layout is computed from the style, not the content.
    pub(crate) fn element(&self, style: StyleRefinement) -> AnyElement {
        // Entity<V> -> AnyView (view.rs:89 `From<Entity<V>> for AnyView`), then
        // `.cached(style)` sets `cached_style` so prepaint is reused unless the
        // entity is dirtied. `IntoElement for AnyView` (view.rs:304) -> element.
        gpui::AnyView::from(self.view.clone())
            .cached(style)
            .into_any_element()
    }
}

/// A `size_full` style refinement — the canonical size-carrying style for a
/// cached panel that should fill its parent. `StyleRefinement` implements
/// `Styled` (gpui `style.rs:283`), so the builder methods apply directly.
pub(crate) fn size_full_style() -> StyleRefinement {
    StyleRefinement::default().size_full()
}
