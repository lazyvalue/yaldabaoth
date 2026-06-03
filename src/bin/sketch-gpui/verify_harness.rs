//! Verification harness — headless GPUI tests (see docs/dev-system.md § Verification harness).
//!
//! The binding constraint on agent throughput is that the GPUI app can't be
//! driven headlessly, so a human is the verification oracle for every change.
//! GPUI ships a real test harness (`#[gpui::test]` + `TestAppContext`, which
//! simulates platform input and runs the async executor via `run_until_parked`)
//! — this module builds on it to drive `SketchGpuiView` (open agent, simulate
//! keystrokes, stream synthetic events, assert state) without a display.
//!
//! `test-support` is enabled via the `gpui` dev-dependency, so this compiles
//! only for test builds — the production binary is unaffected.
//!
//! Stones laid here grow toward: (1) end-to-end action smokes, (2) the
//! O(changed) perf gate at realistic transcript size, (3) golden render output.

#![cfg(test)]

use gpui::{AppContext, TestAppContext};

use crate::SketchGpuiView;
use sketch::theme::Theme;
use std::path::PathBuf;

/// Stone 1: prove GPUI's `test-support` harness wires up in this crate — boot a
/// `TestAppContext` and round-trip an entity through `update`/`read`. If this
/// runs, the headless-driver path is open and we can build up to constructing a
/// real window + `SketchGpuiView` and simulating input.
#[gpui::test]
fn harness_boots(cx: &mut TestAppContext) {
    let value = cx.update(|cx| {
        let entity = cx.new(|_cx| 41u32);
        *entity.read(cx)
    });
    assert_eq!(value, 41, "TestAppContext entity round-trip");
}

/// Stone 2: construct the REAL `SketchGpuiView` in a headless test window and
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
        SketchGpuiView::new_browser(
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
    assert!(readable, "real SketchGpuiView constructs + renders headlessly");
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
        let mut v = SketchGpuiView::new_browser(
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
    let (cold_recomputed, cold_skip) = view.update(vcx, |v, _cx| v.test_edit_cache_stats());
    assert!(
        cold_recomputed >= N,
        "cold paint should highlight all {N} lines, got {cold_recomputed}"
    );
    assert!(!cold_skip, "cold paint is not a skip");

    // --- No-edit re-render: a notify with no buffer mutation must fast-skip
    //     (edit_seq unchanged) and recompute zero lines. ---
    view.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    let (idle_recomputed, idle_skip) = view.update(vcx, |v, _cx| v.test_edit_cache_stats());
    assert_eq!(
        idle_recomputed, 0,
        "no-change re-render must recompute 0 lines (fast skip), got {idle_recomputed}"
    );
    assert!(idle_skip, "no-change re-render must take the fast-skip path");

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
    let (edit_recomputed, edit_skip) = view.update(vcx, |v, _cx| v.test_edit_cache_stats());
    assert!(!edit_skip, "an edit must not fast-skip");
    assert_eq!(
        edit_recomputed, 1,
        "a single-char edit into a {N}-line doc must re-highlight exactly 1 line, got {edit_recomputed}"
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
        let mut v = SketchGpuiView::new_browser(
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
    SketchGpuiView::test_reset_doc_block_builds();
    view.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    let cold = SketchGpuiView::test_doc_block_builds();
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
    SketchGpuiView::test_reset_doc_block_builds();
    view.update(vcx, |v, cx| {
        if let Some(d) = v.doc_mut() {
            d.cursor_block = 1500; // jump into the middle of the doc
        }
        cx.notify();
    });
    vcx.run_until_parked();
    let after_move = SketchGpuiView::test_doc_block_builds();
    assert!(
        after_move <= VISIBLE_CEILING,
        "doc render after a cursor-block move must stay O(visible) (<= {VISIBLE_CEILING}), got {after_move}"
    );
}

// NEXT STONE (not yet built): action-level smokes (e.g. simulate "cmd-b",
// assert the rail opens beside the focused pane). Blocked on a small
// enablement refactor — the keymap is currently registered inline in `main()`'s
// run-closure (`app.bind_keys([...])`), so a test window has no bindings. Extract
// a `register_keymap(app: &mut App)` callable from both `main()` and the harness,
// then `vcx.simulate_keystrokes("cmd-b")` will dispatch through real actions.
