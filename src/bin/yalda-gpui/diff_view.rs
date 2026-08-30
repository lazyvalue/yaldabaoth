//! `DiffView` — the cached body of a Diff tile (`App::Diff` / `DiffTile`,
//! `diff.rs`). Cog node `app-diff-tile` (nd0e). A yux component (see
//! `yux/CLAUDE.md`): embedded via `cached_child` in `render_diff`
//! (`screens.rs`).
//!
//! # Why this observes the ROOT, not a domain entity
//!
//! `TranscriptView` observes its `Entity<AgentSession>`, and `LinearView` /
//! `CogView` own their payload directly (no observe at all — their tile
//! holds no content). `DiffTile` is neither: per spec § Data Model, the
//! derived `DiffModel` + focus + collapse-set are the SPEC's data model
//! fields, owned by `DiffTile` itself — a plain struct living in the
//! workspace layout tree (not a GPUI entity, like `LinearTile`/`CogTile`).
//! There is no separate "model entity" to observe. The only entity through
//! which `DiffTile` is reachable is the ROOT view (`YaldaGpuiView`), so
//! `DiffView` observes THAT, exactly like `TranscriptView` observes its
//! session — filtered through a cheap fingerprint ([`DiffSeqs`]) so an
//! unrelated root notify (typing in an agent tile elsewhere, an unrelated
//! menu, etc.) leaves this view's render flat. This also means a GLOBAL
//! input (text zoom) needs no separate `notify_diff_views` push: it already
//! lives on the same root entity this view observes, so it falls out of the
//! same fingerprint for free (deviation from the transcript/linear precedent
//! documented here, not a gap — see the render-count guard test in
//! `verify_harness.rs`).
//!
//! `render()` reads `DiffTile` fields directly off the root's `&YaldaGpuiView`
//! borrow for the whole element-tree build (no `DiffModel` clone) — the
//! "reads, does not own" contract.

use super::*;

/// The slice-version watermark the observe filter compares across renders.
/// Mirrors `TranscriptSeqs` / the `RootSnapshot` fingerprint idea, but over
/// `DiffTile` fields read off the root. Cheap: no field here costs more than
/// a `Copy` read (the `DiffModel` itself is never hashed — `model_gen` is the
/// proxy for "did the derived diff change").
#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) struct DiffSeqs {
    has_source: bool,
    model_gen: u64,
    focus: DiffFocus,
    collapsed_gen: u64,
    refreshing: bool,
    has_error: bool,
    /// Number of sessions live in the store — so the unbound selector
    /// re-renders when a session is created/closed elsewhere. Cheap (a
    /// `BTreeMap::len()`).
    session_count: usize,
    /// `text_scale.to_bits()` — global zoom input (UXI-TextZoom-1 pattern),
    /// falls out of the same root-observe fingerprint (see module docs).
    text_scale_bits: u32,
}

impl DiffSeqs {
    pub(crate) fn of(tile: &DiffTile, session_count: usize, text_scale: f32) -> Self {
        DiffSeqs {
            has_source: tile.source.is_some(),
            model_gen: tile.model_gen,
            focus: tile.focus,
            collapsed_gen: tile.collapsed_gen,
            refreshing: tile.refreshing,
            has_error: tile.error.is_some(),
            session_count,
            text_scale_bits: text_scale.to_bits(),
        }
    }
}

/// The cached Diff body view. One per Diff tile (owned by the tile via
/// `Entity<DiffView>`, dropped when the tile closes — no registry).
pub(crate) struct DiffView {
    root: WeakEntity<YaldaGpuiView>,
    /// The stable id of the tile this view belongs to — how `render()` finds
    /// its own `DiffTile` back through the root (see module docs).
    window_id: workspace::WindowId,
    last_rendered: DiffSeqs,
    scroll: ScrollHandle,
    perf_label: &'static str,
}

impl DiffView {
    /// Construct a Diff body view and register the `cx.observe(&root)`
    /// subscription that self-notifies on a fingerprint move. `root` is a
    /// STRONG handle for the duration of this call only (needed to register
    /// the subscription); the callback receives its own copy of the
    /// observed entity from gpui, so nothing here captures `root` by `move`
    /// — no retain cycle (mirrors `TranscriptView::new`'s shape exactly,
    /// substituting the session entity for the root entity).
    pub(crate) fn new(
        root: Entity<YaldaGpuiView>,
        window_id: workspace::WindowId,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&root, |this: &mut DiffView, root_ent, cx| {
            let now = root_ent.read(cx).diff_seqs_for(this.window_id);
            if this.last_rendered != now {
                record_notify(this.perf_label, MissReason::Dirtied);
                cx.notify();
            }
        })
        .detach();
        DiffView {
            root: root.downgrade(),
            window_id,
            last_rendered: DiffSeqs::default(),
            scroll: ScrollHandle::new(),
            perf_label: "diff",
        }
    }

    pub(crate) fn perf_label(&self) -> &'static str {
        self.perf_label
    }
}

impl Render for DiffView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        record_render(self.perf_label);
        let Some(root_ent) = self.root.upgrade() else {
            return div().size_full().into_any_element();
        };
        let r = root_ent.read(cx);
        let scale = r.text_scale;
        let st = DetailStyle {
            fg: r.editor_fg(),
            dim: nc(r.theme.agent.dim),
            accent: nc(r.theme.agent.warm_accent),
            err: rgb(0xff6b6b).into(),
            mono: r.code_font.clone(),
            prose: r.body_font.clone(),
            base: px(14.0 * scale),
            pt: 14.0 * scale,
        };
        let editor_bg = r.editor_bg();
        let tile = r.diff_tile_ref(self.window_id);

        let body: AnyElement = match tile {
            None => div().size_full().into_any_element(),
            Some(t) => {
                if let Some(err) = &t.error {
                    diff_error_body(err, &st).into_any_element()
                } else if t.source.is_none() {
                    let candidates = diff_eligible_sessions(r, cx);
                    diff_selector_body(&candidates, &st).into_any_element()
                } else if let Some(model) = &t.model {
                    diff_model_body(model, t.focus, &t.collapsed, &st).into_any_element()
                } else {
                    diff_loading_body(t.refreshing, &st).into_any_element()
                }
            }
        };

        let session_count = r.sessions.ids().count();
        self.last_rendered = tile
            .map(|t| DiffSeqs::of(t, session_count, scale))
            .unwrap_or_default();

        let scroll = self.scroll.clone();
        div()
            .id("diff-body")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px_4()
            .py_3()
            .bg(editor_bg)
            .text_color(st.fg)
            .child(body)
            .into_any_element()
    }
}

/// Sessions eligible for the unbound-tile selector (spec B1): those whose
/// `cwd` looks like a git worktree. A cheap bounded filesystem walk (stat
/// calls for a `.git` entry, dir-or-file to cover linked worktrees) — NOT a
/// git subprocess, so this stays inside the paint-path-purity budget (spec
/// C2 bans git subprocesses / `ReviewState` I/O on render, not a handful of
/// `Path::exists` stats already common elsewhere in this file's render paths).
fn diff_eligible_sessions(
    r: &YaldaGpuiView,
    cx: &GpuiApp,
) -> Vec<(SessionId, String, PathBuf)> {
    r.sessions
        .iter()
        .map(|(id, s)| {
            let s = s.read(cx);
            (id, s.label.clone(), s.cwd.clone())
        })
        .filter(|(_, _, cwd)| looks_like_git_repo(cwd))
        .collect()
}

/// Cheap, bounded upward walk for a `.git` entry (dir for a primary checkout,
/// file for a linked worktree). No subprocess. `pub(crate)` — also used by
/// `diff_ui.rs`'s selector digit-key binder (both must agree on eligibility).
pub(crate) fn looks_like_git_repo(path: &std::path::Path) -> bool {
    let mut cur = Some(path);
    let mut hops = 0;
    while let Some(p) = cur {
        if p.join(".git").exists() {
            return true;
        }
        cur = p.parent();
        hops += 1;
        if hops > 32 {
            break;
        }
    }
    false
}

// ── Domain body builders (Diff-specific; composed from yux primitives) ──────

fn diff_error_body(err: &str, st: &DetailStyle) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .child(
            div()
                .text_color(st.err)
                .font_family(st.mono.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(st.base)
                .child(SharedString::from("diff error")),
        )
        .child(multiline_text(err, st.err, &st.prose, st.base))
}

fn diff_loading_body(refreshing: bool, st: &DetailStyle) -> gpui::Div {
    let msg = if refreshing {
        "deriving diff…"
    } else {
        "no diff yet — press r to refresh"
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .text_color(st.dim)
        .font_family(st.mono.clone())
        .text_size(st.base)
        .child(SharedString::from(msg))
}

fn diff_selector_body(candidates: &[(SessionId, String, PathBuf)], st: &DetailStyle) -> gpui::Div {
    let mut col = div().flex().flex_col().w_full().gap_2();
    col = col.child(
        div()
            .text_color(st.dim)
            .font_family(st.mono.clone())
            .text_size(px(st.pt * 0.9))
            .child(SharedString::from(
                "No diff bound yet. Press a number to diff that session's worktree, \
                 or `p` to diff the current workspace directory.",
            )),
    );
    if candidates.is_empty() {
        col = col.child(
            div()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from("No open sessions look like git worktrees.")),
        );
    } else {
        for (i, (_, label, cwd)) in candidates.iter().enumerate().take(9) {
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_center()
                    .w_full()
                    .px_1()
                    .font_family(st.mono.clone())
                    .text_size(st.base)
                    .child(
                        div()
                            .w(px(24.0))
                            .flex_none()
                            .text_color(st.accent)
                            .child(SharedString::from(format!("{}.", i + 1))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(st.fg)
                            .child(SharedString::from(label.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(st.dim)
                            .child(SharedString::from(cwd.display().to_string())),
                    ),
            );
        }
    }
    col
}

/// First 8 hex chars of a SHA (ASCII, safe to byte-slice).
fn short_sha(s: &str) -> &str {
    &s[..s.len().min(8)]
}

fn diff_model_body(
    model: &DiffModel,
    focus: DiffFocus,
    collapsed: &HashSet<PathBuf>,
    st: &DetailStyle,
) -> gpui::Div {
    let green: Hsla = rgb(0x4caf50).into();
    let red: Hsla = rgb(0xe57373).into();

    let mut col = div().flex().flex_col().w_full().gap_3();
    col = col.child(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pb_2()
            .child(
                div()
                    .text_color(st.fg)
                    .font_family(st.prose.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(st.pt * 1.2))
                    .child(SharedString::from(format!("{} → working tree", model.base))),
            )
            .child(
                div()
                    .text_color(st.dim)
                    .font_family(st.mono.clone())
                    .text_size(px(st.pt * 0.85))
                    .child(SharedString::from(format!(
                        "branch {} · merge-base {} · {} file(s){}",
                        model.branch,
                        short_sha(&model.merge_base),
                        model.files.len(),
                        if model.dirty { " · dirty" } else { "" }
                    ))),
            ),
    );

    if model.files.is_empty() {
        return col.child(
            div()
                .text_color(st.dim)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from("No changes.")),
        );
    }

    for (fi, file) in model.files.iter().enumerate() {
        let is_collapsed = collapsed.contains(&file.path);
        let status_tag = match &file.status {
            FileStatus::Modified => "M".to_string(),
            FileStatus::Added => "A".to_string(),
            FileStatus::Deleted => "D".to_string(),
            FileStatus::Renamed { from } => format!("R {} →", from.display()),
        };
        let file_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .px_1()
            .font_family(st.mono.clone())
            .text_size(st.base)
            .child(
                div()
                    .flex_none()
                    .w(px(18.0))
                    .text_color(st.dim)
                    .child(SharedString::from(if is_collapsed { "▸" } else { "▾" })),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(90.0))
                    .text_color(st.accent)
                    .child(SharedString::from(status_tag)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(st.fg)
                    .child(SharedString::from(file.path.display().to_string())),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(green)
                    .child(SharedString::from(format!("+{}", file.added))),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(red)
                    .child(SharedString::from(format!("-{}", file.removed))),
            );
        col = col.child(probe_bounds_dyn(
            format!("diff-file-{fi}"),
            file_row.into_any_element(),
        ));

        if is_collapsed {
            continue;
        }
        for (hi, hunk) in file.hunks.iter().enumerate() {
            let is_focused = focus.file == fi && focus.hunk == hi;
            let block = diff_hunk_block(hunk, is_focused, st, green, red);
            col = col.child(probe_bounds_dyn(
                format!("diff-hunk-{fi}-{hi}"),
                block.into_any_element(),
            ));
        }
    }
    col
}

fn diff_hunk_block(
    hunk: &Hunk,
    focused: bool,
    st: &DetailStyle,
    green: Hsla,
    red: Hsla,
) -> gpui::Div {
    let transparent: Hsla = rgba(0x00000000).into();
    let bar: Hsla = if focused { st.accent } else { transparent };
    let header_color = if hunk.reviewed { st.dim } else { st.fg };

    let mut lines_col = div().flex().flex_col().w_full().pl_2();
    lines_col = lines_col.child(
        div()
            .text_color(header_color)
            .font_family(st.mono.clone())
            .text_size(px(st.pt * 0.85))
            .child(SharedString::from(if hunk.reviewed {
                format!("{}  ✓ reviewed", hunk.header)
            } else {
                hunk.header.clone()
            })),
    );
    for line in &hunk.lines {
        let (prefix, text, color): (&str, &str, Hsla) = match line {
            DiffLine::Added(t) => ("+", t.as_str(), green),
            DiffLine::Removed(t) => ("-", t.as_str(), red),
            DiffLine::Context(t) => (" ", t.as_str(), st.fg),
        };
        lines_col = lines_col.child(
            div()
                .flex()
                .flex_row()
                .w_full()
                .font_family(st.mono.clone())
                .text_size(st.base)
                .text_color(color)
                .child(
                    div()
                        .flex_none()
                        .w(px(14.0))
                        .child(SharedString::from(prefix.to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(SharedString::from(text.to_string())),
                ),
        );
    }

    div()
        .flex()
        .flex_row()
        .w_full()
        .gap_2()
        .pb_1()
        .child(div().flex_none().w(px(3.0)).bg(bar))
        .child(div().flex_1().min_w_0().child(lines_col))
}
