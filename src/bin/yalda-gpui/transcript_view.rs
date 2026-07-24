//! `TranscriptView` — the flagship ticket-021 widget: a per-session GPUI view
//! entity that OWNS the transcript's scroll/list UI state, READS its
//! `Entity<AgentSession>` directly in `render()`, and invalidates itself by
//! OBSERVING that session (filtered by monotonic version counters). Embedded by
//! `render_agent` via `cached_child` so a chatbox keystroke — which never moves
//! any transcript slice — re-renders only the root chrome + compose, while the
//! transcript's `render()` is skipped and its prepaint reused.
//!
//! # Why this is the durable fix (project.md root cause)
//!
//! `YaldaGpuiView` is the only GPUI entity, so any `cx.notify()` re-runs its
//! whole render+layout tree. Moving the transcript into its own entity gives
//! the framework per-session invalidation granularity for free: a transcript
//! mutation notifies the session (mutation-site notify, timing-correct per fact
//! 4), the view's `cx.observe(&session)` callback fires in effect flush (fact
//! 5), and IF a slice this render reads actually moved it self-notifies — which
//! is the ONLY thing that lands this view in `dirty_views` and busts the
//! `cached()` reuse (facts 3/6). A keystroke in the compose box leaves every
//! observed seq stable ⇒ no self-notify ⇒ render-skip.
//!
//! # Timing law (fact 4 — load-bearing)
//!
//! `render()` NEVER calls `cx.notify()`. Cache mutations the render performs
//! (lines/highlight/view-model caches on `AgentSession`) go through
//! `session.update(cx, …)` WITHOUT an inner notify — a plain mutation, not an
//! invalidation. Invalidation happens only in the observe callback and at
//! mutation sites, both outside the draw.
//!
//! # Stale-capture hazard (ticket 021 risk)
//!
//! A `cached()` hit reuses the prepaint, whose listeners captured the PRIOR
//! render's data. Interactive rows (tool-group expand, links) therefore act via
//! ids/indices resolved at EVENT time through the root `WeakEntity` —
//! never via captured row data.

use super::*;

/// Perf/test label for the dedicated active-You-block item splice (the
/// "/clear worksheet invisible" fix). Advances once per `build_body` that
/// invalidates the You-block list item because its compose-driven content
/// moved — the invalidation GPUI needs to repaint the just-typed text.
pub(crate) const YOU_BLOCK_SPLICE_LABEL: &str = "you_block_splice";

/// The RESTING row background for a committed transcript line (UXI-AgentTile-4 /
/// UXI-AgentTile-23, ADR-0027): a **user** turn (`TurnId::User`) gets the faint
/// `user_turn_bg` tint so the user's own contributions stand out; agent, tool,
/// and system turns stay on the plain tile background (transparent). The nav-focus
/// cursor-row highlight is applied by the caller and OVERRIDES this on its row, so
/// no row shows two competing fills. Pure so the "which turns get a tint" decision
/// is headlessly testable (the actual painted hue is gap #1).
pub(crate) fn committed_row_bg(tag: Option<TurnId>, user_turn_bg: Hsla) -> Hsla {
    match tag {
        Some(TurnId::User(_)) => user_turn_bg,
        _ => rgba(0x00000000).into(),
    }
}

/// The slice-version watermark the observe filter compares across renders. Each
/// field is a monotonic (or monotonic-equivalent) counter for one input the
/// transcript `render()` reads; the observe callback recomputes the live values
/// and self-notifies iff ANY differs from what was last rendered, logging which
/// slice moved for the `YALDA_PERF` notify-reason counter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct TranscriptSeqs {
    /// Document `edit_seq` — text content (insert/delete; freezing via
    /// `freeze_as_user_turn` is an edit so it bumps this too).
    pub(crate) edit_seq: u64,
    /// Frozen-line set fingerprint — covers the pure `add_frozen_lines` path
    /// (worksheet commit) that does NOT bump `edit_seq`. See
    /// `AgentState::frozen_gen`.
    pub(crate) frozen_gen: u64,
    /// Tool-structure generation — `calls`/`order`/`expanded` mutations (a tool
    /// start, an update, an expand toggle). See `AgentState::tools_gen`.
    pub(crate) tools_gen: u64,
    /// Transcript cursor line — the focused row's bar/caret.
    pub(crate) cursor_line: usize,
    /// Transcript cursor column — the caret position within its row.
    pub(crate) cursor_col: usize,
    /// Worksheet edit mode — `make_caret` draws the under-cursor CHARACTER in
    /// `Normal` vs a BLANK block in `Insert`, so a bare `i`/`a` (or `Esc` at
    /// col 0) flips the caret glyph while moving no other seq. Without this the
    /// observe filter would skip the cached transcript and keep the stale caret.
    pub(crate) mode: EditMode,
    /// Selection range projected onto the transcript (highlight bands).
    pub(crate) selection: Option<((usize, usize), (usize, usize))>,
    /// Whether a reply is streaming — drives the thinking indicator's presence
    /// (its pulse animation is frame-driven; this only gates appear/disappear).
    pub(crate) awaiting: bool,
    /// Worksheet cursor-reveal intent latched by a key handler. A toggle to
    /// `true` must re-render so the reveal is consumed.
    pub(crate) pending_reveal: bool,
    /// User-turn jump reveal intent (agent `.` menu jump mode). Like
    /// `pending_reveal`: a pending jump must re-render so build_body resolves
    /// the ordinal to a flat-item index and scrolls. `is_some()` ⇒ a jump is
    /// queued.
    pub(crate) pending_jump: bool,
    /// Model C §4.5: whether focus is on the transcript. The transcript caret +
    /// cursor-row bar render ONLY when focused; flipping focus must bust the
    /// cache so the caret appears/disappears.
    pub(crate) transcript_focused: bool,
    /// UXI-AgentTile-11 (stage 2): the inline You-block renders the live `Compose` INSIDE
    /// the transcript, so its draft text + caret + mode are render inputs of this
    /// cached view. Without them in the fingerprint, typing into the inline block
    /// would not bust the transcript cache (stale caret/text — the cached-surface
    /// bug class). `compose_edit_seq` covers the text; caret + mode cover the
    /// glyph. Inert (constant) when no block is open.
    pub(crate) you_block_open: bool,
    pub(crate) you_block_anchor: Option<usize>,
    pub(crate) compose_edit_seq: u64,
    pub(crate) compose_cursor: (usize, usize),
    pub(crate) compose_mode: EditMode,
    pub(crate) compose_selection: Option<((usize, usize), (usize, usize))>,
    /// Hash of the PARKED You-blocks (rule 6) — their anchors + text. A park/unpark/
    /// submit changes this so the cached transcript re-renders the inline set. (Copy
    /// fingerprint, so a hash not the Vec.) 0 when idle-chatbox/none.
    pub(crate) parked_fp: u64,
}

impl TranscriptSeqs {
    /// Read the live slice versions off an `AgentState`. Cheap: `frozen_gen` is
    /// O(1) (len + last range), the rest are field reads.
    pub(crate) fn of(c: &AgentState) -> Self {
        let cursor = c.editor.cursor();
        // UXI-AgentTile-11 (stage 2): the compose's text/caret/mode are render inputs of
        // THIS cached view ONLY while the inline You-block is active (worksheet,
        // idle, block open). When the compose renders in the bottom panel instead
        // (chatbox mode, or the mid-turn chatbox), its changes must NOT bust the
        // transcript cache — otherwise chatbox typing re-renders the whole
        // transcript (the `transcript_021_*` perf regression). So zero the compose
        // fields off-inline.
        let inline_block_active = c.inline_you_block_active();
        let (compose_edit_seq, compose_cursor, compose_mode, compose_selection) =
            if inline_block_active {
                let compose = c.input_surface.compose();
                let cc = compose.editor.cursor();
                (
                    compose.editor.document().edit_seq(),
                    (cc.line, cc.col),
                    compose.mode,
                    compose.editor.selection_range(),
                )
            } else {
                (0, (0, 0), EditMode::Normal, None)
            };
        Self {
            edit_seq: c.editor.document().edit_seq(),
            frozen_gen: c.frozen_gen(),
            tools_gen: c.tools_gen(),
            cursor_line: cursor.line,
            cursor_col: cursor.col,
            mode: c.mode,
            selection: c.editor.selection_range(),
            awaiting: c.turn_phase.is_awaiting(),
            pending_reveal: c.pending_reveal_cursor,
            pending_jump: c.pending_jump_ord.is_some(),
            transcript_focused: c.focus == AgentFocus::Transcript,
            you_block_open: inline_block_active,
            you_block_anchor: c.you_block_anchor,
            compose_edit_seq,
            compose_cursor,
            compose_mode,
            compose_selection,
            parked_fp: {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                c.parked_you_blocks.hash(&mut h);
                h.finish()
            },
        }
    }

    /// Which slice moved between `self` (last rendered) and `now`. Returns the
    /// `MissReason` to log, or `None` if the slice the render reads is
    /// unchanged (⇒ no self-notify, ⇒ cache reuse).
    pub(crate) fn diff_reason(&self, now: &TranscriptSeqs) -> Option<MissReason> {
        if self == now {
            None
        } else {
            // All transcript invalidations are "our slice moved"; the gpui-
            // internal Bounds/TextStyle reasons are stamped elsewhere (resize,
            // zoom). One label is enough for the audit trail.
            Some(MissReason::Dirtied)
        }
    }

    /// A u64 digest of the whole fingerprint, used to KEY the cached transcript
    /// element's `GlobalElementId` (see `render_agent`). This is the durable
    /// backstop for the dropped-self-notify class: the observe→`cx.notify()`
    /// hop silently no-ops when the view has no `view_path` in the committed
    /// frame (`mark_view_dirty`, gpui window.rs), so the cached prepaint is
    /// reused STALE — the "last message never renders" bug. By folding this
    /// digest into the element id, a moved fingerprint yields a fresh
    /// `GlobalElementId` ⇒ `with_element_state` misses ⇒ `render()` is forced,
    /// with NO dependence on `mark_view_dirty`/`view_path`. The self-notify path
    /// stays the fast O(changed) invalidation; this only closes the hole when a
    /// notify is dropped. Idle typing elsewhere leaves the fingerprint (and thus
    /// the id) stable ⇒ cache hit ⇒ render-skip is preserved.
    pub(crate) fn fingerprint_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// Per-session transcript widget (ticket 021). One per session; the 1:1
/// session↔tile invariant means multi-tile splits need no extra logic.
pub(crate) struct TranscriptView {
    /// The domain model, read directly in `render()` via `.read(cx)` — no
    /// snapshot copy, no push protocol. Also observed at construction.
    pub(crate) session: Entity<AgentSession>,
    /// Weak handle to the root view: render reads it for the GLOBAL theme /
    /// fonts / zoom (not session state), and interactive rows re-enter through
    /// it at event time (stale-capture-safe — ids resolved on click).
    pub(crate) root: WeakEntity<YaldaGpuiView>,
    /// The scroll/list UI state moved out of `AgentState` (widgets own UI
    /// state; models own domain state).
    pub(crate) scroll: TranscriptScroll,
    /// The slice versions this view last rendered. The observe callback
    /// compares against the live values and self-notifies only on a real move.
    pub(crate) last_rendered: TranscriptSeqs,
    /// Stable perf-counter label (per session) so headless render-count
    /// assertions and the `YALDA_PERF` trace can name this transcript.
    pub(crate) perf_label: &'static str,
    /// Select-to-clipboard hit-test sink: every painted text token pushes its
    /// window-space bounds + covered `(line, char)` range here at paint time.
    /// Cleared at the top of `build_body`; refilled at paint. Mouse handlers
    /// read it (`hit_test_tokens`) to map a click/drag point to a transcript
    /// position. Shared `Rc` so the render closure and the handlers see the
    /// same Vec.
    pub(crate) token_hits: std::rc::Rc<RefCell<Vec<TokenHit>>>,
    /// Whether a mouse drag-select is in progress (widget UI state). While set,
    /// the caret is suppressed so EVERY visible line renders via the uniform
    /// (registerable) non-cursor path and the selection band shows instead.
    pub(crate) dragging: bool,
}

impl TranscriptView {
    /// Construct a transcript view for `session` and register the
    /// `cx.observe(&session)` subscription that self-notifies on a slice move.
    /// The follow-output scroll handler is wired onto the fresh `ListState`.
    pub(crate) fn new(
        session: Entity<AgentSession>,
        root: WeakEntity<YaldaGpuiView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroll = TranscriptScroll::new();
        // Wire the follow-output handler: the session owns the `follow_output`
        // intent flag; the list lives here.
        {
            let follow = session.read(cx).follow_output.clone();
            setup_list_follow_handler(&scroll.list_state, &follow);
        }
        // INVALIDATION (project.md facts 4–5): observe the session; the callback
        // runs in effect flush (outside the draw). Compare the slice versions
        // the render reads against what was last rendered, and self-notify ONLY
        // when a slice moved — logging the reason for the notify counter.
        cx.observe(&session, |this: &mut TranscriptView, session, cx| {
            let now = TranscriptSeqs::of(&session.read(cx).state);
            let diff = this.last_rendered.diff_reason(&now);
            // Diagnostic for the recurring "/clear worksheet invisible" bug: with
            // YALDA_CLEAR_DEBUG=1, log EVERY observe fire, whether or not a slice
            // moved. If the user types after /clear and this prints "moved=NONE"
            // (no re-render) while inline-active flips, the keystroke isn't busting
            // the cache — the invisible-text bug, caught on the REAL path.
            {
                let c = &session.read(cx).state;
                crate::clear_log(&format!(
                    "transcript OBSERVE: moved={:?} inline_active={} focus_compose={} \
                     you_block_open={} awaiting={} chatbox={} compose_edit_seq={}->{} \
                     transcript_focused_seq={}",
                    diff,
                    c.inline_you_block_active(),
                    c.focus == AgentFocus::Compose,
                    c.you_block_open,
                    c.turn_phase.is_awaiting(),
                    c.input_surface.is_chatbox(),
                    this.last_rendered.compose_edit_seq,
                    now.compose_edit_seq,
                    now.transcript_focused,
                ));
            }
            if let Some(reason) = diff {
                record_notify(this.perf_label, reason);
                cx.notify();
            }
        })
        .detach();
        Self {
            session,
            root,
            scroll,
            last_rendered: TranscriptSeqs::default(),
            perf_label: "transcript",
            token_hits: std::rc::Rc::new(RefCell::new(Vec::new())),
            dragging: false,
        }
    }
}

impl Render for TranscriptView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(self.perf_label);
        // Build the transcript body first; it consumes the worksheet
        // `pending_reveal_cursor` intent inside its session update. THEN stamp
        // `last_rendered` from a fresh read so the observe filter compares
        // against the POST-render state (otherwise consuming `pending_reveal`
        // mid-render would leave the watermark stale and trigger one spurious
        // re-render on the next notify). Never notifies (timing law, fact 4).
        let body = self.build_body(cx);
        self.last_rendered = TranscriptSeqs::of(&self.session.read(cx).state);
        body
    }
}

impl TranscriptView {
    /// Map a window point to a transcript `(line, char)` via the painted-token
    /// sink, clamping the column to the line's real length.
    fn transcript_pos_at(
        &self,
        cx: &Context<Self>,
        pt: gpui::Point<Pixels>,
    ) -> Option<(usize, usize)> {
        let (line, col) = hit_test_tokens(pt, &self.token_hits.borrow())?;
        let line_len = self
            .session
            .read(cx)
            .state
            .editor
            .document()
            .line_len_chars(line);
        Some((line, col.min(line_len)))
    }

    /// Begin a mouse drag-select: focus the transcript, place the cursor at the
    /// hit and drop the selection anchor there. A click on empty space (no token
    /// hit) clears any existing selection. (UXI-Selection-1.)
    pub(crate) fn transcript_mouse_down(&mut self, ev: &gpui::MouseDownEvent, cx: &mut Context<Self>) {
        let Some((line, col)) = self.transcript_pos_at(cx, ev.position) else {
            self.session.update(cx, |sp, _| {
                sp.state.editor.clear_selection();
                sp.state.drag_protect_line = None;
            });
            self.dragging = false;
            cx.notify();
            return;
        };
        self.session.update(cx, |sp, _| {
            let c = &mut sp.state;
            // bug-0015: freeze the blank-collapse's protected line at its
            // PRE-press value for the whole gesture. Moving the cursor below
            // would otherwise un-protect the old line, drop a flat item and
            // reflow the transcript ~25px under the pointer mid-drag.
            c.drag_protect_line = Some(c.editor.cursor().line);
            c.focus = AgentFocus::Transcript;
            c.editor.cursor_mut().line = line;
            c.editor.cursor_mut().col = col;
            c.editor.anchor_at_cursor();
        });
        self.dragging = true;
        cx.notify();
    }

    /// Extend the drag-select head to the current point (anchor stays put).
    pub(crate) fn transcript_mouse_move(&mut self, ev: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        let Some((line, col)) = self.transcript_pos_at(cx, ev.position) else {
            return;
        };
        let moved = self.session.update(cx, |sp, _| {
            let cur = sp.state.editor.cursor();
            if cur.line == line && cur.col == col {
                return false;
            }
            sp.state.editor.cursor_mut().line = line;
            sp.state.editor.cursor_mut().col = col;
            true
        });
        if moved {
            cx.notify();
        }
    }

    /// Finish the drag: X11-style, a non-empty selection auto-copies to the
    /// system clipboard; an empty one (a bare click) is dropped. (UXI-Selection-1.)
    pub(crate) fn transcript_mouse_up(&mut self, _ev: &gpui::MouseUpEvent, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        let text = self.session.update(cx, |sp, _| {
            let c = &mut sp.state;
            // Gesture over: the collapse pass tracks the live cursor again.
            c.drag_protect_line = None;
            match c.editor.selection_range() {
                Some((a, b)) if a != b => c.editor.selection_text(),
                _ => {
                    c.editor.clear_selection();
                    None
                }
            }
        });
        if let Some(text) = text
            && !text.is_empty()
        {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    /// Build the virtualised transcript list element (the ticket-021 seam: the
    /// row-data build + `render_fn` + `gpui::list` relocated here from
    /// `render_agent`). Reads GLOBAL theme/fonts off the root view; reads + (for
    /// caches only) mutates the session via `session.update` WITHOUT notifying
    /// (timing law); reconciles + re-reveals through `self.scroll`.
    fn build_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // The root view holds the GLOBAL theme / fonts / zoom (not session
        // state). If it's gone (teardown), render an empty sized placeholder —
        // never panic on a transient weak miss.
        let Some(root_ent) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };

        // Select-to-clipboard hit-test sink: refilled every paint from the
        // freshly built rows, so clear the previous frame's tokens before the
        // list rebuilds. `dragging` suppresses the caret so every visible line
        // takes the uniform, registerable non-cursor render path.
        let token_sink = self.token_hits.clone();
        token_sink.borrow_mut().clear();
        let dragging = self.dragging;

        // Snapshot the root-owned render inputs into OWNED locals, releasing the
        // root read borrow before any `session.update` (which needs `&mut cx`).
        let RootSnapshot {
            theme,
            agent_theme: at_snap,
            code_font,
            body_font,
            editor_fg,
            base_style,
            editor_fg_u32,
            frozen_fg_u32,
            syntect_hl,
            show_heading_markers,
            text_scale,
        } = {
            let root = root_ent.read(cx);
            RootSnapshot {
                theme: root.theme.clone(),
                agent_theme: root.theme.agent.clone(),
                code_font: root.code_font.clone(),
                body_font: root.body_font.clone(),
                editor_fg: root.editor_fg(),
                base_style: root.theme.paragraph,
                editor_fg_u32: ncolor_to_u32(root.theme.editor_fg, DEFAULT_FG),
                frozen_fg_u32: ncolor_to_u32(root.theme.agent.frozen_fg, DEFAULT_FG),
                syntect_hl: root.syntect_hl.clone(),
                show_heading_markers: root.show_agent_heading_markers,
                text_scale: root.text_scale,
            }
        };

        // Theme-derived colors used by the row builder (computed from the
        // snapshot, no root borrow held).
        let cursor_color: Hsla = nc(at_snap.cursor);
        let dim_fg: Hsla = nc(at_snap.dim);
        let frozen_bar: Hsla = nc(at_snap.frozen_bar);
        let user_bar: Hsla = nc(at_snap.user_bar);

        // Weak handle for the interactive rows' click listeners. Captured here
        // and resolved (with event-time ids) on click — stale-capture-safe.
        let weak_self = self.root.clone();

        // ── Heavy prep inside the session update (cache mutation only; NO
        // notify — timing law). Returns the snapshots the render closure needs
        // plus the values reconcile/reveal consume. ──
        let session = self.session.clone();
        let TranscriptPrep {
            flat_items_arc,
            gutter_tag_snap,
            lines_snap,
            hl_snap,
            tool_calls_snap,
            expanded_snap,
            frozen_lines_snap,
            lockable_through_snap,
            sel_snap,
            mode_snap,
            cursor_line,
            cursor_col,
            turn_started_snap,
            last_event_at_snap,
            new_count,
            edit_seq,
            block_ranges_active,
            block_ranges_snap,
            follow_tail,
            pending_reveal_line,
            you_block_snap,
            you_parked_snap,
            you_wrap_cols,
            you_block_seq,
        } = session.update(cx, |sp, _scx| {
            let c: &mut AgentState = &mut sp.state;

            // Model C §4.5: the transcript caret + cursor-row bar + selection band
            // render ONLY when focus is on the transcript. Off-focus, a sentinel
            // line (never matches any row) suppresses the caret/row-bar, and the
            // selection band is dropped.
            let transcript_focused = c.focus == AgentFocus::Transcript;
            let cursor = c.editor.cursor();
            // Suppress the caret mid drag-select (see `dragging`) so every
            // visible line renders via the registerable non-cursor path.
            let cursor_line = if transcript_focused && !dragging {
                cursor.line
            } else {
                usize::MAX
            };
            let cursor_col = if transcript_focused && !dragging {
                cursor.col
            } else {
                0
            };
            let line_count = c.editor.document().line_count();
            let edit_seq = c.editor.document().edit_seq();

            // Perf: only re-extract the per-line transcript text when the
            // document actually changed (cached `Rc` reuse on idle frames).
            let lines_rc: std::rc::Rc<Vec<String>> = if c.lines_cache_seq == edit_seq {
                c.lines_cache.clone()
            } else {
                let built: Vec<String> = (0..line_count.max(1))
                    .map(|i| {
                        c.editor
                            .document()
                            .line_text(i)
                            .trim_end_matches('\n')
                            .replace('\t', "    ")
                    })
                    .collect();
                let rc = std::rc::Rc::new(built);
                c.lines_cache = rc.clone();
                c.lines_cache_seq = edit_seq;
                rc
            };
            let lines: &Vec<String> = &lines_rc;

            // Per-line highlight (incremental cache or full bypass).
            let hl_snap: std::rc::Rc<Vec<std::rc::Rc<LineHl>>> = if hl_cache_enabled() {
                c.highlight_cache
                    .snapshot_syn(lines, &theme, edit_seq, &syntect_hl)
            } else {
                let raw = highlight_markdown_lines_syn(lines, &theme, &syntect_hl);
                let stripped = highlight_markdown_lines_stripped_syn(lines, &theme, &syntect_hl);
                std::rc::Rc::new(
                    raw.into_iter()
                        .zip(stripped)
                        .map(|(raw, stripped)| std::rc::Rc::new(LineHl { raw, stripped }))
                        .collect(),
                )
            };

            // Frozen ranges drive both the structural-block cache and the
            // view-model fingerprint.
            let frozen_ranges: Vec<(usize, usize)> = c.editor.frozen_lines().to_vec();
            let frozen_line_count: usize = frozen_ranges.iter().map(|(s, e)| e - s).sum();

            // ── View-model memoization (S1) ──
            let view_model_fp: u64 = c.view_model_fingerprint(line_count, frozen_line_count);
            let memo_hit = c.view_model.cached(view_model_fp).is_some();
            let (flat_items_arc, gutter_tag_snap) = match c.view_model.cached(view_model_fp) {
                Some(hit) => hit,
                None => rebuild_agent_view_model(c, lines, &frozen_ranges, &theme, view_model_fp),
            };

            // TEMP diagnostic (/tmp/yalda-clear-debug.log): what the transcript
            // ACTUALLY builds — whether a live inline YouBlock row is present, the
            // memo hit/miss, and the gate. If a keystroke leaves `has_you_block=false`
            // while inline-active, the render (not the gate) is the break.
            let has_you_block = flat_items_arc
                .iter()
                .any(|it| matches!(it, FlatItem::YouBlock { parked: None }));
            crate::clear_log(&format!(
                "build_body: inline_active={} focus_compose={} you_block_open={} \
                 has_YouBlock_row={has_you_block} flat_items={} memo_hit={memo_hit} fp={view_model_fp}",
                c.inline_you_block_active(),
                c.focus == AgentFocus::Compose,
                c.you_block_open,
                flat_items_arc.len(),
            ));

            let new_count = flat_items_arc.len();
            let block_ranges_active = !c.block_ranges.is_empty();
            let follow_tail = c.follow_tail();

            // Worksheet cursor-reveal AND user-turn-jump intent (INV-RV):
            // consume here, return the target item index so `self.scroll` can
            // issue the scroll after the update. A queued jump (agent `.` menu
            // jump mode) takes precedence — it resolves the stored ordinal to
            // the Nth user `TurnHeader` index in the FRESH flat-item list (tail
            // growth keeps earlier indices stable, but resolving here mirrors
            // `item_for_line`'s render-time discipline).
            let pending_reveal_line = if let Some(ord) = c.pending_jump_ord.take() {
                if std::mem::take(&mut c.pending_jump_end) {
                    // `j` past the newest turn → reveal the buffer's page end
                    // (the last flat item: latest output + the editable tail).
                    Some(flat_items_arc.len().saturating_sub(1))
                } else {
                    user_turn_item_indices(&flat_items_arc).get(ord).copied()
                }
            } else if c.pending_reveal_cursor {
                c.pending_reveal_cursor = false;
                // UXI-TextEditing-1 (stage 2): while the inline You-block is open the edit
                // caret lives in the BLOCK, below its anchor line — so reveal the
                // YouBlock item itself (it grows as you type), not the anchor line
                // above it, or a multi-line reply's caret scrolls below the fold.
                if c.inline_you_block_active() {
                    flat_items_arc
                        .iter()
                        .position(|it| matches!(it, FlatItem::YouBlock { parked: None }))
                        .or_else(|| Some(c.view_model.item_for_line(c.editor.cursor().line)))
                } else {
                    let cl = c.editor.cursor().line;
                    Some(c.view_model.item_for_line(cl))
                }
            } else {
                None
            };

            // Snapshots for the render closure (O(1) pointer clones).
            let lines_snap = lines_rc.clone();
            let tool_calls_snap = c.tools.calls_snapshot();
            let expanded_snap = c.tools.expanded_snapshot();
            let lockable_through_snap = c.editor.lockable_through_line();
            // Selection band renders only when the transcript is focused (§4.5).
            let sel_snap = if transcript_focused {
                c.editor.selection_range()
            } else {
                None
            };
            let mode_snap = c.mode;
            let turn_started_snap = c.turn_phase.turn_started();
            let last_event_at_snap = c.turn_phase.last_event_at();

            // UXI-AgentTile-11 (stage 2): snapshot the inline You-block draft (the separate
            // Compose) so the render arm draws it without re-borrowing the session.
            // Gate EXACTLY like the injection (agent.rs) — `you_block_open` alone
            // would allocate a snapshot every streaming-frame for a block left open
            // mid-turn that is never injected (bug-hunt 11).
            // Anchor → the doc line the block highlights on in nav (tail = last line).
            let last_line = c.editor.document().line_count().saturating_sub(1);
            let you_block_snap = if c.inline_you_block_active() {
                let compose = c.input_surface.compose();
                let cc = compose.editor.cursor();
                let n = compose.editor.document().line_count().max(1);
                Some(YouBlockSnap {
                    lines: (0..n)
                        .map(|i| {
                            compose
                                .editor
                                .document()
                                .line_text(i)
                                .trim_end_matches('\n')
                                .replace('\t', "    ")
                        })
                        .collect(),
                    cursor_line: cc.line,
                    cursor_col: cc.col,
                    mode: compose.mode,
                    anchor_line: c.effective_you_block_anchor().unwrap_or(last_line),
                    focused: c.focus == AgentFocus::Compose,
                    selection: compose.editor.selection_range(),
                    bounds: compose.bounds.clone(),
                })
            } else {
                None
            };
            // Render-input hash of the ACTIVE You-block, driving the dedicated
            // list-item splice below. Folds EXACTLY the fields the `YouBlock`
            // render arm reads (text via `edit_seq`, caret, mode, selection) — a
            // move in any of them changes the drawn element, but none bump the
            // transcript `edit_seq` that `reconcile_list` keys on. 0 when no block
            // is active (the item isn't present, so no splice is owed).
            let you_block_seq = if let Some(snap) = &you_block_snap {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                c.input_surface.compose().editor.document().edit_seq().hash(&mut h);
                snap.cursor_line.hash(&mut h);
                snap.cursor_col.hash(&mut h);
                (snap.mode == EditMode::Insert).hash(&mut h);
                snap.selection.hash(&mut h);
                // Non-zero even for an empty just-opened block so the FIRST
                // keystroke (edit_seq 0→1) is a genuine move off `u64::MAX`.
                let v = h.finish();
                if v == 0 { 1 } else { v }
            } else {
                0
            };
            // Parked You-blocks (rule 6): (anchor_line, read-only text lines) per
            // additional insertion point, rendered by the `YouBlock { parked: Some(i)
            // }` arm. Cheap; only when idle worksheet so it can't allocate mid-turn.
            let you_parked_snap: Vec<(usize, Vec<String>)> =
                if !c.input_surface.is_chatbox() && !c.turn_phase.is_awaiting() {
                    c.parked_you_blocks
                        .iter()
                        .map(|(anchor, text)| {
                            let al = anchor
                                .filter(|&l| c.you_block_anchor_is_legal(l))
                                .unwrap_or(last_line);
                            let lines = text
                                .split('\n')
                                .map(|l| l.replace('\t', "    "))
                                .collect();
                            (al, lines)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            // Measured wrap width (cols) of the compose's box — PARKED blocks render
            // read-only with no `CaptureBounds` of their own, so they reuse this so
            // they wrap at the real column width, not a narrow 40-col fallback (the
            // "strange wordwrap length" once a block parks). 0 = unmeasured.
            let you_wrap_cols = {
                let w = c.input_surface.compose().bounds.get().2;
                if w > 1.0 {
                    (w / crate::CHATBOX_CHAR_W).floor().max(1.0) as usize
                } else {
                    0
                }
            };

            TranscriptPrep {
                flat_items_arc,
                gutter_tag_snap,
                lines_snap,
                hl_snap,
                tool_calls_snap,
                expanded_snap,
                frozen_lines_snap: frozen_ranges,
                lockable_through_snap,
                sel_snap,
                mode_snap,
                cursor_line,
                cursor_col,
                turn_started_snap,
                last_event_at_snap,
                new_count,
                edit_seq,
                block_ranges_active,
                // bug-0017: pair against PARSED-only ranges, in emit order. A
                // `FlatItem::Block` is emitted only for a range that parsed
                // (agent.rs); `c.block_ranges` also holds detected-but-unparsed
                // ranges (rendered as Lines), so cloning it would shift every
                // later block's hit range by one — wrong bands, wrong `raw_base`.
                block_ranges_snap: c
                    .view_model
                    .resolved_blocks
                    .iter()
                    .filter(|(_, b)| b.is_some())
                    .map(|(r, _)| *r)
                    .collect(),
                follow_tail,
                pending_reveal_line,
                you_block_snap,
                you_parked_snap,
                you_wrap_cols,
                you_block_seq,
            }
        });

        // Per-flat-item raw line range for each `FlatItem::Block`, paired in the
        // SAME ascending order the renderer emits blocks (bug-0008). Lets the block
        // render arm register per-raw-line hit-test bands so tables/lists/code are
        // mouse-selectable. Built before `reconcile_list` borrows `block_ranges_active`.
        let block_ranges_by_item: std::rc::Rc<Vec<Option<(usize, usize)>>> = {
            let mut v = vec![None; flat_items_arc.len()];
            let mut bi = 0usize;
            for (i, it) in flat_items_arc.iter().enumerate() {
                if matches!(it, FlatItem::Block(_)) {
                    v[i] = block_ranges_snap.get(bi).copied();
                    bi += 1;
                }
            }
            std::rc::Rc::new(v)
        };

        // ── Reconcile (count parity → splice/reset) + follow-scroll, on the
        // view-owned `TranscriptScroll`. The session borrow is dropped. ──
        self.scroll
            .reconcile_list(block_ranges_active, &flat_items_arc, edit_seq);
        // The ACTIVE inline You-block is ONE list item whose content is driven by
        // the COMPOSE buffer, not the transcript `edit_seq` — so `reconcile_list`
        // (which keys the tail re-measure on `edit_seq`, and `FlatKey::YouBlock` on
        // `parked` only) never marks it dirty when you type into it. GPUI caches
        // rendered list items, so without an explicit splice it repaints the block
        // at its STALE text: the recurring "/clear worksheet invisible" bug — the
        // observe fires and `build_body` runs, but the typed char never appears
        // until an unrelated event (jump bar, chatbox toggle) forces a splice. When
        // the block's render-input hash moves, splice exactly its item so GPUI
        // re-measures it. Targets `YouBlock { parked: None }` (the active block; a
        // parked block's text is frozen). Serves UXI-TextEditing-1 — the caret + its text
        // stay visible as you type. Pinned by
        // `clear_worksheet_you_block_keystroke_splices_item`.
        if you_block_seq != self.scroll.last_you_block_seq {
            if let Some(yb_idx) = flat_items_arc
                .iter()
                .position(|it| matches!(it, FlatItem::YouBlock { parked: None }))
            {
                self.scroll.list_state.splice(yb_idx..yb_idx + 1, 1);
                // Test/perf seam: a splice here IS the fix — it's the invalidation
                // GPUI needs to repaint the You-block with the just-typed text.
                // `clear_worksheet_you_block_keystroke_splices_item` asserts this
                // count advances on the real keystroke path (RED when reverted).
                record_render(YOU_BLOCK_SPLICE_LABEL);
            }
            self.scroll.last_you_block_seq = you_block_seq;
        }
        debug_assert!(
            self.scroll.list_item_count == flat_items_arc.len(),
            "list_item_count ({}) out of sync with flat_items ({})",
            self.scroll.list_item_count,
            flat_items_arc.len(),
        );
        self.scroll
            .reveal_tail_if_following(follow_tail, edit_seq, new_count);
        if let Some(target) = pending_reveal_line {
            // The active You-block is ONE (now unwindowed, growing) list item. Item-
            // granular reveal top-aligns it and strands the caret below the fold on a
            // tall block. So when the reveal targets the active block, scroll to the
            // caret's VISUAL ROW within it, parked ~2 rows above the viewport bottom —
            // the doc-authoring feel (you type at the tail; earlier lines flow up),
            // and UXI-TextEditing-1 holds for any block height. Pinned by
            // `worksheet_tall_you_block_grows_caret_painted_in_viewport`.
            let active_yb_item = flat_items_arc
                .iter()
                .position(|it| matches!(it, FlatItem::YouBlock { parked: None }));
            let reveal_caret_row = you_block_snap
                .as_ref()
                .filter(|yb| yb.focused && Some(target) == active_yb_item);
            if let Some(yb) = reveal_caret_row {
                let wrap_cols = {
                    let bw = yb.bounds.get().2;
                    if bw > 1.0 {
                        (bw / crate::CHATBOX_CHAR_W).floor().max(1.0) as usize
                    } else if you_wrap_cols > 0 {
                        you_wrap_cols
                    } else {
                        40
                    }
                };
                let (caret_vrow, _, _) = crate::compose_visual_metrics(
                    &yb.lines,
                    yb.cursor_line,
                    yb.cursor_col,
                    wrap_cols,
                );
                // Block chrome above the first content row (pt_2 + the "You" label).
                const YB_HEADER_PX: f32 = 34.0;
                const YB_LINE_H: f32 = 18.0;
                let caret_off = YB_HEADER_PX + caret_vrow as f32 * YB_LINE_H;
                let vh = f32::from(self.scroll.list_state.viewport_bounds().size.height);
                let want_from_top = (vh - YB_LINE_H * 2.0).max(0.0);
                let offset = (caret_off - want_from_top).max(0.0);
                self.scroll.list_state.scroll_to(gpui::ListOffset {
                    item_ix: target,
                    offset_in_item: gpui::px(offset),
                });
            } else {
                self.scroll.list_state.scroll_to_reveal_item(target);
            }
        }

        // Helper: "is this line in a frozen range" — inlined into the closure.
        let is_frozen_at = move |line_idx: usize, ranges: &[(usize, usize)]| -> bool {
            ranges.iter().any(|&(s, e)| line_idx >= s && line_idx < e)
        };

        // UXI-AgentTile-11 (stage 2): the inline You-block snapshot is shared into the
        // per-item render closure by refcount (it owns Vecs, so not `Copy`).
        let you_block_snap = std::rc::Rc::new(you_block_snap);
        let you_parked_snap = std::rc::Rc::new(you_parked_snap);

        let render_fn = {
            let flat_items = flat_items_arc.clone();
            let weak_self = weak_self.clone();
            let you_block_snap = you_block_snap.clone();
            let you_parked_snap = you_parked_snap.clone();
            let you_wrap_cols = you_wrap_cols;
            let lines_snap = lines_snap.clone();
            let hl_snap = hl_snap.clone();
            let frozen_lines_snap = frozen_lines_snap.clone();
            let tool_calls_snap = tool_calls_snap.clone();
            let expanded_snap = expanded_snap.clone();
            let code_font_snap = code_font.clone();
            let body_font_snap = body_font.clone();
            let show_heading_markers = show_heading_markers;
            let theme_snap = theme.clone();
            let at_snap = at_snap.clone();
            let self_editor_fg = editor_fg;
            let token_sink_snap = token_sink.clone();
            let block_ranges_by_item = block_ranges_by_item.clone();
            move |idx: usize, _w: &mut Window, _app: &mut GpuiApp| -> AnyElement {
                let item = &flat_items[idx];
                match item {
                    FlatItem::Line(line_idx) => {
                        let line_idx = *line_idx;
                        let line_str = lines_snap.get(line_idx).cloned().unwrap_or_default();
                        let is_frozen = is_frozen_at(line_idx, &frozen_lines_snap);
                        let is_locked = line_idx < lockable_through_snap;
                        let _ = is_locked; // kept for future visual cue parity

                        let mut segs: Vec<Segment> = match hl_snap.get(line_idx) {
                            Some(hl) if is_frozen => hl.stripped.clone(),
                            Some(hl) => hl.raw.clone(),
                            None => vec![(line_str.clone(), base_style)],
                        };
                        let author_tint: NColor = if is_frozen {
                            at_snap.agent_tint
                        } else {
                            at_snap.user_tint
                        };
                        for (_text, style) in segs.iter_mut() {
                            if *style == base_style {
                                *style = style.fg(author_tint);
                            }
                        }
                        if let Some(sel) = sel_snap {
                            let line_chars = line_str.chars().count();
                            if let Some((s, e_col)) =
                                line_selection_range(sel, line_idx, line_chars)
                                && e_col > s
                            {
                                // The editor selection is in RAW document columns; on a
                                // frozen (stripped) line the segs are rendered, so map
                                // the band into rendered space so it lines up with the
                                // painted text (bug-0006). Identity for raw lines.
                                let (bs, be) = if is_frozen {
                                    let rendered: String =
                                        segs.iter().map(|(t, _)| t.as_str()).collect();
                                    if rendered == line_str {
                                        (s, e_col)
                                    } else {
                                        let map =
                                            crate::stripped_to_raw_cols(&line_str, &rendered);
                                        (
                                            crate::raw_to_stripped_col(&map, s),
                                            crate::raw_to_stripped_col(&map, e_col),
                                        )
                                    }
                                } else {
                                    (s, e_col)
                                };
                                if be > bs {
                                    segs =
                                        apply_selection_bg(&segs, bs, be, at_snap.selection_bg);
                                }
                            }
                        }

                        let line_base_fg = if is_frozen {
                            frozen_fg_u32
                        } else {
                            editor_fg_u32
                        };
                        let content = build_wrapped_line(
                            &segs,
                            &line_str,
                            line_idx == cursor_line,
                            cursor_col,
                            mode_snap,
                            cursor_color,
                            base_style,
                            line_base_fg,
                            // Transcript prose renders in the PROPORTIONAL body
                            // font (`styled_line_element` keeps inline-code spans
                            // on `code_font`). The compose box stays monospace —
                            // its caret containment (UXI-TextEditing-1) depends on a fixed
                            // char width.
                            &body_font_snap,
                            &code_font_snap,
                            // Exclude the selection bg from the code-font proxy so
                            // highlighting prose doesn't turn it monospace.
                            Some(at_snap.selection_bg),
                            Some(&token_sink_snap),
                            line_idx,
                        );

                        let line_has_content = !line_str.trim().is_empty();
                        let bar_color: Hsla = if is_frozen {
                            frozen_bar
                        } else if line_has_content {
                            user_bar
                        } else {
                            rgba(0x00000000).into()
                        };
                        let line_text_color = if is_frozen {
                            nc(at_snap.frozen_fg)
                        } else {
                            self_editor_fg
                        };

                        let tag = gutter_tag_snap.get(line_idx).copied().flatten();
                        let prev_tag = if line_idx > 0 {
                            gutter_tag_snap.get(line_idx - 1).copied().flatten()
                        } else {
                            None
                        };
                        let is_first_in_turn = tag != prev_tag;
                        let (label_text, label_color): (SharedString, Hsla) = if !is_first_in_turn {
                            ("   ".into(), dim_fg)
                        } else {
                            match tag {
                                Some(TurnId::Llm(n)) => (format!("{:>3}", n).into(), frozen_bar),
                                Some(TurnId::User(n)) => {
                                    (format!("{:>3}", format!("U{}", n)).into(), user_bar)
                                }
                                Some(TurnId::Tool(n)) => (
                                    format!("{:>3}", format!("T{}", n)).into(),
                                    nc(at_snap.tool_label),
                                ),
                                Some(TurnId::System) | None => ("   ".into(), dim_fg),
                            }
                        };
                        // Row background: the transient nav-focus highlight on the
                        // cursor row wins (shown ONLY while the transcript is focused
                        // for navigation — `cursor_line == usize::MAX` otherwise, so
                        // no row matches during compose). Off the cursor row, a USER
                        // turn gets the faint `user_turn_bg` tint (UXI-AgentTile-23,
                        // ADR-0027) while agent/tool/system turns stay on the plain
                        // tile background (UXI-AgentTile-4).
                        let row_bg: Hsla = if line_idx == cursor_line {
                            let mut h = nc(at_snap.dim);
                            h.a = 0.2;
                            h
                        } else {
                            committed_row_bg(tag, nc(at_snap.user_turn_bg))
                        };

                        // UXI-ParagraphSpacing-1: a COMMITTED (frozen) prose line that
                        // STARTS a new paragraph gets the readability gap as top padding,
                        // so agent/user prose paragraphs break apart — mirroring the block
                        // gap and WP's blank row. A paragraph start is a non-blank line
                        // whose previous SOURCE line is blank (`lines_snap` keeps that
                        // blank even though the blank-collapse pass drops its FlatItem, so
                        // the two paragraphs would otherwise render adjacent). Within-
                        // paragraph soft breaks (previous line non-blank) are untouched,
                        // and the live draft/compose (unfrozen) is excluded — option B.
                        let prev_source_blank = line_idx > 0
                            && lines_snap
                                .get(line_idx - 1)
                                .map(|l| l.trim().is_empty())
                                .unwrap_or(false);
                        let is_paragraph_break =
                            is_frozen && !line_str.trim().is_empty() && prev_source_blank;
                        let mut row = div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .w_full()
                            .py(px(2.0))
                            .bg(row_bg)
                            // UXI-TextZoom-1: scale the conversation prose on the line's
                            // own wrapper (the `claude-body` ambient does NOT reach
                            // across the `gpui::list` item boundary — the working
                            // doc/WP views set the size on each line wrapper too).
                            // The fixed-size gutter child below overrides this.
                            .text_size(px(13.0 * text_scale))
                            .text_color(line_text_color)
                            .child(
                                div()
                                    .w(px(28.0))
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .text_color(label_color)
                                    .font_family(code_font_snap.clone())
                                    .pr_1()
                                    .child(label_text),
                            )
                            .child(div().w(px(3.0)).flex_none().bg(bar_color).mr_2())
                            // Probe the FIRST line's painted content so the harness
                            // can prove the prose height grows with zoom (UXI-TextZoom-1)
                            // — turning the font-px check from a human gap into a
                            // headless guard (`transcript_prose_scales_with_zoom`).
                            .child(if line_idx == 0 {
                                crate::probe_bounds("transcript-line0", content)
                            } else {
                                content
                            });
                        if is_paragraph_break {
                            // Add the gap ON TOP of the row's base 2px top padding
                            // (`.pt` replaces, not adds), so the net increase over a
                            // within-paragraph row is the full paragraph gap.
                            row = row.pt(px(2.0) + crate::paragraph_gap(text_scale));
                        }
                        let row = row.into_any_element();
                        // Per-row bounds probe so the harness can measure the
                        // paragraph-break row's extra height (UXI-ParagraphSpacing-1).
                        #[cfg(test)]
                        let row = crate::probe_bounds_dyn(format!("transcript-row-{line_idx}"), row);
                        row
                    }
                    FlatItem::ToolGroup { anchor_line, ids } => {
                        let anchor = *anchor_line;
                        let calls: Vec<&yalda::acp_channel::ToolCall> = ids
                            .iter()
                            .filter_map(|id| tool_calls_snap.get(id))
                            .collect();
                        if calls.is_empty() {
                            return div().h(px(0.0)).into_any_element();
                        }
                        let group_expanded = expanded_snap.contains(&anchor.to_string());
                        let count = calls.len();

                        use yalda::acp_channel::ToolCallStatus;
                        let has_failed = calls.iter().any(|tc| tc.status == ToolCallStatus::Failed);
                        let has_in_progress = calls
                            .iter()
                            .any(|tc| tc.status == ToolCallStatus::InProgress);
                        let all_completed = calls
                            .iter()
                            .all(|tc| tc.status == ToolCallStatus::Completed);
                        let (group_glyph, group_color): (&str, Hsla) = if has_failed {
                            ("✗", nc(at_snap.tool_failed))
                        } else if has_in_progress {
                            ("◐", nc(at_snap.tool_in_progress))
                        } else if all_completed {
                            ("●", nc(at_snap.tool_completed))
                        } else {
                            ("○", nc(at_snap.tool_pending))
                        };

                        let header_title: String = if count == 1 {
                            let tc = calls[0];
                            let base = if tc.title.is_empty() {
                                "(tool)".to_string()
                            } else {
                                tc.title.clone()
                            };
                            if let Some(detail) = tool_inline_detail(tc) {
                                format!("{} {}", base, detail)
                            } else {
                                base
                            }
                        } else {
                            let mut order: Vec<String> = Vec::new();
                            let mut counts: std::collections::HashMap<String, usize> =
                                std::collections::HashMap::new();
                            for tc in &calls {
                                let label = tool_type_label(tc);
                                if !counts.contains_key(&label) {
                                    order.push(label.clone());
                                }
                                *counts.entry(label).or_insert(0) += 1;
                            }
                            order
                                .iter()
                                .map(|l| format!("{} {}", counts[l], l))
                                .collect::<Vec<_>>()
                                .join(", ")
                        };

                        // A fold header is ONE line. A multi-line title — e.g. a Bash
                        // heredoc / multi-line command, whose `title` is the whole
                        // command — would otherwise render its full body in the header
                        // and read as "tool use not folded" (runtime report).
                        let header_title = crate::fold_header_line(&header_title);

                        let single_policy = if count == 1 {
                            Some(tool_render_policy(calls[0]))
                        } else {
                            None
                        };
                        let expandable = if count > 1 {
                            true
                        } else {
                            !matches!(single_policy, Some(ToolRenderPolicy::HeaderOnly))
                        };
                        let arrow = if !expandable {
                            " "
                        } else if group_expanded {
                            "▼"
                        } else {
                            "▶"
                        };

                        let anchor_str = anchor.to_string();
                        let weak = weak_self.clone();
                        let click_id = anchor_str.clone();
                        let mut header_row = div()
                            .id(SharedString::from(format!("tool-group-{}", anchor)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .py(px(6.0))
                            .px_2()
                            .child(div().text_color(dim_fg).child(arrow))
                            .child(div().text_color(group_color).child(group_glyph))
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(self_editor_fg)
                                    .text_size(px(12.0))
                                    .child(header_title),
                            );

                        if expandable {
                            header_row = header_row.cursor_pointer().on_click(
                                move |_ev: &gpui::ClickEvent,
                                      _w: &mut Window,
                                      app: &mut GpuiApp| {
                                    // STALE-CAPTURE SAFE: the id is resolved at
                                    // event time and toggled through the root,
                                    // never via captured row data.
                                    let id = click_id.clone();
                                    let _ = weak.update(app, |this, cx| {
                                        if let Some(mut c) = this.agent_mut(cx) {
                                            c.tools.toggle_expanded(&id);
                                        }
                                        cx.notify();
                                    });
                                },
                            );
                        }

                        let mut block = div()
                            .flex()
                            .flex_col()
                            .mt(px(16.0))
                            .mb(px(8.0))
                            .mx_4()
                            .child(header_row);

                        if group_expanded && expandable {
                            if count == 1 {
                                let tc = calls[0];
                                block = append_tool_body(
                                    block,
                                    tc,
                                    single_policy.unwrap_or(ToolRenderPolicy::Full),
                                    &code_font_snap,
                                    &at_snap,
                                );
                            } else {
                                for tc in &calls {
                                    let expanded_detail =
                                        expanded_snap.contains(&tc.tool_call_id.0.to_string());
                                    block = block.child(build_tool_block_with_weak(
                                        tc,
                                        expanded_detail,
                                        &code_font_snap,
                                        weak_self.clone(),
                                        &at_snap,
                                    ));
                                }
                            }
                        }

                        block.into_any_element()
                    }
                    FlatItem::Block(rendered_block) => {
                        let range = block_ranges_by_item.get(idx).copied().flatten();
                        let is_table = matches!(**rendered_block, RenderedBlock::Table { .. });
                        // bug-0017: a fenced code block (not a source-file view) uses the
                        // per-line REAL-bounds hit path — each content line self-registers
                        // a `TokenHit` from its own painted bounds AND paints the selection
                        // highlight. This fixes both the even-split band misalignment (the
                        // outer band was offset by the block's `p_2` padding + `[lang]`
                        // header and split over the FENCE-INCLUSIVE raw range) and the
                        // total absence of any painted highlight inside a `FlatItem::Block`.
                        let code_block = matches!(
                            **rendered_block,
                            RenderedBlock::CodeBlock { source_file: false, .. }
                        );
                        let block_hits = match range {
                            Some((s, e)) if e > s && code_block => Some(BlockHits {
                                sink: token_sink_snap.clone(),
                                // Skip the opening ``` fence: content line `li` → raw `s+1+li`.
                                raw_base: s + 1,
                                selection: sel_snap,
                                sel_bg: nc(at_snap.selection_bg),
                            }),
                            _ => None,
                        };
                        let uses_line_hits = block_hits.is_some();
                        let ctx = RenderCtx {
                            theme: &theme_snap,
                            body_font: body_font_snap.clone(),
                            code_font: code_font_snap.clone(),
                            // UXI-TextZoom-1: markdown blocks (headings/code/tables) in
                            // the transcript scale with zoom, like the doc view.
                            text_scale,
                            cursor_block: None,
                            doc_selection: None,
                            line_layouts: None,
                            current_block: None,
                            weak_view: None,
                            doc_dir: None,
                            block_count: 0,
                            show_heading_markers,
                            block_hits,
                        };
                        let inner = block_inner(&ctx, rendered_block);
                        // UXI-ParagraphSpacing-1: base 4px plus HALF the readability
                        // paragraph gap on each side (scaled with zoom), so two adjacent
                        // transcript blocks total the same `8*scale + gap` the Doc view
                        // uses between blocks. PADDING, not margin: transcript items are
                        // `gpui::list` rows, which ignore item margins (the old mt/mb 4
                        // produced no gap) — padding is part of the measured box. Prose
                        // paragraphs are untouched (they carry a blank `FlatItem::Line`).
                        let half = px(crate::PARAGRAPH_GAP_PX / 2.0 * text_scale);
                        let el = div()
                            .pt(px(4.0 * text_scale) + half)
                            .pb(px(4.0 * text_scale) + half)
                            .child(inner)
                            .into_any_element();
                        if uses_line_hits {
                            // Code-block content lines already self-registered per-line
                            // hit bands + selection inside `block_inner`.
                            return el;
                        }
                        // bug-0008: register hit-test bands so the mouse can select a
                        // parsed block's content (tables) — otherwise a block registers
                        // NO tokens and is unselectable. Tables get PER-CELL bands (skip
                        // the non-rendered `---` separator row so vertical bands align to
                        // painted rows); other blocks get one full-width band per raw line.
                        match range {
                            Some((s, e)) if e > s => {
                                let rows: Vec<(usize, Vec<(usize, usize)>)> = if is_table {
                                    (s..e)
                                        .filter_map(|l| {
                                            let t = lines_snap.get(l)?;
                                            if !t.contains('|') || is_table_separator_line(t) {
                                                return None;
                                            }
                                            Some((l, parse_table_cell_ranges(t)))
                                        })
                                        .collect()
                                } else {
                                    (s..e)
                                        .map(|l| {
                                            let len = lines_snap
                                                .get(l)
                                                .map(|t| t.chars().count())
                                                .unwrap_or(0);
                                            (l, vec![(0usize, len)])
                                        })
                                        .collect()
                                };
                                register_block_hits_on_paint(el, token_sink_snap.clone(), rows)
                            }
                            _ => el,
                        }
                    }
                    FlatItem::TurnHeader { role } => {
                        let (label, accent): (&str, Hsla) = match role {
                            TurnRole::Claude => ("Claude", nc(at_snap.turn_header_agent)),
                            TurnRole::User => ("You", nc(at_snap.turn_header_user)),
                        };
                        let rule_color = nc(at_snap.turn_rule);
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .w_full()
                            .pt(px(32.0))
                            .pb(px(8.0))
                            .px_4()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(accent)
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(body_font_snap.clone())
                                    .child(SharedString::from(label)),
                            )
                            .child(div().flex_1().h(px(1.0)).bg(rule_color))
                            .into_any_element()
                    }
                    FlatItem::ThinkingIndicator => {
                        let phase = if let Some(t) = turn_started_snap {
                            let ms = t.elapsed().as_millis() as f64;
                            ((ms / 750.0).sin() * 0.5 + 0.5) as f32
                        } else {
                            1.0
                        };
                        let alpha = 0.3 + phase * 0.7;

                        const STALL_WARN_S: u64 = 30;
                        let elapsed_s = turn_started_snap
                            .map(|t| t.elapsed().as_secs())
                            .unwrap_or(0);
                        let quiet_s = last_event_at_snap
                            .map(|t| t.elapsed().as_secs())
                            .unwrap_or(0);
                        let fmt_ms = |s: u64| format!("{}:{:02}", s / 60, s % 60);
                        let stalled = quiet_s >= STALL_WARN_S;

                        let dot_color = if stalled {
                            nc(at_snap.warm_accent)
                        } else {
                            Hsla {
                                h: 0.53,
                                s: 0.9,
                                l: 0.76,
                                a: alpha,
                            }
                        };
                        let (label, label_color) = if stalled {
                            (
                                format!(
                                    "No reply for {} (running {}) — the API may be overloaded. ⌘. to stop · ⌘. again to force-restart",
                                    fmt_ms(quiet_s),
                                    fmt_ms(elapsed_s),
                                ),
                                nc(at_snap.warm_accent),
                            )
                        } else {
                            (
                                format!("Thinking\u{2026} {}", fmt_ms(elapsed_s)),
                                Hsla {
                                    h: 0.0,
                                    s: 0.0,
                                    l: 0.6,
                                    a: alpha,
                                },
                            )
                        };
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .w_full()
                            .pt_3()
                            .pb_2()
                            .pl_1()
                            .pr_4()
                            .gap_2()
                            .child(div().text_size(px(14.0)).text_color(dot_color).child("●"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.0))
                                    .text_color(label_color)
                                    .font_family(body_font_snap.clone())
                                    .child(SharedString::from(label)),
                            )
                            .into_any_element()
                    }
                    FlatItem::YouBlock { parked } => {
                        // UXI-AgentTile-11 rules 5/6: an inline You-block rendered INSIDE the
                        // transcript at its anchor. `parked = None` is the ACTIVE block
                        // (live compose snapshot, caret-bearing, measured width); a
                        // `parked = Some(i)` is an additional insertion point shown
                        // read-only from its stored text. Draft text always lives
                        // OUTSIDE the transcript (Model C). Word-wrapped (UXI-AgentTile-9);
                        // active caret on its visual row, windowed (UXI-TextEditing-1).
                        let accent = cursor_color;
                        let fg = self_editor_fg;
                        let sel_bg = nc(at_snap.selection_bg);
                        // (lines, caret_line, caret_col, mode, focused, selection, anchor_line)
                        let active = parked.is_none();
                        let bounds = you_block_snap.as_ref().as_ref().map(|yb| yb.bounds.clone());
                        let (lines, caret_line, caret_col, mode, focused, selection, anchor_line) =
                            match parked {
                                None => match you_block_snap.as_ref() {
                                    Some(yb) => (
                                        &yb.lines,
                                        yb.cursor_line,
                                        yb.cursor_col,
                                        yb.mode,
                                        yb.focused,
                                        yb.selection,
                                        yb.anchor_line,
                                    ),
                                    None => return div().into_any_element(),
                                },
                                Some(i) => match you_parked_snap.get(*i) {
                                    // Read-only: no caret (caret_line out of range), no sel.
                                    Some((al, l)) => {
                                        (l, usize::MAX, 0, EditMode::Normal, false, None, *al)
                                    }
                                    None => return div().into_any_element(),
                                },
                            };
                        // Active block measures its own box; parked blocks (no bounds)
                        // reuse the compose's measured column width so they wrap at the
                        // real width, not a narrow 40-col fallback ("strange wordwrap").
                        let box_w = bounds.as_ref().map(|b| b.get().2).unwrap_or(0.0);
                        let wrap_cols = if box_w > 1.0 {
                            (box_w / crate::CHATBOX_CHAR_W).floor().max(1.0) as usize
                        } else if you_wrap_cols > 0 {
                            you_wrap_cols
                        } else {
                            40
                        };
                        // INTENT — co-authoring a document: the inline You-block renders
                        // EVERY line and GROWS with its content. It is part of the doc
                        // flow, never a fixed-height box that scrolls its own text out of
                        // view. Keeping the caret visible is the TRANSCRIPT scroll's job
                        // (reveal/follow the caret row below), not an internal window.
                        // (Was windowed to 10 logical lines around the caret — the "You
                        // div has limited space and scrolls after a while" bug. UXI-TextEditing-1
                        // is now upheld by revealing the caret's row within the block, not
                        // by truncating the block.)
                        let mut inner = div().flex().flex_col().w_full().min_w_0();
                        for (i, line) in lines.iter().enumerate() {
                            inner = inner.child(crate::build_chatbox_wrapped_line(
                                line,
                                focused && i == caret_line,
                                caret_col,
                                mode,
                                accent,
                                selection,
                                i,
                                &code_font_snap,
                                fg,
                                sel_bg,
                                wrap_cols,
                            ));
                        }
                        // Nav-focus highlight: while navigating the transcript, the
                        // block the cursor is on tints like an agent row (the cursor
                        // is "on" a block when it sits on the block's anchor line).
                        // `cursor_line` is `usize::MAX` off-nav, so this never lights
                        // up during compose/idle.
                        // you-div SCOPED-NORMAL indicator: when the block holds focus
                        // in Normal mode (Esc-once: editing the reply with motions),
                        // tint it with the accent and badge the label `You · NORMAL`,
                        // so it's unmistakable you're editing THIS block (vs Insert,
                        // vs the nav-focus tint). Insert keeps the plain `You`.
                        let scoped_normal = focused && mode == EditMode::Normal;
                        let row_bg: Hsla = if scoped_normal {
                            let mut h = accent;
                            h.a = 0.12;
                            h
                        } else if cursor_line == anchor_line {
                            let mut h = nc(at_snap.dim);
                            h.a = 0.2;
                            h
                        } else {
                            rgba(0x00000000).into()
                        };
                        let label = if scoped_normal {
                            SharedString::from("You · NORMAL")
                        } else {
                            SharedString::new_static("You")
                        };
                        let mut block = div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .min_w_0()
                            .pt_2()
                            .pb_2()
                            .pl_2()
                            .border_l_2()
                            .border_color(accent)
                            .bg(row_bg)
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(accent)
                                    .font_family(code_font_snap.clone())
                                    .child(label),
                            );
                        // Only the ACTIVE block measures its width (it owns the bounds
                        // Cell that drives wrap_cols); parked blocks render plainly.
                        block = match (active, bounds) {
                            (true, Some(sink)) => block.child(crate::CaptureBounds {
                                inner: inner.into_any_element(),
                                sink,
                            }),
                            _ => block.child(inner),
                        };
                        crate::probe_bounds("you-block", block.into_any_element())
                    }
                }
            }
        };

        // The transcript body fills the cached slot (`cached_child` bakes in
        // `size_full`, but the inner list still needs to fill). Default
        // (visible-only) measuring — NOT `Auto` (which measures every item every
        // frame): the body parent is `flex_1().min_h_0()` so the list fills the
        // viewport and scrolls without sizing to content.
        crate::probe_bounds(
            "transcript-viewport",
            div()
            .id("claude-body")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .px_6()
            .py_3()
            // Select-to-clipboard (UXI-Selection-1): mouse drag over the transcript
            // selects text and auto-copies on release. Hit-testing maps the
            // window point to a `(line, char)` via the painted-token sink.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, ev: &gpui::MouseDownEvent, _w, cx| {
                    this.transcript_mouse_down(ev, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, _w, cx| {
                this.transcript_mouse_move(ev, cx);
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, ev: &gpui::MouseUpEvent, _w, cx| {
                    this.transcript_mouse_up(ev, cx);
                }),
            )
            // UXI-TextZoom-1: the conversation prose scales with document zoom (the
            // FlatItem::Line rows inherit this base size); chrome keeps fixed px.
            .text_size(px(13.0 * text_scale))
            .font_family(code_font.clone())
            .text_color(editor_fg)
            .child(
                gpui::list(self.scroll.list_state.clone(), render_fn)
                    .flex_1()
                    .w_full(),
            )
            .into_any_element(),
        )
    }
}

/// Owned snapshot of the root view's GLOBAL render inputs (theme / fonts /
/// zoom), taken before the session update releases the root borrow.
struct RootSnapshot {
    theme: Theme,
    agent_theme: yalda::theme::AgentTheme,
    code_font: SharedString,
    body_font: SharedString,
    editor_fg: Hsla,
    base_style: NStyle,
    editor_fg_u32: u32,
    frozen_fg_u32: u32,
    syntect_hl: std::rc::Rc<yalda::highlight::Highlighter>,
    /// Global heading-marker toggle (agent-chat-only). Pushed via
    /// `notify_transcript_views`, so it busts the cache without a per-session
    /// seq. Threaded into the `FlatItem::Block` `RenderCtx`.
    show_heading_markers: bool,
    /// Document text-zoom multiplier (UXI-TextZoom-1). GLOBAL, not session state;
    /// pushed via `notify_transcript_views(TextStyle)` on every zoom change, so
    /// it busts the cache without a per-session seq. Multiplies the transcript's
    /// conversation prose + markdown-block sizes, the same way the buffer doc
    /// view scales. Chrome (gutter, tool/turn labels) and the fixed-geometry
    /// compose input (its own pinned caret/line-box) stay at native size.
    text_scale: f32,
}

/// Everything the row-render closure + reconcile/reveal consume, computed inside
/// the session update and returned so the borrow can end before the list builds.
struct TranscriptPrep {
    flat_items_arc: std::rc::Rc<Vec<FlatItem>>,
    gutter_tag_snap: std::rc::Rc<Vec<Option<TurnId>>>,
    lines_snap: std::rc::Rc<Vec<String>>,
    hl_snap: std::rc::Rc<Vec<std::rc::Rc<LineHl>>>,
    tool_calls_snap:
        std::rc::Rc<std::collections::HashMap<ToolCallKey, yalda::acp_channel::ToolCall>>,
    expanded_snap: std::rc::Rc<std::collections::HashSet<String>>,
    frozen_lines_snap: Vec<(usize, usize)>,
    lockable_through_snap: usize,
    sel_snap: Option<((usize, usize), (usize, usize))>,
    mode_snap: EditMode,
    cursor_line: usize,
    cursor_col: usize,
    turn_started_snap: Option<std::time::Instant>,
    last_event_at_snap: Option<std::time::Instant>,
    new_count: usize,
    edit_seq: u64,
    block_ranges_active: bool,
    /// Raw (start,end) line range of each parsed block, ascending — paired with the
    /// `FlatItem::Block` render order to register per-line hit-test bands (bug-0008).
    block_ranges_snap: Vec<(usize, usize)>,
    follow_tail: bool,
    pending_reveal_line: Option<usize>,
    /// UXI-AgentTile-11 (stage 2): live snapshot of the inline You-block's draft for the
    /// `FlatItem::YouBlock` render arm — the per-logical-line text, the caret
    /// position, and the edit mode. `None` when no block is open.
    you_block_snap: Option<YouBlockSnap>,
    you_parked_snap: Vec<(usize, Vec<String>)>,
    you_wrap_cols: usize,
    /// Render-input hash of the ACTIVE You-block (compose text + caret + mode +
    /// selection), or 0 when no block is active. The You-block is one list item
    /// driven by the COMPOSE buffer, not the transcript `edit_seq`, so
    /// `reconcile_list` can't see its content move. When this differs from
    /// `TranscriptScroll::last_you_block_seq`, `build_body` splices that one item
    /// so GPUI re-measures it instead of repainting the stale cached element (the
    /// "/clear worksheet invisible" bug). See [`TranscriptScroll::last_you_block_seq`].
    you_block_seq: u64,
}

/// Per-frame snapshot of the inline You-block draft (the separate `Compose`),
/// rendered inside the transcript at its anchor (UXI-AgentTile-11 rule 5, stage 2).
struct YouBlockSnap {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    mode: EditMode,
    /// The doc line the block is anchored at (tail → the last line) — the transcript
    /// nav cursor "is on" the block when it sits on this line, which drives the
    /// focus highlight in nav, matching agent rows (UXI-AgentTile-11 nav feedback).
    anchor_line: usize,
    /// Whether the compose holds focus. The caret renders ONLY when focused — a
    /// persisted (non-empty Esc) block shows no caret while the user navigates the
    /// transcript, so there aren't two carets on screen (bug-hunt-2 B5).
    focused: bool,
    /// The compose selection, so a visual selection inside the inline reply is
    /// actually highlighted (bug-hunt-2 B6).
    selection: Option<((usize, usize), (usize, usize))>,
    /// Measured inner width sink (shared with the `Compose`) — written via
    /// `CaptureBounds` during paint, read next frame to word-wrap at the real
    /// column count (UXI-AgentTile-9), exactly like the bottom-panel chatbox.
    bounds: std::rc::Rc<std::cell::Cell<(f32, f32, f32, f32)>>,
}
