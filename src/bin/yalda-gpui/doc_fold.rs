//! Heading navigation + folding for the Doc (Buffer `Viewing`) view — graph
//! 4f1 `heading-nav`, UXI-Buffer-13 (`]]` / `[[`) and UXI-Buffer-14 (`za` /
//! `zM` / `zR`).
//!
//! The fold state lives on the tile's [`DocState`] (`folds`), keyed by each
//! folded heading's first source line (the block index for an unmapped,
//! string-backed Doc). The value is the heading's identity `(level, text)`, used
//! to re-key a fold after a re-parse moved its line ([`DocState::rekey_folds`]);
//! a fold whose heading vanished is dropped. The derived per-block visibility
//! ([`FoldLayout`]) is recomputed on the mutation path only (a fold command or
//! `set_blocks`), bumps `fold_seq` — a `DocSeqs` input — and is read by the
//! cached `DocView` body, which paints a hidden block as a zero-height row.

use super::*;
use std::collections::BTreeMap;

/// Folded headings of one Doc: first source line → `(level, plain text)`.
pub(crate) type DocFolds = BTreeMap<usize, (u8, String)>;

/// Per-block visibility derived from `(blocks, folds)`. Shared by `Rc` with the
/// `'static` list render closure.
#[derive(Default, Debug)]
pub(crate) struct FoldLayout {
    hidden: Vec<bool>,
    /// Folded, painted heading block → how many blocks its fold hides (> 0).
    folded: HashMap<usize, usize>,
}

/// The level of `b` if it is a (top-level) heading block.
fn heading_level(b: &RenderedBlock) -> Option<u8> {
    match b {
        RenderedBlock::Heading { level, .. } => Some(*level),
        _ => None,
    }
}

impl FoldLayout {
    /// A folded heading hides every following block up to (not including) the
    /// next heading of the same or a higher level (smaller number). Headings
    /// nested inside a hidden section are hidden with it, folded or not.
    fn compute(blocks: &[RenderedBlock], key: impl Fn(usize) -> usize, folds: &DocFolds) -> Self {
        let mut hidden = vec![false; blocks.len()];
        let mut folded = HashMap::new();
        // The outermost fold currently hiding blocks: (heading block, level).
        let mut open: Option<(usize, u8)> = None;
        for (i, b) in blocks.iter().enumerate() {
            let level = heading_level(b);
            if let (Some((_, fl)), Some(l)) = (open, level)
                && l <= fl
            {
                open = None;
            }
            if let Some((owner, _)) = open {
                hidden[i] = true;
                *folded.entry(owner).or_insert(0) += 1;
            } else if let Some(l) = level
                && folds.contains_key(&key(i))
            {
                open = Some((i, l));
            }
        }
        FoldLayout { hidden, folded }
    }

    /// Whether block `i` is inside a folded section (not painted, skipped by nav).
    pub(crate) fn is_hidden(&self, i: usize) -> bool {
        self.hidden.get(i).copied().unwrap_or(false)
    }

    /// `Some(n)` when block `i` is a painted, folded heading hiding `n` blocks.
    pub(crate) fn hidden_count(&self, i: usize) -> Option<usize> {
        self.folded.get(&i).copied()
    }
}

impl DocState {
    /// The fold key of block `i`: its first source line, or the block index
    /// for an unmapped Doc (the same coordinate the outline's `entry.line` uses).
    pub(crate) fn fold_key(&self, i: usize) -> usize {
        self.spans.get(i).map_or(i, |s| s.lines.start)
    }

    /// `(level, plain text)` of block `i` if it is a heading.
    fn heading_identity(&self, i: usize) -> Option<(u8, String)> {
        match self.blocks.get(i)? {
            RenderedBlock::Heading { level, content } => Some((*level, content.text_content())),
            _ => None,
        }
    }

    pub(crate) fn is_block_hidden(&self, i: usize) -> bool {
        self.fold_layout.is_hidden(i)
    }

    /// Rebuild the visibility layout from `folds` (the mutation path — never a
    /// render): bump `fold_seq` for the body's fingerprint, splice the rows
    /// whose visibility flipped so the list re-measures them (a folded row's
    /// cached height would otherwise skew reveals), and pull a now-hidden cursor
    /// back onto the folded heading that hides it.
    pub(crate) fn refold(&mut self) {
        let layout = {
            let spans = &self.spans;
            FoldLayout::compute(
                &self.blocks,
                |i| spans.get(i).map_or(i, |s| s.lines.start),
                &self.folds,
            )
        };
        let changed: Vec<usize> = (0..layout.hidden.len())
            .filter(|&i| layout.is_hidden(i) != self.fold_layout.is_hidden(i))
            .collect();
        if let (Some(&lo), Some(&hi)) = (changed.first(), changed.last())
            && hi < self.list.len()
        {
            self.list.state().splice(lo..hi + 1, hi + 1 - lo);
        }
        self.fold_layout = Rc::new(layout);
        self.fold_seq = self.fold_seq.wrapping_add(1);
        if self.is_block_hidden(self.cursor_block) {
            self.cursor_block = (0..self.cursor_block)
                .rev()
                .find(|&j| !self.is_block_hidden(j))
                .unwrap_or(0);
        }
    }

    /// Re-key every fold against freshly parsed blocks (called by
    /// `set_blocks`): each folded heading maps to the heading with the same
    /// `(level, text)` nearest its old line, else to a same-level heading still
    /// on that line (its text was edited in place); otherwise the fold is
    /// dropped — its heading vanished.
    pub(crate) fn rekey_folds(&mut self) {
        if !self.folds.is_empty() {
            let headings: Vec<(usize, u8, String)> = (0..self.blocks.len())
                .filter_map(|i| self.heading_identity(i).map(|(l, t)| (self.fold_key(i), l, t)))
                .collect();
            let mut out = DocFolds::new();
            for (&old_line, (level, text)) in &self.folds {
                let free = |h: &&(usize, u8, String)| !out.contains_key(&h.0);
                let best = headings
                    .iter()
                    .filter(free)
                    .filter(|h| h.1 == *level && h.2 == *text)
                    .min_by_key(|h| h.0.abs_diff(old_line))
                    .or_else(|| {
                        headings
                            .iter()
                            .filter(free)
                            .find(|h| h.0 == old_line && h.1 == *level)
                    });
                if let Some((line, l, t)) = best {
                    out.insert(*line, (*l, t.clone()));
                }
            }
            self.folds = out;
        }
        self.refold();
    }

    /// Replace the fold set (carried across a Doc → Edit → Doc round trip) and
    /// re-key it against the current blocks.
    pub(crate) fn restore_folds(&mut self, folds: DocFolds) {
        self.folds = folds;
        self.rekey_folds();
    }

    /// The heading whose section holds block `i`: `i` itself if it is a
    /// heading, else the nearest heading above it.
    fn section_heading(&self, i: usize) -> Option<usize> {
        (0..=i.min(self.blocks.len().checked_sub(1)?))
            .rev()
            .find(|&j| heading_level(&self.blocks[j]).is_some())
    }

    /// How many blocks the section under heading `h` spans (its fold would hide).
    fn section_len(&self, h: usize) -> usize {
        let Some(level) = self.blocks.get(h).and_then(heading_level) else {
            return 0;
        };
        self.blocks[h + 1..]
            .iter()
            .take_while(|b| heading_level(b).is_none_or(|l| l > level))
            .count()
    }

    /// `za`: fold / unfold the section under the cursor (UXI-Buffer-14). Folding
    /// from inside the section lands the cursor on its heading. `Err` names why
    /// nothing happened (no heading above the cursor / an empty section).
    pub(crate) fn toggle_fold_at_cursor(&mut self) -> Result<(), &'static str> {
        let h = self
            .section_heading(self.cursor_block)
            .ok_or("no heading above the cursor")?;
        let key = self.fold_key(h);
        if self.folds.remove(&key).is_none() {
            if self.section_len(h) == 0 {
                return Err("nothing to fold under this heading");
            }
            let id = self.heading_identity(h).ok_or("no heading above the cursor")?;
            self.folds.insert(key, id);
        }
        self.refold();
        self.reveal_block(self.cursor_block);
        Ok(())
    }

    /// `zM`: fold every heading that has a section. Returns how many.
    pub(crate) fn fold_all(&mut self) -> usize {
        let all: Vec<usize> = (0..self.blocks.len())
            .filter(|&i| self.section_len(i) > 0)
            .collect();
        for &h in &all {
            if let Some(id) = self.heading_identity(h) {
                self.folds.insert(self.fold_key(h), id);
            }
        }
        self.refold();
        self.reveal_block(self.cursor_block);
        all.len()
    }

    /// `zR`: unfold everything.
    pub(crate) fn unfold_all(&mut self) {
        self.folds.clear();
        self.refold();
        self.reveal_block(self.cursor_block);
    }

    /// Open every fold hiding block `i` (a jump — outline, local-menu goto,
    /// Edit→Doc landing — that targets a block inside a folded section).
    pub(crate) fn reveal_fold(&mut self, i: usize) {
        while self.is_block_hidden(i) {
            // Hidden runs start right after their (painted) folded heading.
            let Some(owner) = (0..i).rev().find(|&j| !self.is_block_hidden(j)) else {
                return;
            };
            if self.folds.remove(&self.fold_key(owner)).is_none() {
                return; // defensive: layout out of step with folds
            }
            self.refold();
        }
    }

    /// Step `delta` painted blocks from `from` (negative = up), stopping at the
    /// first / last painted block. `None` when no painted block lies that way.
    pub(crate) fn visible_step(&self, from: usize, delta: isize) -> Option<usize> {
        let mut at = from;
        let mut moved = false;
        for _ in 0..delta.unsigned_abs() {
            let next = if delta > 0 {
                (at + 1..self.blocks.len()).find(|&j| !self.is_block_hidden(j))
            } else {
                (0..at).rev().find(|&j| !self.is_block_hidden(j))
            };
            match next {
                Some(j) => {
                    at = j;
                    moved = true;
                }
                None => break,
            }
        }
        moved.then_some(at)
    }

    /// The last painted block (`G`).
    pub(crate) fn last_visible(&self) -> Option<usize> {
        (0..self.blocks.len()).rev().find(|&j| !self.is_block_hidden(j))
    }

    /// `]]`: the next painted heading block after `from`.
    pub(crate) fn heading_after(&self, from: usize) -> Option<usize> {
        (from + 1..self.blocks.len())
            .find(|&j| heading_level(&self.blocks[j]).is_some() && !self.is_block_hidden(j))
    }

    /// `[[`: the nearest painted heading block before `from`.
    pub(crate) fn heading_before(&self, from: usize) -> Option<usize> {
        (0..from.min(self.blocks.len()))
            .rev()
            .find(|&j| heading_level(&self.blocks[j]).is_some() && !self.is_block_hidden(j))
    }

    /// Scroll so block `i` is the first thing in view (the outline jump and
    /// `]]`/`[[` — a heading lands at the top, not merely revealed).
    pub(crate) fn scroll_block_to_top(&self, i: usize) {
        if i < self.list.len() {
            self.list.state().scroll_to(gpui::ListOffset {
                item_ix: i,
                offset_in_item: gpui::px(0.0),
            });
        }
    }
}

impl YaldaGpuiView {
    /// `]]` / `[[` (UXI-Buffer-13): put the Doc cursor on the next / previous
    /// painted heading and scroll it to the top of the view.
    fn doc_heading_jump(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(d) = self.doc_mut() else { return };
        let target = if forward {
            d.heading_after(d.cursor_block)
        } else {
            d.heading_before(d.cursor_block)
        };
        match target {
            Some(h) => {
                d.cursor_block = h;
                d.scroll_block_to_top(h);
            }
            None => {
                self.transient_status = Some(
                    if forward {
                        "no next heading"
                    } else {
                        "no previous heading"
                    }
                    .into(),
                );
            }
        }
        cx.notify();
    }

    pub(crate) fn next_heading(&mut self, _: &NextHeading, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_heading_jump(true, cx);
    }

    pub(crate) fn prev_heading(&mut self, _: &PrevHeading, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_heading_jump(false, cx);
    }

    /// `za` (UXI-Buffer-14).
    pub(crate) fn toggle_fold(&mut self, _: &ToggleFold, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.doc_mut() else { return };
        if let Err(why) = d.toggle_fold_at_cursor() {
            self.transient_status = Some(why.into());
        }
        cx.notify();
    }

    /// `zM` (UXI-Buffer-14).
    pub(crate) fn fold_all(&mut self, _: &FoldAll, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.doc_mut() else { return };
        if d.fold_all() == 0 {
            self.transient_status = Some("no sections to fold".into());
        }
        cx.notify();
    }

    /// `zR` (UXI-Buffer-14).
    pub(crate) fn unfold_all(&mut self, _: &UnfoldAll, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.doc_mut() {
            d.unfold_all();
            cx.notify();
        }
    }

    /// The fold keys (source lines) of the focused Doc — the outline rail marks
    /// those headings folded. Empty when the focus isn't a Doc.
    pub(crate) fn focused_doc_fold_lines(&self) -> Vec<usize> {
        match self.workspace.focused_content() {
            Some(App::Buffer(BufferApp::Viewing(d))) => d.folds.keys().copied().collect(),
            _ => Vec::new(),
        }
    }
}

/// A folded heading's painted row: the heading followed by a muted `… N hidden`
/// marker (UXI-Buffer-14). Chrome-free wrapper so the heading renders exactly
/// as unfolded.
pub(crate) fn folded_heading_row(
    heading: AnyElement,
    hidden: usize,
    idx: usize,
    muted: Hsla,
    text_scale: f32,
) -> AnyElement {
    let marker = div()
        .flex_none()
        .pl_2()
        .pt(px(4.0 * text_scale))
        .text_size(px(13.0 * text_scale))
        .text_color(muted)
        .child(SharedString::from(format!(
            "… {hidden} hidden block{}",
            if hidden == 1 { "" } else { "s" }
        )))
        .into_any_element();
    #[cfg(test)]
    let marker = probe_bounds_dyn(format!("doc-fold-marker-{idx}"), marker);
    #[cfg(not(test))]
    let _ = idx;
    div()
        .flex()
        .flex_row()
        .items_start()
        .w_full()
        .child(div().flex_1().min_w_0().child(heading))
        .child(marker)
        .into_any_element()
}
