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
    bound: bool,
    model_gen: u64,
    focus: DiffFocus,
    collapsed_gen: u64,
    refreshing: bool,
    has_error: bool,
    /// `WorktreePicker::gen_` — bumped by every picker mutation (rows loaded,
    /// selection moved, loading/not-a-repo/error flips), so the unbound
    /// picker re-renders exactly when its own state moves.
    picker_gen: u64,
    /// `text_scale.to_bits()` — global zoom input (UXI-TextZoom-1 pattern),
    /// falls out of the same root-observe fingerprint (see module docs).
    text_scale_bits: u32,
}

impl DiffSeqs {
    pub(crate) fn of(tile: &DiffTile, text_scale: f32) -> Self {
        DiffSeqs {
            bound: tile.worktree.is_some(),
            model_gen: tile.model_gen,
            focus: tile.focus,
            collapsed_gen: tile.collapsed_gen,
            refreshing: tile.refreshing,
            has_error: tile.error.is_some(),
            picker_gen: tile.picker.gen_,
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
        let selected_bg: Hsla = nc(r.theme.overlay.selected_bg);
        let tile = r.diff_tile_ref(self.window_id);

        let body: AnyElement = match tile {
            None => div().size_full().into_any_element(),
            Some(t) => {
                if t.worktree.is_none() {
                    diff_picker_body(&t.picker, self.window_id, selected_bg, &st, cx)
                        .into_any_element()
                } else if let Some(err) = &t.error {
                    diff_error_body(err, &st).into_any_element()
                } else if let Some(model) = &t.model {
                    diff_model_body(model, t.focus, &t.collapsed, &st).into_any_element()
                } else {
                    diff_loading_body(t.refreshing, &st).into_any_element()
                }
            }
        };

        self.last_rendered = tile.map(|t| DiffSeqs::of(t, scale)).unwrap_or_default();

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

/// `$HOME/…` → `~/…` for the picker's dimmed path line.
fn home_relative(path: &std::path::Path) -> String {
    let raw = path.display().to_string();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home).display().to_string();
        if let Some(rest) = raw.strip_prefix(&home)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("~{rest}");
        }
    }
    raw
}

/// The worktree picker (spec rev 2 B1, UXI-Diff-10): a title, one
/// `picker_option_row_detailed` per worktree (branch prominent, home-relative
/// path dimmed, a "primary" badge on the primary checkout), then the "Pick a
/// folder…" row and the key-hint footer. Rows are clickable; the handler
/// carries only the ROW INDEX and resolves the row at event time through the
/// root (`diff_picker_activate`) — yux rule 4, since a cache hit replays this
/// render's listeners.
fn diff_picker_body(
    picker: &WorktreePicker,
    window_id: workspace::WindowId,
    selected_bg: Hsla,
    st: &DetailStyle,
    cx: &Context<DiffView>,
) -> gpui::Div {
    let mut col = div().flex().flex_col().w_full().gap(px(2.0));
    col = col.child(
        div()
            .pb_2()
            .text_color(st.fg)
            .font_family(st.prose.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(st.pt * 1.2))
            .child(SharedString::from("Review a worktree")),
    );

    let status: Option<(&str, Hsla)> = if picker.loading && picker.rows.is_empty() {
        Some(("Finding worktrees…", st.dim))
    } else if picker.not_a_repo {
        Some(("Not inside a git repository.", st.dim))
    } else {
        picker.error.as_deref().map(|e| (e, st.err))
    };
    if let Some((text, color)) = status {
        col = col.child(probe_bounds(
            "diff-picker-status",
            div()
                .py_1()
                .px(px(10.0))
                .text_color(color)
                .font_family(st.mono.clone())
                .text_size(st.base)
                .child(SharedString::from(text.to_string()))
                .into_any_element(),
        ));
    }

    let root_listener = |index: usize| {
        cx.listener(move |this: &mut DiffView, _ev: &MouseDownEvent, _w, cx| {
            let Some(root) = this.root.upgrade() else {
                return;
            };
            let wid = this.window_id;
            root.update(cx, |r, cx| r.diff_picker_activate(wid, index, cx));
        })
    };

    for (i, row) in picker.rows.iter().enumerate() {
        let label = row.label();
        let path = home_relative(&row.path);
        let el = picker_option_row_detailed(
            SharedString::from(format!("diff-picker-row-{window_id}-{i}")),
            "⎇",
            &label,
            Some((&path, st.dim)),
            row.is_primary.then_some(("primary", st.dim)),
            picker.selected == i,
            st.accent,
            st.fg,
            selected_bg,
            &st.prose,
            &st.mono,
        )
        .on_mouse_down(MouseButton::Left, root_listener(i));
        col = col.child(probe_bounds_dyn(
            format!("diff-picker-row-{i}"),
            el.into_any_element(),
        ));
    }

    let folder = picker.folder_index();
    let folder_row = picker_option_row(
        SharedString::from(format!("diff-picker-folder-{window_id}")),
        "…",
        "Pick a folder…",
        None,
        picker.selected == folder,
        st.accent,
        st.accent,
        selected_bg,
        &st.prose,
        &st.mono,
    )
    .on_mouse_down(MouseButton::Left, root_listener(folder));
    col = col.child(probe_bounds("diff-picker-folder-row", folder_row.into_any_element()));

    col.child(
        div()
            .pt_3()
            .px(px(10.0))
            .text_color(st.dim)
            .font_family(st.mono.clone())
            .text_size(px(12.0))
            .child(SharedString::from("j/k move · enter review · p pick folder")),
    )
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
