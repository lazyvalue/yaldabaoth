//! Verification harness — headless GPUI tests (see docs/dev-system.md § Verification harness).
//!
//! The GPUI app **is** drivable headlessly: GPUI ships a real test harness
//! (`#[gpui::test]` + `TestAppContext`, which simulates platform input and runs
//! the async executor via `run_until_parked`), and this module builds on it to
//! drive the real `YaldaGpuiView` (open agent, simulate keystrokes, stream
//! synthetic events, assert state) without a display. So state-level behavior
//! is no longer human-only — the human is the oracle for the three things a
//! headless test still can't reach, not for every change.
//!
//! `test-support` is enabled via the `gpui` dev-dependency, so this compiles
//! only for test builds — the production binary is unaffected.
//!
//! Solved here: driving the real view + real keystrokes + the agent reducer,
//! and the O(changed) render-count proxy. The three remaining gaps (still
//! human-verified): (1) the full GUI↔server↔agent loop in one process (seam
//! tests drive cores directly because `sent` can't be true with no daemon);
//! (2) golden render output (painted pixels / layout geometry); (3) wall-clock
//! perf as a gate (count is a proxy; debug masks wins).

#![cfg(test)]

use gpui::{AppContext, TestAppContext, point, px};

use crate::YaldaGpuiView;
use std::path::PathBuf;
use yalda::theme::Theme;

/// Stone 1: prove GPUI's `test-support` harness wires up in this crate — boot a
/// `TestAppContext` and round-trip an entity through `update`/`read`. If this
/// runs, the headless-driver path is open and we can build up to constructing a
/// real window + `YaldaGpuiView` and simulating input.
#[gpui::test]
fn harness_boots(cx: &mut TestAppContext) {
    let value = cx.update(|cx| {
        let entity = cx.new(|_cx| 41u32);
        *entity.read(cx)
    });
    assert_eq!(value, 41, "TestAppContext entity round-trip");
}

/// Stone 2: construct the REAL `YaldaGpuiView` in a headless test window and
/// render a frame (`run_until_parked` drives layout/paint via the test
/// platform). This is a *capability* proof — the production view is headlessly
/// constructable, renderable, and its state is readable — not a verification of
/// any landed feature. Asserting specific feature behavior (rail opens on
/// Cmd-B, etc.) is the next stone and is deliberately not done here.
#[gpui::test]
fn constructs_and_renders_real_view(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    // Drive one full frame headlessly — would panic if construction/layout/paint
    // hit a missing dependency (fonts, assets, etc.).
    vcx.run_until_parked();
    // State is readable through the entity handle (the hook future stones use to
    // assert post-action state).
    let readable = view.read_with(vcx, |_v, _cx| true);
    assert!(
        readable,
        "real YaldaGpuiView constructs + renders headlessly"
    );
}

/// Workspace → agent CWD inheritance (untitled.md Workspace + Agent TODOs;
/// ADR-0023). The cwd is a required, typed field on every `Tab`
/// ([`WorkspaceCwd`]) — "no cwd" is unrepresentable — so `agent_base_cwd` is
/// total and always surfaces the active workspace's dir. Includes the
/// regression that motivated the type: an **ephemeral** virtual workspace
/// (jump-panel) inherits the spawning workspace's cwd instead of silently
/// falling back to the process dir.
#[gpui::test]
fn workspace_cwd_inheritance(cx: &mut TestAppContext) {
    use crate::workspace::WorkspaceCwd;
    let start = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (view, vcx) = cx.add_window_view({
        let start = start.clone();
        move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            YaldaGpuiView::new_browser(start, Theme::default(), focus_handle)
        }
    });
    vcx.run_until_parked();

    // A browser workspace boots with its start dir as the cwd — always present.
    let booted = view.read_with(vcx, |v, _| v.active_workspace_cwd());
    assert_eq!(booted, Some(start.clone()), "workspace boots with a real cwd");

    // Set CWD → both the surfaced cwd and what a new agent inherits move.
    view.update(vcx, |v, _cx| {
        v.workspace
            .active_tab_mut()
            .expect("active tab")
            .set_cwd(WorkspaceCwd::new(PathBuf::from("/Users/scott/ws/fulcrum")));
    });
    let base = view.read_with(vcx, |v, _| v.agent_base_cwd());
    assert_eq!(
        base,
        PathBuf::from("/Users/scott/ws/fulcrum"),
        "a new agent inherits the active workspace's cwd"
    );

    // The regression (ADR-0023): opening an ephemeral virtual workspace (what a
    // jump-panel free-session click does) must inherit the spawning workspace's
    // cwd — NOT reset to the process dir. With the old empty-`kv` ephemeral tab,
    // `agent_base_cwd` here returned the launch dir.
    view.update(vcx, |v, _cx| {
        v.workspace
            .open_ephemeral_tab(crate::App::Agent(crate::AgentTile::new()));
    });
    let in_ephemeral = view.read_with(vcx, |v, _| v.agent_base_cwd());
    assert_eq!(
        in_ephemeral,
        PathBuf::from("/Users/scott/ws/fulcrum"),
        "an agent created in an ephemeral virtual workspace inherits the \
         spawning workspace's cwd, not the process dir"
    );
}

/// Browser start dir (untitled.md "New buffers ... root dir based on CWD key
/// set for workspace ... File browser should default to CWD of buffers ... when
/// moving from file view/editor to browser, the browser's directory should be
/// the parent directory of the file we just left"). Three rules, in priority
/// order: (1) continuity — leaving a file-backed buffer lands in that file's
/// parent dir; (2) else the workspace registry `"cwd"`; (3) else the process
/// dir. Pins the resolution `open_browser_inner` / new-tile paths share.
#[gpui::test]
fn browser_start_dir_resolution(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();

    // (3) No workspace cwd, focused on a standalone browser (no file) →
    // process dir.
    let fallback = view.read_with(vcx, |v, _| v.browser_start_dir());
    assert_eq!(
        fallback,
        crate::process_cwd(),
        "no cwd, no file → process dir"
    );

    // (2) Workspace cwd set, still no file-backed buffer → the workspace cwd.
    let ws_dir = std::env::temp_dir();
    view.update(vcx, |v, _cx| {
        v.workspace
            .active_tab_mut()
            .expect("active tab")
            .set_cwd(crate::workspace::WorkspaceCwd::new(ws_dir.clone()));
    });
    let from_ws = view.read_with(vcx, |v, _| v.browser_start_dir());
    assert_eq!(from_ws, ws_dir, "workspace cwd wins over process dir");

    // (1) Continuity: open a real file, then the browser lands in its parent
    // dir — overriding even the workspace cwd.
    let mut file = std::env::temp_dir();
    file.push("yalda-browser-start-dir-test.md");
    std::fs::write(&file, "# hi\n").expect("write temp file");
    let parent = file.parent().expect("temp file has parent").to_path_buf();
    view.update(vcx, |v, _cx| {
        assert!(v.open_file(file.clone()), "open temp file");
    });
    let from_file = view.read_with(vcx, |v, _| v.browser_start_dir());
    assert_eq!(
        from_file,
        std::fs::canonicalize(&parent).unwrap_or(parent),
        "leaving a file lands in its parent dir"
    );
    let _ = std::fs::remove_file(&file);
}

/// Tier-3 latency gate: prove the Edit view does **O(changed)** highlight work
/// per keystroke, not O(document). Open a 3000-line buffer, render it (cold:
/// every line highlighted once), then perform a single-character insert and
/// render again — the incremental cache must re-highlight ~1 line, NOT ~3000.
/// A no-edit re-render must recompute 0. The assertion is on the cache's
/// recompute counter, not wall-clock, so it's deterministic in CI.
#[gpui::test]
fn edit_view_keystroke_is_o_changed(cx: &mut TestAppContext) {
    const N: usize = 3000;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        );
        // A representative markdown buffer: prose + a fenced rust block,
        // repeated to N lines. Mixed content exercises the fence-aware
        // reconcile, not just plain paragraphs.
        let block = [
            "## Section heading",
            "Some **bold** prose with `inline code` here.",
            "```rust",
            "fn f(x: usize) -> usize { x * 2 }",
            "```",
            "",
        ];
        let mut buf = String::new();
        for i in 0..N {
            buf.push_str(block[i % block.len()]);
            buf.push('\n');
        }
        v.test_open_edit(&buf);
        v
    });

    // --- Cold paint: first render highlights every line once. ---
    vcx.run_until_parked();
    let (cold_recomputed, cold_skip) = view.update(vcx, |v, cx| v.test_edit_cache_stats());
    assert!(
        cold_recomputed >= N,
        "cold paint should highlight all {N} lines, got {cold_recomputed}"
    );
    assert!(!cold_skip, "cold paint is not a skip");

    // --- No-edit re-render: a notify with no buffer mutation must fast-skip
    //     (edit_seq unchanged) and recompute zero lines. ---
    view.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    let (idle_recomputed, idle_skip) = view.update(vcx, |v, cx| v.test_edit_cache_stats());
    assert_eq!(
        idle_recomputed, 0,
        "no-change re-render must recompute 0 lines (fast skip), got {idle_recomputed}"
    );
    assert!(
        idle_skip,
        "no-change re-render must take the fast-skip path"
    );

    // --- One keystroke: insert a single char, then render. Only the edited
    //     line (a plain prose line, no fence toggle) should re-highlight. ---
    view.update(vcx, |v, cx| {
        let e = v.edit_mut().expect("edit view");
        // Put the cursor on a prose line (index 1 in the repeating block) so
        // the insert doesn't toggle a code fence and cascade below it.
        e.editor.set_cursor(1, 0);
        e.editor.insert_char('X');
        cx.notify();
    });
    vcx.run_until_parked();
    let (edit_recomputed, edit_skip) = view.update(vcx, |v, cx| v.test_edit_cache_stats());
    assert!(!edit_skip, "an edit must not fast-skip");
    assert_eq!(
        edit_recomputed, 1,
        "a single-char edit into a {N}-line doc must re-highlight exactly 1 line, got {edit_recomputed}"
    );
}

/// Regression (scroll anchoring): `splice_list_to_items` must keep the
/// viewport anchored across a line edit, NOT snap it to item 0. The old path
/// `ListState::reset()`-ed on every line-count change, which nulls
/// `logical_scroll_top`; the same-frame `scroll_to_reveal_item` then computed
/// against unmeasured (zero-height) rows and jumped to the top of the file —
/// the "view jumps to the top whenever a newline is added/removed" bug.
#[gpui::test]
fn edit_list_splice_preserves_scroll_anchor(_cx: &mut TestAppContext) {
    // Case A: an edit BELOW the viewport top leaves the top line anchored.
    let list = gpui::ListState::new(0, gpui::ListAlignment::Top, px(20.));
    let old: Vec<usize> = (0..100).collect();
    crate::splice_list_to_items(&list, &[], &old); // populate the list: 0 → 100
    list.scroll_to(gpui::ListOffset {
        item_ix: 80,
        offset_in_item: px(0.),
    });
    let mut new = old.clone();
    new.remove(90); // delete a line below the viewport top
    crate::splice_list_to_items(&list, &old, &new);
    assert_eq!(list.item_count(), 99, "one line removed");
    assert_eq!(
        list.logical_scroll_top().item_ix,
        80,
        "an edit below the viewport top must leave the top line anchored, not jump to 0"
    );

    // Case B: a deletion ABOVE the viewport top shifts the anchor down by the
    // number of removed lines (same content stays under the top edge) — and
    // still never collapses to 0.
    let list2 = gpui::ListState::new(0, gpui::ListAlignment::Top, px(20.));
    let old2: Vec<usize> = (0..100).collect();
    crate::splice_list_to_items(&list2, &[], &old2);
    list2.scroll_to(gpui::ListOffset {
        item_ix: 80,
        offset_in_item: px(0.),
    });
    let mut new2 = old2.clone();
    new2.remove(10); // delete a line above the viewport top
    crate::splice_list_to_items(&list2, &old2, &new2);
    assert_eq!(
        list2.logical_scroll_top().item_ix,
        79,
        "deleting a line above the viewport shifts the anchor down by one, never to 0"
    );
}

/// Regression (end-to-end): a newline DELETE (line merge) deep in the buffer
/// must keep the viewport where it was — it must NOT jump to the top of the
/// file leaving the caret off-screen. Drives the real Edit render path.
#[gpui::test]
fn edit_newline_delete_keeps_viewport_anchored(cx: &mut TestAppContext) {
    const N: usize = 200;
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        );
        let mut buf = String::new();
        for i in 0..N {
            buf.push_str(&format!("line {i}"));
            buf.push('\n');
        }
        v.test_open_edit(&buf);
        v
    });
    vcx.run_until_parked();

    // Scroll the viewport into the middle of the file.
    let count_before = view.update(vcx, |v, _cx| {
        let e = v.edit_mut().expect("edit view");
        e.list.state().scroll_to(gpui::ListOffset {
            item_ix: 80,
            offset_in_item: px(0.),
        });
        e.list.len()
    });

    // Backspace at column 0, well below the viewport top: a line MERGE (the line
    // count shrinks). The old reset() path snapped the viewport to item 0 here.
    view.update(vcx, |v, cx| {
        use crate::EditOps;
        let e = v.edit_mut().expect("edit view");
        e.editor.set_cursor(120, 0);
        e.editor.backspace();
        cx.notify();
    });
    vcx.run_until_parked();

    let (top, count) = view.update(vcx, |v, _cx| {
        let e = v.edit_mut().expect("edit view");
        (
            e.list.state().logical_scroll_top().item_ix,
            e.list.len(),
        )
    });
    assert_eq!(count, count_before - 1, "a line merge removes exactly one line");
    assert!(
        top >= 80,
        "after a newline delete below the fold the viewport must stay anchored \
         (was item 80), not jump to the top of the file — got item {top}"
    );
}

/// A pure COLUMN move (cursor_line unchanged) must re-reveal the cursor's line.
/// The reveal anchor used to be `(edit_seq, cursor_line)` only, so moving the
/// caret horizontally along a wide soft-wrapped line (e.g. a markdown table row)
/// never scrolled — the caret on a wrapped continuation row drifted off-screen.
/// With `cursor_col` in the anchor, the column move scrolls the line back into
/// view.
#[gpui::test]
fn edit_column_move_reveals_cursor_line(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        );
        // Line 0 is a wide (wrapping) row; many short lines follow it.
        let mut buf = String::new();
        buf.push_str(&"word ".repeat(60));
        buf.push('\n');
        for i in 0..80 {
            buf.push_str(&format!("line {i}\n"));
        }
        v.test_open_edit(&buf);
        v
    });
    vcx.run_until_parked();

    // Scroll the viewport far below line 0 (so line 0 is off-screen), WITHOUT a
    // cursor change — the reveal must not fire yet (cursor still at 0,0).
    view.update(vcx, |v, _cx| {
        let e = v.edit_mut().expect("edit view");
        e.list.state().scroll_to(gpui::ListOffset {
            item_ix: 50,
            offset_in_item: px(0.),
        });
    });

    // A column-only move on line 0 (no edit, same line) must re-reveal it.
    view.update(vcx, |v, cx| {
        let e = v.edit_mut().expect("edit view");
        e.editor.set_cursor(0, 30);
        cx.notify();
    });
    vcx.run_until_parked();

    let top = view.update(vcx, |v, _| {
        v.edit_mut()
            .expect("edit view")
            .list
            .state()
            .logical_scroll_top()
            .item_ix
    });
    assert_eq!(
        top, 0,
        "a column move on line 0 must scroll it back into view (item 0), got {top}"
    );
}

/// Latency gate (audit #1/#2): prove the Doc view renders **O(visible)**, not
/// O(document). Open a 3000-block doc and render it — the virtualized
/// `gpui::list` must build only the visible block window (a few dozen
/// `block_element`s), NOT one per block. Then move the focused block and
/// re-render: the build count must stay bounded, never spiking toward 3000.
/// The assertion is on the deterministic block-build counter, not wall-clock.
#[gpui::test]
fn doc_view_render_is_o_visible(cx: &mut TestAppContext) {
    const N: usize = 3000;
    // Generous ceiling: a tall window plus list overdraw still builds only a
    // small constant window. The point is O(visible) << O(document), so this
    // is far below N. (Empirically a few dozen; 200 leaves slack for overdraw
    // and future window-size changes without ever approaching N.)
    const VISIBLE_CEILING: usize = 200;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        );
        // 3000 short paragraph blocks — each is its own `RenderedBlock`, so the
        // doc list has N items. (One block per non-blank line.)
        let mut md = String::with_capacity(N * 16);
        for i in 0..N {
            md.push_str(&format!("Paragraph block number {i}.\n\n"));
        }
        v.test_open_doc(&md);
        v
    });

    // --- Cold paint: force a fresh frame and count the blocks built. The
    //     counter was zeroed by `test_open_doc`; a `notify` + `run_until_parked`
    //     drives exactly one virtualized render. Virtualization must build only
    //     the visible window, not all N blocks. ---
    YaldaGpuiView::test_reset_doc_block_builds();
    view.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    let cold = YaldaGpuiView::test_doc_block_builds();
    assert!(
        cold > 0,
        "doc list must build the visible window (got 0 — splash not cleared / list not rendering)"
    );
    assert!(
        cold <= VISIBLE_CEILING,
        "cold doc paint must be O(visible) (<= {VISIBLE_CEILING}), got {cold} for a {N}-block doc"
    );
    assert!(
        cold < N / 2,
        "cold doc paint built {cold} of {N} blocks — virtualization is not in effect"
    );

    // --- Move the focused block and re-render. The build count must stay
    //     bounded (still just the visible window), never O(document). ---
    YaldaGpuiView::test_reset_doc_block_builds();
    view.update(vcx, |v, cx| {
        if let Some(d) = v.doc_mut() {
            d.cursor_block = 1500; // jump into the middle of the doc
        }
        cx.notify();
    });
    vcx.run_until_parked();
    let after_move = YaldaGpuiView::test_doc_block_builds();
    assert!(
        after_move <= VISIBLE_CEILING,
        "doc render after a cursor-block move must stay O(visible) (<= {VISIBLE_CEILING}), got {after_move}"
    );
}

/// Regression gate for the `TextLayout::bounds()` panic (runtime-only class the
/// other gates missed). `doc_pos_at` (mouse hit-testing) iterates **every**
/// entry in `line_layouts` and calls `.bounds()`, which panics on a layout that
/// was measured but not prepainted. `ListSizingBehavior::Auto` measured ALL doc
/// lines, registering thousands of un-prepainted layouts → crash on the next
/// mouse-over-doc. This drives a hit-test that touches the whole sink to prove
/// every registered layout is painted (bounds set).
#[gpui::test]
fn doc_hit_test_never_touches_unpainted_layout(cx: &mut TestAppContext) {
    const N: usize = 3000;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        );
        let mut md = String::with_capacity(N * 16);
        for i in 0..N {
            md.push_str(&format!("Paragraph block number {i}.\n\n"));
        }
        v.test_open_doc(&md);
        v
    });
    vcx.run_until_parked();

    // Sanity: the virtualized list actually rendered lines (not a collapsed,
    // zero-height body — which removing `Auto` could cause if the parent didn't
    // bound the height). Otherwise the hit-test below would be vacuous.
    let registered = view.update(vcx, |v, cx| v.line_layouts.borrow().len());
    assert!(
        registered > 0,
        "doc body rendered no lines (list collapsed?)"
    );
    assert!(
        registered < N,
        "registered {registered} of {N} layouts — measuring all lines again (Auto regressed?)"
    );

    // Hit-test FAR BELOW all content: `doc_pos_at` must iterate every registered
    // layout and call `.bounds()` on each. If any registered line was measured
    // but not prepainted, this panics → test fails. With visible-only measuring
    // every registered layout is painted, so it returns None safely.
    let miss = view.update(vcx, |v, cx| v.doc_pos_at(point(px(40.0), px(1_000_000.0))));
    assert!(miss.is_none(), "a point far below all content hits no line");

    // And hit-testing still resolves: a point inside a painted line yields a pos.
    let p = view.update(vcx, |v, cx| {
        let ll = v.line_layouts.borrow();
        let b = ll.values().next().expect("a painted line").bounds();
        point(b.left() + px(2.0), b.top() + px(2.0))
    });
    let hit = view.update(vcx, |v, cx| v.doc_pos_at(p));
    assert!(
        hit.is_some(),
        "a point inside a painted line resolves to a DocPos"
    );
}

/// End-to-end click-drag selection, verified WITHOUT pixels (GPUI's test
/// platform discards the rendered scene). Drives real `simulate_mouse_*` events
/// through the actual `doc_mouse_*` handlers (the path that panicked), then
/// inspects the *render-decision tap* — what the renderer decided to paint /
/// highlight. Asserts: a selection model exists; every painted line within the
/// dragged block range received the selection background; and all highlighted
/// lines were actually painted (no decision on an un-shown line).
#[gpui::test]
fn doc_selection_drag_highlights_dragged_lines(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    use std::collections::{BTreeSet, HashSet};
    const N: usize = 200;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        );
        // One single-line paragraph block per line, so a vertical drag spans a
        // known, contiguous range of block indices.
        let mut md = String::with_capacity(N * 24);
        for i in 0..N {
            md.push_str(&format!("Paragraph block number {i}.\n\n"));
        }
        v.test_open_doc(&md);
        v
    });
    // Settle the one-time geometry change the always-on jump panel induces (it
    // insets the content area) before reading painted bounds — otherwise the
    // drag endpoints would be sampled from the pre-settle (wider) layout and the
    // simulated clicks would miss the re-laid lines. See the matching note in
    // `transcript_021_chatbox_keystroke_is_render_flat`.
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    // Pick drag endpoints from the REAL painted bounds: top-most painted line →
    // bottom-most painted line (both guaranteed prepainted, so bounds() is safe).
    let (start, end, start_block, end_block) = view.update(vcx, |v, cx| {
        let ll = v.line_layouts.borrow();
        let mut keys: Vec<(usize, usize)> = ll.keys().copied().collect();
        keys.sort();
        assert!(
            keys.len() >= 3,
            "need several painted lines, got {}",
            keys.len()
        );
        let a = keys[0];
        let b = keys[keys.len() - 1];
        let ba = ll.get(&a).unwrap().bounds();
        let bb = ll.get(&b).unwrap().bounds();
        (
            point(ba.left() + px(2.0), ba.top() + px(2.0)),
            point(bb.right() - px(2.0), bb.top() + px(2.0)),
            a.0,
            b.0,
        )
    });

    // Real click-drag through the actual handlers (down/move call doc_pos_at).
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();

    // Model: a selection exists after the drag.
    assert!(
        view.update(vcx, |v, cx| v.doc_selection.is_some()),
        "drag produced no doc selection"
    );

    // Decision tap: render one clean frame and inspect what was highlighted.
    YaldaGpuiView::test_reset_doc_render_tap();
    view.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    let tap = YaldaGpuiView::test_doc_render_tap();

    assert!(
        !tap.selection.is_empty(),
        "no line received selection background"
    );
    let selected: HashSet<(usize, usize)> =
        tap.selection.iter().map(|&(b, l, ..)| (b, l)).collect();
    // No inverted byte ranges.
    for &(b, l, s, e) in &tap.selection {
        assert!(
            e >= s,
            "inverted selection byte range on ({b},{l}): {s}..{e}"
        );
    }
    // Every PAINTED line within the dragged block range is highlighted — the
    // visual correctness claim, deterministic and pixel-free.
    for &(b, l) in &tap.painted {
        if b >= start_block && b <= end_block {
            assert!(
                selected.contains(&(b, l)),
                "painted line ({b},{l}) in dragged range {start_block}..={end_block} not highlighted"
            );
        }
    }
    // And the highlighted range actually reaches both drag endpoints.
    let sel_blocks: BTreeSet<usize> = tap.selection.iter().map(|&(b, ..)| b).collect();
    assert!(
        sel_blocks.contains(&start_block) && sel_blocks.contains(&end_block),
        "highlighted blocks {sel_blocks:?} don't span dragged {start_block}..={end_block}"
    );
}

/// UXI-Selection-1 (doc surface): X11-style select-to-clipboard. Finalizing a
/// non-empty mouse drag over the rendered doc writes the selected text to the
/// system clipboard automatically — no Cmd-C. Drives the REAL `doc_mouse_*`
/// handlers, then reads the clipboard back through the test platform.
///
/// Negative control: revert the `write_to_clipboard` branch in `doc_mouse_up`
/// and this asserts the clipboard is stale/empty (fails RED for the right
/// reason — the copy didn't happen).
#[gpui::test]
fn doc_drag_autocopies_selection_to_clipboard(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    const N: usize = 40;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        );
        let mut md = String::with_capacity(N * 24);
        for i in 0..N {
            md.push_str(&format!("Paragraph block number {i}.\n\n"));
        }
        v.test_open_doc(&md);
        v
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    // Seed the clipboard with a sentinel so a "no copy happened" bug is
    // distinguishable from an empty write.
    view.update(vcx, |_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("SENTINEL-NOT-COPIED".into()))
    });

    // Drag across the first painted line (left edge → right edge) so the whole
    // line's text is selected.
    let (start, end) = view.update(vcx, |v, cx| {
        let ll = v.line_layouts.borrow();
        let mut keys: Vec<(usize, usize)> = ll.keys().copied().collect();
        keys.sort();
        let b = ll.get(&keys[0]).unwrap().bounds();
        (
            point(b.left() + px(1.0), b.top() + px(2.0)),
            point(b.right() - px(1.0), b.top() + px(2.0)),
        )
    });
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();

    let clip = view
        .update(vcx, |_, cx| cx.read_from_clipboard())
        .and_then(|it| it.text())
        .unwrap_or_default();
    assert_ne!(
        clip, "SENTINEL-NOT-COPIED",
        "drag-release did not overwrite the clipboard — auto-copy never fired"
    );
    assert!(
        clip.contains("Paragraph block number 0"),
        "clipboard {clip:?} does not hold the selected first-line text"
    );
}

// ---- M0: agent pump/reconciler SEAM tests --------------------------------
//
// The double-render and resume bugs both lived in the seam between the pure
// reconciler (unit-tested in `agent_transcript`) and the impure server pump
// (`apply_server_batch`) + bind (`apply_open_agent_resolution`). These drive
// the REAL pump path through a headless view so a regression in the wiring —
// not just the pure logic — is caught here.

/// Hermetic headless browser view: the session server is forced OFF via the
/// test seam (`with_no_session_server`), so the harness never reaches out to
/// whatever `yalda-session-server` is running on the dev box. With no server,
/// `spawn_attach_sessions` / `spawn_list_sessions_for_picker` early-return, so
/// binds are deterministic and survive `run_until_parked`. Returns the
/// build-root closure to hand to `add_window_view` (whose `&mut
/// VisualTestContext` return tie-up keeps it from being wrapped in a helper).
#[cfg(test)]
fn hermetic_browser_view(
    window: &mut gpui::Window,
    cx: &mut gpui::Context<YaldaGpuiView>,
) -> YaldaGpuiView {
    let focus_handle = cx.focus_handle();
    focus_handle.focus(window);
    crate::with_no_session_server(|| {
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    })
}

/// Install an agent tile on `view` bound to one server-managed session,
/// optionally carrying `server_sid`. The focused tile shows the new session.
#[cfg(test)]
fn install_agent_slot(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    server_sid: Option<&str>,
) {
    use crate::{AgentSession, AgentState, AgentTile, App};
    let sid = server_sid.map(|s| s.to_string());
    view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        let session = AgentSession {
            state: AgentState::new_server_managed(None),
            label: "claude-1".into(),
            cwd: PathBuf::from("."),
            resume_id: None,
        };
        let id = v.show_local_session(session, cx);
        if let Some(sid) = sid {
            v.sessions.bind_sid(id, sid).expect("fresh sid binds");
        }
        let _ = id;
    });
}

/// Boot a hermetic browser view (no agent tile focused) so a session created on
/// it lands **free** (no tile binds it) — the jump-panel "open elsewhere" case.
#[cfg(test)]
fn boot_browser<'a>(
    cx: &'a mut TestAppContext,
) -> (gpui::Entity<YaldaGpuiView>, &'a mut gpui::VisualTestContext) {
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    // Dismiss the splash so `render` reaches the real screen (incl. the jump
    // panel) — wall-clock doesn't advance under `run_until_parked`, so the
    // splash deadline never expires headlessly.
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();
    (view, vcx)
}

/// Add a **free** agent session to the store (the focused tile is a browser, so
/// `show_local_session` binds nothing). Returns its `SessionId`.
#[cfg(test)]
fn add_free_session(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    label: &str,
) -> crate::SessionId {
    use crate::{AgentSession, AgentState};
    let label = label.to_string();
    view.update(vcx, |v, cx| {
        let session = AgentSession {
            state: AgentState::new_server_managed(None),
            label,
            cwd: PathBuf::from("."),
            resume_id: None,
        };
        let id = v.show_local_session(session, cx);
        // Mirror production: session lifecycle changes notify the root.
        cx.notify();
        id
    })
}

/// The inline jump panel renders without disturbing the cached transcript: a
/// chatbox keystroke (compose-only session mutation) must still leave the
/// TRANSCRIPT render-flat even though the panel shares the root render. (The
/// panel itself is intentionally not cached — see `render_jump_panel`; the
/// guarantee that matters is that it doesn't bloat expensive cached surfaces,
/// which `transcript_021_*` / `linear_*_is_render_flat` enforce.) This test
/// pins that the panel is actually live in the tree by rendering with sessions
/// present.
#[gpui::test]
fn jump_panel_renders_with_sessions(cx: &mut TestAppContext) {
    crate::perf_reset("jump_panel");
    let (_view, vcx, _id, _session) = boot_with_transcript(cx);
    vcx.run_until_parked();
    assert!(
        crate::perf_render_count("jump_panel") >= 1,
        "the jump panel must render as part of the root frame"
    );
}

/// UXI-Workspace-1: `ctrl-<n>` jumps straight to the n-th workspace — the digit the
/// jump panel shows. Exercises the FULL dispatch chain: `register_keymap`
/// installed `ctrl-3 → GotoWorkspace3`, and the focused screen root wired
/// `.workspace_nav(cx)` so the action lands on `goto_workspace_number`. A digit
/// past the last workspace is a no-op (no panic, no spurious switch).
#[gpui::test]
fn ctrl_digit_switches_workspace(cx: &mut TestAppContext) {
    use crate::workspace::WorkspaceCwd;
    use crate::{App, BrowserWindow, BufferApp};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);

    // Build four real (non-ephemeral) workspaces: the boot tab + three more
    // INHABITED browser tabs (an empty-layout workspace renders a bare div with
    // no action handlers, so its root couldn't dispatch the global jump — a
    // separate edge state; here we want real, switchable workspaces).
    view.update(vcx, |v, _| {
        let cwd = PathBuf::from(".");
        for _ in 0..3 {
            v.workspace.push_initial_tab(
                App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd.clone()))),
                WorkspaceCwd::new(cwd.clone()),
            );
        }
        assert_eq!(v.workspace.tabs.len(), 4);
        v.workspace.set_active_tab(0); // start the keystroke run on workspace 1
        assert_eq!(v.workspace.active_tab, 0);
    });
    vcx.run_until_parked();

    // ctrl-3 → third workspace (index 2).
    vcx.simulate_keystrokes("ctrl-3");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.active_tab, 2, "ctrl-3 selects the 3rd workspace");
    });

    // ctrl-1 → back to the first.
    vcx.simulate_keystrokes("ctrl-1");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.active_tab, 0, "ctrl-1 selects the 1st workspace");
    });

    // ctrl-9 with only four workspaces is a no-op (stays put).
    vcx.simulate_keystrokes("ctrl-9");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_tab, 0,
            "a digit past the last workspace does nothing"
        );
    });
}

/// REGRESSION ("ctrl-tab does nothing"): tab-cycling (NextTab/PrevTab) was wired ONLY
/// on the doc screen, so it was DEAD whenever an agent/worksheet tile was focused
/// (where the user actually was). It's now in `workspace_nav`, wired on every screen.
/// This drives the AGENT screen (not the browser) and switches with the reliable
/// `cmd-shift-[`/`]` binding (Ctrl-Tab is OS-mangled on macOS — a 4th genuine gap;
/// this test is focus-accurate, proving the WIRING, not the OS delivery). RED before
/// folding next_tab/prev_tab into `workspace_nav`.
#[gpui::test]
fn workspace_cycle_works_from_the_agent_screen(cx: &mut TestAppContext) {
    use crate::workspace::WorkspaceCwd;
    use crate::{App, BrowserWindow, BufferApp};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_worksheet_nav(cx); // FOCUSED on an agent/worksheet tile
    view.update(vcx, |v, _| {
        let cwd = PathBuf::from(".");
        for _ in 0..2 {
            v.workspace.push_initial_tab(
                App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd.clone()))),
                WorkspaceCwd::new(cwd.clone()),
            );
        }
        assert_eq!(v.workspace.tabs.len(), 3, "agent workspace + 2 more");
        v.workspace.set_active_tab(0); // start on the agent workspace
    });
    vcx.run_until_parked();

    vcx.simulate_keystrokes("cmd-shift-]");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_tab, 1,
            "cmd-shift-] advances the workspace FROM THE AGENT SCREEN (the dead-wiring bug)"
        );
    });

    vcx.simulate_keystrokes("cmd-shift-[");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.active_tab, 0, "cmd-shift-[ goes back");
    });
}

/// `goto_workspace_number` numbers NON-ephemeral workspaces 1..N — the same
/// numbering the jump panel paints (`idx + 1`) and skips ephemeral virtual
/// workspaces, so the displayed digit and the `ctrl-<n>` target always agree.
#[gpui::test]
fn workspace_number_skips_ephemeral(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let sid = add_free_session(&view, vcx, "claude-1");
    view.update(vcx, |v, _| v.push_empty_workspace()); // workspaces 1 and 2 are real
    // Open the free session → an ephemeral workspace is appended (sorts last).
    view.update(vcx, |v, cx| v.jump_to_session(sid, cx));
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.tabs.len(), 3, "two real + one ephemeral");
        assert!(v.workspace.active_is_ephemeral());
    });
    // ctrl-2 must land on the 2nd REAL workspace (index 1), not the ephemeral.
    view.update(vcx, |v, cx| v.goto_workspace_number(2, cx));
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.active_tab, 1, "number 2 = 2nd non-ephemeral tab");
        assert!(!v.workspace.active_is_ephemeral());
    });
}

/// UXI-JumpPanel-1: the jump-panel agent status dot reflects the session's turn phase.
/// `dot_status` is the headless-verifiable mapping (the actual hue is a paint
/// detail — gap 1). Working while a reply is in flight; waiting-for-you once the
/// turn finishes; neutral when disconnected.
#[gpui::test]
fn agent_status_dot_reflects_turn_phase(cx: &mut TestAppContext) {
    use crate::AgentDotStatus;
    let (view, vcx) = boot_browser(cx);
    let id = add_free_session(&view, vcx, "claude-1");

    let status = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.read_with(vcx, |v, cx| {
            let rows = v.jump_panel_agent_rows(cx);
            assert_eq!(rows.len(), 1, "exactly the one session we added");
            rows[0].dot_status()
        })
    };

    // Idle (fresh session, turn finished) → it's the user's move.
    assert_eq!(
        status(&view, vcx),
        AgentDotStatus::WaitingForYou,
        "an idle session waits for you"
    );

    // A reply in flight → working.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.turn_phase = crate::TurnPhase::begin(std::time::Instant::now())
        });
    });
    assert_eq!(
        status(&view, vcx),
        AgentDotStatus::Working,
        "a mid-turn session is working"
    );
}

/// Unit: the dot-status mapping is total and disconnected wins (a disconnected
/// agent shows neutral even if it was mid-turn before the drop).
#[test]
fn agent_dot_status_mapping() {
    use crate::{AgentDotStatus, AgentRow, JumpTarget};
    let row = |connected, awaiting| AgentRow {
        target: JumpTarget::Roster("s".into()),
        label: "x".into(),
        cwd: std::path::PathBuf::from("/"),
        bound: false,
        connected,
        awaiting,
    };
    assert_eq!(row(true, Some(true)).dot_status(), AgentDotStatus::Working);
    assert_eq!(
        row(true, Some(false)).dot_status(),
        AgentDotStatus::WaitingForYou
    );
    assert_eq!(row(true, None).dot_status(), AgentDotStatus::Neutral);
    assert_eq!(row(false, Some(true)).dot_status(), AgentDotStatus::Neutral);
}

/// Jump-panel selection of a FREE session opens an ephemeral virtual workspace
/// (ADR-0021): a new single-tile tab bound to the session, made active. Leaving
/// it (any workspace switch) tears it down and returns the session to free —
/// the session itself survives in the store the whole time.
#[gpui::test]
fn jump_to_free_session_opens_then_tears_down_ephemeral(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let sid = add_free_session(&view, vcx, "claude-1");

    // Precondition: one real tab, session is free.
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.tabs.len(), 1, "starts with one workspace");
        assert!(
            v.agent_tile_id_bound_to(sid).is_none(),
            "session starts free"
        );
    });

    // Jump to the free session → ephemeral virtual workspace.
    view.update(vcx, |v, cx| v.jump_to_session(sid, cx));
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.tabs.len(), 2, "ephemeral workspace added");
        assert!(
            v.workspace.active_is_ephemeral(),
            "the ephemeral workspace is active"
        );
        assert!(
            v.agent_tile_id_bound_to(sid).is_some(),
            "the ephemeral tile binds the session"
        );
    });

    // Jump away (back to the real workspace 0) → ephemeral torn down, free again.
    view.update(vcx, |v, cx| v.select_tab(0, cx));
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.tabs.len(), 1, "ephemeral workspace torn down");
        assert!(
            !v.workspace.active_is_ephemeral(),
            "no ephemeral workspace remains"
        );
        assert!(
            v.agent_tile_id_bound_to(sid).is_none(),
            "session returned to free"
        );
        assert!(v.sessions.contains(sid), "session itself survives the teardown");
    });
}

/// Selecting a *different* free session while a virtual workspace is open
/// REPLACES it (we never accumulate more than one ephemeral tab), and the first
/// session returns to free.
#[gpui::test]
fn jump_to_second_free_session_replaces_ephemeral(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let a = add_free_session(&view, vcx, "claude-1");
    let b = add_free_session(&view, vcx, "claude-2");

    view.update(vcx, |v, cx| v.jump_to_session(a, cx));
    view.update(vcx, |v, cx| v.jump_to_session(b, cx));
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.tabs.len(), 2, "still exactly one ephemeral tab");
        assert!(
            v.agent_tile_id_bound_to(b).is_some(),
            "the second session is now shown"
        );
        assert!(
            v.agent_tile_id_bound_to(a).is_none(),
            "the first session returned to free"
        );
        assert!(v.sessions.contains(a) && v.sessions.contains(b));
    });
}

/// Jump-panel selection of a BOUND session focuses its existing tile in place —
/// no new tile, no ephemeral workspace (the 1:1 invariant is preserved).
#[gpui::test]
fn jump_to_bound_session_focuses_existing_tile(cx: &mut TestAppContext) {
    use crate::{App, BrowserWindow, BufferApp};
    let (view, vcx) = boot_browser(cx);
    // Workspace 0: an agent tile bound to S1.
    install_agent_slot(&view, vcx, Some("S1"));
    let (sid, owner_tab, owner_wid) = view.update(vcx, |v, _| {
        let sid = v.sessions.locate("S1").expect("S1 bound");
        let wid = v.agent_tile_id_bound_to(sid).expect("S1 has a tile");
        let tab = v.workspace.tab_containing(wid).expect("tile in a tab");
        (sid, tab, wid)
    });
    // Add a second workspace and switch to it, so jumping must cross back.
    view.update(vcx, |v, cx| {
        v.workspace.push_initial_tab(
            App::Buffer(BufferApp::Picking(BrowserWindow::standalone(PathBuf::from(".")))),
            crate::workspace::WorkspaceCwd::new(PathBuf::from(".")),
        );
        cx.notify();
    });
    let tabs_before = view.update(vcx, |v, _| v.workspace.tabs.len());

    view.update(vcx, |v, cx| v.jump_to_session(sid, cx));
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.tabs.len(),
            tabs_before,
            "no new tile/workspace created for a bound session"
        );
        assert_eq!(
            v.workspace.active_tab, owner_tab,
            "focus moved to the owner's workspace"
        );
        assert_eq!(
            v.agent_tile_id_bound_to(sid),
            Some(owner_wid),
            "still the same single bound tile"
        );
    });
}

/// Strict 1:1: a server session is bound by at most ONE tile, even across
/// workspaces. Resolving an AlreadyBound conflict must NOT bind a second tile to
/// the owner — that regression let the same session show in two workspaces. The
/// duplicate tile returns to a selector and focus navigates to the owner.
#[gpui::test]
fn agent_session_binds_at_most_one_tile(cx: &mut TestAppContext) {
    use crate::{AgentTile, App};
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();

    // Workspace 0: an agent tile bound to session "S1".
    install_agent_slot(&view, vcx, Some("S1"));
    let owner = view.update(vcx, |v, _| v.sessions.locate("S1").expect("S1 bound"));

    // Workspace 1: a fresh agent tile, now focused.
    view.update(vcx, |v, _cx| {
        v.workspace.push_initial_tab(
            App::Agent(AgentTile::new()),
            crate::workspace::WorkspaceCwd::new(PathBuf::from(".")),
        );
    });

    // Attempt to bind the already-owned session from workspace 1's tile.
    view.update(vcx, |v, cx| v.focus_existing_session(owner, cx));

    let (bound_tiles, active) = view.update(vcx, |v, _| {
        let mut n = 0;
        for tab in v.workspace.tabs.iter() {
            tab.layout.for_each_leaf(&mut |w| {
                if let App::Agent(t) = &w.content
                    && t.bound == Some(owner)
                {
                    n += 1;
                }
            });
        }
        (n, v.workspace.active_tab)
    });
    assert_eq!(bound_tiles, 1, "a session binds at most one tile");
    assert_eq!(active, 0, "focus navigated to the owning workspace");
}

/// Reopening a multiturn session into Worksheet mode lands the caret on an
/// EDITABLE tail with the transcript intact and in order. Replays a transcript
/// (agent text + a tool call + more text) ending in `ReplayComplete` — exactly
/// what attaching to an existing session does — then switches to Worksheet and
/// asserts the caret is findable on a non-frozen last line. Guards the basic
/// resume→worksheet path the user reported broken (the actual repro needs a
/// more specific trigger; this pins that the happy path stays correct).
#[gpui::test]
fn worksheet_resume_multiturn_caret_on_editable_tail(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ReplyEvent, ToolCall};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        let tc = ToolCall::new("tool-1", "Read");
        let batch = vec![
            ev(ReplyEvent::Chunk("agent turn one reply\n".into())),
            ev(ReplyEvent::UserMessage("user second prompt\n".into())),
            ev(ReplyEvent::ToolCallStarted(tc)),
            ev(ReplyEvent::Chunk("agent turn two after the tool\n".into())),
            ev(ReplyEvent::ReplayComplete),
        ];
        v.apply_server_batch(batch, cx);
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let text = c.editor.document().full_text();
        assert!(text.contains("agent turn one reply"), "turn-one text present");
        assert!(
            text.contains("agent turn two after the tool"),
            "turn-two text present and ordered after the tool"
        );
        let last = c.editor.document().line_count().saturating_sub(1);
        assert_eq!(c.editor.cursor().line, last, "caret on the last line");
        assert!(
            !c.editor.is_frozen_line(last),
            "the caret's last line is an EDITABLE tail — the user can find it and type"
        );
    });
}

/// REGRESSION (live report "the cursor can go below the end of the visible
/// buffer"): when a session is ALREADY in Worksheet mode while replay populates
/// the transcript — the restore path, where `slot.mode == Worksheet` is set on
/// the fresh session BEFORE its history streams in — the caret must end on the
/// editable tail (the last line), not stranded at line 0 where it was born.
/// Unlike `worksheet_resume_multiturn_caret_on_editable_tail` (which toggles
/// INTO worksheet after replay, exercising the toggle path), here worksheet is
/// entered before any content, so only an end-of-replay snap lands the caret.
#[gpui::test]
fn worksheet_already_active_during_replay_lands_caret_on_tail(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Restore path: the persisted slot was in Worksheet mode, so the session is
    // flipped to Worksheet BEFORE its history replays (mirrors agent_ui.rs:90 /
    // main.rs:1898). The caret is at its birth position (line 0).
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
        assert_eq!(c.editor.cursor().line, 0, "caret starts at line 0");
    });

    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        let batch = vec![
            ev(ReplyEvent::Chunk("agent reply line one\n".into())),
            ev(ReplyEvent::Chunk("agent reply line two\n".into())),
            ev(ReplyEvent::ReplayComplete),
        ];
        v.apply_server_batch(batch, cx);
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let last = c.editor.document().line_count().saturating_sub(1);
        assert!(last > 0, "transcript actually grew during replay");
        assert_eq!(
            c.editor.cursor().line,
            last,
            "caret must snap to the editable tail at end of replay, not stay at line 0"
        );
        assert!(c.pending_reveal_cursor, "tail snap queues a viewport reveal");
    });
}

/// REGRESSION (live screenshot "interleaved toolcalls with agent text"): a
/// single agent text run streamed as two deltas with a tool call landing
/// BETWEEN them, while the run is still OPEN (first delta ends mid-token, no
/// trailing '\n'), must NOT be split around the tool. The reducer used to force
/// the continuation to EOF below the tool anchor (`find_llm_insertion_point`'s
/// `ends_with('\n')` → different-turn → EOF branch), bisecting the token —
/// e.g. the code span `mode=max` rendered as "`m" | ToolSearch | "ode=max".
/// Fix: an open run rejoins at end-of-content so the token stays whole and the
/// tool group renders AFTER the completed text. Drives the REAL reducer path
/// (`apply_server_batch` → `apply_reply_events` → `append_llm_chunk_floored`).
#[gpui::test]
fn tool_call_midtoken_does_not_split_agent_text_run(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ReplyEvent, ToolCall};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx);

    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        let tc = ToolCall::new("tool-1", "ToolSearch");
        let batch = vec![
            // First delta ends mid-token (no trailing '\n') — the run is OPEN.
            ev(ReplyEvent::Chunk("only re-push the 8 GB `m".into())),
            ev(ReplyEvent::ToolCallStarted(tc)),
            // Continuation completes the `mode=max` token.
            ev(ReplyEvent::Chunk("ode=max cache when inputs changed.\n".into())),
        ];
        v.apply_server_batch(batch, cx);
    });
    vcx.run_until_parked();
    // Force a view-model rebuild so flat items reflect the current buffer.
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let text = c.editor.document().full_text();
        // The token is WHOLE — no tool/blank line spliced through `mode=max`.
        assert!(
            text.contains("8 GB `mode=max cache"),
            "agent text run stays contiguous across the interrupting tool call; got:\n{text:?}"
        );
        // Order preserved: the tool group still renders, AFTER the reassembled
        // text line (never before it, never inside it).
        let items = &c.view_model.flat_items_cache;
        let text_idx = items.iter().position(|it| {
            matches!(it, crate::FlatItem::Line(l)
                if c.editor.document().line_text(*l).contains("mode=max"))
        });
        let tool_idx = items
            .iter()
            .position(|it| matches!(it, crate::FlatItem::ToolGroup { .. }));
        let text_idx = text_idx.expect("the reassembled agent line is rendered");
        let tool_idx = tool_idx.expect("the tool group is rendered");
        assert!(
            tool_idx > text_idx,
            "tool group renders AFTER the completed text run (text@{text_idx}, tool@{tool_idx})"
        );
    });
}

/// REGRESSION (live report "undo erased the buffer"): agent content that
/// streams while the user is mid-insert in Worksheet mode must NOT become
/// user-undoable. The bug: `begin_insert` opens ONE undo group for the whole
/// insert session; an agent chunk's `programmatic_insert` recorded into that
/// open group, so undoing the user's edit reverted the ENTIRE transcript. Fix:
/// programmatic (agent) splices are non-undoable and only shift the user's own
/// recorded splices. Here undo must remove the USER's text but keep every agent
/// turn.
#[gpui::test]
fn worksheet_resume_undo_does_not_erase_transcript(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ReplyEvent, ToolCall};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    // Worksheet mode, and the user is in INSERT mode (composing / hunting for
    // the caret) — which opens ONE undo group for the whole insert session.
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        c.editor.begin_insert();
        for ch in "my reply".chars() {
            c.editor.insert_char(ch);
        }
    });

    // Now the agent streams (a new turn / a resume replay) WHILE that group is
    // still open. Each programmatic chunk records into the USER's group.
    view.update(vcx, |v, cx| {
        let tc = ToolCall::new("tool-1", "Read");
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk("agent turn one reply\n".into())),
                ev(ReplyEvent::ToolCallStarted(tc)),
                ev(ReplyEvent::Chunk("agent turn two after the tool\n".into())),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    // User drops back to Normal (Esc) — this COMMITS the insert group, which now
    // holds the user's text AND the agent chunks that streamed into it.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().editor.end_insert();
    });

    let before = view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().editor.document().full_text()
    });
    // Undo a bunch, exactly as the user did.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        for _ in 0..15 {
            c.editor.undo();
        }
    });
    let after = view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().editor.document().full_text()
    });
    assert!(
        before.contains("my reply"),
        "sanity: the user's typed text was present before undo.\n{before}"
    );
    assert!(
        after.contains("agent turn one reply") && after.contains("agent turn two after the tool"),
        "undo MUST NOT erase the agent transcript.\nBEFORE:\n{before}\n---\nAFTER:\n{after}"
    );
    assert!(
        !after.contains("my reply"),
        "undo SHOULD still revert the user's OWN edit (just not the agent's).\nAFTER:\n{after}"
    );
}

#[cfg(test)]
fn active_transcript_text(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
) -> String {
    view.update(vcx, |v, cx| {
        v.agent_mut(cx)
            .map(|c| c.editor.document().full_text())
            .unwrap_or_default()
    })
}

/// The double-render regression, driven through the REAL `apply_server_batch`:
/// an assistant chunk streams BEFORE the server replays the user-prompt echo
/// (the exact order the old suffix dedup mishandled). The user's text must
/// appear exactly once.
#[gpui::test]
fn agent_seam_suppresses_double_render_when_chunk_precedes_echo(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::agent_transcript::UserTurnOrigin;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    view.update(vcx, |v, cx| {
        // What submit_chatbox does on send success: optimistic echo of "hello".
        v.agent_mut(cx)
            .unwrap()
            .insert_user_turn("hello", UserTurnOrigin::LocalSubmit, false);
        // Assistant streams FIRST, then the server's UserPrompt echo arrives.
        let batch = vec![
            ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::Chunk("world response text".into()),
            },
            ServerNotification::UserPrompt {
                session_id: "S1".into(),
                text: "hello".into(),
            },
        ];
        v.apply_server_batch(batch, cx);
    });

    let text = active_transcript_text(&view, &mut *vcx);
    assert_eq!(
        text.matches("hello").count(),
        1,
        "user input must render exactly once; transcript was:\n{text}"
    );
    assert!(
        text.contains("world response text"),
        "assistant chunk missing"
    );
}

/// The resume routing-drop invariant: a ReplyEvent whose `session_id` has no
/// bound slot is dropped (this is WHY the bind must precede the attach/replay),
/// and once the slot is bound the same event routes into it. This is the seam
/// the bind-before-attach restructure relies on.
#[gpui::test]
fn agent_seam_routes_reply_only_after_session_is_bound(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    // Slot exists but is NOT yet bound to any server session id.
    install_agent_slot(&view, &mut *vcx, None);

    // Pre-bind: an event for "S1" can't route — it is dropped, not buffered.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::Chunk("early-dropped".into()),
            }],
            cx,
        );
    });
    assert!(
        !active_transcript_text(&view, &mut *vcx).contains("early-dropped"),
        "an event for an unbound session must not route (this is the drop the \
         bind-before-attach restructure exists to avoid)"
    );

    // Bind the sid (what apply_open_agent_resolution does), then re-feed.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("a session is bound");
        v.sessions
            .bind_sid(id, "S1".into())
            .expect("fresh sid binds");
    });
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::Chunk("late-routed".into()),
            }],
            cx,
        );
    });
    assert!(
        active_transcript_text(&view, &mut *vcx).contains("late-routed"),
        "once bound, the session's events must route into its slot"
    );
}

// ---- Action-level smoke (the keymap-extraction payoff) -------------------
//
// First end-to-end keystroke→action→state test. It exercises the FULL GPUI
// dispatch path — real keymap, real action, real handler — that was untestable
// while the keymap lived inline in `main()`'s run-closure. The rail is the
// deliberate target: a repeatedly-regressed surface, and a global (`None`-
// context) binding, so this also proves global Cmd-shortcuts reach a focused
// screen's `on_action` wiring headlessly.

/// `cmd-b` (`ToggleFileBrowserRail`, a global binding) opens a file-browser
/// rail on the active tab, and a second `cmd-b` closes it. This only works if
/// `register_keymap` installed the binding AND the focused screen's root wired
/// `on_action(toggle_file_browser_rail)` — i.e. the whole dispatch chain.
#[gpui::test]
fn cmd_b_toggles_file_browser_rail(cx: &mut TestAppContext) {
    // Install the production keymap on the test app — the extraction this stone
    // depended on. Without it, `simulate_keystrokes` would no-op.
    cx.update(crate::register_keymap);

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();

    // Read the rail kind on the active tab: None = no rail, Some(true) = a
    // file-browser rail, Some(false) = some other rail kind.
    let rail_kind = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            v.workspace
                .active_tab()
                .and_then(|t| t.rail.as_ref())
                .map(|r| r.content.is_file_browser())
        })
    };

    // Precondition: a fresh browser view has no rail open.
    assert_eq!(
        rail_kind(&view, vcx),
        None,
        "fresh view should have no rail"
    );

    // Dismiss the splash overlay so input dispatches against the real screen
    // (the production app auto-clears it after 1.5s; do it directly here).
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();

    // Act: press Cmd-B.
    vcx.simulate_keystrokes("cmd-b");
    vcx.run_until_parked();
    assert_eq!(
        rail_kind(&view, vcx),
        Some(true),
        "cmd-b must open a file-browser rail beside the focused tile"
    );

    // Act again: a second Cmd-B closes the same rail (two-state toggle).
    vcx.simulate_keystrokes("cmd-b");
    vcx.run_until_parked();
    assert_eq!(
        rail_kind(&view, vcx),
        None,
        "a second cmd-b must close the file-browser rail"
    );
}

// ---- Worksheet-submit double-render seam ---------------------------------
//
// The worksheet analogue of `agent_seam_suppresses_double_render_when_chunk_
// precedes_echo`. `submit_worksheet` used to hand-compute its turn number and
// freeze the authored lines WITHOUT routing through the reconciler, so the
// server's `UserPrompt` echo of the same prompt was treated as a new turn and
// re-rendered (the live worksheet double-render). The fix routes the submit
// through `commit_worksheet_turn` -> `register_user_turn`, registering the
// prompt as a `LocalSubmit` so the echo is content-matched and suppressed.
//
// We drive the dedup CORE (`commit_worksheet_turn`) directly rather than the
// full `submit_worksheet`, because the latter only commits inside its `if sent`
// branch and `sent` can never be true headlessly (no session-server daemon, no
// real channel) — driving the entrypoint would pass VACUOUSLY. The negative
// control below proves the assertion actually bites.

/// Seed a single editable (unfrozen) line of `token` into the active agent
/// slot's transcript editor and put it in Worksheet mode — the authored line a
/// worksheet submit would freeze.
#[cfg(test)]
fn seed_worksheet_line(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    token: &str,
) {
    view.update(vcx, |v, cx| {
        let mut claude = v.agent_mut(cx).expect("active agent slot");
        claude.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
        for ch in token.chars() {
            claude.editor.insert_char(ch);
        }
    });
}

/// THE fix: a worksheet submit committed through `commit_worksheet_turn`
/// suppresses the server `UserPrompt` echo, so the authored prompt renders
/// exactly once even when an assistant chunk streams before the echo (the
/// order the old suffix heuristic mishandled).
#[gpui::test]
fn agent_seam_worksheet_submit_suppresses_double_render(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // A single editable line, so the joined prompt_body equals the echo body
    // and suppression is by content identity (the multi-line join equivalence
    // is pinned by the pure reconciler test in `agent_transcript`).
    const TOKEN: &str = "WKSHT_PROMPT_TOKEN";
    seed_worksheet_line(&view, &mut *vcx, TOKEN);

    // Commit the worksheet submit through the shared reconciler core, exactly as
    // `submit_worksheet` does on send-success: derive k, freeze line 0 in place,
    // and register the prompt so the echo is suppressed.
    view.update(vcx, |v, cx| {
        let mut claude = v.agent_mut(cx).unwrap();
        let collected = vec![(0usize, TOKEN.to_string())];
        let k = claude.commit_worksheet_turn(&collected, TOKEN);
        assert_eq!(k, Some(1), "first worksheet submit is turn 1");
    });
    assert_eq!(
        active_transcript_text(&view, &mut *vcx)
            .matches(TOKEN)
            .count(),
        1,
        "authored worksheet line should appear once before the echo"
    );

    // Assistant streams FIRST, then the server's UserPrompt echo arrives. No
    // TurnEnded before the echo — this is the live in-flight case where the
    // reconciler must suppress on content identity alone.
    view.update(vcx, |v, cx| {
        let batch = vec![
            ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::Chunk("assistant reply text".into()),
            },
            ServerNotification::UserPrompt {
                session_id: "S1".into(),
                text: TOKEN.into(),
            },
        ];
        v.apply_server_batch(batch, cx);
    });

    let text = active_transcript_text(&view, &mut *vcx);
    assert_eq!(
        text.matches(TOKEN).count(),
        1,
        "worksheet prompt must render exactly once after the echo; transcript:\n{text}"
    );
    assert!(
        text.contains("assistant reply text"),
        "assistant chunk missing"
    );
}

/// Negative control proving the test above is not vacuous: an authored
/// worksheet line that is NOT registered with the reconciler (the pre-fix
/// behavior) IS double-rendered by the very same echo. If suppression silently
/// stopped working, the test above would still need this contrast to be
/// trustworthy — here the echo is treated as a brand-new turn and appended.
#[gpui::test]
fn agent_seam_worksheet_unregistered_line_double_renders(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    const TOKEN: &str = "WKSHT_PROMPT_TOKEN";
    seed_worksheet_line(&view, &mut *vcx, TOKEN);
    // Deliberately DO NOT call commit_worksheet_turn / register_user_turn — the
    // reconciler never learns this prompt was locally submitted.

    view.update(vcx, |v, cx| {
        let batch = vec![
            ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::Chunk("assistant reply text".into()),
            },
            ServerNotification::UserPrompt {
                session_id: "S1".into(),
                text: TOKEN.into(),
            },
        ];
        v.apply_server_batch(batch, cx);
    });

    assert_eq!(
        active_transcript_text(&view, &mut *vcx)
            .matches(TOKEN)
            .count(),
        2,
        "an un-registered worksheet prompt is double-rendered by the echo (the bug the \
         reconciler chokepoint closes — this is what keeps the suppression test honest)"
    );
}

/// Regression for the live crash: pipelined worksheet submits — a second prompt
/// committed before the first turn's `TurnEnded` advances `last_seen` — must get
/// DISTINCT turn numbers. The naive `k = current_turn() = last_seen + 1` reuses
/// the same `k` for both (worksheet invites submitting again while the agent is
/// still working), which trips the M3 double-insert tripwire and aborts the app.
#[gpui::test]
fn agent_seam_pipelined_worksheet_submits_get_distinct_turns(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Turn 1 has settled (its TurnEnded advanced last_seen to 1).
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().replay_turns.last_seen = 1;
    });

    // First in-flight submit -> turn 2.
    let k1 = view.update(vcx, |v, cx| {
        v.agent_mut(cx)
            .unwrap()
            .commit_worksheet_turn(&[(0usize, "alpha".to_string())], "alpha")
    });
    assert_eq!(k1, Some(2), "first in-flight submit is turn 2");

    // Second submit BEFORE turn 2's boundary advances last_seen (still 1). It
    // must be a distinct turn (3), NOT collide on 2 and trip the tripwire.
    let k2 = view.update(vcx, |v, cx| {
        v.agent_mut(cx)
            .unwrap()
            .commit_worksheet_turn(&[(0usize, "beta".to_string())], "beta")
    });
    assert_eq!(
        k2,
        Some(3),
        "a pipelined submit must get a fresh turn number, not reuse the in-flight turn's k"
    );
}

/// Sibling of the pipelined case: a live/server echo that ISN'T suppressed
/// (its text didn't match a pending submit) and arrives before the in-flight
/// turn's boundary settles must also mint a DISTINCT turn rather than collide
/// on `current_turn()` — the same crash class, reached via `origin = Echo`
/// instead of `LocalSubmit`. Pins the guard to the whole non-replay branch.
#[gpui::test]
fn agent_seam_unsuppressed_echo_during_inflight_turn_gets_distinct_k(cx: &mut TestAppContext) {
    use yalda::agent_transcript::UserTurnOrigin;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().replay_turns.last_seen = 1;
    });

    // Local submit -> turn 2 (in flight; last_seen still 1).
    let k1 = view.update(vcx, |v, cx| {
        v.agent_mut(cx)
            .unwrap()
            .register_user_turn("alpha", UserTurnOrigin::LocalSubmit, false)
    });
    assert_eq!(k1, Some(2));

    // A live echo with NON-matching text (not suppressed) before turn 2's
    // boundary settles. Must be a distinct turn, not a collision on 2.
    let k2 = view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().register_user_turn(
            "a totally different unsuppressed echo",
            UserTurnOrigin::Echo,
            false,
        )
    });
    assert_eq!(
        k2,
        Some(3),
        "an unsuppressed echo during an in-flight turn must mint a fresh k, not reuse it"
    );
}

// ---- ActiveOverlay (A.2: 5 mutually-exclusive Options -> one enum) --------

/// The "make illegal states unrepresentable" payoff: `open_overlay` REPLACES
/// (never stacks), `clear_overlay` resets, and the type system guarantees at
/// most one overlay variant is active — so the old "two overlays Some at once"
/// (a menu stranded behind a rename) is no longer representable.
#[gpui::test]
fn active_overlay_open_replaces_and_clears(cx: &mut TestAppContext) {
    use crate::{ActiveOverlay, BufferSwitcher, WorkspacePicker, WorkspacePickerMode};

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        assert!(!v.has_overlay(), "fresh view has no overlay");

        v.open_overlay(ActiveOverlay::BufferSwitcher(BufferSwitcher {
            selected: 0,
            filter_mode: false,
            filter_text: String::new(),
        }));
        assert!(v.has_overlay() && v.overlay_is_buffer());
        assert!(v.buffer_ref().is_some());
        assert!(
            v.menu_ref().is_none()
                && v.rename_ref().is_none()
                && v.workspace_picker_ref().is_none(),
            "exactly one variant active — mutual exclusion is type-enforced"
        );

        // open REPLACES, never stacks: opening a different overlay drops the
        // previous one (the tab-double-click-behind-menu case can't strand).
        v.open_overlay(ActiveOverlay::WorkspacePicker(WorkspacePicker {
            mode: WorkspacePickerMode::Move,
            selected: 0,
        }));
        assert!(v.overlay_is_workspace());
        assert!(
            v.buffer_ref().is_none(),
            "buffer overlay dropped on replace"
        );

        v.clear_overlay();
        assert!(!v.has_overlay(), "clear_overlay returns to None");
    });
}

// ---- InputSurface (A.11: input_mode + chatbox:Option -> one enum) ---------

/// The merged surface enforces "a chatbox exists IFF Chatbox mode" by type, and
/// the Ctrl-Alt-Enter toggle flips between the two — destroying the box on the
/// way to Worksheet and minting a fresh one on the way back (unchanged
/// behavior, now unrepresentable-illegal-state).
// Model C: toggling is a pure placement flip — the `Compose` value (draft text,
// cursor) is preserved across the round trip, never stranded or dropped. This
// replaces the old asymmetry assertion ("a chatbox exists iff Chatbox variant"),
// which no longer holds: the compose exists in both placements.
#[gpui::test]
fn toggle_preserves_compose_value(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // New sessions default to Worksheet. Type a draft into the compose.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert!(!c.input_surface.is_chatbox(), "default is worksheet");
        let cb = c.input_surface.compose_mut();
        for ch in "hello draft".chars() {
            cb.editor.insert_char(ch);
        }
        assert_eq!(c.input_surface.compose().text(), "hello draft");
    });

    // Toggle -> Chatbox: placement flips, the draft is preserved (not moved into
    // the transcript, not dropped).
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).unwrap();
        assert!(c.input_surface.is_chatbox(), "now in chatbox placement");
        assert_eq!(
            c.input_surface.compose().text(),
            "hello draft",
            "draft survives the placement flip untouched"
        );
    });

    // Toggle back -> Worksheet: still the same draft.
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).unwrap();
        assert!(!c.input_surface.is_chatbox());
        assert_eq!(c.input_surface.compose().text(), "hello draft");
    });
}

/// Model C §4.5: `toggle_agent_focus` flips between compose and the read-only
/// transcript and back. Default focus is `Compose`.
#[gpui::test]
fn toggle_agent_focus_round_trips(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Worksheet (default) rests in transcript nav. toggle → Compose OPENS a block
    // (a visible surface); toggle back → Transcript nav.
    view.update(vcx, |v, cx| {
        assert_eq!(v.agent_mut(cx).unwrap().focus, crate::AgentFocus::Transcript);
    });
    view.update(vcx, |v, cx| v.toggle_agent_focus(cx));
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).unwrap();
        assert_eq!(c.focus, crate::AgentFocus::Compose);
        assert!(c.inline_you_block_active(), "focus→Compose opened a visible block");
    });
    view.update(vcx, |v, cx| v.toggle_agent_focus(cx));
    view.update(vcx, |v, cx| {
        assert_eq!(v.agent_mut(cx).unwrap().focus, crate::AgentFocus::Transcript);
    });
}

/// Model C INV-1: in worksheet placement, a blank submit is a no-op and the
/// transcript stays empty — the draft is never written into the transcript
/// (it lives in the separate compose). Also pins that submit with no channel
/// surfaces a status rather than freezing a phantom turn.
#[gpui::test]
fn worksheet_blank_submit_is_noop(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Default is worksheet; leave the compose blank.
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).unwrap();
        assert!(!c.input_surface.is_chatbox());
        assert!(c.input_surface.compose().text().trim().is_empty());
    });

    let before = view
        .update(vcx, |v, cx| {
            v.agent_mut(cx).unwrap().editor.document().full_text()
        })
        ;
    view.update(vcx, |v, cx| v.submit_compose(cx));
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert_eq!(
            c.editor.document().full_text(),
            before,
            "a blank submit must not write the transcript"
        );
        assert_eq!(
            c.input_surface.mode(),
            crate::InputModeKind::Worksheet,
            "submit preserves placement"
        );
    });
}

// ---- Phase 8 Stage C: AgentEvent TOTAL reducer (NEEDS-RUNTIME) ------------
//
// These drive the canonical `AgentEvent` stream through the REAL
// `apply_server_batch` -> `apply_agent_event` -> `settle_agent_effect` path on a
// headless view. They cover the spec §7 reducer contract (total match, explicit
// Unknown / CompactedSummary arms), the §4 generation rebaseline rule, and the
// §7/H5 idempotent finalize. The events→reducer→VIEW (render) leg is NOT
// covered (GPUI is not headlessly verifiable end-to-end) — see the worklog's
// NEEDS-RUNTIME list.

/// Build a `Notification::Agent` for session `sid` with the given envelope+kind.
#[cfg(test)]
fn agent_note(
    sid: &str,
    generation: u64,
    turn: u64,
    seq: u64,
    kind: yalda::agent_event::AgentEventKind,
) -> yalda::session_proto::Notification {
    yalda::session_proto::Notification::Agent {
        event: yalda::agent_event::AgentEvent::new(sid.into(), generation, turn, seq, kind),
    }
}

/// A boot + one bound server-managed slot, ready to feed batches. Returns the
/// borrowed `VisualTestContext` (lifetime tied to `cx`, like the inline tests).
#[cfg(test)]
fn boot_with_bound_slot<'a>(
    cx: &'a mut TestAppContext,
    sid: &str,
) -> (gpui::Entity<YaldaGpuiView>, &'a mut gpui::VisualTestContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, vcx, Some(sid));
    (view, vcx)
}

/// The end-to-end Stage C reducer: a synthetic AgentEvent sequence drives the
/// transcript via the TOTAL reducer once the §9 gate flips on the first
/// forwarded `TurnEnded`. Asserts (a) the gate stays closed until a boundary so
/// the first turn's chunks are NOT double-applied (legacy stream owns them — but
/// here no legacy stream runs, so they're simply absent until the gate flips on
/// the next turn), (b) once authoritative, chunks land tagged by the FORWARDED
/// `event.turn`, (c) finalize ran (phase Idle) after `TurnEnded`.
#[gpui::test]
fn agent_reducer_drives_transcript_after_gate_flips(cx: &mut TestAppContext) {
    use yalda::agent_event::{AgentEventKind as K, ChunkRole, TurnOutcome};

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // Turn 1: ChannelOpened (rebaselines to gen 1), a chunk, then a TurnEnded.
    // The chunk arrives BEFORE the gate flips, so the reducer must NOT apply it
    // (the legacy stream would, but there's none here). The TurnEnded flips the
    // gate and finalizes turn 1.
    view.update(vcx, |v, cx| {
        let batch = vec![
            agent_note("S1", 1, 1, 0, K::ChannelOpened { resumed: false }),
            agent_note(
                "S1",
                1,
                1,
                1,
                K::Chunk {
                    text: "pre-gate chunk".into(),
                    role: ChunkRole::Message,
                },
            ),
            agent_note(
                "S1",
                1,
                1,
                2,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
        ];
        v.apply_server_batch(batch, cx);
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert_eq!(c.generation, 1, "ChannelOpened rebaselined to gen 1");
        assert!(
            c.agent_stream_authoritative,
            "the forwarded TurnEnded must flip the per-session gate"
        );
        assert!(
            !c.editor.document().full_text().contains("pre-gate chunk"),
            "a pre-gate chunk must NOT be applied by the reducer (legacy stream owns it)"
        );
        assert!(
            matches!(c.turn_phase, crate::TurnPhase::Idle),
            "TurnEnded finalized turn 1 (phase Idle)"
        );
        assert!(
            c.finalized.contains(&(1, 1)),
            "the (generation, turn) ledger records the finalized boundary"
        );
    });

    // Turn 2: now authoritative — chunks land, tagged by event.turn==2.
    view.update(vcx, |v, cx| {
        let batch = vec![
            agent_note(
                "S1",
                1,
                2,
                3,
                K::Chunk {
                    text: "live turn-2 prose".into(),
                    role: ChunkRole::Message,
                },
            ),
            agent_note(
                "S1",
                1,
                2,
                4,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
        ];
        v.apply_server_batch(batch, cx);
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        let text = c.editor.document().full_text();
        assert!(
            text.contains("live turn-2 prose"),
            "post-gate chunk must be applied by the reducer; transcript:\n{text}"
        );
        // The chunk's line is tagged Llm(2) — sourced from the FORWARDED turn,
        // not current_turn() inference. Resolve anchors first (immutable), then
        // read the metadata view.
        let anchors: Vec<_> = (0..c.editor.document().line_count())
            .filter_map(|line| c.editor.anchor_for_line_opt(line))
            .collect();
        let meta = c.editor.metadata::<crate::TurnId>();
        let tagged_turn_2 = anchors
            .iter()
            .any(|a| matches!(meta.get(*a), Some(crate::TurnId::Llm(2))));
        assert!(
            tagged_turn_2,
            "the turn-2 chunk must be tagged Llm(2) from event.turn"
        );
        assert!(c.finalized.contains(&(1, 2)), "turn 2 finalized once");
    });
}

/// §7/H5 idempotent finalize: delivering `TurnEnded` for the SAME
/// `(generation, turn)` twice finalizes once — no double trailing newline,
/// single phase flip. (Models the dual-stream duplicate: a forwarded boundary
/// plus a lingering inference during additive rollout.)
#[gpui::test]
fn agent_reducer_finalize_is_idempotent_on_generation_turn(cx: &mut TestAppContext) {
    use yalda::agent_event::{AgentEventKind as K, ChunkRole, TurnOutcome};

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // Flip the gate + stream a chunk WITHOUT a trailing newline.
    view.update(vcx, |v, cx| {
        let batch = vec![
            agent_note("S1", 1, 1, 0, K::ChannelOpened { resumed: false }),
            agent_note(
                "S1",
                1,
                1,
                1,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
            agent_note(
                "S1",
                1,
                2,
                2,
                K::Chunk {
                    text: "no newline here".into(),
                    role: ChunkRole::Message,
                },
            ),
        ];
        v.apply_server_batch(batch, cx);
    });

    // First TurnEnded for (1,2): finalizes, adds the trailing newline.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                1,
                2,
                3,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            )],
            cx,
        );
    });
    let after_first = active_transcript_text(&view, vcx);

    // SECOND TurnEnded for the SAME (1,2): must be a no-op on the buffer.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                1,
                2,
                4,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            )],
            cx,
        );
    });
    let after_second = active_transcript_text(&view, vcx);

    assert_eq!(
        after_first, after_second,
        "a duplicate TurnEnded for the same (generation, turn) must NOT mutate the buffer"
    );
    assert_eq!(
        after_second.matches("no newline here").count(),
        1,
        "the chunk text appears exactly once"
    );
    // Exactly one trailing newline (the finalize added one; the dup added none).
    assert!(after_second.ends_with('\n'));
    assert!(
        !after_second.ends_with("\n\n"),
        "idempotent finalize must not append a second trailing newline"
    );
}

/// §4 generation rebaseline: a strictly-newer generation wipes the transcript
/// FIRST, then adopts the new generation; a stray OLDER-generation event after
/// the bump is ignored.
#[gpui::test]
fn agent_reducer_rebaselines_on_newer_generation(cx: &mut TestAppContext) {
    use yalda::agent_event::{AgentEventKind as K, ChunkRole, TurnOutcome};

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // Gen 1: authoritative, with content on screen.
    view.update(vcx, |v, cx| {
        let batch = vec![
            agent_note("S1", 1, 1, 0, K::ChannelOpened { resumed: false }),
            agent_note(
                "S1",
                1,
                1,
                1,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
            agent_note(
                "S1",
                1,
                2,
                2,
                K::Chunk {
                    text: "GEN1-CONTENT".into(),
                    role: ChunkRole::Message,
                },
            ),
        ];
        v.apply_server_batch(batch, cx);
    });
    assert!(active_transcript_text(&view, vcx).contains("GEN1-CONTENT"));

    // Gen 2 ChannelOpened: must reset_for_replay (wipe GEN1-CONTENT) and adopt
    // gen 2. The gate resets too (a fresh channel), so the gen-2 chunk before
    // the gen-2 boundary is NOT applied — matching the §9 first-turn rule.
    view.update(vcx, |v, cx| {
        let batch = vec![
            agent_note("S1", 2, 1, 0, K::ChannelOpened { resumed: true }),
            agent_note(
                "S1",
                2,
                1,
                1,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
            agent_note(
                "S1",
                2,
                2,
                2,
                K::Chunk {
                    text: "GEN2-CONTENT".into(),
                    role: ChunkRole::Message,
                },
            ),
            agent_note(
                "S1",
                2,
                2,
                3,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
        ];
        v.apply_server_batch(batch, cx);
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert_eq!(c.generation, 2, "adopted the newer generation");
        let text = c.editor.document().full_text();
        assert!(
            !text.contains("GEN1-CONTENT"),
            "the newer generation must wipe the old transcript; got:\n{text}"
        );
        assert!(text.contains("GEN2-CONTENT"), "gen-2 content rebuilt");
    });

    // A stray OLDER-generation (gen 1) event after the bump is ignored.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                1,
                9,
                99,
                K::Chunk {
                    text: "STRAY-OLD-GEN".into(),
                    role: ChunkRole::Message,
                },
            )],
            cx,
        );
    });
    assert!(
        !active_transcript_text(&view, vcx).contains("STRAY-OLD-GEN"),
        "an event from a superseded (older) generation must be ignored"
    );
}

/// §7/§8 explicit Unknown + CompactedSummary arms: Unknown renders nothing (but
/// is not an error), CompactedSummary inserts a deterministic placeholder.
#[gpui::test]
fn agent_reducer_unknown_and_compacted_summary_arms(cx: &mut TestAppContext) {
    use yalda::agent_event::{AgentEventKind as K, TurnOutcome};

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // Flip the gate so the reducer drives, then feed CompactedSummary + Unknown.
    view.update(vcx, |v, cx| {
        let unknown = K::Unknown {
            tag: "speculative_decode".into(),
            raw: serde_json::json!({"kind":"speculative_decode","tokens":128}),
        };
        let batch = vec![
            agent_note("S1", 1, 1, 0, K::ChannelOpened { resumed: false }),
            agent_note(
                "S1",
                1,
                1,
                1,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
            agent_note(
                "S1",
                1,
                2,
                2,
                K::CompactedSummary {
                    through_turn: 5,
                    summary: "earlier work".into(),
                },
            ),
            agent_note("S1", 1, 2, 3, unknown),
        ];
        v.apply_server_batch(batch, cx);
    });

    let text = active_transcript_text(&view, vcx);
    assert!(
        text.contains("history compacted through turn 5"),
        "CompactedSummary must surface a deterministic placeholder; got:\n{text}"
    );
    assert!(
        text.contains("earlier work"),
        "the summary text is included"
    );
    assert!(
        !text.contains("speculative_decode"),
        "an Unknown event must render nothing (no broken block)"
    );
}

/// §9 no-double-apply guard: with BOTH the legacy `ReplyEvent` stream and the
/// canonical `Agent` stream carrying the same turn-2 chunk, the gate routes the
/// chunk through exactly ONE driver — it must appear EXACTLY ONCE.
#[gpui::test]
fn agent_reducer_no_double_apply_across_streams(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::agent_event::{AgentEventKind as K, ChunkRole, TurnOutcome};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // Turn 1 boundary flips the gate -> the Agent stream becomes authoritative.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                agent_note("S1", 1, 1, 0, K::ChannelOpened { resumed: false }),
                agent_note(
                    "S1",
                    1,
                    1,
                    1,
                    K::TurnEnded {
                        outcome: TurnOutcome::Completed,
                    },
                ),
            ],
            cx,
        );
    });

    // Turn 2: the SAME chunk arrives on BOTH streams in one batch. The legacy
    // ReplyEvent arm is now inert (gate is set), the Agent arm applies it once.
    view.update(vcx, |v, cx| {
        let batch = vec![
            ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::Chunk("DEDUP-ME".into()),
            },
            agent_note(
                "S1",
                1,
                2,
                2,
                K::Chunk {
                    text: "DEDUP-ME".into(),
                    role: ChunkRole::Message,
                },
            ),
            agent_note(
                "S1",
                1,
                2,
                3,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            ),
        ];
        v.apply_server_batch(batch, cx);
    });

    let text = active_transcript_text(&view, vcx);
    assert_eq!(
        text.matches("DEDUP-ME").count(),
        1,
        "the chunk must apply from exactly one stream; transcript:\n{text}"
    );
}

/// Bug 3: the forwarded `Agent { TurnEnded }` boundary and the legacy
/// `ServerNotification::TurnEnded` for the SAME (generation, turn) must collapse
/// to ONE ledger entry, so finalize fires EXACTLY ONCE.
///
/// The bug: the legacy arm keyed the idempotent ledger with `turn_count` (1-based
/// SETTLED count), while the Agent reducer arm keys with the envelope's 0-based
/// `turn` (the server sets `completed_turn = turns - 1`). So the two streams
/// inserted DISTINCT keys — `(gen, turns-1)` and `(gen, turns)` — and BOTH
/// finalized, defeating the §7/§9 exactly-once backstop. It was benign only
/// because `finalize_agent_turn` happens to be buffer-idempotent; the ledger
/// itself is the ground truth, so this test asserts on the ledger directly.
///
/// FAILS before the fix (ledger holds both `(1,1)` and `(1,2)` → two finalizes);
/// PASSES after (the legacy arm converts `turn_count - 1`, so both collapse to
/// `(1,1)` and the second finalize is a no-op).
#[gpui::test]
fn agent_reducer_legacy_and_forwarded_turn_ended_collapse(cx: &mut TestAppContext) {
    use yalda::agent_event::{AgentEventKind as K, TurnOutcome};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // Rebaseline to generation 1 so both streams share the same generation key.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                1,
                0,
                0,
                K::ChannelOpened { resumed: false },
            )],
            cx,
        );
    });

    // The FORWARDED boundary for turn 1: envelope `turn` is 0-based, so the
    // server emits `turn = 1` for the SECOND completed turn (settled count 2).
    // It finalizes the ledger key (generation 1, turn 1).
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                1,
                1,
                1,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            )],
            cx,
        );
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert!(
            c.finalized.contains(&(1, 1)),
            "forwarded TurnEnded must finalize the 0-based (generation 1, turn 1) key; \
             ledger={:?}",
            c.finalized
        );
    });

    // The LEGACY boundary for the SAME turn: `turn_count` is the 1-based settled
    // count, so it is 2 for the turn whose 0-based index is 1. With the fix the
    // legacy arm converts `turn_count - 1 == 1`, hitting the SAME ledger key, so
    // the second finalize is a no-op. With the BUG it would key `(1, 2)` — a
    // DISTINCT entry — and finalize a second time.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::TurnEnded {
                session_id: "S1".into(),
                turn_count: 2,
                generation: 1,
            }],
            cx,
        );
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        // The forwarded and legacy boundaries collapsed to a SINGLE ledger entry.
        assert!(
            c.finalized.contains(&(1, 1)),
            "the shared 0-based key must be present; ledger={:?}",
            c.finalized
        );
        assert!(
            !c.finalized.contains(&(1, 2)),
            "BUG 3: the legacy boundary must NOT key a DISTINCT 1-based entry — that \
             would finalize a second time for the same turn; ledger={:?}",
            c.finalized
        );
        // Exactly one ledger entry for this (generation, turn) boundary: the two
        // streams deduped to one finalize.
        let entries_for_turn = c.finalized.iter().filter(|(g, _)| *g == 1).count();
        assert_eq!(
            entries_for_turn, 1,
            "the forwarded + legacy TurnEnded for one (generation, turn) must collapse to \
             ONE ledger entry (exactly-once finalize); ledger={:?}",
            c.finalized
        );
    });
}

/// LIVE-TURN-AFTER-REPLAY (the phase-8 runtime "stuck thinking" bug),
/// SUBMIT-AFTER-REPLAY ordering — the user types only once replay has fully
/// completed and the phase has already returned to Idle.
///
/// Reproduces the real symptom at the fold level: resume a server-managed
/// session (a daemon restart / re-attach replays the full `event_log` at
/// generation 0), then send a fresh LIVE prompt. The replayed stream ends with
/// `TurnOutcome::ReplayEnd`, which flips `agent_stream_authoritative = true`
/// (the §9 gate) so the legacy `ReplyEvent` path goes inert and the live turn is
/// supposed to drive entirely through the `AgentEvent` reducer.
///
/// The flow modelled (all at generation 0 — a re-attach does NOT respawn, and a
/// disk-recovered log is at gen 0):
///   replay: legacy ReplyEvent chunk for turn 0  (gate still closed)
///           forwarded Agent TurnEnded{Completed, turn 0}  (flips the gate)
///           forwarded Agent TurnEnded{ReplayEnd}  (no live turn in flight → Idle)
///   live:   forwarded Agent Chunk{Message, turn 1}
///           forwarded Agent TurnEnded{Completed, turn 1}
///
/// The key the bug turns on: the server stamps the `ReplayEnd` envelope `turn`
/// with the current settled count, and `finish_replay` folds the replay cursor
/// into `last_seen`, so `last_seen` ends up EQUAL to the upcoming live turn's
/// `completed_turn` index (1). The buggy `ReplayEnded` arm keyed the per-turn
/// ledger on `last_seen`, pre-occupying `(0, 1)` — the LIVE turn's key. To
/// faithfully reproduce that aliasing the replay cursor must be driven up to the
/// live index BEFORE `ReplayEnd` (the legacy replay path leaves it there); a
/// `replay_turn` left at 0 folds `last_seen` to 0 and never collides, which is
/// why an under-driven version of this test passed even against the bug.
///
/// Asserts the live turn (a) folds its message content into the transcript and
/// (b) FINALIZES — `turn_phase` returns to `Idle` (not stuck "thinking") and the
/// live `(generation, turn)` lands in the finalize ledger exactly once.
#[gpui::test]
fn agent_reducer_live_turn_after_replay_finalizes(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::agent_event::{AgentEventKind as K, ChunkRole, TurnOutcome};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // ── REPLAY burst (generation 0) ────────────────────────────────────────
    // Replayed turn 0: the legacy ReplyEvent stream drives the chunk (gate is
    // still closed), and the forwarded Agent TurnEnded{Completed, turn 0} marks
    // the boundary — that first forwarded boundary FLIPS the §9 gate.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ServerNotification::ReplyEvent {
                    session_id: "S1".into(),
                    event: ReplyEvent::Chunk("replayed turn-0 prose".into()),
                },
                agent_note(
                    "S1",
                    0,
                    0,
                    0,
                    K::TurnEnded {
                        outcome: TurnOutcome::Completed,
                    },
                ),
            ],
            cx,
        );
    });

    // Drive the replay cursor up to the live turn's index, the way the legacy
    // replay path leaves it just before `ReplayEnd` (a replayed user boundary
    // seeds replay_turn = last_seen + 1 = 1). Without this `finish_replay` folds
    // `last_seen` to 0 and the bug's aliasing never occurs.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().replay_turns.replay_turn = 1;
    });

    // End of the replayed prefix. `finish_replay` now folds `last_seen` → 1,
    // which aliases the upcoming live turn's `completed_turn` (1).
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                0,
                1,
                1,
                K::TurnEnded {
                    outcome: TurnOutcome::ReplayEnd,
                },
            )],
            cx,
        );
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert!(
            c.agent_stream_authoritative,
            "the forwarded TurnEnded must flip the §9 gate after replay"
        );
        assert!(
            matches!(c.turn_phase, crate::TurnPhase::Idle),
            "after replay (no live turn in flight) the phase is Idle, not thinking"
        );
        assert_eq!(
            c.replay_turns.last_seen, 1,
            "finish_replay folded the cursor to the live turn's index (the aliasing \
             precondition the bug needs)"
        );
        assert!(
            !c.finalized.contains(&(0, 1)),
            "ReplayEnd must NOT pre-occupy the live turn's (0, 1) key; ledger={:?}",
            c.finalized
        );
    });

    // ── LIVE turn after replay ─────────────────────────────────────────────
    // The user submits a fresh prompt: the turn begins (thinking indicator on),
    // then the live content + boundary stream through the now-authoritative
    // reducer. envelope turn == 1 (settled count 2 -> completed_turn 1).
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        c.turn_phase = crate::TurnPhase::begin(std::time::Instant::now());
    });

    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                agent_note(
                    "S1",
                    0,
                    1,
                    2,
                    K::Chunk {
                        text: "LIVE-AFTER-REPLAY".into(),
                        role: ChunkRole::Message,
                    },
                ),
                agent_note(
                    "S1",
                    0,
                    1,
                    3,
                    K::TurnEnded {
                        outcome: TurnOutcome::Completed,
                    },
                ),
            ],
            cx,
        );
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        let text = c.editor.document().full_text();
        assert!(
            text.contains("LIVE-AFTER-REPLAY"),
            "the live turn's message content must fold into the transcript; \
             transcript:\n{text}"
        );
        assert!(
            matches!(c.turn_phase, crate::TurnPhase::Idle),
            "BUG: the live turn must FINALIZE — turn_phase back to Idle, not stuck \
             'thinking'; phase={:?}",
            c.turn_phase
        );
        assert!(
            c.finalized.contains(&(0, 1)),
            "the live (generation 0, turn 1) boundary must finalize exactly once; \
             ledger={:?}",
            c.finalized
        );
    });
}

/// LIVE-TURN-AFTER-REPLAY across a MULTI-TURN replayed prefix (two replayed
/// turns, then a live turn). Confirms the aliasing holds at a higher turn index
/// and that the replayed turn boundaries' OWN ledger keys stay distinct from
/// both the ReplayEnd settle and the live turn's finalize.
///
/// Replay turns 0 and 1 each finalize their own boundary keys `(0, 0)`/`(0, 1)`.
/// `finish_replay` folds the cursor to 2 — which aliases the upcoming live
/// turn's `completed_turn` (settled count 3 → 2). The buggy `ReplayEnded` arm
/// keyed the per-turn ledger on that folded `last_seen` (2), pre-occupying the
/// LIVE turn's `(0, 2)` key so its `TurnEnded` no-op'd and `turn_phase` never
/// returned to Idle → STUCK THINKING.
///
/// As with the single-turn case, the replay cursor must be driven up to the live
/// index before `ReplayEnd` — an under-driven cursor folds `last_seen` low and
/// never collides, so the test would pass even against the bug.
#[gpui::test]
fn agent_reducer_live_turn_after_multi_turn_replay_finalizes(cx: &mut TestAppContext) {
    use yalda::agent_event::{AgentEventKind as K, ChunkRole, TurnOutcome};

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // Replay two completed turns (0 and 1). Each is applied via the boundary
    // observe-path and finalizes its OWN key; the first flips the gate.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                agent_note(
                    "S1",
                    0,
                    0,
                    0,
                    K::TurnEnded {
                        outcome: TurnOutcome::Completed,
                    },
                ),
                agent_note(
                    "S1",
                    0,
                    1,
                    1,
                    K::TurnEnded {
                        outcome: TurnOutcome::Completed,
                    },
                ),
            ],
            cx,
        );
    });

    // Drive the replay cursor to the upcoming live turn's index (2), the way the
    // legacy replay path leaves it after two replayed user boundaries.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().replay_turns.replay_turn = 2;
    });

    // ReplayEnd: `finish_replay` folds `last_seen` → 2, aliasing the live turn.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                0,
                2,
                2,
                K::TurnEnded {
                    outcome: TurnOutcome::ReplayEnd,
                },
            )],
            cx,
        );
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert!(c.agent_stream_authoritative, "gate flipped after replay");
        assert!(
            c.finalized.contains(&(0, 0)),
            "replayed turn 0 finalized its own key"
        );
        assert!(
            c.finalized.contains(&(0, 1)),
            "replayed turn 1 finalized its own key"
        );
        assert_eq!(
            c.replay_turns.last_seen, 2,
            "finish_replay folded the cursor to the live turn's index"
        );
        assert!(
            !c.finalized.contains(&(0, 2)),
            "ReplayEnd must NOT pre-occupy the live turn's (0, 2) key; ledger={:?}",
            c.finalized
        );
    });

    // Live turn 2: begin (thinking), stream content, end. Stamped turn 2.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().turn_phase = crate::TurnPhase::begin(std::time::Instant::now());
    });
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                agent_note(
                    "S1",
                    0,
                    2,
                    3,
                    K::Chunk {
                        text: "LIVE-PROSE".into(),
                        role: ChunkRole::Message,
                    },
                ),
                agent_note(
                    "S1",
                    0,
                    2,
                    4,
                    K::TurnEnded {
                        outcome: TurnOutcome::Completed,
                    },
                ),
            ],
            cx,
        );
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        let text = c.editor.document().full_text();
        assert!(
            text.contains("LIVE-PROSE"),
            "live content folded; transcript:\n{text}"
        );
        assert!(
            matches!(c.turn_phase, crate::TurnPhase::Idle),
            "live turn after multi-turn replay must finalize (Idle), not stay thinking; \
             phase={:?}",
            c.turn_phase
        );
        assert!(
            c.finalized.contains(&(0, 2)),
            "live turn 2 finalized its own ledger key; ledger={:?}",
            c.finalized
        );
    });
}

/// LIVE-TURN-AFTER-REPLAY — the STUCK-THINKING root cause (suspect b, ReplayEnd
/// finalize pre-occupies the live turn's ledger key).
///
/// Models the real ordering where the user submits the live prompt while replay
/// is still finishing (or the `ReplayEnd` marker simply trails the live submit):
///
///   1. replayed turn 0 boundary  -> finalizes (0, 0), last_seen = 0
///   2. USER SUBMITS  -> turn_phase = Awaiting (thinking); next live turn is 1
///   3. ReplayEnd (envelope turn 1)  -> `settle` finalizes (gen, replay_turns
///      .last_seen). After `finish_replay`, last_seen has been folded to the
///      replay cursor, which is the SAME index the live turn will carry.
///   4. live Chunk (turn 1)  -> folds content
///   5. live TurnEnded{Completed, turn 1}  -> `settle` finalizes (gen, 1)
///
/// BUG: step 3 finalizes the live turn's key (gen, 1) ahead of step 5, so step 5
/// is an idempotent no-op and never flips `turn_phase` back to Idle — the live
/// turn renders its content but the "thinking" indicator never clears.
///
/// FAILS before the fix (turn_phase stuck Awaiting); PASSES after (ReplayEnd
/// finalizes its OWN envelope turn, leaving the live turn's key free).
#[gpui::test]
fn agent_reducer_replay_end_does_not_steal_live_turn_finalize(cx: &mut TestAppContext) {
    use yalda::agent_event::{AgentEventKind as K, ChunkRole, TurnOutcome};

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    // Step 1: replayed turn 0 completes. Note last_seen advances to 0.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                0,
                0,
                0,
                K::TurnEnded {
                    outcome: TurnOutcome::Completed,
                },
            )],
            cx,
        );
    });

    // Step 2: the user submits a live prompt mid-resume → turn begins (thinking).
    // Drive `replay_turns` so the replay cursor lands on the live turn index, the
    // way the legacy replay path leaves it just before `ReplayEnd` (a replayed
    // user boundary seeds replay_turn = last_seen + 1 = 1).
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        c.replay_turns.replay_turn = 1; // pending replayed boundary for turn 1
        c.turn_phase = crate::TurnPhase::begin(std::time::Instant::now());
    });

    // Step 3: ReplayEnd arrives (envelope turn 1). A live turn is in flight, so it
    // must NOT flip to Idle — and it must NOT finalize the live turn's key.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                0,
                1,
                1,
                K::TurnEnded {
                    outcome: TurnOutcome::ReplayEnd,
                },
            )],
            cx,
        );
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert!(
            c.turn_phase.is_awaiting(),
            "ReplayEnd with a live turn in flight must keep the phase Awaiting"
        );
        assert!(
            !c.finalized.contains(&(0, 1)),
            "BUG: ReplayEnd must NOT finalize the live turn's (0, 1) key ahead of the \
             live TurnEnded — that pre-occupies the ledger and wedges the live finalize; \
             ledger={:?}",
            c.finalized
        );
    });

    // Step 4 + 5: the live turn streams its content and ends (envelope turn 1).
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                agent_note(
                    "S1",
                    0,
                    1,
                    2,
                    K::Chunk {
                        text: "LIVE-MID-RESUME".into(),
                        role: ChunkRole::Message,
                    },
                ),
                agent_note(
                    "S1",
                    0,
                    1,
                    3,
                    K::TurnEnded {
                        outcome: TurnOutcome::Completed,
                    },
                ),
            ],
            cx,
        );
    });

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        let text = c.editor.document().full_text();
        assert!(
            text.contains("LIVE-MID-RESUME"),
            "live content folded; transcript:\n{text}"
        );
        assert!(
            matches!(c.turn_phase, crate::TurnPhase::Idle),
            "STUCK-THINKING BUG: the live turn after replay must finalize (Idle), not stay \
             Awaiting forever; phase={:?}",
            c.turn_phase
        );
    });
}

// ---- Session picker (in-tile, empty-ring) --------------------------------
//
// A fresh Agent app opens into a picker: an empty `AgentRing` whose `picker`
// is Some. The render path must handle the empty ring (no `active()` panic),
// navigation must wrap, and activating a row must bind the ring's first slot
// and clear the picker.

/// Install an unbound agent tile in selector mode on `view`, seeding the
/// universal roster (universal-agent-list) with `sessions` (cwd ".") so the
/// selector — which now PROJECTS from the roster — has rows to show.
#[cfg(test)]
fn install_agent_picker(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    sessions: &[(&str, &str)],
) {
    use crate::{AgentTile, App, SessionPicker};
    use yalda::session_proto::SessionInfo;
    view.update(vcx, |v, cx| {
        for (sid, label) in sessions {
            v.agent_roster.upsert(SessionInfo {
                session_id: sid.to_string(),
                acp_session_id: None,
                label: label.to_string(),
                cwd: PathBuf::from("."),
                turns: 3,
                connected: true,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            });
        }
        let mut tile = AgentTile::new();
        tile.picker = Some(SessionPicker::new());
        v.set_screen(App::Agent(tile));
    });
}

/// The empty-ring selector renders headlessly without panicking on
/// `ring.active()`, and projects the FREE sessions from the universal roster
/// (universal-agent-list) for its cwd.
#[gpui::test]
fn session_picker_renders_empty_ring(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();

    // Empty roster → only the "start new" row; renders without panic.
    install_agent_picker(&view, &mut *vcx, &[]);
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        let tile = v.agent_tile().expect("agent tile");
        assert!(tile.bound.is_none(), "tile stays unbound until a row binds");
        // The picker has no cached cwd — it projects from the active workspace's
        // live cwd (`agent_base_cwd`).
        let cwd = v.agent_base_cwd();
        assert!(v.picker_projection(&cwd).0.is_empty(), "no free rows yet");
    });

    // Seed two roster sessions for this cwd → the selector projects two FREE
    // rows (plus the "start new" row = 3 total). No async reducer, no per-tile
    // cache — a pure view of the shared roster.
    install_agent_picker(&view, &mut *vcx, &[("S1", "claude-1"), ("S2", "claude-2")]);
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        let (free, bound) = v.picker_projection(&v.agent_base_cwd());
        assert_eq!(free.len(), 2, "two FREE sessions projected from the roster");
        assert!(bound.is_empty(), "none bound to a tile yet");
    });
}

/// The selector is a live projection of the shared roster (universal-agent-list,
/// ADR-0022): selecting/binding a session in ONE tile immediately moves it from
/// FREE → bound in ANOTHER tile's selector — no per-tile cache to go stale, and
/// no per-tile async result to misroute (the property ADR-0020's INV-PR routing
/// previously protected is now designed out).
#[gpui::test]
fn selector_projection_reflects_binding_across_tiles(cx: &mut TestAppContext) {
    use crate::{AgentTile, App, SessionPicker};
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    // Two server sessions in the roster (cwd ".").
    view.update(vcx, |v, cx| {
        for (sid, label) in [("S1", "claude-1"), ("S2", "claude-2")] {
            v.agent_roster.upsert(SessionInfo {
                session_id: sid.into(),
                acp_session_id: None,
                label: label.into(),
                cwd: PathBuf::from("."),
                turns: 0,
                connected: true,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            });
        }
    });

    // An unbound selector tile. Both sessions are FREE.
    view.update(vcx, |v, cx| {
        let mut t = AgentTile::new();
        t.picker = Some(SessionPicker::new());
        v.set_screen(App::Agent(t));
    });
    let cwd = view.read_with(vcx, |v, _| v.agent_base_cwd());
    view.read_with(vcx, |v, _| {
        let (free, bound) = v.picker_projection(&cwd);
        assert_eq!(free.len(), 2, "both sessions free before any binding");
        assert!(bound.is_empty());
    });

    // Bind S1 to a (different) tile — the canonical "a session was selected".
    install_agent_slot(&view, &mut *vcx, Some("S1"));
    vcx.run_until_parked();

    // The selector projection now shows S1 as bound (in use), S2 still free —
    // WITHOUT any explicit refresh of the selector itself.
    view.read_with(vcx, |v, _| {
        let (free, bound) = v.picker_projection(&cwd);
        assert_eq!(free.len(), 1, "S1 left the free list once it was bound");
        assert_eq!(free[0].sid, "S2");
        assert_eq!(bound.len(), 1, "S1 now shows in the IN USE column");
        assert_eq!(bound[0].sid, "S1");
    });
}

/// The workspace cwd (`Set CWD`) is PERSISTED: it survives a save→restore
/// (process restart). Hermetic — the workspace file is redirected to a tempdir
/// (no touch to `~/.yalda`). Save writes `PersistedTab.cwd`; restore reads it
/// back into the typed `Tab.cwd` (ADR-0023).
#[gpui::test]
fn workspace_cwd_persists_across_restart(cx: &mut TestAppContext) {
    use crate::persist::with_workspace_path;
    use crate::workspace::WorkspaceCwd;
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("workspace.json");
    let set = std::env::temp_dir().join("yalda-persisted-cwd");

    // Session 1: Set CWD on the active workspace, then save to disk.
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();
    with_workspace_path(file.clone(), || {
        view.update(vcx, |v, _cx| {
            v.workspace
                .active_tab_mut()
                .expect("active tab")
                .set_cwd(WorkspaceCwd::new(set.clone()));
            v.save_workspace_state();
        });
    });

    // Session 2 ("restart"): a fresh view restores from the same file — the cwd
    // we set is back, so a new agent would again inherit it.
    let (view2, vcx2) = cx.add_window_view(hermetic_browser_view);
    vcx2.run_until_parked();
    let restored = with_workspace_path(file.clone(), || {
        view2.update(vcx2, |v, cx| {
            assert!(v.restore_workspace_from_disk(cx), "a snapshot was restored");
            v.active_workspace_cwd()
        })
    });
    assert_eq!(
        restored,
        Some(set),
        "the workspace cwd set via Set CWD survives a save→restore"
    );
}

/// A new agent inherits the workspace's LIVE cwd at create time — including a
/// `Set CWD` done AFTER the selector was already open. Regression: the selector
/// cached its cwd when it opened, so "open agent → Set CWD → Start a new
/// session" created the agent in the OLD dir. The picker no longer caches a cwd;
/// `Start a new session` reads `agent_base_cwd` live.
#[gpui::test]
fn new_agent_uses_live_workspace_cwd_after_set_cwd(cx: &mut TestAppContext) {
    use crate::workspace::WorkspaceCwd;
    use crate::{AgentTile, App, SessionPicker};
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    // An agent tile sitting in its selector (picker open) BEFORE any Set CWD.
    view.update(vcx, |v, _cx| {
        let mut t = AgentTile::new();
        t.picker = Some(SessionPicker::new());
        v.set_screen(App::Agent(t));
    });

    // Now Set CWD on the active workspace — AFTER the selector is already open.
    let target = PathBuf::from("/tmp/yalda-live-cwd-test");
    view.update(vcx, |v, _cx| {
        v.workspace
            .active_tab_mut()
            .expect("active tab")
            .set_cwd(WorkspaceCwd::new(target.clone()));
    });

    // Activate row 0 ("Start a new session"). Hermetic: the create round-trip is
    // a no-op, but the placeholder session is bound synchronously with its cwd.
    view.update(vcx, |v, cx| v.agent_picker_activate(0, cx));
    vcx.run_until_parked();

    let session_cwd = view.read_with(vcx, |v, cx| {
        let id = v.agent_tile().expect("agent tile").bound.expect("a session bound");
        v.sessions.get(id).expect("session").read(cx).cwd.clone()
    });
    assert_eq!(
        session_cwd, target,
        "a new agent must use the cwd set on the workspace, not the one cached \
         when the selector opened"
    );
}

/// INV-PR regression (the close path the adversarial review flagged): when a
/// session closes from a background/async trigger, the replacement selector's
/// list is addressed by `agent_tile_id_bound_to(sid)` — the BOUND tile's id,
/// resolved INDEPENDENTLY of focus. The pre-fix close path listed against
/// `focused_window_id()`, so a bound-but-unfocused tile hung on "loading…"
/// forever. This drives the close path AND asserts the focus-independent query
/// directly: a revert to focus-based routing fails here, not silently passes.
#[gpui::test]
fn session_close_shows_selector_on_bound_tile_not_focused(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState, AgentTile, App};

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    let (win_a, win_b) = view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        let mk = |label: &str| AgentSession {
            state: AgentState::new_server_managed(None),
            label: label.into(),
            cwd: PathBuf::from("."),
            resume_id: None,
        };
        // Tile A → bound to sid "A". Capture A's WindowId.
        let id_a = v.show_local_session(mk("claude-A"), cx);
        v.sessions.bind_sid(id_a, "A".into()).unwrap();
        let win_a = v.workspace.focused_window_id().expect("focused A");
        // Split → tile B, focus it, bind sid "B". B now holds focus.
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));
        let id_b = v.show_local_session(mk("claude-B"), cx);
        v.sessions.bind_sid(id_b, "B".into()).unwrap();
        let win_b = v.workspace.focused_window_id().expect("focused B");
        (win_a, win_b)
    });
    assert_ne!(win_a, win_b);

    // The capture query (what the close path feeds to `spawn_list_sessions…`)
    // must resolve to A by BINDING, even though B holds focus. This is the
    // exact value a revert to `focused_window_id()` would get wrong.
    view.read_with(vcx, |v, cx| {
        let sid_a = v.sessions.locate("A").expect("sid A in store");
        assert_eq!(
            v.agent_tile_id_bound_to(sid_a),
            Some(win_a),
            "the close path must target the BOUND tile A, not the focused tile B"
        );
    });

    // Close sid A while B is focused (mirrors a server SessionClosed broadcast).
    view.update(vcx, |v, cx| {
        v.reconcile_session_closed("A", cx);
    });
    vcx.run_until_parked();

    let tile_state = |v: &YaldaGpuiView, id: crate::workspace::WindowId| {
        for tab in v.workspace.tabs.iter() {
            if let Some(w) = tab.layout.find_leaf(id)
                && let App::Agent(t) = &w.content
            {
                return Some((t.bound.is_some(), t.picker.is_some()));
            }
        }
        None
    };
    view.read_with(vcx, |v, cx| {
        assert_eq!(
            tile_state(v, win_a),
            Some((false, true)),
            "A (the bound, UNFOCUSED tile) must be unbound and showing a loading picker"
        );
        assert_eq!(
            tile_state(v, win_b),
            Some((true, false)),
            "B (focused) must be untouched — still bound, no picker hijacked onto it"
        );
    });
}

/// `next_agent_label` never hands out a name already taken by a session in the
/// store, and reuses a freed number in the gap. This is what keeps two freshly
/// created sessions from both being "claude-1".
#[gpui::test]
fn next_agent_label_is_unique_and_fills_gaps(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState};
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();

    let add = |view: &gpui::Entity<YaldaGpuiView>,
               vcx: &mut gpui::VisualTestContext,
               label: &str| {
        let label = label.to_string();
        view.update(vcx, |v, cx| {
            v.show_local_session(
                AgentSession {
                    state: AgentState::new_server_managed(None),
                    label,
                    cwd: PathBuf::from("."),
                    resume_id: None,
                },
                cx,
            );
        });
    };

    // Empty store → claude-1.
    view.read_with(vcx, |v, cx| {
        assert_eq!(v.next_agent_label(cx), "claude-1");
    });
    // With claude-1 present → claude-2 (NOT another claude-1).
    add(&view, &mut *vcx, "claude-1");
    view.read_with(vcx, |v, cx| {
        assert_eq!(v.next_agent_label(cx), "claude-2");
    });
    // Leave a gap (claude-1, claude-3) → the next label fills the hole.
    add(&view, &mut *vcx, "claude-3");
    view.read_with(vcx, |v, cx| {
        assert_eq!(v.next_agent_label(cx), "claude-2");
    });
}

/// j/k navigation wraps at both ends across the picker's rows.
#[gpui::test]
fn session_picker_navigation_wraps(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_picker(&view, &mut *vcx, &[("S1", "claude-1"), ("S2", "claude-2")]);
    vcx.run_until_parked();

    let selected = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.read_with(vcx, |v, cx| {
            v.agent_tile().unwrap().picker.as_ref().unwrap().selected
        })
    };

    // 3 rows (new + S1 + S2). Down from 0 → 1 → 2 → wraps to 0.
    view.update(vcx, |v, cx| v.agent_picker_move(1, cx));
    view.update(vcx, |v, cx| v.agent_picker_move(1, cx));
    assert_eq!(selected(&view, &mut *vcx), 2);
    view.update(vcx, |v, cx| v.agent_picker_move(1, cx));
    assert_eq!(
        selected(&view, &mut *vcx),
        0,
        "down past the end wraps to top"
    );
    // Up from 0 wraps to the last row.
    view.update(vcx, |v, cx| v.agent_picker_move(-1, cx));
    assert_eq!(
        selected(&view, &mut *vcx),
        2,
        "up past the top wraps to bottom"
    );
}

/// Activating a listed row binds the tile to the chosen session and clears the
/// picker; the bound session SURVIVES the attach round-trip (hermetic — no
/// server, so the attach early-returns rather than dropping the session).
#[gpui::test]
fn session_picker_activation_binds_slot(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();
    install_agent_picker(&view, &mut *vcx, &[("S1", "claude-1"), ("S2", "claude-2")]);
    vcx.run_until_parked();

    // Activate row 2 (the second listed session, sid "S2").
    view.update(vcx, |v, cx| v.agent_picker_activate(2, cx));
    // Park the executor: with the server off, the attach round-trip is a no-op,
    // so the bind must SURVIVE (regression guard for the orphaned-tile bug).
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        let tile = v.agent_tile().expect("agent tile");
        assert!(
            tile.bound.is_some(),
            "a session is bound after activation and survives the attach"
        );
        assert!(tile.picker.is_none(), "picker cleared once a session binds");
        assert_eq!(v.sessions.len(), 1, "exactly one session in the store");
        let id = tile.bound.unwrap();
        assert_eq!(
            v.sessions.sid_of(id),
            Some("S2"),
            "the bound session carries the chosen session id"
        );
    });
}

// ---- Ownership invariants (hermetic — server forced off) -----------------

/// INV-2 (no mirror): two tiles bound to two DISTINCT sids; a server batch for
/// sid X mutates EXACTLY one tile's transcript. The deleted fan-out used to
/// mirror X into every tile holding it; with strict 1:1 + store routing that
/// is structurally impossible.
#[gpui::test]
fn two_sessions_route_to_exactly_one_tile(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState, AgentTile, App};
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    // Two agent tiles (a split), each bound to its own sid.
    let (id_a, id_b) = view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        let mk = |label: &str| AgentSession {
            state: AgentState::new_server_managed(None),
            label: label.into(),
            cwd: PathBuf::from("."),
            resume_id: None,
        };
        // First tile → sid A.
        let id_a = v.show_local_session(mk("claude-A"), cx);
        v.sessions.bind_sid(id_a, "A".into()).unwrap();
        // Split off a second agent tile, focus it, bind → sid B.
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));
        let id_b = v.show_local_session(mk("claude-B"), cx);
        v.sessions.bind_sid(id_b, "B".into()).unwrap();
        (id_a, id_b)
    });
    assert_ne!(id_a, id_b);

    // A server batch for sid A only.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::ReplyEvent {
                session_id: "A".into(),
                event: ReplyEvent::Chunk("only-in-A".into()),
            }],
            cx,
        );
    });

    view.read_with(vcx, |v, cx| {
        let text_a = v
            .sessions
            .get(id_a)
            .unwrap()
            .read(cx)
            .state
            .editor
            .document()
            .full_text();
        let text_b = v
            .sessions
            .get(id_b)
            .unwrap()
            .read(cx)
            .state
            .editor
            .document()
            .full_text();
        assert!(
            text_a.contains("only-in-A"),
            "sid A's tile received the chunk"
        );
        assert!(
            !text_b.contains("only-in-A"),
            "sid B's tile must NOT mirror sid A's output (INV-2 no fan-out)"
        );
    });
}

/// The AlreadyBound dedup path (#1): a duplicate resolution for an already-owned
/// sid CLOSES the orphan placeholder and FOCUSES the existing owner, rather than
/// leaving an orphan stuck on "attaching…".
#[gpui::test]
fn duplicate_resolution_closes_orphan_and_focuses_owner(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState, AgentTile, App, OpenResolution};

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    // Owner: a tile already bound to sid "S".
    let owner_id = view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        let id = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(None),
                label: "owner".into(),
                cwd: PathBuf::from("."),
                resume_id: None,
            },
            cx,
        );
        v.sessions.bind_sid(id, "S".into()).unwrap();
        id
    });

    // A second tile mints a placeholder mid-open, then a duplicate Created("S")
    // resolution lands on it — `apply_open_agent_resolution` must close the
    // orphan and point this tile at the owner.
    view.update(vcx, |v, cx| {
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));
        let token = crate::alloc_open_token();
        let orphan = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(Some("attaching…".into())),
                label: "orphan".into(),
                cwd: PathBuf::from("."),
                resume_id: None,
            },
            cx,
        );
        if let Some(tile) = v.agent_tile_mut() {
            tile.pending_open_token = Some(token);
        }
        let before = v.sessions.len();
        assert_eq!(before, 2, "owner + orphan placeholder");
        v.apply_open_agent_resolution(
            token,
            OpenResolution::Created {
                sid: "S".into(),
                acp_id: None,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            },
            cx,
        );
        let _ = orphan;
    });

    view.read_with(vcx, |v, cx| {
        assert_eq!(
            v.sessions.len(),
            1,
            "orphan placeholder dropped — only the owner remains"
        );
        let tile = v.agent_tile().expect("focused agent tile");
        assert_eq!(
            tile.bound,
            Some(owner_id),
            "the focused tile now shows the existing owner (focus-on-conflict)"
        );
        assert!(tile.picker.is_none());
    });
}

/// THE `/clear` async completion, real view method: after the server round-trip
/// binds the new session (`apply_open_agent_resolution` → `Created` → `Bound`), the
/// worksheet must end TYPEABLE (inline You-block active, focus=Compose) — this is the
/// path the pure /clear can't reach in the harness (gap #2), and where the "can't see
/// what I type after clear" bug lived: the async bind left the session resting in NAV.
/// Drives the REAL method with a synthetic `Created`; RED without the settle at the
/// bind, GREEN with it.
#[gpui::test]
fn clear_async_bind_leaves_worksheet_typeable(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // Post-/clear placeholder as the async path could leave it: fresh worksheet, NOT
    // typeable (resting nav, block closed), a pending open token, no sid yet.
    let token = view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.editor =
                yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.close_you_block();
            c.focus = crate::AgentFocus::Transcript;
        });
        let token = crate::alloc_open_token();
        if let Some(tile) = v.agent_tile_mut() {
            tile.pending_open_token = Some(token);
        }
        token
    });
    // The REAL server-resolution handler binds the new sid.
    view.update(vcx, |v, cx| {
        v.apply_open_agent_resolution(
            token,
            crate::OpenResolution::Created {
                sid: "S-cleared".into(),
                acp_id: None,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            },
            cx,
        );
    });
    view.read_with(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("still bound after resolution");
        let (active, focus) = v
            .read_session(id, cx, |c| (c.inline_you_block_active(), c.focus))
            .unwrap();
        assert!(
            active,
            "after the async /clear bind the worksheet must be typeable (inline block active) \
             — else keystrokes fall into nav and nothing repaints"
        );
        assert_eq!(focus, crate::AgentFocus::Compose, "focused so typing lands + repaints");
    });
}

/// Multi-session save→restore: persisting N bound sessions and loading them back
/// yields N slots, each carrying its OWN sid/label — the mapping
/// `restore_agent_leaves` zips one slot per leaf. Hermetic: the persistence file
/// is redirected to a tempdir (no touch to `~/.yalda`).
#[test]
fn multi_session_persistence_round_trips_distinct_sids() {
    use crate::{
        InputModeKind, SessionSnapshot, load_persisted_acp_sessions, save_persisted_acp_sessions,
        with_acp_persist_path,
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("acp_sessions.json");
    let cwd = PathBuf::from("/tmp/proj");

    let snaps = vec![
        SessionSnapshot {
            id: "SID-A".into(),
            label: "claude-A".into(),
            active: true,
            mode: InputModeKind::Chatbox,
            tasklist_open: false,
            subagents_open: false,
            cwd: cwd.clone(),
            compose_draft: None,
        },
        SessionSnapshot {
            id: "SID-B".into(),
            label: "claude-B".into(),
            active: false,
            mode: InputModeKind::Worksheet,
            tasklist_open: true,
            subagents_open: false,
            cwd: cwd.clone(),
            compose_draft: None,
        },
    ];

    let loaded = with_acp_persist_path(file.clone(), || {
        save_persisted_acp_sessions(&cwd, &snaps);
        load_persisted_acp_sessions(&cwd)
    });

    assert_eq!(loaded.len(), 2, "both sessions round-trip");
    // Each slot kept its OWN sid + label (no cross-binding).
    assert_eq!(loaded[0].id, "SID-A");
    assert_eq!(loaded[0].label, "claude-A");
    assert!(loaded[0].active, "first session is the active one");
    assert_eq!(loaded[1].id, "SID-B");
    assert_eq!(loaded[1].label, "claude-B");
    assert_eq!(loaded[1].mode, InputModeKind::Worksheet);
    assert!(loaded[1].tasklist_open);
}

// ---- Render-skip keystone + invalidation model (rev 2) -------------------
//
// The single most important mechanism of the responsiveness refactor: a child
// panel embedded as a *cached* `AnyView` must have its `render()` SKIPPED when
// the parent re-renders but the child was never notified (its inputs
// unchanged), and re-run only when the child entity is itself dirtied — the
// rev-2 way: a `cx.observe(model) -> cx.notify()` on the view, or a notify at
// the model's mutation site. (Rev 1's fingerprint poll from inside `render()`
// is retired — see `project.md` "Design history" and `cached_panel.rs` docs.)
//
// Three tests pin the model:
//   * `cached_panel_skips_render_until_child_is_notified` — the render-skip
//     proof: a parent-only notify does NOT re-render the cached child; a notify
//     on the child entity does. (Adapted from the old fingerprint test.)
//   * `cached_observe_protocol_busts_cache_fresh` — the CANONICAL protocol:
//     mutate a model entity inside `update`, the view's `cx.observe` callback
//     notifies the view, and the next frame re-renders fresh (zero frames late).
//   * `cached_notify_from_render_is_parked` — the TIMING-LAW pin: a `cx.notify`
//     issued from INSIDE a `render()` does NOT invalidate that frame and does
//     NOT schedule a redraw (`project.md` fact 4 / `window.rs:116`). This pins
//     the gpui behavior rev 1 tripped over; a gpui upgrade that changes it
//     fails loudly here.

#[cfg(test)]
thread_local! {
    /// Incremented inside `Probe::render`. The render-skip proofs read this
    /// across notify cycles (mirrors the `VIEW_MODEL_REBUILDS` counter idiom).
    static PROBE_RENDERS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Test-only leaf view: counts its own renders. Its `render()` returns a SIZED
/// div (cached layout is sized from the style, but the content still must be a
/// real element). Also feeds the `cached_panel` perf counter under a label so
/// the instrumentation path is exercised headlessly.
#[cfg(test)]
struct Probe {
    /// Optional model the probe observes; mutating it (via the observe wiring
    /// in `cached_observe_protocol_busts_cache_fresh`) busts the cache.
    _model: Option<gpui::Entity<u64>>,
}

#[cfg(test)]
impl gpui::Render for Probe {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::Styled;
        PROBE_RENDERS.with(|c| c.set(c.get() + 1));
        crate::record_render("test-probe");
        gpui::div().size_full()
    }
}

/// Parent test view: holds the child `Entity<Probe>` and embeds it via the
/// rev-2 [`cached_child`] helper inside a sized container. The parent
/// re-renders every frame (root always does); the cached child must not, unless
/// its own entity is dirtied.
#[cfg(test)]
struct CachedHost {
    child: gpui::Entity<Probe>,
}

#[cfg(test)]
impl gpui::Render for CachedHost {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled};
        // A sized container so the cached child has real bounds to fill.
        gpui::div()
            .w(px(400.0))
            .h(px(300.0))
            .child(crate::cached_child(self.child.clone()))
    }
}

/// THE keystone proof, rev 2. Three legs:
///   (a) first frame populates the cache: render-count == 1.
///   (b) notify the PARENT only + re-render: the child's render-count MUST stay
///       at 1 — a parent re-render does NOT re-render the un-dirtied cached
///       child. This is the core render-skip guarantee (parent notify dirties
///       the parent + its ancestors, NOT the child — `window.rs:1304`).
///   (c) notify the CHILD entity + re-render: render-count MUST increment to 2
///       (the cached entity is in `dirty_views`, so its render runs).
#[gpui::test]
fn cached_panel_skips_render_until_child_is_notified(cx: &mut TestAppContext) {
    PROBE_RENDERS.with(|c| c.set(0));

    let (host, vcx) = cx.add_window_view(|_window, cx| {
        let child = cx.new(|_cx| Probe { _model: None });
        CachedHost { child }
    });

    // --- (a) Initial frame populates the cache. ---
    vcx.run_until_parked();
    let after_first = PROBE_RENDERS.with(|c| c.get());
    assert_eq!(
        after_first, 1,
        "first frame must render the child exactly once (cache populated), got {after_first}"
    );

    // --- (b) Notify the PARENT only. The child is NOT in dirty_views, so its
    //     cached prepaint is reused and render() is SKIPPED. If `cached_child`
    //     forgot `.cached()`, the parent re-render would re-run the child here
    //     and this assert would fail — so it is NOT a tautology. ---
    host.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    let after_parent_notify = PROBE_RENDERS.with(|c| c.get());
    assert_eq!(
        after_parent_notify, 1,
        "a PARENT re-render must NOT re-render the un-dirtied cached child \
         (render-skip is the whole point); render-count went to {after_parent_notify}"
    );

    // --- (c) Notify the CHILD entity. It enters `dirty_views`, so cached()
    //     re-runs its render. This is the rev-2 invalidation: notify the view
    //     itself (what a `cx.observe` callback or mutation site would do). ---
    host.update(vcx, |v, cx| v.child.update(cx, |_p, cx| cx.notify()));
    vcx.run_until_parked();
    let after_child_notify = PROBE_RENDERS.with(|c| c.get());
    assert_eq!(
        after_child_notify, 2,
        "notifying the cached child entity must re-run its render exactly once more, \
         got {after_child_notify}"
    );
}

/// CANONICAL OBSERVE-PROTOCOL test. A view `cx.observe`s a model entity; the
/// callback `cx.notify()`s the VIEW. Mutating the model inside `cx.update` (the
/// mutation site notifies the model) fires the observer OUTSIDE the draw
/// (`apply_notify_effect`, `app.rs:1301`), which notifies the view, dirtying it
/// — so the NEXT frame re-renders the cached child fresh, zero frames late.
/// This is the rev-2 cache-busting path (`project.md` fact 5 / component model).
#[gpui::test]
fn cached_observe_protocol_busts_cache_fresh(cx: &mut TestAppContext) {
    PROBE_RENDERS.with(|c| c.set(0));

    let (host, vcx) = cx.add_window_view(|_window, cx| {
        // The domain model: a plain entity standing in for an AgentSession.
        let model = cx.new(|_cx| 0u64);
        // The cached child observes the model and self-notifies on change —
        // the canonical `cx.observe(model) -> cx.notify(view)` wiring.
        let child = cx.new(|cx| {
            cx.observe(&model, |_probe, _model, cx| {
                // Outside the draw (effect flush): legitimate to notify the view.
                crate::record_notify("test-probe", crate::MissReason::Dirtied);
                cx.notify();
            })
            .detach();
            Probe {
                _model: Some(model.clone()),
            }
        });
        CachedHost { child }
    });

    // First frame populates the cache.
    vcx.run_until_parked();
    assert_eq!(
        PROBE_RENDERS.with(|c| c.get()),
        1,
        "first frame renders the child once"
    );

    // Mutate the MODEL at its mutation site (notify the model). The view's
    // observe callback fires in effect flush and notifies the view; the next
    // frame re-renders the cached child — fresh, not a frame late.
    host.update(vcx, |v, cx| {
        let model = v.child.read(cx)._model.clone().expect("model");
        model.update(cx, |n, cx| {
            *n += 1;
            cx.notify();
        });
    });
    vcx.run_until_parked();
    assert_eq!(
        PROBE_RENDERS.with(|c| c.get()),
        2,
        "mutating the observed model must bust the cached child's cache via the \
         observe->notify protocol — exactly one extra render, zero frames late"
    );
}

/// TIMING-LAW PIN. A `cx.notify` issued from INSIDE a `render()` is parked
/// (`invalidate_view` under `draw_phase != None`, `window.rs:116`): it does NOT
/// dirty the current frame (`dirty_views` is drained at draw start,
/// `window.rs:1926`) and does NOT schedule a next frame (the loop draws only
/// when `is_dirty()`, `window.rs:128`). So a view that notifies itself from
/// render renders ONCE and then goes quiet — the render count stays flat until
/// an EXTERNAL notify arrives. This pins the exact gpui behavior rev 1 tripped
/// over; a gpui change that makes mid-draw notify self-perpetuate (or schedule
/// a frame) would spin the render loop and fail loudly here.
#[gpui::test]
fn cached_notify_from_render_is_parked(cx: &mut TestAppContext) {
    use std::cell::Cell;
    thread_local! {
        static SELF_NOTIFY_RENDERS: Cell<u64> = const { Cell::new(0) };
    }

    // A view that (illegally) notifies itself from inside render().
    struct SelfNotifier;
    impl gpui::Render for SelfNotifier {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::Styled;
            SELF_NOTIFY_RENDERS.with(|c| c.set(c.get() + 1));
            // THE forbidden call. Under the timing law this is parked: it must
            // not re-dirty this view for another frame.
            cx.notify();
            gpui::div().size_full()
        }
    }

    SELF_NOTIFY_RENDERS.with(|c| c.set(0));
    let (view, vcx) = cx.add_window_view(|_window, _cx| SelfNotifier);

    // Drive frames. If the mid-draw notify scheduled a redraw, the render loop
    // would spin and this count would climb past 1.
    vcx.run_until_parked();
    let after_first = SELF_NOTIFY_RENDERS.with(|c| c.get());
    assert_eq!(
        after_first, 1,
        "a notify issued from render must NOT schedule another frame on its own; \
         render ran {after_first} times (loop is spinning => timing law broken)"
    );

    // Park again to be sure no deferred frame is pending. Still flat.
    vcx.run_until_parked();
    assert_eq!(
        SELF_NOTIFY_RENDERS.with(|c| c.get()),
        1,
        "mid-draw notify is parked: no redraw is scheduled, count stays flat"
    );

    // An EXTERNAL notify (outside the draw — the legitimate path) DOES schedule
    // a redraw: exactly one more render. Proves the view is still wired, the
    // first assertions weren't passing because rendering was somehow disabled.
    view.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    let after_external = SELF_NOTIFY_RENDERS.with(|c| c.get());
    assert_eq!(
        after_external, 2,
        "an external (non-draw) notify must schedule exactly one redraw, got {after_external}"
    );
}

// ===================================================================
// TICKET 021 — TranscriptView render-count regressions.
//
// The flagship invariants, asserted headlessly via the `cached_panel` render
// counter (`perf_render_count("transcript")`). The GUI cannot be driven for
// PAINT headlessly, so these prove RENDER-skip / render-fresh (the cache path),
// NOT the on-screen pixels — the human runtime `sample` profile remains the
// paint-thread ground truth (flagged in the ticket).
//
// Model (project.md component model): a chatbox keystroke moves no transcript
// slice ⇒ the observe filter does NOT self-notify ⇒ the cached transcript's
// render() is SKIPPED (count FLAT). A worksheet edit / stream chunk / tool
// expand / theme/zoom moves a slice (or a global) ⇒ the view is notified
// OUTSIDE the draw ⇒ the NEXT frame re-renders fresh (count +1), zero frames
// late — including the FINAL append of a streaming burst (the rev-1 stale-tail
// hazard).

/// Boot a browser view, focus it, run a frame, install ONE bound agent slot,
/// and return the focused session's id + entity. The first `render_agent`
/// (driven by the next `run_until_parked`) lazily creates the `TranscriptView`.
#[cfg(test)]
fn boot_with_transcript<'a>(
    cx: &'a mut TestAppContext,
) -> (
    gpui::Entity<YaldaGpuiView>,
    &'a mut gpui::VisualTestContext,
    crate::SessionId,
    gpui::Entity<crate::AgentSession>,
) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));
    // Dismiss the startup splash — it would otherwise short-circuit `render`
    // before `render_agent` runs (wall-clock doesn't advance under
    // `run_until_parked`, so the 1.5s deadline never expires headlessly).
    let (id, session) = view.update(vcx, |v, cx| {
        v.splash_until = None;
        let id = v.focused_bound_session().expect("bound session");
        let ent = v.session_entity(id).expect("session entity");
        cx.notify();
        (id, ent)
    });
    (view, vcx, id, session)
}

/// UXI-Selection-1 (agent surface): X11-style select-to-clipboard over the transcript.
/// A real mouse drag over the rendered transcript selects text and auto-copies
/// it to the system clipboard on release. Drives the REAL `simulate_mouse_*`
/// path (dispatched to `TranscriptView::transcript_mouse_*`), picking drag
/// endpoints from the PAINTED token-hit sink, then reads the clipboard back.
///
/// Negative control: comment out the `write_to_clipboard` in
/// `transcript_mouse_up` and this asserts the clipboard stays the sentinel
/// (fails RED — the copy never fired). A second control: revert the
/// `register_token_on_paint` wiring and the sink is empty ⇒ no endpoints ⇒
/// the assert on a non-empty sink fails.
#[gpui::test]
fn transcript_drag_autocopies_selection_to_clipboard(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (view, vcx, id, session) = boot_with_transcript(cx);

    // Put a known line into the transcript editor (the read-only conversation
    // surface). `programmatic_insert` mirrors a streamed append.
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "Hello agent world here\nsecond line\n");
        cx.notify();
    });
    vcx.run_until_parked();
    // Settle the one-time jump-panel geometry inset (see the transcript-021
    // tests) so painted token bounds are final before we sample them.
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    // Seed a sentinel so "no copy happened" ≠ "copied empty".
    view.update(vcx, |_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("SENTINEL-NOT-COPIED".into()))
    });

    // Grab the transcript view + its painted token-hit sink.
    let tv = view
        .update(vcx, |v, _| v.transcript_views.get(&id).cloned())
        .expect("transcript view exists");
    let tokens: Vec<crate::TokenHit> = tv.update(vcx, |t, _| t.token_hits.borrow().clone());
    assert!(
        !tokens.is_empty(),
        "no transcript tokens registered — the paint-time hit-test sink is empty \
         (register_token_on_paint not wired?)"
    );

    // Drag across the whole first line (its leftmost token's left edge → its
    // rightmost token's right edge, at the row's vertical middle).
    let line0: Vec<&crate::TokenHit> = tokens.iter().filter(|t| t.line_idx == 0).collect();
    assert!(!line0.is_empty(), "first transcript line painted no tokens");
    let left = line0
        .iter()
        .map(|t| t.bounds.left())
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap();
    let right = line0
        .iter()
        .map(|t| t.bounds.right())
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap();
    let midy = line0[0].bounds.top() + (line0[0].bounds.bottom() - line0[0].bounds.top()) / 2.0;
    let start = point(left + px(1.0), midy);
    let end = point(right - px(1.0), midy);

    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();

    let clip = view
        .update(vcx, |_, cx| cx.read_from_clipboard())
        .and_then(|it| it.text())
        .unwrap_or_default();
    assert_ne!(
        clip, "SENTINEL-NOT-COPIED",
        "transcript drag-release did not overwrite the clipboard — auto-copy never fired"
    );
    assert!(
        clip.contains("Hello agent world"),
        "clipboard {clip:?} does not hold the dragged first-line text"
    );
}

/// (a) A chatbox keystroke re-renders the root chrome + compose, but the
/// transcript's render() is SKIPPED: its count stays FLAT. This is finding #1
/// (chatbox keystroke re-lays-out the static transcript) closed.
#[gpui::test]
fn transcript_021_chatbox_keystroke_is_render_flat(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (_view, vcx, _id, session) = boot_with_transcript(cx);

    // First real frame: render_agent runs, creates + renders the transcript.
    // The always-on jump panel (jump-panel; spec-jump-panel.md) insets the
    // content area, so the transcript re-measures ONCE as the window geometry
    // settles. Drive that one-time bounds settle to completion (an extra forced
    // frame) BEFORE capturing the baseline, so what we measure below is purely
    // the keystroke's effect — not chrome layout settling.
    vcx.run_until_parked();
    _view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");
    assert!(base >= 1, "transcript must render at least once on first frame");

    // A real chatbox keystroke mutates the COMPOSE editor (inside the
    // `InputSurface::Chatbox`) and notifies the SESSION — but the transcript's
    // observed seqs (transcript `edit_seq`, frozen/tools gen, cursor, …) read
    // the *transcript* editor, which is untouched. So the observe filter must
    // NOT self-notify, and the cached transcript's render() must be skipped.
    // This is the slice-filter doing its job, not merely `cached()`.
    for _ in 0..5 {
        session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
            if let Some(cb) = s.state.input_surface.chatbox_mut() {
                let len = cb.editor.document().rope().len_chars();
                cb.editor.programmatic_insert(len, "x");
            }
            // The keystroke path notifies the session (mutation-site notify).
            cx.notify();
        });
        vcx.run_until_parked();
    }
    let after = crate::perf_render_count("transcript");
    assert_eq!(
        after, base,
        "a chatbox keystroke (compose-only session mutation) must NOT re-render \
         the cached transcript — the observe slice-filter sees no transcript slice \
         move, so render() is skipped; count must stay flat ({base}), got {after}"
    );
}

/// (b) A worksheet edit (a real session mutation + notify) busts the cache: the
/// observe filter sees `edit_seq` move and self-notifies, so the NEXT frame
/// re-renders the transcript exactly once.
#[gpui::test]
fn transcript_021_session_edit_busts_cache(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (_view, vcx, _id, session) = boot_with_transcript(cx);
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // Mutate the session transcript at its mutation site (insert text + notify
    // the session) — exactly what a worksheet keystroke / stream chunk does.
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state.editor.programmatic_insert(0, "hello from claude\n");
        cx.notify();
    });
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");
    assert_eq!(
        after,
        base + 1,
        "a session transcript edit must re-render the transcript exactly once \
         on the next frame (base {base}), got {after}"
    );
}

/// (b′) The STALE-TAIL case (the rev-1 hazard): a STREAMING BURST of chunks,
/// where the FINAL append must also land a +1. Each chunk is a separate
/// session.update+notify; the last one must not be stranded behind the cache.
#[gpui::test]
fn transcript_021_streaming_burst_final_append_renders(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (_view, vcx, _id, session) = boot_with_transcript(cx);
    vcx.run_until_parked();
    let mut expected = crate::perf_render_count("transcript");

    // Stream several chunks; assert the count advances on EACH, including the
    // final append — there is no frame where a fresh chunk is left invisible.
    for i in 0..4 {
        session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
            s.state
                .editor
                .programmatic_insert(s.state.editor.document().rope().len_chars(), "chunk ");
            cx.notify();
        });
        vcx.run_until_parked();
        expected += 1;
        let now = crate::perf_render_count("transcript");
        assert_eq!(
            now, expected,
            "streaming chunk #{i} must re-render the transcript (expected {expected}), got {now}"
        );
    }
}

/// (b″) THE STALE-TAIL BACKSTOP (Option A): the fingerprint-keyed transcript
/// element id must force a render even when the self-notify hop is DROPPED —
/// the "last agent message never renders" bug. Root cause: the transcript's
/// `cx.observe`→`cx.notify()` silently no-ops when the view has no `view_path`
/// in the committed frame (`mark_view_dirty`, gpui window.rs), so the cached
/// prepaint is reused STALE until an unrelated event heals it.
///
/// We reproduce a dropped notify DETERMINISTICALLY: mutate the transcript
/// editor via `session.update` WITHOUT calling `cx.notify()`. That advances
/// `edit_seq` (so `TranscriptSeqs` moves) while leaving the session's observers
/// unfired — the transcript entity never enters `dirty_views`, exactly as when
/// `mark_view_dirty` eats the notify. We then force a ROOT frame (the root is
/// always in the tree; a real batch does this via `apply_server_batch`'s tail
/// `cx.notify()`). With the fix, the moved fingerprint yields a fresh
/// `GlobalElementId` ⇒ `with_element_state` misses ⇒ the transcript re-renders
/// (+1). Render count is the ground-truth skip/no-skip oracle (a skipped render
/// reuses the stale prepaint AND its paint).
///
/// NEGATIVE CONTROL (observed RED): revert `render_agent`'s transcript embed to
/// the fingerprint-independent `cached_child(transcript_view)`. Then the id is
/// stable, the transcript is not in `dirty_views`, and the bounds/mask/text
/// cache key is unchanged ⇒ the prepaint is reused ⇒ count stays FLAT (base+0)
/// — the stale-tail bug, reproduced. This is the ONLY path that re-renders the
/// transcript here, so the delta is attributable solely to the id backstop.
#[gpui::test]
fn transcript_dropped_notify_id_forces_render(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (view, vcx, _id, session) = boot_with_transcript(cx);
    // Settle the one-time jump-panel bounds inset (see the 021 tests) BEFORE
    // the baseline, so what we measure is purely the id backstop — not chrome
    // geometry settling busting the cache on its own.
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");
    assert!(base >= 1, "transcript must render at least once on first frame");

    // SILENT transcript mutation — the dropped-notify simulation. `edit_seq`
    // advances (fingerprint moves) but NO `cx.notify()` fires, so the observe
    // callback never runs and the transcript entity never enters `dirty_views`.
    session.update(vcx, |s, _cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "DROPPED_NOTIFY_TAIL\n");
        // Deliberately NO cx.notify(): this is the dropped-notify condition.
    });
    // Force a ROOT frame without touching the session (root is always in the
    // committed tree — its notify can't be parked). The transcript can ONLY
    // re-render here via the fingerprint-keyed element id.
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let after = crate::perf_render_count("transcript");
    assert_eq!(
        after,
        base + 1,
        "a transcript mutation whose self-notify was DROPPED must still force a \
         render via the fingerprint-keyed element id (base {base}), got {after} — \
         the stale-tail backstop failed"
    );
}

/// (c) A tool-group expand toggle bumps `tools_gen`, which the observe filter
/// watches ⇒ +1. Toggling expanded is the canonical tool-structure mutation.
#[gpui::test]
fn transcript_021_tool_expand_busts_cache(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (_view, vcx, _id, session) = boot_with_transcript(cx);
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state.tools.toggle_expanded("anchor-7");
        cx.notify();
    });
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");
    assert_eq!(
        after,
        base + 1,
        "a tool expand toggle (tools_gen bump) must re-render the transcript once \
         (base {base}), got {after}"
    );
}

/// (d) Theme and zoom are GLOBAL: their action handlers notify each live
/// transcript view directly (event context). Each must yield +1.
#[gpui::test]
fn transcript_021_theme_and_zoom_bust_cache(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // Theme swap → notify_transcript_views(Refresh).
    view.update(vcx, |v, cx| {
        v.set_theme(crate::ThemeName::Nightfox, cx);
    });
    vcx.run_until_parked();
    let after_theme = crate::perf_render_count("transcript");
    assert_eq!(
        after_theme,
        base + 1,
        "a theme swap must re-render the transcript once (base {base}), got {after_theme}"
    );

    // Zoom in → notify_transcript_views(TextStyle). Drive the scale setter
    // directly (the `zoom_in` action handler is a thin wrapper over it that
    // also needs a `&mut Window` we don't have headlessly).
    view.update(vcx, |v, cx| {
        v.set_text_scale(v.text_scale * crate::TEXT_SCALE_STEP, cx);
    });
    vcx.run_until_parked();
    let after_zoom = crate::perf_render_count("transcript");
    assert_eq!(
        after_zoom,
        after_theme + 1,
        "a zoom-in must re-render the transcript once ({after_theme} -> got {after_zoom})"
    );
}

/// UXI-TextZoom-1: the transcript's conversation PROSE actually scales with document
/// zoom — the painted height of a prose line grows when `text_scale` grows. The
/// layout probe gives real painted bounds, so this is a headless guard for the
/// font-px effect (not just the cache-bust). Pins the fix for "agent text didn't
/// resize with Cmd-±" — the size must live on the line's own wrapper, since the
/// `claude-body` ambient doesn't cross the `gpui::list` item boundary.
#[gpui::test]
fn transcript_prose_scales_with_zoom(cx: &mut TestAppContext) {
    let (view, vcx, id, session) = boot_with_transcript(cx);
    // Seed a line of prose into the transcript editor so `FlatItem::Line(0)`
    // renders (the probed row). Mirrors `transcript_021_session_edit_busts_cache`.
    session.update(vcx, |s, cx| {
        s.state
            .editor
            .programmatic_insert(0, "hello world, a line of agent prose\n");
        cx.notify();
    });
    // Focus the transcript so line 0 carries the cursor — `gpui::list` only
    // paints VISIBLE items, and `pending_reveal_cursor` (below) scrolls the
    // cursor row into view so the probe has a painted target.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.focus = crate::AgentFocus::Transcript);
    });
    view.update(vcx, |v, cx| v.set_text_scale(1.0, cx));
    for _ in 0..4 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    let probe_h = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| -> f32 {
        for _ in 0..2 {
            view.update(vcx, |v, cx| {
                if let Some(mut c) = v.agent_mut(cx) {
                    c.pending_reveal_cursor = true;
                }
                cx.notify();
            });
            vcx.run_until_parked();
        }
        crate::layout_probe_begin();
        view.update(vcx, |v, cx| {
            if let Some(mut c) = v.agent_mut(cx) {
                c.pending_reveal_cursor = true;
            }
            cx.notify();
        });
        vcx.run_until_parked();
        let b = crate::layout_probe_get("transcript-line0");
        crate::layout_probe_end();
        b.expect("transcript line 0 did not paint").3
    };

    let h1 = probe_h(&view, vcx);
    assert!(h1 > 1.0, "prose line has no height at scale 1.0 ({h1})");

    // Zoom to 2x; the SAME prose line must paint materially taller.
    view.update(vcx, |v, cx| v.set_text_scale(2.0, cx));
    for _ in 0..4 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    let h2 = probe_h(&view, vcx);
    assert!(
        h2 > h1 * 1.4,
        "prose did NOT scale with zoom: line height {h1}px (1x) vs {h2}px (2x)"
    );
}

/// Heading-marker toggle is a GLOBAL transcript render input (agent `.` menu →
/// "toggle heading markers"), pushed via `notify_transcript_views(Refresh)` like
/// theme/zoom — not a per-session seq. Flipping it must re-render the transcript
/// exactly once and flip the root flag. Default on, so the first toggle is off.
#[gpui::test]
fn transcript_021_heading_marker_toggle_busts_cache(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // Default on.
    assert!(
        view.read_with(vcx, |v, _| v.show_agent_heading_markers),
        "heading markers default on"
    );

    // Toggle off → one transcript re-render, flag flips.
    view.update(vcx, |v, cx| v.toggle_agent_heading_markers(cx));
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");
    assert_eq!(
        after,
        base + 1,
        "toggling heading markers must re-render the transcript once (base {base}), got {after}"
    );
    assert!(
        !view.read_with(vcx, |v, _| v.show_agent_heading_markers),
        "toggle flips the flag off"
    );
}

/// User-turn jump mode (agent `.` menu → "jump between user turns"): the toggle
/// handler flips the per-session `user_turn_jump_mode` flag, and `pending_jump`
/// is a covered `TranscriptSeqs` render input — setting `pending_jump_ord`
/// self-notifies the cached transcript (the observe slice-filter sees the seq
/// move). With no user turns yet, toggling-on reports the empty case via the
/// session status and leaves no jump queued.
#[gpui::test]
fn transcript_021_user_turn_jump_toggle(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (view, vcx, _id, session) = boot_with_transcript(cx);
    vcx.run_until_parked();

    // Toggle: per-session flag flips off → on.
    assert!(
        !session.read_with(vcx, |s, _| s.state.user_turn_jump_mode),
        "jump mode default off"
    );
    view.update(vcx, |v, cx| v.toggle_agent_jump_mode(cx));
    vcx.run_until_parked();
    assert!(
        session.read_with(vcx, |s, _| s.state.user_turn_jump_mode),
        "toggle turns jump mode on"
    );
    // No user turns in a fresh transcript → no jump queued, empty-case status.
    assert_eq!(
        session.read_with(vcx, |s, _| s.state.pending_jump_ord),
        None,
        "no user turns ⇒ nothing to jump to"
    );

    // Toggle off again.
    view.update(vcx, |v, cx| v.toggle_agent_jump_mode(cx));
    vcx.run_until_parked();
    assert!(
        !session.read_with(vcx, |s, _| s.state.user_turn_jump_mode),
        "toggle turns jump mode back off"
    );

    // SEQ-COVERAGE for `pending_jump` (CLAUDE.md rule 2): a queued jump must be
    // visible to the observe slice-filter, i.e. `TranscriptSeqs::of` must move
    // when `pending_jump_ord` becomes `Some`. Assert the fingerprint directly
    // (independent of the headless redraw scheduler).
    let before = session.read_with(vcx, |s, _| crate::TranscriptSeqs::of(&s.state).pending_jump);
    session.update(vcx, |s, _cx| s.state.pending_jump_ord = Some(0));
    let after = session.read_with(vcx, |s, _| crate::TranscriptSeqs::of(&s.state).pending_jump);
    assert!(!before, "no jump queued ⇒ pending_jump seq is false");
    assert!(after, "a queued jump ⇒ pending_jump seq flips true (busts the cache)");
}

/// (e) Follow-tail still reveals grown content: after a streaming append while
/// following, the list's registered item count grows to match the freshly
/// built flat-items (the reconcile + reveal path runs in TranscriptView's
/// render). Reads the count through the view's `TranscriptScroll`.
#[gpui::test]
fn transcript_021_follow_tail_reveals_grown_content(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (view, vcx, id, session) = boot_with_transcript(cx);
    vcx.run_until_parked();

    let count_before = view.update(vcx, |v, cx| {
        v.transcript_views
            .get(&id)
            .map(|tv| tv.read(cx).scroll.list_item_count)
            .unwrap_or(0)
    });

    // Append several lines (rows) while following (the default).
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "line one\nline two\nline three\n");
        cx.notify();
    });
    vcx.run_until_parked();

    let count_after = view.update(vcx, |v, cx| {
        v.transcript_views
            .get(&id)
            .map(|tv| tv.read(cx).scroll.list_item_count)
            .unwrap_or(0)
    });
    assert!(
        count_after > count_before,
        "follow-tail: the transcript's registered item count must grow with the \
         appended rows (before {count_before}, after {count_after})"
    );
}

/// (f) SEQ-COVERAGE for `c.mode`: a bare worksheet mode flip (Normal⇄Insert)
/// moves NO other seq — no cursor move, no `edit_seq` bump — yet `make_caret`
/// draws the under-cursor CHARACTER in Normal vs a BLANK block in Insert. So
/// `mode` is a render input that MUST be in `TranscriptSeqs`; flipping it alone
/// must self-notify and re-render the transcript exactly once. Without the
/// `mode` field the observe filter returns `None` ⇒ stale caret (the bug the
/// adversarial review flagged: `i`/`a` into Insert, or `Esc` at col 0).
#[gpui::test]
fn transcript_021_mode_flip_busts_cache(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (_view, vcx, _id, session) = boot_with_transcript(cx);
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // Flip ONLY the edit mode (what a bare `i`/`a` does: begin_insert flips
    // `*mode` with no cursor move and no edit_seq bump). Notify the session as
    // the key handler does.
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        let before = s.state.mode;
        s.state.mode = match before {
            crate::EditMode::Normal => crate::EditMode::Insert,
            crate::EditMode::Insert => crate::EditMode::Normal,
        };
        cx.notify();
    });
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");
    assert_eq!(
        after,
        base + 1,
        "a bare worksheet mode flip (caret glyph change, no other seq move) must \
         re-render the transcript once so the caret is fresh (base {base}), got {after}"
    );
}

/// (g) The thinking-indicator CLOCK lives inside the cached `TranscriptView`.
/// Its ~1Hz anim tick must bust the cached child for every awaiting session —
/// a root notify cannot (facts 3/6) and no session seq moves during a stall.
/// `tick_awaiting_transcript_views` is that route; here it must yield +1 on an
/// awaiting session and 0 on an idle one (the bug the review flagged: the clock
/// froze during a stall because the tick notified the root, not the view).
#[gpui::test]
fn transcript_021_anim_tick_busts_awaiting_cache(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (view, vcx, _id, session) = boot_with_transcript(cx);
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // IDLE: a tick must NOT touch the transcript (nothing is awaiting).
    let ticked_idle = view.update(vcx, |v, cx| v.tick_awaiting_transcript_views(cx));
    vcx.run_until_parked();
    assert!(!ticked_idle, "idle session: anim tick must notify no transcript view");
    assert_eq!(
        crate::perf_render_count("transcript"),
        base,
        "idle session: anim tick must not re-render the transcript"
    );

    // AWAITING: put the session mid-turn (with timers in the past so the clock
    // is live), then tick — the cached transcript MUST re-render so the
    // `Thinking… mm:ss` clock advances.
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        let past = std::time::Instant::now() - std::time::Duration::from_secs(35);
        s.state.turn_phase = crate::TurnPhase::Awaiting {
            started: past,
            last_event: past,
        };
        cx.notify();
    });
    vcx.run_until_parked();
    // The awaiting flip itself bumps the `awaiting` seq ⇒ one render. Anchor the
    // anim-tick assertion off the post-flip count.
    let after_await = crate::perf_render_count("transcript");

    let ticked = view.update(vcx, |v, cx| v.tick_awaiting_transcript_views(cx));
    vcx.run_until_parked();
    assert!(ticked, "awaiting session: anim tick must notify its transcript view");
    let after_tick = crate::perf_render_count("transcript");
    assert_eq!(
        after_tick,
        after_await + 1,
        "awaiting anim tick must bust the cached transcript so the stall clock \
         advances (post-await {after_await}), got {after_tick}"
    );
}

// ── yux cached-body regression: LinearView (linear_view.rs) ─────────────────
// The Linear tile's body is a cached child entity. Typing in the tile's input
// line notifies the ROOT (re-rendering the input row only); the body's entity
// is NOT notified, so its render() is skipped. These pin that contract — the
// yux/CLAUDE.md rule-5 render-count test for the second cached surface.

/// Boot a window, open a Linear tile, render once (lazily creating the cached
/// `LinearView`), and return the view + its body entity.
#[cfg(test)]
fn boot_with_linear<'a>(
    cx: &'a mut TestAppContext,
) -> (
    gpui::Entity<YaldaGpuiView>,
    &'a mut gpui::VisualTestContext,
    gpui::Entity<crate::LinearView>,
) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        v.open_linear_inner(cx);
    });
    vcx.run_until_parked();
    let lv = view.update(vcx, |v, _cx| match v.workspace.focused_content() {
        Some(crate::App::Linear(tile)) => {
            tile.view.clone().expect("render_linear lazily creates the LinearView")
        }
        _ => panic!("expected a focused Linear tile"),
    });
    (view, vcx, lv)
}

/// Typing in the Linear input notifies only the root; the cached body's
/// render() is SKIPPED (count stays flat).
#[gpui::test]
fn linear_input_keystroke_is_render_flat(cx: &mut TestAppContext) {
    crate::perf_reset("linear");
    let (view, vcx, _lv) = boot_with_linear(cx);
    // Absorb the one-time bounds settle the always-on jump panel induces
    // (it insets the content area) before baselining — see the matching note in
    // `transcript_021_chatbox_keystroke_is_render_flat`.
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("linear");
    assert!(base >= 1, "linear body must render at least once on first frame");

    for _ in 0..5 {
        view.update(vcx, |v, cx| {
            if let Some(crate::App::Linear(tile)) = v.workspace.focused_content_mut() {
                tile.input.push('x');
            }
            cx.notify(); // mirrors handle_linear_key's Char path (root notify)
        });
        vcx.run_until_parked();
    }
    let after = crate::perf_render_count("linear");
    assert_eq!(
        after, base,
        "typing in the Linear input (root-only notify) must NOT re-render the \
         cached body; count must stay flat ({base}), got {after}"
    );
}

/// A body payload change (a fetch landing) notifies the body entity itself, so
/// the NEXT frame re-renders it exactly once.
#[gpui::test]
fn linear_state_change_busts_cache(cx: &mut TestAppContext) {
    crate::perf_reset("linear");
    let (_view, vcx, lv) = boot_with_linear(cx);
    vcx.run_until_parked();
    let base = crate::perf_render_count("linear");

    lv.update(vcx, |v, cx| {
        v.set_state(crate::LinearViewState::Error("boom".into()));
        cx.notify(); // mutation-site notify (the only thing that busts the cache)
    });
    vcx.run_until_parked();

    let after = crate::perf_render_count("linear");
    assert_eq!(
        after,
        base + 1,
        "a body payload change must re-render the cached body exactly once \
         (base {base}), got {after}"
    );
}

/// Navigating the project picker (a body-owned mutation) busts the cached body
/// exactly once per move — the picker is part of LinearView's state.
#[gpui::test]
fn linear_picker_move_busts_cache(cx: &mut TestAppContext) {
    crate::perf_reset("linear");
    let (_view, vcx, lv) = boot_with_linear(cx);
    vcx.run_until_parked();

    // Seed a multi-candidate picker.
    lv.update(vcx, |v, cx| {
        let cands = vec![
            crate::ProjectCandidate {
                id: "a".into(),
                name: Some("Alpha".into()),
                state: Some("started".into()),
            },
            crate::ProjectCandidate {
                id: "b".into(),
                name: Some("Beta".into()),
                state: Some("planned".into()),
            },
        ];
        v.set_state(crate::LinearViewState::ProjectPicker {
            candidates: cands,
            selected: 0,
        });
        cx.notify();
    });
    vcx.run_until_parked();
    let base = crate::perf_render_count("linear");

    // One move → one body re-render; selection advanced.
    lv.update(vcx, |v, cx| {
        v.picker_move(1);
        cx.notify();
    });
    vcx.run_until_parked();

    let after = crate::perf_render_count("linear");
    assert_eq!(
        after,
        base + 1,
        "a picker move must re-render the cached body exactly once (base {base}), got {after}"
    );
    let sel = lv.update(vcx, |v, _| v.selected_candidate().and_then(|c| c.name));
    assert_eq!(sel.as_deref(), Some("Beta"), "picker_move advanced the selection");
}

/// Entering browse on a loaded project body puts the cursor on its first issue,
/// and moving the browse cursor (a body-owned mutation) busts the cached body
/// exactly once. (The tile's Normal mode calls `enter_select`; here we drive the
/// view directly, so we call it explicitly.)
#[gpui::test]
fn linear_nav_move_busts_cache(cx: &mut TestAppContext) {
    crate::perf_reset("linear");
    let (_view, vcx, lv) = boot_with_linear(cx);
    vcx.run_until_parked();

    // Seed a project with two issues — the body's NavTargets.
    lv.update(vcx, |v, cx| {
        let issue = |id: &str| crate::IssueRef {
            identifier: Some(id.into()),
            title: Some(format!("{id} title")),
            state: None,
        };
        v.set_state(crate::LinearViewState::Project(Box::new(crate::ProjectDetail {
            name: Some("Fulcrum".into()),
            description: None,
            content: None,
            state: None,
            url: None,
            lead: None,
            target_date: None,
            milestones: None,
            issues: Some(crate::NodeList {
                nodes: vec![issue("FUL-19"), issue("FUL-620")],
            }),
            updates: None,
        })));
        v.enter_select();
        cx.notify();
    });
    vcx.run_until_parked();
    let base = crate::perf_render_count("linear");

    // enter_select put the cursor on the first target.
    let target0 = lv.update(vcx, |v, _| match v.selected_target() {
        Some(crate::NavTarget::Issue(id)) => Some(id),
        _ => None,
    });
    assert_eq!(target0.as_deref(), Some("FUL-19"), "browse starts on first issue");

    // One move → one body re-render; cursor advanced to the next issue.
    lv.update(vcx, |v, cx| {
        v.nav_move(1);
        cx.notify();
    });
    vcx.run_until_parked();

    let after = crate::perf_render_count("linear");
    assert_eq!(
        after,
        base + 1,
        "a browse-cursor move must re-render the cached body exactly once (base {base}), got {after}"
    );
    let target1 = lv.update(vcx, |v, _| match v.selected_target() {
        Some(crate::NavTarget::Issue(id)) => Some(id),
        _ => None,
    });
    assert_eq!(target1.as_deref(), Some("FUL-620"), "nav_move advanced the cursor");
}

/// The Linear tile is modal: in Normal mode printable keys are commands, not
/// text — `<space>` opens the tile/app (LINEAR) menu (so menus are reachable at
/// all), and a non-bound letter is a no-op (never typed into the query).
/// Regression for the "can't access any menus, every key types into the input" trap.
#[gpui::test]
fn linear_normal_mode_frees_keys_for_menus(cx: &mut TestAppContext) {
    use crate::{Key, KMods, KeyPress, LinearMode};
    let kp = |c: char| KeyPress::new(Key::Char(c), KMods::NONE);
    let (view, vcx, _lv) = boot_with_linear(cx);

    // Default mode is Insert: a letter types into the query.
    view.update(vcx, |v, cx| {
        v.handle_linear_insert_key(kp('x'), cx);
    });
    let typed = view.update(vcx, |v, _| match v.workspace.focused_content() {
        Some(crate::App::Linear(t)) => t.input.clone(),
        _ => String::new(),
    });
    assert_eq!(typed, "x", "Insert mode types printable keys into the query");

    // Switch to Normal: a letter is now a no-op (NOT appended), and `<space>`
    // opens the global menu instead of inserting a space.
    view.update(vcx, |v, cx| {
        v.linear_set_mode(LinearMode::Normal, cx);
        v.handle_linear_normal_key(kp('z'), cx);
    });
    let after_letter = view.update(vcx, |v, _| match v.workspace.focused_content() {
        Some(crate::App::Linear(t)) => t.input.clone(),
        _ => String::new(),
    });
    assert_eq!(after_letter, "x", "Normal mode does NOT type unbound letters");

    // `<space>` in Normal mode is intercepted as a leader (universal path) and
    // opens the tile/app (LINEAR) local menu — the tile is not in text entry.
    let (consumed, opened, header) = view.update(vcx, |v, cx| {
        let consumed = v.leader_intercept(&kp(' '), cx);
        let header = v.menu_ref().map(|m| m.header);
        (consumed, v.overlay_is_menu(), header)
    });
    assert!(consumed, "`<space>` is consumed as a leader in Normal mode");
    assert!(opened, "`<space>` in Normal mode opens the menu");
    assert_eq!(header, Some("LINEAR"), "`<space>` opens the tile/app (LINEAR) local menu");

    // And `.` (after closing the space menu) opens the per-workspace menu.
    let dot_header = view.update(vcx, |v, cx| {
        v.clear_overlay();
        v.leader_intercept(&kp('.'), cx);
        v.menu_ref().map(|m| m.header)
    });
    assert_eq!(dot_header, Some("MENU"), "`.` opens the per-workspace command menu");
}

/// The universal leader rule: when a tile is NOT in text entry, `<space>`/`.`/
/// `?` are intercepted as menu-openers; when it IS (e.g. Linear Insert mode),
/// they are left for the tile to type. Covers the "leaders have highest
/// priority when not in insert mode" property.
#[gpui::test]
fn leader_intercept_respects_insert_mode(cx: &mut TestAppContext) {
    use crate::{Key, KMods, KeyPress, LinearMode};
    let kp = |c: char| KeyPress::new(Key::Char(c), KMods::NONE);
    let (view, vcx, _lv) = boot_with_linear(cx);

    // Linear opens in Insert: a leader is NOT intercepted (it's text).
    let insert = view.update(vcx, |v, cx| v.leader_intercept(&kp(' '), cx));
    assert!(!insert, "in Insert mode a leader is left to the tile as text");

    // Switch to Normal: now the leader IS intercepted.
    let normal = view.update(vcx, |v, cx| {
        v.linear_set_mode(LinearMode::Normal, cx);
        v.leader_intercept(&kp('.'), cx)
    });
    assert!(normal, "in Normal mode a leader is intercepted as a menu-opener");
}

/// REGRESSION (live report: in worksheet mode `<space>` opened the WORKSPACE
/// menu instead of the tile menu). `focused_in_insert_mode` for a bound agent
/// must reflect the COMPOSE buffer (focus + its mode) in BOTH placements — never
/// the read-only transcript editor's `mode`, which defaults to Insert and so
/// wrongly reported "in text entry" in worksheet, suppressing the universal
/// leaders. With leaders suppressed, a bare `<space>` fell into the compose's
/// Normal-key dispatch → `NormalOutcome::OpenMenu` → the workspace menu.
#[gpui::test]
fn focused_in_insert_mode_tracks_compose_not_transcript(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Worksheet placement, focus on the compose, compose in NORMAL — but the
    // transcript editor's own mode is Insert (its default). The bug read THAT.
    let in_insert = view.update(vcx, |v, cx| {
        {
            let mut c = v.agent_mut(cx).expect("agent");
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.input_surface.compose_mut().mode = crate::EditMode::Normal;
            c.focus = crate::AgentFocus::Compose;
            c.mode = crate::EditMode::Insert; // transcript default — must be ignored
        }
        v.focused_in_insert_mode(cx)
    });
    assert!(
        !in_insert,
        "compose in Normal ⇒ NOT text entry ⇒ leaders fire (space → tile menu)"
    );

    // Compose in Insert with a NON-EMPTY draft ⇒ text entry ⇒ leaders are left to the
    // tile as text. (An empty worksheet block keeps the leaders live — the empty-draft
    // heuristic — so seed a char to exercise the genuine text-entry case.)
    let in_insert2 = view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.input_surface.compose_mut().mode = crate::EditMode::Insert;
        c.input_surface.compose_mut().editor.insert_char('x');
        drop(c);
        v.focused_in_insert_mode(cx)
    });
    assert!(in_insert2, "compose in Insert + non-empty draft ⇒ text entry ⇒ space types");

    // Transcript focus is read-only NAVIGATION ⇒ leaders fire even though the
    // compose is still Insert.
    let in_insert3 = view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").focus = crate::AgentFocus::Transcript;
        v.focused_in_insert_mode(cx)
    });
    assert!(
        !in_insert3,
        "transcript focus ⇒ navigation ⇒ leaders fire (space → tile menu)"
    );
}

/// The global (Yaldabaoth) menu lists every workspace by number with a
/// `goto-workspace-N` command, plus name/new entries; dispatching one switches
/// the active workspace. Covers untitled.md "Global Scope › Commands".
#[gpui::test]
fn global_menu_lists_and_switches_workspaces(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();

    // Add two more workspaces (3 total). A bare Linear tile needs no args.
    view.update(vcx, |v, _cx| {
        let cwd = crate::workspace::WorkspaceCwd::new(PathBuf::from("."));
        v.workspace
            .push_initial_tab(crate::App::Linear(crate::LinearTile::new()), cwd.clone());
        v.workspace
            .push_initial_tab(crate::App::Linear(crate::LinearTile::new()), cwd);
    });

    // The menu enumerates each workspace + the name/new commands.
    let cmds: Vec<String> = view.update(vcx, |v, _| {
        v.global_menu()
            .iter()
            .filter_map(|n| match &n.action {
                crate::MenuAction::Command(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    });
    for expect in [
        "goto-workspace-0",
        "goto-workspace-1",
        "goto-workspace-2",
        "rename-tab",
        "new-tab",
    ] {
        assert!(cmds.contains(&expect.to_string()), "global menu missing {expect}: {cmds:?}");
    }

    // Dispatching a goto command switches the active workspace.
    view.update(vcx, |v, cx| v.dispatch_menu_command("goto-workspace-2", cx));
    let active = view.update(vcx, |v, _| v.workspace.active_tab);
    assert_eq!(active, 2, "goto-workspace-2 activated the third workspace");

    view.update(vcx, |v, cx| v.dispatch_menu_command("goto-workspace-0", cx));
    let active = view.update(vcx, |v, _| v.workspace.active_tab);
    assert_eq!(active, 0, "goto-workspace-0 activated the first workspace");
}

/// The universal agent roster (universal-agent-list), driven through the REAL
/// `apply_server_batch`: a `SessionCreated` broadcast for a session this GUI has
/// NEVER opened makes it appear in the jump panel's agent rows (always-visible
/// active sessions); a `SessionRenamed` updates its label in place; a
/// `SessionClosed` removes it. This is the end-to-end wire the no-op hook used
/// to drop on the floor.
#[gpui::test]
fn roster_surfaces_unopened_session_and_tracks_rename_close(cx: &mut TestAppContext) {
    use yalda::session_proto::Notification as ServerNotification;
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();

    let info = SessionInfo {
        session_id: "srv-1".into(),
        acp_session_id: Some("acp-1".into()),
        label: "claude-7".into(),
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::PermissionMode::ReadOnly,
    };

    // A session created elsewhere on the server — never opened in this GUI.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionCreated {
                session: info.clone(),
            }],
            cx,
        );
    });
    let rows = view.update(vcx, |v, cx| v.jump_panel_agent_rows(cx));
    assert_eq!(rows.len(), 1, "roster session appears in the jump panel");
    assert_eq!(rows[0].label, "claude-7");
    assert!(!rows[0].bound, "an unopened session is free (no tile binds it)");
    assert!(matches!(rows[0].target, crate::JumpTarget::Roster(ref s) if s == "srv-1"));

    // A rename broadcast updates the label in place.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionRenamed {
                session_id: "srv-1".into(),
                label: "renamed-session".into(),
            }],
            cx,
        );
    });
    let rows = view.update(vcx, |v, cx| v.jump_panel_agent_rows(cx));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "renamed-session", "rename updates the row label");

    // A close broadcast removes it from the roster (and so from the panel).
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionClosed {
                session_id: "srv-1".into(),
            }],
            cx,
        );
    });
    let rows = view.update(vcx, |v, cx| v.jump_panel_agent_rows(cx));
    assert!(rows.is_empty(), "closed session is gone from the roster");
}

/// Unit (jump-reorder, UXI-JumpPanel-2): `order_grouped_rows` applies the user's
/// drag-reordered order on top of the cwd grouping, and is a total no-op when
/// both order lists are empty (the default — alphabetical groups, by-label
/// sessions). Doubles as the negative control: the empty-order assertion holds
/// only because the sort ranks unlisted items last (stable), and the non-empty
/// assertions hold only because the order lists actually drive the sort — revert
/// either sort in `order_grouped_rows` and one of these fails.
#[test]
fn jump_reorder_ordering_applies_and_defaults_to_alpha() {
    use crate::{group_agent_rows_by_cwd, order_grouped_rows, AgentRow, JumpTarget};
    let row = |sid: &str, label: &str, cwd: &str| AgentRow {
        target: JumpTarget::Roster(sid.into()),
        label: label.into(),
        cwd: std::path::PathBuf::from(cwd),
        bound: false,
        connected: true,
        awaiting: None,
    };
    // Two projects; alpha has two sessions (incoming by-label a,b), beta one.
    let mk = || {
        vec![
            row("s-a", "a", "/work/alpha"),
            row("s-b", "b", "/work/alpha"),
            row("s-z", "z", "/work/beta"),
        ]
    };
    let keys = |g: &Vec<(String, Vec<(usize, AgentRow)>)>| {
        g.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
    };
    let sess = |g: &Vec<(String, Vec<(usize, AgentRow)>)>, idx: usize| {
        g[idx].1.iter().map(|(_, r)| r.label.clone()).collect::<Vec<_>>()
    };

    // Empty orders → default: groups alphabetical (alpha, beta); alpha's
    // sessions in by-label order (a, b). (Negative control for "no drag".)
    let g = order_grouped_rows(group_agent_rows_by_cwd(mk()), &[], &[]);
    assert_eq!(keys(&g), vec!["/work/alpha", "/work/beta"], "default: alpha before beta");
    assert_eq!(sess(&g, 0), vec!["a", "b"], "default: sessions by label");

    // A cwd order flips the groups (beta before alpha).
    let cwd_order = vec!["/work/beta".to_string(), "/work/alpha".to_string()];
    let g = order_grouped_rows(group_agent_rows_by_cwd(mk()), &cwd_order, &[]);
    assert_eq!(keys(&g), vec!["/work/beta", "/work/alpha"], "cwd order reorders headers");

    // A session order flips alpha's sessions (b before a); groups still alpha.
    let sess_order = vec!["s-b".to_string(), "s-a".to_string()];
    let g = order_grouped_rows(group_agent_rows_by_cwd(mk()), &[], &sess_order);
    let alpha_idx = keys(&g).iter().position(|k| k == "/work/alpha").unwrap();
    assert_eq!(sess(&g, alpha_idx), vec!["b", "a"], "session order reorders within group");
}

/// Unit (jump-reorder): `reorder_move` drops the dragged item into the target's
/// slot (target shifts down); a no-op when dragged == target or absent.
#[test]
fn jump_reorder_move_semantics() {
    use crate::reorder_move;
    let mut v = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    // Drag c onto a → c takes a's slot.
    reorder_move(&mut v, "c", "a");
    assert_eq!(v, vec!["c", "a", "b"]);
    // Drag c onto b → c between a and b's new positions (b's slot).
    reorder_move(&mut v, "c", "b");
    assert_eq!(v, vec!["a", "c", "b"], "dragged item lands in target's slot");
    // Same item is a no-op.
    let before = v.clone();
    reorder_move(&mut v, "a", "a");
    assert_eq!(v, before, "self-drop is a no-op");
    // Missing dragged is a no-op.
    reorder_move(&mut v, "zzz", "a");
    assert_eq!(v, before, "absent dragged is a no-op");
}

/// jump-reorder (UXI-JumpPanel-2), REAL path: seed two cwd groups on the roster, then
/// call the exact methods the drop handlers invoke. `reorder_cwd_group` reorders
/// the headers (and persists the order); `reorder_session` reorders sessions
/// WITHIN a group; and a cross-cwd `reorder_session` is REFUSED — a session can
/// never be dragged into a cwd it doesn't belong in. Drives the production view
/// (the GPUI mouse-drag GESTURE that dispatches these is the runtime gap — gap
/// #2, no headless drag-dispatch seam — but the state change these methods make
/// is the real code the drop runs).
#[gpui::test]
fn jump_reorder_methods_reorder_and_gate_by_cwd(cx: &mut TestAppContext) {
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();

    let info = |sid: &str, label: &str, cwd: &str| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: std::path::PathBuf::from(cwd),
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::PermissionMode::ReadOnly,
    };
    // alpha: {a1, a2}; beta: {b1}.
    view.update(vcx, |v, _| {
        v.agent_roster.upsert(info("a1", "a-one", "/proj/alpha"));
        v.agent_roster.upsert(info("a2", "a-two", "/proj/alpha"));
        v.agent_roster.upsert(info("b1", "b-one", "/proj/beta"));
    });

    // Snapshot the ordered, grouped view the way render does.
    let snapshot = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            let g = crate::order_grouped_rows(
                crate::group_agent_rows_by_cwd(v.jump_panel_agent_rows(cx)),
                &v.jump_cwd_order,
                &v.jump_session_order,
            );
            g.into_iter()
                .map(|(k, rows)| {
                    (k, rows.into_iter().map(|(_, r)| r.label).collect::<Vec<_>>())
                })
                .collect::<Vec<_>>()
        })
    };

    // Default: alpha before beta; alpha sessions by label (a-one, a-two).
    let g = snapshot(&view, vcx);
    assert_eq!(g[0].0, "/proj/alpha");
    assert_eq!(g[1].0, "/proj/beta");
    assert_eq!(g[0].1, vec!["a-one", "a-two"]);

    // Reorder the HEADERS: drop beta onto alpha → beta first. Persisted.
    view.update(vcx, |v, cx| v.reorder_cwd_group("/proj/beta", "/proj/alpha", cx));
    let g = snapshot(&view, vcx);
    assert_eq!(g[0].0, "/proj/beta", "cwd drag reordered the group headers");
    assert!(
        view.update(vcx, |v, _| v.jump_cwd_order.first().map(|s| s == "/proj/beta").unwrap_or(false)),
        "cwd order persisted on the view"
    );

    // Reorder WITHIN alpha: drop a2 onto a1 → a-two before a-one.
    view.update(vcx, |v, cx| v.reorder_session("a2", "a1", cx));
    let g = snapshot(&view, vcx);
    let alpha = g.iter().find(|(k, _)| k == "/proj/alpha").unwrap();
    assert_eq!(alpha.1, vec!["a-two", "a-one"], "session drag reordered within the group");

    // CROSS-CWD is refused: dragging b1 (beta) onto a1 (alpha) does nothing.
    let before = view.update(vcx, |v, _| v.jump_session_order.clone());
    view.update(vcx, |v, cx| v.reorder_session("b1", "a1", cx));
    let after = view.update(vcx, |v, _| v.jump_session_order.clone());
    assert_eq!(before, after, "a session cannot be reordered into another cwd group");
    // And b1 is still under beta, not alpha.
    let g = snapshot(&view, vcx);
    assert!(g.iter().any(|(k, rows)| k == "/proj/beta" && rows.contains(&"b-one".to_string())));
    assert!(g.iter().all(|(k, rows)| k != "/proj/alpha" || !rows.contains(&"b-one".to_string())));
}

/// Unit: the jump panel groups agent-session rows under per-cwd subheaders
/// (agent-sessions-by-cwd). Sessions sharing a cwd land in one group; groups are
/// ordered by their display path (stable, alphabetized headers); and every row
/// keeps its original flat index (so its id / click listener stay stable
/// regardless of grouping).
#[test]
fn jump_panel_groups_agent_rows_by_cwd() {
    use crate::{group_agent_rows_by_cwd, AgentRow, JumpTarget};
    let row = |label: &str, cwd: &str| AgentRow {
        target: JumpTarget::Roster(label.into()),
        label: label.into(),
        cwd: std::path::PathBuf::from(cwd),
        bound: false,
        connected: true,
        awaiting: None,
    };
    // Two projects, one with two sessions; input order is by-label (a,b,c).
    let rows = vec![
        row("a", "/work/beta"),
        row("b", "/work/alpha"),
        row("c", "/work/alpha"),
    ];
    let groups = group_agent_rows_by_cwd(rows);
    assert_eq!(groups.len(), 2, "one group per distinct cwd");
    // Headers alphabetized by display path: alpha before beta.
    assert_eq!(groups[0].0, "/work/alpha");
    assert_eq!(groups[1].0, "/work/beta");
    // alpha holds b (idx 1) and c (idx 2), in incoming order, original indices kept.
    let alpha: Vec<(usize, &str)> =
        groups[0].1.iter().map(|(i, r)| (*i, r.label.as_str())).collect();
    assert_eq!(alpha, vec![(1, "b"), (2, "c")]);
    // beta holds a (idx 0).
    let beta: Vec<(usize, &str)> =
        groups[1].1.iter().map(|(i, r)| (*i, r.label.as_str())).collect();
    assert_eq!(beta, vec![(0, "a")]);
}

/// THE ACTUAL ROOT CAUSE of "/clear worksheet invisible", caught on the real
/// path — the mechanism the six paint/render-count fixes all MISSED.
///
/// The inline You-block is ONE `FlatItem::YouBlock` list item whose content is
/// driven by the COMPOSE buffer, not the transcript `edit_seq`. GPUI's
/// `ListState` caches rendered items and only re-measures one when it's spliced.
/// `reconcile_list` splices the tail on a transcript `edit_seq` move and diffs
/// `FlatKey::YouBlock` on `parked` only — so a keystroke into the You-block (which
/// bumps the *compose* seq, not the transcript seq, and doesn't change the key)
/// left the item un-spliced. GPUI repainted its stale cached element → the typed
/// char was invisible until an unrelated event (jump bar, chatbox toggle) forced a
/// splice. The fix: `build_body` hashes the active You-block's render inputs
/// (`you_block_seq`) and splices exactly that item when the hash moves.
///
/// This asserts on the SPLICE (`YOU_BLOCK_SPLICE_LABEL`), not on paint: the
/// headless harness re-renders every list item each frame, which MASKS the
/// `ListState` item-cache staleness — that mask is precisely why the prior
/// paint-based repros were falsely GREEN. The splice count is the one observable
/// that reflects the real GPUI invalidation.
///
/// NEGATIVE CONTROL (mandatory, observed): delete the `you_block_seq != …` splice
/// block in `build_body` (transcript_view.rs) and this fails RED — the count stays
/// flat at 0, i.e. the You-block item is never invalidated ⇒ the user's invisible
/// text. Restore it and it passes. Verified by commenting the block out.
#[gpui::test]
fn clear_worksheet_you_block_keystroke_splices_item(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // Rest in the exact post-/clear typeable worksheet: fresh transcript, focus on
    // the Compose (UXI-AgentTile-12 gate → inline You-block active), idle, Insert.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.editor =
                yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.close_you_block();
            c.focus = crate::AgentFocus::Compose;
            c.input_surface.compose_mut().mode = crate::EditMode::Insert;
            c.turn_phase = crate::TurnPhase::Idle;
        });
    });
    vcx.run_until_parked();

    // Sanity: we ARE in the state where a You-block item is present + active, so a
    // keystroke's staleness would actually be user-visible (non-vacuous).
    let active = view
        .update(vcx, |v, cx| v.agent_read(cx, |c| c.inline_you_block_active()))
        .unwrap_or(false);
    assert!(active, "precondition: inline You-block must be active (else nothing to keep fresh)");

    // Let the initial render settle so `last_you_block_seq` has caught up to the
    // empty block, THEN start the measurement window — so the count we read is
    // attributable to the KEYSTROKE, not the first paint.
    vcx.run_until_parked();
    crate::perf_reset(crate::YOU_BLOCK_SPLICE_LABEL);
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count(crate::YOU_BLOCK_SPLICE_LABEL);
    assert_eq!(base, 0, "a plain notify (no compose change) must NOT splice the You-block item");

    // The user types — through the REAL key handler, no `i`, no toggle.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx));
    vcx.run_until_parked();
    let after = crate::perf_render_count(crate::YOU_BLOCK_SPLICE_LABEL);

    let text = view
        .update(vcx, |v, cx| v.agent_read(cx, |c| c.input_surface.compose().text()))
        .expect("session");
    assert_eq!(text.trim(), "h", "sanity: the char landed in the compose buffer");
    assert!(
        after > base,
        "ROOT CAUSE: typing into the active You-block MUST splice its list item so GPUI \
         re-measures + repaints the new text (splice count {base} -> {after}); flat == the \
         cached-item staleness the user sees as invisible text",
    );
}

/// The jump panel can be hidden/summoned via `cmd-j` / the `?` menu
/// (jump-panel; spec-jump-panel.md). It defaults visible and renders; toggling
/// it off stops it rendering (and flips the menu label); toggling on brings it
/// back. The toggle routes through `toggle_jump_panel_impl`, the same path the
/// `cmd-j` action and the `toggle-jump-panel` menu command use.
#[gpui::test]
fn jump_panel_toggle_hides_and_summons(cx: &mut TestAppContext) {
    crate::perf_reset("jump_panel");
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    // Dismiss the startup splash — it short-circuits `render` before the panel
    // embed (wall-clock doesn't advance under `run_until_parked`).
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();

    // Defaults visible, renders, and the menu offers to hide it.
    assert!(view.update(vcx, |v, _| v.jump_panel_visible), "defaults visible");
    let rendered = crate::perf_render_count("jump_panel");
    assert!(rendered >= 1, "visible panel renders at least once");
    let menu_has = |v: &mut YaldaGpuiView, label: &str| {
        v.global_menu().iter().any(|n| n.label == label)
    };
    assert!(view.update(vcx, |v, _| menu_has(v, "hide jump panel")));

    // Hide it (via the menu command). It stops rendering; menu now offers show.
    view.update(vcx, |v, cx| v.dispatch_menu_command("toggle-jump-panel", cx));
    assert!(!view.update(vcx, |v, _| v.jump_panel_visible), "now hidden");
    let base = crate::perf_render_count("jump_panel");
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert_eq!(
        crate::perf_render_count("jump_panel"),
        base,
        "a hidden jump panel is not rendered"
    );
    assert!(view.update(vcx, |v, _| menu_has(v, "show jump panel")));

    // Summon it again — it renders once more.
    view.update(vcx, |v, cx| v.dispatch_menu_command("toggle-jump-panel", cx));
    assert!(view.update(vcx, |v, _| v.jump_panel_visible), "visible again");
    vcx.run_until_parked();
    assert!(
        crate::perf_render_count("jump_panel") > base,
        "summoned panel renders again"
    );
}

/// VERIFICATION HARNESS (#3.2 — PAINTED-BOUNDS proof of UXI-TextEditing-1). The model-level
/// guard `compose_wrapped_caret_never_below_the_fold` proves the window MATH; this
/// proves the PAINT: in a compose draft that wraps far past the box, after a real
/// layout/paint pass (`run_until_parked`) the caret's row must actually be painted
/// AND its bounds must sit inside the compose box. The virtualized compose list
/// never paints an off-screen row, so if the caret scrolled below the fold (the
/// recurring chatbox bug) the `compose-cursor-row` probe is empty and this fails —
/// the proof a state-level test can't give. This is the harness capability that
/// closes the caret-visibility regression class.
#[gpui::test]
fn compose_caret_row_painted_inside_box_when_wrapped(cx: &mut TestAppContext) {
    // boot_with_transcript dismisses the startup splash (which otherwise
    // short-circuits `render` before `render_agent` runs — wall-clock doesn't
    // advance under run_until_parked, so the splash never expires headlessly).
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    // The bottom compose BOX is the chatbox surface (default is now worksheet, whose
    // idle draft renders inline). This test targets the boxed compose's caret math.
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));

    // Seed a draft that wraps WELL beyond the 8-row box, caret left at the end.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let ed = &mut c.input_surface.compose_mut().editor;
        for i in 0..40 {
            for ch in
                format!("line {i} with plenty of words so it wraps once or twice across the box")
                    .chars()
            {
                ed.insert_char(ch);
            }
            ed.insert_char('\n');
        }
    });

    // Settle: the virtualized compose list measures item heights lazily, so the
    // authoritative scroll_to(item) lands only after the visible items are
    // measured. Drive several frames so measurement + scroll converge before we
    // probe (mirrors the doc-drag test's settle dance).
    for _ in 0..4 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let caret = crate::layout_probe_get("compose-cursor-row");
    let box_bounds =
        view.update(vcx, |v, cx| v.agent_read(cx, |c| c.input_surface.compose().bounds.get()));
    crate::layout_probe_end();

    let (_, box_y, _, box_h) =
        box_bounds.expect("compose box bounds were not captured (box did not paint)");
    // The virtualized compose list never paints an off-screen row, so a missing
    // probe == the caret scrolled BELOW THE FOLD (the exact reported bug).
    let (_, caret_y, _, caret_h) = caret.unwrap_or_else(|| {
        panic!(
            "compose cursor row was NOT painted — the caret is below the fold \
             (UXI-TextEditing-1 violated). box=[{box_y}, {}] h={box_h}",
            box_y + box_h
        )
    });

    assert!(box_h > 1.0, "compose box has no height ({box_h}) — nothing painted");
    // The caret row's TOP must be inside the box (caret glyph visible, not below
    // the fold). A genuine below-fold caret is either unpainted (handled above) or
    // off by a full row (≥18px); the small bottom tolerance only absorbs the 1px
    // borders / sub-pixel rounding (the box inner height isn't an exact row
    // multiple), NOT a real overflow.
    assert!(
        caret_y >= box_y - 1.0,
        "caret row top {caret_y} is above the compose box top {box_y}",
    );
    assert!(
        caret_y < box_y + box_h,
        "caret row top {caret_y} is at/below the box bottom {} — BELOW THE FOLD",
        box_y + box_h,
    );
    assert!(
        caret_y + caret_h <= box_y + box_h + 3.0,
        "caret row bottom {} clipped beyond the box bottom {} by more than border slack",
        caret_y + caret_h,
        box_y + box_h,
    );
}

/// VERIFICATION HARNESS (UXI-AgentTile-11, stage 2 — painted proof the two surfaces render
/// in DIFFERENT places). The chatbox is a pinned box at the window bottom
/// (`compose-box`); the worksheet's open You-block renders INLINE in the transcript
/// (`you-block`), not at the bottom. (Supersedes the UXI-AgentTile-10 flush-vs-boxed
/// geometry test, whose premise — an always-present worksheet compose box — is gone.)
#[gpui::test]
fn worksheet_renders_flush_chatbox_renders_boxed(cx: &mut TestAppContext) {
    let (view, vcx, _id, _session) = boot_with_transcript(cx);

    // Worksheet (the default) with an open You-block: the editable reply paints
    // INLINE in the transcript; there is no pinned bottom box.
    assert!(
        !view.update(vcx, |v, cx| v.agent_read(cx, |c| c.input_surface.is_chatbox()))
            .expect("bound agent session"),
        "precondition: a fresh session defaults to Worksheet"
    );
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("i"), window, cx));
    vcx.run_until_parked();
    let (_, yb_y, _, _) = probe_dirty(&view, vcx, "you-block").expect("worksheet block paints inline");
    assert!(
        probe_dirty(&view, vcx, "compose-box").is_none(),
        "the worksheet block is inline — no bottom box"
    );

    // Toggle to Chatbox: a pinned bottom box paints; the inline block is gone.
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));
    vcx.run_until_parked();
    let (_, chat_y, _, _) = probe_dirty(&view, vcx, "compose-box").expect("chatbox paints a bottom box");
    assert!(probe_dirty(&view, vcx, "you-block").is_none(), "chatbox has no inline block");

    // The inline block sits in the scrolling transcript column, ABOVE where the
    // pinned chatbox sits at the window bottom — a genuinely different placement.
    assert!(
        yb_y < chat_y + 1.0,
        "inline you-block top ({yb_y}) is at/above the pinned chatbox top ({chat_y})"
    );
}

/// Probe a painted bounds tag, DIRTYING the cached transcript first so its inner
/// elements (e.g. the inline `you-block`) actually re-paint this frame — a bare
/// root notify reuses the cached subtree and never re-runs `probe_bounds` inside
/// it. `pending_reveal_cursor` is a `TranscriptSeqs` input, so toggling it on the
/// probe frame busts the cache deterministically. Returns `None` if the tag
/// didn't paint.
fn probe_dirty(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    tag: &'static str,
) -> Option<(f32, f32, f32, f32)> {
    for _ in 0..2 {
        view.update(vcx, |v, cx| {
            if let Some(mut c) = v.agent_mut(cx) {
                c.pending_reveal_cursor = true;
            }
            cx.notify();
        });
        vcx.run_until_parked();
    }
    crate::layout_probe_begin();
    view.update(vcx, |v, cx| {
        if let Some(mut c) = v.agent_mut(cx) {
            c.pending_reveal_cursor = true;
        }
        cx.notify();
    });
    vcx.run_until_parked();
    let b = crate::layout_probe_get(tag);
    crate::layout_probe_end();
    b
}

/// Build a bare (no-modifier) key-down event for driving the REAL
/// `handle_claude_key` dispatch directly (single chars also fill `key_char`).
fn ws_bare_key(key: &str) -> gpui::KeyDownEvent {
    gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers: gpui::Modifiers::default(),
            key: key.to_string(),
            key_char: (key.chars().count() == 1).then(|| key.to_string()),
        },
        is_held: false,
    }
}

/// Boot a real view with a bound session, switched to **Worksheet** mode resting
/// in transcript navigation (UXI-AgentTile-11 default for the worksheet).
fn boot_worksheet_nav(
    cx: &mut TestAppContext,
) -> (gpui::Entity<YaldaGpuiView>, &mut gpui::VisualTestContext) {
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    // New sessions now DEFAULT to Worksheet resting in nav (stage 3 / bug-hunt-2 B3) —
    // no toggle needed. Verify that resting state.
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(!c.input_surface.is_chatbox(), "default is worksheet");
        assert_eq!(c.focus, crate::AgentFocus::Transcript, "worksheet rests in nav");
        assert!(!c.you_block_open, "no You-block until Insert");
    });
    (view, vcx)
}

/// UXI-AgentTile-14: Cmd+V with an image on the clipboard stages it as a pending
/// attachment on the compose (rather than typing garbage), base64-encoded with
/// its mime type — the payload that becomes an ACP `ContentBlock::Image`. Drives
/// the REAL key handler (`handle_claude_key` → `paste_into_compose`) against the
/// REAL test-platform clipboard.
///
/// Negative control: delete the `cb.pending_images.push(pending)` in
/// `paste_into_compose` and the staged-count assert fails RED (nothing staged).
#[gpui::test]
fn image_paste_stages_pending_attachment(cx: &mut TestAppContext) {
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    // A distinctive fake PNG byte payload on the clipboard.
    let png: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
    {
        let png = png.clone();
        view.update(vcx, |_, cx| {
            let img = gpui::Image::from_bytes(gpui::ImageFormat::Png, png);
            cx.write_to_clipboard(gpui::ClipboardItem::new_image(&img));
        });
    }
    // Cmd+V through the real key path.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_cmd_key("v"), w, cx));
    vcx.run_until_parked();

    let staged = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                c.input_surface
                    .compose()
                    .pending_images
                    .iter()
                    .map(|p| (p.mime_type.clone(), p.data.clone(), p.label.clone()))
                    .collect::<Vec<_>>()
            })
        })
        .expect("session");
    assert_eq!(staged.len(), 1, "Cmd+V with a clipboard image stages one attachment");
    assert_eq!(staged[0].0, "image/png", "mime type carried from the clipboard format");
    assert!(staged[0].2.contains("PNG"), "chip label names the format: {}", staged[0].2);
    // The base64 payload decodes back to the exact bytes the agent will read.
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&staged[0].1)
        .expect("valid base64");
    assert_eq!(decoded, png, "the staged data round-trips to the original image bytes");

    // The compose editor stayed empty — Cmd+V did NOT type the 'v' or paste junk.
    let compose_text = view
        .update(vcx, |v, cx| v.agent_read(cx, |c| c.input_surface.compose().text()))
        .expect("session");
    assert!(
        compose_text.trim().is_empty(),
        "an image paste must not put text in the compose; got {compose_text:?}"
    );
}

/// Boot a REAL worksheet session backed by an in-process test channel (NO server
/// sid) so a real `submit` takes the `channel.send()==Ok` path and drives the
/// production mid-turn transition — the seam that closes verification gap #2 for
/// the GUI half. Keep the returned controls alive (they retain `prompt_rx`).
/// Behind `test-support` (the in-process transport feature); run these with
/// `cargo test --bin yalda-gpui --features test-support`.
#[cfg(feature = "test-support")]
fn boot_worksheet_channel(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<YaldaGpuiView>,
    &mut gpui::VisualTestContext,
    crate::SessionId,
    yalda::acp_channel::TestChannelControls,
) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, None); // None sid ⇒ channel.send() path
    let (client, controls) = yalda::acp_channel::AcpChannelClient::test_connected();
    let id = view.update(vcx, |v, cx| {
        v.splash_until = None;
        let id = v.focused_bound_session().expect("bound session");
        v.with_session(id, cx, |c| c.channel = Some(client));
        cx.notify();
        id
    });
    (view, vcx, id, controls)
}

/// Open a tail You-block, type `text`, and submit through the REAL path
/// (`submit_agent` → `submit_compose` → `submit_worksheet_blocks` →
/// `channel.send()`), leaving the session in the production post-submit mid-turn
/// state — NOT a hand-set `turn_phase`.
#[cfg(feature = "test-support")]
fn worksheet_real_submit(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    text: &str,
) {
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            for ch in text.chars() {
                c.input_surface.compose_mut().editor.insert_char(ch);
            }
        });
    });
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();
}

/// UXI-AgentTile-14 (end-to-end, real submit path): a pasted image staged on the
/// compose rides a REAL worksheet submit — it reaches the channel as a
/// `PromptPayload` carrying the image attachment, the transcript records a
/// `🖼 image N (EXT)` marker for it, and the staged attachment clears after send.
/// Drives `handle_claude_key` (Cmd+V) → `submit_agent` → `submit_worksheet_blocks`
/// → `channel.send_payload` against the in-process test channel, then reads the
/// payload back off `TestChannelControls::try_recv_prompt`.
///
/// Negative controls: (a) drop `images` from the worksheet `PromptPayload` and
/// the `payload.images` assert fails; (b) remove the marker-block push and the
/// transcript-contains assert fails; (c) the `pending_images` clear rides
/// `InputSurface::new` on the reset — leave a stale vec and the cleared assert
/// fails.
#[cfg(feature = "test-support")]
#[gpui::test]
fn image_submit_sends_block_marks_transcript_and_clears(cx: &mut TestAppContext) {
    let (view, vcx, _id, controls) = boot_worksheet_channel(cx);

    // Stage a pasted PNG via the real Cmd+V path (in worksheet nav).
    let png: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 7, 7, 7, 7];
    {
        let png = png.clone();
        view.update(vcx, |_, cx| {
            let img = gpui::Image::from_bytes(gpui::ImageFormat::Png, png);
            cx.write_to_clipboard(gpui::ClipboardItem::new_image(&img));
        });
    }
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_cmd_key("v"), w, cx));
    vcx.run_until_parked();

    // Open a You-block, type text, submit through the REAL path.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            for ch in "hey".chars() {
                c.input_surface.compose_mut().editor.insert_char(ch);
            }
        });
    });
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();

    // 1. The channel received the image as a real attachment on the prompt.
    let payload = controls
        .prompt_rx
        .try_recv()
        .expect("a prompt reached the channel");
    assert_eq!(payload.text, "hey", "the typed text was sent");
    assert_eq!(
        payload.images.len(),
        1,
        "the pasted image rode the submit as an attachment"
    );
    assert_eq!(payload.images[0].mime_type, "image/png");
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&payload.images[0].data)
        .expect("valid base64");
    assert_eq!(decoded, png, "the agent receives the exact image bytes");

    // 2. The transcript records a marker for the sent image.
    let transcript = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.editor.document().full_text())
        })
        .expect("session");
    assert!(
        transcript.contains("🖼 image 1 (PNG)"),
        "transcript must mark the sent image; got:\n{transcript}"
    );

    // 3. The staged attachment cleared after a successful submit.
    let remaining = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().pending_images.len())
        })
        .expect("session");
    assert_eq!(remaining, 0, "attachments clear after a successful submit");
}

/// ACCEPTANCE (real-state, gap #2 closed): reach mid-turn through the REAL submit
/// path, then check the REPORTED `m` behavior. On `main` this is EXPECTED TO FAIL
/// (the mid-turn mark-chord exclusion from `eb6bb4c` is stranded on
/// `jump-pane-nav`, unmerged) — the failure is the mechanical proof the bug is
/// live on main. It passes once the guard fix lands on main.
#[cfg(feature = "test-support")]
#[gpui::test]
fn real_midturn_worksheet_m_types_not_marks(cx: &mut TestAppContext) {
    let (view, vcx, id, _controls) = boot_worksheet_channel(cx);
    // CONTROL: genuine (idle) transcript nav — bare `m` DOES start a mark chord.
    // (This half kills the `try_start_mark_chord` mutants the audit found surviving:
    // "return false" / "delete the `m` arm" both break this assertion.)
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("m"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.pending_mark_chord,
            Some('m'),
            "idle transcript nav: bare `m` starts a mark chord"
        );
        v.pending_mark_chord = None;
    });
    worksheet_real_submit(&view, vcx, "do the thing");
    view.update(vcx, |v, cx| {
        assert!(
            v.read_session(id, cx, |c| c.turn_phase.is_awaiting()).unwrap(),
            "real submit must start a turn (we are genuinely mid-turn)"
        );
        v.pending_mark_chord = None;
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("m"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.pending_mark_chord, None,
            "mid-turn worksheet `m` must NOT start a mark chord (it should type)"
        );
        let text = v
            .read_session(id, cx, |c| c.input_surface.compose().text())
            .unwrap();
        assert!(text.contains('m'), "mid-turn `m` types into the chatbox (got {text:?})");
    });
}

/// ACCEPTANCE (real-state, rule-7 revised): mid-turn (reached via the REAL submit)
/// with an EMPTY steering draft, the worksheet rests in nav — so `<space>` OPENS
/// the tile menu (the reported "leaders don't work mid-turn" bug). Red before the
/// `focused_in_insert_mode` empty-draft change; green after.
#[cfg(feature = "test-support")]
#[gpui::test]
fn real_midturn_worksheet_empty_draft_space_opens_menu(cx: &mut TestAppContext) {
    let (view, vcx, id, _controls) = boot_worksheet_channel(cx);
    worksheet_real_submit(&view, vcx, "go"); // submit RESETS the compose ⇒ empty draft
    view.update(vcx, |v, cx| {
        assert!(v.read_session(id, cx, |c| c.turn_phase.is_awaiting()).unwrap());
        assert!(
            v.read_session(id, cx, |c| c.input_surface.compose().text().trim().is_empty())
                .unwrap(),
            "post-submit steering draft is empty"
        );
        assert!(
            !v.focused_in_insert_mode(cx),
            "empty-draft mid-turn worksheet rests in nav ⇒ leaders active"
        );
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("space"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert!(
            matches!(v.active_overlay, crate::ActiveOverlay::Menu(_)),
            "mid-turn <space> with an empty draft OPENS the tile menu (rule 7 revised)"
        );
    });
}

/// ACCEPTANCE (real-state, rule-7 revised): once the user has TYPED a steer
/// mid-turn (non-empty draft), the keystrokes belong to the chatbox — `<space>`
/// types a space, it does NOT open a menu (so multi-word steering is unbroken).
#[cfg(feature = "test-support")]
#[gpui::test]
fn real_midturn_worksheet_typed_draft_space_is_suppressed(cx: &mut TestAppContext) {
    let (view, vcx, id, _controls) = boot_worksheet_channel(cx);
    worksheet_real_submit(&view, vcx, "go");
    // Type into the mid-turn chatbox so the draft is non-empty.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("f"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert!(
            !v.read_session(id, cx, |c| c.input_surface.compose().text().trim().is_empty())
                .unwrap(),
            "typed a char ⇒ draft non-empty"
        );
        assert!(
            v.focused_in_insert_mode(cx),
            "non-empty steer ⇒ text entry ⇒ leaders suppressed"
        );
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("space"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert!(
            matches!(v.active_overlay, crate::ActiveOverlay::None),
            "mid-turn <space> with a draft in progress must NOT open a menu"
        );
        let text = v
            .read_session(id, cx, |c| c.input_surface.compose().text())
            .unwrap();
        assert!(text.contains(' '), "the space typed into the steer (got {text:?})");
    });
}

/// A `ModelsAvailable` reply through the REAL reducer captures the advertised
/// picklist into `available_models` and syncs `agent_model` to the current
/// selection (UXI-AgentTile-16). Negative control: the list is empty before the reply.
#[gpui::test]
fn agent_reply_models_available_captures_picklist(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ModelOption, ReplyEvent};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Negative control: no models advertised yet.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        assert!(
            v.read_session(id, cx, |c| c.available_models.is_empty())
                .unwrap(),
            "picklist is empty before any ModelsAvailable reply"
        );
    });

    let opts = vec![
        ModelOption { id: "default".into(), label: "Default".into() },
        ModelOption { id: "claude-fable-5[1m]".into(), label: "Fable".into() },
        ModelOption { id: "sonnet".into(), label: "Sonnet".into() },
    ];
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::ModelsAvailable {
                    current: "sonnet".into(),
                    options: opts.clone(),
                },
            }],
            cx,
        );
        let id = v.focused_bound_session().expect("bound");
        assert_eq!(
            v.read_session(id, cx, |c| c.available_models.clone()).unwrap(),
            opts,
            "advertised picklist captured verbatim + in order"
        );
        assert_eq!(
            v.read_session(id, cx, |c| c.agent_model.clone()).unwrap(),
            Some("sonnet".to_string()),
            "current selection synced to agent_model"
        );
    });
}

/// Regression (the "still no models" bug): once a session is
/// `agent_stream_authoritative` (it has completed a turn), the legacy
/// `ReplyEvent` arm goes inert — so the picklist must ride the canonical
/// `Agent` stream to survive. Feed `ModelsAvailable` as an `Agent` notification
/// on an authoritative session and assert the switcher list populates.
#[gpui::test]
fn agent_authoritative_models_available_via_agent_stream(cx: &mut TestAppContext) {
    use yalda::acp_channel::ModelOption;
    use yalda::agent_event::AgentEventKind;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Make the session authoritative — this is what makes the legacy ReplyEvent
    // arm inert, the exact condition that dropped the picklist in the field.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| c.agent_stream_authoritative = true);
    });

    let opts = vec![
        ModelOption { id: "default".into(), label: "Default".into() },
        ModelOption { id: "claude-fable-5[1m]".into(), label: "Fable".into() },
        ModelOption { id: "sonnet".into(), label: "Sonnet".into() },
    ];
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note(
                "S1",
                1,
                1,
                1,
                AgentEventKind::ModelsAvailable {
                    current: "default".into(),
                    options: opts.clone(),
                },
            )],
            cx,
        );
        let id = v.focused_bound_session().expect("bound");
        assert_eq!(
            v.read_session(id, cx, |c| c.available_models.clone()).unwrap(),
            opts,
            "authoritative session captures the picklist via the Agent stream"
        );
        assert_eq!(
            v.read_session(id, cx, |c| c.agent_model.clone()).unwrap(),
            Some("default".to_string()),
            "current selection synced via the Agent stream"
        );
    });
}

/// The agent tile menu grows a "switch model" submenu whose children are the
/// advertised models — the current one marked ✓, each dispatching
/// `set-model:<id>` (UXI-AgentTile-16). Negative control: no submenu before any model
/// is advertised.
#[gpui::test]
fn agent_menu_lists_advertised_models_and_marks_current(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ModelOption, ReplyEvent};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Before models are advertised the "switch model" entry is still present
    // (discoverability) but drills into a disabled placeholder, not a picklist.
    view.update(vcx, |v, cx| {
        let menu = v.agent_local_menu_dynamic(cx);
        let sub = menu
            .iter()
            .find(|n| n.label == "switch model")
            .expect("switch-model entry always present for discoverability");
        let crate::MenuAction::Submenu(children) = &sub.action else {
            panic!("switch model is a submenu");
        };
        assert!(
            children.iter().all(|c| matches!(c.action, crate::MenuAction::Label(_))),
            "pre-advertise: only a placeholder label, no set-model commands"
        );
    });

    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::ModelsAvailable {
                    current: "sonnet".into(),
                    options: vec![
                        ModelOption { id: "default".into(), label: "Default".into() },
                        ModelOption { id: "sonnet".into(), label: "Sonnet".into() },
                    ],
                },
            }],
            cx,
        );
        let menu = v.agent_local_menu_dynamic(cx);
        let sub = menu
            .iter()
            .find(|n| n.label == "switch model")
            .expect("switch-model submenu present once models are advertised");
        let crate::MenuAction::Submenu(children) = &sub.action else {
            panic!("switch model is a submenu");
        };
        let labels: Vec<&str> = children.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"Sonnet ✓"), "current model marked: {labels:?}");
        assert!(labels.contains(&"Default"), "other model unmarked: {labels:?}");
        let cmds: Vec<&str> = children
            .iter()
            .filter_map(|c| match &c.action {
                crate::MenuAction::Command(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            cmds.contains(&"set-model:sonnet") && cmds.contains(&"set-model:default"),
            "each child dispatches set-model:<id>: {cmds:?}"
        );
    });
}

/// The REAL switch path: `set_agent_model` (what the `set-model:<id>` menu
/// command invokes) drives the session's channel with a `session/set_config_option`
/// carrying the chosen model id. Direct-spawn channel (no server sid) so the
/// request is observable on `TestChannelControls`. Negative control: nothing is
/// enqueued until `set_agent_model` runs.
#[cfg(feature = "test-support")]
#[gpui::test]
fn set_agent_model_issues_set_config_on_channel(cx: &mut TestAppContext) {
    let (view, vcx, _id, controls) = boot_worksheet_channel(cx);
    assert!(
        controls.try_recv_set_model().is_none(),
        "no model switch enqueued before set_agent_model"
    );
    view.update(vcx, |v, cx| v.set_agent_model("sonnet".to_string(), cx));
    vcx.run_until_parked();
    assert_eq!(
        controls.try_recv_set_model(),
        Some("sonnet".to_string()),
        "set_agent_model forwards the model id to the channel's set_model"
    );
}

/// The bare mark chord fires for BOTH `m` AND `'` in idle transcript nav. Mutation
/// testing found the `'` arm of `try_start_mark_chord` untested (only `m` was
/// covered); this pins it (deleting the arm ⇒ `pending_mark_chord` stays None ⇒
/// this fails).
#[gpui::test]
fn mark_chord_fires_for_m_and_apostrophe_in_idle_nav(cx: &mut TestAppContext) {
    for (key, expect) in [("m", 'm'), ("'", '\'')] {
        let (view, vcx) = boot_worksheet_nav(cx);
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(key), w, cx));
        vcx.run_until_parked();
        view.update(vcx, |v, _| {
            assert_eq!(
                v.pending_mark_chord,
                Some(expect),
                "idle transcript nav: bare `{key}` starts a mark chord"
            );
        });
    }
}

/// `focused_in_insert_mode` truth table for the agent tile — the gate that decides
/// whether the `<space>`/`.`/`?` leaders fire. Mutation testing found its `==`
/// (focus is Compose) and `||` (compose-insert OR mid-turn-steer) untested. Pin
/// both: a focused compose in Insert IS text entry (leaders suppressed) regardless
/// of turn phase; idle transcript nav is NOT (leaders fire).
#[gpui::test]
fn focused_in_insert_mode_agent_tile_gate(cx: &mut TestAppContext) {
    // Idle transcript nav ⇒ NOT text entry (leaders must fire).
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update(vcx, |v, cx| {
        assert!(
            !v.focused_in_insert_mode(cx),
            "idle worksheet nav (focus=Transcript) is navigation, not text entry"
        );
    });
    // An EMPTY worksheet block focused in Insert is NOT text entry — the leaders must
    // still open the menu (empty-draft heuristic), even though you can type into it.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.focus = crate::AgentFocus::Compose;
            c.input_surface.compose_mut().mode = crate::EditMode::Insert;
        });
        assert!(
            !v.focused_in_insert_mode(cx),
            "empty worksheet block (focus=Compose, Insert) ⇒ leaders still fire"
        );
    });
    // Once the draft is NON-empty ⇒ IS text entry (leaders suppressed). Non-empty +
    // Insert exercises the `compose_insert` clause, killing the `==` and `||` mutants.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.input_surface.compose_mut().editor.insert_char('x');
        });
        assert!(
            v.focused_in_insert_mode(cx),
            "focus=Compose + Insert + non-empty draft ⇒ text entry ⇒ leaders suppressed"
        );
    });
}

/// THE REPORTED BUG, end-to-end: after `/clear` the worksheet must be immediately
/// TYPEABLE — the user just cleared to write, and types WITHOUT pressing `i`; the
/// text must land in the compose and be visible. (Prior "fixes" kept the block open
/// but rested it in NAV, so the keystrokes were eaten as navigation and nothing
/// appeared — the "can't see anything I'm typing after clear" bug. This test drives
/// the real key handler with NO `i`.)
#[gpui::test]
fn worksheet_typing_after_clear_is_visible_without_pressing_i(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // Land in the post-/clear resting state: a settled fresh (empty) worksheet.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.editor =
                yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.close_you_block();
            c.focus = crate::AgentFocus::Transcript;
            c.settle_input_focus();
        });
    });
    // The user types immediately — NO `i` — because they just cleared to write.
    for ch in ["h", "e", "l", "l", "o"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    vcx.run_until_parked();
    let text = view
        .update(vcx, |v, cx| v.agent_read(cx, |c| c.input_surface.compose().text()))
        .unwrap();
    assert_eq!(
        text.trim(),
        "hello",
        "after /clear, typing (no `i`) must land in the compose + be visible (got {text:?})"
    );
}

// ============================================================================
// /clear worksheet-invisible reproduction (docs/projects/clear-worksheet-invisible)
//
// The recurring bug: after `/clear` in worksheet mode, typed text is INVISIBLE
// until a chatbox toggle. The text IS in the buffer — it just doesn't REPAINT.
// Prior tests asserted the BUFFER (`compose().text() == "hello"`) or a hand-built
// GATE state (`inline_you_block_active()`), never that a keystroke actually
// RE-RENDERS the cached transcript. This measures the real mechanism: typing must
// bust the cached transcript (render count advances). Flat count = invisible = bug.
// ============================================================================

/// REPRO A — the SIMULATED post-clear resting state (same setup as the legacy
/// `worksheet_typing_after_clear_is_visible_without_pressing_i`, which asserted
/// the buffer). Here we assert the real invalidation: a keystroke after `/clear`
/// must RE-RENDER the cached transcript (so the You-block repaints with the text).
#[gpui::test]
fn repro_clear_worksheet_typed_text_repaints_simulated(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // Land in the settled post-/clear resting state.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.editor =
                yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.close_you_block();
            c.focus = crate::AgentFocus::Transcript;
            c.settle_input_focus();
        });
    });
    vcx.run_until_parked();
    crate::perf_reset("transcript");
    // Force a clean baseline render, then measure the delta the keystroke causes.
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // The user types — NO `i`, NO mode toggle — via the REAL key handler.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx));
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");

    let (active, text) = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                (c.inline_you_block_active(), c.input_surface.compose().text())
            })
        })
        .expect("session");
    assert_eq!(text.trim(), "h", "sanity: the char landed in the compose buffer");
    assert!(
        active,
        "inline You-block must be active after /clear so the keystroke renders"
    );
    assert!(
        after > base,
        "a keystroke after /clear MUST re-render the cached transcript so the typed \
         text repaints (render count {base} -> {after}); flat == the invisible-text bug"
    );
}

/// REPRO B — THE REAL REDUCER PATH. After `/clear` the fresh server session's
/// channel opens, which the reducer sees as a `ChannelOpened` that rebaselines
/// the generation → `reset_for_replay` → `settle`. That is the exact step no
/// prior `/clear` test ran BEFORE the user types. We feed it to the ALREADY-bound
/// session (the bind/attach dance can't run headlessly without a server — the
/// deferred `spawn_attach_sessions` unbinds with no server; that's gap #2, not the
/// bug), then a REAL keystroke, and assert the cached transcript RE-RENDERS.
#[gpui::test]
fn repro_clear_worksheet_typed_text_repaints_real_path(cx: &mut TestAppContext) {
    use yalda::agent_event::AgentEventKind as K;

    let (view, vcx) = boot_worksheet_nav(cx); // bound sid "S1", splash dismissed
    // Post-/clear resting worksheet, exactly as `settle_input_focus` leaves it.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.editor =
                yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.close_you_block();
            c.focus = crate::AgentFocus::Transcript;
            c.settle_input_focus();
        });
    });
    vcx.run_until_parked();

    // The fresh channel opens: ChannelOpened rebaselines gen → reset_for_replay →
    // settle. THE UNTESTED TAIL — driven through the REAL reducer.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note("S1", 1, 1, 0, K::ChannelOpened { resumed: false })],
            cx,
        );
    });
    vcx.run_until_parked();

    crate::perf_reset("transcript");
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // The user types — NO `i`, NO mode toggle — through the REAL key handler.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx));
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");

    let (active, text, awaiting, open, chatbox) = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                (
                    c.inline_you_block_active(),
                    c.input_surface.compose().text(),
                    c.turn_phase.is_awaiting(),
                    c.you_block_open,
                    c.input_surface.is_chatbox(),
                )
            })
        })
        .expect("session still bound");
    assert_eq!(text.trim(), "h", "sanity: the char landed in the compose buffer");
    assert!(
        active,
        "REAL PATH: inline You-block inactive after /clear + ChannelOpened replay \
         (you_block_open={open}, awaiting={awaiting}, chatbox={chatbox}) — \
         keystrokes won't render",
    );
    assert!(
        after > base,
        "REAL PATH: a keystroke after /clear MUST re-render the cached transcript so \
         the typed text repaints (render count {base} -> {after}); flat == the \
         invisible-text bug the user reports",
    );
}

/// THE BUG, on the real render path (the one that matters). "The hole" is the
/// state the `/clear` symptom reduces to (Fable's analysis, spec.md §1):
/// `focus=Compose ∧ you_block_open=false ∧ idle ∧ worksheet` — keystrokes route
/// to the compose (routing keys on `focus`, agent_ui.rs:4231) but nothing paints
/// it (painting keys on `you_block_open` via `inline_you_block_active`, and the
/// bottom box only shows when chatbox/awaiting — screens.rs:1188). This drives
/// the REAL key handler + REAL render and asserts the typed char both busts the
/// cached transcript (render count) AND paints an inline You-block. The hole
/// precondition is set directly because it is an invariant VIOLATION with no
/// single named producer — the FIX (deriving the gate from `focus`) heals it
/// regardless of producer.
///
/// NEGATIVE CONTROL (mandatory): revert `inline_you_block_active` to
/// `you_block_open && ...` and this fails RED — flat render count AND no
/// `you-block` paint — the exact user symptom.
#[gpui::test]
fn clear_worksheet_hole_types_and_paints(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // Enter the hole: focus=Compose, block CLOSED, idle, worksheet, Insert.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.editor =
                yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.close_you_block(); // you_block_open = false
            c.focus = crate::AgentFocus::Compose; // routed-to but (pre-fix) un-painted
            c.input_surface.compose_mut().mode = crate::EditMode::Insert;
            c.turn_phase = crate::TurnPhase::Idle;
        });
    });
    vcx.run_until_parked();

    // Pre-assert the FULL four-part hole so a future refactor can't make this test
    // vacuous (critique axis 5.1).
    let (focus_compose, open, awaiting, chatbox) = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                (
                    c.focus == crate::AgentFocus::Compose,
                    c.you_block_open,
                    c.turn_phase.is_awaiting(),
                    c.input_surface.is_chatbox(),
                )
            })
        })
        .expect("session");
    assert!(
        focus_compose && !open && !awaiting && !chatbox,
        "precondition: must be in the hole (focus=Compose, block closed, idle, worksheet) \
         got (focus_compose,open,awaiting,chatbox)=({focus_compose},{open},{awaiting},{chatbox})"
    );

    crate::perf_reset("transcript");
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // REAL typing + REAL render, with the paint probe active so the keystroke's
    // re-render (if any) is captured.
    crate::layout_probe_begin();
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx));
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");
    let you_block = crate::layout_probe_get("you-block");
    let viewport = crate::layout_probe_get("transcript-viewport");
    crate::layout_probe_end();

    let text = view
        .update(vcx, |v, cx| v.agent_read(cx, |c| c.input_surface.compose().text()))
        .expect("session");
    assert_eq!(text.trim(), "h", "sanity: the char landed in the compose buffer");
    // The assertions the six prior fixes never made — RENDER + PAINT, not buffer:
    assert!(
        after > base,
        "typing in the hole MUST bust the cached transcript (render count {base} -> {after}); \
         flat == the invisible-text bug",
    );
    let (_, by, _, bh) = you_block
        .expect("typing in the hole MUST paint an inline You-block (invisible-text bug)");
    let (_, vy, _, vh) = viewport.expect("transcript viewport did not paint");
    // The block must paint INSIDE the visible viewport — a block painted off-screen
    // would be just as invisible (non-vacuous paint, critique axis 5.2).
    assert!(
        by >= vy - 0.5 && by + bh <= vy + vh + 0.5,
        "the You-block [{by}, {}] must lie inside the transcript viewport [{vy}, {}]",
        by + bh,
        vy + vh,
    );
}

/// REPRO C — the FRESH TranscriptView lifecycle. `clear_agent_session` does
/// `self.transcript_views.remove(&id)` and rebinds to a new session, so a BRAND
/// NEW `TranscriptView` (with `last_rendered = default`) is created and must
/// repaint on the first keystroke. Reproduce that: settle, ChannelOpened, then
/// DROP the transcript view (as clear does), let it re-create on a render, then
/// type — and assert the fresh view re-renders.
#[gpui::test]
fn repro_clear_worksheet_typed_text_repaints_fresh_transcript_view(cx: &mut TestAppContext) {
    use yalda::agent_event::AgentEventKind as K;
    let (view, vcx) = boot_worksheet_nav(cx);
    let id = view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            c.editor =
                yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.close_you_block();
            c.focus = crate::AgentFocus::Transcript;
            c.settle_input_focus();
        });
        id
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![agent_note("S1", 1, 1, 0, K::ChannelOpened { resumed: false })],
            cx,
        );
    });
    vcx.run_until_parked();

    // Drop the TranscriptView (what clear_agent_session does via transcript_views.
    // remove) so the next render builds a FRESH one with a default watermark.
    view.update(vcx, |v, _| {
        v.transcript_views.remove(&id);
    });
    crate::perf_reset("transcript");
    // One render re-creates + first-renders the fresh view (stamps last_rendered).
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx));
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");

    let (active, text) = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| (c.inline_you_block_active(), c.input_surface.compose().text()))
        })
        .expect("session");
    assert_eq!(text.trim(), "h", "sanity: the char landed in the compose buffer");
    assert!(active, "inline You-block active");
    assert!(
        after > base,
        "FRESH VIEW: a keystroke must re-render the newly-created transcript view \
         (render count {base} -> {after}); flat == invisible-text bug",
    );
}

/// THE full real `/clear` sequence, SERVER branch, end-to-end — the seam no prior
/// test composed. Each earlier clear test covered ONE hop (hand-built "the hole",
/// or dropped/re-created the TV for the SAME id, or drove `apply_open_agent_resolution`
/// but asserted state only, or spliced over a hand-built state) — none drove
/// `clear_agent_session` (server branch) → async `Created` bind → keystroke → PAINT.
/// That gap is exactly where the 7×-recurring "worksheet invisible until I click"
/// bug hid: green tests, broken app. This forces the real client/server `/clear`
/// branch via the `FORCE_SERVER_CLEAR_BRANCH` seam (no live server needed —
/// `spawn_create_agent_session` bails, leaving the placeholder mid-open), drives the
/// REAL async completion, then a REAL keystroke, and asserts the typed char PAINTS an
/// inline You-block inside the transcript viewport (UXI-AgentTile-12, cached-view-swap arm).
///
/// ROOT CAUSE this pins: `/clear` drops the old session's `TranscriptView` and the
/// rebind creates a new one that GPUI hands the SAME entity slot; embedded at the same
/// tree position it inherits the dropped view's stale cached prepaint AND — never
/// painted into the committed dispatch tree — its self-notifies are dropped by
/// `mark_view_dirty` (empty `view_path`). It FREEZES: typed text never repaints until a
/// click forces a refresh. Fix: `transcript_view_for` defers a full window refresh when
/// it CREATES a view, painting it fresh into the dispatch tree.
///
/// Negative control (mandatory, observed RED): comment out the
/// `cx.defer(|app| app.refresh_windows())` in `transcript_view_for` → after_r/after_s
/// stay 0 and `you-block` never paints — the exact "invisible until I click" symptom,
/// caught on the FULL real path for the first time. (A prior control also holds: revert
/// the You-block splice in `transcript_view.rs` → the splice counter stays flat.)
#[gpui::test]
fn real_clear_server_branch_then_type_paints(cx: &mut TestAppContext) {
    // HERMETIC construction (session_server = None) so the forced server branch's
    // `spawn_create_agent_session` bails and leaves the placeholder mid-open — a
    // live dev-box server would otherwise complete the round-trip and consume the
    // token before we can drive the resolution ourselves.
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));
    let old_session = view.update(vcx, |v, cx| {
        v.splash_until = None;
        let id = v.focused_bound_session().expect("bound session");
        let ent = v.session_entity(id).expect("session entity");
        cx.notify();
        ent
    });
    vcx.run_until_parked();
    // Give the OLD session real history so the clear is a genuine reset (non-vacuous).
    old_session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "old turn one\nold turn two\n");
        cx.notify();
    });
    vcx.run_until_parked();

    // CONTROL (non-vacuous guard): typing into the BOOT session BEFORE any clear
    // paints the you-block — proving this hermetic harness renders the transcript at
    // all, so the post-clear flatness below is the BUG, not a dead window.
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().unwrap();
        v.with_session(id, cx, |c| {
            c.editor =
                yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
            c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
            c.settle_input_focus();
        });
    });
    vcx.run_until_parked();
    crate::perf_reset("transcript");
    crate::layout_probe_begin();
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("x"), w, cx));
    vcx.run_until_parked();
    let ctrl_r = crate::perf_render_count("transcript");
    let ctrl_yb = crate::layout_probe_get("you-block");
    crate::layout_probe_end();
    assert!(
        ctrl_r > 0 && ctrl_yb.is_some(),
        "control: the pre-clear boot transcript must render + paint (else the test is vacuous)"
    );

    // REAL `/clear`, forced down the SERVER branch. `spawn_create_agent_session`
    // bails (no live server) but leaves the placeholder bound with a
    // `pending_open_token` — exactly the real mid-open state.
    view.update(vcx, |v, cx| {
        crate::with_server_clear_branch(|| v.clear_agent_session(cx));
    });
    vcx.run_until_parked();

    // The placeholder tile carries the token `/clear` minted for the async round-trip.
    let token = view
        .update(vcx, |v, _| v.agent_tile().and_then(|t| t.pending_open_token))
        .expect("clear left a pending open token on the placeholder tile");

    // REAL async completion: the server round-trip binds the fresh sid + re-settles.
    view.update(vcx, |v, cx| {
        v.apply_open_agent_resolution(
            token,
            crate::OpenResolution::Created {
                sid: "S-fresh".into(),
                acp_id: None,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            },
            cx,
        );
    });
    vcx.run_until_parked();

    // Precondition: the typeable idle worksheet the user faces post-clear (non-vacuous).
    let (active, focus) = view
        .update(vcx, |v, cx| v.agent_read(cx, |c| (c.inline_you_block_active(), c.focus)))
        .expect("bound after resolution");
    assert!(
        active && focus == crate::AgentFocus::Compose,
        "post-clear worksheet must be typeable inline (active={active}, focus={focus:?})"
    );

    crate::perf_reset("transcript");
    crate::perf_reset(crate::YOU_BLOCK_SPLICE_LABEL);
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base_r = crate::perf_render_count("transcript");
    let base_s = crate::perf_render_count(crate::YOU_BLOCK_SPLICE_LABEL);

    // REAL keystroke, through the REAL key handler, paint probe active.
    crate::layout_probe_begin();
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx));
    vcx.run_until_parked();
    let after_r = crate::perf_render_count("transcript");
    let after_s = crate::perf_render_count(crate::YOU_BLOCK_SPLICE_LABEL);
    let you_block = crate::layout_probe_get("you-block");
    let viewport = crate::layout_probe_get("transcript-viewport");
    crate::layout_probe_end();

    let text = view
        .update(vcx, |v, cx| v.agent_read(cx, |c| c.input_surface.compose().text()))
        .expect("session");
    assert_eq!(text.trim(), "h", "sanity: the char landed in the compose buffer");
    assert!(
        after_r > base_r,
        "REAL PATH: typing after /clear MUST bust the cached transcript ({base_r} -> {after_r}); \
         flat == the invisible-text bug",
    );
    assert!(
        after_s > base_s,
        "REAL PATH: typing after /clear MUST splice the You-block item ({base_s} -> {after_s}); \
         flat == the ListState cached-item staleness the user sees as invisible text",
    );
    let (_, by, _, bh) = you_block.expect("typed char MUST paint an inline You-block");
    let (_, vy, _, vh) = viewport.expect("transcript viewport did not paint");
    assert!(
        by >= vy - 0.5 && by + bh <= vy + vh + 0.5,
        "the You-block [{by}, {}] must lie inside the transcript viewport [{vy}, {}]",
        by + bh,
        vy + vh,
    );
}

/// `focused_in_insert_mode` for the raw EDIT view (`App::Buffer::Editing`): Insert IS
/// text entry (leaders suppressed); Normal is navigation (leaders fire). Kills the
/// `e.mode == Insert` mutant surviving in this arm.
#[gpui::test]
fn focused_in_insert_mode_edit_view_arm(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    view.update(vcx, |v, _| v.test_open_edit("hello\nworld\n"));
    let set_mode_read = |view: &gpui::Entity<YaldaGpuiView>,
                         vcx: &mut gpui::VisualTestContext,
                         m: crate::EditMode|
     -> bool {
        view.update(vcx, |v, cx| {
            if let Some(crate::App::Buffer(crate::BufferApp::Editing(e))) =
                v.workspace.focused_content_mut()
            {
                e.mode = m;
            }
            v.focused_in_insert_mode(cx)
        })
    };
    assert!(
        !set_mode_read(&view, vcx, crate::EditMode::Normal),
        "edit view in Normal is navigation (leaders fire)"
    );
    assert!(
        set_mode_read(&view, vcx, crate::EditMode::Insert),
        "edit view in Insert IS text entry (leaders suppressed)"
    );
}

/// Undo restores the caret to the edit's recorded column, NOT a stale sticky
/// column left over from an earlier j/k run. `undo`/`redo` wrote `cursor.col`
/// directly, so `clamp_cursor_col`'s `desired_col.unwrap_or` then overrode it.
/// Drives the REAL `handle_edit_key` path. NEGATIVE CONTROL: change `set_col`
/// back to a bare `self.cursor.col = col` in `EditorView::undo` → caret lands at
/// col 6, not col 0.
#[gpui::test]
fn undo_restores_recorded_column_not_sticky(cx: &mut TestAppContext) {
    use crate::EditOps;
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    view.update(vcx, |v, _| v.test_open_edit("abcdefghij\nkl\n"));
    view.update(vcx, |v, _| {
        v.edit_mut().unwrap().mode = crate::EditMode::Normal;
        v.edit_mut().unwrap().editor.cursor_set(0, 0);
    });
    let key = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext, k: &str| {
        view.update_in(vcx, |v, w, cx| v.handle_edit_key(&ws_bare_key(k), w, cx));
    };
    // Real insert edit at col 0 (undo group recorded there), then build a HIGH
    // sticky column via a horizontal run + a vertical down/up, then undo.
    key(&view, vcx, "i"); // insert mode, begin_insert at (0,0)
    key(&view, vcx, "Z"); // type 'Z' → "Zabcdefghij", caret (0,1)
    key(&view, vcx, "escape"); // end_insert; caret steps back to (0,0)
    for _ in 0..6 {
        key(&view, vcx, "l"); // → col 6 (clears desired_col each step)
    }
    key(&view, vcx, "j"); // down to short line → desired_col becomes 6
    key(&view, vcx, "k"); // back up → caret at (0,6), desired_col still 6
    key(&view, vcx, "u"); // undo the insert (recorded at col 0)
    let cur = view.update(vcx, |v, _| v.edit_mut().unwrap().editor.cursor());
    assert_eq!(
        (cur.line, cur.col),
        (0, 0),
        "undo restores the edit's column (0), not the stale sticky column (6)"
    );
}

/// A numeric count prefix repeats a Normal-mode motion: `10j` moves ten lines,
/// not one. The count used to be taken-and-discarded for every motion except
/// gg/G. Drives the REAL `handle_edit_key` path (digits accumulate in the
/// KeybindManager, then `j` fires). NEGATIVE CONTROL: revert the `for _ in 0..n`
/// loop in the `move-down` arm → the caret lands on line 1, not line 10.
#[gpui::test]
fn count_prefix_repeats_normal_motion(cx: &mut TestAppContext) {
    use crate::EditOps;
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    let text = (0..20).map(|i| format!("line {i}\n")).collect::<String>();
    view.update(vcx, |v, _| v.test_open_edit(&text));
    // Drop to Normal (test_open_edit rests in Insert).
    view.update(vcx, |v, _| {
        v.edit_mut().unwrap().mode = crate::EditMode::Normal;
    });
    for k in ["1", "0", "j"] {
        view.update_in(vcx, |v, w, cx| v.handle_edit_key(&ws_bare_key(k), w, cx));
    }
    let line = view.update(vcx, |v, _| v.edit_mut().unwrap().editor.cursor().line);
    assert_eq!(line, 10, "`10j` moves the caret ten lines down");
}

/// The Normal-mode block caret lands ON the char under the cursor, even when the
/// cursor sits exactly on a word start (a token boundary — where `w`/`b` land).
/// The old `<=` predicate handed the boundary to the PRECEDING token, drawing a
/// blank caret box before the word instead of highlighting its first char.
/// This is the decision function `build_wrapped_line` runs for every cursor row.
/// NEGATIVE CONTROL: change `<` back to `<=` in `caret_token_split` → the
/// boundary assertion below flips from token 2 to token 1.
#[gpui::test]
fn caret_token_split_lands_on_word_start(_cx: &mut TestAppContext) {
    // "foo bar" → tokens ["foo"(3), " "(1), "bar"(3)]. Cursor col 4 = the 'b'.
    let lens = [3usize, 1, 3];
    assert_eq!(
        crate::caret_token_split(&lens, 4),
        Some((2, 0)),
        "caret on a word start is owned by that word's token at split 0 (the 'b'), \
         not a blank box after the space"
    );
    // Mid-token still resolves inside the token.
    assert_eq!(crate::caret_token_split(&lens, 1), Some((0, 1)), "col 1 = 2nd char of 'foo'");
    // On the last char (Normal max) → owned by the last token.
    assert_eq!(crate::caret_token_split(&lens, 6), Some((2, 2)), "col 6 = 'r' of 'bar'");
    // Past the last char (EOL beam) → no owner, caller draws a trailing caret.
    assert_eq!(crate::caret_token_split(&lens, 7), None, "col 7 = EOL, trailing caret");
    // The space between words is owned by the space token, not the word after.
    assert_eq!(crate::caret_token_split(&lens, 3), Some((1, 0)), "col 3 = the space itself");
}

/// Build a Cmd-modified (platform) key-down event — for verifying that unbound
/// Cmd chords do NOT reach the text buffer.
fn ws_cmd_key(key: &str) -> gpui::KeyDownEvent {
    gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers: gpui::Modifiers {
                platform: true,
                ..Default::default()
            },
            key: key.to_string(),
            key_char: (key.chars().count() == 1).then(|| key.to_string()),
        },
        is_held: false,
    }
}

/// Arrow keys / Home / End / forward-Delete move the caret in Insert mode on the
/// BUFFER edit view. These used to fall through `dispatch_insert_core`'s `_ => {}`
/// arm and silently no-op — the "arrows are dead in the editor" bug. Drives the
/// REAL `handle_edit_key` dispatch. NEGATIVE CONTROL: delete the `Key::Left/Right/
/// Home/End/Delete` arms and every assertion below the first fails.
#[gpui::test]
fn edit_view_insert_arrows_move_caret_and_delete(cx: &mut TestAppContext) {
    use crate::EditOps;
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    view.update(vcx, |v, _| v.test_open_edit("hello world\n"));
    let key = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext, k: &str| {
        view.update_in(vcx, |v, w, cx| v.handle_edit_key(&ws_bare_key(k), w, cx));
    };
    let col = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| -> usize {
        view.update(vcx, |v, _| v.edit_mut().unwrap().editor.cursor().col)
    };
    key(&view, vcx, "right");
    key(&view, vcx, "right");
    key(&view, vcx, "right");
    assert_eq!(col(&view, vcx), 3, "three Right presses move the caret to col 3");
    key(&view, vcx, "end");
    assert_eq!(col(&view, vcx), 11, "End moves to the line end");
    key(&view, vcx, "home");
    assert_eq!(col(&view, vcx), 0, "Home moves to col 0");
    key(&view, vcx, "delete");
    let text = view.update(vcx, |v, _| v.edit_mut().unwrap().editor.line_text_at_cursor());
    assert_eq!(
        text.trim_end(),
        "ello world",
        "forward-Delete at col 0 removes the char under the caret"
    );
}

/// The SAME `dispatch_insert_core` arrow arms make caret motion work in the AGENT
/// compose (the message box). Drives the REAL `handle_claude_key` path so the
/// unification is verified on both surfaces. NEGATIVE CONTROL: without the
/// `Key::Left` arm the caret stays at col 5.
#[gpui::test]
fn compose_insert_arrows_move_caret(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // `i` opens the tail You-block in Insert; then type "hello" through the real
    // dispatch so the caret advances char-by-char.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    for ch in ["h", "e", "l", "l", "o"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    let col = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| -> usize {
        view.update(vcx, |v, cx| {
            let id = v.focused_bound_session().expect("bound");
            v.read_session(id, cx, |c| c.input_surface.compose().editor.cursor().col)
                .expect("session")
        })
    };
    assert_eq!(col(&view, vcx), 5, "typing 'hello' leaves the caret at col 5");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("left"), w, cx));
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("left"), w, cx));
    assert_eq!(col(&view, vcx), 3, "two Left presses move the compose caret to col 3");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("home"), w, cx));
    assert_eq!(col(&view, vcx), 0, "Home moves the compose caret to col 0");
}

/// An unbound Cmd chord must NOT type its bare letter into the buffer (cmd-s /
/// cmd-z reflexes) nor fire the letter's vim action. Drives the REAL
/// `handle_edit_key`. NEGATIVE CONTROL: drop the `platform` mapping in
/// `keystroke_to_keypress` (or the PLATFORM guards) → the buffer gains a 'g'.
#[gpui::test]
fn cmd_chord_does_not_type_into_edit_buffer(cx: &mut TestAppContext) {
    use crate::EditOps;
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let fh = cx.focus_handle();
        fh.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            fh,
        )
    });
    vcx.run_until_parked();
    view.update(vcx, |v, _| v.test_open_edit("hi\n"));
    // Insert mode: cmd-g must not insert a 'g'.
    view.update_in(vcx, |v, w, cx| v.handle_edit_key(&ws_cmd_key("g"), w, cx));
    let text = view.update(vcx, |v, _| v.edit_mut().unwrap().editor.line_text_at_cursor());
    assert_eq!(text.trim_end(), "hi", "cmd-g inserts nothing in Insert mode");
    // Normal mode: cmd-a must not run `insert-after` (which would flip to Insert).
    view.update(vcx, |v, _| {
        v.edit_mut().unwrap().mode = crate::EditMode::Normal;
    });
    view.update_in(vcx, |v, w, cx| v.handle_edit_key(&ws_cmd_key("a"), w, cx));
    let mode = view.update(vcx, |v, _| v.edit_mut().unwrap().mode);
    assert_eq!(mode, crate::EditMode::Normal, "cmd-a does not fire insert-after");
}

/// `focused_in_insert_mode` for the file BROWSER (`App::Buffer::Picking`): filter mode
/// IS text entry (leaders suppressed); idle is navigation. Kills the `filter_mode ||
/// rename.is_some()` mutant surviving in this arm (filter-only ≠ AND of both).
#[gpui::test]
fn focused_in_insert_mode_browser_arm(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let set_filter_read =
        |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext, filter: bool| -> bool {
            view.update(vcx, |v, cx| {
                if let Some(b) = v.browser_mut() {
                    b.fb.filter_mode = filter;
                    b.fb.rename = None;
                }
                v.focused_in_insert_mode(cx)
            })
        };
    assert!(
        !set_filter_read(&view, vcx, false),
        "idle browser is navigation (leaders fire)"
    );
    assert!(
        set_filter_read(&view, vcx, true),
        "browser filter mode IS text entry (leaders suppressed) — filter alone, no rename"
    );
}

/// UXI-AgentTile-11 rules 1–3: in the worksheet, navigation is free (no compose chrome);
/// an Insert-entry key (`i`) opens a You-block (compose focus + Insert); leaving
/// Insert with NO non-whitespace text DISCARDS it — the transcript is
/// byte-identical to before and no chrome remains.
#[gpui::test]
fn worksheet_insert_opens_and_empty_esc_discards_you_block(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);

    let seq_before = view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.document().edit_seq()
    });

    // `i` opens a You-block.
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("i"), window, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "i opens a You-block");
        assert_eq!(c.focus, crate::AgentFocus::Compose, "focus moves to the block");
        assert_eq!(
            c.input_surface.compose().mode,
            crate::EditMode::Insert,
            "the block is in Insert"
        );
    });

    // Type only whitespace, then Esc Esc → drop to Normal, then leave → discard
    // (layered Esc: 1st = Normal in the block, 2nd = leave; empty ⇒ discard).
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("space"), window, cx));
    vcx.run_until_parked();
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("escape"), window, cx));
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("escape"), window, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(!c.you_block_open, "empty Esc discards the You-block (rule 3)");
        assert_eq!(c.focus, crate::AgentFocus::Transcript, "back to navigation");
        assert!(
            c.input_surface.compose().text().trim().is_empty(),
            "draft cleared on discard"
        );
        assert_eq!(
            c.editor.document().edit_seq(),
            seq_before,
            "the transcript is byte-identical — no phantom You turn (rule 3 / INV-1)"
        );
    });
}

/// UXI-AgentTile-11 rule 4: a You-block with real text PERSISTS after Esc (pending the
/// next Submit) — focus returns to navigation but the block stays open with its
/// text. Re-entering Insert reuses the same single block (rule 6).
#[gpui::test]
fn worksheet_nonempty_you_block_persists_after_esc(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);

    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("i"), window, cx));
    vcx.run_until_parked();
    for k in ["h", "i"] {
        view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key(k), window, cx));
    }
    vcx.run_until_parked();
    // 1st Esc → Normal IN the block (edit-in-place); focus stays on the compose.
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("escape"), window, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(c.focus, crate::AgentFocus::Compose, "1st Esc stays in the block");
        assert_eq!(c.input_surface.compose().mode, crate::EditMode::Normal, "now Normal");
        assert!(c.you_block_open);
    });
    // 2nd Esc → leave to nav; the non-empty block persists (rule 4).
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("escape"), window, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "non-empty block persists (rule 4)");
        assert_eq!(c.focus, crate::AgentFocus::Transcript, "2nd Esc returns to nav");
        assert_eq!(c.input_surface.compose().text().trim(), "hi", "draft retained");
    });

    // Re-entering Insert at the SAME anchor resumes the block, text kept.
    view.update(vcx, |v, cx| {
        let anchor = v.agent_mut(cx).expect("agent").you_block_anchor;
        if let Some(a) = anchor {
            v.agent_mut(cx).expect("agent").editor.cursor_mut().line = a;
        }
    });
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("i"), window, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open);
        assert_eq!(c.focus, crate::AgentFocus::Compose);
        assert_eq!(c.input_surface.compose().text().trim(), "hi", "same block, text kept");
    });
}

/// REGRESSION (runtime report): you can drop to Normal IN a You-block and re-enter
/// Insert into the SAME region — use Helix motions to edit, or return to your text
/// after a second thought. 1st Esc = Normal (stay in block), `i`/motions work, the
/// block stays the active editable surface (does NOT jump to transcript nav).
#[gpui::test]
fn worksheet_block_normal_then_insert_again(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    for ch in ["h", "e", "l", "l", "o"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    vcx.run_until_parked();
    // 1st Esc → Normal IN the block (still the active surface).
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(c.focus, crate::AgentFocus::Compose, "stay in the block, not nav");
        assert_eq!(c.input_surface.compose().mode, crate::EditMode::Normal);
        assert!(c.inline_you_block_active(), "block still the visible active surface");
    });
    // A Helix motion edits within the reply (Normal-mode key routes to the compose).
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("b"), w, cx));
    vcx.run_until_parked();
    // `i` re-enters Insert into the SAME region (the reported bug: couldn't do this).
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(c.focus, crate::AgentFocus::Compose);
        assert_eq!(c.input_surface.compose().mode, crate::EditMode::Insert, "back in Insert");
        assert_eq!(c.input_surface.compose().text().trim(), "hello", "same block, text intact");
        assert!(c.parked_you_blocks.is_empty(), "no spurious second block");
    });
}

/// UXI-AgentTile-11 rules 2/6/7 (painted, stage 2): navigating idle paints NEITHER the
/// inline You-block NOR the bottom chatbox; an open You-block paints INLINE (the
/// `you-block` probe) and NOT the bottom box; mid-turn paints the bottom chatbox
/// (`compose-box`) and NOT the inline block.
#[gpui::test]
fn worksheet_compose_visibility_tracks_block_and_turn(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);

    // Idle, navigating, no block → no inline block, no bottom box.
    assert!(probe_dirty(&view, vcx, "you-block").is_none(), "idle nav: no inline block");
    assert!(probe_dirty(&view, vcx, "compose-box").is_none(), "idle nav: no bottom box");

    // Open a You-block → it paints INLINE, not as the bottom box (rules 2/6).
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("i"), window, cx));
    vcx.run_until_parked();
    assert!(
        probe_dirty(&view, vcx, "you-block").is_some(),
        "an open You-block paints inline in the transcript (rule 2)"
    );
    assert!(
        probe_dirty(&view, vcx, "compose-box").is_none(),
        "the open block is inline — NOT the bottom box"
    );

    // Discard (Esc Esc: Normal then leave; empty ⇒ discard), then go mid-turn.
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("escape"), window, cx));
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("escape"), window, cx));
    vcx.run_until_parked();
    assert!(probe_dirty(&view, vcx, "you-block").is_none(), "discarded → no inline block");
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.turn_phase = crate::TurnPhase::begin(std::time::Instant::now());
    });
    assert!(
        probe_dirty(&view, vcx, "compose-box").is_some(),
        "mid-turn shows the bottom chatbox (rule 7)"
    );
    assert!(
        probe_dirty(&view, vcx, "you-block").is_none(),
        "mid-turn suppresses the inline block (no double compose)"
    );
}

/// REGRESSION (user-reported: "typed characters don't show up until later").
/// The inline You-block renders INSIDE the cached `TranscriptView`, so a keystroke
/// must notify the SESSION entity to fire its `cx.observe` and bust the transcript
/// cache. The compose dispatch used `with_session_silent` (no session notify), so
/// inline typing left the transcript stale until an unrelated event repainted.
/// Here: typing into an open inline block MUST re-render the transcript, and the
/// text must land in the compose.
#[gpui::test]
fn worksheet_inline_typing_rerenders_transcript(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (view, vcx) = boot_worksheet_nav(cx);

    // Open a You-block, then settle one frame so the baseline excludes the open.
    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("i"), window, cx));
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // Type into the inline block — each keystroke must bust the transcript cache.
    for k in ["h", "e", "l", "l", "o"] {
        view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key(k), window, cx));
        vcx.run_until_parked();
    }
    let after = crate::perf_render_count("transcript");
    assert!(
        after > base,
        "typing into the inline You-block must re-render the cached transcript \
         (session-notify busts the observe) — base {base}, after {after}; a flat \
         count is the 'chars appear later' stale-render bug"
    );
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.agent_mut(cx).expect("agent").input_surface.compose().text().trim(),
            "hello",
            "the typed text landed in the compose draft"
        );
    });
}

/// REGRESSION (user-reported: "where I am inserting jumps around"). The inline
/// You-block must anchor at the caret's line — injected right after that line's
/// rendered item — NOT silently fall to the transcript tail when the anchor line
/// isn't its own `FlatItem::Line`. Open a block with the caret on an EARLY line of
/// the latest turn and assert the `YouBlock` item is not last (it sits inline,
/// above later content), and that it stays put across keystrokes.
#[gpui::test]
fn worksheet_you_block_anchors_at_cursor_not_tail(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx); // defaults to worksheet nav

    // A multi-line latest agent turn so there ARE lines after the anchor.
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk("para one\npara two\npara three\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();

    // Park the transcript caret on the FIRST line of the latest turn (an early,
    // legal anchor), then open a block there.
    let (anchor, last_line) = view
        .update(vcx, |v, cx| {
            let mut c = v.agent_mut(cx).expect("agent");
            let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
            c.editor.cursor_mut().line = s;
            (s, c.editor.document().line_count().saturating_sub(1))
        });
    assert!(anchor < last_line, "anchor is genuinely above the tail");

    view.update_in(vcx, |v, window, cx| v.handle_claude_key(&ws_bare_key("i"), window, cx));
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "block opened at the early line");
        let fi = &c.view_model.flat_items_cache;
        let yb = fi
            .iter()
            .position(|it| matches!(it, crate::FlatItem::YouBlock { parked: None }))
            .expect("YouBlock injected into the flat list");
        assert!(
            yb < fi.len() - 1,
            "the You-block must sit INLINE above later turn content, not at the \
             tail (idx {yb} of {}) — falling to the tail is the 'jumps around' bug",
            fi.len()
        );
    });
}

/// REGRESSION (runtime: "can't type the m character in chatbox mode"): the bare-`m`
/// mark chord must NOT fire in the editable compose — `m` is typeable in Insert, and
/// in compose-Normal it routes to the editor (no pending mark chord).
#[gpui::test]
fn m_is_typeable_in_compose(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    for ch in ["m", "a", "p"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.agent_mut(cx).expect("agent").input_surface.compose().text().trim(),
            "map",
            "m types in the compose (Insert), not eaten by a mark chord"
        );
    });
    // Drop to compose-Normal (1st Esc), press m → still no mark chord started.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("m"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, _cx| {
        assert!(
            v.pending_mark_chord.is_none(),
            "m in compose-Normal must NOT start a mark chord"
        );
    });
}

/// REGRESSION (runtime: "sometimes my cursor edits an existing you-div, sometimes it
/// creates a new one"): the caret on an EXISTING block's anchor RESUMES that block
/// (active or parked), deterministically — it does not spawn a duplicate.
#[gpui::test]
fn worksheet_cursor_on_existing_block_resumes_it(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk("alpha\nbeta\ngamma\ndelta\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    let s = view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        s
    });
    // Block 1 "first" at s, Esc Esc to nav.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    for ch in ["f", "i", "r", "s", "t"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    // Block 2 "second" at s+2, Esc Esc to nav (block 1 now parked).
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.cursor_mut().line = s + 2;
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    for ch in ["s", "e", "c"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    vcx.run_until_parked();

    // Navigate BACK to block 1's anchor (s) and press i → RESUMES block 1, not a 3rd.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.cursor_mut().line = s;
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.input_surface.compose().text().trim(),
            "first",
            "cursor on block 1's anchor resumes block 1 (edits the existing one)"
        );
        assert_eq!(c.you_block_anchor, Some(s));
        // Exactly two blocks total still (block 2 parked, block 1 active) — no dup.
        assert_eq!(c.parked_you_blocks.len(), 1, "no duplicate block created");
        assert_eq!(c.parked_you_blocks[0].1.trim(), "sec");
    });
}

/// UXI-AgentTile-11 rule 6 (MULTIPLE insertion points end-to-end): open two blocks at
/// different anchors, confirm both render as separate inline `YouBlock`s, and that
/// gather+freeze commits BOTH in place (each text present in the transcript).
#[gpui::test]
fn worksheet_multiple_insertion_points(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk("alpha\nbeta\ngamma\ndelta\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();

    // Block 1 at an early line.
    let s = view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        s
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    for ch in ["o", "n", "e"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    // Esc Esc back to nav (1st = Normal in block, 2nd = leave; block 1 persists),
    // navigate down, then `i` for a 2nd insertion point.
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.cursor_mut().line = s + 2;
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    for ch in ["t", "w", "o"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        // State: one parked ("one"), one active ("two").
        assert_eq!(c.parked_you_blocks.len(), 1, "two insertion points (1 parked + active)");
        assert_eq!(c.parked_you_blocks[0].1.trim(), "one");
        assert_eq!(c.input_surface.compose().text().trim(), "two");
        // Render: two inline YouBlocks.
        let n_blocks = c
            .view_model
            .flat_items_cache
            .iter()
            .filter(|it| matches!(it, crate::FlatItem::YouBlock { .. }))
            .count();
        assert_eq!(n_blocks, 2, "both insertion points render inline");
        // Gather + freeze both under one turn → both texts land in the transcript.
        let blocks = c.collect_you_blocks();
        assert_eq!(blocks.len(), 2, "gather returns both blocks");
        c.freeze_you_blocks(&blocks, 1);
        let full = c.editor.document().full_text();
        assert!(full.contains("one") && full.contains("two"), "both frozen in place");
    });
}

/// UXI-AgentTile-11 rule 6 (MULTIPLE insertion points): with a non-empty block open at
/// anchor A, navigating to a DIFFERENT legal line and pressing `i` opens a SECOND
/// block there — PARKING the first at A (its text kept, never dragged to the new
/// line). Pressing `i` at the SAME anchor resumes in place.
#[gpui::test]
fn worksheet_reentering_insert_keeps_block_anchor(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx); // defaults to worksheet nav
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk("alpha\nbeta\ngamma\ndelta\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();

    // Open a block on an early line, type text, Esc to navigate (block persists).
    let anchor_a = view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        s
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    for k in ["h", "i"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(k), w, cx));
    }
    // Esc Esc → Normal then leave to nav (block persists, non-empty).
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "block persists after Esc (non-empty)");
        assert_eq!(c.you_block_anchor, Some(anchor_a));
    });

    // Navigate to a LATER line, press `i` → a SECOND insertion point: the first is
    // PARKED at anchor A (text kept, NOT dragged), a fresh active opens at the new line.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.cursor_mut().line = anchor_a + 2;
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.parked_you_blocks.len(),
            1,
            "the first block is parked as a second insertion point"
        );
        assert_eq!(c.parked_you_blocks[0].0, Some(anchor_a), "parked at its ORIGINAL anchor");
        assert_eq!(c.parked_you_blocks[0].1.trim(), "hi", "parked text kept, not dragged");
        assert_ne!(
            c.you_block_anchor,
            Some(anchor_a),
            "the new active block is at the new line, not A"
        );
        assert!(c.input_surface.compose().text().trim().is_empty(), "fresh active block");
        assert_eq!(c.focus, crate::AgentFocus::Compose);
    });

    // Pressing `i` again at the SAME (new) anchor resumes in place (no third block).
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.agent_mut(cx).expect("agent").parked_you_blocks.len(),
            1,
            "i at the same anchor resumes — no extra parked block"
        );
    });
}

/// REGRESSION (bug-hunt 1/7): a stale You-block anchor must NEVER place a reply in
/// old history. After the transcript grows past the anchor's turn,
/// `effective_you_block_anchor()` returns None (⇒ tail), and an anchor one line PAST
/// the latest turn is illegal (bug-hunt 13).
#[gpui::test]
fn worksheet_stale_anchor_is_rejected(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ev(ReplyEvent::Chunk("aa\nbb\ncc\ndd\n".into()))],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let lc = c.editor.document().line_count();
        assert!(lc >= 3, "need a few lines to tag");
        // Deterministically tag: line 0 belongs to an OLD agent turn Llm(1); every
        // other content line to the LATEST turn Llm(2). (The synthetic stream path
        // can't be made to advance the turn number, so tag directly — this tests the
        // guard logic, which is what the real per-turn numbering feeds.)
        let a0 = c.editor.anchor_for_line(0);
        c.editor
            .metadata_mut::<crate::TurnId>()
            .insert(a0, crate::TurnId::Llm(1));
        for l in 1..lc {
            let a = c.editor.anchor_for_line(l);
            c.editor
                .metadata_mut::<crate::TurnId>()
                .insert(a, crate::TurnId::Llm(2));
        }
        c.you_block_anchor = Some(0);
        assert!(
            !c.you_block_anchor_is_legal(0),
            "line 0 (Llm 1, OLD turn) is illegal vs the latest Llm 2 (bug-hunt 5/13)"
        );
        assert!(
            c.you_block_anchor_is_legal(1),
            "a line in the latest agent turn is legal"
        );
        assert_eq!(
            c.effective_you_block_anchor(),
            None,
            "a stale anchor resolves to None (⇒ tail append), never mid-history (bug-hunt 1)"
        );
    });
}

/// REGRESSION (bug-hunt 2 + "/clear can't type"): a replay/reconnect rebuild must
/// not let a pending reply re-materialize at a STALE line — but it also must not
/// leave the worksheet un-typeable. `reset_for_replay` re-settles: it clears the
/// stale anchor and reopens the block at the TAIL (`anchor = None`, the stale-safe
/// case — `effective_you_block_anchor` folds `None` to a tail append, never
/// mid-history). The empty rebuilt transcript ⇒ the block stays OPEN so typed
/// chars repaint (a historyless `/clear` session gets no `ReplayEnd` to re-settle).
#[gpui::test]
fn worksheet_replay_reopens_tail_block_stale_safe(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open);
        c.reset_for_replay();
        assert!(
            c.inline_you_block_active(),
            "the rebuilt (empty) worksheet stays typeable — else post-/clear typing \
             doesn't repaint (bug-hunt 2 stayed stale-safe: see the anchor below)"
        );
        assert_eq!(
            c.you_block_anchor, None,
            "the stale anchor is cleared → the reopened block is at the TAIL, not \
             a stale mid-history line (bug-hunt 2)"
        );
    });
}

/// REGRESSION (bug-hunt 6): mid-turn in the worksheet, a keystroke edits the bottom
/// CHATBOX (the steering box), not transcript navigation — even though focus is
/// nominally on the transcript when the turn began.
#[gpui::test]
fn worksheet_midturn_typing_routes_to_chatbox(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // Begin a turn; worksheet rests in transcript focus.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.turn_phase = crate::TurnPhase::begin(std::time::Instant::now());
        assert_eq!(c.focus, crate::AgentFocus::Transcript);
    });
    for k in ["h", "i"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(k), w, cx));
    }
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.input_surface.compose().text().trim(),
            "hi",
            "mid-turn worksheet typing must reach the chatbox (rule 7), not nav"
        );
    });
}

/// INTENT (co-authoring a document): a tall inline You-block GROWS to render every
/// line (no internal window/scroll) AND the caret stays painted inside the transcript
/// viewport — UXI-TextEditing-1 upheld by the transcript scroll following the caret, not by
/// truncating the block. PAINTED proof (layout probe), not window math: type a block
/// far taller than the viewport, then assert the caret's painted row lies inside the
/// painted transcript viewport. (Replaces the old test that merely re-checked the
/// `YB_WIN=10` window it asserted — the "You div scrolls after a while" bug.)
#[gpui::test]
fn worksheet_tall_you_block_grows_caret_painted_in_viewport(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx));
    vcx.run_until_parked();
    // Co-author a LONG note — enough lines to far exceed any test viewport, so the
    // reveal is genuinely forced to scroll (a shorter block that just fits would make
    // the assertion vacuous). Caret ends at the tail.
    for _ in 0..90 {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("x"), w, cx));
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("enter"), w, cx));
    }
    vcx.run_until_parked();
    let n = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().editor.document().line_count())
        })
        .unwrap();
    assert!(n > 80, "block genuinely long ({n} lines)");
    // Settle: the You-block lives in the CACHED transcript, so force it to re-render +
    // re-reveal by mutating the session (agent_mut notifies) and re-latching the caret
    // reveal — a bare root notify would skip the cached child. Lazy item measurement
    // means the reveal scroll lands only after several frames.
    let bust_and_reveal = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            if let Some(mut c) = v.agent_mut(cx) {
                c.pending_reveal_cursor = true;
            }
            cx.notify();
        });
        vcx.run_until_parked();
    };
    for _ in 0..5 {
        bust_and_reveal(&view, vcx);
    }
    crate::layout_probe_begin();
    bust_and_reveal(&view, vcx);
    let caret = crate::layout_probe_get("compose-cursor-row");
    let viewport = crate::layout_probe_get("transcript-viewport");
    let block = crate::layout_probe_get("you-block");
    crate::layout_probe_end();

    let (_, vy, _, vh) = viewport.expect("transcript viewport did not paint");
    let (_, _, _, bh) = block.expect("you-block did not paint");
    // NON-VACUOUS: the block must actually be taller than the viewport, else "caret
    // visible" proves nothing (the block simply fit). This is the growth intent.
    assert!(
        bh > vh,
        "block height {bh} must exceed viewport {vh} (else the test is vacuous)"
    );
    let (_, cy, _, ch) =
        caret.expect("caret row was NOT painted — the caret is below the fold (UXI-TextEditing-1)");
    assert!(
        cy >= vy - 0.5 && cy + ch <= vy + vh + 0.5,
        "UXI-TextEditing-1: caret row [{cy}, {}] must lie inside the transcript viewport [{vy}, {}] \
         (block {bh}px tall grew past the {vh}px viewport, yet the caret stays visible)",
        cy + ch,
        vy + vh
    );
}

/// REGRESSION (runtime report: "on first open I don't see anything"): a FRESH
/// worksheet session (empty transcript) must show an input immediately — settle
/// opens a tail You-block (focus=Compose) so there's a visible place to type.
#[gpui::test]
fn fresh_worksheet_session_shows_an_input(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        // Simulate a brand-new session: empty transcript, empty draft, resting nav.
        c.editor = yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
        c.close_you_block();
        c.focus = crate::AgentFocus::Transcript;
        assert!(c.editor.document().is_empty(), "fresh transcript");
        c.settle_input_focus();
        assert!(c.you_block_open, "a fresh session opens a VISIBLE tail input block");
        assert!(c.inline_you_block_active(), "the input is visible");
        assert_eq!(
            c.focus,
            crate::AgentFocus::Compose,
            "rests focused+Insert so typing lands immediately (no `i`); the space leader \
             still opens the tile menu on the empty block via the empty-draft heuristic"
        );
        assert_eq!(
            c.input_surface.compose().mode,
            crate::EditMode::Insert,
            "the fresh block is in Insert — type and see it"
        );
    });
}

/// REGRESSION (runtime: "when an agent tile is focused I can't use the tile menu —
/// Leader: space"): a fresh worksheet rests in nav, so bare `space` opens the tile
/// menu (it was regressed when the block auto-focused Insert and `space` typed).
#[gpui::test]
fn fresh_worksheet_space_opens_the_tile_menu(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_worksheet_nav(cx);
    // Land in the fresh-session resting state (empty transcript, settled).
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.editor = yalda::editor::Editor::new(String::new(), std::path::PathBuf::from("*claude*"));
        c.close_you_block();
        c.focus = crate::AgentFocus::Transcript;
        c.settle_input_focus();
    });
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();
    assert!(
        !view.read_with(vcx, |v, _| v.overlay_is_menu()),
        "no menu before pressing space"
    );
    vcx.simulate_keystrokes("space");
    vcx.run_until_parked();
    assert!(
        view.read_with(vcx, |v, _| v.overlay_is_menu()),
        "bare space on a focused (nav) worksheet opens the tile menu"
    );
}

/// REGRESSION (round-3 restore edge): `settle_input_focus` makes focus/You-block
/// consistent with the restored placement + draft. A restored chatbox focuses its
/// box; a restored worksheet draft shows as a tail block (not hidden); an empty
/// worksheet rests in nav. Upholds focus=Compose ⇒ a visible surface.
#[gpui::test]
fn worksheet_restore_settles_focus_and_block(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);

    // Restored worksheet WITH a draft → tail block, focus=Compose, visible.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.input_surface =
            crate::InputSurface::with_draft(crate::InputModeKind::Worksheet, "restored draft");
        c.focus = crate::AgentFocus::Transcript; // as the constructor leaves it
        c.you_block_open = false;
        c.settle_input_focus();
        assert!(c.you_block_open, "restored worksheet draft opens a tail block");
        assert_eq!(c.you_block_anchor, None, "at the tail");
        assert_eq!(
            c.focus,
            crate::AgentFocus::Compose,
            "a restored draft rests focused+Insert so you continue writing immediately"
        );
        assert!(c.inline_you_block_active(), "the draft is visible");
    });

    // Restored worksheet EMPTY draft but WITH transcript content → nav (there is
    // history to navigate; a fresh/empty transcript would instead open a tail block).
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.editor.append_llm_chunk(crate::TurnId::Llm(1), "an agent reply line\n");
        c.input_surface = crate::InputSurface::new(crate::InputModeKind::Worksheet);
        c.settle_input_focus();
        assert!(!c.you_block_open, "with content + empty draft, rest in nav");
        assert_eq!(c.focus, crate::AgentFocus::Transcript);
    });

    // Restored chatbox → focus the box.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.input_surface =
            crate::InputSurface::with_draft(crate::InputModeKind::Chatbox, "box draft");
        c.focus = crate::AgentFocus::Transcript;
        c.settle_input_focus();
        assert_eq!(c.focus, crate::AgentFocus::Compose, "chatbox focuses its box");
        assert!(!c.you_block_open, "chatbox has no inline block");
    });
}

/// REGRESSION (bug-hunt-2 B1): the `f` focus-toggle (Transcript→Compose) in an idle
/// worksheet must OPEN a You-block — leaving focus=Compose with no visible surface
/// would make typing vanish into the void.
#[gpui::test]
fn worksheet_focus_toggle_opens_visible_block(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update(vcx, |v, cx| v.toggle_agent_focus(cx));
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(c.focus, crate::AgentFocus::Compose);
        assert!(
            c.inline_you_block_active(),
            "focus→Compose in idle worksheet must open a VISIBLE inline block (B1)"
        );
        assert_eq!(c.input_surface.compose().mode, crate::EditMode::Insert);
    });
    // And typing now lands in the (visible) block.
    for k in ["o", "k"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(k), w, cx));
    }
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.agent_mut(cx).expect("agent").input_surface.compose().text().trim(),
            "ok"
        );
    });
}

/// REGRESSION (round-2, leader-gate): mid-turn in the worksheet, input routes to the
/// bottom chatbox while focus STAYS on the transcript (focus=Compose would strand on
/// stop). `focused_in_insert_mode` returns true mid-turn-worksheet so the universal
/// leaders (`space`/`.`/`?`) do NOT fire — a space is typed into the steer.
#[gpui::test]
fn worksheet_midturn_space_types_into_chatbox(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // REAL mid-turn state: awaiting, focus stays on the transcript (NOT Compose).
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        c.turn_phase = crate::TurnPhase::begin(std::time::Instant::now());
        c.focus = crate::AgentFocus::Transcript;
        c.input_surface.compose_mut().mode = crate::EditMode::Insert;
    });
    for k in ["a", "space", "b"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(k), w, cx));
    }
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.agent_mut(cx).expect("agent").input_surface.compose().text(),
            "a b",
            "space must type into the mid-turn chatbox, not fire the leader menu"
        );
    });
}

/// REGRESSION (bug-hunt 6 follow-through): when the turn ends, a non-empty mid-turn
/// draft is NOT lost — it carries over as a tail You-block; an empty draft returns
/// to transcript navigation.
#[gpui::test]
fn worksheet_turn_end_carries_over_draft_or_rests_in_nav(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // Non-empty draft at turn end → tail You-block.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let cb = c.input_surface.compose_mut();
        let n = cb.editor.document().rope().len_chars();
        cb.editor.programmatic_insert(n, "carry");
        let g = c.generation; c.finalize_agent_turn_idem(g, 1);
    });
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "non-empty draft carries over as a block");
        assert_eq!(c.you_block_anchor, None, "carried over at the tail");
        assert_eq!(c.focus, crate::AgentFocus::Compose);
    });
    // Empty draft at turn end → nav.
    let (view2, vcx2) = boot_worksheet_nav(cx);
    view2.update(vcx2, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let g = c.generation; c.finalize_agent_turn_idem(g, 1);
        assert!(!c.you_block_open, "empty draft → no block");
        assert_eq!(c.focus, crate::AgentFocus::Transcript, "rests in navigation");
    });
}

/// VERIFICATION HARNESS (painted-bounds proof of UXI-AgentTile-3 layout): the subagent
/// (Task) list renders in the RIGHT sidepanel — to the right of the compose box,
/// one subagent per line. Register a Task subagent tool call so the list appears,
/// drive a real paint pass, and assert (via the layout probe) that the
/// `subagent-panes` strip is painted and its left edge is at/right-of the compose
/// box's right edge — i.e. beside the chatbox, not above it.
///
/// Negative control: revert the sidepanel restructure (put the panels back above
/// the compose in a `flex_col`) and `panes_x` drops below `box_x + box_w` → RED.
#[gpui::test]
fn subagent_panes_paint_right_of_compose(cx: &mut TestAppContext) {
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    // Need the bottom compose box on screen (default is worksheet now); the panes
    // sit beside it. Enter chatbox so the `compose-box` probe has a target.
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));

    // Register a Task subagent (Think + prompt) into the bound session's tool
    // state so `subagents()` is non-empty and the panes render.
    view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        let mut c = v.agent_mut(cx).expect("agent");
        let id: ToolCallId = "task-pane".into();
        let mut tc = ToolCall::new(id.clone(), "Explore repo".to_string());
        tc.kind = ToolKind::Think;
        tc.raw_input = Some(serde_json::json!({"prompt": "map the code"}));
        let anchor = c.editor.anchor_for_line(0);
        c.tools.register(crate::ToolCallKey::from_id(&id), tc, anchor);
    });

    // Settle, then probe a clean paint pass.
    for _ in 0..4 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let panes = crate::layout_probe_get("subagent-panes");
    let box_bounds =
        view.update(vcx, |v, cx| v.agent_read(cx, |c| c.input_surface.compose().bounds.get()));
    crate::layout_probe_end();

    let (box_x, _, box_w, box_h) = box_bounds.expect("compose box did not paint");
    let (panes_x, _, panes_w, panes_h) =
        panes.expect("subagent panes did NOT paint — they should appear when a subagent exists");

    assert!(panes_h > 1.0, "subagent panes have no height ({panes_h})");
    assert!(panes_w > 1.0, "subagent panes have no width ({panes_w})");
    let _ = box_h;
    // The sidepanel's left edge is at/right-of the compose box's right edge (slack
    // for the 1px border) — i.e. it sits BESIDE the chatbox, in the right column.
    assert!(
        panes_x + 2.0 >= box_x + box_w,
        "subagent panes left {panes_x} is NOT right of the compose right {} — not in the sidepanel",
        box_x + box_w,
    );
}

/// Both segments (Plan + Subagents) render in ONE sidepanel, stacked: the Plan
/// segment on top, the Subagents segment below it, both inside the painted
/// `agent-sidepanel` bounds. Proves the "both visible on the same sidepanel"
/// requirement, not two mutually-exclusive views.
#[gpui::test]
fn plan_and_subagents_share_the_sidepanel(cx: &mut TestAppContext) {
    let (view, vcx, _id, _session) = boot_with_transcript(cx);

    // A live plan (opens the Plan segment) AND a Task subagent (opens Subagents).
    set_plan(&view, vcx, 2);
    view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        let mut c = v.agent_mut(cx).expect("agent");
        c.tasklist_open = true;
        let id: ToolCallId = "task-share".into();
        let mut tc = ToolCall::new(id.clone(), "Explore repo".to_string());
        tc.kind = ToolKind::Think;
        tc.raw_input = Some(serde_json::json!({"prompt": "map the code"}));
        let anchor = c.editor.anchor_for_line(0);
        c.tools.register(crate::ToolCallKey::from_id(&id), tc, anchor);
    });

    for _ in 0..4 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let side = crate::layout_probe_get("agent-sidepanel");
    let plan = crate::layout_probe_get("tasklist-panel");
    let subs = crate::layout_probe_get("subagent-panes");
    crate::layout_probe_end();

    let (sx, sy, sw, sh) = side.expect("sidepanel did not paint");
    let (px_, py, _, ph) = plan.expect("Plan segment did not paint");
    let (qx, qy, _, qh) = subs.expect("Subagents segment did not paint");

    // Both segments live inside the sidepanel's painted box …
    assert!(
        px_ >= sx - 1.0 && py >= sy - 1.0 && py + ph <= sy + sh + 1.0,
        "Plan segment not inside the sidepanel",
    );
    assert!(
        qx >= sx - 1.0 && qy >= sy - 1.0 && qy + qh <= sy + sh + 1.0,
        "Subagents segment not inside the sidepanel",
    );
    let _ = sw;
    // … and Plan is stacked ABOVE Subagents (segmented, both visible).
    assert!(
        py + ph <= qy + 2.0,
        "Plan segment bottom {} is not above the Subagents segment top {qy}",
        py + ph,
    );
}

/// Panel highlight SWAPS the main view to the subagent's context: focusing the
/// Subagents panel (Cmd-0, which previews the first row) sets `focused_subagent`
/// to that subagent — the render swap trigger. Drives the REAL `focus_agent_panel`
/// entry point.
///
/// Negative control: drop `reveal_panel_selection` from `focus_agent_panel` and
/// `focused_subagent` stays `None` → the assert fails RED.
#[gpui::test]
fn panel_highlight_swaps_to_subagent(cx: &mut TestAppContext) {
    let (view, vcx, id, _session) = boot_with_transcript(cx);

    let sub_key = view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        let mut c = v.agent_mut(cx).expect("agent");
        let tid: ToolCallId = "sub-swap".into();
        let mut tc = ToolCall::new(tid.clone(), "Explore repo".to_string());
        tc.kind = ToolKind::Think;
        tc.raw_input = Some(serde_json::json!({"prompt": "map the code"}));
        let anchor = c.editor.anchor_for_line(0);
        let key = crate::ToolCallKey::from_id(&tid);
        c.tools.register(key.clone(), tc, anchor);
        c.subagents_open = true;
        c.tasklist_open = false;
        key
    });
    vcx.run_until_parked();

    // Cmd-0 focuses the panel; the first Subagents row previews → swap set.
    let focused = view
        .update(vcx, |v, cx| {
            v.focus_agent_panel(cx);
            v.read_session(id, cx, |c| c.focused_subagent.clone())
        })
        .expect("session");
    assert_eq!(
        focused,
        Some(sub_key),
        "highlighting the subagent must set focused_subagent (the view-swap trigger)"
    );

    // Leaving the subagent view (unfocus) returns to the main transcript.
    let after_back = view
        .update(vcx, |v, cx| {
            v.unfocus_subagent(cx);
            v.read_session(id, cx, |c| c.focused_subagent.clone())
        })
        .expect("session");
    assert_eq!(after_back, None, "back must clear the subagent swap");
}

/// The subagent swap actually PAINTS (UXI-AgentTile-6): with a subagent focused, the
/// main area renders the `subagent-view` (Back header + its context) and the
/// cached `transcript-viewport` is NOT painted — proving the view was replaced,
/// not just a state flag flipped. Clearing focus (Back / Esc) restores it.
///
/// Negative control: render the transcript unconditionally (drop the
/// `focused_subagent` match arm) and `subagent-view` never paints → RED.
#[gpui::test]
fn subagent_focus_swaps_the_painted_view(cx: &mut TestAppContext) {
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let key = view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        let mut c = v.agent_mut(cx).expect("agent");
        let tid: ToolCallId = "sub-paint".into();
        let mut tc = ToolCall::new(tid.clone(), "Explore repo".to_string());
        tc.kind = ToolKind::Think;
        tc.raw_input = Some(serde_json::json!({"prompt": "map the code"}));
        let anchor = c.editor.anchor_for_line(0);
        let k = crate::ToolCallKey::from_id(&tid);
        c.tools.register(k.clone(), tc, anchor);
        k
    });
    vcx.run_until_parked();

    // Focus the subagent → swap. Probe a clean paint pass.
    view.update(vcx, |v, cx| v.focus_subagent(key, cx));
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let swapped = crate::layout_probe_get("subagent-view");
    let transcript_while_swapped = crate::layout_probe_get("transcript-viewport");
    crate::layout_probe_end();
    assert!(
        swapped.is_some(),
        "the subagent context view must paint when a subagent is focused"
    );
    assert!(
        transcript_while_swapped.is_none(),
        "the transcript must NOT paint while swapped to the subagent view"
    );

    // Back (unfocus) → the subagent view is gone (the transcript, a cached view,
    // repaints via cached-scene replay, so its own paint-time probe need not
    // re-fire; the swap's disappearance is the reliable signal that the main
    // view returned).
    view.update(vcx, |v, cx| v.unfocus_subagent(cx));
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let subagent_after_back = crate::layout_probe_get("subagent-view");
    crate::layout_probe_end();
    assert!(
        subagent_after_back.is_none(),
        "the subagent view must be gone after Back — the main view returned"
    );
}

/// Enter on a panel row activates it: reveals it in the transcript AND leaves
/// panel focus so the revealed content is readable. Drives the real
/// `panel_activate_selection`.
#[gpui::test]
fn panel_enter_reveals_and_exits(cx: &mut TestAppContext) {
    let (view, vcx, id, session) = boot_with_transcript(cx);
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state.editor.programmatic_insert(0, "l0\nl1\nl2\nl3\n");
        cx.notify();
    });
    view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        let mut c = v.agent_mut(cx).expect("agent");
        let tid: ToolCallId = "sub-enter".into();
        let mut tc = ToolCall::new(tid.clone(), "Explore repo".to_string());
        tc.kind = ToolKind::Think;
        tc.raw_input = Some(serde_json::json!({"prompt": "x"}));
        let anchor = c.editor.anchor_for_line(2);
        c.tools
            .register(crate::ToolCallKey::from_id(&tid), tc, anchor);
        c.subagents_open = true;
        c.tasklist_open = false;
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| v.focus_agent_panel(cx));
    let in_panel = view
        .update(vcx, |v, cx| {
            v.read_session(id, cx, |c| c.focus == crate::AgentFocus::Panel)
        })
        .expect("session");
    assert!(in_panel, "Cmd-0 should focus the panel");

    // REAL Enter.
    view.update(vcx, |v, cx| v.panel_activate_selection(cx));
    let focus_after = view
        .update(vcx, |v, cx| v.read_session(id, cx, |c| c.focus))
        .expect("session");
    assert_ne!(
        focus_after,
        crate::AgentFocus::Panel,
        "Enter must leave panel focus so the revealed line is readable"
    );
}

// === Steering queue (spec-turn-steering.md, UXI-AgentTile-13) ===

/// Submitting while a turn is in flight DELIVERS the steer immediately (the
/// worker forwards it mid-turn for promptQueueing agents) and commits the user
/// turn — it does NOT start a competing local turn (it rides the in-flight turn;
/// the running clocks are not reset) and the compose clears.
#[gpui::test]
fn steering_submit_while_awaiting_sends_immediately(cx: &mut TestAppContext) {
    use crate::agent::{InputSurface, TurnPhase};
    let (view, vcx, id, _session) = boot_with_transcript(cx);

    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.turn_phase = TurnPhase::begin(std::time::Instant::now());
            let m = c.input_surface.mode;
            c.input_surface = InputSurface::with_draft(m, "steer me");
        });
    });
    view.update(vcx, |v, cx| v.submit_compose(cx));

    let (awaiting, compose_empty, in_transcript) = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| {
            (
                c.turn_phase.is_awaiting(),
                c.input_surface.compose().text().trim().is_empty(),
                c.editor.document().full_text().contains("steer me"),
            )
        })
        .unwrap()
    });
    assert!(awaiting, "the in-flight turn keeps running (steer rides it)");
    assert!(compose_empty, "compose is cleared after sending");
    assert!(
        in_transcript,
        "the sent steer is committed to the transcript as a user turn"
    );
}

/// `stop_agent_inner` (the function Esc and ⌘. both call) interrupts ONLY when a
/// turn is in flight: Idle stays Idle; Awaiting → StopRequested.
#[gpui::test]
fn stop_interrupts_only_when_in_flight(cx: &mut TestAppContext) {
    use crate::agent::TurnPhase;
    let (view, vcx, id, _session) = boot_with_transcript(cx);

    // No turn in flight ⇒ no-op.
    view.update(vcx, |v, cx| v.stop_agent_inner(cx));
    let still_idle = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| matches!(c.turn_phase, TurnPhase::Idle)).unwrap()
    });
    assert!(still_idle, "stop with no turn in flight is a no-op");

    // Turn in flight ⇒ a stop is requested.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.turn_phase = TurnPhase::begin(std::time::Instant::now()));
        v.stop_agent_inner(cx);
    });
    let requested = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| c.turn_phase.stop_requested()).unwrap()
    });
    assert!(requested, "stop while in flight requests an interrupt");
}

/// REGRESSION (adversarial review): a steer submitted after a graceful Esc-stop
/// (turn in `StopRequested`) must SUPERSEDE the pending cancel — the send begins
/// a clean `Awaiting` turn, not leave it stuck in "stopping…" (which would make
/// the next Esc a hard force-restart). Only a cleanly-`Awaiting` turn is
/// preserved across a mid-turn steer.
#[gpui::test]
fn steering_after_stop_request_supersedes_pending_cancel(cx: &mut TestAppContext) {
    use crate::agent::{InputSurface, TurnPhase};
    let (view, vcx, id, _session) = boot_with_transcript(cx);

    // Turn in flight, then a graceful Stop → StopRequested.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.turn_phase = TurnPhase::begin(std::time::Instant::now()));
        v.stop_agent_inner(cx);
    });
    let pending = view
        .read_with(vcx, |v, cx| v.read_session(id, cx, |c| c.turn_phase.stop_requested()).unwrap());
    assert!(pending, "precondition: a graceful stop is pending");

    // Submit a steer — should supersede the pending cancel.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            let m = c.input_surface.mode;
            c.input_surface = InputSurface::with_draft(m, "keep going");
        });
        v.submit_compose(cx);
    });
    let (awaiting, still_pending) = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| {
            (
                matches!(c.turn_phase, TurnPhase::Awaiting { .. }),
                c.turn_phase.stop_requested(),
            )
        })
        .unwrap()
    });
    assert!(awaiting, "a steer after a stop-request begins a clean Awaiting turn");
    assert!(
        !still_pending,
        "the pending cancel is superseded — the next Esc is graceful, not a hard restart"
    );
}

/// Esc is UNBOUND from stopping a turn (runtime report: it conflicted with mode
/// switching — Esc is the Insert→Normal / leave-block key). With a turn in flight,
/// pressing Esc must NOT request a stop; the turn keeps streaming. (`⌘.` is the
/// stop, via `stop_agent`.)
#[gpui::test]
fn esc_does_not_stop_in_flight_turn(cx: &mut TestAppContext) {
    use crate::agent::TurnPhase;
    cx.update(crate::register_keymap);
    let (view, vcx, id, _session) = boot_with_transcript(cx);

    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.turn_phase = TurnPhase::begin(std::time::Instant::now()));
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    let (requested, awaiting) = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| {
            (c.turn_phase.stop_requested(), c.turn_phase.is_awaiting())
        })
        .unwrap()
    });
    assert!(!requested, "Esc must NOT request a stop (unbound)");
    assert!(awaiting, "the turn keeps streaming after Esc");

    // The explicit stop path still works.
    view.update(vcx, |v, cx| v.stop_agent_inner(cx));
    vcx.run_until_parked();
    let requested2 = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| c.turn_phase.stop_requested()).unwrap()
    });
    assert!(requested2, "explicit stop (⌘.) still requests a cancel");
}

/// Mid-turn ordering + echo-dedup (UXI-AgentTile-13), through the REAL reducer: a steer
/// sent while a turn is streaming commits AFTER the agent content that preceded
/// it, exactly once — the agent's later echo of the same prompt is suppressed
/// (no phantom / no duplicate).
#[gpui::test]
fn steering_midturn_ordering_and_dedup(cx: &mut TestAppContext) {
    use crate::agent::TurnPhase;
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;
    let (view, vcx, id, _session) = boot_with_transcript(cx);
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };

    // Turn 1 in flight; first agent content arrives.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.turn_phase = TurnPhase::begin(std::time::Instant::now())
        });
        v.apply_server_batch(vec![ev(ReplyEvent::Chunk("agent line A\n".into()))], cx);
    });
    vcx.run_until_parked();

    // User steers mid-turn (immediate send commits the user turn).
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            let m = c.input_surface.mode;
            c.input_surface = crate::agent::InputSurface::with_draft(m, "STEER ONE");
        });
        v.submit_compose(cx);
    });
    vcx.run_until_parked();

    // The agent later echoes the user prompt on the stream — must be deduped.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(vec![ev(ReplyEvent::UserMessage("STEER ONE\n".into()))], cx);
    });
    vcx.run_until_parked();

    let text = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| c.editor.document().full_text()).unwrap()
    });
    let a = text.find("agent line A").expect("agent content present");
    let s = text.find("STEER ONE").expect("steer committed to transcript");
    assert!(
        a < s,
        "the mid-turn steer lands AFTER the agent content that preceded it"
    );
    assert_eq!(
        text.matches("STEER ONE").count(),
        1,
        "steer committed exactly once — the agent's echo is deduped (no phantom)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// NOVEL VERIFICATION METHOD — agent-tile STATE-MACHINE FUZZER + invariant oracle
// ─────────────────────────────────────────────────────────────────────────────
// Every other test here is example-based: each pins ONE scripted scenario, so a
// regression that only surfaces under an unanticipated *sequence* of operations
// (toggle worksheet → stream a chunk → submit mid-turn → stop → toggle plan → …)
// slips through. This is the project's first PROPERTY-BASED test: it drives the
// REAL `YaldaGpuiView` through many deterministic-random operation sequences and,
// AFTER EVERY operation, runs one cross-cutting ORACLE that re-checks the whole
// invariant contract at once (`assert_agent_invariants`):
//   • UXI-TextEditing-1 (model): the compose caret is always at a valid position (line in
//     range, col ≤ that line's length) — it can never point off the buffer.
//   • INV-ORDER: the frozen transcript is append-only — its frozen-line count
//     never decreases, so no operation can rewrite or drop committed history.
//   • turn_phase well-formedness: `stop_requested ⇒ awaiting`.
//   • focus is always one of {Compose, Transcript}.
//   • liveness: no operation (or its render via `run_until_parked`) panics.
// A seeded LCG (no wall-clock, no RNG) chooses the op stream, so any failure
// reproduces exactly from its `seed`/`step`. This explores the interaction space
// the example tests cannot — the limit-test the user asked for.

/// The cross-cutting invariant oracle. Reads the live `AgentState` and asserts
/// the whole contract; `prev_frozen` carries the append-only watermark across
/// operations so a shrink anywhere in the sequence is caught.
#[cfg(test)]
fn assert_agent_invariants(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    id: crate::SessionId,
    prev_frozen: &mut usize,
    ctx: &str,
) {
    use crate::agent::AgentFocus;
    let (caret_line_ok, caret_col_ok, frozen, focus_ok, stop_ok) = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| {
            let comp = c.input_surface.compose();
            let cur = comp.editor.cursor();
            let lc = comp.editor.document().line_count().max(1);
            let caret_line_ok = cur.line < lc;
            let cl = cur.line.min(lc - 1);
            let line_len = comp
                .editor
                .document()
                .line_text(cl)
                .trim_end_matches('\n')
                .chars()
                .count();
            let caret_col_ok = cur.col <= line_len;
            let frozen = (0..c.editor.document().line_count())
                .filter(|&i| c.editor.is_frozen_line(i))
                .count();
            // UXI-AgentTile-3: panel focus is a legal focus, and you can only hold it
            // while at least one bottom panel is open (the toggles auto-exit when
            // the last one closes). The selected row is clamped at render, so the
            // stored `panel_sel` never indexes a panic.
            let focus_ok = matches!(
                c.focus,
                AgentFocus::Compose | AgentFocus::Transcript | AgentFocus::Panel
            ) && (c.focus != AgentFocus::Panel || c.tasklist_open || c.subagents_open);
            let stop_ok = !c.turn_phase.stop_requested() || c.turn_phase.is_awaiting();
            // UXI-AgentTile-11: a You-block exists ONLY in the worksheet (never chatbox
            // mode). The stored anchor is deliberately NOT asserted legal here — it
            // may go transiently stale and is re-validated at every consumption site
            // (effective_you_block_anchor), so a stale stored value is harmless.
            // A block (active OR parked) exists only in the worksheet — never chatbox.
            let block_mode_ok = (!c.you_block_open && c.parked_you_blocks.is_empty())
                || !c.input_surface.is_chatbox();
            // The EFFECTIVE anchor (what consumers use) is always legal-or-None.
            let eff_ok = c.effective_you_block_anchor().is_none_or(|a| c.you_block_anchor_is_legal(a));
            // focus==Compose ⇒ there is a VISIBLE editable surface (the bottom box in
            // chatbox/mid-turn, or the inline block) — never focus-into-the-void
            // (bug-hunt-2 B1/B4).
            let visible_surface_ok = c.focus != AgentFocus::Compose
                || c.input_surface.is_chatbox()
                || c.turn_phase.is_awaiting()
                || c.inline_you_block_active();
            (
                caret_line_ok,
                caret_col_ok,
                frozen,
                focus_ok,
                stop_ok && block_mode_ok && eff_ok && visible_surface_ok,
            )
        })
        .unwrap()
    });
    assert!(caret_line_ok, "UXI-TextEditing-1: compose caret line out of range [{ctx}]");
    assert!(caret_col_ok, "UXI-TextEditing-1: compose caret col past end of line [{ctx}]");
    assert!(focus_ok, "focus is Compose or Transcript [{ctx}]");
    assert!(
        stop_ok,
        "turn_phase stop⇒awaiting, You-block⇒worksheet, effective-anchor legal [{ctx}]"
    );
    assert!(
        frozen >= *prev_frozen,
        "INV-ORDER: frozen transcript shrank ({} -> {frozen}) — not append-only [{ctx}]",
        *prev_frozen,
    );
    *prev_frozen = frozen;
}

#[gpui::test]
fn agent_tile_statemachine_fuzz_holds_invariants(cx: &mut TestAppContext) {
    use crate::agent::TurnPhase;
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    // Deterministic LCG (no wall-clock / no RNG so failures reproduce by seed).
    fn next(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };

    for seed in 1u64..=20 {
        let (view, vcx, id, _session) = boot_with_transcript(cx);
        let mut st = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        let mut prev_frozen = 0usize;
        let mut spawn_n = 0u64;
        assert_agent_invariants(&view, vcx, id, &mut prev_frozen, "init");

        for step in 0..100 {
            let op = next(&mut st) % 20;
            match op {
                0 => view.update(vcx, |v, cx| {
                    v.with_session(id, cx, |c| c.input_surface.compose_mut().editor.insert_char('x'));
                }),
                1 => view.update(vcx, |v, cx| {
                    v.with_session(id, cx, |c| c.input_surface.compose_mut().editor.insert_char('\n'));
                }),
                2 => view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx)),
                3 => view.update(vcx, |v, cx| v.submit_compose(cx)),
                4 => view.update(vcx, |v, cx| {
                    v.apply_server_batch(vec![ev(ReplyEvent::Chunk("agent chunk\n".into()))], cx);
                }),
                5 => view.update(vcx, |v, cx| {
                    v.apply_server_batch(vec![ev(ReplyEvent::UserMessage("echo\n".into()))], cx);
                }),
                6 => view.update(vcx, |v, cx| {
                    v.apply_server_batch(vec![ev(ReplyEvent::ReplayComplete)], cx);
                }),
                7 => view.update(vcx, |v, cx| v.toggle_tasklist(cx)),
                8 => {
                    spawn_n += 1;
                    view.update(vcx, |v, cx| {
                        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
                        if let Some(mut c) = v.agent_mut(cx) {
                            let tcid: ToolCallId = format!("fuzz-task-{spawn_n}").into();
                            let mut tc = ToolCall::new(tcid.clone(), "Explore".to_string());
                            tc.kind = ToolKind::Think;
                            tc.raw_input = Some(serde_json::json!({"prompt": "do x"}));
                            let anchor = c.editor.anchor_for_line(0);
                            c.tools.register(crate::ToolCallKey::from_id(&tcid), tc, anchor);
                        }
                    });
                }
                9 => view.update(vcx, |v, cx| v.toggle_agent_focus(cx)),
                10 => view.update(vcx, |v, cx| v.stop_agent_inner(cx)),
                // UXI-AgentTile-11: drive the real You-block open / discard key paths so the
                // fuzzer exercises the inline-edit lifecycle against the oracle.
                11 => view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("i"), w, cx)),
                12 => {
                    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx))
                }
                13 => view.update(vcx, |v, cx| v.toggle_subagents(cx)),
                // UXI-AgentTile-3: enter/leave panel focus and navigate it through the
                // real Cmd-0 handler + key path, against the oracle.
                14 => view.update(vcx, |v, cx| v.focus_agent_panel(cx)),
                15 => view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("j"), w, cx)),
                16 => view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("k"), w, cx)),
                17 => view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx)),
                18 => view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("l"), w, cx)),
                _ => view.update(vcx, |v, cx| {
                    v.with_session(id, cx, |c| {
                        c.turn_phase = TurnPhase::begin(std::time::Instant::now())
                    });
                }),
            }
            vcx.run_until_parked();
            let label = format!("seed={seed} step={step} op={op}");
            assert_agent_invariants(&view, vcx, id, &mut prev_frozen, &label);
        }
    }
}

/// Register a `Task` tool-call as a subagent (the structured signal the harness
/// emits) so the bottom Subagents panel has a selectable row.
#[cfg(test)]
fn register_subagent(view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext, n: u64) {
    view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        if let Some(mut c) = v.agent_mut(cx) {
            let tcid: ToolCallId = format!("task-{n}").into();
            let mut tc = ToolCall::new(tcid.clone(), format!("Explore {n}"));
            tc.kind = ToolKind::Think;
            tc.raw_input = Some(serde_json::json!({"prompt": "do x"}));
            let anchor = c.editor.anchor_for_line(0);
            c.tools.register(crate::ToolCallKey::from_id(&tcid), tc, anchor);
        }
        cx.notify();
    });
    vcx.run_until_parked();
}

/// Give the focused session a Plan with `n` entries so the Tasklist column has
/// selectable rows.
#[cfg(test)]
fn set_plan(view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext, n: usize) {
    // `Plan` / `PlanEntry` are `#[non_exhaustive]` (no struct literal from here),
    // so build the plan the way production gets it: deserialize the ACP JSON.
    use yalda::acp_channel::Plan;
    let entries: Vec<_> = (0..n)
        .map(|i| serde_json::json!({"content": format!("step {i}"), "priority": "medium", "status": "pending"}))
        .collect();
    let plan: Plan = serde_json::from_value(serde_json::json!({ "entries": entries }))
        .expect("valid plan json");
    view.update(vcx, |v, cx| {
        if let Some(mut c) = v.agent_mut(cx) {
            c.current_plan = Some(plan);
        }
        cx.notify();
    });
    vcx.run_until_parked();
}

/// UXI-AgentTile-3: `h`/`l` switch the active column (Plan left / Subagents right) and
/// `j`/`k` then move within that column; the per-column row index is preserved.
#[gpui::test]
fn agent_panel_hl_switches_columns(cx: &mut TestAppContext) {
    use crate::agent::PanelColumn;
    let (view, vcx, id, _s) = boot_with_transcript(cx);
    set_plan(&view, vcx, 2);
    register_subagent(&view, vcx, 1);
    register_subagent(&view, vcx, 2);
    view.update(vcx, |v, cx| v.toggle_tasklist(cx)); // open the Plan column
    view.update(vcx, |v, cx| v.focus_agent_panel(cx));
    // Both columns open → entry lands on the LEFT (Plan).
    let col = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.read_with(vcx, |v, cx| {
            v.read_session(id, cx, |c| (c.panel_col, c.panel_sel)).unwrap()
        })
    };
    assert_eq!(col(&view, vcx), (PanelColumn::Tasklist, 0), "starts in Plan");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("l"), w, cx));
    assert_eq!(col(&view, vcx), (PanelColumn::Subagents, 0), "l → Subagents");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("j"), w, cx));
    assert_eq!(col(&view, vcx), (PanelColumn::Subagents, 1), "j moves within Subagents");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx));
    assert_eq!(
        col(&view, vcx),
        (PanelColumn::Tasklist, 1),
        "h → Plan, row clamped into the column"
    );
    // h again is a no-op (already leftmost).
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("h"), w, cx));
    assert_eq!(col(&view, vcx).0, PanelColumn::Tasklist, "h at the left edge is a no-op");
}

/// UXI-AgentTile-3: `Cmd-0` (here through `focus_agent_panel`) enters panel focus when
/// the bottom region has rows, and `Esc` restores the focus captured on entry.
#[gpui::test]
fn agent_panel_cmd0_enters_and_esc_restores(cx: &mut TestAppContext) {
    let (view, vcx, id, _s) = boot_with_transcript(cx);
    register_subagent(&view, vcx, 1);
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.focus = crate::AgentFocus::Compose);
    });
    view.update(vcx, |v, cx| v.focus_agent_panel(cx));
    let focus = view.read_with(vcx, |v, cx| v.read_session(id, cx, |c| c.focus).unwrap());
    assert_eq!(focus, crate::AgentFocus::Panel, "Cmd-0 enters panel focus");

    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("escape"), w, cx));
    let focus = view.read_with(vcx, |v, cx| v.read_session(id, cx, |c| c.focus).unwrap());
    assert_eq!(
        focus,
        crate::AgentFocus::Compose,
        "Esc returns to the previous focus"
    );
}

/// UXI-AgentTile-3: vim `j`/`k`/`g`/`G` move the panel selection, clamped to the row
/// count.
#[gpui::test]
fn agent_panel_vim_moves_selection(cx: &mut TestAppContext) {
    let (view, vcx, id, _s) = boot_with_transcript(cx);
    for n in 1..=3 {
        register_subagent(&view, vcx, n);
    }
    view.update(vcx, |v, cx| v.focus_agent_panel(cx));
    let sel = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.read_with(vcx, |v, cx| v.read_session(id, cx, |c| c.panel_sel).unwrap())
    };
    assert_eq!(sel(&view, vcx), 0, "selection starts at the top");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("j"), w, cx));
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("j"), w, cx));
    assert_eq!(sel(&view, vcx), 2, "j moves down");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("j"), w, cx));
    assert_eq!(sel(&view, vcx), 2, "j clamps at the last row");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("k"), w, cx));
    assert_eq!(sel(&view, vcx), 1, "k moves up");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("g"), w, cx));
    assert_eq!(sel(&view, vcx), 0, "g jumps to the top");
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("G"), w, cx));
    assert_eq!(sel(&view, vcx), 2, "G jumps to the bottom");
}

/// UXI-AgentTile-3: `Enter` on a subagent row focuses that subagent's output and
/// leaves panel focus.
#[gpui::test]
fn agent_panel_enter_focuses_subagent(cx: &mut TestAppContext) {
    let (view, vcx, id, _s) = boot_with_transcript(cx);
    register_subagent(&view, vcx, 1);
    register_subagent(&view, vcx, 2);
    view.update(vcx, |v, cx| v.focus_agent_panel(cx));
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("j"), w, cx));
    let want = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| {
            match &c.panel_column_rows(c.panel_col)[c.panel_sel] {
                crate::agent::PanelItem::Subagent(k) => Some(k.clone()),
                _ => None,
            }
        })
        .unwrap()
    });
    view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key("enter"), w, cx));
    let (focused, focus) = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| (c.focused_subagent.clone(), c.focus))
            .unwrap()
    });
    assert_eq!(focused, want, "Enter focuses the selected subagent");
    assert_ne!(
        focus,
        crate::AgentFocus::Panel,
        "Enter leaves panel focus after activating"
    );
}

/// UXI-AgentTile-3: the real `cmd-0` keymap binding (AgentView-scoped) reaches the
/// panel-focus handler — proving it shadows the global zoom-reset in agent tiles.
#[gpui::test]
fn agent_panel_cmd0_binding_enters_panel(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, id, _s) = boot_with_transcript(cx);
    register_subagent(&view, vcx, 1);
    vcx.simulate_keystrokes("cmd-0");
    vcx.run_until_parked();
    let focus = view.read_with(vcx, |v, cx| v.read_session(id, cx, |c| c.focus).unwrap());
    assert_eq!(
        focus,
        crate::AgentFocus::Panel,
        "cmd-0 in an agent tile enters panel focus via the AgentView binding"
    );
}

/// UXI-AgentTile-3: closing the last open panel while panel-focused auto-exits (you
/// can never be panel-focused with no panel open).
#[gpui::test]
fn agent_panel_closing_last_panel_exits_focus(cx: &mut TestAppContext) {
    let (view, vcx, id, _s) = boot_with_transcript(cx);
    register_subagent(&view, vcx, 1);
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.focus = crate::AgentFocus::Transcript);
    });
    view.update(vcx, |v, cx| v.focus_agent_panel(cx));
    assert_eq!(
        view.read_with(vcx, |v, cx| v.read_session(id, cx, |c| c.focus).unwrap()),
        crate::AgentFocus::Panel
    );
    // Subagents is the only open panel; closing it must drop panel focus back to
    // the captured Transcript focus.
    view.update(vcx, |v, cx| v.toggle_subagents(cx));
    assert_eq!(
        view.read_with(vcx, |v, cx| v.read_session(id, cx, |c| c.focus).unwrap()),
        crate::AgentFocus::Transcript,
        "closing the last panel exits panel focus to the captured focus"
    );
}

// ── Keybindings reference + rebind tile (App::Keymap) ──────────────────────────

/// The `DEFAULT_BINDINGS` table is internally consistent: every action name
/// resolves via `build_action`, every context predicate parses, and every
/// keystroke string parses. This is what makes `register_keymap` = the table
/// truthful — a typo'd action (e.g. `OpenKeymap` never added to `actions!`)
/// would be silently skipped by `apply` and this guard catches it.
#[gpui::test]
fn keymap_registry_table_is_valid(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    cx.update(|app| {
        let reg = crate::KeymapRegistry::defaults();
        let bad = reg.validate(app);
        assert!(bad.is_empty(), "invalid keymap table entries: {bad:?}");
        assert!(
            reg.entries.len() > 100,
            "the ported table should hold the full keymap, got {}",
            reg.entries.len()
        );
    });
}

/// A rebind persists and reloads: mutate an entry, `persist`, then `load` a
/// fresh registry and see the override survive — while an unrelated entry stays
/// at its default. Also the negative half: an empty/garbage keystroke is
/// rejected (`rebind` returns false, entry unchanged).
#[gpui::test]
fn keymap_rebind_persists_and_reloads(_cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("keymap-overrides.json");
    crate::persist::with_keymap_overrides_path(path, || {
        let mut reg = crate::KeymapRegistry::defaults();
        let idx = reg
            .entries
            .iter()
            .find(|e| e.action == "ScrollDown" && e.default_keystrokes == "j")
            .expect("ScrollDown/j must exist")
            .idx;

        // Negative: garbage / empty keys are refused, entry stays default.
        assert!(!reg.rebind(idx, ""), "empty keys must be rejected");
        assert_eq!(reg.entry(idx).unwrap().keystrokes, "j");

        // Positive: a valid rebind takes, persists, and reloads.
        assert!(reg.rebind(idx, "y"), "valid keys must be accepted");
        assert!(reg.entry(idx).unwrap().is_changed());
        reg.persist();

        let reloaded = crate::KeymapRegistry::load();
        assert_eq!(
            reloaded.entry(idx).unwrap().keystrokes,
            "y",
            "the override must survive a reload"
        );
        // An untouched entry is still its default after reload.
        let quit = reloaded
            .entries
            .iter()
            .find(|e| e.action == "Quit")
            .unwrap();
        assert_eq!(quit.keystrokes, "cmd-q");
        assert!(!quit.is_changed());
    });
}

/// Conflict detection: two entries with the SAME keystrokes in overlapping
/// contexts are reported; the same keystrokes in disjoint contexts are not.
#[gpui::test]
fn keymap_conflict_detection(_cx: &mut TestAppContext) {
    let mut reg = crate::KeymapRegistry::defaults();
    // Pick a global entry and rebind it to collide with another global one.
    let zoom_in = reg
        .entries
        .iter()
        .find(|e| e.action == "ZoomIn" && e.default_keystrokes == "cmd-=")
        .unwrap()
        .idx;
    // `cmd-q` (Quit, global) already exists — colliding onto it must be flagged.
    assert!(reg.rebind(zoom_in, "cmd-q"));
    let conflicts = reg.conflicts(zoom_in);
    assert!(
        !conflicts.is_empty(),
        "cmd-q collision in the global context must be reported"
    );

    // A disjoint-context reuse is NOT a conflict: `j` exists in YaldaView,
    // BrowserView, and RailView independently — none should conflict with each
    // other (they can never both be active).
    let yalda_j = reg
        .entries
        .iter()
        .find(|e| e.action == "ScrollDown" && e.context == Some("YaldaView"))
        .unwrap()
        .idx;
    let browser_conflicts: Vec<usize> = reg
        .conflicts(yalda_j)
        .into_iter()
        .filter(|&i| reg.entry(i).map(|e| e.context) == Some(Some("BrowserView")))
        .collect();
    assert!(
        browser_conflicts.is_empty(),
        "j in YaldaView must not conflict with j in BrowserView"
    );
}

/// REAL PATH: open a Keymap tile, drive the actual key handler (filter → return
/// to browse → begin rebind → capture a chord → commit) via `simulate_keystrokes`,
/// and assert the live registry entry changed. Exercises `handle_keymap_key`,
/// the capture keyboard-grab, and the commit path the user's keystrokes run.
#[gpui::test]
fn keymap_rebind_via_real_keystrokes(cx: &mut TestAppContext) {
    use crate::App;
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);

    // Swap the focused tile to the keybindings sheet.
    view.update(vcx, |v, cx| v.open_keymap_inner(cx));
    vcx.run_until_parked();
    assert!(
        view.read_with(vcx, |v, _| v.workspace.focused_content().is_some()),
        "a focused tile must exist"
    );

    // Precondition: the outline-rail toggle is at its default.
    let keys_of = |view: &gpui::Entity<YaldaGpuiView>,
                   vcx: &mut gpui::VisualTestContext,
                   action: &str| {
        view.read_with(vcx, |v, _| {
            v.keymap_registry
                .entries
                .iter()
                .find(|e| e.action == action)
                .map(|e| e.keystrokes.clone())
                .unwrap()
        })
    };
    assert_eq!(keys_of(&view, vcx, "ToggleOutlineRail"), "cmd-shift-o");

    // Filter down to the unique "outline" row (cursor lands on it), return to
    // browse, begin a rebind, capture `y`, and commit.
    vcx.simulate_keystrokes("/ o u t l i n e enter r y enter");
    vcx.run_until_parked();

    assert_eq!(
        keys_of(&view, vcx, "ToggleOutlineRail"),
        "y",
        "rebinding through the real key handler must update the live registry"
    );
    // The tile must remain an App::Keymap (never silently become a buffer).
    assert!(matches!(
        view.read_with(vcx, |v, _| matches!(
            v.workspace.focused_content(),
            Some(App::Keymap(_))
        )),
        true
    ));
}

/// The Keymap body is a cached child: an unrelated root notify leaves its render
/// count flat, while moving its own browse cursor busts it. Mirrors the
/// `linear_*_is_render_flat` / `transcript_021_*` perf guards.
#[gpui::test]
fn keymap_body_is_cached_and_self_invalidates(cx: &mut TestAppContext) {
    crate::perf_reset("keymap");
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    view.update(vcx, |v, cx| v.open_keymap_inner(cx));
    vcx.run_until_parked();
    let base = crate::perf_render_count("keymap");
    assert!(base >= 1, "the keymap body must paint once after the tile opens");

    // Moving the browse cursor (a body-owned mutation) busts the cached body —
    // the mutation-site notify is the only thing that re-renders it.
    let vw = view
        .read_with(vcx, |v, _| v.keymap_focused_view())
        .expect("keymap body must exist");
    vw.update(vcx, |kv, c| {
        kv.move_cursor(1, 50);
        c.notify();
    });
    vcx.run_until_parked();
    let after_move = crate::perf_render_count("keymap");
    assert!(
        after_move > base,
        "moving the cursor must re-render the keymap body (base {base}, got {after_move})"
    );

    // An unrelated root repaint must NOT re-render the cached body.
    view.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    assert_eq!(
        crate::perf_render_count("keymap"),
        after_move,
        "a root-only notify must not re-render the cached keymap body"
    );
}

// ── Session recap (recap-panel, UXI-AgentTile-15) ──────────────────────────────────
//
// A recap is an LLM prose summary of ONE agent session, keyed by `SessionId`
// (`self.recaps`) and rendered INSIDE that session's agent tile above the
// subagents/tasks panels — re-runnable and dismissed. The live generation runs
// on a throwaway subprocess (dev-system § Verification harness gap 2) which
// `spawn_recap_worker` skips under `cfg(test)`; these tests drive the REAL entry
// points — the menu dispatch (`recap-session` / `recap-dismiss`) and the reducer
// methods the pump calls (`apply_recap_event` / `finalize_recap`) — so the panel
// state machine (UXI-AgentTile-15 property 2) is fully covered headlessly.

/// Seed the focused session's transcript so a recap has something to summarize.
fn seed_recap_transcript(
    session: &gpui::Entity<crate::AgentSession>,
    vcx: &mut gpui::VisualTestContext,
) {
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "user: implement the recap\nagent: added the panel\n");
        cx.notify();
    });
    vcx.run_until_parked();
}

/// Summoning a recap (REAL menu entry `recap-session`) pins a `Generating` recap
/// keyed to the FOCUSED session.
///
/// Negative control: revert the `RecapStatus::Generating` insert in
/// `start_recap_for` (e.g. skip the `self.recaps.insert`) and `.expect("recap
/// present")` fails RED — no panel was pinned.
#[gpui::test]
fn recap_summon_sets_generating(cx: &mut TestAppContext) {
    let (view, vcx, id, session) = boot_with_transcript(cx);
    seed_recap_transcript(&session, vcx);
    let sess_label = session.update(vcx, |s, _| s.label.clone());

    view.update(vcx, |v, cx| v.dispatch_menu_command("recap-session", cx));

    let (status, label) = view.update(vcx, |v, _| {
        let r = v.recaps.get(&id).expect("recap present after summon");
        (r.status.clone(), r.session_label.clone())
    });
    assert_eq!(
        status,
        crate::RecapStatus::Generating,
        "summon must flip the panel to Generating"
    );
    assert_eq!(label, sess_label, "recap targets the focused session's label");
}

/// Summoning with an EMPTY transcript is a no-op with a status note — nothing to
/// recap, no panel pinned.
#[gpui::test]
fn recap_summon_empty_transcript_is_noop(cx: &mut TestAppContext) {
    let (view, vcx, id, _session) = boot_with_transcript(cx);
    // No seed → transcript empty.
    view.update(vcx, |v, cx| v.dispatch_menu_command("recap-session", cx));
    let present = view.update(vcx, |v, _| v.recaps.contains_key(&id));
    assert!(!present, "empty transcript must not pin a recap");
}

/// Streamed chunks accumulate into the panel through the REAL reducer the pump
/// uses (`apply_recap_event`), and `finalize_recap` flips a non-empty run to
/// `Ready` while preserving the text.
///
/// Negative control: remove the `r.text.push_str(&text)` in `apply_recap_event`
/// and the `contains` asserts fail RED (text never accumulates); or drop the
/// `RecapStatus::Ready` arm in `finalize_recap` and the final status assert
/// fails.
#[gpui::test]
fn recap_chunks_accumulate_and_finalize_ready(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    let (view, vcx, id, session) = boot_with_transcript(cx);
    seed_recap_transcript(&session, vcx);
    view.update(vcx, |v, cx| v.dispatch_menu_command("recap-session", cx));
    let token = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().token);

    view.update(vcx, |v, cx| {
        v.apply_recap_event(id, token, ReplyEvent::Chunk("- Working on the recap\n".into()), cx);
        v.apply_recap_event(id, token, ReplyEvent::Chunk("- Panel added\n".into()), cx);
    });

    let (text, status) = view.update(vcx, |v, _| {
        let r = v.recaps.get(&id).unwrap();
        (r.text.clone(), r.status.clone())
    });
    assert!(
        text.contains("Working on the recap") && text.contains("Panel added"),
        "chunks must accumulate into the recap text (got {text:?})"
    );
    assert_eq!(
        status,
        crate::RecapStatus::Generating,
        "recap stays Generating until finalize"
    );

    view.update(vcx, |v, cx| v.finalize_recap(id, token, cx));
    let (text2, status2) = view.update(vcx, |v, _| {
        let r = v.recaps.get(&id).unwrap();
        (r.text.clone(), r.status.clone())
    });
    assert_eq!(status2, crate::RecapStatus::Ready, "non-empty run finalizes Ready");
    assert_eq!(text2, text, "finalize preserves the accumulated text");
}

/// Finalizing a run that produced no text flips to `Failed` (empty reply).
///
/// Negative control: change `finalize_recap`'s empty branch to always set
/// `Ready` and this asserts the wrong status (fails RED).
#[gpui::test]
fn recap_empty_reply_fails(cx: &mut TestAppContext) {
    let (view, vcx, id, session) = boot_with_transcript(cx);
    seed_recap_transcript(&session, vcx);
    view.update(vcx, |v, cx| v.dispatch_menu_command("recap-session", cx));
    let token = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().token);

    view.update(vcx, |v, cx| v.finalize_recap(id, token, cx));
    let status = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().status.clone());
    assert!(
        matches!(status, crate::RecapStatus::Failed(_)),
        "an empty reply must finalize Failed, got {status:?}"
    );
}

/// Dismissing the recap (REAL menu entry `recap-dismiss`) clears the focused
/// session's panel.
#[gpui::test]
fn recap_dismiss_clears(cx: &mut TestAppContext) {
    let (view, vcx, id, session) = boot_with_transcript(cx);
    seed_recap_transcript(&session, vcx);
    view.update(vcx, |v, cx| v.dispatch_menu_command("recap-session", cx));
    assert!(view.update(vcx, |v, _| v.recaps.contains_key(&id)), "recap pinned");

    view.update(vcx, |v, cx| v.dispatch_menu_command("recap-dismiss", cx));
    assert!(
        view.update(vcx, |v, _| !v.recaps.contains_key(&id)),
        "dismiss must clear the recap panel"
    );
}

/// Re-running supersedes the prior run: the token bumps, the text resets, and a
/// LATE event from the stale run is ignored (token guard, UXI-AgentTile-15 property 2).
///
/// Negative control: drop the `r.token != token` guard in `apply_recap_event`
/// and the stale-chunk assert fails RED (the old run scribbles on the new one).
#[gpui::test]
fn recap_rerun_supersedes_stale_run(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    let (view, vcx, id, session) = boot_with_transcript(cx);
    seed_recap_transcript(&session, vcx);

    view.update(vcx, |v, cx| v.dispatch_menu_command("recap-session", cx));
    let t1 = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().token);
    view.update(vcx, |v, cx| {
        v.apply_recap_event(id, t1, ReplyEvent::Chunk("first run text\n".into()), cx);
    });

    // Re-run (the panel ⟳ path) supersedes.
    view.update(vcx, |v, cx| v.rerun_recap(id, cx));
    let t2 = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().token);
    assert_ne!(t1, t2, "re-run must bump the run token");
    let fresh = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().text.clone());
    assert!(fresh.is_empty(), "re-run resets the accumulated text");

    // A late event tagged with the OLD token must be dropped.
    view.update(vcx, |v, cx| {
        v.apply_recap_event(id, t1, ReplyEvent::Chunk("STALE from run 1\n".into()), cx);
    });
    let after_stale = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().text.clone());
    assert!(
        !after_stale.contains("STALE"),
        "a stale run's chunk must be ignored (got {after_stale:?})"
    );

    // The current run still accepts its own events.
    view.update(vcx, |v, cx| {
        v.apply_recap_event(id, t2, ReplyEvent::Chunk("second run text\n".into()), cx);
    });
    let live = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().text.clone());
    assert!(live.contains("second run text"), "current run accepts its events");
}

/// The recap panel PAINTS inside the agent tile, ABOVE the compose box, when
/// pinned (UXI-AgentTile-15). Asserts real painted geometry via the layout probe:
/// non-vacuous area, and positioned above the compose (its natural slot above
/// the subagents/tasks panels).
///
/// Negative control: remove the `render_agent_recap` call in `render_agent` and
/// `layout_probe_get("recap-panel")` returns `None` (fails RED — nothing
/// painted).
#[gpui::test]
fn recap_panel_paints_in_agent_tile(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    let (view, vcx, id, session) = boot_with_transcript(cx);
    seed_recap_transcript(&session, vcx);
    view.update(vcx, |v, cx| v.dispatch_menu_command("recap-session", cx));
    let token = view.update(vcx, |v, _| v.recaps.get(&id).unwrap().token);
    // Give the panel multi-line visible content so a paint is non-vacuous.
    view.update(vcx, |v, cx| {
        v.apply_recap_event(
            id,
            token,
            ReplyEvent::Chunk("- point one\n- point two\n- point three\n".into()),
            cx,
        );
        v.finalize_recap(id, token, cx);
    });
    // Switch to the chatbox so the compose renders as a pinned box (in the
    // default worksheet state the compose is inline and paints no "compose-box"),
    // giving a stable anchor to prove the recap sits ABOVE it.
    view.update(vcx, |v, cx| v.dispatch_menu_command("agent-input-toggle", cx));
    // Settle geometry.
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let recap = crate::layout_probe_get("recap-panel");
    let compose = crate::layout_probe_get("compose-box");
    crate::layout_probe_end();

    let (_rx, ry, rw, rh) = recap.expect(
        "recap-panel did not paint — the pinned recap is invisible (UXI-AgentTile-15 violated)",
    );
    assert!(rw > 1.0 && rh > 1.0, "recap panel painted with no area (w={rw}, h={rh})");
    // It renders inside the tile, above the compose (its slot above the
    // subagents/tasks panels). The compose box always paints, so this is a real
    // placement check, not a vacuous one.
    let (_cx2, cy, _cw, _ch) = compose.expect("compose box did not paint");
    assert!(
        ry < cy,
        "recap panel must paint ABOVE the compose (recap y={ry}, compose y={cy})"
    );
}
