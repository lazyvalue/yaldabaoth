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

// NEXT STONE (not yet built): action-level smokes (e.g. simulate "cmd-b",
// assert the rail opens beside the focused pane). Blocked on a small
// enablement refactor — the keymap is currently registered inline in `main()`'s
// run-closure (`app.bind_keys([...])`), so a test window has no bindings. Extract
// a `register_keymap(app: &mut App)` callable from both `main()` and the harness,
// then `vcx.simulate_keystrokes("cmd-b")` will dispatch through real actions.
