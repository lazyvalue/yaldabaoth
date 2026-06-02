//! Incremental highlight cache for the agent transcript pane.
//!
//! The agent pane re-highlights its entire transcript on every `cx.notify()`
//! (two passes — raw + stripped — over every line), then deep-clones both
//! results into the `'static` list-render closure. At a few-thousand-line
//! transcript that is several O(n) string-allocating passes per frame, which
//! is what saturates the main thread and starves keystrokes.
//!
//! This cache makes that work `O(changed)`:
//!   * **Fast skip** — if the document's `edit_seq`, line count, and theme are
//!     all unchanged since the last snapshot, the previous snapshot is handed
//!     back untouched (the scroll / cursor-blink / cross-pane-notify case).
//!   * **Per-line reconcile** — otherwise each line is compared by content
//!     hash *and* by the fenced-code state on entry; only lines that actually
//!     changed (or whose fence context shifted) are re-highlighted. Unchanged
//!     lines keep their existing `Rc<LineHl>` — no string re-allocation.
//!
//! Correctness across code fences comes from carrying the running `FenceState`
//! through the reconcile and storing the entry-state per line, so a fence
//! toggle correctly invalidates every line below it (not a fixed ±N window).

use std::rc::Rc;

use sketch::highlight::Highlighter;
use sketch::md_highlight::{advance_fence, highlight_one_line, FenceState, Segment};
use sketch::style::Style;
use sketch::theme::Theme;

/// Raw + stripped highlight segments for one source line. Shared via `Rc` so
/// unchanged lines survive a reconcile without re-allocating their segment
/// strings, and so the per-frame snapshot handed to the list render closure is
/// an O(1) pointer clone rather than a deep copy.
pub struct LineHl {
    /// Highlighting with raw markdown delimiters intact (editable user lines).
    pub raw: Vec<Segment>,
    /// Highlighting with inline delimiters stripped (frozen agent prose).
    pub stripped: Vec<Segment>,
}

impl LineHl {
    fn empty() -> Self {
        LineHl {
            raw: Vec::new(),
            stripped: Vec::new(),
        }
    }
}

/// Fingerprint of the theme fields the highlighter reads. Cheap to compare
/// (`Style` is `Copy + Eq`); a change forces a full re-highlight, which is
/// fine — theme switches are rare.
#[derive(Clone, PartialEq, Eq)]
struct ThemeFp {
    paragraph: Style,
    code_inline: Style,
    code_block_bg: Style,
    heading: [Style; 6],
}

impl ThemeFp {
    fn of(t: &Theme) -> Self {
        ThemeFp {
            paragraph: t.paragraph,
            code_inline: t.code_inline,
            code_block_bg: t.code_block_bg,
            heading: t.heading,
        }
    }
}

/// Compact fence fingerprint for cache invalidation. We only need to know
/// whether the line was inside a fence and what language was active — if
/// either changes, the line must be re-highlighted.
#[derive(Clone, PartialEq, Eq)]
struct FenceFp {
    in_fence: bool,
    /// Stores a hash of the language string rather than the string itself,
    /// to keep the per-line overhead at 2 words instead of 3 + heap alloc.
    lang_hash: u64,
}

impl FenceFp {
    fn of(fence: &FenceState) -> Self {
        FenceFp {
            in_fence: fence.in_fence,
            lang_hash: fence
                .lang
                .as_deref()
                .map(|s| {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    s.hash(&mut h);
                    h.finish()
                })
                .unwrap_or(0),
        }
    }
}

/// Per-pane incremental highlight cache. One lives on each `AgentState`.
pub struct HighlightCache {
    /// Per-line cached highlight, parallel to the source line vector.
    lines: Vec<Rc<LineHl>>,
    /// Per-line content hash, parallel to `lines`.
    hashes: Vec<u64>,
    /// Fence state on *entry* to each line, as used when that line was
    /// highlighted. A cached line is only reusable if both its hash and its
    /// entry-fence state still match.
    fence_before: Vec<FenceFp>,
    /// Document `edit_seq` captured at the last snapshot.
    last_edit_seq: u64,
    /// False until the first snapshot — guards the fast-skip path so that
    /// `edit_seq == 0` (a never-edited document) doesn't read uninitialized
    /// cache state.
    primed: bool,
    /// Theme fields in effect for the cached segments.
    theme_fp: Option<ThemeFp>,
    /// Last snapshot handed out, reused verbatim on the fast-skip path.
    snapshot: Option<Rc<Vec<Rc<LineHl>>>>,
    /// Stats for instrumentation: lines re-highlighted on the last reconcile.
    pub last_recomputed: usize,
    /// Whether the last `snapshot()` call took the fast-skip path.
    pub last_was_skip: bool,
}

impl HighlightCache {
    pub fn new() -> Self {
        HighlightCache {
            lines: Vec::new(),
            hashes: Vec::new(),
            fence_before: Vec::new(),
            last_edit_seq: 0,
            primed: false,
            theme_fp: None,
            snapshot: None,
            last_recomputed: 0,
            last_was_skip: false,
        }
    }

    /// Reconcile against `lines` and return a cheap shareable snapshot for the
    /// list render closure. Only lines whose content hash or inbound fence
    /// state changed are re-highlighted; everything else is reused.
    pub fn snapshot(
        &mut self,
        lines: &[String],
        theme: &Theme,
        edit_seq: u64,
    ) -> Rc<Vec<Rc<LineHl>>> {
        self.snapshot_inner(lines, theme, edit_seq, None)
    }

    /// Like `snapshot`, but with syntect-based code block highlighting.
    pub fn snapshot_syn(
        &mut self,
        lines: &[String],
        theme: &Theme,
        edit_seq: u64,
        hl: &Highlighter,
    ) -> Rc<Vec<Rc<LineHl>>> {
        self.snapshot_inner(lines, theme, edit_seq, Some(hl))
    }

    fn snapshot_inner(
        &mut self,
        lines: &[String],
        theme: &Theme,
        edit_seq: u64,
        hl: Option<&Highlighter>,
    ) -> Rc<Vec<Rc<LineHl>>> {
        let fp = ThemeFp::of(theme);
        let theme_changed = self.theme_fp.as_ref() != Some(&fp);

        // Fast path: nothing the highlighter depends on has changed.
        if self.primed
            && !theme_changed
            && edit_seq == self.last_edit_seq
            && lines.len() == self.lines.len()
        {
            if let Some(snap) = &self.snapshot {
                self.last_recomputed = 0;
                self.last_was_skip = true;
                return snap.clone();
            }
        }
        self.last_was_skip = false;

        if theme_changed {
            // Styles are baked into cached segments — drop everything.
            self.lines.clear();
            self.hashes.clear();
            self.fence_before.clear();
            self.theme_fp = Some(fp);
        }

        let n = lines.len();
        let no_fence = FenceFp {
            in_fence: false,
            lang_hash: 0,
        };
        if self.lines.len() != n {
            self.lines.resize_with(n, || Rc::new(LineHl::empty()));
            // `u64::MAX` is the "no cached hash" sentinel: a freshly grown slot
            // never matches a real line hash, so it is always recomputed.
            self.hashes.resize(n, u64::MAX);
            self.fence_before.resize(n, no_fence.clone());
        }

        let mut fence = FenceState::new();
        let mut recomputed = 0;
        for i in 0..n {
            let h = hash_line(&lines[i]);
            let entry_fp = FenceFp::of(&fence);
            let reuse = self.hashes[i] == h && self.fence_before[i] == entry_fp;
            if !reuse {
                let (raw, _) = highlight_one_line(&lines[i], &fence, theme, false, hl);
                let (stripped, _) = highlight_one_line(&lines[i], &fence, theme, true, hl);
                self.lines[i] = Rc::new(LineHl { raw, stripped });
                self.hashes[i] = h;
                self.fence_before[i] = entry_fp;
                recomputed += 1;
            }
            // Advance fence state regardless of reuse. Use the cheap byte-scan
            // `advance_fence` rather than a full `highlight_one_line`: during
            // live streaming `edit_seq` bumps every chunk so the fast-skip path
            // is missed and this loop runs every frame. Calling the full
            // highlighter here would re-tokenize + re-allocate every non-fence
            // line each frame, making the reconcile O(transcript) instead of
            // O(changed). `advance_fence` mirrors the fence branches exactly.
            fence = advance_fence(&lines[i], &fence);
        }

        // Cloning `Vec<Rc<LineHl>>` is N pointer copies (refcount bumps), no
        // string allocation. The closure owns this snapshot for the frame.
        let snap = Rc::new(self.lines.clone());
        self.snapshot = Some(snap.clone());
        self.last_edit_seq = edit_seq;
        self.primed = true;
        self.last_recomputed = recomputed;
        snap
    }
}

fn hash_line(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sketch::md_highlight::{highlight_markdown_lines, highlight_markdown_lines_stripped};

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(|l| l.to_string()).collect()
    }

    /// The incremental snapshot must be byte-identical to a from-scratch
    /// batch highlight of the same lines — this is the whole correctness
    /// contract of the cache.
    fn assert_matches_batch(cache_snap: &[Rc<LineHl>], lines: &[String], theme: &Theme) {
        let raw = highlight_markdown_lines(lines, theme);
        let stripped = highlight_markdown_lines_stripped(lines, theme);
        assert_eq!(cache_snap.len(), lines.len(), "line count");
        for i in 0..lines.len() {
            assert_eq!(cache_snap[i].raw, raw[i], "raw mismatch at line {i}: {:?}", lines[i]);
            assert_eq!(
                cache_snap[i].stripped, stripped[i],
                "stripped mismatch at line {i}: {:?}",
                lines[i]
            );
        }
    }

    #[test]
    fn cold_snapshot_matches_batch() {
        let theme = Theme::default();
        let mut cache = HighlightCache::new();
        let ls = lines("# Heading\nplain **bold** text\n```rust\nlet x = 1;\n```\nafter fence");
        let snap = cache.snapshot(&ls, &theme, 1);
        assert_matches_batch(&snap, &ls, &theme);
        // Every line was cold → all recomputed.
        assert_eq!(cache.last_recomputed, ls.len());
    }

    #[test]
    fn fast_skip_when_edit_seq_unchanged() {
        let theme = Theme::default();
        let mut cache = HighlightCache::new();
        let ls = lines("alpha\nbravo\ncharlie");
        let first = cache.snapshot(&ls, &theme, 7);
        let second = cache.snapshot(&ls, &theme, 7);
        assert!(cache.last_was_skip, "same edit_seq must take the fast path");
        assert_eq!(cache.last_recomputed, 0);
        // Same allocation handed back.
        assert!(Rc::ptr_eq(&first, &second));
    }

    #[test]
    fn appending_only_rehighlights_the_tail() {
        let theme = Theme::default();
        let mut cache = HighlightCache::new();
        let ls = lines("one\ntwo\nthree");
        cache.snapshot(&ls, &theme, 1);
        let mut grown = ls.clone();
        grown.push("four".into());
        grown.push("five".into());
        let snap = cache.snapshot(&grown, &theme, 2);
        assert_matches_batch(&snap, &grown, &theme);
        // Only the two appended lines should re-highlight; the original
        // three are reused.
        assert_eq!(cache.last_recomputed, 2);
    }

    #[test]
    fn opening_a_fence_invalidates_lines_below() {
        // Regression guard for the spec-synthesis bug: a fence can span far
        // more than a fixed ±N window, so toggling one must re-derive every
        // line beneath it, not just nearby ones.
        let theme = Theme::default();
        let mut cache = HighlightCache::new();
        let before = lines("text\nstill text\nmore text\nfinal text");
        cache.snapshot(&before, &theme, 1);
        // Turn line 0 into a fence opener: lines 1..=3 are now *inside* a
        // code block and must be highlighted as code.
        let after = lines("```\nstill text\nmore text\nfinal text");
        let snap = cache.snapshot(&after, &theme, 2);
        assert_matches_batch(&snap, &after, &theme);
        // Line 0 changed text; lines 1-3 changed fence context → all 4.
        assert_eq!(cache.last_recomputed, 4);
    }

    #[test]
    fn closing_a_fence_restores_lines_below() {
        let theme = Theme::default();
        let mut cache = HighlightCache::new();
        let fenced = lines("```\ncode line\nmore code\nplain?");
        cache.snapshot(&fenced, &theme, 1);
        // Insert a closing fence by editing line 0 away — now nothing is fenced.
        let unfenced = lines("prose\ncode line\nmore code\nplain?");
        let snap = cache.snapshot(&unfenced, &theme, 2);
        assert_matches_batch(&snap, &unfenced, &theme);
    }

    #[test]
    fn mid_buffer_edit_only_touches_one_line() {
        let theme = Theme::default();
        let mut cache = HighlightCache::new();
        let ls = lines("aaa\nbbb\nccc\nddd\neee");
        cache.snapshot(&ls, &theme, 1);
        let mut edited = ls.clone();
        edited[2] = "ccc changed".into();
        let snap = cache.snapshot(&edited, &theme, 2);
        assert_matches_batch(&snap, &edited, &theme);
        assert_eq!(cache.last_recomputed, 1);
    }

    /// Build a representative agent transcript: headings, prose with inline
    /// markup, and multi-line fenced code blocks, repeated to `n` lines.
    fn synthetic_transcript(n: usize) -> Vec<String> {
        let block = [
            "## Section heading",
            "Here is some **bold** prose with `inline code` and a [link](http://x).",
            "More explanation text that wraps the idea across a sentence or two.",
            "",
            "```rust",
            "fn example(x: usize) -> usize {",
            "    let mut total = 0;",
            "    for i in 0..x { total += i * 2; }",
            "    total",
            "}",
            "```",
            "- a bullet point",
            "- another bullet with *emphasis*",
            "",
        ];
        let mut out = Vec::with_capacity(n);
        let mut i = 0;
        while out.len() < n {
            out.push(block[i % block.len()].to_string());
            i += 1;
        }
        out
    }

    /// Old per-frame cost: two full highlight passes plus the two full
    /// `Vec<Vec<Segment>>` deep-clones the render closure used to take.
    fn full_path(lines: &[String], theme: &Theme) -> usize {
        let raw = highlight_markdown_lines(lines, theme);
        let stripped = highlight_markdown_lines_stripped(lines, theme);
        let raw_snap = raw.clone();
        let stripped_snap = stripped.clone();
        raw_snap.len() + stripped_snap.len()
    }

    fn ms(d: std::time::Duration) -> f64 {
        d.as_secs_f64() * 1e3
    }

    /// Not a pass/fail test — a measurement harness. Run with:
    ///   cargo test --release --bin sketch-gpui -- --ignored --nocapture perf_report
    #[test]
    #[ignore]
    fn perf_report() {
        let theme = Theme::default();
        for &n in &[500usize, 2000, 5000] {
            let lines = synthetic_transcript(n);
            let iters = 50;

            // --- OLD full path (every frame re-highlights all + 2 clones) ---
            let t = std::time::Instant::now();
            let mut acc = 0;
            for _ in 0..iters {
                acc += full_path(&lines, &theme);
            }
            std::hint::black_box(acc);
            let full_ms = ms(t.elapsed()) / iters as f64;

            // --- CACHE cold (first paint of a session) ---
            let t = std::time::Instant::now();
            let mut cache = HighlightCache::new();
            let _ = cache.snapshot(&lines, &theme, 1);
            let cold_ms = ms(t.elapsed());

            // --- CACHE no-change (scroll / cursor blink / cross-pane notify) ---
            let t = std::time::Instant::now();
            for _ in 0..iters {
                std::hint::black_box(cache.snapshot(&lines, &theme, 1));
            }
            let skip_ms = ms(t.elapsed()) / iters as f64;

            // --- CACHE streaming append (tail grows one line per frame) ---
            let mut grow_cache = HighlightCache::new();
            let mut grown = lines.clone();
            let _ = grow_cache.snapshot(&grown, &theme, 1);
            let t = std::time::Instant::now();
            for k in 0..iters {
                grown.push("streamed token line of agent output".to_string());
                std::hint::black_box(grow_cache.snapshot(&grown, &theme, 2 + k as u64));
            }
            let stream_ms = ms(t.elapsed()) / iters as f64;

            // --- CACHE single mid-buffer edit (worksheet typing) ---
            let mut edit_cache = HighlightCache::new();
            let mut edited = lines.clone();
            let _ = edit_cache.snapshot(&edited, &theme, 1);
            let mid = n / 2;
            let t = std::time::Instant::now();
            for k in 0..iters {
                edited[mid] = format!("edited line keystroke {k}");
                std::hint::black_box(edit_cache.snapshot(&edited, &theme, 2 + k as u64));
            }
            let edit_ms = ms(t.elapsed()) / iters as f64;

            println!(
                "\n=== {n} lines (avg ms/frame over {iters} iters) ===\n\
                 OLD full path (2 highlight + 2 clone) : {full_ms:8.3} ms\n\
                 CACHE cold (first paint)              : {cold_ms:8.3} ms\n\
                 CACHE no-change (scroll/idle)         : {skip_ms:8.3} ms   [{:.0}x faster]\n\
                 CACHE streaming append (tail+1)       : {stream_ms:8.3} ms   [{:.0}x faster]\n\
                 CACHE single-line edit (typing)       : {edit_ms:8.3} ms   [{:.0}x faster]",
                full_ms / skip_ms.max(1e-6),
                full_ms / stream_ms.max(1e-6),
                full_ms / edit_ms.max(1e-6),
            );
        }
    }

    #[test]
    fn theme_change_invalidates_everything() {
        let mut cache = HighlightCache::new();
        let ls = lines("# H\nbody\n```\ncode\n```");
        let dark = Theme::dracula();
        cache.snapshot(&ls, &dark, 5);
        // Same text + same edit_seq, but a genuinely different theme must NOT
        // fast-skip — cached segments carry baked-in colors.
        let light = Theme::solarized_light();
        let snap = cache.snapshot(&ls, &light, 5);
        assert!(!cache.last_was_skip);
        assert_matches_batch(&snap, &ls, &light);
        assert_eq!(cache.last_recomputed, ls.len());
    }
}
