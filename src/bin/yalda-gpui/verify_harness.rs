//! Verification harness — headless GPUI tests (see docs/dev-system.md § Verification harness).
//!
//! The binding constraint on agent throughput is that the GPUI app can't be
//! driven headlessly, so a human is the verification oracle for every change.
//! GPUI ships a real test harness (`#[gpui::test]` + `TestAppContext`, which
//! simulates platform input and runs the async executor via `run_until_parked`)
//! — this module builds on it to drive `YaldaGpuiView` (open agent, simulate
//! keystrokes, stream synthetic events, assert state) without a display.
//!
//! `test-support` is enabled via the `gpui` dev-dependency, so this compiles
//! only for test builds — the production binary is unaffected.
//!
//! Stones laid here grow toward: (1) end-to-end action smokes, (2) the
//! O(changed) perf gate at realistic transcript size, (3) golden render output.

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

/// Workspace KV registry → agent CWD inheritance (untitled.md Workspace +
/// Agent TODOs). A `"cwd"` written into the active workspace's registry is
/// what `active_workspace_cwd` surfaces — the value an agent session created
/// without an explicit cwd inherits before falling back to the process cwd.
/// Absent / empty keys yield `None` so the call site falls through to
/// `process_cwd`.
#[gpui::test]
fn workspace_kv_cwd_inheritance(cx: &mut TestAppContext) {
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

    // No registry entry → None (caller uses process cwd).
    let none = view.read_with(vcx, |v, _| v.active_workspace_cwd());
    assert_eq!(none, None, "absent cwd key yields None");

    // Write a cwd into the active tab's registry → inherited.
    view.update(vcx, |v, _cx| {
        v.workspace
            .active_tab_mut()
            .expect("active tab")
            .kv_set("cwd", "/tmp/example-ws");
    });
    let got = view.read_with(vcx, |v, _| v.active_workspace_cwd());
    assert_eq!(
        got,
        Some(PathBuf::from("/tmp/example-ws")),
        "registry cwd is inherited"
    );

    // Empty string is treated as unset.
    view.update(vcx, |v, _cx| {
        v.workspace
            .active_tab_mut()
            .expect("active tab")
            .kv_set("cwd", "");
    });
    let empty = view.read_with(vcx, |v, _| v.active_workspace_cwd());
    assert_eq!(empty, None, "empty cwd key yields None");
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
        claude.input_surface = crate::InputSurface::Worksheet;
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
    use crate::{ActiveOverlay, BufferSwitcher, SessionSwitcher};

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
                && v.session_ref().is_none()
                && v.workspace_picker_ref().is_none(),
            "exactly one variant active — mutual exclusion is type-enforced"
        );

        // open REPLACES, never stacks: opening a different overlay drops the
        // previous one (the tab-double-click-behind-menu case can't strand).
        v.open_overlay(ActiveOverlay::SessionSwitcher(SessionSwitcher {
            selected: 0,
        }));
        assert!(v.overlay_is_session());
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
#[gpui::test]
fn input_surface_toggle_round_trips(cx: &mut TestAppContext) {
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

    // New session starts in Chatbox — a box exists.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert!(c.input_surface.is_chatbox());
        assert!(
            c.input_surface.chatbox().is_some(),
            "chatbox exists iff Chatbox variant"
        );
    });

    // Toggle -> Worksheet: the box is gone (no stranded Some).
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).unwrap();
        assert!(!c.input_surface.is_chatbox());
        assert!(
            c.input_surface.chatbox().is_none(),
            "worksheet carries no chatbox"
        );
    });

    // Toggle back -> Chatbox: a fresh box.
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));
    view.update(vcx, |v, cx| {
        assert!(v.agent_mut(cx).unwrap().input_surface.is_chatbox());
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

/// Install an empty agent ring in picker mode on `view`. When `sessions` is
/// non-empty the list is marked loaded; otherwise the picker stays in its
/// "loading" state (`sessions: None`).
#[cfg(test)]
fn install_agent_picker(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    sessions: &[(&str, &str)],
) {
    use crate::{AgentTile, App, PickerSession, SessionPicker};
    let sessions: Vec<PickerSession> = sessions
        .iter()
        .map(|(sid, label)| PickerSession {
            sid: sid.to_string(),
            acp_id: None,
            label: label.to_string(),
            turns: 3,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
        })
        .collect();
    view.update(vcx, |v, cx| {
        let mut tile = AgentTile::new();
        let mut picker = SessionPicker::loading(PathBuf::from("."));
        if !sessions.is_empty() {
            picker.sessions = Some(sessions);
        }
        tile.picker = Some(picker);
        v.set_screen(App::Agent(tile));
    });
}

/// The empty-ring picker renders headlessly in BOTH its loading and loaded
/// states without panicking on `ring.active()`, and `apply_picker_sessions`
/// wires a list result into the focused picker.
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

    // Loading state (sessions: None) renders without panic.
    install_agent_picker(&view, &mut *vcx, &[]);
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        let tile = v.agent_tile().expect("agent tile");
        assert!(tile.bound.is_none(), "tile stays unbound until a row binds");
        let p = tile.picker.as_ref().expect("picker present");
        assert!(p.sessions.is_none(), "still loading");
        assert_eq!(p.row_count(), 1, "only the 'new session' row while loading");
    });

    // Fold in a list result through the real reducer, then render again. The
    // result is addressed to the originating tile by its WindowId (INV-PR).
    let target = view.read_with(vcx, |v, cx| v.workspace.focused_window_id());
    view.update(vcx, |v, cx| {
        v.apply_picker_sessions(
            target,
            PathBuf::from("."),
            Ok((
                vec![
                    crate::PickerSession {
                        sid: "S1".into(),
                        acp_id: None,
                        label: "claude-1".into(),
                        turns: 2,
                        connected: true,
                        permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                    },
                    crate::PickerSession {
                        sid: "S2".into(),
                        acp_id: None,
                        label: "claude-2".into(),
                        turns: 9,
                        connected: false,
                        permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                    },
                ],
                // One bound session — informational column, not a selectable row.
                vec![crate::PickerSession {
                    sid: "S3".into(),
                    acp_id: None,
                    label: "claude-3".into(),
                    turns: 1,
                    connected: true,
                    permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                }],
            )),
            cx,
        );
    });
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        let p = v.agent_tile().unwrap().picker.as_ref().unwrap();
        assert_eq!(
            p.row_count(),
            3,
            "new-session row + two FREE sessions = 3 rows (bound ones don't count)"
        );
        assert_eq!(p.bound.len(), 1, "the bound session is stored separately");
    });
}

/// INV-PR regression: a `list_sessions` result lands on the tile that
/// REQUESTED it (addressed by WindowId), not on whichever tile is focused when
/// the async result arrives. This is the "two restored agent tiles — one hangs
/// on 'loading sessions…' forever while the other fills, and picking in one
/// opens a session in the other" bug. With two unbound picker tiles in a split,
/// we deliver tile A's result while tile B is focused and assert A (not B) is
/// the one that fills.
#[gpui::test]
fn picker_list_result_routes_to_originating_tile_not_focused(cx: &mut TestAppContext) {
    use crate::{AgentTile, App, PickerSession, SessionPicker};
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    let mk_tile = || {
        let mut t = AgentTile::new();
        t.picker = Some(SessionPicker::loading(PathBuf::from(".")));
        t
    };

    // Tile A (focused) gets a loading picker; record its WindowId.
    let win_a = view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(mk_tile()));
        v.workspace.focused_window_id().expect("focused window A")
    });
    // Split off tile B with its own loading picker — focus moves to B.
    let win_b = view.update(vcx, |v, cx| {
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(mk_tile()));
        v.workspace.focused_window_id().expect("focused window B")
    });
    assert_ne!(win_a, win_b, "the split produced two distinct tiles");

    // Deliver tile A's list result WHILE B is focused. The pre-fix reducer
    // routed by `agent_tile_mut()` (focus) and would have filled B and left A
    // loading forever.
    view.update(vcx, |v, cx| {
        v.apply_picker_sessions(
            Some(win_a),
            PathBuf::from("."),
            Ok((
                vec![PickerSession {
                    sid: "S1".into(),
                    acp_id: None,
                    label: "claude-1".into(),
                    turns: 1,
                    connected: true,
                    permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                }],
                vec![],
            )),
            cx,
        );
    });
    vcx.run_until_parked();

    // Read a specific tile's picker-loaded state by WindowId.
    let loaded = |v: &YaldaGpuiView, id: crate::workspace::WindowId| -> Option<bool> {
        for tab in v.workspace.tabs.iter() {
            if let Some(w) = tab.layout.find_leaf(id)
                && let App::Agent(t) = &w.content
            {
                return t.picker.as_ref().map(|p| p.sessions.is_some());
            }
        }
        None
    };
    view.read_with(vcx, |v, cx| {
        assert_eq!(
            loaded(v, win_a),
            Some(true),
            "A's list must land on A — the tile that requested it"
        );
        assert_eq!(
            loaded(v, win_b),
            Some(false),
            "B must be untouched — still loading, NOT hijacked because it holds focus"
        );
    });
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
        },
        SessionSnapshot {
            id: "SID-B".into(),
            label: "claude-B".into(),
            active: false,
            mode: InputModeKind::Worksheet,
            tasklist_open: true,
            subagents_open: false,
            cwd: cwd.clone(),
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

/// (a) A chatbox keystroke re-renders the root chrome + compose, but the
/// transcript's render() is SKIPPED: its count stays FLAT. This is finding #1
/// (chatbox keystroke re-lays-out the static transcript) closed.
#[gpui::test]
fn transcript_021_chatbox_keystroke_is_render_flat(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (_view, vcx, _id, session) = boot_with_transcript(cx);

    // First real frame: render_agent runs, creates + renders the transcript.
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
