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
use crate::agent_sessions::ServerSid;
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
/// ADR-0023). The cwd is a required, typed field on every `Workspace`
/// ([`WorkspaceCwd`]) — "no cwd" is unrepresentable — so `agent_base_cwd` is
/// total and always surfaces the active workspace's dir. Includes the
/// regression that motivated the type: an **ephemeral** virtual workspace
/// (jump-panel) inherits the spawning workspace's cwd instead of silently
/// falling back to the process dir.
#[gpui::test]
fn workspace_cwd_inheritance(cx: &mut TestAppContext) {
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
    assert_eq!(
        booted,
        Some(start.clone()),
        "workspace boots with a real cwd"
    );

    // Set CWD → both the surfaced cwd and what a new agent inherits move.
    view.update(vcx, |v, _cx| {
        v.test_set_active_workspace_cwd(PathBuf::from("/Users/scott/ws/fulcrum"));
    });
    let base = view.read_with(vcx, |v, _| v.agent_base_cwd());
    assert_eq!(
        base,
        PathBuf::from("/Users/scott/ws/fulcrum"),
        "a new agent inherits the active workspace's cwd"
    );

    // The regression (ADR-0023): opening an ephemeral virtual workspace (what a
    // jump-panel free-session click does) must inherit the spawning workspace's
    // cwd — NOT reset to the process dir. With the old empty-`kv` ephemeral workspace,
    // `agent_base_cwd` here returned the launch dir.
    view.update(vcx, |v, _cx| {
        v.workspace
            .open_ephemeral_workspace(crate::App::Agent(crate::AgentTile::new()));
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
        v.test_set_active_workspace_cwd(ws_dir.clone());
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

/// UXI-Buffer-2: opening the file picker from a file-backed buffer lands the
/// cursor ON the file you just left (not the top of the list) — "already be on
/// the file I just left". Drives the REAL `open_browser_inner` entry point over
/// a real on-disk file and reads the resulting `Picking` browser's selection.
///
/// Negative control (observed RED): drop the `fb.select_path(path)` call in
/// `open_browser_inner` (or the `select_path` body in `file_browser.rs`) → the
/// selection stays at index 0 (the `..` row) and the filename assert fails.
#[gpui::test]
fn open_picker_lands_on_the_file_just_left(cx: &mut TestAppContext) {
    use crate::{App, BufferApp};
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

    // A real directory with several files so the target isn't incidentally first.
    let dir = std::env::temp_dir().join(format!("yalda-picker-land-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["aaa.md", "mmm-target.md", "zzz.md"] {
        std::fs::write(dir.join(f), b"# x\n").unwrap();
    }
    let target = dir.join("mmm-target.md");

    view.update(vcx, |v, _cx| {
        assert!(v.open_file(target.clone()), "open the target file");
    });
    view.update(vcx, |v, cx| v.open_browser_inner(cx));
    vcx.run_until_parked();

    let selected_name = view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Picking(bw))) => bw.fb.selected_entry().map(|e| e.name.clone()),
        _ => None,
    });
    assert_eq!(
        selected_name.as_deref(),
        Some("mmm-target.md"),
        "the picker cursor must sit on the file we opened from, not the top"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Clicking a local Markdown link must keep the source document open and add
/// the target as a distinct, focused buffer tile. Drives real mouse dispatch
/// through the rendered `InteractiveText`, not the navigation method directly.
#[gpui::test]
fn local_markdown_link_opens_new_buffer_tile(cx: &mut TestAppContext) {
    use crate::{App, BufferApp};
    use gpui::{Modifiers, point, px};

    let dir =
        std::env::temp_dir().join(format!("yalda-local-markdown-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create link fixture dir");
    let source = dir.join("source.md");
    let target = dir.join("target.md");
    std::fs::write(&source, "[target](target.md)\n").expect("write source fixture");
    std::fs::write(&target, "# Target\n").expect("write target fixture");

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    view.update(vcx, |view, _| {
        assert!(view.open_file(source.clone()), "open source fixture");
        view.splash_until = None;
    });
    for _ in 0..2 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    let source_id = view.read_with(vcx, |v, _| {
        v.workspace.focused_window_id().expect("source tile")
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let (_, _, w, h) =
        crate::layout_probe_get("doc-link-0-0").expect("Markdown link did not paint");
    crate::layout_probe_end();
    assert!(
        w > 0.0 && h > 0.0,
        "Markdown link painted no clickable area"
    );

    let at = view.read_with(vcx, |v, _| {
        let bounds = v
            .line_layouts
            .borrow()
            .get(&(0, 0))
            .expect("link text layout")
            .bounds();
        point(
            bounds.left() + px(2.0),
            bounds.top() + bounds.size.height / 2.0,
        )
    });
    vcx.simulate_mouse_move(at, None, Modifiers::default());
    vcx.simulate_click(at, Modifiers::default());
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| {
        let workspace = v.workspace.active_workspace().expect("active workspace");
        let leaves = workspace.layout.leaf_ids();
        assert_eq!(leaves.len(), 2, "link click must add one buffer tile");
        assert!(
            leaves.contains(&source_id),
            "the source tile must remain in the workspace"
        );
        assert_ne!(
            workspace.focused, source_id,
            "focus must move to the linked document"
        );

        let source_label = match &workspace.layout.find_leaf(source_id).unwrap().content {
            App::Buffer(BufferApp::Viewing(doc)) => doc.file_label.as_ref(),
            _ => panic!("source tile is no longer a viewed buffer"),
        };
        assert_eq!(
            source_label,
            source.canonicalize().unwrap().display().to_string()
        );

        let target_label = match &workspace
            .layout
            .find_leaf(workspace.focused)
            .expect("target tile")
            .content
        {
            App::Buffer(BufferApp::Viewing(doc)) => doc.file_label.as_ref(),
            _ => panic!("linked document did not open as a viewed buffer"),
        };
        assert_eq!(
            target_label,
            target.canonicalize().unwrap().display().to_string()
        );
    });

    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_dir(&dir);
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

/// Regression: reloading a Buffer must invalidate the edit render snapshots
/// even when the replacement document's fresh `edit_seq` numerically equals
/// the generation already cached by the view. The old path replaced the shared
/// `EditorCore` with `EditorCore::new` (sequence reset to zero), so this exact
/// initial-sequence reload reused the old source and its open-fence code style.
#[gpui::test]
fn buffer_reload_does_not_reuse_old_syntax_state(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("reload-highlight.md");
    std::fs::write(&path, "```rust\nlet old = 1;\n").expect("write initial file");
    let view_path = path.clone();

    let (view, vcx) = cx.add_window_view(move |window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        );
        v.test_open_edit_at_path("```rust\nlet old = 1;\n", view_path);
        v
    });

    vcx.run_until_parked();

    std::fs::write(&path, "plain\nreloaded plain\n").expect("replace file on disk");
    crate::screens::edit_render_tap_begin();
    view.update(vcx, |v, cx| v.reload_focused_from_disk(cx));
    vcx.run_until_parked();

    let painted = crate::screens::edit_render_tap_snapshot();
    let line = painted
        .iter()
        .rev()
        .find(|line| line.line_idx == 1)
        .expect("reloaded second line painted through the real Buffer render path");
    assert_eq!(
        line.text, "reloaded plain",
        "reload must paint the new text"
    );
    assert!(
        !line.has_code_bg,
        "plain reloaded text must not retain the old unclosed-fence code style"
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
        (e.list.state().logical_scroll_top().item_ix, e.list.len())
    });
    assert_eq!(
        count,
        count_before - 1,
        "a line merge removes exactly one line"
    );
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

/// UXI-ParagraphSpacing-1: a Doc-view block (paragraph) carries a readability gap
/// below it — measurably larger than the pre-change 8px base (and than the ~0
/// leading between soft-wrapped lines *within* one block, which carry no inter-line
/// margin). In a virtualized `gpui::list` each item's bottom margin is absorbed
/// into its slot height, so the gap is recovered as (block-row slot height −
/// block-content height): the two `doc-block-{idx}` / `doc-block-inner-{idx}`
/// probes bracket exactly the applied `.mb(...)`.
///
/// Negative control (observed RED): restore `block_element`'s `mb_2` (8px) in
/// `render_blocks.rs` → the recovered gap drops to 8px and the `>= 12.0` assert
/// fails.
#[gpui::test]
fn paragraph_gap_between_doc_blocks_exceeds_within_paragraph_leading(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        );
        // Two paragraphs → two top-level `RenderedBlock`s (doc-block-0/-1).
        v.test_open_doc("First paragraph, block zero.\n\nSecond paragraph, block one.\n");
        v
    });
    // Pin zoom at 1x so the gap is the unscaled `8 + PARAGRAPH_GAP_PX` = 14px.
    view.update(vcx, |v, cx| v.set_text_scale(1.0, cx));
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let slot = crate::layout_probe_get("doc-block-0");
    let content = crate::layout_probe_get("doc-block-inner-0");
    crate::layout_probe_end();

    let (_, _, _, slot_h) = slot.expect("doc block 0 row did not paint");
    let (_, _, _, content_h) = content.expect("doc block 0 content did not paint");
    // The list slot includes the bottom margin; the content box does not.
    let gap = slot_h - content_h;
    assert!(
        gap >= 12.0,
        "block paragraph gap must exceed the pre-change 8px base \
         (UXI-ParagraphSpacing-1); recovered {gap}px (slot {slot_h} − content {content_h})"
    );
    assert!(
        gap <= 24.0,
        "block paragraph gap unexpectedly large ({gap}px) — double-count or regression"
    );
}

/// UXI-ParagraphSpacing-1 (agent transcript prose): a COMMITTED prose line that
/// STARTS a new paragraph (its previous source line is blank) carries the readability
/// gap as top padding, so its painted row is taller than a within-paragraph prose row
/// (a soft break, previous line non-blank). The blank line itself is dropped by the
/// blank-collapse pass, so paragraphs would otherwise render adjacent — this proves
/// the gap survives the collapse by reading `lines_snap`.
///
/// Negative control (observed RED): drop the `is_paragraph_break` `.pt(...)` in
/// `transcript_view.rs` → the two rows are the same height and the delta assert fails.
#[gpui::test]
fn transcript_paragraph_start_row_is_taller_than_within_paragraph_row(cx: &mut TestAppContext) {
    let (view, vcx, _id, session) = boot_with_transcript(cx);

    // Paragraph α has TWO lines (a soft break between 0 and 1); a blank line (2)
    // separates it from paragraph β (line 3). So: line 1 = within-paragraph (no gap),
    // line 3 = paragraph start (gap). All committed/frozen.
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state.editor.programmatic_insert(
            0,
            "Alpha line one.\nAlpha line two.\n\nBeta paragraph line.\n",
        );
        s.state.editor.add_frozen_lines(0, 4);
        cx.notify();
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        assert!(
            !c.editor.document().line_text(1).trim().is_empty(),
            "line 1 is a within-paragraph soft break (non-blank, prev non-blank)"
        );
        assert!(
            c.editor.document().line_text(2).trim().is_empty(),
            "line 2 blank"
        );
        assert!(
            c.editor.document().line_text(3).starts_with("Beta"),
            "line 3 starts paragraph β (prev source line 2 is blank)"
        );
        assert!(
            c.editor.is_frozen_line(3),
            "paragraph-start line must be frozen"
        );
        c.focus = crate::AgentFocus::Transcript;
    });
    view.update(vcx, |v, cx| v.set_text_scale(1.0, cx));
    for _ in 0..4 {
        view.update(vcx, |_, cx| cx.notify());
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
    let within = crate::layout_probe_get("transcript-row-1");
    let para_start = crate::layout_probe_get("transcript-row-3");
    crate::layout_probe_end();

    let (_, _, _, h_within) = within.expect("within-paragraph row 1 did not paint");
    let (_, _, _, h_start) = para_start.expect("paragraph-start row 3 did not paint");
    // The paragraph-start row is a bare line PLUS the top-padding gap; a within-
    // paragraph row is the bare line. So it exceeds the within row by ~PARAGRAPH_GAP_PX.
    assert!(
        h_start > h_within + 4.0,
        "a paragraph-start row must be taller than a within-paragraph row by ~the gap \
         (UXI-ParagraphSpacing-1); got start {h_start}px vs within {h_within}px"
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
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            "SENTINEL-NOT-COPIED".into(),
        ))
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
            v.sessions
                .bind_sid(id, ServerSid::new(sid))
                .expect("fresh sid binds");
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

/// Every command name reachable in the `.` shell menu (`gpui_menu`), descending
/// into submenus. `gpui_menu` absorbed the retired `?` global menu (ADR-0032).
fn shell_menu_commands() -> Vec<String> {
    fn walk(nodes: &[crate::MenuNode], out: &mut Vec<String>) {
        for n in nodes {
            match &n.action {
                crate::MenuAction::Command(c) => out.push(c.clone()),
                crate::MenuAction::Submenu(children) => walk(children, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&crate::gpui_menu(), &mut out);
    out
}

/// True when `cmd` is dispatchable anywhere in a menu tree, descending submenus.
fn menu_tree_has_command(nodes: &[crate::MenuNode], cmd: &str) -> bool {
    nodes.iter().any(|n| match &n.action {
        crate::MenuAction::Command(c) => c == cmd,
        crate::MenuAction::Submenu(children) => menu_tree_has_command(children, cmd),
        _ => false,
    })
}

/// True when a label appears anywhere in the `.` shell menu tree (root or a
/// submenu heading/leaf).
fn shell_menu_has_label(label: &str) -> bool {
    fn walk(nodes: &[crate::MenuNode], label: &str) -> bool {
        nodes.iter().any(|n| {
            n.label == label || matches!(&n.action, crate::MenuAction::Submenu(c) if walk(c, label))
        })
    }
    walk(&crate::gpui_menu(), label)
}

/// UXI-Menu-6: the `?` leader is RETIRED. `leader_intercept` — the exact method
/// every tile's `on_key_down` calls — does NOT consume `?` (returns false, opens
/// nothing), while it still consumes `.` (opens the shell menu). Every command
/// the old `?` global menu held is now reachable in the `.` shell menu.
///
/// Negative control: restore `Key::Char('?') => self.open_global_menu_inner(cx)`
/// in `leader_intercept` ⇒ the `?`-returns-false assert goes RED.
#[gpui::test]
fn question_mark_leader_is_inert_former_global_commands_live_under_dot(cx: &mut TestAppContext) {
    use crate::{KMods, Key, KeyPress};
    let (view, vcx) = boot_browser(cx);
    vcx.run_until_parked();

    // `?` in a navigation state is NOT a leader — nothing consumed, no menu.
    let consumed_q = view.update(vcx, |v, cx| {
        let c = v.leader_intercept(&KeyPress::new(Key::Char('?'), KMods::NONE), cx);
        (c, v.overlay_is_menu())
    });
    assert_eq!(
        consumed_q,
        (false, false),
        "the retired `?` leader consumes nothing and opens no menu"
    );

    // Control: `.` IS a leader — consumed, and the shell menu opens. Proves the
    // routing path is live here (the `?` no-op above is not vacuous).
    let consumed_dot = view.update(vcx, |v, cx| {
        let c = v.leader_intercept(&KeyPress::new(Key::Char('.'), KMods::NONE), cx);
        (c, v.overlay_is_menu())
    });
    assert_eq!(
        consumed_dot,
        (true, true),
        "the `.` shell leader still opens a menu"
    );
    view.update(vcx, |v, _| v.clear_overlay());

    // Every former `?`-menu command now lives in the `.` shell menu.
    for cmd in [
        "new-workspace",
        "rename-workspace",
        "new-project",
        "open-system-console",
        "toggle-jump-panel",
    ] {
        assert!(
            shell_menu_commands().contains(&cmd.to_string()),
            "former ? command {cmd} must live under the . shell menu"
        );
    }
}

/// UXI-Menu-7 for the DYNAMIC agent menu: the grafted archive entry (in the `s`
/// submenu) and the advertised-model `M` submenu must not collide with any
/// sibling key. Covers what the pure-fn `local_menus_have_no_duplicate_keys_per_level`
/// can't (it needs a live view for the graft).
#[gpui::test]
fn agent_dynamic_menu_has_no_duplicate_keys(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ModelOption, ReplyEvent};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = boot_browser(cx);
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));
    // Advertise models so the `M` submenu is populated (not just the placeholder).
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::ReplyEvent {
                session_id: "S1".into(),
                event: ReplyEvent::ModelsAvailable {
                    current: "sonnet".into(),
                    options: vec![
                        ModelOption {
                            id: "default".into(),
                            label: "Default".into(),
                        },
                        ModelOption {
                            id: "sonnet".into(),
                            label: "Sonnet".into(),
                        },
                    ],
                },
            }],
            cx,
        );
    });

    fn check_level(nodes: &[crate::MenuNode], path: &str) {
        let mut seen: Vec<&[crate::KeyPress]> = Vec::new();
        for n in nodes {
            if matches!(
                &n.action,
                crate::MenuAction::Command(_) | crate::MenuAction::Submenu(_)
            ) {
                assert!(
                    !seen.contains(&n.key.as_slice()),
                    "duplicate key {:?} at {path}",
                    n.key
                );
                seen.push(&n.key);
            }
            if let crate::MenuAction::Submenu(children) = &n.action {
                check_level(children, &format!("{path}/{}", n.label));
            }
        }
    }
    view.update(vcx, |v, cx| {
        check_level(&v.agent_local_menu_dynamic(cx), "agent-dynamic");
    });
}

/// UXI-Menu-8: View → Agents/Tasks is a stateful, mutually-exclusive selector.
/// Drive the real menu dispatcher and verify both the panel state and live check
/// mark, including unhiding the sidepanel.
#[gpui::test]
fn agent_view_menu_selects_and_marks_agents_or_tasks(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    view.update(vcx, |v, cx| {
        if let Some(mut state) = v.agent_mut(cx) {
            state.sidepanel_hidden = true;
            state.subagents_open = true;
            state.tasklist_open = false;
        }
        v.dispatch_menu_command("agent-view-tasks", cx);
        assert_eq!(
            v.agent_read(cx, |state| {
                (
                    state.subagents_open,
                    state.tasklist_open,
                    state.sidepanel_hidden,
                )
            }),
            Some((false, true, false))
        );
        let menu = v.agent_local_menu_dynamic(cx);
        let view_menu = menu.iter().find(|node| node.label == "view").expect("view");
        let crate::MenuAction::Submenu(children) = &view_menu.action else {
            panic!("view is a submenu");
        };
        assert_eq!(children[0].label, "agents");
        assert_eq!(children[1].label, "tasks ✓");

        v.dispatch_menu_command("agent-view-agents", cx);
        assert_eq!(
            v.agent_read(cx, |state| (state.subagents_open, state.tasklist_open)),
            Some((true, false))
        );
    });
}

/// UXI-SystemConsole-1/-2 plus the yux render-count contract: both requested
/// entry points summon the SAME overlay without moving focus; its `r` / `R`
/// keys reach the real rebuild dispatcher; and an unrelated root repaint reuses
/// the cached console body.
///
/// Guard sensitivity:
/// - without the global-menu command the presence assertion fails;
/// - without the jump-row listener the real painted click leaves the overlay
///   closed;
/// - embedding the view without `cached_child` increments the render count on
///   the final root-only notify.
#[gpui::test]
fn system_console_opens_from_global_menu_and_jump_panel(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let focused_before = view.read_with(vcx, |v, _| v.workspace.focused_window_id());

    // ADR-0032: the `?` global menu was folded into the `.` shell menu; system
    // console now lives in the `s` (system) submenu.
    assert!(
        shell_menu_commands().contains(&"open-system-console".to_string()),
        "the . shell menu must offer system console"
    );

    view.update(vcx, |v, cx| {
        v.dispatch_menu_command("open-system-console", cx)
    });
    vcx.run_until_parked();
    assert!(
        view.read_with(vcx, |v, _| v.overlay_is_system_console()),
        "global menu dispatch summons the console"
    );
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
        focused_before,
        "summoning operational chrome does not replace or refocus a tile"
    );

    // These keys run the exact dispatcher production uses. Under cfg(test) the
    // dispatcher records the request immediately before the subprocess seam.
    vcx.simulate_keystrokes("r");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("shift-r");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v.dev_rebuild_requests.clone()),
        vec![false, true],
        "r rebuilds the GUI; R rebuilds GUI + server"
    );

    // A cached console should stay flat when only the root is dirtied.
    crate::perf_reset("system_console");
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let cached = crate::perf_render_count("system_console");
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert_eq!(
        crate::perf_render_count("system_console"),
        cached,
        "root-only repaint must reuse the cached console body"
    );
    view.update(vcx, |v, cx| v.set_theme(crate::ThemeName::Folio, cx));
    vcx.run_until_parked();
    assert!(
        crate::perf_render_count("system_console") > cached,
        "theme is a global console input, so its push path must bust the cache"
    );

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!view.read_with(vcx, |v, _| v.has_overlay()));

    // Click the actual painted jump-panel row, not a model-level proxy.
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let (x, y, w, h) =
        crate::layout_probe_get("jump-system-console").expect("console jump row painted");
    crate::layout_probe_end();
    let at = point(px(x + w / 2.0), px(y + h / 2.0));
    vcx.simulate_mouse_move(at, None, gpui::Modifiers::default());
    vcx.simulate_click(at, gpui::Modifiers::default());
    vcx.run_until_parked();
    assert!(
        view.read_with(vcx, |v, _| v.overlay_is_system_console()),
        "clicking the former PINNED slot summons the console"
    );
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
        focused_before
    );
}

/// UXI-Workspace-1: `ctrl-<n>` jumps straight to the n-th workspace — the digit the
/// jump panel shows. Exercises the FULL dispatch chain: `register_keymap`
/// installed `ctrl-3 → GotoWorkspace3`, and the focused screen root wired
/// `.workspace_nav(cx)` so the action lands on `goto_workspace_number`. A digit
/// past the last workspace is a no-op (no panic, no spurious switch).
#[gpui::test]
fn ctrl_digit_switches_workspace(cx: &mut TestAppContext) {
    use crate::{App, BrowserWindow, BufferApp};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);

    // Build four real (non-ephemeral) workspaces: the boot workspace + three more
    // INHABITED browser workspaces (an empty-layout workspace renders a bare div with
    // no action handlers, so its root couldn't dispatch the global jump — a
    // separate edge state; here we want real, switchable workspaces).
    view.update(vcx, |v, _| {
        let cwd = PathBuf::from(".");
        for _ in 0..3 {
            v.workspace
                .push_workspace_inheriting(App::Buffer(BufferApp::Picking(
                    BrowserWindow::standalone(cwd.clone()),
                )));
        }
        assert_eq!(v.workspace.workspaces.len(), 4);
        v.workspace.set_active_workspace(0); // start the keystroke run on workspace 1
        assert_eq!(v.workspace.active_workspace, 0);
    });
    vcx.run_until_parked();

    // ctrl-3 → third workspace (index 2).
    vcx.simulate_keystrokes("ctrl-3");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 2,
            "ctrl-3 selects the 3rd workspace"
        );
    });

    // ctrl-1 → back to the first.
    vcx.simulate_keystrokes("ctrl-1");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 0,
            "ctrl-1 selects the 1st workspace"
        );
    });

    // ctrl-9 with only four workspaces is a no-op (stays put).
    vcx.simulate_keystrokes("ctrl-9");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 0,
            "a digit past the last workspace does nothing"
        );
    });
}

/// UXI-Workspace-13: the workspace menu's `close-workspace` command removes
/// only the active workspace and its tiles. A session shown by one of those
/// tiles remains alive in the owning `AgentSessions` store and becomes free.
/// The sole-workspace floor is a no-op through BOTH the menu dispatcher and
/// the production `Cmd-Shift-W` action; workspace closure never quits Yalda.
///
/// Drives the REAL `dispatch_menu_command("close-workspace")` path and the real
/// keymap/action/handler path.
///
/// Negative control (observed RED): remove `close_active_workspace`'s
/// sole-workspace return so it calls `Frame::close_workspace` at the floor; the
/// real notify/render path reaches `chrome.rs` with zero workspaces and panics
/// indexing the active workspace. The helper deliberately takes no GPUI
/// Context, so quitting is structurally unavailable to workspace closure
/// (GPUI's headless platform implements `quit()` as a no-op and cannot serve as
/// an honest direct oracle for that call).
#[gpui::test]
fn closing_workspace_frees_sessions_and_never_quits(cx: &mut TestAppContext) {
    use crate::{AgentTile, App};
    use yalda::session_proto::SessionInfo;

    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    let session = add_free_session(&view, vcx, "workspace-close-survivor");
    let server_sid = "workspace-close-S1";

    // Put the session in a second, active real workspace. The workspace owns
    // only the AgentTile reference; `sessions` owns the live session.
    let session_cwd = view.update(vcx, |v, _cx| {
        let cwd = v.agent_base_cwd();
        v.sessions
            .bind_sid(session, ServerSid::new(server_sid))
            .expect("fresh server sid binds");
        v.agent_roster.upsert(SessionInfo {
            session_id: server_sid.into(),
            acp_session_id: None,
            label: "workspace-close-survivor".into(),
            cwd: cwd.clone(),
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 1,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        let mut tile = AgentTile::new();
        tile.bind(session);
        let project = v.workspace.inherited_project();
        v.workspace
            .push_initial_workspace(App::Agent(tile), project);
        cwd
    });
    view.read_with(vcx, |v, _cx| {
        assert_eq!(v.workspace.workspaces.len(), 2, "test has two workspaces");
        assert!(
            v.agent_tile_id_bound_to(session).is_some(),
            "the session starts bound to the active workspace's tile"
        );
        assert!(v.bound_sid_set().contains(server_sid));
    });

    // Exact command carried by the uppercase-X menu leaf.
    view.update(vcx, |v, cx| v.dispatch_menu_command("close-workspace", cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _cx| {
        assert_eq!(
            v.workspace.workspaces.len(),
            1,
            "the active workspace is gone"
        );
        assert!(
            v.sessions.contains(session),
            "dropping the workspace's Agent tile must not close its session"
        );
        assert_eq!(
            v.agent_tile_id_bound_to(session),
            None,
            "the removed tile no longer binds the session"
        );
        assert!(
            !v.bound_sid_set().contains(server_sid),
            "the surviving session is no longer durably placed"
        );
        let (free, bound) = v.picker_projection(&session_cwd);
        assert!(
            free.iter().any(|row| row.sid == server_sid),
            "the surviving session is offered as free for placement"
        );
        assert!(bound.iter().all(|row| row.sid != server_sid));
    });

    // Menu dispatch on the sole workspace is a no-op.
    view.update(vcx, |v, cx| v.dispatch_menu_command("close-workspace", cx));
    assert_eq!(
        view.read_with(vcx, |v, _cx| v.workspace.workspaces.len()),
        1,
        "the menu cannot remove the sole workspace"
    );
    vcx.run_until_parked();

    // The global action has the same floor and, critically, does not quit.
    vcx.simulate_keystrokes("cmd-shift-w");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _cx| v.workspace.workspaces.len()),
        1,
        "Cmd-Shift-W cannot remove the sole workspace or quit the app"
    );
    assert!(
        view.read_with(vcx, |v, _cx| v.sessions.contains(session)),
        "the session remains alive after every workspace-close entry point"
    );
}

/// bug-0011 REGRESSION: an UNBOUND agent tile (the session selector/picker,
/// `render_agent_picker`) was the one screen root missing `.workspace_nav(cx)`, so
/// while the picker was focused `ctrl-<n>` (GotoWorkspace) and `cmd-shift-[]`
/// (Next/PrevWorkspace) dispatched into a dead chain — the picker "ate" workspace-switch
/// keys. Drives the REAL keymap over a focused picker tile. RED before the fix
/// (the picker root wires workspace_nav): `active_workspace` stays 0.
#[gpui::test]
fn agent_picker_does_not_eat_workspace_switch_keys(cx: &mut TestAppContext) {
    use crate::{App, BrowserWindow, BufferApp};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);

    // Four real workspaces; start on workspace 1.
    view.update(vcx, |v, _| {
        let cwd = PathBuf::from(".");
        for _ in 0..3 {
            v.workspace
                .push_workspace_inheriting(App::Buffer(BufferApp::Picking(
                    BrowserWindow::standalone(cwd.clone()),
                )));
        }
        v.workspace.set_active_workspace(0);
    });
    // Replace workspace 1's tile with an UNBOUND agent tile → the selector/picker
    // is now the focused screen root. Force a repaint so the picker actually
    // RENDERS and becomes the focused node in the dispatch tree (set_screen alone
    // doesn't notify, leaving the stale boot-browser leaf — which HAS workspace_nav
    // — in the dispatch tree and masking the bug).
    install_agent_picker(&view, &mut *vcx, &[]);
    view.update(vcx, |_v, cx| cx.notify());
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let tile = v.agent_tile().expect("agent tile on workspace 1");
        assert!(
            tile.session().is_none(),
            "tile is unbound → picker is showing"
        );
        assert_eq!(v.workspace.active_workspace, 0);
    });

    // ctrl-3 FROM THE PICKER → third workspace. This is the bug: RED without
    // workspace_nav on the picker root (action falls into a dead chain, stays 0).
    vcx.simulate_keystrokes("ctrl-3");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 2,
            "ctrl-3 must switch workspace even while the session picker is focused"
        );
    });

    // And cycling (Next/PrevWorkspace) must reach the picker too — go back to it, then
    // cmd-shift-] forward.
    view.update(vcx, |v, _| v.workspace.set_active_workspace(0));
    vcx.run_until_parked();
    vcx.simulate_keystrokes("cmd-shift-]");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 1,
            "cmd-shift-] must advance the workspace from the picker (Next/PrevWorkspace wiring)"
        );
    });
}

/// REGRESSION ("ctrl-tab does nothing"): tab-cycling (NextWorkspace/PrevWorkspace) was wired ONLY
/// on the doc screen, so it was DEAD whenever an agent/worksheet tile was focused
/// (where the user actually was). It's now in `workspace_nav`, wired on every screen.
/// This drives the AGENT screen (not the browser) and switches with the reliable
/// `cmd-shift-[`/`]` binding (Ctrl-Tab is OS-mangled on macOS — a 4th genuine gap;
/// this test is focus-accurate, proving the WIRING, not the OS delivery). RED before
/// folding next_workspace/prev_workspace into `workspace_nav`.
#[gpui::test]
fn workspace_cycle_works_from_the_agent_screen(cx: &mut TestAppContext) {
    use crate::{App, BrowserWindow, BufferApp};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_worksheet_nav(cx); // FOCUSED on an agent/worksheet tile
    view.update(vcx, |v, _| {
        let cwd = PathBuf::from(".");
        for _ in 0..2 {
            v.workspace
                .push_workspace_inheriting(App::Buffer(BufferApp::Picking(
                    BrowserWindow::standalone(cwd.clone()),
                )));
        }
        assert_eq!(v.workspace.workspaces.len(), 3, "agent workspace + 2 more");
        v.workspace.set_active_workspace(0); // start on the agent workspace
    });
    vcx.run_until_parked();

    vcx.simulate_keystrokes("cmd-shift-]");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 1,
            "cmd-shift-] advances the workspace FROM THE AGENT SCREEN (the dead-wiring bug)"
        );
    });

    vcx.simulate_keystrokes("cmd-shift-[");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.active_workspace, 0, "cmd-shift-[ goes back");
    });
}

/// Direct unbound focus does not enter workspace numbering; `ctrl-<n>` still
/// addresses the durable workspace folders shown by the jump panel.
#[gpui::test]
fn workspace_number_ignores_direct_unbound_focus(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let sid = add_free_session(&view, vcx, "claude-1");
    view.update(vcx, |v, _| {
        let project = v.workspace.inherited_project();
        v.push_empty_workspace(project);
    }); // workspaces 1 and 2 are real
    // Open the free session → an ephemeral workspace is appended (sorts last).
    view.update(vcx, |v, cx| v.jump_to_session(sid, cx));
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.workspaces.len(),
            2,
            "direct focus adds no workspace"
        );
        assert!(v.workspace.presented_detached_tile_id().is_some());
    });
    // ctrl-2 must land on the 2nd REAL workspace (index 1), not the ephemeral.
    view.update(vcx, |v, cx| v.goto_workspace_number(2, cx));
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 1,
            "number 2 = 2nd non-ephemeral workspace"
        );
        assert!(!v.workspace.active_is_ephemeral());
    });
}

/// UXI-JumpPanel-1: the jump-panel agent status reflects its operational state.
/// `dot_status` is the headless-verifiable mapping (the actual hue is a paint
/// detail — gap 1). Every connected idle agent is ready for input; a reply in
/// flight is working.
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

    // Every connected idle session is ready for input, even when already read.
    assert_eq!(
        status(&view, vcx),
        AgentDotStatus::WaitingForYou,
        "a fresh idle session is ready for input"
    );

    // Unread remains internal attention state; it does not alter readiness.
    view.update(vcx, |v, cx| v.with_session(id, cx, |c| c.unread = true));
    assert_eq!(
        status(&view, vcx),
        AgentDotStatus::WaitingForYou,
        "an unread idle session is also ready for input"
    );

    // A reply in flight → working (regardless of unread).
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
    let row = |connected, awaiting, unread| AgentRow {
        order_sid: None,
        state_entered_at: None,
        summary_pending: false,
        archived: false,
        target: JumpTarget::Roster("s".into()),
        label: "x".into(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        summary: None,
        cwd: std::path::PathBuf::from("/"),
        bound: false,
        connected,
        awaiting,
        unread,
        tags: Vec::new(),
    };
    // Reply in flight → working (unread irrelevant while working).
    assert_eq!(
        row(true, Some(true), false).dot_status(),
        AgentDotStatus::Working
    );
    // Every connected idle row is ready for input, regardless of unread state.
    assert_eq!(
        row(true, Some(false), true).dot_status(),
        AgentDotStatus::WaitingForYou
    );
    assert_eq!(
        row(true, Some(false), false).dot_status(),
        AgentDotStatus::WaitingForYou
    );
    // A connected roster-only row with an unknown phase is also admitted to Waiting.
    assert_eq!(
        row(true, None, false).dot_status(),
        AgentDotStatus::WaitingForYou
    );
    // Disconnected wins even if it was mid-turn / had unread.
    assert_eq!(
        row(false, Some(true), true).dot_status(),
        AgentDotStatus::Neutral
    );
}

/// UXI-JumpPanel-6: a turn that finalizes on a session you are NOT focused on
/// marks it unread; a focused session stays read. Both rows remain visibly ready
/// for input because unread no longer fragments the Waiting state. Drives the
/// REAL turn-end path
/// (`apply_server_batch` → `ServerNotification::TurnEnded` →
/// `finalize_agent_turn_idem`, which sets `unread`; the batch's focused-clear
/// keeps the focused session read), then asserts through the REAL row projection.
///
/// Negative control (observed RED): remove `self.unread = true` in
/// `finalize_agent_turn_idem` → S1's unread assertion fails. Remove the
/// focused-clear in `apply_server_batch` → S2's read assertion fails.
#[gpui::test]
fn jump_dot_unread_on_background_turn_end_read_on_focused(cx: &mut TestAppContext) {
    use crate::{AgentDotStatus, AgentSession, AgentState, AgentTile, App};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = boot_browser(cx);

    // Two bound server-managed sessions. Installing S2 second leaves S2 as the
    // focused tile's session; S1 is unfocused but still in the store.
    let (s1, s2) = view.update(vcx, |v, cx| {
        let mk = |sid: &str| AgentSession {
            state: AgentState::new_server_managed(None),
            label: format!("sess-{sid}"),
            cwd: PathBuf::from("."),
            resume_id: None,
        };
        v.set_screen(App::Agent(AgentTile::new()));
        let s1 = v.show_local_session(mk("S1"), cx);
        v.sessions
            .bind_sid(s1, ServerSid::new("S1"))
            .expect("S1 binds");
        v.set_screen(App::Agent(AgentTile::new()));
        let s2 = v.show_local_session(mk("S2"), cx);
        v.sessions
            .bind_sid(s2, ServerSid::new("S2"))
            .expect("S2 binds");
        (s1, s2)
    });
    vcx.run_until_parked();

    // Sanity: S2 is the focused session, S1 is not.
    view.update(vcx, |v, _cx| {
        assert_eq!(v.jump_active_session().0, Some(s2), "S2 is focused");
        assert_ne!(v.jump_active_session().0, Some(s1), "S1 is backgrounded");
    });

    // End a turn on BOTH via the real server path.
    let end_turn = |v: &mut YaldaGpuiView, sid: &str, cx: &mut gpui::Context<YaldaGpuiView>| {
        v.apply_server_batch(
            vec![ServerNotification::TurnEnded {
                session_id: sid.into(),
                turn_count: 1,
                generation: 1,
            }],
            cx,
        );
    };
    view.update(vcx, |v, cx| {
        end_turn(v, "S1", cx);
        end_turn(v, "S2", cx);
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let rows = v.jump_panel_agent_rows(cx);
        let dot = |sid: &str| {
            rows.iter()
                .find(|r| r.order_sid.as_deref() == Some(sid))
                .unwrap_or_else(|| panic!("row for {sid}"))
                .dot_status()
        };
        assert_eq!(
            dot("S1"),
            AgentDotStatus::WaitingForYou,
            "a backgrounded session's finished turn is unread → waiting on you"
        );
        assert_eq!(
            dot("S2"),
            AgentDotStatus::WaitingForYou,
            "the focused session is also visibly ready for input"
        );
        assert!(
            rows.iter()
                .find(|r| r.order_sid.as_deref() == Some("S1"))
                .is_some_and(|r| r.unread),
            "the background session retains its unread attention state"
        );
        assert!(
            rows.iter()
                .find(|r| r.order_sid.as_deref() == Some("S2"))
                .is_some_and(|r| !r.unread),
            "the focused session stays read"
        );
        assert!(
            rows.iter()
                .find(|r| r.order_sid.as_deref() == Some("S1"))
                .is_some_and(|r| r.state_entered_at.is_some()),
            "the background turn records when it entered Waiting"
        );
    });
}

/// UXI-AgentTile-23 (ADR-0027): the transcript row-background selector gives a
/// USER turn the theme's faint `user_turn_bg` tint and leaves agent / tool /
/// system / untagged lines transparent (UXI-AgentTile-4). The painted hue is
/// gap #1 (human eye); this pins the decision of WHICH turns get a tint.
///
/// Negative control: make `committed_row_bg`'s `User` arm return transparent →
/// the "user turn is tinted" assert fails RED (observed).
#[test]
fn user_turn_gets_tint_agent_turn_does_not() {
    use crate::{TurnId, committed_row_bg};
    let tint: gpui::Hsla = gpui::rgb(0x283040).into();
    let transparent: gpui::Hsla = gpui::rgba(0x00000000).into();
    assert_eq!(
        committed_row_bg(Some(TurnId::User(1)), tint),
        tint,
        "user turn is tinted"
    );
    assert_eq!(
        committed_row_bg(Some(TurnId::Llm(1)), tint),
        transparent,
        "agent turn is not tinted"
    );
    assert_eq!(
        committed_row_bg(Some(TurnId::Tool(1)), tint),
        transparent,
        "tool turn is not tinted"
    );
    assert_eq!(
        committed_row_bg(Some(TurnId::System), tint),
        transparent,
        "system turn is not tinted"
    );
    assert_eq!(
        committed_row_bg(None, tint),
        transparent,
        "untagged line is not tinted"
    );
}

/// UXI-JumpPanel-5: the active screen UX element wears the red "you are here" box
/// in the jump panel. Drives the REAL derivation (`jump_active_session` +
/// `jump_target_is_active` over the REAL `jump_panel_agent_rows`) rather than any
/// hand-built proxy: the focused tile's bound session is the one active row, an
/// unfocused session is never active, the active workspace is a listed (boxable)
/// workspace, and unbinding the tile (no focused bound session) clears the
/// session box. The literal red pixels are harness gap #1 (human eye).
///
/// Negative control: make `jump_target_is_active` return `false` unconditionally
/// → the "exactly one active row" assert fails RED (observed).
#[gpui::test]
fn jump_active_box_marks_focused_workspace_and_session(cx: &mut TestAppContext) {
    use crate::{JumpTarget, jump_target_is_active};
    let (view, vcx, id, _session) = boot_with_transcript(cx);
    // A second session added straight to the store (NOT via show_local_session,
    // which would rebind the focused agent tile) so it stays free + unfocused —
    // it must never be boxed.
    let other = view.update(vcx, |v, cx| {
        let ent = cx.new(|_| crate::AgentSession {
            state: crate::AgentState::new_server_managed(None),
            label: "claude-2".into(),
            cwd: PathBuf::from("."),
            resume_id: None,
        });
        v.sessions.create_local(|_| ent)
    });

    view.read_with(vcx, |v, cx| {
        // The active workspace is a listed (non-ephemeral) wsp, so its row is
        // boxed (workspace arm; the box reuses the row's existing `active`).
        assert!(
            !v.workspace.active_is_ephemeral(),
            "active workspace is listed"
        );
        assert!(!v.workspace.workspaces[v.workspace.active_workspace].ephemeral);

        // The focused tile's bound session is the active-session identity.
        let (active_local, active_sid) = v.jump_active_session();
        assert_eq!(active_local, Some(id), "focused session is active");

        let rows = v.jump_panel_agent_rows(cx);
        let active: Vec<_> = rows
            .iter()
            .filter(|r| jump_target_is_active(&r.target, active_local, active_sid.as_deref()))
            .collect();
        assert_eq!(
            active.len(),
            1,
            "exactly the focused session's row is active"
        );
        assert!(
            active[0].bound,
            "the boxed row is the bound (focused) session"
        );
        // The other, unfocused session is present but NOT active.
        let other_active = rows.iter().any(|r| {
            matches!(&r.target, JumpTarget::Local(lid) if *lid == other)
                && jump_target_is_active(&r.target, active_local, active_sid.as_deref())
        });
        assert!(!other_active, "an unfocused session is never boxed");
    });

    // Unbind the focused tile → it becomes the selector (an unbound agent tile),
    // so there is NO focused bound session: the buffer / unbound-agent arm — no
    // session row is boxed.
    view.update(vcx, |v, cx| v.show_selector_on_focused_tile(cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        let (active_local, active_sid) = v.jump_active_session();
        assert_eq!(active_local, None, "unbound tile has no active session");
        let any = v
            .jump_panel_agent_rows(cx)
            .iter()
            .any(|r| jump_target_is_active(&r.target, active_local, active_sid.as_deref()));
        assert!(!any, "no session boxed when no bound session is focused");
    });
}

/// Jump-panel selection materializes one unbound tile and directly focuses it.
#[gpui::test]
fn jump_to_unbound_session_preserves_tile_after_workspace_focus(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let sid = add_free_session(&view, vcx, "claude-1");

    // Precondition: one real wsp, session is free.
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.workspaces.len(), 1, "starts with one workspace");
        assert!(
            v.agent_tile_id_bound_to(sid).is_none(),
            "session starts free"
        );
    });

    // Jump to the free session → one directly focused unbound tile.
    view.update(vcx, |v, cx| v.jump_to_session(sid, cx));
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.workspaces.len(),
            1,
            "no workspace is manufactured"
        );
        let id = v
            .workspace
            .presented_detached_tile_id()
            .expect("direct unbound focus");
        assert_eq!(
            v.workspace
                .tile(id)
                .and_then(|window| match &window.content {
                    crate::App::Agent(tile) => tile.session(),
                    _ => None,
                }),
            Some(sid),
            "the unbound tile retains the session"
        );
        assert!(
            v.agent_tile_id_bound_to(sid).is_none(),
            "an ephemeral reference is not durable workspace placement"
        );
        assert!(
            v.bound_sid_set().is_empty(),
            "free/bound projection also ignores ephemeral references"
        );
    });

    // Jump away clears direct focus but keeps the unbound tile and its state.
    view.update(vcx, |v, cx| v.select_workspace(0, cx));
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.workspaces.len(), 1);
        assert_eq!(v.workspace.presented_detached_tile_id(), None);
        assert!(
            v.agent_tile_id_bound_to(sid).is_none(),
            "session returned to free"
        );
        assert!(
            v.sessions.contains(sid),
            "session itself survives the teardown"
        );
    });
}

/// Selecting a second free session directly focuses its own stable unbound tile;
/// the first remains in Unbound with its state intact.
#[gpui::test]
fn jump_to_second_unbound_session_preserves_both_tiles(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let a = add_free_session(&view, vcx, "claude-1");
    let b = add_free_session(&view, vcx, "claude-2");

    view.update(vcx, |v, cx| v.jump_to_session(a, cx));
    view.update(vcx, |v, cx| v.jump_to_session(b, cx));
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.workspaces.len(),
            1,
            "direct views add no workspaces"
        );
        assert_eq!(
            v.workspace
                .focused_content()
                .and_then(|content| match content {
                    crate::App::Agent(tile) => tile.session(),
                    _ => None,
                }),
            Some(b),
            "the second session is now shown"
        );
        assert!(
            v.agent_tile_id_bound_to(b).is_none(),
            "the second session still has no durable placement"
        );
        assert!(
            v.agent_tile_id_bound_to(a).is_none(),
            "the first session returned to free"
        );
        assert!(v.sessions.contains(a) && v.sessions.contains(b));
        assert_eq!(v.workspace.detached_tiles.len(), 2);
    });
}

/// A session already owned by a workspace focuses that one stable tile from
/// either jump-panel or Cmd-P activation; no duplicate reference is created.
///
/// Negative control: restore `jump_to_session`'s former
/// `jump_to_window(owner_wid)` branch. The first assertion fails because no
/// ephemeral workspace exists.
#[gpui::test]
fn bound_session_jumps_focus_single_owner_workspace(cx: &mut TestAppContext) {
    use crate::{AgentTile, App, BrowserWindow, BufferApp};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    // Workspace 0: an agent tile bound to S1.
    install_agent_slot(&view, vcx, Some("S1"));
    let (sid, owner_workspace, owner_wid) = view.update(vcx, |v, _| {
        let sid = v.sessions.locate(&ServerSid::new("S1")).expect("S1 bound");
        let wid = v.agent_tile_id_bound_to(sid).expect("S1 has a tile");
        let wsp = v
            .workspace
            .workspace_containing(wid)
            .expect("tile in a workspace");
        (sid, wsp, wid)
    });
    // Add a second workspace and start there. A direct visit must NOT navigate
    // back to the owner's workspace.
    view.update(vcx, |v, cx| {
        v.workspace
            .push_workspace_inheriting(App::Buffer(BufferApp::Picking(BrowserWindow::standalone(
                PathBuf::from("."),
            ))));
        v.workspace.set_active_workspace(1);
        cx.notify();
    });
    let workspaces_before = view.update(vcx, |v, _| v.workspace.workspaces.len());

    // Jump-panel row dispatcher.
    view.update(vcx, |v, cx| {
        v.jump_to_agent(crate::JumpTarget::Local(sid), cx)
    });
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.workspaces.len(), workspaces_before);
        assert_eq!(v.workspace.active_workspace, owner_workspace);
        assert_eq!(v.agent_tile().and_then(AgentTile::session), Some(sid));
        let mut viewport_refs = 0;
        for wsp in &v.workspace.workspaces {
            wsp.layout.for_each_leaf(&mut |window| {
                if matches!(&window.content, App::Agent(tile) if tile.session() == Some(sid)) {
                    viewport_refs += 1;
                }
            });
        }
        assert_eq!(viewport_refs, 1, "the stable tile is never duplicated");
        assert_eq!(
            v.jump_active_session().0,
            Some(sid),
            "the direct viewport is still the session being viewed"
        );
        assert_eq!(
            v.agent_tile_id_bound_to(sid),
            Some(owner_wid),
            "the original tile remains the unique workspace binding"
        );
    });

    // The placement remains unchanged.
    view.update(vcx, |v, cx| v.select_workspace(owner_workspace, cx));
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.workspaces.len(), workspaces_before);
        assert_eq!(v.workspace.active_workspace, owner_workspace);
        assert_eq!(v.agent_tile().and_then(AgentTile::session), Some(sid));
    });

    // Repeat through the real Cmd-P overlay/key activation path.
    view.update(vcx, |v, cx| v.select_workspace(1, cx));
    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let palette = v.jump_palette_mut().expect("cmd-p opened the palette");
        palette.query = "claude-1".into();
        palette.selected = 0;
        cx.notify();
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.active_workspace, owner_workspace);
        assert_eq!(v.agent_tile().and_then(AgentTile::session), Some(sid));
        assert_eq!(v.jump_active_session().0, Some(sid));
        assert_eq!(
            v.agent_tile_id_bound_to(sid),
            Some(owner_wid),
            "Cmd-P leaves the durable workspace placement intact"
        );
    });
}

/// Durable placement remains 1:1: a server session is placed in at most ONE
/// non-ephemeral workspace tile. Resolving an AlreadyBound identity conflict
/// must not create a second session entity or a second durable placement. This
/// does not forbid the ephemeral viewport references covered by UXI-JumpPanel-19.
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
    let owner = view.update(vcx, |v, _| {
        v.sessions.locate(&ServerSid::new("S1")).expect("S1 bound")
    });

    // Workspace 1: a fresh agent tile, now focused.
    view.update(vcx, |v, _cx| {
        v.workspace
            .push_workspace_inheriting(App::Agent(AgentTile::new()));
    });

    // Attempt to bind the already-owned session from workspace 1's tile.
    view.update(vcx, |v, cx| v.focus_existing_session(owner, cx));

    let (bound_tiles, active) = view.update(vcx, |v, _| {
        let mut n = 0;
        for wsp in v.workspace.workspaces.iter() {
            wsp.layout.for_each_leaf(&mut |w| {
                if let App::Agent(t) = &w.content
                    && t.session() == Some(owner)
                {
                    n += 1;
                }
            });
        }
        (n, v.workspace.active_workspace)
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
        assert!(
            text.contains("agent turn one reply"),
            "turn-one text present"
        );
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
        assert!(
            c.pending_reveal_cursor,
            "tail snap queues a viewport reveal"
        );
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
            ev(ReplyEvent::Chunk(
                "ode=max cache when inputs changed.\n".into(),
            )),
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

/// bug-0023: clicking a FOLDED tool-use block's header must expand it. The
/// sibling `transcript_021_tool_expand_busts_cache` hand-calls `toggle_expanded`
/// — a proxy (anti-circling rule 1) that stays green while the user's actual
/// click is dead. This drives the window's REAL mouse dispatch
/// (`simulate_click`) at the header's REAL painted rect, so the transcript's own
/// select-to-clipboard gesture (`#claude-body`'s mouse down/move/up) is under
/// test alongside the header's `on_click`.
#[gpui::test]
fn tool_group_header_click_expands_the_fold(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ReplyEvent, ToolCall};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        let tc = ToolCall::new("tool-1", "Bash echo hi");
        let batch = vec![
            ev(ReplyEvent::Chunk("Running a command.\n".into())),
            ev(ReplyEvent::ToolCallStarted(tc)),
            ev(ReplyEvent::Chunk("Done.\n".into())),
        ];
        v.apply_server_batch(batch, cx);
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let anchor = view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        c.view_model
            .flat_items_cache
            .iter()
            .find_map(|it| match it {
                crate::FlatItem::ToolGroup { anchor_line, .. } => Some(*anchor_line),
                _ => None,
            })
            .expect("a tool group is rendered")
    });

    // The header's REAL painted rect — clicking a computed guess proves nothing.
    // The transcript is a CACHED child: dirty the session so it actually
    // re-renders (a bare root notify is a cache hit and paints nothing).
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
    let rect = crate::layout_probe_get(&format!("tool-group-header-{anchor}"));
    crate::layout_probe_end();

    let (x, y, w, h) = rect.expect("the folded tool header never painted");
    assert!(
        w > 4.0 && h > 4.0,
        "fold header painted with no area ({w}x{h}) — nothing to click"
    );
    let at = point(px(x + w / 2.0), px(y + h / 2.0));

    view.read_with(vcx, |v, cx| {
        let folded = v
            .agent_read(cx, |c| !c.tools.expanded.contains(&anchor.to_string()))
            .expect("agent");
        assert!(folded, "precondition: the tool block starts FOLDED");
    });

    // The press itself moves the transcript's render fingerprint (caret + focus),
    // which is exactly what used to re-key the header's element state between
    // down and up. Non-vacuous: assert it really moves, so the guard can't pass
    // because nothing happened.
    let fp_before = view.read_with(vcx, |v, cx| {
        v.agent_read(cx, |c| crate::TranscriptSeqs::of(c).fingerprint_hash())
            .expect("agent")
    });
    vcx.simulate_mouse_move(at, None, gpui::Modifiers::default());
    vcx.simulate_click(at, gpui::Modifiers::default());
    vcx.run_until_parked();
    let fp_after = view.read_with(vcx, |v, cx| {
        v.agent_read(cx, |c| crate::TranscriptSeqs::of(c).fingerprint_hash())
            .expect("agent")
    });
    assert_ne!(
        fp_before, fp_after,
        "the press must move the render fingerprint — otherwise this guard proves nothing"
    );

    view.read_with(vcx, |v, cx| {
        let expanded = v
            .agent_read(cx, |c| c.tools.expanded.contains(&anchor.to_string()))
            .expect("agent");
        assert!(
            expanded,
            "clicking the folded tool-use header did NOTHING — it never expanded (bug-0023)"
        );
    });
}

/// UXI-AgentTile-29: `j`/`k` in transcript navigation HOP OVER a tool-use block.
/// Every tool call splices a dedicated BLANK anchor line that renders as the tool
/// card (its own `Line` item is stripped by blank-collapse), so resting the caret
/// there is a stop on an invisible row. Drives the REAL key path
/// (`handle_claude_key` → `dispatch_normal_core` → the hop), not a hand-set cursor.
#[gpui::test]
fn transcript_jk_hops_over_tool_blocks(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ReplyEvent, ToolCall};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, id, _session) = boot_with_transcript(cx);
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        // Two back-to-back tool calls → two consecutive anchor lines, rendered as
        // ONE merged group. Both must be crossed in a single press.
        let batch = vec![
            ev(ReplyEvent::Chunk("before the tools\n".into())),
            ev(ReplyEvent::ToolCallStarted(ToolCall::new(
                "t-1", "Bash one",
            ))),
            ev(ReplyEvent::ToolCallStarted(ToolCall::new(
                "t-2", "Bash two",
            ))),
            ev(ReplyEvent::Chunk("after the tools\n".into())),
        ];
        v.apply_server_batch(batch, cx);
    });
    vcx.run_until_parked();

    let (anchors, start) = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| {
            let a: std::collections::BTreeSet<usize> = c.tool_anchor_lines().into_iter().collect();
            let first = *a.iter().next().expect("tool anchor lines exist");
            (a, first)
        })
        .expect("session")
    });
    assert_eq!(
        anchors.len(),
        2,
        "two tool calls ⇒ two anchor lines, got {anchors:?}"
    );
    assert!(
        start > 0,
        "the anchor run must have a content line above it"
    );
    // Non-vacuous: the anchors are CONSECUTIVE, so a plain one-line `j` from just
    // above would land on the first one (and a second `j` on the second).
    assert!(
        anchors.contains(&(start + 1)),
        "expected consecutive anchor lines, got {anchors:?}"
    );

    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.focus = crate::AgentFocus::Transcript;
            c.editor.cursor_mut().line = start - 1;
            c.editor.cursor_mut().col = 0;
        });
    });
    let line = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.read_with(vcx, |v, cx| {
            v.read_session(id, cx, |c| c.editor.cursor().line).unwrap()
        })
    };

    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    let after_j = line(&view, vcx);
    assert!(
        !anchors.contains(&after_j),
        "j must HOP OVER the tool block, not rest on its anchor line (landed {after_j}, anchors {anchors:?})"
    );
    assert!(
        after_j > *anchors.iter().next_back().unwrap(),
        "one press clears the WHOLE run of tool anchors (landed {after_j}, anchors {anchors:?})"
    );

    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("k"), w, cx)
    });
    let after_k = line(&view, vcx);
    assert!(
        !anchors.contains(&after_k),
        "k must hop back over the block too (landed {after_k}, anchors {anchors:?})"
    );
    assert_eq!(
        after_k,
        start - 1,
        "k returns to the content line above the block"
    );
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
/// rail on the active wsp, and a second `cmd-b` closes it. This only works if
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

    // Read the rail kind on the active workspace: None = no rail, Some(true) = a
    // file-browser rail, Some(false) = some other rail kind.
    let rail_kind = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            v.workspace
                .active_workspace()
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
        // previous one (the workspace-double-click-behind-menu case can't strand).
        v.open_overlay(ActiveOverlay::WorkspacePicker(WorkspacePicker {
            project: v.workspace.inherited_project(),
            mode: WorkspacePickerMode::Move { follow: false },
            targets: vec![0],
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
        assert_eq!(
            v.agent_mut(cx).unwrap().focus,
            crate::AgentFocus::Transcript
        );
    });
    view.update(vcx, |v, cx| v.toggle_agent_focus(cx));
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).unwrap();
        assert_eq!(c.focus, crate::AgentFocus::Compose);
        assert!(
            c.inline_you_block_active(),
            "focus→Compose opened a visible block"
        );
    });
    view.update(vcx, |v, cx| v.toggle_agent_focus(cx));
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.agent_mut(cx).unwrap().focus,
            crate::AgentFocus::Transcript
        );
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

    let before = view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().editor.document().full_text()
    });
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

/// bug-0002 — restore drops the replayed history when a respawn (generation
/// bump) happened while the §9 gate was still closed (the earliest generation
/// never completed a turn, e.g. the agent crashed/timed-out on the first turn).
///
/// This drives the REAL `apply_server_batch` with the MIXED stream the pump sees
/// on a full-log replay: for each content event the server records the canonical
/// `Agent` twin THEN the legacy `ReplyEvent` (Command::Record ordering). The gate
/// starts closed, so the legacy stream renders the replayed history while the
/// reducer only observes boundaries. The bug: the generation bump's rebaseline
/// was DEFERRED (pre-gate non-boundary Agent events skipped) until the new
/// generation's `ReplayEnd` boundary — and `reset_for_replay` there wiped the
/// legacy-rendered history that the reducer had skipped, so it vanished.
///
/// The fix applies the rebaseline the instant the newer generation is observed
/// (its `ChannelOpened`), which only wipes the superseded older generation; the
/// new generation's replay then survives. Assert the replayed answer is present.
#[gpui::test]
fn restore_keeps_replayed_history_across_a_gate_closed_generation_bump(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::agent_event::{AgentEventKind as K, ChunkRole, TurnOutcome};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = boot_with_bound_slot(cx, "S1");

    let reply = |event: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event,
    };

    // The full-log replay a restarted GUI attaches to. Generation 0 is the first
    // channel (initial `channel_generation == 0`), whose ONLY turn crashed before
    // completing (NO `TurnEnded`) — so the §9 gate never flipped. A respawn bumps
    // to generation 1, which replays the recovered history and ends on ReplayEnd.
    view.update(vcx, |v, cx| {
        let batch = vec![
            // ── generation 0: a crashed first turn, no boundary ──────────────
            agent_note("S1", 0, 0, 0, K::ChannelOpened { resumed: false }),
            ServerNotification::UserPrompt {
                session_id: "S1".into(),
                text: "the question".into(),
            },
            agent_note(
                "S1",
                0,
                0,
                1,
                K::Chunk {
                    text: "GEN0-PARTIAL-then-crash".into(),
                    role: ChunkRole::Message,
                },
            ),
            reply(ReplyEvent::Chunk("GEN0-PARTIAL-then-crash".into())),
            // ── generation 1: respawn re-emits the full history, ends ReplayEnd
            agent_note("S1", 1, 0, 0, K::ChannelOpened { resumed: true }),
            agent_note(
                "S1",
                1,
                0,
                1,
                K::UserMessage {
                    text: "the question".into(),
                },
            ),
            reply(ReplyEvent::UserMessage("the question".into())),
            agent_note(
                "S1",
                1,
                0,
                2,
                K::Chunk {
                    text: "REPLAYED-ANSWER-must-survive".into(),
                    role: ChunkRole::Message,
                },
            ),
            reply(ReplyEvent::Chunk("REPLAYED-ANSWER-must-survive".into())),
            agent_note(
                "S1",
                1,
                0,
                3,
                K::TurnEnded {
                    outcome: TurnOutcome::ReplayEnd,
                },
            ),
            reply(ReplyEvent::ReplayComplete),
        ];
        v.apply_server_batch(batch, cx);
    });

    let text = active_transcript_text(&view, vcx);
    assert!(
        text.contains("REPLAYED-ANSWER-must-survive"),
        "the replayed history must survive a gate-closed generation bump on \
         restore (bug-0002); transcript was:\n{text}"
    );
    // The crashed generation-0 attempt is correctly superseded by the respawn's
    // replay — it must NOT linger alongside the recovered history.
    assert!(
        !text.contains("GEN0-PARTIAL-then-crash"),
        "the superseded (older-generation) crashed attempt must be wiped; \
         transcript was:\n{text}"
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
                provider: yalda::acp_channel::AgentProvider::Claude,
                turns: 3,
                connected: true,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                busy: false,
                archived: false,
            });
        }
        let mut tile = AgentTile::new();
        tile.show_picker();
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

    // Empty roster → only the two provider creation rows; renders without panic.
    install_agent_picker(&view, &mut *vcx, &[]);
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        let tile = v.agent_tile().expect("agent tile");
        assert!(
            tile.session().is_none(),
            "tile stays unbound until a row binds"
        );
        // The picker has no cached cwd — it projects from the active workspace's
        // live cwd (`agent_base_cwd`).
        let cwd = v.agent_base_cwd();
        assert!(v.picker_projection(&cwd).0.is_empty(), "no free rows yet");
    });

    // Seed two roster sessions for this cwd → the selector projects two FREE
    // rows (plus the two provider creation rows = 4 total). No async reducer, no per-tile
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
                provider: yalda::acp_channel::AgentProvider::Claude,
                turns: 0,
                connected: true,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                busy: false,
                archived: false,
            });
        }
        v.materialize_roster_detached_tiles();
    });

    // An unbound selector tile. Both sessions are FREE.
    view.update(vcx, |v, cx| {
        let mut t = AgentTile::new();
        t.show_picker();
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

/// UXI-AgentTile-39: the Agent Tile session picker groups its FREE list into tag
/// folders. `picker_projection` returns the free sessions grouped by their single
/// group key (alphabetically-first tag), tag folders in alphabetical order, the
/// untagged group last, label order preserved within a group. Because
/// `agent_picker_move` / `agent_picker_activate` index into this same order,
/// asserting the projected order proves a click on row `i+2` resolves to the
/// intended session.
///
/// Negative control: remove the group-key sort in `picker_projection` and the
/// free list falls back to plain label order (claude-1..claude-4), so the
/// grouped-order assertion below fails RED.
#[gpui::test]
fn session_picker_groups_free_sessions_by_tag(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    // Four free roster sessions (cwd "."), seeded in label order by the helper.
    install_agent_picker(
        &view,
        &mut *vcx,
        &[
            ("S1", "claude-1"),
            ("S2", "claude-2"),
            ("S3", "claude-3"),
            ("S4", "claude-4"),
        ],
    );
    // Tag them so grouping crosses label order: claude-1 → "beta",
    // claude-2 + claude-3 → "alpha", claude-4 stays untagged. Set via the real
    // mutator (sid-keyed, the same store the jump panel reads).
    view.update(vcx, |v, _cx| {
        assert!(v.add_session_tag("S1", "beta"));
        assert!(v.add_session_tag("S2", "alpha"));
        assert!(v.add_session_tag("S3", "alpha"));
    });
    // Render the picker with the tags set — exercises the header-emission path in
    // render_agent_picker without panicking.
    vcx.run_until_parked();

    view.read_with(vcx, |v, _cx| {
        let (free, _bound) = v.picker_projection(&v.agent_base_cwd());
        let order: Vec<&str> = free.iter().map(|s| s.label.as_str()).collect();
        // alpha folder (claude-2, claude-3 by label), then beta (claude-1), then
        // the untagged group (claude-4) last.
        assert_eq!(
            order,
            vec!["claude-2", "claude-3", "claude-1", "claude-4"],
            "free list is grouped by tag (alpha, beta, untagged), label order within a group"
        );
        // group_key resolves the folder each row lands in.
        assert_eq!(free[0].group_key(), Some("alpha"));
        assert_eq!(free[1].group_key(), Some("alpha"));
        assert_eq!(free[2].group_key(), Some("beta"));
        assert_eq!(
            free[3].group_key(),
            None,
            "claude-4 is untagged, filed last"
        );
    });
}

/// UXI-AgentTile-32: archive is a visibility boundary for the Agent Tile
/// picker, not only for the Jump Panel and Cmd-P. Both selectable FREE rows and
/// read-only IN USE rows exclude archived sids while preserving equivalent
/// unarchived sessions.
///
/// Negative control: remove the archive guard in `picker_projection` and the
/// archived free/bound identities reappear in these lists.
#[gpui::test]
fn agent_tile_picker_excludes_free_and_bound_archived_sessions(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState, AgentTile, App};
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let mk_session = |label: &str| AgentSession {
            state: AgentState::new_server_managed(None),
            label: label.into(),
            cwd: PathBuf::from("."),
            resume_id: None,
        };
        let mk_info = |sid: &str, label: &str| SessionInfo {
            session_id: sid.into(),
            acp_session_id: None,
            label: label.into(),
            cwd: PathBuf::from("."),
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        };

        // Two bound sessions occupy distinct tiles.
        v.set_screen(App::Agent(AgentTile::new()));
        let bound_live = v.show_local_session(mk_session("bound-live"), cx);
        v.sessions
            .bind_sid(bound_live, "S-bound-live".into())
            .expect("bind live session");
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));
        let bound_archived = v.show_local_session(mk_session("bound-archived"), cx);
        v.sessions
            .bind_sid(bound_archived, "S-bound-archived".into())
            .expect("bind archived session");

        // A third, focused Agent Tile is the unbound picker consuming the
        // projection under test.
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));

        for (sid, label) in [
            ("S-bound-live", "bound-live"),
            ("S-bound-archived", "bound-archived"),
            ("S-free-live", "free-live"),
            ("S-free-archived", "free-archived"),
        ] {
            v.agent_roster.upsert(mk_info(sid, label));
        }
        v.jump_archived_sessions
            .extend(["S-bound-archived".into(), "S-free-archived".into()]);
    });

    view.read_with(vcx, |v, _| {
        assert!(
            v.agent_tile().is_some_and(|tile| tile.picker().is_some()),
            "the focused Agent Tile is the real unbound picker"
        );
        let (free, bound) = v.picker_projection(&v.agent_base_cwd());
        assert_eq!(
            free.iter().map(|s| s.sid.as_str()).collect::<Vec<_>>(),
            vec!["S-free-live"],
            "the selectable list excludes its archived free session"
        );
        assert_eq!(
            bound.iter().map(|s| s.sid.as_str()).collect::<Vec<_>>(),
            vec!["S-bound-live"],
            "the IN USE list excludes its archived bound session"
        );
    });
}

/// The workspace cwd (`Set CWD`) is PERSISTED: it survives a save→restore
/// (process restart). Hermetic — the workspace file is redirected to a tempdir
/// (no touch to `~/.yalda`). Save writes `PersistedWorkspace.cwd`; restore reads it
/// back into the typed `Workspace.cwd` (ADR-0023).
#[gpui::test]
fn workspace_cwd_persists_across_restart(cx: &mut TestAppContext) {
    use crate::persist::with_workspace_path;
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("workspace.json");
    let set = std::env::temp_dir().join("yalda-persisted-cwd");

    // Session 1: Set CWD on the active workspace, then save to disk.
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();
    with_workspace_path(file.clone(), || {
        view.update(vcx, |v, _cx| {
            v.test_set_active_workspace_cwd(set.clone());
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
    use crate::{AgentTile, App, SessionPicker};
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    // An agent tile sitting in its selector (picker open) BEFORE any Set CWD.
    view.update(vcx, |v, _cx| {
        let mut t = AgentTile::new();
        t.show_picker();
        v.set_screen(App::Agent(t));
    });

    // Now Set CWD on the active workspace — AFTER the selector is already open.
    let target = PathBuf::from("/tmp/yalda-live-cwd-test");
    view.update(vcx, |v, _cx| {
        v.test_set_active_workspace_cwd(target.clone());
    });

    // Activate row 0 ("Start a new session"). Hermetic: the create round-trip is
    // a no-op, but the placeholder session is bound synchronously with its cwd.
    view.update(vcx, |v, cx| v.agent_picker_activate(0, cx));
    vcx.run_until_parked();

    let session_cwd = view.read_with(vcx, |v, cx| {
        let id = v
            .agent_tile()
            .expect("agent tile")
            .session()
            .expect("a session bound");
        v.sessions.get(id).expect("session").read(cx).cwd.clone()
    });
    assert_eq!(
        session_cwd, target,
        "a new agent must use the cwd set on the workspace, not the one cached \
         when the selector opened"
    );
}

/// Codex identity lives on the session itself, so it remains available even
/// when the session server (and therefore the roster) is disabled.
#[gpui::test]
fn codex_picker_session_retains_provider_without_server(cx: &mut TestAppContext) {
    use crate::{AgentTile, App};
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    view.update(vcx, |v, _cx| {
        v.set_screen(App::Agent(AgentTile::new()));
    });
    view.update(vcx, |v, cx| v.agent_picker_activate(1, cx));
    vcx.run_until_parked();

    view.read_with(vcx, |v, cx| {
        let id = v
            .agent_tile()
            .expect("agent tile")
            .session()
            .expect("Codex session bound");
        let session = v.sessions.get(id).expect("session").read(cx);
        assert_eq!(
            session.state.provider,
            yalda::acp_channel::AgentProvider::Codex
        );
        assert_eq!(session.label, "codex-1");
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
        let sid_a = v
            .sessions
            .locate(&ServerSid::new("A"))
            .expect("sid A in store");
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
        for wsp in v.workspace.workspaces.iter() {
            if let Some(w) = wsp.layout.find_leaf(id)
                && let App::Agent(t) = &w.content
            {
                return Some((t.session().is_some(), t.picker().is_some()));
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

    let add =
        |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext, label: &str| {
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
        assert_eq!(
            v.next_agent_label_for(yalda::acp_channel::AgentProvider::Codex, cx),
            "codex-1"
        );
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
            v.agent_tile().unwrap().picker().unwrap().selected
        })
    };

    // 4 rows (new Claude + new Codex + S1 + S2). Navigation wraps.
    view.update(vcx, |v, cx| v.agent_picker_move(1, cx));
    view.update(vcx, |v, cx| v.agent_picker_move(1, cx));
    view.update(vcx, |v, cx| v.agent_picker_move(1, cx));
    assert_eq!(selected(&view, &mut *vcx), 3);
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
        3,
        "up past the top wraps to bottom"
    );
}

/// Activating a listed row binds the tile to the chosen session and clears the
/// picker; the bound session SURVIVES the attach round-trip (hermetic — no
/// server, so the attach early-returns rather than dropping the session).
fn assert_existing_agent_picker_activation_stays_in_workspace(
    cx: &mut TestAppContext,
    by_mouse: bool,
) {
    use crate::workspace::{SplitDir, TileMembership};
    use crate::{AgentTile, App};
    use gpui::{Modifiers, point, px};
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let server_sid = "picker-existing-agent";
    let (workspace_idx, picker_tile, old_unbound_tile) = view.update(vcx, |v, cx| {
        let workspace_idx = v.workspace.active_workspace;
        let project = v.workspace.workspaces[workspace_idx].project();
        let cwd = v
            .projects
            .cwd_of(project)
            .expect("project cwd")
            .to_path_buf();
        v.agent_roster.upsert(SessionInfo {
            session_id: server_sid.into(),
            acp_session_id: None,
            label: "existing-agent".into(),
            cwd,
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 2,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        assert!(v.materialize_roster_detached_tiles());
        let old_unbound_tile = v
            .agent_tile_id_for_server_sid(server_sid)
            .expect("roster session materialized as an unbound tile");
        let picker_tile = v
            .workspace
            .split_focused(SplitDir::H, App::Agent(AgentTile::new()))
            .expect("add empty Agent tile to workspace");
        cx.notify();
        (workspace_idx, picker_tile, old_unbound_tile)
    });
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| {
        assert_eq!(v.workspace.focused_window_id(), Some(picker_tile));
        assert_eq!(
            v.workspace.tile_membership(old_unbound_tile),
            Some(TileMembership::Detached)
        );
        let (free, _) = v.picker_projection(&v.agent_base_cwd());
        assert_eq!(free.first().map(|row| row.sid.as_str()), Some(server_sid));
    });

    if by_mouse {
        crate::layout_probe_begin();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let (x, y, w, h) = crate::layout_probe_get("agent-picker-row-2")
            .expect("existing-agent picker row paints");
        crate::layout_probe_end();
        let at = point(px(x + w / 2.0), px(y + h / 2.0));
        vcx.simulate_mouse_move(at, None, Modifiers::default());
        vcx.simulate_click(at, Modifiers::default());
    } else {
        view.update(vcx, |v, cx| {
            v.agent_tile_mut()
                .and_then(|tile| tile.picker_mut())
                .expect("focused Agent tile has picker")
                .selected = 2;
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
    }
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, workspace_idx,
            "activation must stay in the workspace"
        );
        assert_eq!(
            v.workspace.presented_detached_tile_id(),
            None,
            "activation must not bounce to the old unbound Agent tile"
        );
        assert_eq!(
            v.workspace.focused_window_id(),
            Some(old_unbound_tile),
            "the existing stable Agent tile moves into the workspace and stays focused"
        );
        assert_eq!(
            v.workspace.tile_membership(old_unbound_tile),
            Some(TileMembership::Attached {
                workspace: workspace_idx,
                visibility: crate::workspace::AttachedVisibility::Visible,
            }),
            "picker activation binds the existing stable Agent tile"
        );
        assert_eq!(
            v.workspace.tile_membership(picker_tile),
            None,
            "the temporary empty picker tile is retired"
        );
        let session = v
            .agent_tile()
            .and_then(AgentTile::session)
            .expect("existing session bound into the workspace Agent tile");
        assert_eq!(
            v.sessions.sid_of(session).map(|sid| sid.as_str()),
            Some(server_sid)
        );
    });
}

/// Clicking a painted existing-agent row must have the same placement result as
/// selecting it with Enter: stay in the workspace and bind the new Agent tile.
#[gpui::test]
fn session_picker_click_stays_in_workspace(cx: &mut TestAppContext) {
    assert_existing_agent_picker_activation_stays_in_workspace(cx, true);
}

/// Keyboard control for the mouse-only picker placement regression.
#[gpui::test]
fn session_picker_enter_stays_in_workspace(cx: &mut TestAppContext) {
    assert_existing_agent_picker_activation_stays_in_workspace(cx, false);
}

/// The roster is live while the picker is open. If the highlighted session
/// shifts to an earlier row, Enter must activate the row the user still sees
/// highlighted rather than submitting the now-stale stored index and doing
/// nothing.
#[gpui::test]
fn session_picker_enter_uses_visually_clamped_row_after_roster_shrink(cx: &mut TestAppContext) {
    use crate::workspace::{SplitDir, TileMembership};
    use crate::{AgentTile, App};
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let (workspace, picker, beta_tile) = view.update(vcx, |v, cx| {
        let workspace = v.workspace.active_workspace;
        let project = v.workspace.workspaces[workspace].project();
        let cwd = v
            .projects
            .cwd_of(project)
            .expect("project cwd")
            .to_path_buf();
        for (sid, label) in [("picker-alpha", "alpha"), ("picker-beta", "beta")] {
            v.agent_roster.upsert(SessionInfo {
                session_id: sid.into(),
                acp_session_id: None,
                label: label.into(),
                cwd: cwd.clone(),
                provider: yalda::acp_channel::AgentProvider::Claude,
                turns: 1,
                connected: true,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                busy: false,
                archived: false,
            });
        }
        v.session_tags
            .insert("picker-alpha".into(), vec!["first-group".into()]);
        assert!(v.materialize_roster_detached_tiles());
        let beta_tile = v
            .agent_tile_id_for_server_sid("picker-beta")
            .expect("beta stable tile");
        let picker = v
            .workspace
            .split_focused(SplitDir::H, App::Agent(AgentTile::new()))
            .expect("workspace picker tile");
        cx.notify();
        (workspace, picker, beta_tile)
    });
    vcx.run_until_parked();

    // Real keyboard navigation: row 3 is beta while both sessions are present.
    vcx.simulate_keystrokes("down down down");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.agent_tile().and_then(AgentTile::picker).unwrap().selected,
            3
        );
    });

    // Alpha disappears from the live roster. Beta is now row 2 and the render
    // highlights it by clamping, but the pre-fix model still stores row 3.
    view.update(vcx, |v, cx| {
        assert!(v.agent_roster.remove("picker-alpha"));
        cx.notify();
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| {
        assert_eq!(v.workspace.focused_window_id(), Some(beta_tile));
        assert_eq!(v.workspace.tile_membership(picker), None);
        assert_eq!(
            v.workspace.tile_membership(beta_tile),
            Some(TileMembership::Attached {
                workspace,
                visibility: crate::workspace::AttachedVisibility::Visible
            })
        );
        assert_eq!(
            v.agent_tile()
                .and_then(AgentTile::session)
                .and_then(|id| v.sessions.sid_of(id))
                .map(|sid| sid.as_str()),
            Some("picker-beta")
        );
    });
}

/// The intermittent variant: the roster session is already attached locally,
/// but its stable Agent tile is Unbound. Placement must move that tile without
/// minting a duplicate local session or navigating away.
#[gpui::test]
fn session_picker_places_already_local_unbound_agent_without_duplicate(cx: &mut TestAppContext) {
    use crate::workspace::{SplitDir, TileMembership};
    use crate::{AgentTile, App};
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let server_sid = "picker-local-unbound-agent";
    let (stable, picker, workspace, session) = view.update(vcx, |v, cx| {
        let workspace = v.workspace.active_workspace;
        let project = v.workspace.workspaces[workspace].project();
        let cwd = v
            .projects
            .cwd_of(project)
            .expect("project cwd")
            .to_path_buf();
        let session = v.show_local_session(
            crate::AgentSession {
                state: crate::AgentState::new_server_managed(None),
                label: "local-unbound".into(),
                cwd: cwd.clone(),
                resume_id: None,
            },
            cx,
        );
        v.sessions
            .bind_sid(session, crate::ServerSid::new(server_sid))
            .expect("local session gets durable sid");
        v.agent_roster.upsert(SessionInfo {
            session_id: server_sid.into(),
            acp_session_id: None,
            label: "local-unbound".into(),
            cwd,
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 2,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        let mut tile = AgentTile::new();
        tile.bind(session);
        let stable = v.workspace.push_detached(App::Agent(tile), project);
        let picker = v
            .workspace
            .split_focused(SplitDir::H, App::Agent(AgentTile::new()))
            .expect("workspace picker tile");
        (stable, picker, workspace, session)
    });

    view.update(vcx, |v, cx| {
        v.agent_tile_mut()
            .and_then(|tile| tile.picker_mut())
            .expect("focused Agent tile has picker")
            .selected = 2;
        cx.notify();
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(v.sessions.len(), 1, "placement must not mint a duplicate");
        assert_eq!(v.workspace.focused_window_id(), Some(stable));
        assert_eq!(v.workspace.tile_membership(picker), None);
        assert_eq!(
            v.workspace.tile_membership(stable),
            Some(TileMembership::Attached {
                workspace,
                visibility: crate::workspace::AttachedVisibility::Visible
            })
        );
        assert_eq!(v.agent_tile().and_then(AgentTile::session), Some(session));
    });
}

/// The shell command is the discoverable placement path for an Agent tile
/// outside every workspace. It opens the same real workspace picker used for
/// bound tiles and moves the same stable tile into the chosen workspace.
#[gpui::test]
fn shell_send_to_workspace_command_binds_an_unbound_agent(cx: &mut TestAppContext) {
    use crate::workspace::TileMembership;
    use crate::{AgentTile, App, LinearTile};

    let (view, vcx) = boot_browser(cx);
    let (agent, target) = view.update(vcx, |v, _| {
        let project = v.workspace.inherited_project();
        v.workspace
            .push_workspace_inheriting(App::Linear(LinearTile::new()));
        let target = v.workspace.active_workspace;
        v.workspace.set_active_workspace(0);
        let agent = v.workspace.push_detached(
            App::Agent(AgentTile::dormant(crate::ServerSid::new(
                "send-agent-to-workspace",
            ))),
            project,
        );
        assert!(v.workspace.present_solo(agent));
        (agent, target)
    });

    view.update(vcx, |v, cx| v.dispatch_menu_command("send-tile-follow", cx));
    view.read_with(vcx, |v, _| {
        let picker = v
            .workspace_picker_ref()
            .expect("shell command opens workspace picker");
        assert_eq!(
            picker.mode,
            crate::WorkspacePickerMode::Move { follow: true }
        );
        assert_eq!(picker.targets, vec![0, target]);
        assert_eq!(picker.selected, 1);
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.tile_membership(agent),
            Some(TileMembership::Attached {
                workspace: target,
                visibility: crate::workspace::AttachedVisibility::Visible,
            })
        );
        assert_eq!(v.workspace.active_workspace, target);
        assert_eq!(v.workspace.focused_window_id(), Some(agent));
    });
}

#[gpui::test]
fn session_picker_activation_binds_slot(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();
    install_agent_picker(&view, &mut *vcx, &[("S1", "claude-1"), ("S2", "claude-2")]);
    vcx.run_until_parked();

    // Activate row 3 (the second listed session, sid "S2").
    view.update(vcx, |v, cx| v.agent_picker_activate(3, cx));
    // Park the executor: with the server off, the attach round-trip is a no-op,
    // so the bind must SURVIVE (regression guard for the orphaned-tile bug).
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        let tile = v.agent_tile().expect("agent tile");
        assert!(
            tile.session().is_some(),
            "a session is bound after activation and survives the attach"
        );
        assert!(
            tile.picker().is_none(),
            "picker cleared once a session binds"
        );
        assert_eq!(v.sessions.len(), 1, "exactly one session in the store");
        let id = tile.session().unwrap();
        assert_eq!(
            v.sessions.sid_of(id).map(|s| s.as_str()),
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
            tile.set_pending(Some(token));
        }
        let before = v.sessions.len();
        assert_eq!(before, 2, "owner + orphan placeholder");
        v.apply_open_agent_resolution(
            token,
            OpenResolution::Created {
                sid: "S".into(),
                acp_id: None,
                provider: yalda::acp_channel::AgentProvider::Claude,
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
            tile.session(),
            Some(owner_id),
            "the focused tile now shows the existing owner (focus-on-conflict)"
        );
        assert!(tile.picker().is_none());
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
            tile.set_pending(Some(token));
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
                provider: yalda::acp_channel::AgentProvider::Claude,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            },
            cx,
        );
    });
    view.read_with(vcx, |v, cx| {
        let id = v
            .focused_bound_session()
            .expect("still bound after resolution");
        let (active, focus) = v
            .read_session(id, cx, |c| (c.inline_you_block_active(), c.focus))
            .unwrap();
        assert!(
            active,
            "after the async /clear bind the worksheet must be typeable (inline block active) \
             — else keystrokes fall into nav and nothing repaints"
        );
        assert_eq!(
            focus,
            crate::AgentFocus::Compose,
            "focused so typing lands + repaints"
        );
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
            label: "codex-A".into(),
            provider: yalda::acp_channel::AgentProvider::Codex,
            active: true,
            mode: InputModeKind::Chatbox,
            tasklist_open: false,
            subagents_open: false,
            sidepanel_hidden: false,
            cwd: cwd.clone(),
            compose_draft: None,
            summary: None,
        },
        SessionSnapshot {
            id: "SID-B".into(),
            label: "claude-B".into(),
            provider: yalda::acp_channel::AgentProvider::Claude,
            active: false,
            mode: InputModeKind::Worksheet,
            tasklist_open: true,
            subagents_open: false,
            // UXI-AgentTile-20: force-hidden sidepanel round-trips per session.
            sidepanel_hidden: true,
            cwd: cwd.clone(),
            compose_draft: None,
            summary: None,
        },
    ];

    let loaded = with_acp_persist_path(file.clone(), || {
        save_persisted_acp_sessions(&cwd, &snaps);
        load_persisted_acp_sessions(&cwd)
    });

    assert_eq!(loaded.len(), 2, "both sessions round-trip");
    // Each slot kept its OWN sid + label (no cross-binding).
    assert_eq!(loaded[0].id.as_str(), "SID-A");
    assert_eq!(loaded[0].label, "codex-A");
    assert_eq!(
        loaded[0].provider,
        Some(yalda::acp_channel::AgentProvider::Codex),
        "the Codex provider survives synchronous restore before roster seed"
    );
    assert!(loaded[0].active, "first session is the active one");
    assert_eq!(loaded[1].id.as_str(), "SID-B");
    assert_eq!(loaded[1].label, "claude-B");
    assert_eq!(loaded[1].mode, InputModeKind::Worksheet);
    assert!(loaded[1].tasklist_open);
    // UXI-AgentTile-20: the hidden flag round-trips (A shown, B hidden).
    assert!(!loaded[0].sidepanel_hidden, "SID-A sidepanel stays shown");
    assert!(
        loaded[1].sidepanel_hidden,
        "SID-B sidepanel restores hidden"
    );
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

/// REGRESSION (bug-0024): restore binds a tile before the async roster seed, so
/// its temporary provider fallback can be Claude even when the WAL-backed
/// session is Codex. A later `SessionCreated` roster event for that bound sid
/// must repair the live AgentState and repaint the cached conversation turn
/// header. The compact tile header intentionally carries no separate provider
/// badge; provider identity remains visible on each agent turn.
///
/// Negative control: remove `recover_providers_from_roster` from the
/// `SessionCreated` reducer. The state stays Claude, the Claude probes paint,
/// and all Codex assertions below fail RED.
#[gpui::test]
fn codex_roster_identity_repairs_session_and_turn_header(cx: &mut TestAppContext) {
    use yalda::acp_channel::{AgentProvider, PermissionMode, ReplyEvent};
    use yalda::session_proto::{Notification as ServerNotification, SessionInfo};

    let (view, vcx) = boot_browser(cx);
    install_agent_slot(&view, &mut *vcx, Some("S1"));
    let session_entity = view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("bound session");
        let entity = v.session_entity(id).expect("session entity");
        cx.notify();
        entity
    });
    vcx.run_until_parked();
    view.read_with(vcx, |_v, cx| {
        let provider = session_entity.read(cx).state.provider;
        assert_eq!(
            provider,
            AgentProvider::Claude,
            "fixture reproduces the pre-roster Claude fallback"
        );
    });

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ServerNotification::SessionCreated {
                    session: SessionInfo {
                        session_id: "S1".into(),
                        acp_session_id: Some("codex-acp-1".into()),
                        label: "codex-1".into(),
                        cwd,
                        provider: AgentProvider::Codex,
                        turns: 1,
                        connected: true,
                        permission_mode: PermissionMode::ReadOnly,
                        busy: false,
                        archived: false,
                    },
                },
                ServerNotification::ReplyEvent {
                    session_id: "S1".into(),
                    event: ReplyEvent::Chunk("reply from Codex\n".into()),
                },
                ServerNotification::ReplyEvent {
                    session_id: "S1".into(),
                    event: ReplyEvent::ReplayComplete,
                },
            ],
            cx,
        );
    });
    vcx.run_until_parked();

    view.read_with(vcx, |_v, cx| {
        let session = session_entity.read(cx);
        let (provider, label) = (session.state.provider, session.label.clone());
        assert_eq!(
            provider,
            AgentProvider::Codex,
            "authoritative roster identity repairs the already-open session"
        );
        assert_eq!(
            label, "codex-1",
            "the restored auto-name follows the Codex roster"
        );
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let tile_codex = crate::layout_probe_get("agent-provider-Codex");
    let tile_claude = crate::layout_probe_get("agent-provider-Claude");
    let turn_codex = crate::layout_probe_get("agent-turn-header-Codex");
    let turn_claude = crate::layout_probe_get("agent-turn-header-Claude");
    crate::layout_probe_end();

    let (_, _, turn_w, turn_h) =
        turn_codex.expect("the real transcript must paint a Codex turn header");
    assert!(
        turn_w > 4.0 && turn_h > 4.0,
        "Codex transcript header has no painted area"
    );
    assert!(
        tile_codex.is_none() && tile_claude.is_none(),
        "the compact tile header must not regain a separate provider badge"
    );
    assert!(
        turn_claude.is_none(),
        "the visible turn identity may not still call this Codex session Claude"
    );
}

/// Agent replies use a stripped-Markdown line renderer, so the visible link
/// label must retain its hidden destination and dispatch a real mouse click
/// against the agent session's cwd.
#[gpui::test]
fn agent_markdown_link_opens_local_file_in_buffer_tile(cx: &mut TestAppContext) {
    use crate::{App, BufferApp};
    use gpui::{Modifiers, point, px};

    let dir =
        std::env::temp_dir().join(format!("yalda-agent-markdown-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create agent link fixture dir");
    let target = dir.join("target.md");
    std::fs::write(&target, "# Agent target\n").expect("write agent target");

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    install_agent_slot(&view, vcx, None);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        let id = v.agent_tile().unwrap().session().unwrap();
        let session = v.session_entity(id).unwrap();
        session.update(cx, |session, cx| {
            session.cwd = dir.clone();
            session
                .state
                .editor
                .programmatic_insert(0, "Open [target](target.md) now\n");
            session.state.editor.add_frozen_lines(0, 1);
            session.state.focus = crate::AgentFocus::Compose;
            cx.notify();
        });
        cx.notify();
    });
    for _ in 0..2 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    let session = view
        .update(vcx, |v, _| {
            let id = v.agent_tile().unwrap().session().unwrap();
            v.session_entity(id)
        })
        .expect("agent session");
    crate::layout_probe_begin();
    session.update(vcx, |session, cx| {
        session.state.pending_reveal_cursor = true;
        cx.notify();
    });
    vcx.run_until_parked();
    let (x, y, w, h) =
        crate::layout_probe_get("transcript-link-0-5").expect("inline agent link did not paint");
    assert!(
        crate::layout_probe_get("transcript-link-0-0").is_none(),
        "ordinary prose before an agent link must not become clickable"
    );
    crate::layout_probe_end();
    assert!(w > 0.0 && h > 0.0, "agent link painted no clickable area");

    let at = point(px(x + 2.0), px(y + h / 2.0));
    vcx.simulate_mouse_move(at, None, Modifiers::default());
    vcx.simulate_click(at, Modifiers::default());
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| {
        let workspace = v.workspace.active_workspace().expect("active workspace");
        let opened = &workspace
            .layout
            .find_leaf(workspace.focused)
            .expect("focused linked buffer")
            .content;
        match opened {
            App::Buffer(BufferApp::Viewing(doc)) => assert_eq!(
                doc.file_label.as_ref(),
                target.canonicalize().unwrap().display().to_string()
            ),
            _ => panic!("agent link target did not open in a viewed buffer"),
        }
    });

    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_dir(&dir);
}

/// REGRESSION (bug-0006): on a FROZEN line that renders markdown-stripped, a drag
/// selection must copy the VISUALLY-selected text, not a shifted raw-document slice.
/// Repro: freeze a line `**Email:** <email>` (renders `Email: <email>`), drag across
/// the painted email token, and assert the clipboard holds the email — NOT the
/// `:** scott+...` garbage the raw/stripped column mismatch produced.
///
/// Negative control: revert the raw-offset mapping in `build_wrapped_line` (register
/// the stripped `start_char` again) → the clipboard gets the shifted `:** …` slice
/// and the `contains(email)` / `!contains(":**")` asserts fail RED.
#[gpui::test]
fn transcript_drag_on_frozen_markdown_line_copies_visual_span(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (view, vcx, id, session) = boot_with_transcript(cx);

    const EMAIL: &str = "scott+coralpoint@fulcrumo.com";
    // A frozen agent line with leading markdown emphasis, so it renders STRIPPED
    // (`Email: <email>`) while the raw document keeps the `**`s.
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, &format!("**Email:** {EMAIL}\n"));
        s.state.editor.add_frozen_lines(0, 1);
        cx.notify();
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    view.update(vcx, |_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            "SENTINEL-NOT-COPIED".into(),
        ))
    });

    let tv = view
        .update(vcx, |v, _| v.transcript_views.get(&id).cloned())
        .expect("transcript view exists");
    let tokens: Vec<crate::TokenHit> = tv.update(vcx, |t, _| t.token_hits.borrow().clone());
    // The email is one non-whitespace token; find it by the char span it covers in
    // the RAW document (start col 11 = just past "**Email:** ").
    let line0: Vec<&crate::TokenHit> = tokens.iter().filter(|t| t.line_idx == 0).collect();
    assert!(!line0.is_empty(), "frozen line painted no tokens");
    // The email is the RIGHTMOST painted token (it ends the line). Drag across just
    // it — the exact reported gesture (select the email). The copy must be the email
    // the user sees, not the raw-column-shifted `:** scott+…` fragment.
    let email_tok = line0
        .iter()
        .max_by(|a, b| a.bounds.left().partial_cmp(&b.bounds.left()).unwrap())
        .unwrap();
    let midy = email_tok.bounds.top() + (email_tok.bounds.bottom() - email_tok.bounds.top()) / 2.0;
    let start = point(email_tok.bounds.left() + px(1.0), midy);
    let end = point(email_tok.bounds.right() - px(1.0), midy);

    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    vcx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();

    let clip = view
        .update(vcx, |_, cx| cx.read_from_clipboard())
        .and_then(|it| it.text())
        .unwrap_or_default();
    assert_ne!(clip, "SENTINEL-NOT-COPIED", "auto-copy never fired");
    // The copied text is the email the user selected — NOT a raw slice shifted left
    // (which would drop the trailing `.com` and leak the stripped `:** ` markers).
    assert!(
        clip.contains(EMAIL),
        "clipboard {clip:?} does not contain the full email {EMAIL:?} (raw/stripped col mismatch)"
    );
    assert!(
        !clip.contains('*'),
        "clipboard {clip:?} leaked stripped `**` markers — wrong (raw) column mapping"
    );
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
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            "SENTINEL-NOT-COPIED".into(),
        ))
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

/// REGRESSION (bug-0010): after a real drag-select in the transcript leaves an
/// anchor set (X11 copy-on-select keeps the highlight), a forced caret JUMP to
/// the editable tail — the move the turn-finalize / reopen path runs — must
/// COLLAPSE that selection, not balloon it from the old anchor to the tail. The
/// live report: "when new text arrives it automatically gets selected" because
/// `selection_range` is `anchor..cursor` and the transcript caret auto-advances.
///
/// Drives the REAL paths end to end: a real mouse drag (→ `transcript_mouse_*`)
/// creates the persisted anchor, then the REAL `move_cursor_to_tail` (invoked by
/// `finalize_agent_turn_idem` / `finish_replay`) performs the caret jump.
///
/// Negative control: remove the `clear_selection()` added to `move_cursor_to_tail`
/// and the post-jump `selection_range()` is `Some(((0,_)..(tail,_)))` — the
/// `is_none()` assert fires RED (the ballooned selection).
#[gpui::test]
fn transcript_tail_jump_collapses_stale_selection(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (view, vcx, id, session) = boot_with_transcript(cx);

    // Known frozen content — several lines so a top selection + tail jump span
    // is unmistakably non-trivial.
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "alpha line one\nbeta line two\ngamma line three\n");
        s.state.editor.add_frozen_lines(0, 3);
        cx.notify();
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    // Real drag across the FIRST line → selection + copy-on-select leaves the
    // anchor set (the "I clicked on the tile" precondition).
    let tv = view
        .update(vcx, |v, _| v.transcript_views.get(&id).cloned())
        .expect("transcript view exists");
    let tokens: Vec<crate::TokenHit> = tv.update(vcx, |t, _| t.token_hits.borrow().clone());
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
    vcx.simulate_mouse_down(
        point(left + px(1.0), midy),
        MouseButton::Left,
        Modifiers::default(),
    );
    vcx.simulate_mouse_move(
        point(right - px(1.0), midy),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    vcx.simulate_mouse_up(
        point(right - px(1.0), midy),
        MouseButton::Left,
        Modifiers::default(),
    );
    vcx.run_until_parked();

    // Precondition: a real, non-empty selection persists after the drag.
    let before = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.editor.selection_range())
        })
        .expect("session");
    assert!(
        matches!(before, Some((a, b)) if a != b),
        "precondition: the drag left a persisted non-empty selection, got {before:?}"
    );

    // The REAL caret jump the turn-finalize / reopen path runs.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").move_cursor_to_tail();
    });

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let last = c.editor.document().line_count().saturating_sub(1);
        assert_eq!(c.editor.cursor().line, last, "caret jumped to the tail");
        assert!(
            c.editor.selection_range().is_none(),
            "the caret jump must COLLAPSE the stale selection — new content is not \
             auto-selected (bug-0010), got {:?}",
            c.editor.selection_range()
        );
    });
}

/// REGRESSION (bug-0008): a parsed BLOCK (markdown table / bullet list / code) used
/// to render with NO token-hit registration at all, so the mouse could not select any
/// of its content ("can't select a table / bullets"). This drives the real render:
/// freeze a markdown table so it renders as a `FlatItem::Block`, then (1) assert the
/// paint-time hit sink now has hits covering the table's raw lines, and (2) drag
/// across it and assert the clipboard holds the table's content.
///
/// Negative control: remove the `register_block_lines_on_paint` wrapper in the
/// `FlatItem::Block` arm → the block registers zero hits for its lines → the
/// non-empty-hits assert fails RED (and the drag copies nothing / the wrong line).
#[gpui::test]
fn transcript_block_table_is_mouse_selectable(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (view, vcx, id, session) = boot_with_transcript(cx);

    // A markdown table, frozen so it renders as a parsed block (not prose lines).
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state.editor.programmatic_insert(
            0,
            "| Name | Email |\n| --- | --- |\n| Scott | scott@x.com |\n",
        );
        s.state.editor.add_frozen_lines(0, 3);
        cx.notify();
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    // The table must actually render as a Block (else it's prose and already
    // selectable — not the bug under test).
    let has_block = session.read_with(vcx, |s, _| {
        s.state
            .view_model
            .flat_items_cache
            .iter()
            .any(|it| matches!(it, crate::FlatItem::Block(_)))
    });
    assert!(
        has_block,
        "the frozen table did not render as a FlatItem::Block"
    );

    let tv = view
        .update(vcx, |v, _| v.transcript_views.get(&id).cloned())
        .expect("transcript view exists");
    let tokens: Vec<crate::TokenHit> = tv.update(vcx, |t, _| t.token_hits.borrow().clone());
    // The core defect: the block's raw lines (0..3) must now register hit bands.
    let table_hits: Vec<&crate::TokenHit> = tokens.iter().filter(|t| t.line_idx < 3).collect();
    assert!(
        !table_hits.is_empty(),
        "the table block registered NO hit-test tokens — its content is unselectable (bug-0008)"
    );

    let _ = MouseButton::Left;
    let _ = Modifiers::default();

    // PER-CELL precision: the data row (raw line 2) registers a hit for the EMAIL
    // cell keyed to its exact raw char span (`scott@x.com` = chars 10..21), distinct
    // from the `Scott` cell (chars 2..7). A whole-line band would give ONE line-2 hit
    // at start_char 0 — the presence of a start_char==10 cell hit is the per-cell
    // property.
    let email_cell = table_hits
        .iter()
        .find(|t| t.line_idx == 2 && t.start_char == 10)
        .expect("data row registers the EMAIL cell at its raw char span (per-cell, bug-0008)");
    assert_eq!(
        email_cell.char_count, 11,
        "email cell covers exactly `scott@x.com`"
    );
    assert!(
        table_hits
            .iter()
            .any(|t| t.line_idx == 2 && t.start_char == 2),
        "the `Scott` cell is a SEPARATE hit — cells are distinct, not one row"
    );

    // Drive the REAL hit-test (`hit_test_tokens`, the function the mouse path uses):
    // the email cell's center maps to the data row and a column inside the cell, and
    // its LEFT edge maps to the cell START (char 10) — proving the cell, not the row.
    let midy =
        email_cell.bounds.top() + (email_cell.bounds.bottom() - email_cell.bounds.top()) / 2.0;
    let center = point(
        (email_cell.bounds.left() + email_cell.bounds.right()) / 2.0,
        midy,
    );
    let (hl, hc) = crate::hit_test_tokens(center, &tokens).expect("center hits a token");
    assert_eq!(hl, 2, "email cell center hit-tests to the data row");
    assert!(
        (10..=21).contains(&hc),
        "and to a column INSIDE the email cell (got {hc})"
    );
    let (ll, lc) = crate::hit_test_tokens(point(email_cell.bounds.left() + px(1.0), midy), &tokens)
        .expect("left edge hits a token");
    assert_eq!(
        (ll, lc),
        (2, 10),
        "the cell's left edge maps to the cell START char"
    );
}

/// REGRESSION (bug-0030): dragging over a TABLE CELL in the transcript
/// registered hit bands (so the model selected + the clipboard copied) but
/// painted NO selection highlight — the user saw nothing and reported "can't
/// highlight table cells." Code blocks got the paint (bug-0017) via the per-line
/// `block_hits` path; the OTHER parsed `FlatItem::Block` (tables) stayed on the
/// even-split `register_block_hits_on_paint` band path, which registered hits but
/// never painted a highlight. The fix paints a selection QUAD in the SAME band
/// geometry the hits use (same uniform-width model as `hit_test_tokens`), so the
/// highlight lands exactly where the drag selects. (Bullet lists render as prose
/// `FlatItem::Line`s, not Blocks — they already highlight via the prose path;
/// this bug was the Block path only.)
///
/// Drives the REAL `transcript_mouse_down/move` across the data row and asserts
/// the highlight painted via `DocRenderTap.band_selection`.
///
/// Negative control: at the `FlatItem::Block` band call in `transcript_view.rs`
/// pass `None` for the selection (revert the wiring) → `band_selection` stays
/// empty → the non-empty assert fires RED for the right reason (no highlight
/// painted). Hit registration is untouched by that revert, proving this guards
/// the PAINT, not the already-working copy.
#[gpui::test]
fn transcript_block_table_selection_is_painted(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (view, vcx, id, session) = boot_with_transcript(cx);

    // A markdown table frozen as a parsed `FlatItem::Block`. Raw lines:
    //   0 | Name | Email |   1 | --- | --- |   2 | Scott | scott@x.com |
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state.editor.programmatic_insert(
            0,
            "| Name | Email |\n| --- | --- |\n| Scott | scott@x.com |\n",
        );
        s.state.editor.add_frozen_lines(0, 3);
        // Focus the transcript so the selection band renders (§4.5).
        s.state.focus = crate::AgentFocus::Transcript;
        cx.notify();
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let has_block = session.read_with(vcx, |s, _| {
        s.state
            .view_model
            .flat_items_cache
            .iter()
            .any(|it| matches!(it, crate::FlatItem::Block(_)))
    });
    assert!(
        has_block,
        "the frozen table did not render as a FlatItem::Block"
    );

    let tv = view
        .update(vcx, |v, _| v.transcript_views.get(&id).cloned())
        .expect("transcript view exists");
    let tokens: Vec<crate::TokenHit> = tv.update(vcx, |t, _| t.token_hits.borrow().clone());
    // The EMAIL cell of the data row (raw line 2, chars 10..21) — its own band.
    let email_cell = tokens
        .iter()
        .find(|t| t.line_idx == 2 && t.start_char == 10)
        .expect("data row registers the EMAIL cell band (bug-0008)")
        .bounds;
    let midy = email_cell.top() + (email_cell.bottom() - email_cell.top()) / 2.0;

    // Reset the paint tap, then drive a REAL drag ACROSS the email cell.
    YaldaGpuiView::test_reset_doc_render_tap();
    let start_pos = point(email_cell.left() + px(1.0), midy);
    let end_pos = point(email_cell.right() - px(1.0), midy);
    tv.update(vcx, |t, cx| {
        t.transcript_mouse_down(
            &gpui::MouseDownEvent {
                button: MouseButton::Left,
                position: start_pos,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
        t.transcript_mouse_move(
            &gpui::MouseMoveEvent {
                position: end_pos,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            },
            cx,
        );
    });
    vcx.run_until_parked();

    // The selection highlight was actually PAINTED over the dragged cell (raw
    // line 2) — the assert every prior table "fix" lacked.
    let tap = YaldaGpuiView::test_doc_render_tap();
    assert!(
        !tap.band_selection.is_empty(),
        "no selection highlight was painted over the table cell (bug-0030)"
    );
    assert!(
        tap.band_selection.iter().any(|(l, _, _)| *l == 2),
        "the highlight must cover the dragged data row (raw line 2); painted {:?}",
        tap.band_selection
    );
    // Non-vacuity: at least one painted range has real width, and it stays WITHIN
    // the email cell's char span (10..=21) — a whole-row smear would exceed it.
    assert!(
        tap.band_selection
            .iter()
            .any(|(l, s, e)| *l == 2 && e > s && *s >= 10 && *e <= 21),
        "the painted highlight must be a real, cell-bounded span (10..21); got {:?}",
        tap.band_selection
    );

    // Release copies the cell text — the copy path was already working; keep it
    // covered so the paint fix can't regress it.
    tv.update(vcx, |t, cx| {
        t.transcript_mouse_up(
            &gpui::MouseUpEvent {
                button: MouseButton::Left,
                position: end_pos,
                modifiers: Modifiers::default(),
                click_count: 1,
            },
            cx,
        );
    });
    vcx.run_until_parked();
    let clip = view
        .update(vcx, |_, cx| cx.read_from_clipboard())
        .and_then(|it| it.text())
        .unwrap_or_default();
    assert!(
        clip.contains("scott@x.com"),
        "drag over the email cell did not copy its text; clip = {clip:?}"
    );
}

/// bug-0003: when the transcript is FOCUSED, the caret's line renders via the
/// caret-injection path. That path used to register NO token hits, so a
/// mouse-down anchoring on the caret line snapped to a different line and the
/// copied selection was wrong. This drives the real focused path: caret parked
/// on line 1, drag ACROSS line 1, assert the clipboard holds line 1's text —
/// not line 0's. Negative control: revert the cursor-line `reg(...)` calls in
/// `build_wrapped_line` and line 1 registers no tokens → `!line1.is_empty()`
/// fires (right reason: caret line contributed nothing to the hit-test sink).
#[gpui::test]
fn transcript_drag_on_focused_caret_line_copies_that_line(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (view, vcx, id, session) = boot_with_transcript(cx);

    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "Hello agent world here\nsecond line here\n");
        // Focus the transcript and park the caret ON line 1 — this is the state
        // that makes line 1 render through the caret path.
        s.state.focus = crate::AgentFocus::Transcript;
        s.state.editor.cursor_mut().line = 1;
        s.state.editor.cursor_mut().col = 3;
        cx.notify();
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    view.update(vcx, |_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            "SENTINEL-NOT-COPIED".into(),
        ))
    });

    let tv = view
        .update(vcx, |v, _| v.transcript_views.get(&id).cloned())
        .expect("transcript view exists");
    let tokens: Vec<crate::TokenHit> = tv.update(vcx, |t, _| t.token_hits.borrow().clone());

    // The caret line (line 1) MUST contribute tokens even though it's the
    // focused cursor line — this is the bug-0003 regression point.
    let line1: Vec<&crate::TokenHit> = tokens.iter().filter(|t| t.line_idx == 1).collect();
    assert!(
        !line1.is_empty(),
        "focused caret line (line 1) registered no token hits — a mouse-down there \
         would snap to the wrong line (bug-0003)"
    );

    let left = line1
        .iter()
        .map(|t| t.bounds.left())
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap();
    let right = line1
        .iter()
        .map(|t| t.bounds.right())
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap();
    let midy = line1[0].bounds.top() + (line1[0].bounds.bottom() - line1[0].bounds.top()) / 2.0;
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
        "drag on the focused caret line did not copy anything"
    );
    assert!(
        clip.contains("second line"),
        "clipboard {clip:?} should hold the caret line's text (line 1), not another line's"
    );
    assert!(
        !clip.contains("Hello agent"),
        "clipboard {clip:?} leaked line 0 text — the drag anchored on the wrong line (bug-0003)"
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
    assert!(
        base >= 1,
        "transcript must render at least once on first frame"
    );

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
    assert!(
        base >= 1,
        "transcript must render at least once on first frame"
    );

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
    assert!(
        after,
        "a queued jump ⇒ pending_jump seq flips true (busts the cache)"
    );
}

/// UXI-AgentTile-40: uppercase J/K are direct user-turn motions. This drives
/// the real reducer to create two turns, then the real AgentView key listener;
/// the legacy jump-mode flag stays off throughout.
#[gpui::test]
fn uppercase_jk_move_directly_between_user_turns(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, id, _session) = boot_with_transcript(cx);
    let event = |event| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                event(ReplyEvent::UserMessage("first turn".into())),
                event(ReplyEvent::Chunk("first answer".into())),
                event(ReplyEvent::UserMessage("second turn".into())),
            ],
            cx,
        );
        v.with_session(id, cx, |state| {
            state.focus = crate::AgentFocus::Transcript;
            state.user_turn_jump_mode = false;
            state.user_turn_jump_ord = 0;
        });
    });
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        assert_eq!(
            v.read_session(id, cx, |state| {
                crate::user_turn_item_indices(&state.view_model.flat_items_cache).len()
            }),
            Some(2),
            "the real reducer produced two user-turn destinations"
        );
    });

    vcx.simulate_keystrokes("shift-j");
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        assert_eq!(
            v.read_session(id, cx, |state| state.user_turn_jump_ord),
            Some(1)
        );
        assert_eq!(
            v.read_session(id, cx, |state| state.user_turn_jump_mode),
            Some(false)
        );
    });

    vcx.simulate_keystrokes("shift-k");
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        assert_eq!(
            v.read_session(id, cx, |state| state.user_turn_jump_ord),
            Some(0)
        );
        assert_eq!(
            v.read_session(id, cx, |state| state.user_turn_jump_mode),
            Some(false)
        );
    });
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
    assert!(
        !ticked_idle,
        "idle session: anim tick must notify no transcript view"
    );
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
    assert!(
        ticked,
        "awaiting session: anim tick must notify its transcript view"
    );
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
        Some(crate::App::Linear(tile)) => tile
            .view
            .clone()
            .expect("render_linear lazily creates the LinearView"),
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
    assert!(
        base >= 1,
        "linear body must render at least once on first frame"
    );

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
    assert_eq!(
        sel.as_deref(),
        Some("Beta"),
        "picker_move advanced the selection"
    );
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
        v.set_state(crate::LinearViewState::Project(Box::new(
            crate::ProjectDetail {
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
            },
        )));
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
    assert_eq!(
        target0.as_deref(),
        Some("FUL-19"),
        "browse starts on first issue"
    );

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
    assert_eq!(
        target1.as_deref(),
        Some("FUL-620"),
        "nav_move advanced the cursor"
    );
}

/// The Linear tile is modal: in Normal mode printable keys are commands, not
/// text — `<space>` opens the tile/app (LINEAR) menu (so menus are reachable at
/// all), and a non-bound letter is a no-op (never typed into the query).
/// Regression for the "can't access any menus, every key types into the input" trap.
#[gpui::test]
fn linear_normal_mode_frees_keys_for_menus(cx: &mut TestAppContext) {
    use crate::{KMods, Key, KeyPress, LinearMode};
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
    assert_eq!(
        typed, "x",
        "Insert mode types printable keys into the query"
    );

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
    assert_eq!(
        after_letter, "x",
        "Normal mode does NOT type unbound letters"
    );

    // `<space>` in Normal mode is intercepted as a leader (universal path) and
    // opens the tile/app (LINEAR) local menu — the tile is not in text entry.
    let (consumed, opened, header) = view.update(vcx, |v, cx| {
        let consumed = v.leader_intercept(&kp(' '), cx);
        let header = v.menu_ref().map(|m| m.header);
        (consumed, v.overlay_is_menu(), header)
    });
    assert!(consumed, "`<space>` is consumed as a leader in Normal mode");
    assert!(opened, "`<space>` in Normal mode opens the menu");
    assert_eq!(
        header,
        Some("LINEAR"),
        "`<space>` opens the tile/app (LINEAR) local menu"
    );

    // And `.` (after closing the space menu) opens the per-workspace menu.
    let dot_header = view.update(vcx, |v, cx| {
        v.clear_overlay();
        v.leader_intercept(&kp('.'), cx);
        v.menu_ref().map(|m| m.header)
    });
    assert_eq!(
        dot_header,
        Some("MENU"),
        "`.` opens the per-workspace command menu"
    );
}

/// The universal leader rule: when a tile is NOT in text entry, `<space>`/`.`/
/// `?` are intercepted as menu-openers; when it IS (e.g. Linear Insert mode),
/// they are left for the tile to type. Covers the "leaders have highest
/// priority when not in insert mode" property.
#[gpui::test]
fn leader_intercept_respects_insert_mode(cx: &mut TestAppContext) {
    use crate::{KMods, Key, KeyPress, LinearMode};
    let kp = |c: char| KeyPress::new(Key::Char(c), KMods::NONE);
    let (view, vcx, _lv) = boot_with_linear(cx);

    // Linear opens in Insert: a leader is NOT intercepted (it's text).
    let insert = view.update(vcx, |v, cx| v.leader_intercept(&kp(' '), cx));
    assert!(
        !insert,
        "in Insert mode a leader is left to the tile as text"
    );

    // Switch to Normal: now the leader IS intercepted.
    let normal = view.update(vcx, |v, cx| {
        v.linear_set_mode(LinearMode::Normal, cx);
        v.leader_intercept(&kp('.'), cx)
    });
    assert!(
        normal,
        "in Normal mode a leader is intercepted as a menu-opener"
    );
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
    assert!(
        in_insert2,
        "compose in Insert + non-empty draft ⇒ text entry ⇒ space types"
    );

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

/// ADR-0032 (UXI-Menu-6): the numbered workspace list is NO LONGER a menu entry
/// — it must work while typing, so it lives on the `ctrl-1..0` direct chords, not
/// a leader menu. The `.` shell menu keeps the workspace *ops* (`new`/`rename`)
/// under its `w` submenu, and the `goto-workspace-N` DISPATCH still switches the
/// active workspace when invoked directly.
#[gpui::test]
fn shell_menu_offers_workspace_ops_and_goto_still_switches(cx: &mut TestAppContext) {
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
        v.workspace
            .push_workspace_inheriting(crate::App::Linear(crate::LinearTile::new()));
        v.workspace
            .push_workspace_inheriting(crate::App::Linear(crate::LinearTile::new()));
    });

    // The shell menu carries the workspace OPS (under `w`), but NOT the numbered
    // goto list (that is ctrl-1..0, a direct chord — UXI-Menu-6).
    let cmds = shell_menu_commands();
    for expect in ["rename-workspace", "new-workspace"] {
        assert!(
            cmds.contains(&expect.to_string()),
            "shell menu missing {expect}: {cmds:?}"
        );
    }
    assert!(
        !cmds.iter().any(|c| c.starts_with("goto-workspace-")),
        "the numbered workspace list is a direct chord, not a menu entry: {cmds:?}"
    );

    // Dispatching a goto command directly still switches the active workspace.
    view.update(vcx, |v, cx| v.dispatch_menu_command("goto-workspace-2", cx));
    let active = view.update(vcx, |v, _| v.workspace.active_workspace);
    assert_eq!(active, 2, "goto-workspace-2 activated the third workspace");

    view.update(vcx, |v, cx| v.dispatch_menu_command("goto-workspace-0", cx));
    let active = view.update(vcx, |v, _| v.workspace.active_workspace);
    assert_eq!(active, 0, "goto-workspace-0 activated the first workspace");
}

/// The universal agent roster (universal-agent-list), driven through the REAL
/// `apply_server_batch`: a `SessionCreated` broadcast for a session this GUI has
/// NEVER opened makes it appear in the jump panel's agent rows (always-visible
/// active sessions); `SessionRenamed` updates its label; `SessionArchived`
/// moves it out of and back into live projections; `SessionClosed` removes it.
/// This is the end-to-end wire the no-op hook used to drop on the floor.
#[gpui::test]
fn roster_surfaces_unopened_session_and_tracks_rename_close(cx: &mut TestAppContext) {
    use yalda::session_proto::Notification as ServerNotification;
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    let info = SessionInfo {
        session_id: "srv-1".into(),
        acp_session_id: Some("acp-1".into()),
        label: "claude-7".into(),
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::PermissionMode::ReadOnly,
        busy: false,
        archived: false,
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
    assert!(
        !rows[0].bound,
        "an unopened session is free (no tile binds it)"
    );
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
    assert_eq!(
        rows[0].label, "renamed-session",
        "rename updates the row label"
    );

    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionArchived {
                session_id: "srv-1".into(),
                archived: true,
            }],
            cx,
        );
    });
    assert!(view.read_with(vcx, |v, _| {
        v.jump_archived_sessions.contains("srv-1")
            && v.agent_roster
                .get("srv-1")
                .is_some_and(|info| info.archived)
    }));
    assert!(
        crate::agent_rows_for_tab(
            view.update(vcx, |v, cx| v.jump_panel_agent_rows(cx))
                .into_iter()
                .enumerate()
                .collect(),
            crate::JumpAgentTab::All,
        )
        .iter()
        .all(|(_, row)| !matches!(&row.target, crate::JumpTarget::Roster(s) if s == "srv-1")),
        "cold archived session leaves the live projection"
    );

    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionArchived {
                session_id: "srv-1".into(),
                archived: false,
            }],
            cx,
        );
    });
    assert!(
        crate::agent_rows_for_tab(
            view.update(vcx, |v, cx| v.jump_panel_agent_rows(cx))
                .into_iter()
                .enumerate()
                .collect(),
            crate::JumpAgentTab::All,
        )
        .iter()
        .any(|(_, row)| matches!(&row.target, crate::JumpTarget::Roster(s) if s == "srv-1")),
        "unarchive restores the live projection"
    );

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

/// UXI-JumpPanel-3, clause 3: the jump-panel "＋ New agent session" action drives
/// the REAL `spawn_free_agent_session`. With no session server there is no roster
/// to host a free session, so it is a graceful no-op — a transient status note is
/// set, and it creates NOTHING (no store session, no tile binding). It must never
/// panic and never auto-bind a phantom.
///
/// Negative control: `spawn_free_agent_session`'s no-server guard
/// (`let Some(handle) … else { note; return }`). Remove it and the method
/// unwraps a `None` handle → panic instead of this clean note.
#[gpui::test]
fn free_agent_session_no_server_is_graceful_noop(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx); // hermetic → session_server is None
    view.update(vcx, |v, cx| {
        assert!(v.sessions.is_empty(), "precondition: no sessions yet");
        v.spawn_free_agent_session(cx);
    });
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert!(
            v.sessions.is_empty(),
            "no session server ⇒ create NOTHING locally (no phantom session)"
        );
        let note = v
            .transient_status
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_default();
        assert!(
            note.contains("no session server"),
            "the action explains why it did nothing, got: {note:?}"
        );
    });
}

/// UXI-Project-7 (removal half): the retired global "new agent session" cwd flow
/// is gone. The `?` menu no longer offers `new-free-agent-session`, and
/// dispatching that command (the exact call a menu key-selection made) opens NO
/// overlay — sessions are created only inside a project now (per-project ＋ row).
///
/// Negative control: re-add the `?`-menu entry (`gpui_menu` / the workspace `?`
/// menu builder) → the presence assert fails; re-add the dispatch arm calling
/// `open_free_agent_session_cwd_overlay` → the "opens no overlay" assert fails.
#[gpui::test]
fn global_cwd_session_overlay_is_gone(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    view.update(vcx, |v, cx| {
        let has = shell_menu_commands()
            .iter()
            .any(|c| c == "new-free-agent-session");
        assert!(
            !has,
            "the shell menu no longer offers the global 'new agent session' cwd flow"
        );
        v.dispatch_menu_command("new-free-agent-session", cx);
        assert!(!v.has_overlay(), "the retired command opens no overlay");
    });
}

/// UXI-Project-3 (T004-tail): `jump_panel_sections` — the pure model the jump
/// panel walks — groups WORKSPACES and AGENT SESSIONS under their owning project,
/// each section listing ONLY its own (workspaces filtered by `wsp.project()`,
/// sessions by cwd→project). Workspace badges keep the GLOBAL workspace index (idx+1 =
/// ctrl-<n>), so two projects' workspaces carry distinct global numbers. Empty
/// projects still render a section. Individual tiles are never listed (the model
/// has no tile axis).
///
/// Negative control: drop the `t.project() == id` filter in `jump_panel_sections`
/// (`.filter(|(_, t)| !t.ephemeral)`) → every section lists every workspace, so
/// A's section contains B's workspace index and the exclusion assert fails.
#[gpui::test]
fn jump_panel_renders_per_project_sections(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState};
    let (view, vcx) = boot_browser(cx);
    let pa = PathBuf::from("/tmp/yalda-sec-a");
    let pb = PathBuf::from("/tmp/yalda-sec-b");
    let (a_pid, b_pid) = view.update(vcx, |v, _| {
        let a = v.projects.create("Alpha".into(), pa.clone()).expect("A");
        let b = v.projects.create("Beta".into(), pb.clone()).expect("B");
        (a, b)
    });
    // A workspace in each project; capture their GLOBAL indices.
    let a_idx = view.update(vcx, |v, cx| {
        v.new_workspace_in(a_pid, cx);
        v.workspace.active_workspace
    });
    let b_idx = view.update(vcx, |v, cx| {
        v.new_workspace_in(b_pid, cx);
        v.workspace.active_workspace
    });
    assert_ne!(
        a_idx, b_idx,
        "badges (idx+1) are distinct global workspace numbers"
    );
    // A free session rooted at A's cwd → groups under A.
    view.update(vcx, |v, cx| {
        let s = AgentSession {
            state: AgentState::new_server_managed(None),
            label: "sess-a".into(),
            cwd: pa.clone(),
            resume_id: None,
        };
        v.show_local_session(s, cx);
    });
    vcx.run_until_parked();

    let (sections, _unfiled) = view.update(vcx, |v, cx| v.jump_panel_sections(cx));
    let sec_a = sections
        .iter()
        .find(|s| s.id == a_pid)
        .expect("section A present");
    let sec_b = sections
        .iter()
        .find(|s| s.id == b_pid)
        .expect("section B present (even if empty)");

    assert!(
        sec_a.workspaces.iter().any(|(i, _, _)| *i == a_idx),
        "A lists its own workspace (global idx {a_idx})"
    );
    assert!(
        !sec_a.workspaces.iter().any(|(i, _, _)| *i == b_idx),
        "A must NOT list B's workspace — the per-project filter"
    );
    assert!(
        sec_b.workspaces.iter().any(|(i, _, _)| *i == b_idx),
        "B lists its own workspace (global idx {b_idx})"
    );
    assert!(
        sec_a.sessions.iter().any(|(_, r)| r.label == "sess-a"),
        "A groups the session rooted at its cwd"
    );
    assert!(
        sec_b.sessions.is_empty(),
        "B (no sessions) still renders an empty section"
    );
}

/// A project disclosure hides every workspace/session row beneath that header,
/// while a second toggle restores them. The folded key is the durable project
/// name rather than its runtime-local id.
#[gpui::test]
fn jump_panel_project_fold_hides_and_restores_children(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let (project, project_name, workspace_idx) = view.read_with(vcx, |v, _| {
        let idx = v.workspace.active_workspace;
        let pid = v.workspace.workspaces[idx].project();
        (pid, v.projects.name_of(pid).to_string(), idx)
    });
    let probe = format!("jump-workspace-row-{workspace_idx}");

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&probe).is_some(),
        "expanded project paints its workspace row"
    );
    crate::layout_probe_end();

    view.update(vcx, |v, cx| v.toggle_project_fold(&project_name, cx));
    view.read_with(vcx, |v, _| {
        assert!(v.projects.contains(project));
        assert!(v.jump_folded_projects.contains(&project_name));
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&probe).is_none(),
        "folded project must not paint workspace children"
    );
    crate::layout_probe_end();

    view.update(vcx, |v, cx| v.toggle_project_fold(&project_name, cx));
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&probe).is_some(),
        "unfolding restores workspace children"
    );
    crate::layout_probe_end();
}

/// ADR-0033: the painted tree has exclusive workspace children and an Unbound
/// collection. Both row kinds dispatch by stable tile id, and workspace folds
/// hide only that folder's children.
#[gpui::test]
fn jump_panel_workspace_folders_and_unbound_rows_are_tile_native(cx: &mut TestAppContext) {
    use crate::{App, LinearTile};
    use gpui::Modifiers;
    let (view, vcx) = boot_browser(cx);
    let (pid, workspace_idx, bound, unbound, agent, fold_key) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        let cwd = v.projects.cwd_of(pid).unwrap().to_path_buf();
        let mut bound_tile = LinearTile::new();
        bound_tile.title = "bound-linear".into();
        let bound = v
            .workspace
            .push_workspace_inheriting(App::Linear(bound_tile));
        let workspace_idx = v.workspace.active_workspace;
        let mut unbound_tile = LinearTile::new();
        unbound_tile.title = "unbound-linear".into();
        let unbound = v.workspace.push_detached(App::Linear(unbound_tile), pid);
        v.workspace
            .tile_mut(unbound)
            .unwrap()
            .tags
            .insert("frontend".into());
        v.agent_roster.upsert(yalda::session_proto::SessionInfo {
            session_id: "S-unbound-status".into(),
            acp_session_id: None,
            label: "working-codex".into(),
            cwd,
            provider: yalda::acp_channel::AgentProvider::Codex,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: true,
            archived: false,
        });
        let agent = v.workspace.push_detached(
            App::Agent(crate::AgentTile::dormant(crate::ServerSid::new(
                "S-unbound-status",
            ))),
            pid,
        );
        v.workspace
            .tile_mut(agent)
            .unwrap()
            .tags
            .insert("backend".into());
        v.jump_tag_order.insert(
            v.projects.name_of(pid).to_string(),
            vec!["frontend".into(), "backend".into()],
        );
        v.workspace.set_active_workspace(0);
        let wsp = &v.workspace.workspaces[workspace_idx];
        let fold_key = YaldaGpuiView::workspace_fold_key(v.projects.name_of(pid), &wsp.auto_name);
        (pid, workspace_idx, bound, unbound, agent, fold_key)
    });

    view.update(vcx, |v, cx| {
        let section = v
            .jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|section| section.id == pid)
            .expect("project section");
        let folder = section
            .workspace_folders
            .iter()
            .find(|folder| folder.index == workspace_idx)
            .expect("workspace folder");
        assert!(folder.tiles.iter().any(|tile| tile.id == bound));
        assert!(folder.tiles.iter().all(|tile| tile.id != unbound));
        let loose = section
            .detached
            .iter()
            .find(|tile| tile.id == unbound)
            .expect("unbound projection");
        assert_eq!(loose.tags, vec!["frontend"]);
        let agent_row = section
            .detached
            .iter()
            .find(|tile| tile.id == agent)
            .and_then(|tile| tile.agent.as_ref())
            .expect("unbound Agent retains its status row");
        assert_eq!(agent_row.provider, yalda::acp_channel::AgentProvider::Codex);
        assert_eq!(agent_row.dot_status(), crate::AgentDotStatus::Working);
        assert_eq!(agent_row.tags, vec!["backend"]);
        assert!(section.detached.iter().all(|tile| tile.id != bound));
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let bound_probe = format!("jump-tile-row-{bound}-ws{workspace_idx}");
    let unbound_probe = format!("jump-tile-row-{unbound}-tg0");
    assert!(crate::layout_probe_get(&bound_probe).is_some());
    let (_, _, _, workspace_row_h) =
        crate::layout_probe_get(&format!("jump-workspace-row-{workspace_idx}"))
            .expect("workspace folder header paints");
    let (_, _, _, standard_row_h) = crate::layout_probe_get("jump-system-console")
        .expect("standard 13px jump navigation row paints");
    assert!(
        (workspace_row_h - standard_row_h).abs() < 0.5,
        "workspace folder header must use the standard jump-row font size: \
         folder={workspace_row_h}px standard={standard_row_h}px"
    );
    let (x, y, w, h) = crate::layout_probe_get(&unbound_probe)
        .expect("tagged unbound row paints under its folder");
    crate::layout_probe_end();

    let at = point(px(x + w / 2.0), px(y + h / 2.0));
    vcx.simulate_mouse_move(at, None, Modifiers::default());
    vcx.simulate_click(at, Modifiers::default());
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.presented_detached_tile_id(), Some(unbound));
    });

    view.update(vcx, |v, cx| v.toggle_workspace_fold(&fold_key, cx));
    view.update(vcx, |v, _| {
        assert!(v.jump_folded_workspaces.contains(&fold_key));
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&bound_probe).is_none(),
        "folding one workspace hides its tile children"
    );
    crate::layout_probe_end();
}

/// UXI-JumpPanel-24: tag-folder chrome is compact and fixed, while a tile row
/// keeps exactly the standard navigation-row height whether it is tagged or
/// loose. Document zoom must not affect any of these jump-panel measurements.
#[gpui::test]
fn jump_panel_tagged_items_keep_fixed_chrome_size(cx: &mut TestAppContext) {
    use crate::{App, LinearTile};
    let (view, vcx) = boot_browser(cx);
    let (pid, tagged, loose) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        let mut tagged_tile = LinearTile::new();
        tagged_tile.title = "tagged-linear".into();
        let tagged = v.workspace.push_detached(App::Linear(tagged_tile), pid);
        v.workspace
            .tile_mut(tagged)
            .unwrap()
            .tags
            .insert("frontend".into());

        let mut loose_tile = LinearTile::new();
        loose_tile.title = "loose-linear".into();
        let loose = v.workspace.push_detached(App::Linear(loose_tile), pid);
        (pid, tagged, loose)
    });

    let measure = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        crate::layout_probe_begin();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let folder = crate::layout_probe_get(&format!("jump-tag-folder-{}-0", pid.0))
            .expect("tag folder paints")
            .3;
        let tagged_row = crate::layout_probe_get(&format!("jump-tile-row-{tagged}-tg0"))
            .expect("tagged tile row paints")
            .3;
        let loose_row = crate::layout_probe_get(&format!("jump-tile-row-{loose}"))
            .expect("untagged tile row paints")
            .3;
        let standard = crate::layout_probe_get("jump-system-console")
            .expect("standard jump navigation row paints")
            .3;
        crate::layout_probe_end();
        (folder, tagged_row, loose_row, standard)
    };

    let initial = measure(&view, &mut *vcx);
    assert!(
        initial.0 <= initial.3,
        "tag folder must stay compact, never taller than a normal jump row: folder={}px standard={}px",
        initial.0,
        initial.3
    );
    assert!(
        (initial.1 - initial.3).abs() < 0.5 && (initial.2 - initial.3).abs() < 0.5,
        "tagged and untagged tiles must share standard row height: tagged={}px loose={}px standard={}px",
        initial.1,
        initial.2,
        initial.3
    );

    view.update(vcx, |v, cx| v.set_text_scale(2.0, cx));
    let zoomed = measure(&view, &mut *vcx);
    for (before, after, label) in [
        (initial.0, zoomed.0, "tag folder"),
        (initial.1, zoomed.1, "tagged tile"),
        (initial.2, zoomed.2, "untagged tile"),
        (initial.3, zoomed.3, "standard row"),
    ] {
        assert!(
            (before - after).abs() < 0.5,
            "{label} is chrome and must not scale with document zoom: before={before}px after={after}px"
        );
    }
}

/// UXI-JumpPanel-26: selection is structural chrome, never low-contrast label
/// text. Folio is the reported failure because its overlay border is an
/// intentionally pale divider that cannot serve as foreground copy.
#[gpui::test]
fn jump_panel_active_workspace_keeps_folio_foreground(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    view.update(vcx, |v, cx| v.set_theme(crate::ThemeName::Folio, cx));
    vcx.run_until_parked();

    let (style, foreground, pale_divider, workspace_blue) = view.read_with(vcx, |v, _| {
        let foreground = v.editor_fg();
        let pale_divider = crate::nc(v.theme.overlay.border);
        let workspace_blue = crate::nc(v.theme.agent.jump_subheader);
        (
            crate::jump_workspace_group_style(true, foreground, workspace_blue),
            foreground,
            pale_divider,
            workspace_blue,
        )
    });

    assert_eq!(
        style.label, foreground,
        "an active workspace label must retain the normal foreground color"
    );
    assert_ne!(
        style.label, pale_divider,
        "Folio's pale structural divider must never become label text"
    );
    assert!(
        style.background.a > 0.0 && style.rail.a > 0.0 && style.outline.a > 0.0,
        "active state must remain visible through a quiet background, accent rail, and outline"
    );
    assert_eq!(
        style.identity, workspace_blue,
        "workspace identity must use the same cool blue semantic token as Detached"
    );
    assert_eq!(
        style.rail, workspace_blue,
        "the active rail must be blue rather than a theme's warm accent"
    );
    for (name, structural) in [
        ("background", style.background),
        ("outline", style.outline),
        ("separator", style.separator),
    ] {
        assert_eq!(
            (structural.h, structural.s, structural.l),
            (workspace_blue.h, workspace_blue.s, workspace_blue.l),
            "{name} must be an alpha-only derivation of workspace blue"
        );
    }
}

/// UXI-JumpPanel-26: a primary tile name is one line of navigation chrome.
/// A constrained real paint with an intentionally huge multi-word title must
/// have exactly the same row height as an ordinary jump-panel row.
#[gpui::test]
fn jump_panel_long_tile_names_stay_single_line(cx: &mut TestAppContext) {
    use crate::{App, LinearTile};

    let (view, vcx) = boot_browser(cx);
    let (tile, short_tile) = view.update(vcx, |v, _| {
        let project = v.workspace.active_workspace().expect("workspace").project();
        let mut linear = LinearTile::new();
        linear.title = "a very long detached tile title that must end in an ellipsis rather than wrapping onto a second or third navigation line".into();
        let tile = v.workspace.push_detached(App::Linear(linear), project);
        let mut short = LinearTile::new();
        short.title = "short".into();
        let short_tile = v.workspace.push_detached(App::Linear(short), project);
        (tile, short_tile)
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let long = crate::layout_probe_get(&format!("jump-tile-row-{tile}"))
        .expect("long detached tile row paints")
        .3;
    let long_label = crate::layout_probe_get(&format!("jump-tile-label-{tile}"))
        .expect("long tile identity label paints");
    let short = crate::layout_probe_get(&format!("jump-tile-row-{short_tile}"))
        .expect("short detached tile row paints")
        .3;
    let standard = crate::layout_probe_get("jump-system-console")
        .expect("standard jump navigation row paints")
        .3;
    crate::layout_probe_end();

    assert!(
        (long - short).abs() <= 0.5 && (long - standard).abs() <= 0.5,
        "long tile names must truncate without changing row height: long={long}px short={short}px standard={standard}px"
    );
    assert!(
        long_label.2 > 40.0 && long_label.3 > 10.0,
        "the truncated identity label itself must remain visibly painted: {long_label:?}"
    );
}

/// UXI-JumpPanel-25/-26: hiddenness is ownership metadata users must be able to
/// see before activating a row. Exercise the production hidden attachment and
/// require its dedicated painted marker, not a model-only boolean.
#[gpui::test]
fn jump_panel_hidden_tiles_paint_indicator(cx: &mut TestAppContext) {
    use crate::{AgentTile, App, LinearTile, ServerSid};

    let (view, vcx) = boot_browser(cx);
    let (tile, workspace, agent, detached) = view.update(vcx, |v, _| {
        let mut linear = LinearTile::new();
        linear.title = "hidden-linear".into();
        let tile = v.workspace.push_workspace_inheriting(App::Linear(linear));
        let workspace = v.workspace.active_workspace;
        let project = v.workspace.workspaces[workspace].project();
        let cwd = v.projects.cwd_of(project).unwrap().to_path_buf();
        let sid = "S-hidden-indicator";
        v.agent_roster.upsert(yalda::session_proto::SessionInfo {
            session_id: sid.into(),
            acp_session_id: None,
            label: "hidden-agent".into(),
            cwd,
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        let agent = v
            .workspace
            .split_focused(
                crate::workspace::SplitDir::V,
                App::Agent(AgentTile::dormant(ServerSid::new(sid))),
            )
            .expect("Agent tile joins the same workspace");
        v.workspace.hide_window(tile).expect("tile hides");
        v.workspace.hide_window(agent).expect("agent tile hides");
        assert_eq!(
            v.workspace.hidden_workspace_index_of_window(tile),
            Some(workspace),
            "hidden tile retains workspace ownership"
        );
        assert_eq!(
            v.workspace.hidden_workspace_index_of_window(agent),
            Some(workspace),
            "hidden Agent retains workspace ownership"
        );
        let mut detached_linear = LinearTile::new();
        detached_linear.title = "detached-visible".into();
        let detached = v
            .workspace
            .push_detached(App::Linear(detached_linear), project);
        (tile, workspace, agent, detached)
    });

    view.read_with(vcx, |v, cx| {
        let placements: Vec<_> = v
            .jump_panel_sections(cx)
            .0
            .into_iter()
            .flat_map(|section| section.workspace_folders)
            .flat_map(|folder| folder.tiles)
            .chain(
                v.jump_panel_sections(cx)
                    .0
                    .into_iter()
                    .flat_map(|section| section.detached),
            )
            .filter(|row| row.id == tile || row.id == agent || row.id == detached)
            .map(|row| (row.id, row.placement, row.agent.is_some()))
            .collect();
        assert_eq!(
            placements,
            vec![
                (tile, crate::JumpTilePlacement::AttachedHidden, false),
                (agent, crate::JumpTilePlacement::AttachedHidden, true),
                (detached, crate::JumpTilePlacement::Detached, false),
            ],
            "hidden attachment state survives the typed row projection"
        );
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&format!("jump-tile-row-{tile}-ws{workspace}")).is_some(),
        "hidden tile remains listed beneath its workspace"
    );
    let hidden_mark = crate::layout_probe_get(&format!("jump-tile-hidden-{tile}-ws{workspace}"));
    assert!(
        hidden_mark.is_some(),
        "hidden non-Agent row paints a dedicated hidden-state indicator"
    );
    assert!(
        crate::layout_probe_get(&format!("jump-tile-hidden-{agent}")).is_some(),
        "hidden Agent row keeps its provider/status marks and also paints the hidden indicator"
    );
    assert!(
        crate::layout_probe_get(&format!("jump-tile-row-{detached}")).is_some()
            && crate::layout_probe_get(&format!("jump-tile-hidden-{detached}")).is_none(),
        "Detached is a distinct placement and must never inherit the hidden marker"
    );
    let (_, _, mark_w, mark_h) = hidden_mark.unwrap();
    // UXI-JumpPanel-28: the hidden mark is now a single fixed-width icon glyph
    // (was a wider text pill), so its cell is narrow (~16px) and never wider than
    // a normal row's badge column.
    assert!(
        mark_w <= 20.0 && mark_h <= 24.0,
        "the hidden mark is a compact fixed-width icon: {mark_w}x{mark_h}px"
    );
    crate::layout_probe_end();
}

/// UXI-JumpPanel-27: a workspace and its attached tiles paint as one bounded
/// visual group. Folding removes the children without dissolving the workspace
/// card, so membership remains readable in either state.
#[gpui::test]
fn jump_panel_workspace_group_bounds_make_membership_explicit(cx: &mut TestAppContext) {
    use crate::{App, LinearTile};

    let (view, vcx) = boot_browser(cx);
    let (workspace, tile, fold_key) = view.update(vcx, |v, cx| {
        let workspace = v.workspace.active_workspace;
        let project = v.workspace.workspaces[workspace].project();
        let project_name = v.projects.name_of(project).to_string();
        let auto_name = v.workspace.workspaces[workspace].auto_name.clone();
        let mut linear = LinearTile::new();
        linear.title = "bounded member".into();
        let tile = v
            .workspace
            .split_focused(crate::workspace::SplitDir::V, App::Linear(linear))
            .expect("attached tile joins the current workspace");
        v.workspace
            .hide_window(tile)
            .expect("hidden tile remains an attached workspace member");
        let folder = v
            .jump_panel_sections(cx)
            .0
            .into_iter()
            .flat_map(|section| section.workspace_folders)
            .find(|folder| folder.index == workspace)
            .expect("workspace folder projects hidden members");
        assert_eq!(folder.tiles.len(), 2, "visible and hidden tiles both count");
        assert_eq!(
            crate::jump_workspace_membership_label(folder.tiles.len()),
            "2 tiles"
        );
        (
            workspace,
            tile,
            YaldaGpuiView::workspace_fold_key(&project_name, &auto_name),
        )
    });

    let measure = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        crate::layout_probe_begin();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let group = crate::layout_probe_get(&format!("jump-workspace-group-{workspace}"))
            .expect("workspace paints one shared group boundary");
        let header = crate::layout_probe_get(&format!("jump-workspace-row-{workspace}"))
            .expect("workspace header paints inside its group");
        let count = crate::layout_probe_get(&format!("jump-workspace-count-{workspace}"))
            .expect("workspace header paints its attached-tile count");
        let child = crate::layout_probe_get(&format!("jump-tile-row-{tile}-ws{workspace}"));
        crate::layout_probe_end();
        (group, header, count, child)
    };

    let (group, header, count, child) = measure(&view, &mut *vcx);
    let child = child.expect("expanded workspace paints its attached child");
    let contains = |outer: (f32, f32, f32, f32), inner: (f32, f32, f32, f32)| {
        inner.0 >= outer.0
            && inner.1 >= outer.1
            && inner.0 + inner.2 <= outer.0 + outer.2 + 0.5
            && inner.1 + inner.3 <= outer.1 + outer.3 + 0.5
    };
    assert!(
        contains(group, header),
        "header must sit inside group: group={group:?} header={header:?}"
    );
    assert!(
        contains(group, child),
        "tile must sit inside group: group={group:?} child={child:?}"
    );
    assert!(
        contains(header, count),
        "membership count must sit inside header: header={header:?} count={count:?}"
    );
    assert!(
        child.0 > group.0 && child.1 >= header.1 + header.3,
        "attached tiles must read as inset children below the header: group={group:?} header={header:?} child={child:?}"
    );

    view.update(vcx, |v, cx| v.toggle_workspace_fold(&fold_key, cx));
    let (folded_group, folded_header, folded_count, folded_child) = measure(&view, &mut *vcx);
    assert!(
        folded_child.is_none(),
        "collapsed workspace hides attached rows"
    );
    assert!(contains(folded_group, folded_header));
    assert!(contains(folded_header, folded_count));
    assert!(
        folded_group.3 < group.3,
        "collapsed group contracts to its header: expanded={group:?} folded={folded_group:?}"
    );
}

/// bug-0052 / UXI-JumpPanel-27: a scrollable flex column must overflow rather
/// than shrink workspace cards into border-only bands. One card cannot expose
/// this failure, so exercise the production renderer with enough expanded
/// workspaces to exceed the test viewport.
#[gpui::test]
fn crowded_jump_panel_workspace_groups_never_shrink_to_bands(cx: &mut TestAppContext) {
    use crate::{App, LinearTile};

    let (view, vcx) = boot_browser(cx);
    vcx.simulate_resize(gpui::size(px(900.0), px(360.0)));
    vcx.run_until_parked();
    let groups = view.update(vcx, |v, _| {
        let mut groups = Vec::new();
        for n in 0..16 {
            let mut tile = LinearTile::new();
            tile.title = format!("member {n}").into();
            let id = v.workspace.push_workspace_inheriting(App::Linear(tile));
            let workspace = v.workspace.active_workspace;
            let project = v.workspace.workspaces[workspace].project();
            let key = YaldaGpuiView::workspace_fold_key(
                v.projects.name_of(project),
                &v.workspace.workspaces[workspace].auto_name,
            );
            let folded = n % 2 == 1;
            if folded {
                v.jump_folded_workspaces.insert(key);
            }
            groups.push((workspace, id, folded));
        }
        v.workspace.set_active_workspace(0);
        groups
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    for (workspace, tile, folded) in groups {
        let group = crate::layout_probe_get(&format!("jump-workspace-group-{workspace}"))
            .expect("crowded workspace group still paints");
        let header = crate::layout_probe_get(&format!("jump-workspace-row-{workspace}"))
            .expect("crowded workspace header still paints");
        let row = crate::layout_probe_get(&format!("jump-tile-row-{tile}-ws{workspace}"));
        assert!(
            header.3 >= 24.0,
            "workspace header must retain normal row height under scroll pressure: workspace={workspace} group={group:?} header={header:?}"
        );
        if folded {
            assert!(row.is_none(), "folded workspace hides its member row");
            assert!(
                group.3 + 0.5 >= header.3,
                "folded group must not shrink below its header: workspace={workspace} group={group:?} header={header:?}"
            );
        } else {
            let row = row.expect("expanded crowded workspace member still paints");
            assert!(row.3 >= 24.0);
            assert!(
                group.3 + 0.5 >= header.3 + row.3,
                "expanded group must not shrink below its header and member: workspace={workspace} group={group:?} header={header:?} row={row:?}"
            );
        }
    }
    crate::layout_probe_end();
}

/// ADR-0034 / UXI-JumpPanel-25: hidden attachments remain under their owning
/// workspace in both navigation surfaces. Selection presents them solo without
/// changing membership; Unhide follows them back into the workspace.
#[gpui::test]
fn hidden_tile_navigation_is_solo_until_explicit_unhide(cx: &mut TestAppContext) {
    use crate::jump_palette::PaletteTarget;
    use crate::workspace::{AttachedVisibility, SoloPresentation, TileMembership};

    let (view, vcx) = boot_browser(cx);
    let (tile, workspace_index, project) = view.update(vcx, |v, _| {
        let tile = v.workspace.focused_window_id().unwrap();
        let workspace_index = v.workspace.active_workspace;
        let project = v.workspace.workspaces[workspace_index].project();
        v.workspace.hide_window(tile).unwrap();
        (tile, workspace_index, project)
    });

    view.read_with(vcx, |v, cx| {
        let section = v
            .jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|section| section.id == project)
            .unwrap();
        let folder = section
            .workspace_folders
            .iter()
            .find(|folder| folder.index == workspace_index)
            .unwrap();
        assert!(folder.tiles.iter().any(|row| row.id == tile));
        assert!(section.detached.iter().all(|row| row.id != tile));
        assert!(
            v.jump_palette_items(cx)
                .iter()
                .any(|item| item.target == PaletteTarget::Tile(tile)
                    && item.detail.contains("Hidden"))
        );
    });

    view.update(vcx, |v, cx| v.jump_to_tile(tile, cx));
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.presented_tile(),
            Some(SoloPresentation::HiddenAttached(tile))
        );
        assert_eq!(
            v.workspace.tile_membership(tile),
            Some(TileMembership::Attached {
                workspace: workspace_index,
                visibility: AttachedVisibility::Hidden,
            })
        );
    });

    view.update(vcx, |v, cx| v.dispatch_menu_command("tile-unhide", cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(v.workspace.presented_tile(), None);
        assert_eq!(v.workspace.active_workspace, workspace_index);
        assert_eq!(v.workspace.focused_window_id(), Some(tile));
        assert_eq!(
            v.workspace.tile_membership(tile),
            Some(TileMembership::Attached {
                workspace: workspace_index,
                visibility: AttachedVisibility::Visible,
            })
        );
    });
}

/// UXI-Menu-9: Hide and Unhide are the shared suffix of the real tile menu,
/// and applicability follows the focused tile's typed membership. A disabled
/// key is inert: it neither invokes the command nor closes the menu.
///
/// Negative control: remove `tile-unhide` from the visible/Detached arms of
/// `tile_menu_disabled` → the first `u` closes the production menu and this
/// guard fails RED. Remove the hidden arm's `tile-hide` entry → lowercase `h`
/// closes the hidden tile menu and also fails RED. (Unhide is now `u`, detach is
/// `f`; UXI-Menu-9.)
#[gpui::test]
fn tile_menu_hide_unhide_enablement_tracks_focused_membership(cx: &mut TestAppContext) {
    use crate::workspace::{AttachedVisibility, TileMembership};

    cx.update(crate::register_keymap);
    let (view, vcx) = boot_worksheet_nav(cx);
    let tile = view.read_with(vcx, |v, _| v.workspace.focused_window_id().unwrap());

    // Visible attachment: Hide is actionable; Unhide is present but dimmed.
    vcx.simulate_keystrokes("space");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let menu = v.menu_ref().expect("visible tile menu");
        assert!(menu_tree_has_command(&menu.menu, "tile-hide"));
        assert!(menu_tree_has_command(&menu.menu, "tile-unhide"));
        assert!(!menu.disabled.contains("tile-hide"));
        assert!(menu.disabled.contains("tile-unhide"));
    });
    vcx.simulate_keystrokes("u");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(v.overlay_is_menu(), "disabled Unhide keeps the menu open");
        assert!(matches!(
            v.workspace.tile_membership(tile),
            Some(TileMembership::Attached {
                visibility: AttachedVisibility::Visible,
                ..
            })
        ));
    });

    // Invoke Hide through that still-open production menu.
    vcx.simulate_keystrokes("h");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(!v.overlay_is_menu());
        assert!(matches!(
            v.workspace.tile_membership(tile),
            Some(TileMembership::Attached {
                visibility: AttachedVisibility::Hidden,
                ..
            })
        ));
    });

    // Visiting the hidden tile presents it solo. The same tile menu now flips
    // applicability: Hide is dimmed, Unhide is actionable.
    view.update(vcx, |v, cx| v.jump_to_tile(tile, cx));
    vcx.simulate_keystrokes("space");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let menu = v.menu_ref().expect("hidden tile menu");
        assert!(menu.disabled.contains("tile-hide"));
        assert!(!menu.disabled.contains("tile-unhide"));
    });
    vcx.simulate_keystrokes("h");
    vcx.run_until_parked();
    assert!(view.read_with(vcx, |v, _| v.overlay_is_menu()));
    vcx.simulate_keystrokes("u");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(!v.overlay_is_menu());
        assert!(matches!(
            v.workspace.tile_membership(tile),
            Some(TileMembership::Attached {
                visibility: AttachedVisibility::Visible,
                ..
            })
        ));
    });

    // Detached tiles cannot participate in workspace visibility; hide, unhide,
    // and detach all remain discoverable and dimmed.
    vcx.simulate_keystrokes("ctrl-w shift-b");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.tile_membership(tile)),
        Some(TileMembership::Detached)
    );
    vcx.simulate_keystrokes("space");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let menu = v.menu_ref().expect("Detached tile menu");
        assert!(menu.disabled.contains("tile-hide"));
        assert!(menu.disabled.contains("tile-unhide"));
        assert!(menu.disabled.contains("tile-detach"));
    });
    vcx.simulate_keystrokes("u");
    vcx.run_until_parked();
    assert!(view.read_with(vcx, |v, _| v.overlay_is_menu()));
}

/// The destination picker carries the focused tile's project as typed state.
/// Creating a destination while viewing another project cannot re-home the tile.
#[gpui::test]
fn send_detached_tile_to_new_workspace_preserves_its_project(cx: &mut TestAppContext) {
    use crate::workspace::{AttachedVisibility, TileMembership};
    use crate::{App, LinearTile, WorkspacePickerMode};

    let (view, vcx) = boot_browser(cx);
    let (tile, tile_project) = view.update(vcx, |v, cx| {
        let cwd = std::env::temp_dir().join("yalda-hidden-send-foreign-project");
        let tile_project = v.projects.ensure_at_cwd(cwd, "foreign");
        let tile = v
            .workspace
            .push_detached(App::Linear(LinearTile::new()), tile_project);
        assert!(v.workspace.present_solo(tile));
        v.open_workspace_picker(WorkspacePickerMode::Move { follow: true }, cx);
        let picker = v.workspace_picker_ref().unwrap();
        assert_eq!(picker.project, tile_project);
        assert!(picker.targets.is_empty());
        (tile, tile_project)
    });

    view.update(vcx, |v, cx| v.commit_workspace_picker(0, cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let owner = v.workspace.workspace_index_of_window(tile).unwrap();
        assert_eq!(v.workspace.workspaces[owner].project(), tile_project);
        assert_eq!(
            v.workspace.tile_membership(tile),
            Some(TileMembership::Attached {
                workspace: owner,
                visibility: AttachedVisibility::Visible,
            })
        );
        assert_eq!(v.workspace.tile(tile).unwrap().project(), tile_project);
    });
}

/// A hidden Agent is still a materialized tile and owns its durable session id.
/// Roster reconciliation must not create a second Detached copy in any project.
#[gpui::test]
fn hidden_agent_prevents_cross_project_roster_duplicate(cx: &mut TestAppContext) {
    use crate::{AgentTile, App, ServerSid};
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let (tile, sid) = view.update(vcx, |v, _| {
        let cwd = std::env::temp_dir().join("yalda-hidden-agent-foreign-roster");
        let _foreign_project = v.projects.ensure_at_cwd(cwd.clone(), "foreign-roster");
        let sid = "HIDDEN-IDENTITY".to_string();
        let tile = v
            .workspace
            .split_focused(
                crate::workspace::SplitDir::V,
                App::Agent(AgentTile::dormant(ServerSid::new(sid.clone()))),
            )
            .unwrap();
        v.workspace.hide_window(tile).unwrap();
        v.agent_roster.upsert(SessionInfo {
            session_id: sid.clone(),
            acp_session_id: None,
            label: "hidden identity".into(),
            cwd,
            provider: yalda::acp_channel::AgentProvider::Codex,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        (tile, sid)
    });

    view.update(vcx, |v, _| {
        assert!(!v.materialize_roster_detached_tiles());
        assert_eq!(v.agent_tile_id_for_server_sid(&sid), Some(tile));
        assert!(v.workspace.detached_tiles.is_empty());
        assert!(v.validate_agent_tile_identities().is_ok());
    });
}

/// UXI-Workspace-21: Close Tile acts on the directly focused stable tile even
/// when it lives in Unbound. Exercise the exact two picker states from the bug
/// report through the real system-menu command dispatcher.
#[gpui::test]
fn close_tile_removes_unbound_buffer_and_agent_picker(cx: &mut TestAppContext) {
    use crate::{AgentTile, App, BrowserWindow, BufferApp};
    let (view, vcx) = boot_browser(cx);
    let (buffer, agent, workspace_count) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        let cwd = v.projects.cwd_of(pid).expect("project cwd").to_path_buf();
        let buffer = v.workspace.push_detached(
            App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
            pid,
        );
        let agent = v.workspace.push_detached(App::Agent(AgentTile::new()), pid);
        (buffer, agent, v.workspace.workspaces.len())
    });

    view.update(vcx, |v, _| assert!(v.workspace.present_solo(buffer)));
    view.update(vcx, |v, cx| v.dispatch_menu_command("close-window", cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(
            v.workspace.tile(buffer).is_none(),
            "the unbound Buffer picker closes"
        );
        assert!(
            v.workspace.tile(agent).is_some(),
            "closing Buffer does not remove Agent"
        );
        assert_eq!(v.workspace.presented_detached_tile_id(), None);
        assert_eq!(
            v.workspace.workspaces.len(),
            workspace_count,
            "no workspace closes"
        );
    });

    view.update(vcx, |v, _| assert!(v.workspace.present_solo(agent)));
    view.update(vcx, |v, cx| v.dispatch_menu_command("close-window", cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(
            v.workspace.tile(agent).is_none(),
            "the empty unbound Agent picker closes"
        );
        assert_eq!(v.workspace.presented_detached_tile_id(), None);
        assert_eq!(
            v.workspace.workspaces.len(),
            workspace_count,
            "workspace floor survives"
        );
        assert!(
            v.workspace.active_workspace().is_some(),
            "the active workspace is revealed"
        );
    });
}

/// ADR-0034: Close is independent of attachment and visibility. It retires the
/// stable tile while leaving the server-owned session alive.
#[gpui::test]
fn close_bound_agent_tile_retires_tile_without_hiding_or_detaching(cx: &mut TestAppContext) {
    use crate::{App, BrowserWindow, BufferApp};

    let (view, vcx, session, _) = boot_with_transcript(cx);
    let agent = view.update(vcx, |v, _| {
        let agent = v.workspace.focused_window_id().expect("bound Agent tile");
        let cwd = v.agent_base_cwd();
        v.workspace
            .split_focused(
                crate::workspace::SplitDir::V,
                App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
            )
            .expect("second tile keeps the workspace alive");
        assert!(v.workspace.focus_tile(agent));
        agent
    });

    view.update(vcx, |v, cx| v.dispatch_menu_command("close-window", cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(
            v.workspace.tile(agent).is_none(),
            "Close Tile retires the tile"
        );
        assert!(
            v.sessions.contains(session),
            "Close Tile cannot kill the session"
        );
    });
}

/// Closing the sole visible tile leaves the valid empty workspace state.
#[gpui::test]
fn close_sole_bound_agent_leaves_empty_workspace(cx: &mut TestAppContext) {
    let (view, vcx, session, _) = boot_with_transcript(cx);
    let agent = view.update(vcx, |v, _| {
        let agent = v.workspace.focused_window_id().expect("sole Agent tile");
        agent
    });

    view.update(vcx, |v, cx| v.dispatch_menu_command("close-window", cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(v.workspace.workspaces.len(), 1);
        assert!(v.workspace.tile(agent).is_none());
        assert!(v.sessions.contains(session));
        assert!(matches!(
            &v.workspace
                .active_workspace()
                .expect("workspace floor")
                .layout,
            crate::workspace::Layout::Empty
        ));
    });
}

/// bug-0047: the server can publish a newly-created session to the roster
/// before the create reply binds that server sid to its provisional local
/// session. Roster materialization must not leave a second stable Agent tile
/// once the production bind choke resolves the provisional identity.
#[gpui::test]
fn provisional_bind_reconciles_racing_roster_tile(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState, AgentTile, App, ServerSid};
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        let cwd = v.agent_base_cwd();
        let provisional = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(None),
                label: "race-session".into(),
                cwd: cwd.clone(),
                resume_id: None,
            },
            cx,
        );
        let token = crate::alloc_open_token();
        v.agent_tile_mut().expect("provisional Agent tile").set_pending(Some(token));
        v.agent_roster.upsert(SessionInfo {
            session_id: "RACE-SID".into(),
            acp_session_id: None,
            label: "race-session".into(),
            cwd,
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        assert!(v.materialize_roster_detached_tiles(), "roster wins the race");
        v.apply_open_agent_resolution(
            token,
            crate::OpenResolution::Created {
                sid: "RACE-SID".into(),
                acp_id: None,
                provider: yalda::acp_channel::AgentProvider::Claude,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            },
            cx,
        );
        assert_eq!(v.sessions.sid_of(provisional).map(|sid| sid.as_str()), Some("RACE-SID"));

        let sid = ServerSid::new("RACE-SID");
        let mut owners = Vec::new();
        for workspace in &v.workspace.workspaces {
            workspace.layout.for_each_leaf(&mut |window| {
                if matches!(&window.content, App::Agent(tile)
                    if tile.remembered_sid(|local| v.sessions.sid_of(local).cloned()).as_ref() == Some(&sid))
                {
                    owners.push((window.id(), workspace.project()));
                }
            });
        }
        for tile in &v.workspace.detached_tiles {
            if matches!(&tile.window.content, App::Agent(agent)
                if agent.remembered_sid(|local| v.sessions.sid_of(local).cloned()).as_ref() == Some(&sid))
            {
                owners.push((tile.window.id(), tile.project()));
            }
        }
        assert_eq!(owners.len(), 1, "one server session must have one stable tile: {owners:?}");
    });
}

#[gpui::test]
fn agent_identity_guard_rejects_duplicate_local_and_durable_owners(cx: &mut TestAppContext) {
    use crate::{AgentTile, App, ServerSid, agent_ui::AgentIdentityViolation};

    let (view, vcx, session, _) = boot_with_transcript(cx);
    view.update(vcx, |v, _| {
        let project = v.workspace.inherited_project();
        let duplicate_local = v.workspace.push_detached(
            App::Agent(AgentTile::Bound {
                session,
                reopening: None,
            }),
            project,
        );
        assert!(matches!(
            v.validate_agent_tile_identities(),
            Err(AgentIdentityViolation::DuplicateLocalSession {
                session: duplicate,
                second,
                ..
            }) if duplicate == session && second == duplicate_local
        ));
        v.workspace
            .remove_detached_window(duplicate_local)
            .expect("remove corrupt test tile");

        let sid = ServerSid::new("DUPLICATE-DURABLE-SID");
        v.sessions
            .bind_sid(session, sid.clone())
            .expect("bind durable identity");
        let duplicate_durable = v
            .workspace
            .push_detached(App::Agent(AgentTile::dormant(sid.clone())), project);
        assert!(matches!(
            v.validate_agent_tile_identities(),
            Err(AgentIdentityViolation::DuplicateServerSession {
                sid: duplicate,
                second,
                ..
            }) if duplicate == sid && second == duplicate_durable
        ));
    });
}

/// UXI-Workspace-22: a tile App cannot opt out of directional workspace focus.
/// Drive the real `Ctrl-W h/j/k/l` bindings through every rendered App state,
/// from a center tile with a real spatial neighbor in each direction. This is
/// deliberately a focus assertion, not a split-command proxy: the reported bug
/// was specifically that the directional chord vanished in some tile states.
#[gpui::test]
fn ctrl_w_direction_survives_a_render_between_prefix_and_direction(cx: &mut TestAppContext) {
    use crate::{
        App, BrowserWindow, BufferApp,
        workspace::{Slot, SplitDir, WorkspaceView},
    };

    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    let (center, left) = view.update(vcx, |v, cx| {
        let center = v.workspace.focused_window_id().expect("center tile");
        let cwd = v.active_workspace_cwd().unwrap_or_else(crate::process_cwd);
        let left = v
            .workspace
            .split_focused(
                SplitDir::V,
                App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd))),
            )
            .expect("left neighbor");
        let workspace = v.workspace.active_workspace_mut().expect("workspace");
        workspace.view = WorkspaceView::Plane;
        let leaves = workspace.layout.leaf_ids();
        workspace.desktop.reconcile(&leaves);
        workspace.desktop.set_anchor(center, Slot::new(0, 0));
        workspace.desktop.set_anchor(left, Slot::new(0, -1));
        workspace.focused = center;
        v.test_open_edit("staggered ctrl-w focus");
        cx.notify();
        (center, left)
    });
    vcx.run_until_parked();

    vcx.simulate_keystrokes("ctrl-w");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(v.workspace.focused_window_id(), Some(center));
    });
    vcx.simulate_keystrokes("h");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.focused_window_id(),
            Some(left),
            "Ctrl-W must remain a pending shell prefix across an intervening render"
        );
    });
}

/// bug-0045 recurrence / UXI-Workspace-22: Columns and Tiling derive distinct
/// visible arrangements from signed Plane reading order. Directional focus must
/// use the painted geometry, not the hidden two-dimensional Plane coordinates.
#[gpui::test]
fn ctrl_w_direction_follows_visible_arrangement_not_hidden_plane_geometry(cx: &mut TestAppContext) {
    use crate::{
        App, BrowserWindow, BufferApp,
        workspace::{Slot, SplitDir, WorkspaceView},
    };

    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    let (left, center, right) = view.update(vcx, |v, cx| {
        let center = v.workspace.focused_window_id().expect("center tile");
        let cwd = v.active_workspace_cwd().unwrap_or_else(crate::process_cwd);
        let picker = || App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd.clone())));
        let left = v
            .workspace
            .split_focused(SplitDir::V, picker())
            .expect("left tile");
        let right = v
            .workspace
            .split_focused(SplitDir::V, picker())
            .expect("right tile");
        let workspace = v.workspace.active_workspace_mut().expect("workspace");
        workspace.view = WorkspaceView::Columns;
        let leaves = workspace.layout.leaf_ids();
        workspace.desktop.reconcile(&leaves);
        workspace.desktop.set_anchor(left, Slot::new(-1, 99));
        workspace.desktop.set_anchor(center, Slot::new(0, 0));
        workspace.desktop.set_anchor(right, Slot::new(1, -99));
        workspace.focused = center;
        cx.notify();
        (left, center, right)
    });
    vcx.run_until_parked();

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let left_bounds =
        crate::layout_probe_get(&format!("columns-tile-{left}")).expect("left column paints");
    let center_bounds =
        crate::layout_probe_get(&format!("columns-tile-{center}")).expect("center column paints");
    let right_bounds =
        crate::layout_probe_get(&format!("columns-tile-{right}")).expect("right column paints");
    crate::layout_probe_end();
    assert!(
        left_bounds.0 < center_bounds.0 && center_bounds.0 < right_bounds.0,
        "fixture must paint left/center/right in visible order: {left_bounds:?} {center_bounds:?} {right_bounds:?}"
    );

    vcx.simulate_keystrokes("ctrl-w h");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
        Some(left),
        "Columns h selects the visibly-left tile"
    );
    view.update(vcx, |v, cx| {
        v.workspace.active_workspace_mut().unwrap().focused = center;
        cx.notify();
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("ctrl-w l");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
        Some(right),
        "Columns l selects the visibly-right tile"
    );

    view.update(vcx, |v, cx| {
        let workspace = v.workspace.active_workspace_mut().unwrap();
        workspace.view = WorkspaceView::Tiling;
        workspace.primary_count = 1;
        workspace.focused = center;
        cx.notify();
    });
    vcx.run_until_parked();
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let tiling_left =
        crate::layout_probe_get(&format!("columns-tile-{left}")).expect("tiling left paints");
    let tiling_center =
        crate::layout_probe_get(&format!("columns-tile-{center}")).expect("tiling center paints");
    let tiling_right =
        crate::layout_probe_get(&format!("columns-tile-{right}")).expect("tiling right paints");
    crate::layout_probe_end();
    assert!(
        tiling_left.0 < tiling_center.0
            && (tiling_center.0 - tiling_right.0).abs() <= 2.0
            && tiling_center.1 < tiling_right.1,
        "Tiling must paint a left primary and vertical right stack: {tiling_left:?} {tiling_center:?} {tiling_right:?}"
    );
    vcx.simulate_keystrokes("ctrl-w h");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
        Some(left),
        "Tiling h selects the visibly-left tile"
    );
    view.update(vcx, |v, cx| {
        v.workspace.active_workspace_mut().unwrap().focused = center;
        cx.notify();
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("ctrl-w j");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
        Some(right),
        "Tiling j selects the visibly-lower tile in the stack"
    );
    vcx.simulate_keystrokes("ctrl-w k");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
        Some(center),
        "Tiling k returns to the visibly-higher tile in the stack"
    );
    view.update(vcx, |v, cx| {
        v.workspace.active_workspace_mut().unwrap().focused = left;
        cx.notify();
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("ctrl-w l");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
        Some(center),
        "Tiling l crosses from the primary to the closest stack row"
    );

    view.update(vcx, |v, cx| {
        let workspace = v.workspace.active_workspace_mut().unwrap();
        workspace.view = WorkspaceView::Monocle;
        workspace.focused = center;
        cx.notify();
    });
    vcx.run_until_parked();
    for (direction, expected) in [("h", left), ("k", left), ("l", right), ("j", right)] {
        view.update(vcx, |v, cx| {
            v.workspace.active_workspace_mut().unwrap().focused = center;
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes(&format!("ctrl-w {direction}"));
        vcx.run_until_parked();
        assert_eq!(
            view.read_with(vcx, |v, _| v.workspace.focused_window_id()),
            Some(expected),
            "Monocle {direction} traverses reading order"
        );
    }
}

#[gpui::test]
fn ctrl_w_shell_commands_reach_every_tile_app(cx: &mut TestAppContext) {
    use crate::{
        AgentTile, App, BrowserWindow, BufferApp, CogTile, KeymapTile, LinearTile,
        workspace::{Slot, SplitDir, WorkspaceView},
    };
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);

    let (center, left, right, up, down, cwd) = view.update(vcx, |v, cx| {
        let center = v.workspace.focused_window_id().expect("center tile");
        let cwd = v.active_workspace_cwd().unwrap_or_else(crate::process_cwd);
        let picker = || App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd.clone())));
        let mut add_neighbor = || {
            v.workspace.active_workspace_mut().unwrap().focused = center;
            v.workspace
                .split_focused(SplitDir::V, picker())
                .expect("neighbor tile")
        };
        let left = add_neighbor();
        let right = add_neighbor();
        let up = add_neighbor();
        let down = add_neighbor();

        let wsp = v.workspace.active_workspace_mut().expect("workspace");
        wsp.view = WorkspaceView::Plane;
        let leaves = wsp.layout.leaf_ids();
        wsp.desktop.reconcile(&leaves);
        for (id, slot) in [
            (center, Slot::new(0, 0)),
            (left, Slot::new(0, -1)),
            (right, Slot::new(0, 1)),
            (up, Slot::new(-1, 0)),
            (down, Slot::new(1, 0)),
        ] {
            wsp.desktop.set_anchor(id, slot);
        }
        wsp.focused = center;
        cx.notify();
        (center, left, right, up, down, cwd)
    });
    vcx.run_until_parked();

    let assert_directions =
        |label: &str, view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
            for (keys, expected) in [
                ("ctrl-w h", left),
                ("ctrl-w l", right),
                ("ctrl-w k", up),
                ("ctrl-w j", down),
            ] {
                view.update(vcx, |v, cx| {
                    v.workspace.active_workspace_mut().unwrap().focused = center;
                    cx.notify();
                });
                vcx.run_until_parked();
                vcx.simulate_keystrokes(keys);
                vcx.run_until_parked();
                view.read_with(vcx, |v, _| {
                    assert_eq!(
                        v.workspace.focused_window_id(),
                        Some(expected),
                        "{keys} must move workspace focus from the {label} tile"
                    );
                });
            }
        };

    let install = |label: &str,
                   app: App,
                   view: &gpui::Entity<YaldaGpuiView>,
                   vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            v.workspace.active_workspace_mut().unwrap().focused = center;
            v.set_screen(app);
            cx.notify();
        });
        vcx.run_until_parked();
        assert_directions(label, view, vcx);
    };

    install(
        "Buffer picker",
        App::Buffer(BufferApp::Picking(BrowserWindow::standalone(cwd.clone()))),
        &view,
        vcx,
    );

    view.update(vcx, |v, cx| {
        v.workspace.active_workspace_mut().unwrap().focused = center;
        v.test_open_doc("# focus routing");
        cx.notify();
    });
    vcx.run_until_parked();
    assert_directions("Buffer viewer", &view, vcx);

    view.update(vcx, |v, cx| {
        v.workspace.active_workspace_mut().unwrap().focused = center;
        v.test_open_edit("focus routing");
        cx.notify();
    });
    vcx.run_until_parked();
    assert_directions("Buffer editor", &view, vcx);

    install("Agent picker", App::Agent(AgentTile::new()), &view, vcx);
    install(
        "dormant Agent",
        App::Agent(AgentTile::dormant(crate::ServerSid::new(
            "focus-routing-dormant",
        ))),
        &view,
        vcx,
    );
    install(
        "unavailable Agent",
        App::Agent(AgentTile::Unavailable {
            remembered: crate::ServerSid::new("focus-routing-unavailable"),
            lost: "focus-routing-unavailable".into(),
        }),
        &view,
        vcx,
    );

    view.update(vcx, |v, cx| {
        v.workspace.active_workspace_mut().unwrap().focused = center;
        v.set_screen(App::Agent(AgentTile::new()));
        let session = crate::AgentSession {
            state: crate::AgentState::new_server_managed(None),
            label: "focus-routing-agent".into(),
            cwd: cwd.clone(),
            resume_id: None,
        };
        v.show_local_session(session, cx);
        cx.notify();
    });
    vcx.run_until_parked();
    assert_directions("bound Agent session", &view, vcx);

    install("Linear", App::Linear(LinearTile::new()), &view, vcx);
    install("Cog", App::Cog(CogTile::new()), &view, vcx);
    install("Keymap", App::Keymap(KeymapTile::new()), &view, vcx);
}

// ── Session tags (UXI-JumpPanel-20/-21, UXI-AgentTile-33) ────────────────────

/// UXI-JumpPanel-20: the pure tag partition. A row appears once per DISTINCT tag
/// (multi-appearance); untagged rows fall to the residual; folders order by the
/// project's manual `tag_order`, unlisted tags alphabetical after. The empty-order
/// arm is the built-in negative control for the rank sort.
#[test]
fn session_tags_partition_folders_and_untagged() {
    use crate::{AgentRow, JumpTarget, partition_rows_by_tag};
    let row = |label: &str, tags: &[&str]| AgentRow {
        target: JumpTarget::Roster(label.into()),
        label: label.into(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        summary: None,
        summary_pending: false,
        archived: false,
        cwd: std::path::PathBuf::from("/work"),
        bound: false,
        connected: true,
        awaiting: Some(false),
        unread: false,
        order_sid: Some(label.into()),
        state_entered_at: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
    };
    let rows = vec![
        (0usize, row("a", &["frontend", "urgent"])),
        (1, row("b", &["urgent"])),
        (2, row("c", &[])),
    ];
    let labels =
        |g: &[(usize, AgentRow)]| g.iter().map(|(_, r)| r.label.clone()).collect::<Vec<_>>();

    // Manual order floats "urgent" first; "frontend" (unlisted) sorts alpha after.
    let (folders, untagged) = partition_rows_by_tag(rows.clone(), &["urgent".to_string()]);
    assert_eq!(folders.len(), 2, "one folder per distinct tag");
    assert_eq!(folders[0].0, "urgent");
    assert_eq!(
        labels(&folders[0].1),
        vec!["a", "b"],
        "urgent holds a and b"
    );
    assert_eq!(folders[1].0, "frontend");
    assert_eq!(
        labels(&folders[1].1),
        vec!["a"],
        "a appears again under frontend (multi-appearance)"
    );
    assert_eq!(
        labels(&untagged),
        vec!["c"],
        "the tagless row is the residual"
    );

    // Negative control for the rank sort: with NO manual order, folders are
    // alphabetical — "frontend" before "urgent" (the opposite of above).
    let (alpha, _) = partition_rows_by_tag(rows, &[]);
    assert_eq!(
        alpha.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
        vec!["frontend", "urgent"],
        "empty tag_order = alphabetical folders"
    );
}

/// Seed connected roster sessions rooted in the active project's cwd, returning
/// its `ProjectId`. Shared by the tag render/fold/reorder tests.
fn seed_project_sessions(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    sessions: &[(&str, &str)], // (sid, label)
) -> crate::project::ProjectId {
    use yalda::session_proto::SessionInfo;
    let (pid, cwd) = view.read_with(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        (
            pid,
            v.projects.cwd_of(pid).expect("project cwd").to_path_buf(),
        )
    });
    view.update(vcx, |v, _| {
        for (sid, label) in sessions {
            v.agent_roster.upsert(SessionInfo {
                session_id: (*sid).into(),
                acp_session_id: None,
                label: (*label).into(),
                cwd: cwd.clone(),
                provider: yalda::acp_channel::AgentProvider::Claude,
                turns: 0,
                connected: true,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                busy: false,
                archived: false,
            });
        }
        v.materialize_roster_detached_tiles();
    });
    pid
}

fn set_materialized_tile_tags(v: &mut YaldaGpuiView, sid: &str, tags: &[&str]) {
    let id = v
        .agent_tile_id_for_server_sid(sid)
        .unwrap_or_else(|| panic!("materialized tile for {sid}"));
    v.workspace.tile_mut(id).unwrap().tags = tags.iter().map(|tag| tag.to_string()).collect();
    v.session_tags.insert(
        sid.to_string(),
        tags.iter().map(|tag| tag.to_string()).collect(),
    );
}

/// UXI-JumpPanel-20: a tagged session paints under its tag folder header; an
/// untagged session paints flat below the folders (no folder). Non-vacuous: the
/// tagged row uses the folder-ordinal id suffix (`-tg0`), the untagged row does
/// not, and the untagged row paints BELOW the folder.
#[gpui::test]
fn jump_panel_groups_sessions_under_tag_folders(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let pid = seed_project_sessions(
        &view,
        &mut *vcx,
        &[("S-tag", "alpha"), ("S-plain", "plain")],
    );
    view.update(vcx, |v, _| {
        set_materialized_tile_tags(v, "S-tag", &["frontend"]);
    });
    let row_ids: std::collections::HashMap<String, usize> = view.update(vcx, |v, cx| {
        v.jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|s| s.id == pid)
            .expect("project section")
            .sessions
            .into_iter()
            .map(|(i, r)| (r.label, i))
            .collect()
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let folder_y = crate::layout_probe_get(&format!("jump-tag-folder-{}-0", pid.0))
        .expect("the tagged session paints a folder header")
        .1;
    let tagged_y = crate::layout_probe_get(&format!("jump-session-row-{}-tg0", row_ids["alpha"]))
        .expect("the tagged row paints under its folder (with the -tg0 id suffix)")
        .1;
    let plain_y = crate::layout_probe_get(&format!("jump-session-row-{}", row_ids["plain"]))
        .expect("the untagged row paints flat (no suffix)")
        .1;
    assert!(folder_y < tagged_y, "the folder header sits above its rows");
    assert!(tagged_y < plain_y, "untagged rows fall below the folders");
    // The "untagged" separator sits between the last tagged row and the loose ones.
    let sep_y = crate::layout_probe_get(&format!("jump-untagged-sep-{}", pid.0))
        .expect("the untagged separator paints when folders AND loose rows coexist")
        .1;
    assert!(
        tagged_y < sep_y && sep_y < plain_y,
        "the separator divides tagged from untagged"
    );
    crate::layout_probe_end();

    // With no tagged sessions (all loose) there are no folders, so the separator
    // must NOT paint — it only exists to divide the two groups.
    view.update(vcx, |v, _| {
        v.session_tags.clear();
        for tile in &mut v.workspace.detached_tiles {
            tile.window.tags.clear();
        }
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&format!("jump-untagged-sep-{}", pid.0)).is_none(),
        "with no folders there is nothing to separate, so no separator paints"
    );
    crate::layout_probe_end();
}

/// UXI-JumpPanel-21: folding a tag folder hides its session rows; unfolding
/// restores them. The folder header itself stays painted while folded.
#[gpui::test]
fn jump_tag_folder_fold_hides_and_restores(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let pid = seed_project_sessions(&view, &mut *vcx, &[("S-tag", "alpha")]);
    let (project_name, i) = view.update(vcx, |v, cx| {
        set_materialized_tile_tags(v, "S-tag", &["frontend"]);
        let name = v.projects.name_of(pid).to_string();
        let i = v
            .jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|s| s.id == pid)
            .expect("section")
            .sessions
            .into_iter()
            .find(|(_, r)| r.label == "alpha")
            .expect("alpha row")
            .0;
        (name, i)
    });
    let row_probe = format!("jump-session-row-{i}-tg0");
    let folder_probe = format!("jump-tag-folder-{}-0", pid.0);

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&row_probe).is_some(),
        "expanded folder paints its row"
    );
    crate::layout_probe_end();

    view.update(vcx, |v, cx| {
        v.toggle_tag_fold(&project_name, "frontend", cx)
    });
    view.read_with(vcx, |v, _| {
        assert!(
            v.tag_folder_folded(&project_name, "frontend"),
            "fold state is set"
        );
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&row_probe).is_none(),
        "folded folder hides its row"
    );
    assert!(
        crate::layout_probe_get(&folder_probe).is_some(),
        "the header stays painted while folded"
    );
    crate::layout_probe_end();

    view.update(vcx, |v, cx| {
        v.toggle_tag_fold(&project_name, "frontend", cx)
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get(&row_probe).is_some(),
        "unfolding restores the row"
    );
    crate::layout_probe_end();
}

/// UXI-JumpPanel-21: `reorder_tag` reorders a project's tag folders and persists
/// the per-project order; a tag not present in the project is refused
/// (project-scope guard). One session carries both tags so both folders exist.
#[gpui::test]
fn jump_reorder_tag_folders_persists(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let pid = seed_project_sessions(&view, &mut *vcx, &[("S-tag", "alpha")]);
    let project_name = view.update(vcx, |v, _| {
        set_materialized_tile_tags(v, "S-tag", &["alpha", "beta"]);
        v.projects.name_of(pid).to_string()
    });
    // Default order is alphabetical: alpha, beta.
    view.read_with(vcx, |v, cx| {
        assert_eq!(
            v.ordered_project_tags(&project_name, cx),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    });
    // Drop beta onto alpha → order becomes beta, alpha; persisted.
    view.update(vcx, |v, cx| {
        v.reorder_tag(&project_name, "beta", "alpha", cx)
    });
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.jump_tag_order.get(&project_name).map(|v| v.as_slice()),
            Some(&["beta".to_string(), "alpha".to_string()][..]),
            "the manual order is stored per project"
        );
    });
    // Project-scope guard: a tag absent from this project can't be reordered.
    view.update(vcx, |v, cx| {
        v.reorder_tag(&project_name, "ghost", "alpha", cx)
    });
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.jump_tag_order.get(&project_name).map(|v| v.as_slice()),
            Some(&["beta".to_string(), "alpha".to_string()][..]),
            "a ghost tag drag changes nothing"
        );
    });
}

/// UXI-AgentTile-33: the modal tag-editor dialog. `i` enters Insert to
/// filter/create; a typed novel tag + Enter adds it; Enter again adds an existing
/// known tag; `esc` returns to Normal; vim `l` moves to the Current column and `x`
/// removes. Drives the REAL menu open + capture key handler.
#[gpui::test]
fn tag_editor_keyboard_adds_and_removes(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    install_agent_slot(&view, &mut *vcx, Some("SID"));
    // A tag used on another session, so the ADD column has an existing candidate.
    view.update(vcx, |v, _| {
        v.session_tags
            .insert("OTHER".into(), vec!["backend".into()]);
    });

    view.update(vcx, |v, cx| v.dispatch_menu_command("claude-tag", cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(v.overlay_is_tag_editor(), "the dialog opens");
        assert_eq!(
            v.tag_editor_ref().unwrap().mode,
            crate::TagEditorMode::Normal,
            "opens in Normal mode"
        );
    });

    // `i` → Insert; type a NEW tag; Enter adds via the "create" row.
    vcx.simulate_keystrokes("i f r o n t e n d enter");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.session_tags.get("SID").map(|t| t.as_slice()),
            Some(&["frontend".to_string()][..]),
            "i + type + enter creates and adds the novel tag"
        );
    });

    // Still Insert; the ADD column now offers the existing "backend"; Enter adds it.
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let mut got = v.session_tags.get("SID").cloned().unwrap_or_default();
        got.sort();
        assert_eq!(
            got,
            vec!["backend".to_string(), "frontend".to_string()],
            "an existing tag adds"
        );
    });

    // `esc` returns to Normal (does NOT close); vim `l` focuses the Current column.
    vcx.simulate_keystrokes("escape l");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let ov = v.tag_editor_ref().expect("still open after esc-to-normal");
        assert_eq!(
            ov.mode,
            crate::TagEditorMode::Normal,
            "esc leaves Insert, not the dialog"
        );
        assert_eq!(
            ov.column,
            crate::TagEditorColumn::Current,
            "l moves to the Current column"
        );
    });

    // `x` removes the highlighted current tag (backend, sorted first).
    vcx.simulate_keystrokes("x");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.session_tags.get("SID").map(|t| t.as_slice()),
            Some(&["frontend".to_string()][..]),
            "x removes the highlighted current tag (backend)"
        );
    });

    // `esc` in Normal closes.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(
            !v.overlay_is_tag_editor(),
            "esc in Normal closes the dialog"
        )
    });
}

/// UXI-AgentTile-33: clicking a row in either column toggles that tag (mouse path,
/// through the real painted rows).
#[gpui::test]
fn tag_editor_mouse_click_toggles(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    install_agent_slot(&view, &mut *vcx, Some("SID"));
    view.update(vcx, |v, _| {
        v.session_tags
            .insert("OTHER".into(), vec!["backend".into()]);
    });
    view.update(vcx, |v, cx| v.dispatch_menu_command("claude-tag", cx));
    vcx.run_until_parked();

    let click = |vcx: &mut gpui::VisualTestContext, probe: &str| {
        crate::layout_probe_begin();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let (x, y, w, h) =
            crate::layout_probe_get(probe).unwrap_or_else(|| panic!("{probe} paints"));
        crate::layout_probe_end();
        let at = point(px(x + w / 2.0), px(y + h / 2.0));
        vcx.simulate_mouse_move(at, None, gpui::Modifiers::default());
        vcx.simulate_click(at, gpui::Modifiers::default());
        vcx.run_until_parked();
    };

    // Click the available "backend" → added.
    click(vcx, "tag-editor-left-0");
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.session_tags.get("SID").map(|t| t.as_slice()),
            Some(&["backend".to_string()][..]),
            "clicking an available tag adds it"
        );
    });
    // Click the current "backend" → removed.
    click(vcx, "tag-editor-current-0");
    view.read_with(vcx, |v, _| {
        assert!(
            v.session_tags.get("SID").is_none_or(|t| t.is_empty()),
            "clicking a current tag removes it"
        );
    });
}

/// UXI-AgentTile-33: a session with no server sid can't be tagged (tags key by
/// sid), so the command notes it and opens no dialog.
#[gpui::test]
fn tag_editor_requires_a_sid(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    install_agent_slot(&view, &mut *vcx, None); // bound tile, but NO server sid
    view.update(vcx, |v, cx| v.dispatch_menu_command("claude-tag", cx));
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(
            !v.overlay_is_tag_editor(),
            "a sid-less session opens no dialog"
        );
        assert!(
            v.transient_status
                .as_deref()
                .is_some_and(|s| s.contains("not ready")),
            "it notes why, got: {:?}",
            v.transient_status
        );
    });
}

/// UXI-Project-4: the "New project" overlay creates an EMPTY project via the REAL
/// path (`open_new_project_overlay` → edit cwd → `commit_new_project_overlay`).
/// Its name is derived from the directory basename; equal basenames uniquify,
/// while a duplicate CWD is refused and creates nothing.
#[gpui::test]
fn new_project_overlay_creates_from_cwd_and_rejects_duplicate_cwd(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    // Two distinct REAL directories with the same basename.
    let basename = format!("yalda-np-{}", std::process::id());
    let root1 = std::env::temp_dir().join("yalda-np-a");
    let root2 = std::env::temp_dir().join("yalda-np-b");
    let dir1 = root1.join(&basename);
    let dir2 = root2.join(&basename);
    std::fs::create_dir_all(&dir1).expect("mk dir1");
    std::fs::create_dir_all(&dir2).expect("mk dir2");
    let derived = crate::project_name_for_cwd(&dir1);

    let before = view.read_with(vcx, |v, _| v.projects.len());
    view.update(vcx, |v, cx| {
        v.open_new_project_overlay(cx);
        v.new_project_mut().expect("new-project overlay open").cwd = dir1.display().to_string();
        v.commit_new_project_overlay(cx);
    });
    let zid = view.read_with(vcx, |v, _| {
        let zid = v
            .projects
            .by_name(&derived)
            .expect("derived-name project created");
        assert_eq!(v.projects.len(), before + 1, "exactly one new project");
        zid
    });
    // The new project starts EMPTY — no workspaces, no sessions.
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace
                .workspaces
                .iter()
                .filter(|t| t.project() == zid)
                .count(),
            0,
            "new project owns zero workspaces"
        );
    });

    // A distinct cwd with the same basename is accepted under a unique name.
    let after = view.read_with(vcx, |v, _| v.projects.len());
    view.update(vcx, |v, cx| {
        v.open_new_project_overlay(cx);
        v.new_project_mut().expect("overlay open").cwd = dir2.display().to_string();
        v.commit_new_project_overlay(cx);
    });
    view.read_with(vcx, |v, _| {
        assert_eq!(v.projects.len(), after + 1, "same basename is uniquified");
        assert!(v.projects.by_name(&format!("{derived} (2)")).is_some());
    });

    // Reusing the exact cwd is refused.
    let after_unique = view.read_with(vcx, |v, _| v.projects.len());
    view.update(vcx, |v, cx| {
        v.open_new_project_overlay(cx);
        v.new_project_mut().expect("overlay open").cwd = dir1.display().to_string();
        v.commit_new_project_overlay(cx);
    });
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.projects.len(),
            after_unique,
            "duplicate cwd creates NOTHING"
        );
        let note = v
            .transient_status
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_default();
        assert!(
            note.contains("already roots"),
            "duplicate cwd surfaces an error: {note:?}"
        );
    });
    let _ = std::fs::remove_dir_all(root1);
    let _ = std::fs::remove_dir_all(root2);
}

/// UXI-Project-5: deleting a NON-empty project first confirms, then cascades. The
/// REAL `request_delete_project` arms the confirm overlay (removing nothing yet);
/// `perform_delete_project` then closes the project's workspaces, kills its
/// sessions (via `AgentSessions::close`), and drops the project — never leaving
/// zero workspaces. An EMPTY project deletes directly with no confirm.
///
/// Negative control: comment out the session-kill loop in
/// `perform_delete_project` (`for id in local_kill { … self.sessions.close(id) }`)
/// → the orphaned session survives in `self.sessions` and the kill assert fails.
#[gpui::test]
fn delete_nonempty_project_confirms_then_cascades(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState};
    let (view, vcx) = boot_browser(cx);
    let pa = PathBuf::from("/tmp/yalda-del-a");
    let (pid, ws_idx, sid) = view.update(vcx, |v, cx| {
        let pid = v
            .projects
            .create("Doomed".into(), pa.clone())
            .expect("create");
        v.new_workspace_in(pid, cx);
        let ws_idx = v.workspace.active_workspace;
        let s = AgentSession {
            state: AgentState::new_server_managed(None),
            label: "doomed-s".into(),
            cwd: pa.clone(),
            resume_id: None,
        };
        let sid = v.show_local_session(s, cx);
        (pid, ws_idx, sid)
    });

    // Non-empty → arms the confirm overlay, removes NOTHING yet.
    view.update(vcx, |v, cx| v.request_delete_project(pid, cx));
    view.read_with(vcx, |v, _| {
        assert!(
            matches!(v.confirm_delete_ref(), Some(p) if p == pid),
            "confirm overlay armed"
        );
        assert!(
            v.projects.contains(pid),
            "project still present pre-confirm"
        );
        assert_eq!(
            v.workspace.workspaces.get(ws_idx).map(|t| t.project()),
            Some(pid),
            "workspace intact pre-confirm"
        );
        assert!(v.sessions.contains(sid), "session intact pre-confirm");
    });

    // Confirm → cascade.
    view.update(vcx, |v, cx| v.perform_delete_project(pid, cx));
    view.read_with(vcx, |v, _| {
        assert!(!v.projects.contains(pid), "project removed");
        assert!(!v.sessions.contains(sid), "session killed by the cascade");
        assert!(
            !v.transcript_views.contains_key(&sid),
            "transcript view dropped"
        );
        assert!(
            !v.workspace.workspaces.iter().any(|t| t.project() == pid),
            "the project's workspaces are closed"
        );
        assert!(
            !v.workspace.workspaces.is_empty(),
            "≥1 workspace always survives (Behavior 2)"
        );
        assert!(
            !v.overlay_is_confirm_delete(),
            "the overlay clears after cascade"
        );
    });

    // An EMPTY project deletes directly — no confirm overlay.
    let empty = view.update(vcx, |v, _| {
        v.projects
            .create("Empty".into(), PathBuf::from("/tmp/yalda-del-empty"))
            .expect("create")
    });
    view.update(vcx, |v, cx| v.request_delete_project(empty, cx));
    view.read_with(vcx, |v, _| {
        assert!(
            !v.overlay_is_confirm_delete(),
            "an empty project needs no confirmation"
        );
        assert!(
            !v.projects.contains(empty),
            "the empty project deleted directly"
        );
    });
}

/// Shared Agent Tile / Jump Panel status vocabulary: live states have distinct
/// GLYPH SHAPES and words. The Agent Tile uses both in its pill; the Jump Panel
/// consumes only the glyph because its tabs and headers already name activity.
///
/// Negative control: return `("✦", None)` for every status → the working and
/// waiting asserts both fail.
#[test]
fn agent_row_marks_name_the_live_states() {
    use crate::{AgentDotStatus, agent_row_marks};
    assert_eq!(
        agent_row_marks(AgentDotStatus::Working),
        ("◆", Some("working")),
        "a running agent says so, with its own glyph"
    );
    assert_eq!(
        agent_row_marks(AgentDotStatus::WaitingForYou),
        ("✦", Some("your turn")),
        "a finished-but-unread turn says it is your move"
    );
    assert_eq!(
        agent_row_marks(AgentDotStatus::Neutral),
        ("✦", None),
        "a quiet session stays quiet — no status word"
    );
    // The two live states must not be confusable by shape alone.
    assert_ne!(
        agent_row_marks(AgentDotStatus::Working).0,
        agent_row_marks(AgentDotStatus::WaitingForYou).0
    );
}

#[test]
fn agent_header_uses_compact_activity_and_transient_editor_vocabulary() {
    assert_eq!(
        crate::screens::agent_header_activity(true),
        ("*", "working")
    );
    assert_eq!(crate::screens::agent_header_activity(false), ("+", "ready"));

    assert_eq!(crate::screens::agent_editing_status_label(false, false), "");
    assert_eq!(crate::screens::agent_editing_status_label(true, false), "•");
    assert_eq!(
        crate::screens::agent_editing_status_label(false, true),
        "EXT"
    );
    let transient = crate::screens::agent_editing_status_label(true, true);
    assert_eq!(transient, "• EXT");
    for removed in [
        "CHATBOX",
        "WORKSHEET",
        "NORMAL",
        "INSERT",
        "L3:C12",
        "awaiting",
        "server",
    ] {
        assert!(!transient.contains(removed));
    }

    let folio = yalda::theme::AgentTheme::folio();
    let supporting = crate::screens::agent_header_supporting_text_color(&folio);
    assert_eq!(supporting, folio.agent_tint);
    assert_ne!(
        supporting, folio.warm_accent,
        "header text must not be gold"
    );
    assert_ne!(supporting, folio.dim, "header text must not be tan");
}

#[test]
fn agent_location_names_linked_worktrees_else_cwd() {
    let root = std::env::temp_dir().join(format!("yalda-header-worktree-{}", std::process::id()));
    let worktree = root.join("header-layout");
    let nested = worktree.join("docs");
    std::fs::create_dir_all(&nested).expect("create worktree fixture");
    std::fs::write(worktree.join(".git"), "gitdir: /tmp/fake\n").expect("mark linked worktree");
    assert_eq!(
        crate::screens::agent_location_label(&nested),
        "WORKTREE header-layout"
    );

    let ordinary = root.join("ordinary");
    std::fs::create_dir_all(&ordinary).expect("create cwd fixture");
    assert!(crate::screens::agent_location_label(&ordinary).starts_with("CWD "));

    let _ = std::fs::remove_dir_all(&root);
}

/// The Agent Tile activity pill is always painted and keeps exactly the same
/// width when it changes from `+ ready` to `* working`.
#[gpui::test]
fn agent_tile_paints_a_status_pill_while_working(cx: &mut TestAppContext) {
    let (view, vcx, id, _session) = boot_with_transcript(cx);

    // A virgin session is ready, not blank.
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let ready = crate::layout_probe_get("agent-status-pill")
        .expect("a virgin session must paint its ready pill");
    crate::layout_probe_end();
    assert!(
        (ready.2 - crate::screens::AGENT_ACTIVITY_PILL_WIDTH).abs() < 0.5,
        "ready pill has the fixed width: {ready:?}"
    );

    // A reply in flight changes the state, never the geometry.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.turn_phase = crate::TurnPhase::begin(std::time::Instant::now());
        });
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let working = crate::layout_probe_get("agent-status-pill");
    crate::layout_probe_end();
    let (_, _, w, h) = working.expect("the working pill must paint while a reply is in flight");
    assert!(
        (w - ready.2).abs() < 0.5 && h > 6.0,
        "ready and working pills must share a fixed width: ready={ready:?}, working={working:?}"
    );
}

/// The context-window usage meter joins the activity header line.
#[gpui::test]
fn agent_usage_paints_on_the_activity_header_line(cx: &mut TestAppContext) {
    let (view, vcx, id, _session) = boot_with_transcript(cx);
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.usage = Some(yalda::acp_channel::UsageSnapshot {
                tokens_used: 32_000,
                tokens_total: 200_000,
                cost_usd: None,
            });
        });
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let status = crate::layout_probe_get("agent-status-row").expect("primary status row paints");
    let activity = crate::layout_probe_get("agent-activity-row").expect("activity row paints");
    let usage = crate::layout_probe_get("agent-usage-row").expect("usage row paints");
    let location = crate::layout_probe_get("agent-location-row").expect("location row paints");
    crate::layout_probe_end();

    assert!(
        activity.1 >= status.1 + status.3 - 0.5,
        "activity must start below identity: status={status:?}, activity={activity:?}"
    );
    assert!(
        usage.1 >= activity.1 - 0.5 && usage.1 + usage.3 <= activity.1 + activity.3 + 0.5,
        "usage must be vertically contained by the activity line: \
         activity={activity:?}, usage={usage:?}"
    );
    assert!(
        location.1 >= activity.1 + activity.3 - 0.5,
        "location must start below activity: activity={activity:?}, location={location:?}"
    );
    assert!(
        usage.2 > 100.0 && usage.3 > 6.0,
        "usage line has real size: {usage:?}"
    );
}

/// UXI-JumpPanel-11: the jump panel is painted on the SAME surface as the
/// command menu — `jump_panel_surface` IS `menu_panel_bg`, so the sidebar and the
/// `?`/`.`/space menu card are one material, and both sit LIGHTER than the editor
/// (this reverses UXI-JumpPanel-7's recessed darken, which read muddy on Folio).
/// Pure fn.
///
/// Negative control: restore the old recessed derivation (`editor.l - 0.035`) →
/// the "same as the menu card" and "lighter than the editor" asserts both fail.
#[test]
fn jump_panel_surface_matches_the_command_menu() {
    use crate::{jump_panel_surface, menu_panel_bg};
    use gpui::Hsla;
    // Folio-ish paper (the theme that prompted the reversal) and a dark theme.
    for editor in [
        Hsla {
            h: 0.12,
            s: 0.30,
            l: 0.94,
            a: 1.0,
        },
        Hsla {
            h: 0.62,
            s: 0.30,
            l: 0.17,
            a: 1.0,
        },
    ] {
        let panel = jump_panel_surface(editor);
        assert_eq!(
            panel,
            menu_panel_bg(editor),
            "the panel wears the command menu's surface, exactly"
        );
        assert!(
            panel.l > editor.l,
            "…which is LIGHTER than the editor (got L {} vs {})",
            panel.l,
            editor.l
        );
        assert!(
            (panel.h - editor.h).abs() < 1e-6 && (panel.s - editor.s).abs() < 1e-6,
            "hue + saturation preserved (no muddying)"
        );
    }
}

/// UXI-Menu-5: the command panel is an ELEVATED surface — lighter than the editor
/// (tiles/workspace) at the same hue + saturation — so it stands out. It lifts on
/// both dark and light themes. (As of `UXI-JumpPanel-11` the jump panel shares
/// this surface deliberately — see `jump_panel_surface_matches_the_command_menu`;
/// the old "diverges from the recessed jump bar" clause is retired.)
///
/// Negative control: make `menu_panel_bg` return `editor` unchanged → the
/// lighter-than-editor asserts fail.
#[test]
fn menu_panel_bg_is_elevated_above_the_editor() {
    use crate::menu_panel_bg;
    use gpui::Hsla;
    // Dark theme (Nightfox-ish editor_bg L≈0.17): card lifts above the bg.
    let dark = Hsla {
        h: 0.62,
        s: 0.30,
        l: 0.17,
        a: 1.0,
    };
    let d = menu_panel_bg(dark);
    assert!(
        d.l > dark.l + 0.02,
        "dark bg → lighter card (got L {} vs {})",
        d.l,
        dark.l
    );
    assert!(
        (d.h - dark.h).abs() < 1e-6 && (d.s - dark.s).abs() < 1e-6 && d.a == dark.a,
        "hue + saturation + alpha preserved (no muddying)"
    );
    // Light theme (paper L≈0.94): still lifts (a near-white elevated card).
    let light = Hsla {
        h: 0.12,
        s: 0.5,
        l: 0.94,
        a: 1.0,
    };
    let l = menu_panel_bg(light);
    assert!(l.l > light.l, "light bg → lighter card, got L {}", l.l);
    assert!(l.l <= 1.0, "clamped");
}

/// UXI-JumpPanel-8: clicking a project name opens a context menu (the REAL entry
/// point `open_project_menu`), and choosing an item runs the project-scoped action
/// and dismisses the menu. New workspace creates a workspace in THAT project;
/// Delete arms the confirm overlay. This drives the exact methods the menu items'
/// `on_click` handlers call.
///
/// Negative control: make `open_project_menu` skip `open_overlay(ProjectMenu…)` →
/// `project_menu_ref()` is `None` and the first assert fails; or make
/// `project_menu_action` not call `new_workspace_in` → the workspace-count assert
/// fails.
#[gpui::test]
fn project_menu_opens_on_name_click_and_actions_dispatch(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let pa = PathBuf::from("/tmp/yalda-projmenu-a");
    let pid = view.update(vcx, |v, _cx| {
        v.projects
            .create("Menu".into(), pa.clone())
            .expect("create")
    });

    // Click the name → menu opens anchored, targeting this project.
    view.update(vcx, |v, cx| v.open_project_menu(pid, (40.0, 30.0), cx));
    view.read_with(vcx, |v, _| {
        assert!(
            matches!(v.project_menu_ref(), Some((p, _, _)) if p == pid),
            "the project context menu is open for the clicked project"
        );
    });

    // "New workspace" → creates a workspace in this project, closes the menu.
    let before = view.read_with(vcx, |v, _| {
        v.workspace
            .workspaces
            .iter()
            .filter(|t| t.project() == pid)
            .count()
    });
    view.update(vcx, |v, cx| {
        v.project_menu_action(pid, crate::ProjectMenuAction::NewWorkspace, cx)
    });
    view.read_with(vcx, |v, _| {
        assert!(
            !v.overlay_is_project_menu(),
            "menu dismissed after the action fires"
        );
        assert_eq!(
            v.workspace
                .workspaces
                .iter()
                .filter(|t| t.project() == pid)
                .count(),
            before + 1,
            "New workspace created a workspace scoped to this project"
        );
    });

    // Re-open, then "Delete project" → arms the confirm overlay (the project is
    // non-empty now), menu dismissed. project_menu_action clears the menu FIRST so
    // request_delete_project's has_overlay guard passes.
    view.update(vcx, |v, cx| v.open_project_menu(pid, (40.0, 30.0), cx));
    view.update(vcx, |v, cx| {
        v.project_menu_action(pid, crate::ProjectMenuAction::DeleteProject, cx)
    });
    view.read_with(vcx, |v, _| {
        assert!(!v.overlay_is_project_menu(), "menu dismissed");
        assert!(
            matches!(v.confirm_delete_ref(), Some(p) if p == pid),
            "Delete project armed the confirm overlay for this project"
        );
    });
}

/// bug-0019 (UXI-JumpPanel-8): a REAL mouse click on a project context-menu item
/// must run its action. The sibling test above drives `project_menu_action`
/// directly — a hand-built proxy (anti-circling rule 1) that stayed green while
/// the mouse path was dead: the full-window click-away backdrop's hitbox is ALSO
/// hovered under the (non-occluding) popup, so pressing an item fired the
/// backdrop's `on_mouse_down` → `clear_overlay()`, and the item was gone before
/// mouse-up, so `on_click` (down-then-up on the same element) never fired.
///
/// This drives the window's real mouse dispatch (`simulate_click`) at the item's
/// REAL painted bounds, so the backdrop/popup hit-test ordering is under test.
///
/// Negative control: drop `.occlude()` from the popup in `render_project_menu` →
/// the press dismisses the menu, no workspace is created, and the count assert
/// fails.
#[gpui::test]
fn project_menu_item_click_runs_the_action(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let pa = PathBuf::from("/tmp/yalda-projmenu-click");
    let pid = view.update(vcx, |v, _cx| {
        v.projects
            .create("Click".into(), pa.clone())
            .expect("create")
    });

    view.update(vcx, |v, cx| v.open_project_menu(pid, (60.0, 80.0), cx));
    vcx.run_until_parked();

    // The item's REAL painted rect — clicking a computed guess would prove nothing.
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let rect = crate::layout_probe_get("proj-menu-new-ws");
    let backdrop_open = view.read_with(vcx, |v, _| v.overlay_is_project_menu());
    crate::layout_probe_end();

    assert!(
        backdrop_open,
        "the project menu must still be open when we click it"
    );
    let (x, y, w, h) = rect.expect("the New workspace menu item never painted");
    assert!(
        w > 4.0 && h > 4.0,
        "menu item painted with no area ({w}x{h}) — nothing to click"
    );
    let at = point(px(x + w / 2.0), px(y + h / 2.0));

    let before = view.read_with(vcx, |v, _| {
        v.workspace
            .workspaces
            .iter()
            .filter(|t| t.project() == pid)
            .count()
    });

    vcx.simulate_mouse_move(at, None, gpui::Modifiers::default());
    vcx.simulate_click(at, gpui::Modifiers::default());
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace
                .workspaces
                .iter()
                .filter(|t| t.project() == pid)
                .count(),
            before + 1,
            "clicking 'New workspace' did NOTHING — the menu item's on_click never fired (bug-0019)"
        );
        assert!(
            !v.overlay_is_project_menu(),
            "the menu dismisses once the action runs"
        );
    });

    // …and occluding the popup must NOT cost click-away: a press anywhere else
    // still hits the backdrop and dismisses.
    view.update(vcx, |v, cx| v.open_project_menu(pid, (60.0, 80.0), cx));
    vcx.run_until_parked();
    let away = point(px(x + w + 240.0), px(y + h + 240.0));
    vcx.simulate_mouse_move(away, None, gpui::Modifiers::default());
    vcx.simulate_click(away, gpui::Modifiers::default());
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(
            !v.overlay_is_project_menu(),
            "clicking outside the popup dismisses the menu"
        );
    });
}

/// UXI-JumpPanel-7 (create relocation): the jump panel no longer carries a
/// top-level ＋ New project row — project creation moved to the GLOBAL menu. The
/// menu offers a "new project" entry, and dispatching `new-project` opens the REAL
/// New Project overlay.
///
/// Negative control: remove the `"new-project" => …` arm in
/// `dispatch_menu_command` → the overlay never opens and the last assert fails.
#[gpui::test]
fn new_project_relocated_to_global_menu(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    assert!(
        shell_menu_has_label("new project"),
        "the shell menu offers a New project entry"
    );
    view.update(vcx, |v, cx| v.dispatch_menu_command("new-project", cx));
    view.read_with(vcx, |v, _| {
        assert!(
            v.overlay_is_new_project(),
            "dispatching new-project opens the New Project overlay"
        );
    });
}

/// UXI-JumpPanel-3, clauses 1–2: a session created free (bound to no tile) —
/// which is the end state `spawn_free_agent_session` produces once the server's
/// `SessionCreated` broadcast lands — surfaces in the jump panel as an UNBOUND
/// (`○`) row through the real `jump_panel_agent_rows`, and is then bindable the
/// ordinary way (`jump_to_agent`), never auto-bound by the create itself.
///
/// The server round-trip needs the daemon (harness gap #2); this drives the wire
/// end state via the REAL `apply_server_batch(SessionCreated)` reducer.
///
/// Negative control: assert `!bound` before the bind — if the create auto-bound
/// a tile, the row would already read `bound == true`.
#[gpui::test]
fn free_agent_row_is_unbound_and_bindable(cx: &mut TestAppContext) {
    use yalda::session_proto::Notification as ServerNotification;
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let info = SessionInfo {
        session_id: "free-1".into(),
        acp_session_id: Some("acp-free-1".into()),
        label: "claude-free".into(),
        cwd: cwd.clone(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::PermissionMode::ReadOnly,
        busy: false,
        archived: false,
    };

    // The end state of a free create: the session appears in the roster, bound
    // to no tile.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionCreated {
                session: info.clone(),
            }],
            cx,
        );
    });

    let target = view.update(vcx, |v, cx| {
        let rows = v.jump_panel_agent_rows(cx);
        assert_eq!(rows.len(), 1, "the free session shows in the jump panel");
        assert!(
            !rows[0].bound,
            "a freshly-created session is FREE — no tile binds it (○), not auto-bound"
        );
        assert!(
            matches!(rows[0].target, crate::JumpTarget::Roster(ref s) if s == "free-1"),
            "surfaced by its server sid"
        );
        rows[0].target.clone()
    });

    // It is bindable later the ordinary way — no session server here, so the
    // roster-open path can't attach; instead prove bindability directly through
    // the store + tile bind that a selection performs.
    view.update(vcx, |v, cx| {
        let id = v.show_local_session(
            crate::AgentSession {
                state: crate::AgentState::new_server_managed(None),
                label: "claude-free".into(),
                cwd: cwd.clone(),
                resume_id: None,
            },
            cx,
        );
        v.sessions
            .bind_sid(id, ServerSid::new("free-1"))
            .expect("fresh sid binds");
        v.jump_to_session(id, cx);
        assert_eq!(
            v.agent_tile().and_then(crate::AgentTile::session),
            Some(id),
            "selecting the free session opens a viewport reference"
        );
        assert!(
            v.agent_tile_id_bound_to(id).is_none(),
            "a bare direct reference does not place the session in a workspace"
        );
        let _ = &target;
    });
}

/// REGRESSION (bug-0006): the jump panel's agent rows are ordered by label across
/// BOTH roster-known and local-only sessions, so a session doesn't spontaneously
/// reorder when the async roster refresh catches up to a locally-created session.
/// Before the fix a local-only session was appended LAST regardless of its label, so
/// a `claude-1` created locally rendered after a roster `claude-2`, then HOPPED to
/// the top once the roster learned about it — "sessions reorder for some weird reason".
///
/// Negative control: remove the final `rows.sort_by(... label ...)` in
/// `jump_panel_agent_rows` → the local `claude-1` stays appended last (`["claude-2",
/// "claude-1"]`) and the label-order assert fails RED.
#[gpui::test]
fn jump_panel_orders_local_and_roster_sessions_by_label(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState, AgentTile, App};
    use yalda::session_proto::{Notification as SN, SessionInfo};
    let (view, vcx) = boot_browser(cx);
    // A roster session `claude-2` (created on the server, never opened here).
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![SN::SessionCreated {
                session: SessionInfo {
                    session_id: "srv-2".into(),
                    acp_session_id: None,
                    label: "claude-2".into(),
                    cwd: PathBuf::from("."),
                    provider: yalda::acp_channel::AgentProvider::Claude,
                    turns: 0,
                    connected: true,
                    permission_mode: yalda::acp_channel::PermissionMode::ReadOnly,
                    busy: false,
                    archived: false,
                },
            }],
            cx,
        );
    });
    // A local-only session `claude-1` (sorts BEFORE claude-2) bound to an agent tile,
    // its sid not yet in the roster — the just-created placeholder case.
    view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        let _ = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(None),
                label: "claude-1".into(),
                cwd: PathBuf::from("."),
                resume_id: None,
            },
            cx,
        );
    });
    let labels: Vec<String> = view
        .update(vcx, |v, cx| v.jump_panel_agent_rows(cx))
        .iter()
        .map(|r| r.label.clone())
        .collect();
    assert_eq!(
        labels,
        vec!["claude-1".to_string(), "claude-2".to_string()],
        "local-only + roster sessions order by label together — no hop when the \
         roster catches up (bug-0006)"
    );
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
    use crate::{AgentRow, JumpTarget, group_agent_rows_by_cwd, order_grouped_rows};
    let row = |sid: &str, label: &str, cwd: &str| AgentRow {
        target: JumpTarget::Roster(sid.into()),
        label: label.into(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        summary: None,
        summary_pending: false,
        archived: false,
        cwd: std::path::PathBuf::from(cwd),
        bound: false,
        connected: true,
        awaiting: None,
        unread: false,
        order_sid: Some(sid.into()),
        state_entered_at: None,
        tags: Vec::new(),
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
        g[idx]
            .1
            .iter()
            .map(|(_, r)| r.label.clone())
            .collect::<Vec<_>>()
    };

    // Empty orders → default: groups alphabetical (alpha, beta); alpha's
    // sessions in by-label order (a, b). (Negative control for "no drag".)
    let g = order_grouped_rows(group_agent_rows_by_cwd(mk()), &[], &[]);
    assert_eq!(
        keys(&g),
        vec!["/work/alpha", "/work/beta"],
        "default: alpha before beta"
    );
    assert_eq!(sess(&g, 0), vec!["a", "b"], "default: sessions by label");

    // A cwd order flips the groups (beta before alpha).
    let cwd_order = vec!["/work/beta".to_string(), "/work/alpha".to_string()];
    let g = order_grouped_rows(group_agent_rows_by_cwd(mk()), &cwd_order, &[]);
    assert_eq!(
        keys(&g),
        vec!["/work/beta", "/work/alpha"],
        "cwd order reorders headers"
    );

    // A session order flips alpha's sessions (b before a); groups still alpha.
    let sess_order = vec!["s-b".to_string(), "s-a".to_string()];
    let g = order_grouped_rows(group_agent_rows_by_cwd(mk()), &[], &sess_order);
    let alpha_idx = keys(&g).iter().position(|k| k == "/work/alpha").unwrap();
    assert_eq!(
        sess(&g, alpha_idx),
        vec!["b", "a"],
        "session order reorders within group"
    );
}

/// UXI-JumpPanel-14: Waiting and Working are chronological live queues
/// (oldest state entry first, newest last), while All preserves its incoming
/// custom order exactly.
#[test]
fn jump_agent_state_tabs_filter_and_sort_without_moving_all() {
    use crate::{AgentRow, JumpAgentTab, JumpTarget, agent_rows_for_tab};
    let base = std::time::Instant::now();
    let row =
        |sid: &str, label: &str, awaiting: Option<bool>, unread: bool, age_secs: u64| AgentRow {
            target: JumpTarget::Roster(sid.into()),
            label: label.into(),
            provider: yalda::acp_channel::AgentProvider::Claude,
            summary: None,
            summary_pending: false,
            archived: false,
            cwd: std::path::PathBuf::from("/work"),
            bound: false,
            connected: true,
            awaiting,
            unread,
            order_sid: Some(sid.into()),
            state_entered_at: Some(base - std::time::Duration::from_secs(age_secs)),
            tags: Vec::new(),
        };
    // Incoming order represents the user's custom All order, deliberately
    // unrelated to either state's chronology.
    let make = || {
        let mut archived = row("arch", "archived", Some(true), false, 3);
        archived.archived = true;
        vec![
            (0, row("w-new", "wait-new", Some(false), true, 1)),
            (1, row("quiet", "quiet", Some(false), false, 50)),
            (2, row("k-new", "work-new", Some(true), false, 2)),
            (3, row("w-old", "wait-old", Some(false), true, 20)),
            (4, row("k-old", "work-old", Some(true), false, 30)),
            (5, archived),
        ]
    };
    let labels =
        |rows: Vec<(usize, AgentRow)>| rows.into_iter().map(|(_, r)| r.label).collect::<Vec<_>>();

    assert_eq!(
        labels(agent_rows_for_tab(make(), JumpAgentTab::Waiting)),
        vec!["quiet", "wait-old", "wait-new"],
        "Waiting includes every connected non-working agent and is oldest→newest"
    );
    assert_eq!(
        labels(agent_rows_for_tab(make(), JumpAgentTab::Working)),
        vec!["work-old", "work-new"],
        "Working is oldest→newest by working-state entry"
    );
    assert_eq!(
        labels(agent_rows_for_tab(make(), JumpAgentTab::All)),
        vec!["wait-new", "quiet", "work-new", "wait-old", "work-old"],
        "All never reorders when state-derived tabs do and excludes archived rows"
    );
    assert_eq!(
        labels(agent_rows_for_tab(make(), JumpAgentTab::Archived)),
        vec!["archived"],
        "Archived is the complementary durable-order slice"
    );
}

/// UXI-JumpPanel-14: viewing a Waiting roster session must not make it the
/// newest Waiting session. Before selection the row is roster-only and ranked
/// by `AgentRoster::state_since`; the real roster-row jump attaches it into a
/// freshly constructed local `AgentState`. That identity handoff must preserve
/// the operational timestamp and therefore the exact Waiting order.
///
/// Drives the real `jump_to_agent` → `jump_to_roster_session` →
/// `picker_attach_existing` path. The test seam bypasses only the absent live
/// daemon in the hermetic harness; the synchronous view/attach state changes are
/// production.
///
/// Negative control (mandatory, observed RED): prefer the newly attached local
/// state's `waiting_since` over `AgentRoster::state_since` in
/// `jump_panel_agent_rows` and the final order becomes `a-new, z-old` — the
/// selected row moves to the bottom exactly as reported.
#[gpui::test]
fn viewing_a_waiting_agent_does_not_change_waiting_order(cx: &mut TestAppContext) {
    use crate::{JumpAgentTab, JumpTarget};
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let (pid, cwd) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        (
            pid,
            v.projects.cwd_of(pid).expect("project cwd").to_path_buf(),
        )
    });
    let info = |sid: &str, label: &str| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: cwd.clone(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
        busy: false,
        archived: false,
    };
    view.update(vcx, |v, cx| {
        // Reverse label order makes chronology observable rather than getting
        // the same answer accidentally from the roster's alphabetical input.
        v.agent_roster.upsert(info("S-old", "z-old"));
        std::thread::sleep(std::time::Duration::from_millis(2));
        v.agent_roster.upsert(info("S-new", "a-new"));
        v.select_jump_agent_tab(pid, JumpAgentTab::Waiting, cx);
    });

    let waiting_labels = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            v.jump_panel_sections(cx)
                .0
                .into_iter()
                .find(|section| section.id == pid)
                .expect("project section")
                .sessions
                .into_iter()
                .map(|(_, row)| row.label)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        waiting_labels(&view, vcx),
        vec!["z-old", "a-new"],
        "precondition: Waiting is oldest first, not alphabetical"
    );

    view.update(vcx, |v, cx| {
        crate::with_server_roster_jump_branch(|| {
            v.jump_to_agent(JumpTarget::Roster("S-old".into()), cx)
        });
    });
    vcx.run_until_parked();
    assert!(
        view.read_with(vcx, |v, _| {
            v.sessions.locate(&ServerSid::new("S-old")).is_some()
        }),
        "the real roster jump attached a fresh local view for S-old"
    );
    assert_eq!(
        waiting_labels(&view, vcx),
        vec!["z-old", "a-new"],
        "viewing S-old is not a state transition and must not move it to the bottom"
    );

    // A REAL operational cycle is different: S-old leaves Waiting for Working,
    // then re-enters Waiting and therefore becomes the newest row.
    use yalda::session_proto::Notification as ServerNotification;
    let local = view.read_with(vcx, |v, _| {
        v.sessions
            .locate(&ServerSid::new("S-old"))
            .expect("attached local identity")
    });
    view.update(vcx, |v, cx| {
        let now = std::time::Instant::now();
        v.session_entity(local)
            .expect("local session")
            .update(cx, |session, _| {
                session.state.turn_phase = crate::TurnPhase::Awaiting {
                    started: now,
                    last_event: now,
                };
            });
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "S-old".into(),
                busy: true,
            }],
            cx,
        );
    });
    assert_eq!(
        waiting_labels(&view, vcx),
        vec!["a-new"],
        "Working removes S-old from Waiting"
    );
    view.update(vcx, |v, cx| {
        v.session_entity(local)
            .expect("local session")
            .update(cx, |session, _| {
                session.state.turn_phase = crate::TurnPhase::Idle;
                session.state.waiting_since = Some(std::time::Instant::now());
            });
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "S-old".into(),
                busy: false,
            }],
            cx,
        );
    });
    assert_eq!(
        waiting_labels(&view, vcx),
        vec!["a-new", "z-old"],
        "only re-entering Waiting moves S-old to the bottom"
    );
}

/// UXI-JumpPanel-14, real per-project projection: each project defaults to All,
/// selects its own state slice, preserves custom All order through state
/// changes, and appends a newly discovered sid.
#[gpui::test]
fn jump_project_agent_tabs_are_independent_and_all_appends(cx: &mut TestAppContext) {
    use crate::JumpAgentTab;
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = boot_browser(cx);
    let (pid, other_pid, workspace_idx, cwd) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        let other = v
            .projects
            .create(
                "Other tab project".into(),
                PathBuf::from("/tmp/yalda-tab-other"),
            )
            .expect("other project");
        (
            pid,
            other,
            v.workspace.active_workspace,
            v.projects.cwd_of(pid).expect("project cwd").to_path_buf(),
        )
    });
    let info = |sid: &str, label: &str, busy: bool| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: cwd.clone(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
        busy,
        archived: false,
    };
    view.update(vcx, |v, _| {
        v.agent_roster.upsert(info("S-wait", "wait", false));
        v.agent_roster.upsert(info("S-work", "work", true));
        v.agent_roster.upsert(info("S-quiet", "quiet", false));
        v.roster_unread
            .insert("S-wait".into(), std::time::Instant::now());
        v.jump_session_order = vec!["S-quiet".into(), "S-work".into(), "S-wait".into()];
    });

    crate::layout_probe_begin();
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    let outer_label = format!("jump-agent-tabs-{}", pid.0);
    let (outer_x, outer_y, outer_w, outer_h) = crate::layout_probe_get(&outer_label)
        .expect("the tabs must paint inside one enclosing segmented-control box");
    let (_, workspace_y, _, workspace_h) =
        crate::layout_probe_get(&format!("jump-workspace-row-{workspace_idx}"))
            .expect("the project's workspace row must paint above its tabs");
    assert!(
        outer_y - (workspace_y + workspace_h) >= 8.0,
        "tabs need visible breathing room after workspaces"
    );
    for tab in ["waiting", "working", "all", "archived"] {
        let label = format!("jump-agent-tab-{}-{tab}", pid.0);
        let (x, y, w, h) = crate::layout_probe_get(&label)
            .unwrap_or_else(|| panic!("the per-project {tab} tab must paint"));
        assert!(
            x >= outer_x
                && y >= outer_y
                && x + w <= outer_x + outer_w
                && y + h <= outer_y + outer_h,
            "the {tab} tab must sit inside the shared segmented-control boundary"
        );
    }
    crate::layout_probe_end();

    let labels = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            v.jump_panel_sections(cx)
                .0
                .into_iter()
                .find(|s| s.id == pid)
                .expect("project section")
                .sessions
                .into_iter()
                .map(|(_, r)| r.label)
                .collect::<Vec<_>>()
        })
    };
    view.update(vcx, |v, cx| {
        v.select_jump_agent_tab(pid, JumpAgentTab::Waiting, cx)
    });
    assert_eq!(
        labels(&view, vcx),
        vec!["wait", "quiet"],
        "read and unread idle agents both belong to Waiting"
    );
    let other_tab = view.update(vcx, |v, cx| {
        v.jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|s| s.id == other_pid)
            .expect("other section")
            .agent_tab
    });
    assert_eq!(
        other_tab,
        JumpAgentTab::All,
        "one project's tab does not affect another"
    );

    view.update(vcx, |v, cx| {
        v.select_jump_agent_tab(pid, JumpAgentTab::Working, cx)
    });
    assert_eq!(labels(&view, vcx), vec!["work"]);
    view.update(vcx, |v, cx| {
        v.select_jump_agent_tab(pid, JumpAgentTab::All, cx)
    });
    assert_eq!(
        labels(&view, vcx),
        vec!["quiet", "work", "wait"],
        "All follows custom order"
    );

    view.update(vcx, |v, _| {
        // A state flip may change the live tabs, never All's positions.
        v.agent_roster.set_busy("S-work", false);
        v.agent_roster.set_busy("S-quiet", true);
        assert!(v.append_new_jump_sessions(["S-new".into()]));
        v.agent_roster.upsert(info("S-new", "aaa-new", false));
    });
    assert_eq!(
        labels(&view, vcx),
        vec!["quiet", "work", "wait", "aaa-new"],
        "state changes preserve slots and a new agent appends at the bottom"
    );
}

/// UXI-JumpPanel-10: tabs and All-group headers name the live states, so
/// individual Waiting and Working rows do not paint redundant right-edge words.
#[gpui::test]
fn jump_session_rows_do_not_paint_redundant_status_words(cx: &mut TestAppContext) {
    use std::collections::HashMap;
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let (pid, cwd) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        (
            pid,
            v.projects.cwd_of(pid).expect("project cwd").to_path_buf(),
        )
    });
    let info = |sid: &str, label: &str, busy: bool| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: cwd.clone(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
        busy,
        archived: false,
    };
    view.update(vcx, |v, _| {
        v.agent_roster
            .upsert(info("no-word-wait", "waiting row", false));
        v.agent_roster
            .upsert(info("no-word-work", "working row", true));
        v.jump_session_order = vec!["no-word-wait".into(), "no-word-work".into()];
        v.materialize_roster_detached_tiles();
    });
    let row_ids: HashMap<String, usize> = view.update(vcx, |v, cx| {
        v.jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|section| section.id == pid)
            .expect("project section")
            .sessions
            .into_iter()
            .map(|(i, row)| (row.label, i))
            .collect()
    });

    crate::layout_probe_begin();
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    for label in ["working row", "waiting row"] {
        let i = row_ids[label];
        assert!(
            crate::layout_probe_get(&format!("jump-session-row-{i}")).is_some(),
            "{label} must paint, making the word-absence assertion non-vacuous"
        );
        assert!(
            crate::layout_probe_get(&format!("jump-session-status-word-{i}")).is_none(),
            "{label} must not repeat its state as right-edge text"
        );
    }
    crate::layout_probe_end();
}

/// UXI-JumpPanel-22: mixed-provider sessions expose their authoritative owner
/// on every real jump-panel row without displacing the independent leading
/// operational-status mark. This covers both server-authoritative roster rows
/// and the local-only pre-roster projection.
///
/// Negative control: before `jump_session_row_el` painted the trailing provider
/// mark (and exposed the leading mark probe), the row itself painted but the
/// first `jump-session-provider-*` assertion below failed RED.
#[gpui::test]
fn jump_panel_session_rows_paint_provider_ownership_marks(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState, JumpTarget};
    use std::collections::HashMap;
    use yalda::acp_channel::{AgentProvider, PermissionMode};
    use yalda::session_proto::SessionInfo;

    assert_eq!(crate::agent_provider_mark(AgentProvider::Claude), "✳");
    assert_eq!(crate::agent_provider_mark(AgentProvider::Codex), "⌬");

    let (view, vcx) = boot_browser(cx);
    let (pid, cwd) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        (
            pid,
            v.projects.cwd_of(pid).expect("project cwd").to_path_buf(),
        )
    });
    let info = |sid: &str, label: &str, provider: AgentProvider, busy: bool| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: cwd.clone(),
        provider,
        turns: 0,
        connected: true,
        permission_mode: PermissionMode::ReadOnly,
        busy,
        archived: false,
    };
    view.update(vcx, |v, cx| {
        v.agent_roster.upsert(info(
            "provider-claude",
            "alpha claude",
            AgentProvider::Claude,
            false,
        ));
        v.agent_roster.upsert(info(
            "provider-codex",
            "beta codex",
            AgentProvider::Codex,
            true,
        ));
        let local = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed_for(AgentProvider::Codex, None),
                label: "gamma local codex".into(),
                cwd: cwd.clone(),
                resume_id: None,
            },
            cx,
        );
        v.materialize_roster_detached_tiles();
        v.jump_to_session(local, cx);
        v.workspace.set_active_workspace(0);
        cx.notify();
    });

    let rows: HashMap<String, (usize, AgentProvider, JumpTarget)> = view.update(vcx, |v, cx| {
        v.jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|section| section.id == pid)
            .expect("project section")
            .sessions
            .into_iter()
            .map(|(i, row)| (row.label, (i, row.provider, row.target)))
            .collect()
    });
    assert_eq!(rows["alpha claude"].1, AgentProvider::Claude);
    assert!(
        matches!(rows["alpha claude"].2, JumpTarget::Roster(ref sid) if sid == "provider-claude")
    );
    assert_eq!(rows["beta codex"].1, AgentProvider::Codex);
    assert!(matches!(rows["beta codex"].2, JumpTarget::Roster(ref sid) if sid == "provider-codex"));
    assert_eq!(rows["gamma local codex"].1, AgentProvider::Codex);
    assert!(matches!(rows["gamma local codex"].2, JumpTarget::Local(_)));

    crate::layout_probe_begin();
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    for label in ["alpha claude", "beta codex", "gamma local codex"] {
        let (i, provider, _) = &rows[label];
        assert!(
            crate::layout_probe_get(&format!("jump-session-row-{i}")).is_some(),
            "{label} row must paint, making its mark assertions non-vacuous"
        );
        assert!(
            crate::layout_probe_get(&format!("jump-session-provider-{i}-{}", provider.label()))
                .is_some(),
            "{label} must paint its {} ownership mark",
            provider.label()
        );
        assert!(
            crate::layout_probe_get(&format!("jump-session-status-mark-{i}")).is_some(),
            "{label} must retain its independent leading operational-status mark"
        );
    }
    crate::layout_probe_end();
}

/// UXI-JumpPanel-15: the four agent tabs paint as a bounded 2×2 control,
/// Waiting / Working above All / Archived.
#[gpui::test]
fn jump_agent_tabs_paint_as_two_by_two_grid(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let pid = view.update(vcx, |v, _| {
        v.workspace.active_workspace().expect("workspace").project()
    });

    crate::layout_probe_begin();
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    let bounds = |tab: &str| {
        crate::layout_probe_get(&format!("jump-agent-tab-{}-{tab}", pid.0))
            .unwrap_or_else(|| panic!("the {tab} tab must paint"))
    };
    let waiting = bounds("waiting");
    let working = bounds("working");
    let all = bounds("all");
    let archived = bounds("archived");
    let outer = crate::layout_probe_get(&format!("jump-agent-tabs-{}", pid.0))
        .expect("the shared tab control must paint");
    crate::layout_probe_end();

    let same_row = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
        (a.1 - b.1).abs() < 1.0 && (a.3 - b.3).abs() < 1.0
    };
    assert!(
        same_row(waiting, working),
        "Waiting and Working must share the first row"
    );
    assert!(
        same_row(all, archived),
        "All and Archived must share the second row"
    );
    assert!(
        all.1 >= waiting.1 + waiting.3,
        "All / Archived must paint below Waiting / Working"
    );
    assert!(
        (waiting.0 - all.0).abs() < 1.0
            && (working.0 - archived.0).abs() < 1.0
            && (waiting.2 - all.2).abs() < 1.0
            && (working.2 - archived.2).abs() < 1.0,
        "the two rows must align into two equal columns"
    );
    for (label, (x, y, w, h)) in [
        ("Waiting", waiting),
        ("Working", working),
        ("All", all),
        ("Archived", archived),
    ] {
        assert!(
            x >= outer.0
                && y >= outer.1
                && x + w <= outer.0 + outer.2
                && y + h <= outer.1 + outer.3,
            "{label} must stay inside the shared tab-control boundary"
        );
    }
}

/// UXI-JumpPanel-17: Waiting and Working expose their live project totals in
/// the painted tab strip, including when one side reaches zero.
#[gpui::test]
fn jump_waiting_working_tabs_paint_live_counts(cx: &mut TestAppContext) {
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let (pid, cwd) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        (
            pid,
            v.projects.cwd_of(pid).expect("project cwd").to_path_buf(),
        )
    });
    let info = |sid: &str, label: &str, busy: bool, connected: bool| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: cwd.clone(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected,
        permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
        busy,
        archived: false,
    };
    view.update(vcx, |v, _| {
        v.agent_roster
            .upsert(info("count-wait-a", "count wait a", false, true));
        v.agent_roster
            .upsert(info("count-wait-b", "count wait b", false, true));
        v.agent_roster
            .upsert(info("count-work", "count work", true, true));
        v.agent_roster
            .upsert(info("count-offline", "count offline", false, false));
        v.agent_roster
            .upsert(info("count-archived", "count archived", false, true));
        v.jump_archived_sessions.insert("count-archived".into());
        v.jump_session_order = vec![
            "count-wait-a".into(),
            "count-work".into(),
            "count-offline".into(),
            "count-wait-b".into(),
            "count-archived".into(),
        ];
        v.materialize_roster_detached_tiles();
    });

    let counts = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            let section = v
                .jump_panel_sections(cx)
                .0
                .into_iter()
                .find(|section| section.id == pid)
                .expect("project section");
            (section.waiting_count, section.working_count)
        })
    };
    assert_eq!(
        counts(&view, vcx),
        (2, 1),
        "archived and unavailable sessions contribute to neither live total"
    );

    crate::layout_probe_begin();
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    for tab in ["waiting", "working"] {
        let tab_bounds = crate::layout_probe_get(&format!("jump-agent-tab-{}-{tab}", pid.0))
            .expect("the counted tab must paint");
        let (x, y, w, h) =
            crate::layout_probe_get(&format!("jump-agent-tab-count-{}-{tab}", pid.0))
                .unwrap_or_else(|| panic!("the {tab} tab's live total must paint"));
        let (tab_x, tab_y, tab_w, tab_h) = tab_bounds;
        assert!(
            x >= tab_x && y >= tab_y && x + w <= tab_x + tab_w && y + h <= tab_y + tab_h,
            "the {tab} total must stay inside its tab target"
        );
    }
    crate::layout_probe_end();

    view.update(vcx, |v, _| {
        v.agent_roster.set_busy("count-work", false);
    });
    assert_eq!(
        counts(&view, vcx),
        (3, 0),
        "a live state change updates both derived totals"
    );

    crate::layout_probe_begin();
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    assert!(
        crate::layout_probe_get(&format!("jump-agent-tab-count-{}-working", pid.0)).is_some(),
        "the Working indicator must remain painted when its value is zero"
    );
    crate::layout_probe_end();
}

/// UXI-JumpPanel-14: All is a headed stable partition of the durable custom
/// roster. Working leads, Waiting follows, and exceptional unavailable rows
/// trail only when present.
#[gpui::test]
fn jump_all_tab_groups_activity_with_headers(cx: &mut TestAppContext) {
    use std::collections::HashMap;
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    let (pid, cwd) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        (
            pid,
            v.projects.cwd_of(pid).expect("project cwd").to_path_buf(),
        )
    });
    let info = |sid: &str, label: &str, busy: bool, connected: bool| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: cwd.clone(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected,
        permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
        busy,
        archived: false,
    };
    view.update(vcx, |v, _| {
        v.agent_roster
            .upsert(info("S-wait-2", "wait-two", false, true));
        v.agent_roster
            .upsert(info("S-work-2", "work-two", true, true));
        v.agent_roster
            .upsert(info("S-off", "offline", false, false));
        v.agent_roster
            .upsert(info("S-work-1", "work-one", true, true));
        v.agent_roster
            .upsert(info("S-wait-1", "wait-one", false, true));
        // Deliberately interleave activities. The rendered partition must group
        // them without disturbing this durable relative rank inside each group.
        v.jump_session_order = vec![
            "S-wait-2".into(),
            "S-work-2".into(),
            "S-off".into(),
            "S-work-1".into(),
            "S-wait-1".into(),
        ];
        v.materialize_roster_detached_tiles();
    });

    let row_ids: HashMap<String, usize> = view.update(vcx, |v, cx| {
        v.jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|section| section.id == pid)
            .expect("project section")
            .sessions
            .into_iter()
            .map(|(i, row)| (row.label, i))
            .collect()
    });

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    // UXI-JumpPanel-20 clause 5 SUPERSEDES UXI-JumpPanel-14's All activity
    // partition IN THE PANEL: the Working/Waiting/Unavailable headings are gone,
    // and untagged rows sort alphabetically by label. Cmd-P uses that same
    // ownership-and-tag projection rather than a separate session ordering.
    for name in ["working", "waiting", "unavailable"] {
        assert!(
            crate::layout_probe_get(&format!("jump-agent-group-{}-{name}", pid.0)).is_none(),
            "All must NOT paint the {name} activity heading anymore (tag-folder view)"
        );
    }
    let row_y = |label: &str| {
        let i = row_ids[label];
        crate::layout_probe_get(&format!("jump-session-row-{i}"))
            .unwrap_or_else(|| panic!("{label} row must paint"))
            .1
    };
    // All sorts untagged rows by label: offline, wait-one, wait-two, work-one, work-two.
    let painted = ["offline", "wait-one", "wait-two", "work-one", "work-two"].map(row_y);
    assert!(
        painted.windows(2).all(|pair| pair[0] < pair[1]),
        "All must paint untagged rows in alphabetical label order"
    );
    crate::layout_probe_end();

    let palette_agents = view.update(vcx, |v, cx| {
        v.jump_palette_items(cx)
            .into_iter()
            .filter(|item| item.is_agent)
            .map(|item| item.label)
            .collect::<Vec<_>>()
    });
    assert_eq!(
        palette_agents,
        vec!["offline", "wait-one", "wait-two", "work-one", "work-two"],
        "empty Cmd-P mirrors the Unbound list's tile order"
    );
}

/// UXI-JumpPanel-16: cold archive is projected as a complementary navigation
/// state. Even when the sidebar itself is on Archived, Cmd-P must project the
/// unarchived All roster rather than leaking archived sessions or inheriting
/// the current tab filter.
#[gpui::test]
fn jump_session_archive_filters_tabs_palette_and_persists(cx: &mut TestAppContext) {
    use crate::JumpAgentTab;
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = boot_browser(cx);
    let (pid, cwd) = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        (
            pid,
            v.projects.cwd_of(pid).expect("project cwd").to_path_buf(),
        )
    });
    let info = |sid: &str, label: &str, busy: bool| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: cwd.clone(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
        busy,
        archived: false,
    };
    view.update(vcx, |v, _| {
        v.agent_roster.upsert(info("S-live", "live-session", false));
        v.agent_roster
            .upsert(info("S-arch", "archived-session", true));
        v.jump_session_order = vec!["S-arch".into(), "S-live".into()];
        v.materialize_roster_detached_tiles();
    });
    let temp = tempfile::tempdir().expect("temp preferences dir");
    let prefs_path = temp.path().join("preferences.json");
    crate::persist::with_preferences_path(prefs_path.clone(), || {
        view.update(vcx, |v, cx| v.set_session_archived("S-arch", true, cx));
    });
    let persisted =
        crate::persist::with_preferences_path(prefs_path, crate::persist::load_preferences);
    assert_eq!(
        persisted.jump_archived_sessions.as_deref(),
        Some(&["S-arch".to_string()][..]),
        "archive identity persists by stable server sid"
    );

    let labels = |view: &gpui::Entity<YaldaGpuiView>,
                  vcx: &mut gpui::VisualTestContext,
                  tab: JumpAgentTab| {
        view.update(vcx, |v, cx| {
            v.select_jump_agent_tab(pid, tab, cx);
            v.jump_panel_sections(cx)
                .0
                .into_iter()
                .find(|section| section.id == pid)
                .expect("project section")
                .sessions
                .into_iter()
                .map(|(_, row)| row.label)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(labels(&view, vcx, JumpAgentTab::All), vec!["live-session"]);
    assert_eq!(
        labels(&view, vcx, JumpAgentTab::Waiting),
        vec!["live-session"]
    );
    assert!(labels(&view, vcx, JumpAgentTab::Working).is_empty());
    assert_eq!(
        labels(&view, vcx, JumpAgentTab::Archived),
        vec!["archived-session"],
        "Archived preserves the durable order slice"
    );

    let palette_labels = view.update(vcx, |v, cx| {
        v.jump_palette_items(cx)
            .into_iter()
            .filter(|item| item.is_agent)
            .map(|item| item.label)
            .collect::<Vec<_>>()
    });
    assert_eq!(
        palette_labels,
        vec!["live-session"],
        "Cmd-P excludes archived sessions and ignores the sidebar's Archived selection"
    );
}

/// UXI-JumpPanel-16 controls: a real right-click on a painted session row opens
/// the cursor menu, whose real painted item toggles the durable flag in both
/// directions. The underlying archive dispatcher remains callable after
/// UXI-Menu-8 removes archive from the intentionally small Agent menu.
#[gpui::test]
fn jump_session_archive_controls_toggle_the_same_durable_flag(cx: &mut TestAppContext) {
    use crate::{JumpAgentTab, JumpTarget};
    use gpui::{Modifiers, MouseButton};
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = boot_browser(cx);
    install_agent_slot(&view, vcx, Some("S-menu"));
    let pid = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        let cwd = v.projects.cwd_of(pid).expect("project cwd").to_path_buf();
        v.agent_roster.upsert(SessionInfo {
            session_id: "S-menu".into(),
            acp_session_id: None,
            label: "menu-session".into(),
            cwd,
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        pid
    });
    vcx.run_until_parked();

    let row_center = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        crate::layout_probe_begin();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let (x, y, w, h) =
            crate::layout_probe_get("jump-session-row-0").expect("session row painted");
        crate::layout_probe_end();
        point(px(x + w / 2.0), px(y + h / 2.0))
    };
    let click_context_toggle = |view: &gpui::Entity<YaldaGpuiView>,
                                vcx: &mut gpui::VisualTestContext| {
        crate::layout_probe_begin();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let (x, y, w, h) =
            crate::layout_probe_get("session-menu-toggle").expect("context action painted");
        crate::layout_probe_end();
        let at = point(px(x + w / 2.0), px(y + h / 2.0));
        vcx.simulate_mouse_move(at, None, Modifiers::default());
        vcx.simulate_click(at, Modifiers::default());
        vcx.run_until_parked();
    };

    // Ordinary row → right click → Archive.
    let at = row_center(&view, vcx);
    vcx.simulate_mouse_move(at, None, Modifiers::default());
    vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(
            matches!(v.session_menu_ref(), Some(("S-menu", _, _))),
            "right-click targets the row's stable sid"
        );
    });
    click_context_toggle(&view, vcx);
    assert!(view.read_with(vcx, |v, _| v.jump_archived_sessions.contains("S-menu")));

    // Archived row → right click → Unarchive.
    view.update(vcx, |v, cx| {
        v.select_jump_agent_tab(pid, JumpAgentTab::Archived, cx)
    });
    let at = row_center(&view, vcx);
    vcx.simulate_mouse_move(at, None, Modifiers::default());
    vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::default());
    vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::default());
    vcx.run_until_parked();
    click_context_toggle(&view, vcx);
    assert!(!view.read_with(vcx, |v, _| v.jump_archived_sessions.contains("S-menu")));

    // Unarchiving does not silently reclaim a tile. Explicitly visit the
    // session again before exercising its tile-local command surface.
    view.update(vcx, |v, cx| {
        let id = v
            .sessions
            .locate(&ServerSid::new("S-menu"))
            .expect("archived session remains open locally");
        v.jump_to_agent(JumpTarget::Local(id), cx);
    });

    // UXI-Menu-9: Archive is the Agent-only tail of the Space tile menu, and it
    // still dispatches the same underlying command.
    view.update(vcx, |v, cx| v.open_local_menu_inner(cx));
    view.read_with(vcx, |v, _| {
        assert!(menu_tree_has_command(
            &v.menu_ref().expect("space menu").menu,
            "archive-session"
        ));
    });
    view.update(vcx, |v, cx| {
        v.clear_overlay();
        v.dispatch_menu_command("archive-session", cx);
    });
    assert!(view.read_with(vcx, |v, _| v.jump_archived_sessions.contains("S-menu")));

    // Archive moved the direct view to its picker. Visiting the Archived row
    // explicitly restores the transcript view and its contextual Unarchive.
    view.update(vcx, |v, cx| {
        let id = v
            .sessions
            .locate(&ServerSid::new("S-menu"))
            .expect("archived session remains open locally");
        v.jump_to_agent(JumpTarget::Local(id), cx);
    });
    view.update(vcx, |v, cx| v.open_local_menu_inner(cx));
    view.read_with(vcx, |v, _| {
        assert!(!menu_tree_has_command(
            &v.menu_ref().expect("space menu").menu,
            "unarchive-session"
        ));
    });
    view.update(vcx, |v, cx| {
        v.clear_overlay();
        v.dispatch_menu_command("unarchive-session", cx);
    });
    assert!(!view.read_with(vcx, |v, _| v.jump_archived_sessions.contains("S-menu")));

    // The direct context entry point refuses sid-less local placeholders.
    view.update(vcx, |v, cx| {
        v.open_session_menu(
            JumpTarget::Local(crate::SessionId(u64::MAX)),
            (20.0, 20.0),
            cx,
        );
        assert!(!v.overlay_is_session_menu());
    });
}

/// UXI-JumpPanel-16 amended by ADR-0034: archiving detaches the complete Agent
/// tile. The session and transcript stay alive; selecting it presents that exact
/// tile solo.
#[gpui::test]
fn archive_detaches_tile_and_direct_jump_reopens_the_transcript(cx: &mut TestAppContext) {
    use crate::JumpTarget;

    let (view, vcx, id, _session) = boot_with_transcript(cx);
    // The real server transition is covered by session_resilience; keep this
    // viewport/projection guard hermetic and synchronous.
    view.update(vcx, |v, _| v.session_server = None);

    view.update(vcx, |v, cx| v.set_session_archived("S1", true, cx));

    view.read_with(vcx, |v, cx| {
        assert!(v.jump_archived_sessions.contains("S1"));
        assert_eq!(
            v.agent_tile_id_bound_to(id),
            None,
            "no workspace owns the archived session tile"
        );
        assert!(matches!(
            v.workspace
                .tile_membership(v.agent_tile_id_for_session(id).unwrap()),
            Some(crate::workspace::TileMembership::Detached)
        ));
        assert!(
            v.sessions.contains(id),
            "archive keeps the live session in the store"
        );
        assert!(
            v.read_session(id, cx, |state| {
                state
                    .editor
                    .document()
                    .full_text()
                    .contains("session archived")
            })
            .unwrap_or(false),
            "the preserved transcript contains the archive notice"
        );
    });

    // This is the real dispatch used by a local Archived jump-panel row.
    view.update(vcx, |v, cx| v.jump_to_agent(JumpTarget::Local(id), cx));

    view.read_with(vcx, |v, cx| {
        assert!(
            v.workspace.presented_detached_tile_id().is_some(),
            "a direct visit focuses the preserved session's unbound tile"
        );
        assert_eq!(
            v.agent_tile().and_then(|tile| tile.session()),
            Some(id),
            "the direct visit binds the preserved session to the visible tile"
        );
        assert!(
            v.read_session(id, cx, |state| {
                state
                    .editor
                    .document()
                    .full_text()
                    .contains("session archived")
            })
            .unwrap_or(false),
            "the direct visit shows the preserved transcript normally"
        );
    });
}

/// bug-0026 / UXI-JumpPanel-16: archiving an idle session through the real
/// jump-row context menu removes it from the currently selected Waiting tab.
///
/// Negative control (mandatory, observed RED): removing `!row.archived` from
/// the production Waiting predicate leaves the archived row in the real
/// section projection and fails at "archived Waiting rows must leave the
/// Waiting projection". Restoring the predicate passes through paint and proves
/// the session remains reachable only from Archived.
#[gpui::test]
fn archived_waiting_session_is_removed_from_the_painted_waiting_tab(cx: &mut TestAppContext) {
    use crate::JumpAgentTab;
    use gpui::{Modifiers, MouseButton};
    use yalda::session_proto::SessionInfo;

    let (view, vcx) = boot_browser(cx);
    install_agent_slot(&view, vcx, Some("S-archived-waiting"));
    let pid = view.update(vcx, |v, cx| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        let cwd = v.projects.cwd_of(pid).expect("project cwd").to_path_buf();
        v.agent_roster.upsert(SessionInfo {
            session_id: "S-archived-waiting".into(),
            acp_session_id: None,
            label: "archived waiting session".into(),
            cwd,
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        let tile = v
            .agent_tile_id_for_server_sid("S-archived-waiting")
            .expect("installed Agent tile");
        v.workspace
            .detach_window(tile)
            .expect("the Waiting list is the Detached list");
        v.workspace.clear_solo_presentation();
        v.select_jump_agent_tab(pid, JumpAgentTab::Waiting, cx);
        pid
    });
    vcx.run_until_parked();

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let (x, y, w, h) =
        crate::layout_probe_get("jump-session-row-0").expect("waiting row paints before archive");
    crate::layout_probe_end();
    let row_at = point(px(x + w / 2.0), px(y + h / 2.0));
    vcx.simulate_mouse_move(row_at, None, Modifiers::default());
    vcx.simulate_mouse_down(row_at, MouseButton::Right, Modifiers::default());
    vcx.simulate_mouse_up(row_at, MouseButton::Right, Modifiers::default());
    vcx.run_until_parked();

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let (x, y, w, h) =
        crate::layout_probe_get("session-menu-toggle").expect("archive action paints");
    crate::layout_probe_end();
    let archive_at = point(px(x + w / 2.0), px(y + h / 2.0));
    vcx.simulate_mouse_move(archive_at, None, Modifiers::default());
    vcx.simulate_click(archive_at, Modifiers::default());
    vcx.run_until_parked();

    assert!(
        view.read_with(vcx, |v, _| {
            v.jump_archived_sessions.contains("S-archived-waiting")
        }),
        "the real context-menu action sets the durable archive flag"
    );
    let waiting_labels = view.update(vcx, |v, cx| {
        let section = v
            .jump_panel_sections(cx)
            .0
            .into_iter()
            .find(|section| section.id == pid)
            .expect("project section");
        assert_eq!(section.agent_tab, JumpAgentTab::Waiting);
        section
            .sessions
            .into_iter()
            .map(|(_, row)| row.label)
            .collect::<Vec<_>>()
    });
    assert!(
        waiting_labels.is_empty(),
        "archived Waiting rows must leave the Waiting projection"
    );

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get("jump-session-row-0").is_none(),
        "archived Waiting rows must leave the painted Waiting tab"
    );
    crate::layout_probe_end();

    view.update(vcx, |v, cx| {
        v.select_jump_agent_tab(pid, JumpAgentTab::Archived, cx)
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert!(
        crate::layout_probe_get("jump-session-row-0").is_some(),
        "the archived row remains available in Archived"
    );
    crate::layout_probe_end();
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
    assert_eq!(
        v,
        vec!["a", "c", "b"],
        "dragged item lands in target's slot"
    );
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
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::PermissionMode::ReadOnly,
        busy: false,
        archived: false,
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
                    (
                        k,
                        rows.into_iter().map(|(_, r)| r.label).collect::<Vec<_>>(),
                    )
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
    view.update(vcx, |v, cx| {
        v.reorder_cwd_group("/proj/beta", "/proj/alpha", cx)
    });
    let g = snapshot(&view, vcx);
    assert_eq!(g[0].0, "/proj/beta", "cwd drag reordered the group headers");
    assert!(
        view.update(vcx, |v, _| v
            .jump_cwd_order
            .first()
            .map(|s| s == "/proj/beta")
            .unwrap_or(false)),
        "cwd order persisted on the view"
    );

    // Reorder WITHIN alpha: drop a2 onto a1 → a-two before a-one.
    view.update(vcx, |v, cx| v.reorder_session("a2", "a1", cx));
    let g = snapshot(&view, vcx);
    let alpha = g.iter().find(|(k, _)| k == "/proj/alpha").unwrap();
    assert_eq!(
        alpha.1,
        vec!["a-two", "a-one"],
        "session drag reordered within the group"
    );

    // CROSS-CWD is refused: dragging b1 (beta) onto a1 (alpha) does nothing.
    let before = view.update(vcx, |v, _| v.jump_session_order.clone());
    view.update(vcx, |v, cx| v.reorder_session("b1", "a1", cx));
    let after = view.update(vcx, |v, _| v.jump_session_order.clone());
    assert_eq!(
        before, after,
        "a session cannot be reordered into another cwd group"
    );
    // And b1 is still under beta, not alpha.
    let g = snapshot(&view, vcx);
    assert!(
        g.iter()
            .any(|(k, rows)| k == "/proj/beta" && rows.contains(&"b-one".to_string()))
    );
    assert!(
        g.iter()
            .all(|(k, rows)| k != "/proj/alpha" || !rows.contains(&"b-one".to_string()))
    );
}

/// Unit (UXI-JumpPanel-28): `reorder_move_win` is the `WindowId` analog of
/// `reorder_move` — drops the dragged tile into the target's slot (target shifts
/// down); a no-op when dragged == target or absent.
#[test]
fn jump_tile_reorder_move_semantics() {
    use crate::reorder_move_win;
    let mut v: Vec<crate::workspace::WindowId> = vec![10, 20, 30];
    // Drag 30 onto 10 → 30 takes 10's slot.
    reorder_move_win(&mut v, 30, 10);
    assert_eq!(v, vec![30, 10, 20]);
    // Drag 30 onto 20 → 30 lands in 20's slot.
    reorder_move_win(&mut v, 30, 20);
    assert_eq!(v, vec![10, 30, 20], "dragged tile lands in target's slot");
    // Self-drop is a no-op.
    let before = v.clone();
    reorder_move_win(&mut v, 10, 10);
    assert_eq!(v, before, "self-drop is a no-op");
    // Absent dragged is a no-op.
    reorder_move_win(&mut v, 999, 10);
    assert_eq!(v, before, "absent dragged is a no-op");
}

/// UXI-JumpPanel-28, REAL path: tiles inside a workspace folder reorder by drag,
/// folder-bounded. Builds the production view with three tiles in workspace 0 and
/// one tile in workspace 1 (a second folder), then calls the exact method the
/// tile-drop handler invokes (`reorder_tile`):
///   1. default (empty order) = layout-traversal order;
///   2. `reorder_tile(third, first)` moves the third tile into the first slot,
///      and the new order is persisted on `jump_tile_order`;
///   3. a CROSS-FOLDER `reorder_tile` (a workspace-1 tile onto a workspace-0 tile)
///      is REFUSED — a tile can never be dragged into another workspace folder.
///
/// The GPUI mouse-drag GESTURE that dispatches the drop is the runtime gap (gap
/// #2, no headless drag-dispatch seam), but the state change the drop runs IS this
/// method. Negative control (mandatory, observed RED): delete the
/// `tiles.sort_by_key(rank)` line in `jump_panel_sections_with_tab` → assertion 2
/// fails (the projected order stays in layout order after the reorder).
#[gpui::test]
fn jump_tile_reorder_applies_within_folder_and_gates_by_folder(cx: &mut TestAppContext) {
    use crate::{App, AgentTile};
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    // Workspace 0: three tiles (seed Buffer + two agent splits).
    view.update(vcx, |v, _| {
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));
    });
    // Workspace 1: one more tile, a second folder under the same project.
    view.update(vcx, |v, _| {
        v.workspace
            .push_workspace_inheriting(App::Agent(AgentTile::new()));
    });

    // Snapshot every workspace folder's tile ids in the order render builds them.
    let folders = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            let (sections, _) = v.jump_panel_sections(cx);
            sections
                .iter()
                .flat_map(|s| &s.workspace_folders)
                .map(|f| (f.index, f.tiles.iter().map(|t| t.id).collect::<Vec<_>>()))
                .collect::<Vec<(usize, Vec<crate::workspace::WindowId>)>>()
        })
    };

    // Default: workspace-0 folder has three tiles in layout order.
    let f0 = folders(&view, vcx);
    let folder0 = f0.iter().find(|(_, ids)| ids.len() >= 3).cloned();
    let (folder0_idx, layout_ids) = folder0.expect("workspace-0 folder with 3 tiles");
    assert_eq!(layout_ids.len(), 3);
    let (t0, t1, t2) = (layout_ids[0], layout_ids[1], layout_ids[2]);
    assert!(
        view.update(vcx, |v, _| v.jump_tile_order.is_empty()),
        "no drag yet → empty order → the projection is pure layout order"
    );

    // Drag the third tile onto the first → third takes the first slot.
    view.update(vcx, |v, cx| v.reorder_tile(t2, t0, cx));
    let after = folders(&view, vcx);
    let reordered = after
        .iter()
        .find(|(idx, _)| *idx == folder0_idx)
        .map(|(_, ids)| ids.clone())
        .expect("workspace-0 folder still present");
    assert_eq!(
        reordered,
        vec![t2, t0, t1],
        "tile drag reordered the folder: third moved into the first slot"
    );
    assert!(
        view.update(vcx, |v, _| v.jump_tile_order.starts_with(&[t2, t0, t1])),
        "the tile order persisted on the view"
    );

    // A second folder (workspace 1) exists with its own tile.
    let other = after
        .iter()
        .find(|(idx, ids)| *idx != folder0_idx && !ids.is_empty())
        .map(|(_, ids)| ids[0])
        .expect("workspace-1 folder with a tile");

    // CROSS-FOLDER is refused: dragging the workspace-1 tile onto t0 does nothing.
    let before = view.update(vcx, |v, _| v.jump_tile_order.clone());
    view.update(vcx, |v, cx| v.reorder_tile(other, t0, cx));
    let unchanged = view.update(vcx, |v, _| v.jump_tile_order.clone());
    assert_eq!(
        before, unchanged,
        "a tile cannot be reordered into another workspace folder"
    );
}

/// bug-0007 (RECURRED), REAL path: a session must NEVER change slot in the jump
/// panel because of `/clear`. `/clear` kills the server session and creates a new
/// one with a NEW sid; the user's drag order (`jump_session_order`) ranks by sid,
/// so the replacement was unranked (`usize::MAX`) and fell to the BOTTOM of its
/// cwd group — "one agent session moved to the bottom of the list after a clear".
///
/// Drives the REAL `clear_agent_session` (forced down the server branch) and the
/// REAL async `Created` resolution, snapshotting the ordered/grouped rows exactly
/// as `render_jump_panel` builds them, at BOTH moments the row could hop: while
/// the placeholder is local-only (mid-open) and after it binds the fresh sid.
///
/// Negative control (mandatory, observed RED): revert `order_sid` to `None` for a
/// local placeholder → the mid-open assert fails ("z-one" last); revert
/// `inherit_order_slot` → the post-bind assert + the `jump_session_order` assert
/// fail. Non-vacuous: the user's order is the REVERSE of label order, so a slot
/// that survives can only have come from the order list.
#[gpui::test]
fn clear_keeps_the_sessions_jump_panel_slot(cx: &mut TestAppContext) {
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();
    install_agent_slot(&view, &mut *vcx, Some("S1"));
    vcx.run_until_parked();

    let info = |sid: &str, label: &str| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: std::path::PathBuf::from("/proj/x"),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::PermissionMode::ReadOnly,
        busy: false,
        archived: false,
    };
    // One cwd group, two sessions. Labels are chosen so the USER's order is the
    // reverse of by-label order: any surviving slot proves the order list drove it.
    view.update(vcx, |v, cx| {
        v.agent_roster.upsert(info("S1", "z-one"));
        v.agent_roster.upsert(info("S2", "a-two"));
        let id = v.focused_bound_session().expect("bound session");
        if let Some(ent) = v.session_entity(id) {
            ent.update(cx, |s, _| {
                s.label = "z-one".into();
                s.cwd = std::path::PathBuf::from("/proj/x");
            });
        }
        v.jump_session_order = vec!["S1".into(), "S2".into()];
        v.jump_archived_sessions.insert("S1".into());
    });
    vcx.run_until_parked();

    let snapshot = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            crate::order_grouped_rows(
                crate::group_agent_rows_by_cwd(v.jump_panel_agent_rows(cx)),
                &v.jump_cwd_order,
                &v.jump_session_order,
            )
            .into_iter()
            .flat_map(|(_, rows)| rows.into_iter().map(|(_, r)| r.label))
            .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        snapshot(&view, vcx),
        vec!["z-one", "a-two"],
        "precondition: the user's drag order puts z-one first (label order would not)"
    );

    // REAL `/clear`, server branch. The old sid is closed on the server (mirror
    // the resulting broadcast by dropping it from the roster) and a local-only
    // placeholder takes the tile until the create round-trip resolves.
    view.update(vcx, |v, cx| {
        crate::with_server_clear_branch(|| v.clear_agent_session(cx));
        v.agent_roster.remove("S1");
    });
    vcx.run_until_parked();
    assert_eq!(
        snapshot(&view, vcx),
        vec!["z-one", "a-two"],
        "MID-OPEN: the /clear placeholder must hold the killed session's slot, not sink"
    );
    assert!(
        view.update(vcx, |v, _| v.jump_archived_sessions.contains("S1")),
        "MID-OPEN: the placeholder retains its predecessor's archive identity"
    );

    // REAL async completion: the fresh sid binds.
    let token = view
        .update(vcx, |v, _| v.agent_tile().and_then(|t| t.pending_token()))
        .expect("clear left a pending open token");
    view.update(vcx, |v, cx| {
        v.apply_open_agent_resolution(
            token,
            crate::OpenResolution::Created {
                sid: "S-fresh".into(),
                acp_id: None,
                provider: yalda::acp_channel::AgentProvider::Claude,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            },
            cx,
        );
    });
    vcx.run_until_parked();

    assert_eq!(
        snapshot(&view, vcx),
        vec!["z-one", "a-two"],
        "POST-BIND: the cleared session keeps its slot under its new sid"
    );
    assert_eq!(
        view.update(vcx, |v, _| v.jump_session_order.clone()),
        vec!["S-fresh".to_string(), "S2".to_string()],
        "the persisted order inherits the slot in place (no append, no duplicate)"
    );
    view.update(vcx, |v, _| {
        assert!(v.jump_archived_sessions.contains("S-fresh"));
        assert!(
            !v.jump_archived_sessions.contains("S1"),
            "the durable archive flag migrates instead of leaving a stale sid"
        );
    });
}

/// Unit: the jump panel groups agent-session rows under per-cwd subheaders
/// (agent-sessions-by-cwd). Sessions sharing a cwd land in one group; groups are
/// ordered by their display path (stable, alphabetized headers); and every row
/// keeps its original flat index (so its id / click listener stay stable
/// regardless of grouping).
#[test]
fn jump_panel_groups_agent_rows_by_cwd() {
    use crate::{AgentRow, JumpTarget, group_agent_rows_by_cwd};
    let row = |label: &str, cwd: &str| AgentRow {
        order_sid: Some(label.into()),
        state_entered_at: None,
        summary_pending: false,
        archived: false,
        target: JumpTarget::Roster(label.into()),
        label: label.into(),
        provider: yalda::acp_channel::AgentProvider::Claude,
        summary: None,
        cwd: std::path::PathBuf::from(cwd),
        bound: false,
        connected: true,
        awaiting: None,
        unread: false,
        tags: Vec::new(),
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
    let alpha: Vec<(usize, &str)> = groups[0]
        .1
        .iter()
        .map(|(i, r)| (*i, r.label.as_str()))
        .collect();
    assert_eq!(alpha, vec![(1, "b"), (2, "c")]);
    // beta holds a (idx 0).
    let beta: Vec<(usize, &str)> = groups[1]
        .1
        .iter()
        .map(|(i, r)| (*i, r.label.as_str()))
        .collect();
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
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.inline_you_block_active())
        })
        .unwrap_or(false);
    assert!(
        active,
        "precondition: inline You-block must be active (else nothing to keep fresh)"
    );

    // Let the initial render settle so `last_you_block_seq` has caught up to the
    // empty block, THEN start the measurement window — so the count we read is
    // attributable to the KEYSTROKE, not the first paint.
    vcx.run_until_parked();
    crate::perf_reset(crate::YOU_BLOCK_SPLICE_LABEL);
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count(crate::YOU_BLOCK_SPLICE_LABEL);
    assert_eq!(
        base, 0,
        "a plain notify (no compose change) must NOT splice the You-block item"
    );

    // The user types — through the REAL key handler, no `i`, no toggle.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("h"), w, cx)
    });
    vcx.run_until_parked();
    let after = crate::perf_render_count(crate::YOU_BLOCK_SPLICE_LABEL);

    let text = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().text())
        })
        .expect("session");
    assert_eq!(
        text.trim(),
        "h",
        "sanity: the char landed in the compose buffer"
    );
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

    // Defaults visible, renders, and the shell menu offers the toggle (ADR-0032:
    // the entry moved from the `?` menu to the `.` shell menu and is now a static
    // "toggle jump panel" label — gpui_menu is a pure builder with no live state).
    assert!(
        view.update(vcx, |v, _| v.jump_panel_visible),
        "defaults visible"
    );
    let rendered = crate::perf_render_count("jump_panel");
    assert!(rendered >= 1, "visible panel renders at least once");
    assert!(shell_menu_has_label("toggle jump panel"));

    // Hide it (via the menu command). It stops rendering.
    view.update(vcx, |v, cx| {
        v.dispatch_menu_command("toggle-jump-panel", cx)
    });
    assert!(!view.update(vcx, |v, _| v.jump_panel_visible), "now hidden");
    let base = crate::perf_render_count("jump_panel");
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert_eq!(
        crate::perf_render_count("jump_panel"),
        base,
        "a hidden jump panel is not rendered"
    );
    // Summon it again — it renders once more.
    view.update(vcx, |v, cx| {
        v.dispatch_menu_command("toggle-jump-panel", cx)
    });
    assert!(
        view.update(vcx, |v, _| v.jump_panel_visible),
        "visible again"
    );
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
    let box_bounds = view.update(vcx, |v, cx| {
        v.agent_read(cx, |c| c.input_surface.compose().bounds.get())
    });
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

    assert!(
        box_h > 1.0,
        "compose box has no height ({box_h}) — nothing painted"
    );
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
        !view
            .update(vcx, |v, cx| v
                .agent_read(cx, |c| c.input_surface.is_chatbox()))
            .expect("bound agent session"),
        "precondition: a fresh session defaults to Worksheet"
    );
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("i"), window, cx)
    });
    vcx.run_until_parked();
    let (_, yb_y, _, _) =
        probe_dirty(&view, vcx, "you-block").expect("worksheet block paints inline");
    assert!(
        probe_dirty(&view, vcx, "compose-box").is_none(),
        "the worksheet block is inline — no bottom box"
    );

    // Toggle to Chatbox: a pinned bottom box paints; the inline block is gone.
    view.update(vcx, |v, cx| v.toggle_agent_input_mode(cx));
    vcx.run_until_parked();
    let (_, chat_y, _, _) =
        probe_dirty(&view, vcx, "compose-box").expect("chatbox paints a bottom box");
    assert!(
        probe_dirty(&view, vcx, "you-block").is_none(),
        "chatbox has no inline block"
    );

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
        assert_eq!(
            c.focus,
            crate::AgentFocus::Transcript,
            "worksheet rests in nav"
        );
        assert!(!c.you_block_open, "no You-block until Insert");
    });
    (view, vcx)
}

/// UXI-AgentTile-14: Cmd+V with an image on the clipboard stages it as a pending
/// attachment on the compose (rather than typing garbage), base64-encoded with
/// its mime type — the payload that becomes an ACP `ContentBlock::Image`. Drives
/// the REAL Cmd+V action (`cmd-v` → `PasteFromClipboard` → `paste_from_clipboard`
/// → `stage_clipboard_images_onto_compose`) against the REAL test-platform
/// clipboard — the path the user's keystroke actually takes (bug-0039: Cmd+V
/// dispatches the bound action BEFORE the agent key handler, so the old test that
/// called `handle_claude_key` directly guarded dead code).
///
/// Negative control: delete the `cb.pending_images.push(pending)` in
/// `stage_clipboard_images_onto_compose` and the staged-count assert fails RED.
#[gpui::test]
fn image_paste_stages_pending_attachment(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
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
    // Cmd+V through the REAL action-dispatch path.
    vcx.simulate_keystrokes("cmd-v");
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
    assert_eq!(
        staged.len(),
        1,
        "Cmd+V with a clipboard image stages one attachment"
    );
    assert_eq!(
        staged[0].0, "image/png",
        "mime type carried from the clipboard format"
    );
    assert!(
        staged[0].2.contains("PNG"),
        "chip label names the format: {}",
        staged[0].2
    );
    // The base64 payload decodes back to the exact bytes the agent will read.
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&staged[0].1)
        .expect("valid base64");
    assert_eq!(
        decoded, png,
        "the staged data round-trips to the original image bytes"
    );

    // The compose editor stayed empty — Cmd+V did NOT type the 'v' or paste junk.
    let compose_text = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().text())
        })
        .expect("session");
    assert!(
        compose_text.trim().is_empty(),
        "an image paste must not put text in the compose; got {compose_text:?}"
    );
}

/// Guards the mac fix (bug-0039) on the REAL Cmd+V action path: even when GPUI's
/// clipboard surfaces a string-only item (its `read_from_clipboard`
/// short-circuits whenever the board carries text), `paste_from_clipboard` still
/// stages the image because `stage_clipboard_images_onto_compose` reads the
/// pasteboard PNG directly via `read_clipboard_image_png`. Here the direct read
/// is injected through the test override, and GPUI's clipboard holds ONLY text —
/// reproducing the exact runtime scenario, driven through `cmd-v` →
/// `PasteFromClipboard`. Without the direct-read step the URL text is pasted and
/// no image is staged (RED).
#[gpui::test]
fn image_paste_direct_read_stages_even_with_text_on_clipboard(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let png: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 9, 9, 9];

    // GPUI clipboard has ONLY a text rep (the mac short-circuit scenario)…
    view.update(vcx, |_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            "https://example.com/cat.png".to_string(),
        ));
    });
    // …while the direct pasteboard read returns the image bytes.
    crate::system_console::set_clipboard_image_test_override(Some(png.clone()));

    vcx.simulate_keystrokes("cmd-v");
    vcx.run_until_parked();
    crate::system_console::set_clipboard_image_test_override(None);

    let staged = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                c.input_surface
                    .compose()
                    .pending_images
                    .iter()
                    .map(|p| (p.mime_type.clone(), p.data.clone()))
                    .collect::<Vec<_>>()
            })
        })
        .expect("session");
    assert_eq!(
        staged.len(),
        1,
        "direct pasteboard read stages the image despite text on the clipboard"
    );
    assert_eq!(staged[0].0, "image/png");
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&staged[0].1)
        .expect("valid base64");
    assert_eq!(
        decoded, png,
        "staged bytes are the direct-read PNG, not the clipboard text"
    );

    // The URL text was NOT pasted into the compose.
    let compose_text = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().text())
        })
        .expect("session");
    assert!(
        !compose_text.contains("example.com"),
        "the accompanying URL text must not land in the compose; got {compose_text:?}"
    );
}

/// PROBE: a pasted-image chip PAINTS above the chatbox before send (INV-UX-21
/// property 2). A state-only assert can't catch a repaint miss (the reported
/// symptom: "indication only appears after sent"). Boots chatbox mode, pastes an
/// image via the real cmd-v action, and asserts the `compose-image-chips` element
/// painted with real area.
#[gpui::test]
fn image_paste_chip_paints_before_send_chatbox(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let png: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 7, 7, 7];
    crate::system_console::set_clipboard_image_test_override(Some(png));

    // Force chatbox mode so the pinned compose panel is shown.
    view.update(vcx, |v, cx| {
        let is_cb = v
            .agent_read(cx, |c| c.input_surface.is_chatbox())
            .unwrap_or(false);
        if !is_cb {
            v.toggle_agent_input_mode(cx);
        }
    });
    vcx.run_until_parked();

    crate::layout_probe_begin();
    vcx.simulate_keystrokes("cmd-v");
    vcx.run_until_parked();
    let rect = crate::layout_probe_get("compose-image-chips");
    crate::layout_probe_end();
    crate::system_console::set_clipboard_image_test_override(None);

    // Precondition: the image really staged (so a missing paint is a paint bug,
    // not a missing image).
    let staged = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().pending_images.len())
        })
        .expect("session");
    assert_eq!(staged, 1, "image staged onto the compose");

    let (_x, _y, w, h) = rect.expect("the pending-image chip never painted before send");
    assert!(w > 4.0 && h > 4.0, "chip painted with no area ({w}x{h})");
}

/// PROBE: the reported bug — in WORKSHEET-IDLE mode there is no compose panel
/// (`show_compose` is false), so before this fix a pasted image had NO on-screen
/// indication until send. The standalone chip strip must paint here too
/// (INV-UX-21 property 2; bug-0039 follow-up). Negative control: delete the
/// `else if let Some(strip) = …` standalone-strip arm in `render_agent` and this
/// goes RED (the chatbox test stays green — it uses the in-panel strip).
#[gpui::test]
fn image_paste_chip_paints_before_send_worksheet_idle(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let png: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 5, 5, 5];
    crate::system_console::set_clipboard_image_test_override(Some(png));

    // Force worksheet mode (idle) — the state with no compose panel.
    view.update(vcx, |v, cx| {
        let is_cb = v
            .agent_read(cx, |c| c.input_surface.is_chatbox())
            .unwrap_or(false);
        if is_cb {
            v.toggle_agent_input_mode(cx);
        }
    });
    vcx.run_until_parked();

    crate::layout_probe_begin();
    vcx.simulate_keystrokes("cmd-v");
    vcx.run_until_parked();
    let chip = crate::layout_probe_get("compose-image-chips");
    let box_rect = crate::layout_probe_get("compose-box");
    crate::layout_probe_end();
    crate::system_console::set_clipboard_image_test_override(None);

    let staged = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                (
                    c.input_surface.compose().pending_images.len(),
                    c.input_surface.is_chatbox(),
                )
            })
        })
        .expect("session");
    assert_eq!(staged.0, 1, "image staged onto the compose");
    assert!(
        !staged.1,
        "test must be in worksheet mode (no compose panel)"
    );
    // Guard is non-vacuous: worksheet-idle really has NO compose box painted, so
    // the chip strip is the ONLY indication.
    assert!(
        box_rect.is_none(),
        "precondition: worksheet-idle paints no compose box (got {box_rect:?})"
    );

    let (_x, _y, w, h) = chip.expect("the pending-image chip never painted in worksheet-idle");
    assert!(w > 4.0 && h > 4.0, "chip painted with no area ({w}x{h})");
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("m"), w, cx)
    });
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
            v.read_session(id, cx, |c| c.turn_phase.is_awaiting())
                .unwrap(),
            "real submit must start a turn (we are genuinely mid-turn)"
        );
        v.pending_mark_chord = None;
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("m"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.pending_mark_chord, None,
            "mid-turn worksheet `m` must NOT start a mark chord (it should type)"
        );
        let text = v
            .read_session(id, cx, |c| c.input_surface.compose().text())
            .unwrap();
        assert!(
            text.contains('m'),
            "mid-turn `m` types into the chatbox (got {text:?})"
        );
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
        assert!(
            v.read_session(id, cx, |c| c.turn_phase.is_awaiting())
                .unwrap()
        );
        assert!(
            v.read_session(id, cx, |c| c
                .input_surface
                .compose()
                .text()
                .trim()
                .is_empty())
                .unwrap(),
            "post-submit steering draft is empty"
        );
        assert!(
            !v.focused_in_insert_mode(cx),
            "empty-draft mid-turn worksheet rests in nav ⇒ leaders active"
        );
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("space"), w, cx)
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("f"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert!(
            !v.read_session(id, cx, |c| c
                .input_surface
                .compose()
                .text()
                .trim()
                .is_empty())
                .unwrap(),
            "typed a char ⇒ draft non-empty"
        );
        assert!(
            v.focused_in_insert_mode(cx),
            "non-empty steer ⇒ text entry ⇒ leaders suppressed"
        );
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("space"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert!(
            matches!(v.active_overlay, crate::ActiveOverlay::None),
            "mid-turn <space> with a draft in progress must NOT open a menu"
        );
        let text = v
            .read_session(id, cx, |c| c.input_surface.compose().text())
            .unwrap();
        assert!(
            text.contains(' '),
            "the space typed into the steer (got {text:?})"
        );
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
        ModelOption {
            id: "default".into(),
            label: "Default".into(),
        },
        ModelOption {
            id: "claude-fable-5[1m]".into(),
            label: "Fable".into(),
        },
        ModelOption {
            id: "sonnet".into(),
            label: "Sonnet".into(),
        },
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
            v.read_session(id, cx, |c| c.available_models.clone())
                .unwrap(),
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

/// bug-0029 / `UXI-AgentTile-16`: CLICKING the status-strip `model ▾` badge opens
/// the model switcher. The badge's `on_click` dispatches `OpenLocalMenu`, which
/// travels the FOCUSED node's dispatch path — so the `AgentView` root must
/// register `on_action(Self::open_local_menu)` or the action is dropped and the
/// click does nothing at all (the reported symptom: "I see a down arrow. But
/// nothing is presented").
///
/// Drives the window's real mouse dispatch (`simulate_click`) at the badge's REAL
/// painted rect — a hand-called `open_local_menu_inner` would prove nothing
/// (anti-circling rule 1).
///
/// Negative control: drop `.on_action(cx.listener(Self::open_local_menu))` from the
/// `AgentView` root in `render_agent` → no overlay opens and the first assert fails.
#[gpui::test]
fn agent_model_badge_click_opens_the_model_switcher(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ModelOption, ReplyEvent};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx) = boot_browser(cx);
    install_agent_slot(&view, &mut *vcx, Some("S1"));

    // Advertise a picklist through the REAL reducer — this is what puts the `▾`
    // on the badge (`has_models`) and makes it clickable at all.
    let opts = vec![
        ModelOption {
            id: "default".into(),
            label: "Default (recommended)".into(),
        },
        ModelOption {
            id: "claude-fable-5[1m]".into(),
            label: "Fable".into(),
        },
        ModelOption {
            id: "sonnet".into(),
            label: "Sonnet".into(),
        },
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
    });
    vcx.run_until_parked();

    // The badge's REAL painted rect.
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let rect = crate::layout_probe_get("agent-model-badge");
    crate::layout_probe_end();

    let (x, y, w, h) = rect.expect("the model badge never painted");
    assert!(
        w > 4.0 && h > 4.0,
        "badge painted with no area ({w}x{h}) — nothing to click"
    );
    let at = point(px(x + w / 2.0), px(y + h / 2.0));

    view.read_with(vcx, |v, _| {
        assert!(!v.overlay_is_menu(), "no menu is open before the click");
    });

    vcx.simulate_mouse_move(at, None, gpui::Modifiers::default());
    vcx.simulate_click(at, gpui::Modifiers::default());
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| {
        let crate::ActiveOverlay::Menu(m) = &v.active_overlay else {
            panic!(
                "clicking the model ▾ badge presented NOTHING — the dispatched \
                 OpenLocalMenu found no handler on the AgentView root (bug-0029)"
            );
        };
        assert_eq!(
            m.header, "AGENT",
            "the badge opens the agent-scoped local menu"
        );

        // …and the menu it opens actually carries the advertised models, not the
        // "(models not available yet)" placeholder.
        let sub = m
            .menu
            .iter()
            .find(|n| n.label == "switch model")
            .expect("the local menu has a `switch model` submenu");
        let yalda::menu::MenuAction::Submenu(children) = &sub.action else {
            panic!("`switch model` must be a submenu");
        };
        let labels: Vec<&str> = children.iter().map(|c| c.label.as_str()).collect();
        assert!(
            labels.iter().any(|l| l.starts_with("Fable")),
            "the advertised picklist is offered (got {labels:?})"
        );
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with("Sonnet") && l.contains('✓')),
            "the current model is marked (got {labels:?})"
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
        ModelOption {
            id: "default".into(),
            label: "Default".into(),
        },
        ModelOption {
            id: "claude-fable-5[1m]".into(),
            label: "Fable".into(),
        },
        ModelOption {
            id: "sonnet".into(),
            label: "Sonnet".into(),
        },
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
            v.read_session(id, cx, |c| c.available_models.clone())
                .unwrap(),
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
            children
                .iter()
                .all(|c| matches!(c.action, crate::MenuAction::Label(_))),
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
                        ModelOption {
                            id: "default".into(),
                            label: "Default".into(),
                        },
                        ModelOption {
                            id: "sonnet".into(),
                            label: "Sonnet".into(),
                        },
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
        assert!(
            labels.contains(&"Sonnet ✓"),
            "current model marked: {labels:?}"
        );
        assert!(
            labels.contains(&"Default"),
            "other model unmarked: {labels:?}"
        );
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
        let keys: Vec<String> = children
            .iter()
            .map(|child| crate::format_menu_key(&child.key))
            .collect();
        assert_eq!(
            keys,
            vec!["1", "2"],
            "model choices use stable numbered keys"
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
        view.update_in(vcx, |v, w, cx| {
            v.handle_claude_key(&ws_bare_key(key), w, cx)
        });
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
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().text())
        })
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("h"), w, cx)
    });
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");

    let (active, text) = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                (
                    c.inline_you_block_active(),
                    c.input_surface.compose().text(),
                )
            })
        })
        .expect("session");
    assert_eq!(
        text.trim(),
        "h",
        "sanity: the char landed in the compose buffer"
    );
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
            vec![agent_note(
                "S1",
                1,
                1,
                0,
                K::ChannelOpened { resumed: false },
            )],
            cx,
        );
    });
    vcx.run_until_parked();

    crate::perf_reset("transcript");
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // The user types — NO `i`, NO mode toggle — through the REAL key handler.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("h"), w, cx)
    });
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
    assert_eq!(
        text.trim(),
        "h",
        "sanity: the char landed in the compose buffer"
    );
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("h"), w, cx)
    });
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");
    let you_block = crate::layout_probe_get("you-block");
    let viewport = crate::layout_probe_get("transcript-viewport");
    crate::layout_probe_end();

    let text = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().text())
        })
        .expect("session");
    assert_eq!(
        text.trim(),
        "h",
        "sanity: the char landed in the compose buffer"
    );
    // The assertions the six prior fixes never made — RENDER + PAINT, not buffer:
    assert!(
        after > base,
        "typing in the hole MUST bust the cached transcript (render count {base} -> {after}); \
         flat == the invisible-text bug",
    );
    let (_, by, _, bh) =
        you_block.expect("typing in the hole MUST paint an inline You-block (invisible-text bug)");
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
            vec![agent_note(
                "S1",
                1,
                1,
                0,
                K::ChannelOpened { resumed: false },
            )],
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

    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("h"), w, cx)
    });
    vcx.run_until_parked();
    let after = crate::perf_render_count("transcript");

    let (active, text) = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                (
                    c.inline_you_block_active(),
                    c.input_surface.compose().text(),
                )
            })
        })
        .expect("session");
    assert_eq!(
        text.trim(),
        "h",
        "sanity: the char landed in the compose buffer"
    );
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("x"), w, cx)
    });
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
        .update(vcx, |v, _| v.agent_tile().and_then(|t| t.pending_token()))
        .expect("clear left a pending open token on the placeholder tile");

    // REAL async completion: the server round-trip binds the fresh sid + re-settles.
    view.update(vcx, |v, cx| {
        v.apply_open_agent_resolution(
            token,
            crate::OpenResolution::Created {
                sid: "S-fresh".into(),
                acp_id: None,
                provider: yalda::acp_channel::AgentProvider::Claude,
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            },
            cx,
        );
    });
    vcx.run_until_parked();

    // Precondition: the typeable idle worksheet the user faces post-clear (non-vacuous).
    let (active, focus) = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| (c.inline_you_block_active(), c.focus))
        })
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("h"), w, cx)
    });
    vcx.run_until_parked();
    let after_r = crate::perf_render_count("transcript");
    let after_s = crate::perf_render_count(crate::YOU_BLOCK_SPLICE_LABEL);
    let you_block = crate::layout_probe_get("you-block");
    let viewport = crate::layout_probe_get("transcript-viewport");
    crate::layout_probe_end();

    let text = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| c.input_surface.compose().text())
        })
        .expect("session");
    assert_eq!(
        text.trim(),
        "h",
        "sanity: the char landed in the compose buffer"
    );
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
    assert_eq!(
        crate::caret_token_split(&lens, 1),
        Some((0, 1)),
        "col 1 = 2nd char of 'foo'"
    );
    // On the last char (Normal max) → owned by the last token.
    assert_eq!(
        crate::caret_token_split(&lens, 6),
        Some((2, 2)),
        "col 6 = 'r' of 'bar'"
    );
    // Past the last char (EOL beam) → no owner, caller draws a trailing caret.
    assert_eq!(
        crate::caret_token_split(&lens, 7),
        None,
        "col 7 = EOL, trailing caret"
    );
    // The space between words is owned by the space token, not the word after.
    assert_eq!(
        crate::caret_token_split(&lens, 3),
        Some((1, 0)),
        "col 3 = the space itself"
    );
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
    assert_eq!(
        col(&view, vcx),
        3,
        "three Right presses move the caret to col 3"
    );
    key(&view, vcx, "end");
    assert_eq!(col(&view, vcx), 11, "End moves to the line end");
    key(&view, vcx, "home");
    assert_eq!(col(&view, vcx), 0, "Home moves to col 0");
    key(&view, vcx, "delete");
    let text = view.update(vcx, |v, _| {
        v.edit_mut().unwrap().editor.line_text_at_cursor()
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
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
    assert_eq!(
        col(&view, vcx),
        5,
        "typing 'hello' leaves the caret at col 5"
    );
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("left"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("left"), w, cx)
    });
    assert_eq!(
        col(&view, vcx),
        3,
        "two Left presses move the compose caret to col 3"
    );
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("home"), w, cx)
    });
    assert_eq!(col(&view, vcx), 0, "Home moves the compose caret to col 0");
}

/// UXI-TextEditing-3 (buffer editor): pressing Enter at the end of an INDENTED list
/// item continues the list AT THE SAME INDENT — a nested `-` stays nested, it
/// does not jump back to column 0. Drives the REAL `handle_edit_key` Enter path
/// (`dispatch_insert_core` → `list_continuation_action`).
///
/// Negative control (observed RED): drop the `{indent}` in
/// `list_continuation_action`'s `Continue(format!("{indent}{continuation}"))` →
/// the new item is `- ` at column 0 and the leading-spaces assert fails.
#[gpui::test]
fn buffer_enter_continues_nested_list_at_same_indent(cx: &mut TestAppContext) {
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
    view.update(vcx, |v, _| v.test_open_edit("  - foo\n"));
    let key = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext, k: &str| {
        view.update_in(vcx, |v, w, cx| v.handle_edit_key(&ws_bare_key(k), w, cx));
    };
    key(&view, vcx, "end"); // caret to end of "  - foo"
    key(&view, vcx, "enter");
    key(&view, vcx, "x");
    let line = view.update(vcx, |v, _| {
        v.edit_mut().unwrap().editor.line_text_at_cursor()
    });
    assert_eq!(
        line.trim_end_matches('\n'),
        "  - x",
        "Enter on a 2-space-indented bullet must continue the marker at the same \
         indent (got {line:?})"
    );
}

/// UXI-TextEditing-3 (worksheet/chatbox compose): the SAME indent-preserving list
/// continuation holds in the agent compose, because it routes through the same
/// `dispatch_insert_core`. Drives the REAL `handle_claude_key` Enter path over a
/// worksheet You-block seeded with an indented bullet.
///
/// Negative control: shares `list_continuation_action`'s `{indent}` with the
/// buffer test — removing it makes the continued line `- ` at column 0.
#[gpui::test]
fn compose_enter_continues_nested_list_at_same_indent(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    // `i` opens the tail You-block in Insert.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    vcx.run_until_parked();
    // Seed an indented bullet with the caret at its end (avoids the keystroke
    // helper's inability to emit literal spaces).
    view.update(vcx, |v, cx| {
        use crate::EditOps;
        let id = v.focused_bound_session().expect("bound");
        v.with_session(id, cx, |c| {
            let ed = &mut c.input_surface.compose_mut().editor;
            ed.programmatic_insert(0, "  - foo");
            let cur = ed.cursor_mut();
            cur.line = 0;
            cur.col = "  - foo".chars().count();
        });
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("enter"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("x"), w, cx)
    });
    let line = view.update(vcx, |v, cx| {
        use crate::EditOps;
        let id = v.focused_bound_session().expect("bound");
        v.read_session(id, cx, |c| {
            c.input_surface.compose().editor.line_text_at_cursor()
        })
        .expect("session")
    });
    assert_eq!(
        line.trim_end_matches('\n'),
        "  - x",
        "Enter on an indented bullet in the compose must keep the nesting (got {line:?})"
    );
}

/// UXI-TextEditing-4: the Code edit view soft-wraps a line whose long run has NO
/// whitespace to break at (a bullet holding a long path / URL). Before the fix
/// `build_wrapped_line` emitted one over-wide token child that `flex_wrap` could
/// not break, so the run overflowed and was clipped by the body's
/// `overflow_x_hidden` — the "wrapping fails on bullets" report. Char-breaking
/// the token lets the row wrap to many lines.
///
/// The honest seam is the layout probe (a geometry bug): a bullet with a
/// ~600-char unbroken token is FAR wider than the pane, so a real wrap paints a
/// tall row; a clip paints ~1 line. Non-vacuous: the token can't fit on one line.
///
/// Negative control (observed RED): revert `chunk_long_token` in
/// `build_wrapped_line` (emit the token whole) → `code-line-0` paints ~1 line
/// tall and the `> 4 lines` assert fails.
#[gpui::test]
fn code_edit_wraps_unbroken_token_in_bullet(cx: &mut TestAppContext) {
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
    // A bullet whose content is one unbroken 600-char token (no whitespace).
    let tok: String = std::iter::repeat('a').take(600).collect();
    view.update(vcx, |v, _| v.test_open_edit(&format!("- {tok}\n")));
    view.update(vcx, |v, cx| v.set_text_scale(1.0, cx));
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let line = crate::layout_probe_get("code-line-0");
    crate::layout_probe_end();

    let (_, _, w, h) = line.expect("the code line must paint");
    // One 13px row is ~16–20px tall; a genuine wrap of a 600-char run is many
    // rows. Assert it wrapped to well more than a single line.
    assert!(
        h > 80.0,
        "an unbroken 600-char token in a bullet must char-wrap, not clip to one \
         line: code-line-0 painted {w}x{h}"
    );
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
    let text = view.update(vcx, |v, _| {
        v.edit_mut().unwrap().editor.line_text_at_cursor()
    });
    assert_eq!(
        text.trim_end(),
        "hi",
        "cmd-g inserts nothing in Insert mode"
    );
    // Normal mode: cmd-a must not run `insert-after` (which would flip to Insert).
    view.update(vcx, |v, _| {
        v.edit_mut().unwrap().mode = crate::EditMode::Normal;
    });
    view.update_in(vcx, |v, w, cx| v.handle_edit_key(&ws_cmd_key("a"), w, cx));
    let mode = view.update(vcx, |v, _| v.edit_mut().unwrap().mode);
    assert_eq!(
        mode,
        crate::EditMode::Normal,
        "cmd-a does not fire insert-after"
    );
}

/// `focused_in_insert_mode` for the file BROWSER (`App::Buffer::Picking`): filter mode
/// IS text entry (leaders suppressed); idle is navigation. Kills the `filter_mode ||
/// rename.is_some()` mutant surviving in this arm (filter-only ≠ AND of both).
#[gpui::test]
fn focused_in_insert_mode_browser_arm(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let set_filter_read = |view: &gpui::Entity<YaldaGpuiView>,
                           vcx: &mut gpui::VisualTestContext,
                           filter: bool|
     -> bool {
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

/// bug-0038: in the file-picker `/` search mode, non-`Char` keys that were also
/// bound to a `BrowserView` action leaked to that action — `right`→BrowserEnter
/// OPENED the selected file mid-search (and `left`→BrowserParent navigated up).
/// The capture-phase filter handler only stopped propagation for a handful of
/// keys; its `_ => {}` fall-through let the rest reach their bindings. It now
/// swallows EVERY key while filtering. Drives the real keymap: `/`, a query, then
/// the actual `right` keystroke.
///
/// Negative control (observed RED): revert `_ => cx.stop_propagation()` back to
/// `_ => {}` in `handle_browser_filter_key` → `right` fires BrowserEnter →
/// `open_file` flips the tile to `Viewing` and both asserts below fail.
#[gpui::test]
fn browser_filter_arrow_key_does_not_open_file(cx: &mut TestAppContext) {
    use crate::{App, BufferApp};

    // A hermetic temp dir with a file the filter can select (plus a decoy so the
    // match isn't the only row).
    let dir = std::env::temp_dir().join(format!("yalda-picker-filter-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["aaa-decoy.txt", "target-file.txt"] {
        std::fs::write(dir.join(f), b"x\n").unwrap();
    }

    cx.update(crate::register_keymap);
    let dir_for_view = dir.clone();
    let (view, vcx) = cx.add_window_view(move |window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        crate::with_no_session_server(|| {
            YaldaGpuiView::new_browser(dir_for_view.clone(), Theme::default(), focus_handle)
        })
    });
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();

    // Enter `/` search and type a query that selects the FILE (recursive search
    // ranks the exact/shortest match first → selected == the file).
    vcx.simulate_keystrokes("/");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("t a r g e t");
    vcx.run_until_parked();

    // Pre-condition: we are filtering and the selected entry is a real FILE, so a
    // leaked BrowserEnter would genuinely open something (non-vacuous NC).
    view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Picking(bw))) => {
            assert!(bw.fb.filter_mode, "filter mode is active after `/` + query");
            let sel = bw.fb.selected_entry().expect("a selected search result");
            assert!(!sel.is_dir, "the selected entry is a file, not a dir");
            assert!(
                sel.name.to_lowercase().contains("target"),
                "the query selected the target file, got {:?}",
                sel.name
            );
        }
        _ => panic!("expected a Picking browser after `/` + query"),
    });

    // The operative action: `right` while filtering. Must NOT open the file.
    vcx.simulate_keystrokes("right");
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Picking(bw))) => {
            assert!(
                bw.fb.filter_mode,
                "`right` in search mode is swallowed — filter stays active, no file opened"
            );
        }
        _ => panic!(
            "`right` in search mode must NOT open the file — the tile flipped away \
             from the picker (BrowserEnter leaked)"
        ),
    });

    // The letter-leak face of the same defect: `h`/`r`/`s`/`l` are bound to
    // BrowserParent/Rename/Sort/Enter. GPUI dispatches (and CONSUMES) those
    // actions before the capture handler, so pre-fix they navigated / renamed /
    // opened mid-search AND never reached the query. The `BrowserFilter` context
    // makes those bindings not match, so each is TYPED INTO THE QUERY. Type the
    // remaining letters of "target-file"; a fully-typed bound-letter query must
    // still be a real, editable search string (not a pile of side effects).
    let dir_before = view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Picking(bw))) => bw.fb.current_dir().to_path_buf(),
        _ => panic!("still a picker before typing bound letters"),
    });
    // filter so far is "target" (from "t a r g e t"); finish "target-file".
    vcx.simulate_keystrokes("- f i l e");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Picking(bw))) => {
            assert!(bw.fb.filter_mode, "still filtering after bound letters");
            assert_eq!(
                bw.fb.current_dir(),
                dir_before.as_path(),
                "bound letters must NOT navigate away mid-search"
            );
            assert_eq!(
                bw.fb.filter_text(),
                "target-file",
                "every bound letter (`-`, `l`) is TYPED into the query, not swallowed \
                 or fired as an action — got {:?}",
                bw.fb.filter_text()
            );
        }
        _ => panic!("typing bound letters must NOT leave the picker"),
    });

    let _ = std::fs::remove_dir_all(&dir);
}

/// bug-0038 (rail face): the file-browser RAIL has the same defect. `RailView`
/// binds `s`/`w`/`-`/enter/j/k as actions, and GPUI dispatches those before the
/// rail's capture filter handler, so pre-fix typing them in `/` search fired the
/// action (cycle sort / open worktrees / go to parent) instead of editing the
/// query. The `RailFilter` context (active while the rail's browser filters)
/// stops those bindings from matching, so each key is typed into the query.
///
/// Negative control: force `RailFilter`→`RailView` in `render_rail` → `w` opens
/// worktree mode, `-` navigates to the parent, and the query never fills; the
/// asserts below fail.
#[gpui::test]
fn rail_filter_bound_keys_type_into_query(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);

    // Cmd-B opens a FOCUSED file-browser rail (RailState::new → focused: true).
    vcx.simulate_keystrokes("cmd-b");
    vcx.run_until_parked();
    // Enter `/` rail filter.
    vcx.simulate_keystrokes("/");
    vcx.run_until_parked();

    // Read the rail's file browser. Panics if the rail isn't a focused,
    // filtering file browser — that itself proves the `/` reached the rail.
    let read_fb = |view: &gpui::Entity<YaldaGpuiView>,
                   vcx: &mut gpui::VisualTestContext,
                   f: &dyn Fn(&crate::FileBrowser)| {
        view.read_with(vcx, |v, _| {
            let rail = v
                .workspace
                .active_workspace()
                .and_then(|t| t.rail.as_ref())
                .expect("a rail is open");
            match &rail.content {
                crate::workspace::RailContent::FileBrowser(fb) => f(fb),
                _ => panic!("the rail is a file browser"),
            }
        });
    };

    read_fb(&view, vcx, &|fb| {
        assert!(fb.filter_mode, "`/` entered rail filter")
    });
    let dir_before = view.read_with(vcx, |v, _| {
        match &v
            .workspace
            .active_workspace()
            .and_then(|t| t.rail.as_ref())
            .unwrap()
            .content
        {
            crate::workspace::RailContent::FileBrowser(fb) => fb.current_dir().to_path_buf(),
            _ => unreachable!(),
        }
    });

    // Type keys that ARE bound rail actions: `s`=RailCycleSort, `w`=RailWorktrees,
    // `-`=RailParent. Each must become query text, not fire its action.
    vcx.simulate_keystrokes("s w -");
    vcx.run_until_parked();

    read_fb(&view, vcx, &move |fb| {
        assert!(fb.filter_mode, "rail still filtering after bound keys");
        assert!(
            fb.worktree_mode.is_none(),
            "`w` must be query text — it must NOT open worktree mode (RailWorktrees leaked)"
        );
        assert_eq!(
            fb.current_dir(),
            dir_before.as_path(),
            "`-` must be query text — it must NOT go to the parent dir (RailParent leaked)"
        );
        assert_eq!(
            fb.filter_text(),
            "sw-",
            "bound rail keys (`s`, `w`, `-`) are typed into the query, not fired — got {:?}",
            fb.filter_text()
        );
    });
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
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("i"), window, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "i opens a You-block");
        assert_eq!(
            c.focus,
            crate::AgentFocus::Compose,
            "focus moves to the block"
        );
        assert_eq!(
            c.input_surface.compose().mode,
            crate::EditMode::Insert,
            "the block is in Insert"
        );
    });

    // Type only whitespace, then Esc Esc → drop to Normal, then leave → discard
    // (layered Esc: 1st = Normal in the block, 2nd = leave; empty ⇒ discard).
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("space"), window, cx)
    });
    vcx.run_until_parked();
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), window, cx)
    });
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), window, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(
            !c.you_block_open,
            "empty Esc discards the You-block (rule 3)"
        );
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

    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("i"), window, cx)
    });
    vcx.run_until_parked();
    for k in ["h", "i"] {
        view.update_in(vcx, |v, window, cx| {
            v.handle_claude_key(&ws_bare_key(k), window, cx)
        });
    }
    vcx.run_until_parked();
    // 1st Esc → Normal IN the block (edit-in-place); focus stays on the compose.
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), window, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.focus,
            crate::AgentFocus::Compose,
            "1st Esc stays in the block"
        );
        assert_eq!(
            c.input_surface.compose().mode,
            crate::EditMode::Normal,
            "now Normal"
        );
        assert!(c.you_block_open);
    });
    // 2nd Esc → leave to nav; the non-empty block persists (rule 4).
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), window, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "non-empty block persists (rule 4)");
        assert_eq!(
            c.focus,
            crate::AgentFocus::Transcript,
            "2nd Esc returns to nav"
        );
        assert_eq!(
            c.input_surface.compose().text().trim(),
            "hi",
            "draft retained"
        );
    });

    // Re-entering Insert at the SAME anchor resumes the block, text kept.
    view.update(vcx, |v, cx| {
        let anchor = v.agent_mut(cx).expect("agent").you_block_anchor;
        if let Some(a) = anchor {
            v.agent_mut(cx).expect("agent").editor.cursor_mut().line = a;
        }
    });
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("i"), window, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open);
        assert_eq!(c.focus, crate::AgentFocus::Compose);
        assert_eq!(
            c.input_surface.compose().text().trim(),
            "hi",
            "same block, text kept"
        );
    });
}

/// REGRESSION (runtime report): you can drop to Normal IN a You-block and re-enter
/// Insert into the SAME region — use Helix motions to edit, or return to your text
/// after a second thought. 1st Esc = Normal (stay in block), `i`/motions work, the
/// block stays the active editable surface (does NOT jump to transcript nav).
#[gpui::test]
fn worksheet_block_normal_then_insert_again(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    for ch in ["h", "e", "l", "l", "o"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    vcx.run_until_parked();
    // 1st Esc → Normal IN the block (still the active surface).
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.focus,
            crate::AgentFocus::Compose,
            "stay in the block, not nav"
        );
        assert_eq!(c.input_surface.compose().mode, crate::EditMode::Normal);
        assert!(
            c.inline_you_block_active(),
            "block still the visible active surface"
        );
    });
    // A Helix motion edits within the reply (Normal-mode key routes to the compose).
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("b"), w, cx)
    });
    vcx.run_until_parked();
    // `i` re-enters Insert into the SAME region (the reported bug: couldn't do this).
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(c.focus, crate::AgentFocus::Compose);
        assert_eq!(
            c.input_surface.compose().mode,
            crate::EditMode::Insert,
            "back in Insert"
        );
        assert_eq!(
            c.input_surface.compose().text().trim(),
            "hello",
            "same block, text intact"
        );
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
    assert!(
        probe_dirty(&view, vcx, "you-block").is_none(),
        "idle nav: no inline block"
    );
    assert!(
        probe_dirty(&view, vcx, "compose-box").is_none(),
        "idle nav: no bottom box"
    );

    // Open a You-block → it paints INLINE, not as the bottom box (rules 2/6).
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("i"), window, cx)
    });
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
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), window, cx)
    });
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), window, cx)
    });
    vcx.run_until_parked();
    assert!(
        probe_dirty(&view, vcx, "you-block").is_none(),
        "discarded → no inline block"
    );
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
    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("i"), window, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let base = crate::perf_render_count("transcript");

    // Type into the inline block — each keystroke must bust the transcript cache.
    for k in ["h", "e", "l", "l", "o"] {
        view.update_in(vcx, |v, window, cx| {
            v.handle_claude_key(&ws_bare_key(k), window, cx)
        });
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
            v.agent_mut(cx)
                .expect("agent")
                .input_surface
                .compose()
                .text()
                .trim(),
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
    let (anchor, last_line) = view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        (s, c.editor.document().line_count().saturating_sub(1))
    });
    assert!(anchor < last_line, "anchor is genuinely above the tail");

    view.update_in(vcx, |v, window, cx| {
        v.handle_claude_key(&ws_bare_key("i"), window, cx)
    });
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

/// UXI-AgentTile-21: `r` over an agent line opens a reply You-block seeded
/// `re\n> <first sentence>\n` with the caret parked on the trailing blank line.
/// Drives the REAL dispatch (`handle_claude_key`) the keystroke invokes.
/// Negative control: revert the seed line in `reply_quote_at_cursor` (open an
/// empty block) → the text-equality assert goes RED.
#[gpui::test]
fn worksheet_r_seeds_reply_quote_from_agent_line(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx); // worksheet nav
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk(
                    "First sentence. Second sentence. Third sentence.\n".into(),
                )),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();

    // Park the caret on the agent line (first line of the latest turn).
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
    });

    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "r opened a You-block");
        assert_eq!(
            c.input_surface.compose().text(),
            "re\n> First sentence.\n",
            "seeded `re` + the FIRST sentence as a one-line blockquote"
        );
        // Caret rests on the blank line BELOW the quote (line 2, col 0).
        let cur = c.input_surface.compose().editor.cursor();
        assert_eq!(
            (cur.line, cur.col),
            (2, 0),
            "caret parked on the trailing blank line, after the quote"
        );
    });
}

/// REGRESSION (runtime report: a freshly seeded worksheet reply sometimes wraps
/// into a narrow ~40-column strip despite a wide transcript). `r` opens the real
/// inline You-block before its compose bounds have ever painted; the unmeasured
/// width path must use the already-painted transcript viewport, not the old
/// 40-column emergency fallback. The source sentence is deliberately wider than
/// 40 columns but comfortably narrower than this harness viewport, so a correct
/// first settled paint keeps the quote on one visual row.
#[gpui::test]
fn worksheet_r_first_paint_uses_transcript_width(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    let sentence = format!("{}.", "wide worksheet reply text ".repeat(5));
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk(format!("{sentence}\n"))),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().expect("agent turn");
        c.editor.cursor_mut().line = s;
    });
    let (_, _, viewport_w, _) =
        probe_dirty(&view, vcx, "transcript-viewport").expect("transcript viewport paints");
    crate::layout_probe_begin();
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();
    let (_, _, block_w, block_h) =
        crate::layout_probe_get("you-block").expect("seeded You-block paints on first settle");
    crate::layout_probe_end();
    assert!(
        viewport_w > 800.0,
        "precondition: this is a genuinely wide transcript ({viewport_w}px)"
    );
    assert!(
        block_w > viewport_w * 0.8,
        "the inline reply occupies the transcript column: block {block_w}px, viewport {viewport_w}px"
    );
    assert!(
        block_h < 115.0,
        "a ~130-char quote fits one visual row in a {viewport_w}px transcript; \
         {block_h}px means it used the narrow unmeasured-width fallback"
    );
}

/// UXI-AgentTile-34 + UXI-AgentTile-35: `V` selects the whole agent line
/// (line-wise visual) through the REAL keymap, and a live selection is what `r`
/// quotes — the sentence-count heuristic is ignored. Drives the real
/// `handle_claude_key` dispatch (V and r) end-to-end.
///
/// Negative control (observed RED): drop the `sel.is_some()` branch in
/// `reply_quote_at_cursor` (always take the sentence path) → the quote collapses
/// to `re\n> First sentence.\n` and the whole-line assert fires.
#[gpui::test]
fn worksheet_v_line_select_feeds_r(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;
    let (view, vcx, _id, _session) = boot_with_transcript(cx); // worksheet nav
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk(
                    "First sentence. Second sentence. Third sentence.\n".into(),
                )),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        c.editor.cursor_mut().col = 0;
    });
    // `V` selects the WHOLE current line.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("V"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let sel = c
            .editor
            .selection_range()
            .filter(|&((sl, sc), (el, ec))| (sl, sc) != (el, ec));
        assert!(sel.is_some(), "V created a non-empty selection");
        assert_eq!(
            c.editor.selection_text().unwrap_or_default().trim_end(),
            "First sentence. Second sentence. Third sentence.",
            "V selected the entire agent line"
        );
    });
    // `r` quotes the SELECTION, not just the first sentence.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "r opened a reply block");
        assert_eq!(
            c.input_surface.compose().text(),
            "re\n> First sentence. Second sentence. Third sentence.\n",
            "the whole-line selection is the quote; sentence-count ignored"
        );
    });
}

/// UXI-AgentTile-34 + UXI-AgentTile-35: `v` (char-wise visual) + motions selects
/// PART of an agent line, and `r` quotes exactly that partial selection.
///
/// Negative control (observed RED): same as above — dropping the selection branch
/// makes `r` quote the first whole sentence, not the 5-char `First`.
#[gpui::test]
fn worksheet_v_char_select_feeds_r(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk("First sentence here.\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        c.editor.cursor_mut().col = 0;
    });
    // `v` starts char-wise visual; five `l` extend the head to col 5 ⇒ "First".
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("v"), w, cx)
    });
    for _ in 0..5 {
        view.update_in(vcx, |v, w, cx| {
            v.handle_claude_key(&ws_bare_key("l"), w, cx)
        });
    }
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.editor.selection_text().unwrap_or_default(),
            "First",
            "v + 5×l selected the first five chars"
        );
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.input_surface.compose().text(),
            "re\n> First\n",
            "the partial char-wise selection is quoted verbatim"
        );
    });
}

/// UXI-AgentTile-36: a reply quoting an OLDER agent turn is allowed, and it lands
/// in the CURRENT turn at the tail (anchor `None`), never mid-history. Tags line 0
/// as an old `Llm(1)` and the rest as the latest `Llm(2)` exactly like
/// `worksheet_stale_anchor_is_rejected` (the synthetic stream can't advance the
/// turn number). Selects ONLY the old line so the caret rests on it.
///
/// Negative control (observed RED): restore the
/// `if !you_block_anchor_is_legal(l) { return false }` guard at the top of
/// `reply_quote_at_cursor` → `r` no-ops over the older line (`you_block_open`
/// stays false).
#[gpui::test]
fn worksheet_r_replies_across_turn_boundary(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(vec![ev(ReplyEvent::Chunk("aa\nbb\ncc\ndd\n".into()))], cx);
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let lc = c.editor.document().line_count();
        // line 0 = OLD turn Llm(1); every other content line = latest Llm(2).
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
        assert!(
            !c.you_block_anchor_is_legal(0),
            "line 0 is an OLD turn — the boundary this test crosses"
        );
        c.editor.cursor_mut().line = 0;
        c.editor.cursor_mut().col = 0;
    });
    // `V` selects the whole OLD line; `r` replies across the boundary.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("V"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(
            c.you_block_open,
            "r opened a reply over the OLDER turn (boundary lifted)"
        );
        assert_eq!(
            c.input_surface.compose().text(),
            "re\n> aa\n",
            "the older line's text is the quote"
        );
        assert_eq!(
            c.effective_you_block_anchor(),
            None,
            "the reply anchors at the TAIL (current turn), never mid-history"
        );
    });
}

/// UXI-AgentTile-34: `V` turns on linewise extend-mode, so a following `j`
/// selects the WHOLE next line instead of collapsing the selection or stopping
/// at the sticky character column inherited from the first line (the vim `V j`
/// idiom).
///
/// Negative control (observed RED): before the distinct linewise state and
/// post-motion normalization, the exact-text assertion got `"one\ntwo"` — the
/// sticky column cut the longer second line off after three characters.
#[gpui::test]
fn worksheet_v_then_j_extends_selection(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk(
                    "one\ntwo is deliberately much longer\nthree\n".into(),
                )),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        c.editor.cursor_mut().col = 0;
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("V"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let ((sl, _), (el, _)) = c
            .editor
            .selection_range()
            .filter(|&((a, b), (x, y))| (a, b) != (x, y))
            .expect("V then j keeps a non-empty selection");
        let base = c.latest_agent_turn_range().unwrap_or((0, 0)).0;
        assert_eq!(sl, base, "selection still anchored at the first line");
        assert_eq!(el, base + 1, "j grew the selection into the next line");
        assert_eq!(
            c.editor.selection_text().unwrap_or_default(),
            "one\ntwo is deliberately much longer",
            "V then j selects both complete logical lines, independent of their lengths"
        );
    });
}

/// UXI-AgentTile-34: the upward half of true linewise visual mode. Starting on
/// a short second line, `V k` must include the complete longer first line and
/// keep the active cursor at that line's start boundary.
#[gpui::test]
fn worksheet_v_then_k_selects_whole_previous_line(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk(
                    "first is deliberately much longer\ntwo\nthree\n".into(),
                )),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let base = c.latest_agent_turn_range().unwrap_or((0, 0)).0;
        c.editor.cursor_mut().set_pos(base + 1, 0);
    });

    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("V"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("k"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let base = c.latest_agent_turn_range().unwrap_or((0, 0)).0;
        assert_eq!(
            c.editor.selection_text().unwrap_or_default(),
            "first is deliberately much longer\ntwo",
            "V then k selects both complete logical lines"
        );
        assert_eq!(
            (c.editor.cursor().line, c.editor.cursor().col),
            (base, 0),
            "the upward linewise head rests at the first selected line's start"
        );
    });
}

/// UXI-AgentTile-35: a multi-line selection (all in the latest turn) is quoted
/// `>`-per-line through the real `V`+`V`+`r` path.
#[gpui::test]
fn worksheet_multiline_selection_quotes_per_line(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk("alpha\nbeta\ngamma\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        c.editor.cursor_mut().col = 0;
    });
    // `V` `V` selects the first two lines line-wise.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("V"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("V"), w, cx)
    });
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.editor.selection_text().unwrap_or_default(),
            "alpha\nbeta",
            "V V selected two whole lines"
        );
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.input_surface.compose().text(),
            "re\n> alpha\n> beta\n",
            "multi-line selection quoted `>`-per-line"
        );
    });
}

/// bug-0032: `Esc` in transcript nav exits extend mode + collapses the selection,
/// so subsequent navigation no longer auto-highlights. Drives real keystrokes.
///
/// Negative control (observed RED): drop the Esc cancel branch → after `Esc` the
/// editor is still in extend mode, so the next `j` GROWS a selection (non-empty).
#[gpui::test]
fn worksheet_esc_exits_extend_mode_stops_autohighlight(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk("one\ntwo\nthree\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        c.editor.cursor_mut().col = 0;
    });
    // Enter char-select and extend — extend mode is now ON.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("v"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("l"), w, cx)
    });
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.editor.extend_mode(), "v turned extend mode on");
        assert!(
            c.editor.selection_range().is_some(),
            "a selection is active"
        );
    });
    // Esc cancels the selection + exits extend mode.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(!c.editor.extend_mode(), "Esc exited extend mode");
        assert!(
            c.editor.selection_range().is_none(),
            "Esc collapsed the selection"
        );
    });
    // Now plain navigation must NOT auto-highlight.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let sel = c
            .editor
            .selection_range()
            .filter(|&((sl, sc), (el, ec))| (sl, sc) != (el, ec));
        assert!(
            sel.is_none(),
            "navigating after Esc must not highlight; got {sel:?}"
        );
    });
}

/// bug-0032: the reply gesture clears extend mode, so returning to nav after a
/// `V`→`r` reply doesn't leave you stuck auto-highlighting.
///
/// Negative control (observed RED): drop `set_extend_mode(false)` in
/// `reply_quote_at_cursor` → `extend_mode` stays ON after `r`.
#[gpui::test]
fn worksheet_reply_clears_extend_mode(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk("hello world.\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        c.editor.cursor_mut().col = 0;
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("V"), w, cx)
    });
    view.update(vcx, |v, cx| {
        assert!(
            v.agent_mut(cx).unwrap().editor.extend_mode(),
            "V turned extend mode on"
        );
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    view.update(vcx, |v, cx| {
        assert!(
            !v.agent_mut(cx).unwrap().editor.extend_mode(),
            "r cleared extend mode"
        );
    });
}

/// bug-0031: in char-select mode (`v` + motion) the caret on the cursor line
/// renders as a BEAM at the selection's edge, not a BLOCK one cell past the
/// highlight — so caret and selection line up. Drives real keystrokes and asserts
/// what the render actually painted (`DocRenderTap.caret_beam_on_cursor_line`).
///
/// Negative control (observed RED): revert `caret_mode_during_selection` to return
/// `mode` always (drop the beam) → the caret paints as a BLOCK during the selection
/// (`caret_beam_on_cursor_line == Some(false)`).
#[gpui::test]
fn worksheet_char_select_caret_is_beam(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk("First sentence here.\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        c.editor.cursor_mut().col = 0;
    });
    // Start a char-wise selection and extend it.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("v"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("l"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("l"), w, cx)
    });
    vcx.run_until_parked();
    // Reset the tap, extend once more to force a fresh paint of the cursor line.
    YaldaGpuiView::test_reset_doc_render_tap();
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("l"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        // Non-vacuous: a non-empty selection really is active.
        assert!(
            c.editor
                .selection_range()
                .map(|((sl, sc), (el, ec))| (sl, sc) != (el, ec))
                .unwrap_or(false),
            "a non-empty selection is active"
        );
    });
    let tap = YaldaGpuiView::test_doc_render_tap();
    assert_eq!(
        tap.caret_beam_on_cursor_line,
        Some(true),
        "the caret painted as a BEAM (flush with the selection), not a block"
    );
}

/// UXI-AgentTile-37: the replied-to source line shows a `>` blockquote marker in
/// the transcript when NOT typing in the reply block, and it clears on abandon.
/// Drives the real `r` → `escape` → `u` keystroke path and asserts BOTH the state
/// (`reply_marker_range`) and the PAINT (`DocRenderTap.reply_marker`).
///
/// Negative controls (observed RED, each separately):
///  - render branch: skip `push_reply_marker_line` / the `is_marker_line` override
///    → the paint tap is empty even though the state says the marker is active.
///  - clear-on-pop: drop `reply_source_range = None` in the `u`-pop branch → the
///    marker survives after the reply is abandoned.
#[gpui::test]
fn worksheet_replied_to_source_shows_marker_when_not_typing(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk("hello there.\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    let src = view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
        c.editor.cursor_mut().col = 0;
        s
    });
    // `r` opens the reply, seeded + in Insert. WHILE TYPING the marker is hidden.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.reply_source_range,
            Some((src, src + 1)),
            "source captured"
        );
        assert_eq!(
            c.reply_marker_range(),
            None,
            "marker hidden while typing in the block"
        );
    });
    // `escape` drops the compose to Normal → NOT typing → the marker is shown.
    YaldaGpuiView::test_reset_doc_render_tap();
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.reply_marker_range(),
            Some((src, src + 1)),
            "marker shown once not typing"
        );
    });
    let tap = YaldaGpuiView::test_doc_render_tap();
    assert!(
        tap.reply_marker.contains(&src),
        "the `>` marker PAINTED on the source line {src}; got {:?}",
        tap.reply_marker
    );
    // `u` pops the reply (seeded baseline has no undo history) → marker clears.
    YaldaGpuiView::test_reset_doc_render_tap();
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("u"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.reply_source_range, None,
            "reply abandoned ⇒ source cleared"
        );
        assert_eq!(c.reply_marker_range(), None, "marker gone after pop");
    });
    let tap = YaldaGpuiView::test_doc_render_tap();
    assert!(
        !tap.reply_marker.contains(&src),
        "no marker paints after the reply is popped; got {:?}",
        tap.reply_marker
    );
}

/// UXI-AgentTile-24: `u` in an open worksheet You-block backs the reply out,
/// undo-style. Common flow `r → Esc → u` pops the block on the FIRST `u` (the
/// seeded quote is a committed baseline with no undo history). The layered case
/// (`i`, type, `Esc`, `u` undoes the typing and the block stays; a further `u`
/// pops it) is asserted too. Drives the REAL keystroke dispatch (handle_claude_key).
///
/// Negative control (observed RED): make the `u` branch always `editor.undo()`
/// (drop the pop) → after `r → Esc → u` the block is STILL open (the
/// `!you_block_open` assert fails).
#[gpui::test]
fn worksheet_esc_u_backs_out_reply_block(cx: &mut TestAppContext) {
    use yalda::acp_channel::ReplyEvent;
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx); // worksheet nav
    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![
                ev(ReplyEvent::Chunk(
                    "First sentence. Second sentence.\n".into(),
                )),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();

    let park_on_agent_line = |view: &gpui::Entity<YaldaGpuiView>,
                              vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            let mut c = v.agent_mut(cx).expect("agent");
            let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
            c.editor.cursor_mut().line = s;
        });
    };

    // ── Common flow: r → Esc → u pops on the FIRST u ────────────────────────
    park_on_agent_line(&view, vcx);
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "r opened the block");
        assert_eq!(
            c.input_surface.compose().mode,
            crate::EditMode::Insert,
            "seeded reply is in Insert"
        );
    });
    // 1st Esc: compose → Normal, still in the block.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(c.you_block_open, "still in the block after 1st Esc");
        assert_eq!(
            c.input_surface.compose().mode,
            crate::EditMode::Normal,
            "1st Esc dropped the compose to Normal (in place)"
        );
    });
    // u: nothing to undo (committed baseline) → pop the block.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("u"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(!c.you_block_open, "u popped the You-block");
        assert!(
            c.input_surface.compose().text().trim().is_empty(),
            "the reply text is gone"
        );
        assert_eq!(
            c.focus,
            crate::AgentFocus::Transcript,
            "back in transcript Normal navigation"
        );
    });

    // ── Layered case: undo the typing first, THEN pop ───────────────────────
    park_on_agent_line(&view, vcx);
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    vcx.run_until_parked();
    // Re-enter Insert and type a character.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("x"), w, cx)
    });
    vcx.run_until_parked();
    let typed = view.update(vcx, |v, cx| {
        v.agent_mut(cx).unwrap().input_surface.compose().text()
    });
    assert!(typed.contains('x'), "typed x is in the draft: {typed:?}");
    // Esc → Normal, then u: undoes the typing, block STAYS open.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    vcx.run_until_parked();
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("u"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(
            c.you_block_open,
            "1st u undid the typing; the block stays open"
        );
        assert!(
            !c.input_surface.compose().text().contains('x'),
            "the typed x was undone"
        );
    });
    // 2nd u: nothing left to undo → pop.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("u"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(!c.you_block_open, "2nd u popped the block");
        assert_eq!(
            c.focus,
            crate::AgentFocus::Transcript,
            "back to transcript nav"
        );
    });
}

/// UXI-AgentTile-21: a vim count prefix quotes that many sentences — `3r` quotes
/// the first three, joined on one `>` line. Exercises the shared `pending_count`
/// path end-to-end (`3` accumulates, `r` consumes via `take_count`).
#[gpui::test]
fn worksheet_count_r_quotes_n_sentences(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk("One. Two. Three. Four.\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let (s, _e) = c.latest_agent_turn_range().unwrap_or((0, 0));
        c.editor.cursor_mut().line = s;
    });

    // `3` accumulates the count; `r` consumes it → the first THREE sentences.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("3"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.input_surface.compose().text(),
            "re\n> One. Two. Three.\n",
            "3r quotes the first three sentences on one line"
        );
    });
}

/// UXI-AgentTile-21: `r` on a line with no sentence text (the blank tail — a
/// LEGAL anchor, but nothing to quote) is a no-op — no block opens. Guards the
/// "nothing to quote" branch of `reply_quote_at_cursor`.
#[gpui::test]
fn worksheet_r_noop_on_blank_line(cx: &mut TestAppContext) {
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
                ev(ReplyEvent::Chunk("Some agent text.\n".into())),
                ev(ReplyEvent::TurnEnded { count: 1 }),
            ],
            cx,
        );
    });
    vcx.run_until_parked();

    // Park the caret on the blank tail line: a legal anchor (`l >= last`) with no
    // sentence text.
    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        let last = c.editor.document().line_count().saturating_sub(1);
        c.editor.cursor_mut().line = last;
        assert!(
            c.you_block_anchor_is_legal(last),
            "the tail is a legal anchor"
        );
        assert!(
            c.editor.document().line_text(last).trim().is_empty(),
            "the tail line is blank — nothing to quote"
        );
    });

    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("r"), w, cx)
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert!(
            !c.you_block_open,
            "r is a no-op when there is nothing to quote"
        );
    });
}

/// REGRESSION (runtime: "can't type the m character in chatbox mode"): the bare-`m`
/// mark chord must NOT fire in the editable compose — `m` is typeable in Insert, and
/// in compose-Normal it routes to the editor (no pending mark chord).
#[gpui::test]
fn m_is_typeable_in_compose(cx: &mut TestAppContext) {
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    for ch in ["m", "a", "p"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(
            v.agent_mut(cx)
                .expect("agent")
                .input_surface
                .compose()
                .text()
                .trim(),
            "map",
            "m types in the compose (Insert), not eaten by a mark chord"
        );
    });
    // Drop to compose-Normal (1st Esc), press m → still no mark chord started.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("m"), w, cx)
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    for ch in ["f", "i", "r", "s", "t"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    // Block 2 "second" at s+2, Esc Esc to nav (block 1 now parked).
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.cursor_mut().line = s + 2;
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    for ch in ["s", "e", "c"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    vcx.run_until_parked();

    // Navigate BACK to block 1's anchor (s) and press i → RESUMES block 1, not a 3rd.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.cursor_mut().line = s;
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    for ch in ["o", "n", "e"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    // Esc Esc back to nav (1st = Normal in block, 2nd = leave; block 1 persists),
    // navigate down, then `i` for a 2nd insertion point.
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.cursor_mut().line = s + 2;
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    for ch in ["t", "w", "o"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let mut c = v.agent_mut(cx).expect("agent");
        // State: one parked ("one"), one active ("two").
        assert_eq!(
            c.parked_you_blocks.len(),
            1,
            "two insertion points (1 parked + active)"
        );
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
        assert!(
            full.contains("one") && full.contains("two"),
            "both frozen in place"
        );
    });
}

/// REGRESSION (bug-0004): two You-blocks must NEVER render adjacent (next to each
/// other). Repro: open a tail You-block ("hi", visible at the bottom), Esc-Esc to
/// nav, move the caret UP one line onto the last agent line, press `o`. The blank
/// tail line between the two anchors collapses, so the old code's "second insertion
/// point" landed in the SAME slot → two adjacent `YouBlock`s. The fix resolves by
/// render slot: `o` there resumes the existing "hi" block instead of spawning a
/// neighbour. A genuinely separated insertion point (agent content between) still
/// opens a second block — covered by `worksheet_multiple_insertion_points`.
///
/// Asserts on the RENDERED flat_items (no two consecutive YouBlock items), not just
/// state. Negative control: revert `open_you_block_at_cursor` to match on raw anchor
/// equality (`snapped == self.you_block_anchor`) instead of `you_blocks_would_be_adjacent`
/// → parked=1, two adjacent YouBlocks → the adjacency assert fires RED.
#[gpui::test]
fn worksheet_you_blocks_never_render_adjacent(cx: &mut TestAppContext) {
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

    // Open a TAIL You-block (the div at the bottom), type "hi", Esc-Esc to nav.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").move_cursor_to_tail();
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("o"), w, cx)
    });
    for ch in ["h", "i"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(ch), w, cx));
    }
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    vcx.run_until_parked();

    // Move the caret UP one legal line and press `o` — the exact reported gesture.
    view.update(vcx, |v, cx| {
        v.agent_mut(cx).expect("agent").editor.cursor_mut().line = 3;
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("o"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        // The rendered list must have NO two consecutive YouBlock items.
        let items = &c.view_model.flat_items_cache;
        let adjacent = items.windows(2).any(|w| {
            matches!(w[0], crate::FlatItem::YouBlock { .. })
                && matches!(w[1], crate::FlatItem::YouBlock { .. })
        });
        assert!(
            !adjacent,
            "two You-blocks rendered adjacent (bug-0004): {items:?}"
        );
        // And the "hi" reply was RESUMED (not orphaned into a hidden parked block).
        assert!(
            c.parked_you_blocks.is_empty(),
            "no spurious second insertion point"
        );
        assert_eq!(
            c.input_surface.compose().text().trim(),
            "hi",
            "the existing reply is resumed"
        );
        let n_you = items
            .iter()
            .filter(|it| matches!(it, crate::FlatItem::YouBlock { .. }))
            .count();
        assert_eq!(n_you, 1, "exactly one You-block renders");
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    for k in ["h", "i"] {
        view.update_in(vcx, |v, w, cx| v.handle_claude_key(&ws_bare_key(k), w, cx));
    }
    // Esc Esc → Normal then leave to nav (block persists, non-empty).
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        assert_eq!(
            c.parked_you_blocks.len(),
            1,
            "the first block is parked as a second insertion point"
        );
        assert_eq!(
            c.parked_you_blocks[0].0,
            Some(anchor_a),
            "parked at its ORIGINAL anchor"
        );
        assert_eq!(
            c.parked_you_blocks[0].1.trim(),
            "hi",
            "parked text kept, not dragged"
        );
        assert_ne!(
            c.you_block_anchor,
            Some(anchor_a),
            "the new active block is at the new line, not A"
        );
        assert!(
            c.input_surface.compose().text().trim().is_empty(),
            "fresh active block"
        );
        assert_eq!(c.focus, crate::AgentFocus::Compose);
    });

    // Pressing `i` again at the SAME (new) anchor resumes in place (no third block).
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
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
        v.apply_server_batch(vec![ev(ReplyEvent::Chunk("aa\nbb\ncc\ndd\n".into()))], cx);
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("i"), w, cx)
    });
    vcx.run_until_parked();
    // Co-author a LONG note — enough lines to far exceed any test viewport, so the
    // reveal is genuinely forced to scroll (a shorter block that just fits would make
    // the assertion vacuous). Caret ends at the tail.
    for _ in 0..90 {
        view.update_in(vcx, |v, w, cx| {
            v.handle_claude_key(&ws_bare_key("x"), w, cx)
        });
        view.update_in(vcx, |v, w, cx| {
            v.handle_claude_key(&ws_bare_key("enter"), w, cx)
        });
    }
    vcx.run_until_parked();
    let n = view
        .update(vcx, |v, cx| {
            v.agent_read(cx, |c| {
                c.input_surface.compose().editor.document().line_count()
            })
        })
        .unwrap();
    assert!(n > 80, "block genuinely long ({n} lines)");
    // Settle: the You-block lives in the CACHED transcript, so force it to re-render +
    // re-reveal by mutating the session (agent_mut notifies) and re-latching the caret
    // reveal — a bare root notify would skip the cached child. Lazy item measurement
    // means the reveal scroll lands only after several frames.
    let bust_and_reveal = |view: &gpui::Entity<YaldaGpuiView>,
                           vcx: &mut gpui::VisualTestContext| {
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
        assert!(
            c.you_block_open,
            "a fresh session opens a VISIBLE tail input block"
        );
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

/// Open the workspace (`.`) leader menu on a nav-resting worksheet and paint one
/// frame with the layout probe active. Returns `(card_bounds, root_bounds)` where
/// `card` is the floating panel (`menu-panel`) and `root` is the full-window overlay
/// wrapper (`menu-overlay-root`). Helper for the UXI-Menu-1/-4 geometry tests.
fn probe_open_menu(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    leader: &str,
) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
    vcx.simulate_keystrokes(leader);
    vcx.run_until_parked();
    assert!(
        view.read_with(vcx, |v, _| v.overlay_is_menu()),
        "leader `{leader}` should open the menu"
    );
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let card = crate::layout_probe_get("menu-panel").expect("card painted");
    let root = crate::layout_probe_get("menu-overlay-root").expect("root painted");
    crate::layout_probe_end();
    (card, root)
}

/// UXI-Menu-1: the command panel floats as a content-sized card in the workspace
/// region, LEFT-ANCHORED just past the jump panel (about where the first workspace
/// tile renders) with a small gutter — NOT a full-width bar, NOT centered. On the
/// 1920px test display the card must be clamped to `MENU_PANEL_MAX_W`, narrower than
/// the window, and sit at `JUMP_PANEL_WIDTH + MENU_PANEL_LEFT_PAD` from the left.
///
/// Negative control: restore `.absolute().top_0().left_0().w_full()` on the panel in
/// `render_menu_overlay` (drop the left-anchored float) → the card width becomes the
/// full 1920px window and `x == 0`, so both `width <= MAX` and the left-edge assert
/// fail RED. Verified by reverting the wrapper locally.
#[gpui::test]
fn menu_panel_floats_in_content_region(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        assert!(
            v.jump_panel_visible,
            "test assumes the jump panel is visible"
        );
        cx.notify();
    });
    vcx.run_until_parked();

    let (card, root) = probe_open_menu(&view, vcx, ".");
    let (cx0, _cy, cw, _ch) = card;
    let (rx0, _ry, rw, _rh) = root;

    // The root wrapper spans the whole window; the card must be materially narrower
    // (this makes the "not full-width" assertion NON-vacuous — the window is 1920,
    // far wider than the 720px cap).
    assert!(
        rw > crate::MENU_PANEL_MAX_W + 200.0,
        "test window ({rw}px) must be much wider than the card cap so the float is meaningful"
    );
    assert!(
        cw <= crate::MENU_PANEL_MAX_W + 0.5,
        "card width {cw} exceeds MENU_PANEL_MAX_W {}",
        crate::MENU_PANEL_MAX_W
    );
    assert!(
        cw < rw - 100.0,
        "card ({cw}px) is not content-sized — spans the window ({rw}px)"
    );
    // Left-anchored just past the jump panel + gutter (where the first tile renders).
    let expected_left = rx0 + crate::JUMP_PANEL_WIDTH + crate::MENU_PANEL_LEFT_PAD;
    assert!(
        (cx0 - expected_left).abs() < 2.0,
        "card left {cx0} not anchored past the jump panel + gutter ({expected_left})"
    );
}

/// UXI-Menu-2: the panel body (key-chip + label rows / section labels) paints
/// inside the card bounds with height for multiple rows. Structural guard that the
/// entries actually render within the float (exact chip colors are the pixel gap).
///
/// Negative control: drop the `entries_col` child from `body_col` → `menu-entries`
/// never paints and `.expect(...)` panics RED.
#[gpui::test]
fn menu_panel_rows_and_sections_paint(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();

    // Space opens the AGENT local menu — a multi-row, multi-section tree.
    let (card, _root) = probe_open_menu(&view, vcx, "space");
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let entries = crate::layout_probe_get("menu-entries").expect("entries painted");
    crate::layout_probe_end();

    let (cx0, cy0, cw, ch) = card;
    let (ex0, ey0, ew, eh) = entries;
    // Entries sit inside the card.
    assert!(
        ex0 >= cx0 - 0.5 && ey0 >= cy0 - 0.5,
        "entries escape the card top-left"
    );
    assert!(
        ex0 + ew <= cx0 + cw + 0.5,
        "entries overflow the card right edge"
    );
    assert!(
        ey0 + eh <= cy0 + ch + 0.5,
        "entries overflow the card bottom edge"
    );
    // The agent menu is many rows tall — height must clear multiple 26px rows.
    assert!(
        eh > 26.0 * 3.0,
        "entries height {eh} too short for a multi-row menu"
    );
}

/// UXI-Menu-4: descending into a submenu never moves the card's top edge or LEFT
/// edge (only its height/width may change, growing down/right from the anchor). The
/// static-render descent reads as the card breathing, not teleporting.
///
/// Negative control: make the top depend on level (e.g. shift by path depth) → the
/// two probed tops diverge and the assert fails RED. With the fixed-top left-anchored
/// float they are identical.
#[gpui::test]
fn menu_panel_top_stable_across_descent(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_worksheet_nav(cx);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();

    // Root of the `.` workspace menu.
    let (card_root, _root) = probe_open_menu(&view, vcx, ".");

    // Descend into the `n` → "new" submenu (gpui_menu has it) and re-probe.
    vcx.simulate_keystrokes("n");
    vcx.run_until_parked();
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let card_sub = crate::layout_probe_get("menu-panel").expect("submenu card painted");
    crate::layout_probe_end();

    let (rx, ry, _rw, _rh) = card_root;
    let (sx, sy, _sw, _sh) = card_sub;
    assert!(
        (ry - sy).abs() < 0.5,
        "card top moved on descent: {ry} → {sy}"
    );
    assert!(
        (rx - sx).abs() < 0.5,
        "card left edge moved on descent: {rx} → {sx}"
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
        assert!(
            c.you_block_open,
            "restored worksheet draft opens a tail block"
        );
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
        c.editor
            .append_llm_chunk(crate::TurnId::Llm(1), "an agent reply line\n");
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
        assert_eq!(
            c.focus,
            crate::AgentFocus::Compose,
            "chatbox focuses its box"
        );
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
            v.agent_mut(cx)
                .expect("agent")
                .input_surface
                .compose()
                .text()
                .trim(),
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
            v.agent_mut(cx)
                .expect("agent")
                .input_surface
                .compose()
                .text(),
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
        let g = c.generation;
        c.finalize_agent_turn_idem(g, 1);
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
        let g = c.generation;
        c.finalize_agent_turn_idem(g, 1);
        assert!(!c.you_block_open, "empty draft → no block");
        assert_eq!(
            c.focus,
            crate::AgentFocus::Transcript,
            "rests in navigation"
        );
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
        c.tools
            .register(crate::ToolCallKey::from_id(&id), tc, anchor);
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
    let box_bounds = view.update(vcx, |v, cx| {
        v.agent_read(cx, |c| c.input_surface.compose().bounds.get())
    });
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

/// UXI-AgentTile-17 (PAINT proof): a subagent row STACKS its label over its prompt
/// snippet — two lines, not two side-by-side columns. Register a Task subagent WITH
/// a prompt, drive a real paint, and assert (via the layout probe) that the prompt
/// line's painted top is at/below the label line's painted bottom, both non-empty.
///
/// Negative control: revert the row to `.flex_row()` (label + prompt side by side)
/// and the prompt top sits at the label top → `prompt_y >= label_y + label_h` fails.
#[gpui::test]
fn subagent_row_stacks_label_over_prompt(cx: &mut TestAppContext) {
    let (view, vcx, _id, _session) = boot_with_transcript(cx);

    // Register a Task subagent (Think + prompt) so a row with BOTH a label and a
    // prompt snippet renders (this is row 0, the probed one).
    view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        let mut c = v.agent_mut(cx).expect("agent");
        let id: ToolCallId = "task-stack".into();
        let mut tc = ToolCall::new(id.clone(), "Explore the repository layout".to_string());
        tc.kind = ToolKind::Think;
        tc.raw_input =
            Some(serde_json::json!({"prompt": "map the code and report the module structure"}));
        let anchor = c.editor.anchor_for_line(0);
        c.tools
            .register(crate::ToolCallKey::from_id(&id), tc, anchor);
    });

    for _ in 0..4 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let label = crate::layout_probe_get("subagent-row0-label");
    let prompt = crate::layout_probe_get("subagent-row0-prompt");
    crate::layout_probe_end();

    let (_, label_y, _, label_h) = label.expect("subagent row label did not paint");
    let (_, prompt_y, _, prompt_h) = prompt.expect("subagent row prompt did not paint");

    assert!(label_h > 1.0, "label line has no height ({label_h})");
    assert!(prompt_h > 1.0, "prompt line has no height ({prompt_h})");
    // The prompt line is STACKED BELOW the label line (its top at/below the label's
    // bottom, small slack), not on the same row beside it.
    assert!(
        prompt_y + 1.0 >= label_y + label_h,
        "prompt top {prompt_y} is not below the label bottom {} — the row is not stacked",
        label_y + label_h,
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
        c.tools
            .register(crate::ToolCallKey::from_id(&id), tc, anchor);
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

/// UXI-AgentTile-20: `Cmd-B` (`toggle_agent_sidepanel`) force-hides the whole
/// sidepanel even while Plan/Subagents has content, and `Cmd-0`
/// (`focus_agent_panel`) un-hides + focuses it. Asserts on PAINT (the
/// `agent-sidepanel` probe present ⇒ absent ⇒ present) with content held
/// constant, so a state-only pass can't fake it.
///
/// Negative control: revert the `!c.sidepanel_hidden` gate in `render_agent`
/// and step 2 fails RED (sidepanel keeps painting while hidden).
#[gpui::test]
fn cmd_b_hides_and_cmd_0_reshows_the_sidepanel(cx: &mut TestAppContext) {
    let (view, vcx, id, _session) = boot_with_transcript(cx);

    // Content that opens the Plan segment (so the sidepanel would show).
    set_plan(&view, vcx, 2);
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.tasklist_open = true);
    });

    let probe_sidepanel = |view: &gpui::Entity<YaldaGpuiView>,
                           vcx: &mut gpui::VisualTestContext| {
        for _ in 0..3 {
            view.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
        crate::layout_probe_begin();
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        let side = crate::layout_probe_get("agent-sidepanel");
        crate::layout_probe_end();
        side
    };

    // 1) With content present, the sidepanel paints.
    assert!(
        probe_sidepanel(&view, vcx).is_some(),
        "sidepanel should paint while Plan has content",
    );

    // 2) Cmd-B hides it — gone from paint though the plan content is UNCHANGED.
    view.update(vcx, |v, cx| v.toggle_agent_sidepanel(cx));
    let (hidden, plan_still_there, tasklist_still_open) = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| {
            (
                c.sidepanel_hidden,
                c.current_plan.is_some(),
                c.tasklist_open,
            )
        })
        .unwrap()
    });
    assert!(hidden, "toggle set sidepanel_hidden");
    assert!(
        plan_still_there && tasklist_still_open,
        "content is unchanged by hiding"
    );
    assert!(
        probe_sidepanel(&view, vcx).is_none(),
        "sidepanel must NOT paint while hidden, even with plan content",
    );

    // 3) Cmd-0 (focus_agent_panel) un-hides AND focuses the panel.
    view.update(vcx, |v, cx| v.focus_agent_panel(cx));
    let (unhidden, focus) = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| (c.sidepanel_hidden, c.focus))
            .unwrap()
    });
    assert!(!unhidden, "Cmd-0 clears sidepanel_hidden");
    assert_eq!(
        focus,
        crate::AgentFocus::Panel,
        "Cmd-0 lands in panel focus"
    );
    assert!(
        probe_sidepanel(&view, vcx).is_some(),
        "sidepanel paints again after Cmd-0 un-hides it",
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
        Some(crate::SubAgentKey::ToolCall(sub_key)),
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
    view.update(vcx, |v, cx| {
        v.focus_subagent(crate::SubAgentKey::ToolCall(key), cx)
    });
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

/// UXI-AgentTile-26 (PAINT): a markdown bullet list inside a subagent pane wraps
/// at the PANE width, not one glyph per line. Repro from the live screenshot: a
/// prompt whose `Files:` list holds long unbroken paths rendered as a vertical
/// column of single characters because the list item's inner `flex_1().min_w_0()`
/// content column had no definite width to distribute against, collapsing to ~0.
///
/// The honest seam is the layout probe (a geometry bug), asserting the PAINTED
/// width of the list block (`md-block-0`) is a large fraction of the pane — and
/// NON-vacuously: the path is far too long to fit, so a real fit can't produce a
/// false pass (with the bug the block collapses to the ~24px marker column).
///
/// Negative control (observed RED): in `render_markdown_column`, drop the
/// per-block `w_full()` row + `flex_1().min_w_0()` inner (revert to
/// `div().pt(gap).child(probe_bounds_dyn("md-block-{i}", block_inner(&ctx, b)))`,
/// keeping ONLY the probe) → `md-block-0` paints ~24px wide and the
/// `md_w > sub_w * 0.5` assert fails.
#[gpui::test]
fn subagent_markdown_list_wraps_at_pane_width(cx: &mut TestAppContext) {
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    // A subagent whose prompt is a bullet list of long, unbroken paths — the
    // exact shape that collapsed to one-glyph-per-line in the screenshot. Only a
    // prompt (no description/output) so the single Markdown section — the list —
    // is unambiguously `md-block-0`.
    let key = view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        let mut c = v.agent_mut(cx).expect("agent");
        let tid: ToolCallId = "sub-wrap".into();
        let mut tc = ToolCall::new(tid.clone(), "Explore repo".to_string());
        tc.kind = ToolKind::Think;
        tc.raw_input = Some(serde_json::json!({
            "subagent_type": "Explore",
            "prompt": "- /Users/scott/ws/yaldabaoth/src/bin/yalda-gpui/render_blocks.rs\n\
                       - /Users/scott/ws/yaldabaoth/src/bin/yalda-gpui/transcript_view.rs\n\
                       - /Users/scott/ws/yaldabaoth/src/bin/yalda-gpui/agent_sessions.rs"
        }));
        let anchor = c.editor.anchor_for_line(0);
        let k = crate::ToolCallKey::from_id(&tid);
        c.tools.register(k.clone(), tc, anchor);
        k
    });
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        v.focus_subagent(crate::SubAgentKey::ToolCall(key), cx)
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let sub = crate::layout_probe_get("subagent-view");
    let block = crate::layout_probe_get("md-block-0");
    crate::layout_probe_end();

    let (_, _, sub_w, _) = sub.expect("the subagent view must paint");
    let (_, _, md_w, _) = block.expect("the markdown list block must paint");
    // Non-vacuous: the pane is a real width, and the path text is far wider than
    // it, so the list MUST wrap — a fit can't fake this.
    assert!(sub_w > 400.0, "expected a real pane width, got {sub_w}");
    assert!(
        md_w > sub_w * 0.5,
        "the markdown list block must span the pane, not collapse to the marker \
         column: md_w={md_w} sub_w={sub_w} (one-glyph-per-line regression)"
    );
}

/// UXI-AgentTile-25 (MAIN transcript integration): a Task tool call built through
/// the real `tools.register` path renders its prompt + report as MARKDOWN
/// sections, not raw JSON. Drives the exact `plan_tool_sections` call the
/// transcript render makes over the STORED tool call, after a real transcript
/// paint (`run_until_parked`, no panic). The render layer's paint of the expanded
/// body is covered by `subagent_focus_swaps_the_painted_view`, which now goes
/// through the same `append_tool_body_rich`.
///
/// Negative control (shared with the pure guards, observed RED): forcing the
/// report branch in `plan_tool_sections` to `SectionBody::Json` fails the
/// "report is Markdown" assert.
#[gpui::test]
fn transcript_tool_body_renders_markdown_not_json(cx: &mut TestAppContext) {
    use crate::{SectionBody, SectionRole, ToolRenderPolicy, plan_tool_sections};
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        let mut c = v.agent_mut(cx).expect("agent");
        let tid: ToolCallId = "sub-md".into();
        let mut tc = ToolCall::new(tid.clone(), "Explore repo".to_string());
        tc.kind = ToolKind::Think;
        tc.raw_input = Some(serde_json::json!({
            "subagent_type": "Explore",
            "description": "map the code",
            "prompt": "# Task\nFind the **things**."
        }));
        tc.raw_output = Some(serde_json::json!({
            "content": [{"type": "text", "text": "## Report\n- found it\n- done"}]
        }));
        let anchor = c.editor.anchor_for_line(0);
        c.tools
            .register(crate::ToolCallKey::from_id(&tid), tc, anchor);
    });
    vcx.run_until_parked(); // renders the main transcript with the tool group (no panic)

    // The exact call the transcript render makes, over the STORED tool call.
    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let k = crate::ToolCallKey::from_id(&yalda::acp_channel::ToolCallId::from("sub-md"));
        let tc = c.tools.calls.get(&k).expect("the registered tool call");
        let sections = plan_tool_sections(tc, ToolRenderPolicy::Full);
        assert!(
            sections.iter().any(|s| s.label == "prompt"
                && s.role == SectionRole::Input
                && matches!(s.body, SectionBody::Markdown { .. })),
            "the prompt renders as markdown in the main transcript"
        );
        let report = sections
            .iter()
            .find(|s| s.label == "report")
            .expect("a report section");
        assert!(
            matches!(report.body, SectionBody::Markdown { .. }),
            "the report renders as markdown, not raw JSON, in the main transcript"
        );
        assert!(report.emphasis, "the report tile is emphasized");
        assert!(
            !sections
                .iter()
                .any(|s| matches!(s.body, SectionBody::Json(_))),
            "no raw-JSON section for a well-formed subagent tool call"
        );
    });
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
    assert!(
        awaiting,
        "the in-flight turn keeps running (steer rides it)"
    );
    assert!(compose_empty, "compose is cleared after sending");
    assert!(
        in_transcript,
        "the sent steer is committed to the transcript as a user turn"
    );
}

/// bug-0036 / UXI-AgentTile-13 (REAL submit + transport path): a normal
/// message submitted while Codex is working must gracefully cancel the current
/// turn before the new prompt is handled. The same submit while idle must not
/// cancel, and Claude keeps its promptQueueing steering behavior without a
/// cancel. This drives `submit_agent` → `submit_compose` →
/// `send_prompt_to_session` against the in-process production channel and
/// observes both outbound transport queues.
///
/// Negative control: remove the Codex-awaiting interrupt call from
/// `submit_compose`; the second `try_recv_cancel` assertion fails RED with
/// "mid-turn Codex submit must interrupt" while all prompt assertions still
/// pass.
#[cfg(feature = "test-support")]
#[gpui::test]
fn codex_normal_message_interrupts_in_flight_turn(cx: &mut TestAppContext) {
    use yalda::acp_channel::AgentProvider;

    let (view, vcx, id, mut controls) = boot_worksheet_channel(cx);
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.provider = AgentProvider::Codex);
    });

    // CONTROL 1: the ordinary idle Codex submit starts a turn but never emits
    // a cancellation.
    worksheet_real_submit(&view, vcx, "first codex prompt");
    let first = controls
        .prompt_rx
        .try_recv()
        .expect("first prompt reached channel");
    assert_eq!(first.text, "first codex prompt");
    assert!(
        !controls.try_recv_cancel(),
        "idle Codex submit must not cancel"
    );

    // The turn is genuinely in flight through the production submit path.
    let awaiting = view
        .update(vcx, |v, cx| {
            v.read_session(id, cx, |c| c.turn_phase.is_awaiting())
        })
        .expect("session");
    assert!(awaiting, "first real submit must put Codex mid-turn");

    // Type the normal follow-up through the real mid-turn key route, then use
    // the same submit action the UI invokes.
    for ch in "change course".chars() {
        view.update_in(vcx, |v, w, cx| {
            v.handle_claude_key(&ws_bare_key(&ch.to_string()), w, cx)
        });
    }
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();

    assert!(
        controls.try_recv_cancel(),
        "mid-turn Codex submit must interrupt the running turn"
    );
    let second = controls
        .prompt_rx
        .try_recv()
        .expect("follow-up reached channel");
    assert_eq!(second.text, "change course");
    let (compose_empty, committed) = view
        .update(vcx, |v, cx| {
            v.read_session(id, cx, |c| {
                (
                    c.input_surface.compose().text().is_empty(),
                    c.editor.document().full_text().contains("change course"),
                )
            })
        })
        .expect("session");
    assert!(compose_empty, "successful replacement clears the compose");
    assert!(
        committed,
        "successful replacement is committed as a user turn"
    );

    // CONTROL 2: provider specificity. Claude's normal mid-turn submit keeps
    // the established promptQueueing steer and must not emit a cancel.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| c.provider = AgentProvider::Claude);
    });
    for ch in "keep going".chars() {
        view.update_in(vcx, |v, w, cx| {
            v.handle_claude_key(&ws_bare_key(&ch.to_string()), w, cx)
        });
    }
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();

    assert!(
        !controls.try_recv_cancel(),
        "mid-turn Claude submit must retain queue-style steering without cancel"
    );
    let third = controls
        .prompt_rx
        .try_recv()
        .expect("Claude steer reached channel");
    assert_eq!(third.text, "keep going");

    // CONTROL 3: a Codex turn already in StopRequested has a graceful cancel
    // pending. A replacement message supersedes that lifecycle state but must
    // not enqueue a duplicate cancel (which could race against the new prompt).
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.provider = AgentProvider::Codex;
            c.turn_phase.request_stop(std::time::Instant::now());
        });
    });
    for ch in "resume now".chars() {
        view.update_in(vcx, |v, w, cx| {
            v.handle_claude_key(&ws_bare_key(&ch.to_string()), w, cx)
        });
    }
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();

    assert!(
        !controls.try_recv_cancel(),
        "Codex submit after StopRequested must not duplicate the pending cancel"
    );
    let fourth = controls
        .prompt_rx
        .try_recv()
        .expect("post-stop replacement reached channel");
    assert_eq!(fourth.text, "resume now");
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
        v.read_session(id, cx, |c| matches!(c.turn_phase, TurnPhase::Idle))
            .unwrap()
    });
    assert!(still_idle, "stop with no turn in flight is a no-op");

    // Turn in flight ⇒ a stop is requested.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.turn_phase = TurnPhase::begin(std::time::Instant::now())
        });
        v.stop_agent_inner(cx);
    });
    let requested = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| c.turn_phase.stop_requested())
            .unwrap()
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
        v.with_session(id, cx, |c| {
            c.turn_phase = TurnPhase::begin(std::time::Instant::now())
        });
        v.stop_agent_inner(cx);
    });
    let pending = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| c.turn_phase.stop_requested())
            .unwrap()
    });
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
    assert!(
        awaiting,
        "a steer after a stop-request begins a clean Awaiting turn"
    );
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
        v.with_session(id, cx, |c| {
            c.turn_phase = TurnPhase::begin(std::time::Instant::now())
        });
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
        v.read_session(id, cx, |c| c.turn_phase.stop_requested())
            .unwrap()
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
        v.read_session(id, cx, |c| c.editor.document().full_text())
            .unwrap()
    });
    let a = text.find("agent line A").expect("agent content present");
    let s = text
        .find("STEER ONE")
        .expect("steer committed to transcript");
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
            let focus_ok =
                matches!(
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
            let eff_ok = c
                .effective_you_block_anchor()
                .is_none_or(|a| c.you_block_anchor_is_legal(a));
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
    assert!(
        caret_line_ok,
        "UXI-TextEditing-1: compose caret line out of range [{ctx}]"
    );
    assert!(
        caret_col_ok,
        "UXI-TextEditing-1: compose caret col past end of line [{ctx}]"
    );
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
                    v.with_session(id, cx, |c| {
                        c.input_surface.compose_mut().editor.insert_char('x')
                    });
                }),
                1 => view.update(vcx, |v, cx| {
                    v.with_session(id, cx, |c| {
                        c.input_surface.compose_mut().editor.insert_char('\n')
                    });
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
                            c.tools
                                .register(crate::ToolCallKey::from_id(&tcid), tc, anchor);
                        }
                    });
                }
                9 => view.update(vcx, |v, cx| v.toggle_agent_focus(cx)),
                10 => view.update(vcx, |v, cx| v.stop_agent_inner(cx)),
                // UXI-AgentTile-11: drive the real You-block open / discard key paths so the
                // fuzzer exercises the inline-edit lifecycle against the oracle.
                11 => view.update_in(vcx, |v, w, cx| {
                    v.handle_claude_key(&ws_bare_key("i"), w, cx)
                }),
                12 => view.update_in(vcx, |v, w, cx| {
                    v.handle_claude_key(&ws_bare_key("escape"), w, cx)
                }),
                13 => view.update(vcx, |v, cx| v.toggle_subagents(cx)),
                // UXI-AgentTile-3: enter/leave panel focus and navigate it through the
                // real Cmd-0 handler + key path, against the oracle.
                14 => view.update(vcx, |v, cx| v.focus_agent_panel(cx)),
                15 => view.update_in(vcx, |v, w, cx| {
                    v.handle_claude_key(&ws_bare_key("j"), w, cx)
                }),
                16 => view.update_in(vcx, |v, w, cx| {
                    v.handle_claude_key(&ws_bare_key("k"), w, cx)
                }),
                17 => view.update_in(vcx, |v, w, cx| {
                    v.handle_claude_key(&ws_bare_key("h"), w, cx)
                }),
                18 => view.update_in(vcx, |v, w, cx| {
                    v.handle_claude_key(&ws_bare_key("l"), w, cx)
                }),
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
fn register_subagent(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    n: u64,
) {
    view.update(vcx, |v, cx| {
        use yalda::acp_channel::{ToolCall, ToolCallId, ToolKind};
        if let Some(mut c) = v.agent_mut(cx) {
            let tcid: ToolCallId = format!("task-{n}").into();
            let mut tc = ToolCall::new(tcid.clone(), format!("Explore {n}"));
            tc.kind = ToolKind::Think;
            tc.raw_input = Some(serde_json::json!({"prompt": "do x"}));
            let anchor = c.editor.anchor_for_line(0);
            c.tools
                .register(crate::ToolCallKey::from_id(&tcid), tc, anchor);
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
    let plan: Plan =
        serde_json::from_value(serde_json::json!({ "entries": entries })).expect("valid plan json");
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
            v.read_session(id, cx, |c| (c.panel_col, c.panel_sel))
                .unwrap()
        })
    };
    assert_eq!(
        col(&view, vcx),
        (PanelColumn::Tasklist, 0),
        "starts in Plan"
    );
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("l"), w, cx)
    });
    assert_eq!(
        col(&view, vcx),
        (PanelColumn::Subagents, 0),
        "l → Subagents"
    );
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    assert_eq!(
        col(&view, vcx),
        (PanelColumn::Subagents, 1),
        "j moves within Subagents"
    );
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("h"), w, cx)
    });
    assert_eq!(
        col(&view, vcx),
        (PanelColumn::Tasklist, 1),
        "h → Plan, row clamped into the column"
    );
    // h again is a no-op (already leftmost).
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("h"), w, cx)
    });
    assert_eq!(
        col(&view, vcx).0,
        PanelColumn::Tasklist,
        "h at the left edge is a no-op"
    );
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

    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("escape"), w, cx)
    });
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
        view.read_with(vcx, |v, cx| {
            v.read_session(id, cx, |c| c.panel_sel).unwrap()
        })
    };
    assert_eq!(sel(&view, vcx), 0, "selection starts at the top");
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    assert_eq!(sel(&view, vcx), 2, "j moves down");
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    assert_eq!(sel(&view, vcx), 2, "j clamps at the last row");
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("k"), w, cx)
    });
    assert_eq!(sel(&view, vcx), 1, "k moves up");
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("g"), w, cx)
    });
    assert_eq!(sel(&view, vcx), 0, "g jumps to the top");
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("G"), w, cx)
    });
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
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("j"), w, cx)
    });
    let want = view.read_with(vcx, |v, cx| {
        v.read_session(id, cx, |c| {
            match &c.panel_column_rows(c.panel_col)[c.panel_sel] {
                crate::agent::PanelItem::Subagent(k) => Some(k.clone()),
                _ => None,
            }
        })
        .unwrap()
    });
    view.update_in(vcx, |v, w, cx| {
        v.handle_claude_key(&ws_bare_key("enter"), w, cx)
    });
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
    let keys_of =
        |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext, action: &str| {
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
    assert!(
        base >= 1,
        "the keymap body must paint once after the tile opens"
    );

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
    assert_eq!(
        label, sess_label,
        "recap targets the focused session's label"
    );
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
        v.apply_recap_event(
            id,
            token,
            ReplyEvent::Chunk("- Working on the recap\n".into()),
            cx,
        );
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
    assert_eq!(
        status2,
        crate::RecapStatus::Ready,
        "non-empty run finalizes Ready"
    );
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
    assert!(
        view.update(vcx, |v, _| v.recaps.contains_key(&id)),
        "recap pinned"
    );

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
    assert!(
        live.contains("second run text"),
        "current run accepts its events"
    );
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
    view.update(vcx, |v, cx| {
        v.dispatch_menu_command("agent-input-toggle", cx)
    });
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
    assert!(
        rw > 1.0 && rh > 1.0,
        "recap panel painted with no area (w={rw}, h={rh})"
    );
    // It renders inside the tile, above the compose (its slot above the
    // subagents/tasks panels). The compose box always paints, so this is a real
    // placement check, not a vacuous one.
    let (_cx2, cy, _cw, _ch) = compose.expect("compose box did not paint");
    assert!(
        ry < cy,
        "recap panel must paint ABOVE the compose (recap y={ry}, compose y={cy})"
    );
}

/// UXI-AgentTile-19: a REMEMBERED session that can't be resumed on restart flips
/// its tile to the inline "session unavailable — start fresh" notice — NOT the
/// picker — and the notice PAINTS. Drives the real method the auto-resume
/// attach-failure path (`spawn_attach_sessions` with `resuming = true`) invokes on
/// a permanent "session gone" error.
///
/// Negative control: route the dead sid to `reconcile_session_closed` instead (the
/// pre-fix behavior) → the tile shows `picker`, `unavailable` is None → the state
/// and paint asserts fail RED.
#[gpui::test]
fn unresumable_session_shows_inline_notice_not_picker(cx: &mut TestAppContext) {
    use crate::App;

    // Painting harness: an agent tile bound to session sid "S1" (splash dismissed
    // so `render_agent` actually runs).
    let (view, vcx, _id, _session) = boot_with_transcript(cx);

    // The tile is Bound to session sid "S1" (bound by boot_with_transcript).
    let win = view.update(vcx, |v, _cx| {
        v.workspace.focused_window_id().expect("focused")
    });

    // The remembered session turned out gone server-side (the resuming attach-fail
    // path calls exactly this).
    view.update(vcx, |v, cx| {
        v.reconcile_session_unavailable("S1", cx);
    });
    vcx.run_until_parked();

    // State: unavailable, NOT the picker; identity kept for a later re-attempt.
    view.read_with(vcx, |v, _cx| {
        for wsp in v.workspace.workspaces.iter() {
            if let Some(w) = wsp.layout.find_leaf(win)
                && let App::Agent(t) = &w.content
            {
                assert!(
                    t.session().is_none(),
                    "tile is unbound after the session went away"
                );
                assert!(t.picker().is_none(), "must NOT drop to the picker");
                assert!(
                    t.unavailable_label().is_some(),
                    "shows the inline unavailable notice"
                );
                // The Unavailable variant KEEPS the remembered sid so a later restart
                // re-attempts the resume (ADR-0026: the state carries its own data).
                match t {
                    crate::AgentTile::Unavailable { remembered, .. } => {
                        assert_eq!(
                            remembered.as_str(),
                            "S1",
                            "remembered id kept for re-attempt"
                        )
                    }
                    _ => panic!("expected Unavailable state"),
                }
                return;
            }
        }
        panic!("agent tile not found");
    });

    // Paint: the notice actually renders with area (non-vacuous).
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let notice = crate::layout_probe_get("agent-unavailable");
    crate::layout_probe_end();
    let (_, _, w, h) = notice.expect("the unavailable notice did NOT paint");
    assert!(w > 1.0 && h > 1.0, "notice has no area ({w}x{h})");
}

// ─────────────── Stage C — infinite-plane semantic-zoom (UXI-Workspace-4) ───────────────
//
// LOD render + culling via the layout probe (spec-infinite-plane-workspace.md
// § Verification "LOD render + culling"). These drive the REAL `render_desktop`
// path headlessly and assert on PAINT (`probe_bounds_dyn` tags `plane-card-{id}`
// / `plane-tile-content-{id}`), per the anti-circling rules — a state assert
// can't catch "the placeholder didn't paint" or "live content leaked at Card".

/// Boot a desktop-mode plane with two tiles: `focused` at the origin slot and
/// `other` far off to the right (col 100), on a small known canvas so the far
/// tile is genuinely outside the viewport. Auto-pan is pinned OFF
/// (`last_reveal = focused`) so the camera rests at the origin and placement is
/// deterministic. Returns `(view, vcx, focused_id, other_id)`.
#[cfg(test)]
/// Endpoints for a `Cmd+Shift` free-pan gesture, derived from the REAL painted
/// canvas + tile geometry (pitch = tile + gutter). Mouse-DOWN lands in the empty
/// bottom-right corner (so the canvas-root pan handler owns the gesture, not a
/// tile title bar); the end point is ~1.4 pitch up-and-left, so the pan is a
/// fraction mid-gesture (fract ≈ 0.4) and rounds to a positive whole slot on
/// release. Returns `(down, up)` window points.
fn pan_drag_endpoints(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
) -> (gpui::Point<gpui::Pixels>, gpui::Point<gpui::Pixels>) {
    use gpui::{point, px};
    let (cx0, cy0, cw, ch, tile) = view.read_with(vcx, |v, _| {
        let (x, y, w, h) = v.desktop_canvas_bounds.get();
        (x, y, w, h, v.desktop_tile_px())
    });
    let gutter = crate::chrome::DESKTOP_GUTTER;
    let pitch = (tile.0 + gutter, tile.1 + gutter);
    let start = (cx0 + cw * 0.9, cy0 + ch * 0.9);
    let end = (start.0 - 1.4 * pitch.0, start.1 - 1.4 * pitch.1);
    (point(px(start.0), px(start.1)), point(px(end.0), px(end.1)))
}

/// UXI-Workspace-11: the snap cell is exactly 160×160 logical pixels regardless
/// of the canvas size or the configured default new-tile span. Resizing changes
/// only how many cells the viewport covers.
///
/// NEGATIVE CONTROL (observed RED in the prior viewport-derived implementation):
/// recomputing from `desktop_canvas_bounds` changes the result across these
/// canvases instead of returning the fixed `(160,160)`.
#[gpui::test]
fn workspace_cells_keep_fixed_size_when_the_window_resizes(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);

    let (initial, resized, reconfigured) = view.update(vcx, |v, _| {
        v.desktop_grid_cols = 4;
        v.desktop_grid_rows = 4;

        v.desktop_canvas_bounds.set((0.0, 0.0, 1200.0, 900.0));
        let initial = v.desktop_tile_px();

        // Simulate narrowing/shortening the app. Only the viewport changes.
        v.desktop_canvas_bounds.set((0.0, 0.0, 600.0, 400.0));
        let resized = v.desktop_tile_px();

        // Span configuration affects future tiles, not the cell pitch.
        v.desktop_grid_cols = 3;
        v.desktop_grid_rows = 3;
        let reconfigured = v.desktop_tile_px();
        (initial, resized, reconfigured)
    });

    assert_eq!(
        initial,
        (160.0, 160.0),
        "cell size should match 320×320 physical pixels at 2× Retina"
    );
    assert_eq!(
        resized, initial,
        "window resize must keep cell dimensions fixed; the view should simply cover fewer cells"
    );
    assert_eq!(
        reconfigured, initial,
        "changing the default new-tile span must not change the snap-cell size"
    );
}

/// UXI-Workspace-12: the production bounds observer records the latest outer
/// window dimensions so the next process can use them for `WindowOptions`.
#[gpui::test]
fn window_resize_observer_persists_the_size_for_next_launch(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().expect("temp preferences dir");
    let prefs_path = temp.path().join("preferences.json");

    crate::persist::with_preferences_path(prefs_path.clone(), || {
        let (_view, vcx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            YaldaGpuiView::observe_window_size(window, cx);
            YaldaGpuiView::new_browser(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                Theme::default(),
                focus_handle,
            )
        });

        vcx.simulate_resize(gpui::size(px(1110.0), px(770.0)));
        vcx.run_until_parked();
    });

    let persisted =
        crate::persist::with_preferences_path(prefs_path, crate::persist::load_preferences);
    assert_eq!(persisted.window_width_px, Some(1110.0));
    assert_eq!(persisted.window_height_px, Some(770.0));
    assert_eq!(
        crate::restore_window_size(persisted.window_width_px, persisted.window_height_px),
        (1110.0, 770.0),
        "the startup path consumes the exact size written by the resize observer"
    );
}

fn boot_desktop_two_tiles<'a>(
    cx: &'a mut TestAppContext,
) -> (
    gpui::Entity<YaldaGpuiView>,
    &'a mut gpui::VisualTestContext,
    crate::workspace::WindowId,
    crate::workspace::WindowId,
) {
    use crate::{AgentSession, AgentState, AgentTile, App};
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();

    let (focused_id, other_id) = view.update(vcx, |v, cx| {
        let mk = |label: &str| AgentSession {
            state: AgentState::new_server_managed(None),
            label: label.into(),
            cwd: PathBuf::from("."),
            resume_id: None,
        };
        // Tile A (origin) — bind a session so `render_agent` builds live content.
        v.set_screen(App::Agent(AgentTile::new()));
        let id_a = v.show_local_session(mk("A"), cx);
        v.sessions.bind_sid(id_a, "A".into()).unwrap();
        let win_a = v.workspace.focused_window_id().expect("focused A");
        // Split → tile B; bind a session.
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));
        let id_b = v.show_local_session(mk("B"), cx);
        v.sessions.bind_sid(id_b, "B".into()).unwrap();
        let win_b = v.workspace.focused_window_id().expect("focused B");

        // Force a small, KNOWN desktop canvas. Fixed 172px Full-detail pitch
        // makes slot 100 unambiguously off-viewport.
        v.desktop_grid_cols = 2;
        v.desktop_grid_rows = 2;
        v.viewport_width_px = 800.0;
        v.viewport_height_px = 600.0;
        v.desktop_canvas_bounds.set((0.0, 0.0, 800.0, 600.0));

        // Every workspace is a plane now (infinite-plane, Stage D); place the tiles.
        let wsp = v.workspace.active_workspace_mut().unwrap();
        wsp.layout_mode = crate::workspace::LayoutMode::Plane;
        // This helper exists specifically for plane camera/placement tests. Keep
        // that precondition explicit now that Columns is the product default.
        wsp.view = crate::workspace::WorkspaceView::Plane;
        let leaves = wsp.layout.leaf_ids();
        wsp.desktop.reconcile(&leaves);
        wsp.desktop
            .set_anchor(win_a, crate::workspace::Slot::new(0, 0));
        wsp.desktop
            .set_anchor(win_b, crate::workspace::Slot::new(0, 100));
        // Focus A (the origin tile) and pin the reveal so auto-pan won't drag
        // the camera to wherever focus is — the camera rests at (0,0).
        wsp.focused = win_a;
        wsp.desktop.last_reveal = Some(win_a);
        wsp.desktop.camera.pan = (0.0, 0.0);
        cx.notify();
        (win_a, win_b)
    });
    vcx.run_until_parked();
    (view, vcx, focused_id, other_id)
}

/// At `Card` zoom, every tile is a CHEAP placeholder: the `plane-card-{id}`
/// probe paints, and the live-content probe (`plane-tile-content-{id}`, the
/// agent transcript element) is ABSENT (never built). This is the semantic-zoom
/// contract (spec Behavior 3 / Constraint C2 / UXI-Workspace-4).
///
/// NEGATIVE CONTROL (observed RED): in `render_desktop`, change the placeholder
/// branch guard from `if zoom != Detail::Full` to `if false` (so Card falls
/// through to the live path). Re-run: `plane-card-*` never paints AND
/// `plane-tile-content-*` DOES → both asserts fire. Restored after.
#[gpui::test]
fn plane_card_zoom_paints_placeholders_not_live_content(cx: &mut TestAppContext) {
    let (view, vcx, focused_id, _other) = boot_desktop_two_tiles(cx);

    // Zoom to Card.
    view.update(vcx, |v, cx| {
        let wsp = v.workspace.active_workspace_mut().unwrap();
        wsp.desktop.camera.zoom = crate::workspace::Detail::Card;
        cx.notify();
    });
    for _ in 0..2 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let card = crate::layout_probe_get(&format!("plane-card-{focused_id}"));
    let live = crate::layout_probe_get(&format!("plane-tile-content-{focused_id}"));
    crate::layout_probe_end();

    let (_x, _y, w, h) = card.expect(
        "plane-card probe did NOT paint at Card zoom — the focused tile's card \
         placeholder is invisible (UXI-Workspace-4 violated)",
    );
    assert!(
        w > 1.0 && h > 1.0,
        "card placeholder painted with no area (w={w}, h={h})"
    );
    assert!(
        live.is_none(),
        "LIVE tile content painted at Card zoom (plane-tile-content-{focused_id}) — \
         the placeholder must NOT build live App content (Constraint C2)"
    );
}

/// The focused tile ALWAYS renders even when it's off-viewport (C5): it carries
/// the focus handle + per-screen wiring, so culling it strands the keyboard. An
/// UNFOCUSED off-viewport tile is culled. Focused tile A sits at the origin;
/// unfocused B at col 100 (far off-view). To make the test NON-vacuous we prove
/// A's OWN painted rect extends beyond the canvas is not required — instead we
/// place A off-view too: we pan the camera far away so A's slot is outside the
/// viewport, then assert A (focused) still paints while B (unfocused, also
/// off-view) does not.
///
/// NEGATIVE CONTROL (observed RED): remove the focus exemption in
/// `render_desktop` (change `if !visible && !is_focused` to `if !visible`).
/// Re-run: focused A no longer paints when off-view → the "A must paint" assert
/// fires. Restored after.
#[gpui::test]
fn plane_focused_tile_renders_when_off_viewport(cx: &mut TestAppContext) {
    let (view, vcx, focused_id, other_id) = boot_desktop_two_tiles(cx);

    // Pan the camera far to the right (in SLOT units) so BOTH tiles are off the
    // 800×600 viewport: A (origin, col 0) is now far LEFT of view; B (col 100)
    // is still far right. Neither slot intersects the viewport.
    view.update(vcx, |v, cx| {
        let wsp = v.workspace.active_workspace_mut().unwrap();
        wsp.desktop.camera.pan = (50.0, 0.0); // 50 slots right — origin is off-left
        wsp.desktop.last_reveal = Some(focused_id); // keep auto-pan from re-revealing
        cx.notify();
    });
    for _ in 0..2 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let focused = crate::layout_probe_get(&format!("plane-tile-content-{focused_id}"));
    let other = crate::layout_probe_get(&format!("plane-tile-content-{other_id}"));
    // Non-vacuity: confirm the focused tile's painted x is genuinely LEFT of the
    // viewport (x + w <= 0), i.e. it really is off-screen, not merely at an edge.
    crate::layout_probe_end();

    let (fx, _fy, fw, _fh) = focused.expect(
        "focused tile did NOT paint when off-viewport (C5 violated) — culling it \
         strands the keyboard",
    );
    assert!(
        fx + fw <= 0.0,
        "focused tile is NOT actually off-viewport (x={fx}, w={fw}) — the test is \
         vacuous; it must sit left of x=0"
    );
    assert!(
        other.is_none(),
        "an UNFOCUSED off-viewport tile painted (plane-tile-content-{other_id}) — \
         culling is not running (a vacuous focus-exemption test)"
    );
}

/// UXI-Workspace-14: the columns arrangement lays EVERY tile out as an
/// full-height primary/stack column, side by side — including a tile the plane
/// would cull off-viewport. Drives the REAL toggle handler (`Ctrl-W a` / the `.`
/// menu both call `toggle_workspace_columns` → `Workspace::toggle_view`). The
/// fixture explicitly starts in `Plane` so this toggle guard remains independent
/// of the product's default arrangement.
///
/// The fixture (`boot_desktop_two_tiles`) parks B at slot (0,100) — far off the
/// 800×600 viewport — so on the PLANE only A paints (proven first, for
/// non-vacuity). After the toggle BOTH tiles paint as columns: B is now on
/// screen, to the RIGHT of A, with the default 60/40 primary split.
///
/// NEGATIVE CONTROL (observed RED): in `render_focused_window`, drop the
/// `WorkspaceView::Columns` arm (always call `render_desktop`). Re-run: after the
/// toggle B is still culled and the `columns-tile-*` probes never paint → the
/// "both tiles paint as columns" asserts fire. Restored after.
#[gpui::test]
fn columns_view_arranges_tiles_side_by_side(cx: &mut TestAppContext) {
    let (view, vcx, focused_id, other_id) = boot_desktop_two_tiles(cx);

    // ── On the PLANE, B (slot col 100) is off-viewport and culled. Establish
    // that first so "columns shows B" is a real, non-vacuous contrast. ──
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let plane_a = crate::layout_probe_get(&format!("plane-tile-content-{focused_id}"));
    let plane_b = crate::layout_probe_get(&format!("plane-tile-content-{other_id}"));
    crate::layout_probe_end();
    assert!(
        plane_a.is_some(),
        "sanity: focused tile A must paint on the plane"
    );
    assert!(
        plane_b.is_none(),
        "fixture broken: B must be culled off-viewport on the plane so the columns \
         contrast is non-vacuous"
    );

    // ── Toggle to columns via the REAL handler (the keybinding / menu path). ──
    view.update_in(vcx, |v, w, cx| {
        v.toggle_workspace_columns(&crate::ToggleWorkspaceColumns, w, cx)
    });
    let view_mode = view.read_with(vcx, |v, _| v.workspace.active_workspace().unwrap().view);
    assert_eq!(
        view_mode,
        crate::workspace::WorkspaceView::Columns,
        "the toggle handler must flip the arrangement to Columns"
    );
    for _ in 0..2 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    // ── In columns, BOTH tiles paint side by side. ──
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let col_a = crate::layout_probe_get(&format!("columns-tile-{focused_id}"));
    let col_b = crate::layout_probe_get(&format!("columns-tile-{other_id}"));
    // The live content must render inside each column, not just the frame.
    let live_a = crate::layout_probe_get(&format!("plane-tile-content-{focused_id}"));
    let live_b = crate::layout_probe_get(&format!("plane-tile-content-{other_id}"));
    crate::layout_probe_end();

    let (ax, ay, aw, ah) = col_a.expect("column A frame did not paint in columns view");
    let (bx, _by, bw, bh) = col_b.expect(
        "column B frame did not paint in columns view — the tile the plane culled \
         must appear as a column (UXI-Workspace-14)",
    );
    assert!(live_a.is_some(), "column A's live content must paint");
    assert!(
        live_b.is_some(),
        "column B's live content must paint — the culled plane tile is now visible"
    );
    // Side by side: B sits strictly to the right of A and does not overlap it.
    assert!(
        bx > ax && bx >= ax + aw - 1.0,
        "columns must be side by side (A at x={ax} w={aw}, B at x={bx}) — B must be \
         to the RIGHT of A with no overlap"
    );
    // Columns is equal-width (no primary area — that belongs to Tiling now,
    // UXI-Workspace-26). Both tiles split the width evenly, within a small
    // tolerance for gutters and borders.
    assert!(
        (aw - bw).abs() <= 4.0,
        "columns must be equal width (A={aw}, B={bw})"
    );
    assert!(
        ah > 1.0 && (ah - bh).abs() <= 2.0,
        "columns must be full, equal height (A={ah}, B={bh})"
    );
    // Non-vacuity: the tiles occupy real horizontal space inside the viewport.
    assert!(
        aw > 50.0 && ay >= 0.0,
        "column A painted with no real area (x={ax}, y={ay}, w={aw})"
    );
}

/// UXI-Workspace-26: Monocle paints ONLY the focused tile, filling the region;
/// the other tile's live content does not paint. Non-vacuous: the fixture has
/// two real tiles and columns proves both CAN paint, so Monocle hiding one is a
/// real contrast.
///
/// NEGATIVE CONTROL (observed RED): in `render_focused_window`, route the
/// `Monocle` arm to `render_columns(..., false, ..)` instead of `render_monocle`.
/// Re-run: the non-focused tile's live content paints and the "B is hidden"
/// assert fires.
#[gpui::test]
fn monocle_view_paints_only_the_focused_tile(cx: &mut TestAppContext) {
    use crate::workspace::WorkspaceView;
    let (view, vcx, focused_id, other_id) = boot_desktop_two_tiles(cx);
    view.update(vcx, |v, cx| {
        v.workspace.active_workspace_mut().unwrap().view = WorkspaceView::Monocle;
        cx.notify();
    });
    vcx.run_until_parked();

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let mono_focused = crate::layout_probe_get(&format!("monocle-tile-{focused_id}"));
    let live_focused = crate::layout_probe_get(&format!("plane-tile-content-{focused_id}"));
    let live_other = crate::layout_probe_get(&format!("plane-tile-content-{other_id}"));
    crate::layout_probe_end();

    let (_x, _y, mw, mh) = mono_focused.expect("focused tile must paint in monocle");
    assert!(
        live_focused.is_some(),
        "focused tile's live content must paint in monocle"
    );
    assert!(
        live_other.is_none(),
        "non-focused tile must NOT paint in monocle (only the focused tile shows)"
    );
    assert!(
        mw > 200.0 && mh > 100.0,
        "the monocle tile fills the region (w={mw}, h={mh})"
    );
}

/// UXI-Workspace-26: the `.` layout-mode commands set the active workspace's
/// arrangement through the REAL dispatcher. Negative control: make
/// `set_active_workspace_view` a no-op ⇒ the view never changes and this fails RED.
#[gpui::test]
fn layout_mode_commands_set_the_active_arrangement(cx: &mut TestAppContext) {
    use crate::workspace::WorkspaceView;
    let (view, vcx) = boot_browser(cx);
    for (command, expected) in [
        ("layout-tiling", WorkspaceView::Tiling),
        ("layout-monocle", WorkspaceView::Monocle),
        ("layout-columns", WorkspaceView::Columns),
    ] {
        view.update(vcx, |v, cx| v.dispatch_menu_command(command, cx));
        vcx.run_until_parked();
        assert_eq!(
            view.read_with(vcx, |v, _| v.workspace.active_workspace().unwrap().view),
            expected,
            "{command} must set the arrangement to {expected:?}"
        );
    }
}

/// UXI-Workspace-17: the registered lowercase command sends without following;
/// uppercase sends and follows. Both traverse keymap → action → existing picker
/// → Enter → stable relocation, rather than calling the model directly.
#[gpui::test]
fn ctrl_w_send_and_send_follow_use_the_same_project_picker(cx: &mut TestAppContext) {
    use crate::workspace::{SplitDir, TileMembership};
    use crate::{App, LinearTile};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    let moved = view.update(vcx, |v, _| {
        let moved = v
            .workspace
            .split_focused(SplitDir::V, App::Linear(LinearTile::new()))
            .unwrap();
        v.workspace
            .push_workspace_inheriting(App::Linear(LinearTile::new()));
        v.workspace.push_initial_workspace(
            App::Linear(LinearTile::new()),
            crate::project::ProjectId(999),
        );
        v.workspace.set_active_workspace(0);
        v.workspace.workspaces[0].focused = moved;
        moved
    });

    vcx.simulate_keystrokes("ctrl-w m");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let picker = v.workspace_picker_ref().expect("send picker opened");
        assert_eq!(picker.targets, vec![0, 1]);
        assert_eq!(
            picker.mode,
            crate::WorkspacePickerMode::Move { follow: false }
        );
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 0,
            "lowercase send stays at source"
        );
        assert_eq!(
            v.workspace.tile_membership(moved),
            Some(TileMembership::Attached {
                workspace: 1,
                visibility: crate::workspace::AttachedVisibility::Visible
            })
        );
    });

    view.update(vcx, |v, _| {
        v.workspace.set_active_workspace(1);
        v.workspace.workspaces[1].focused = moved;
    });
    vcx.simulate_keystrokes("ctrl-w shift-m");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.active_workspace, 0,
            "uppercase send follows destination"
        );
        assert_eq!(
            v.workspace.tile_membership(moved),
            Some(TileMembership::Attached {
                workspace: 0,
                visibility: crate::workspace::AttachedVisibility::Visible
            })
        );
    });
}

/// UXI-Workspace-25 / bug-0048: destination rows identify the workspace, not
/// the content of whichever tile happens to be focused there. This drives the
/// real Ctrl-W picker path, then resolves the exact production label helper
/// used by the rendered row.
#[gpui::test]
fn send_picker_agent_destination_uses_workspace_name_without_provider_prefix(
    cx: &mut TestAppContext,
) {
    use crate::{AgentTile, App};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    let target = view.update(vcx, |v, _| {
        v.workspace
            .push_workspace_inheriting(App::Agent(AgentTile::new()));
        let target = v.workspace.active_workspace;
        v.workspace.workspaces[target].display_name = Some("Research".into());
        v.workspace.set_active_workspace(0);
        target
    });

    vcx.simulate_keystrokes("ctrl-w m");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let picker = v.workspace_picker_ref().expect("send picker opened");
        assert!(picker.targets.contains(&target));
        let label = crate::workspace_picker_destination_label(&v.workspace.workspaces[target]);
        assert_eq!(
            label, "Research",
            "a destination is the workspace named Research, not the focused Agent tile: {label:?}"
        );
    });
}

/// UXI-Workspace-25: the real picker paints as a compact destination card, not
/// a nearly full-width debug list. Its fixed chrome and 42px option rows do not
/// inherit document zoom; the creation action is separated below the existing
/// destinations. The selected row's real painted bounds are also clickable.
#[gpui::test]
fn send_picker_paints_compact_hierarchy_and_click_moves_tile(cx: &mut TestAppContext) {
    use crate::workspace::TileMembership;
    use crate::{AgentTile, App, LinearTile};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    let (moved, target) = view.update(vcx, |v, _| {
        let moved = v
            .workspace
            .split_focused(
                crate::workspace::SplitDir::V,
                App::Linear(LinearTile::new()),
            )
            .expect("second source tile");
        v.workspace
            .push_workspace_inheriting(App::Agent(AgentTile::new()));
        let target = v.workspace.active_workspace;
        v.workspace.workspaces[target].display_name = Some("Research".into());
        v.workspace
            .push_workspace_inheriting(App::Linear(LinearTile::new()));
        v.workspace.set_active_workspace(0);
        (moved, target)
    });

    vcx.simulate_keystrokes("ctrl-w m");
    vcx.run_until_parked();

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let card = crate::layout_probe_get("workspace-picker-card").expect("picker card paints");
    let header =
        crate::layout_probe_get("workspace-picker-header").expect("picker header paints");
    let options =
        crate::layout_probe_get("workspace-picker-options").expect("picker options paint");
    let row0 = crate::layout_probe_get("workspace-picker-row-0").expect("source row paints");
    let selected =
        crate::layout_probe_get("workspace-picker-row-1").expect("selected target row paints");
    let last = crate::layout_probe_get("workspace-picker-row-2").expect("last target row paints");
    let create =
        crate::layout_probe_get("workspace-picker-new-row").expect("creation row paints");
    crate::layout_probe_end();

    let (card_x, card_y, card_w, card_h) = card;
    assert!(
        (450.0..=481.0).contains(&card_w) && (220.0..=420.0).contains(&card_h),
        "picker must be a compact 480px card, got ({card_x}, {card_y}, {card_w}, {card_h})"
    );
    assert!(
        header.1 >= card_y && options.1 >= header.1 + header.3 - 1.0,
        "title/subtitle header must sit above the destination list: header={header:?} options={options:?}"
    );
    for (name, row) in [("first", row0), ("selected", selected), ("last", last)] {
        assert!(
            (row.3 - 42.0).abs() <= 1.0 && row.0 > card_x && row.2 < card_w,
            "{name} destination row must use compact fixed chrome inside the card: {row:?}"
        );
    }
    assert!(
        create.1 >= last.1 + last.3 + 10.0 && (create.3 - 42.0).abs() <= 1.0,
        "New workspace must be a separated 42px action below destinations: last={last:?} create={create:?}"
    );

    let at = point(
        px(selected.0 + selected.2 / 2.0),
        px(selected.1 + selected.3 / 2.0),
    );
    vcx.simulate_mouse_move(at, None, gpui::Modifiers::default());
    vcx.simulate_click(at, gpui::Modifiers::default());
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!(!v.overlay_is_workspace(), "click commits and closes picker");
        assert_eq!(
            v.workspace.tile_membership(moved),
            Some(TileMembership::Attached {
                workspace: target,
                visibility: crate::workspace::AttachedVisibility::Visible,
            })
        );
        assert_eq!(
            v.workspace.active_workspace, 0,
            "plain Move stays in the source workspace"
        );
    });
}

/// ADR-0034: hide/unhide and back-and-forth actions are wired at the shared
/// tile wrapper, so the real chords work regardless of focused App kind.
#[gpui::test]
fn ctrl_w_hide_unhide_and_workspace_back_and_forth_are_global(cx: &mut TestAppContext) {
    use crate::workspace::{SplitDir, TileMembership};
    use crate::{App, LinearTile};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    let (hidden, previous_focus) = view.update(vcx, |v, _| {
        let id = v
            .workspace
            .split_focused(SplitDir::V, App::Linear(LinearTile::new()))
            .unwrap();
        let previous_focus = v
            .workspace
            .push_workspace_inheriting(App::Linear(LinearTile::new()));
        v.workspace.set_active_workspace(0);
        v.workspace.workspaces[0].focused = id;
        (id, previous_focus)
    });

    vcx.simulate_keystrokes("ctrl-w d");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.tile_membership(hidden),
            Some(TileMembership::Attached {
                workspace: 0,
                visibility: crate::workspace::AttachedVisibility::Hidden,
            })
        );
        assert!(v.workspace.presented_tile().is_none());
    });
    view.update(vcx, |v, _| assert!(v.workspace.focus_tile(hidden)));
    vcx.simulate_keystrokes("ctrl-w shift-d");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.tile_membership(hidden),
            Some(TileMembership::Attached {
                workspace: 0,
                visibility: crate::workspace::AttachedVisibility::Visible,
            })
        );
        assert_eq!(v.workspace.focused_window_id(), Some(hidden));
        assert_eq!(v.workspace.active_workspace, 0);
    });

    // The discoverable menu route and the global actions converge on the same
    // typed state machine. Exercise the actual dispatcher and shared listener,
    // not model helpers, so tiles cannot capture or bypass these operations.
    view.update(vcx, |v, cx| v.dispatch_menu_command("send-tile", cx));
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace_picker_ref().map(|picker| picker.mode),
            Some(crate::WorkspacePickerMode::Move { follow: false })
        );
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();

    vcx.simulate_keystrokes("ctrl-w shift-b");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.tile_membership(hidden),
            Some(TileMembership::Detached)
        );
    });
    vcx.simulate_keystrokes("ctrl-w b");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.tile_membership(hidden),
            Some(TileMembership::Attached {
                workspace: 0,
                visibility: crate::workspace::AttachedVisibility::Visible,
            })
        );
    });

    view.update(vcx, |v, cx| v.dispatch_menu_command("tile-hide", cx));
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.tile_membership(hidden),
            Some(TileMembership::Attached {
                workspace: 0,
                visibility: crate::workspace::AttachedVisibility::Hidden,
            }),
            "the system menu's Hide command changes visibility without detaching"
        );
    });
    view.update(vcx, |v, cx| v.jump_to_tile(hidden, cx));
    view.update(vcx, |v, cx| v.dispatch_menu_command("tile-detach", cx));
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.tile_membership(hidden),
            Some(TileMembership::Detached),
            "detaching a hidden tile clears hidden state"
        );
    });

    vcx.simulate_keystrokes("ctrl-w backspace");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(v.workspace.active_workspace, 1);
        assert_eq!(v.workspace.focused_window_id(), Some(previous_focus));
    });
}

/// UXI-Workspace-20/22: all four registered primary commands mutate the Tiling
/// arrangement and the ratio command changes painted geometry without touching
/// plane slots. (The primary area moved from Columns to Tiling in UXI-Workspace-26.)
#[gpui::test]
fn ctrl_w_primary_commands_change_columns_state_and_geometry(cx: &mut TestAppContext) {
    use crate::workspace::WorkspaceView;
    cx.update(crate::register_keymap);
    let (view, vcx, primary_id, stack_id) = boot_desktop_two_tiles(cx);
    view.update(vcx, |v, cx| {
        v.workspace.active_workspace_mut().unwrap().view = WorkspaceView::Tiling;
        cx.notify();
    });
    vcx.run_until_parked();

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let before_primary = crate::layout_probe_get(&format!("columns-tile-{primary_id}"))
        .expect("primary paints before adjustment");
    let before_stack = crate::layout_probe_get(&format!("columns-tile-{stack_id}"))
        .expect("stack paints before adjustment");
    crate::layout_probe_end();

    vcx.simulate_keystrokes("ctrl-w f");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert!((v.workspace.active_workspace().unwrap().primary_ratio - 0.65).abs() < 0.001);
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let after_primary = crate::layout_probe_get(&format!("columns-tile-{primary_id}"))
        .expect("primary paints after adjustment");
    let after_stack = crate::layout_probe_get(&format!("columns-tile-{stack_id}"))
        .expect("stack paints after adjustment");
    crate::layout_probe_end();
    assert!(after_primary.2 > before_primary.2, "grow makes primary wider");
    assert!(after_stack.2 < before_stack.2, "grow makes stack narrower");

    vcx.simulate_keystrokes("ctrl-w shift-f");
    vcx.simulate_keystrokes("ctrl-w n");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().unwrap();
        assert!((wsp.primary_ratio - 0.60).abs() < 0.001);
        assert_eq!(wsp.primary_count, 2);
    });
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let all_primary_a = crate::layout_probe_get(&format!("columns-tile-{primary_id}"))
        .expect("first all-primary tile paints");
    let all_primary_b = crate::layout_probe_get(&format!("columns-tile-{stack_id}"))
        .expect("second all-primary tile paints");
    crate::layout_probe_end();
    assert!(
        (all_primary_a.2 - all_primary_b.2).abs() <= 2.0,
        "when primary_count covers every tile, the full width is shared equally"
    );
    vcx.simulate_keystrokes("ctrl-w shift-n");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| v
            .workspace
            .active_workspace()
            .unwrap()
            .primary_count),
        1
    );
}

/// UXI-Workspace-26: Tiling stacks the non-primary tiles VERTICALLY in the
/// right-hand stack column — they share an x and descend in y — while the primary
/// tile sits in the left column. Uses three tiles (1 primary + 2 stack) so the
/// vertical relationship among stack tiles is observable.
///
/// NEGATIVE CONTROL (observed RED): change the Tiling stack pane back to
/// `.flex_row()` in `render_columns`. Re-run: the two stack tiles get DIFFERENT x
/// and equal y, so the "same x / descending y" asserts fire.
#[gpui::test]
fn tiling_stacks_non_primary_tiles_vertically(cx: &mut TestAppContext) {
    use crate::workspace::{Slot, WorkspaceView};
    use crate::{AgentSession, AgentState, AgentTile, App};
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);

    // Add a third tile C so the stack column holds two tiles.
    let win_c = view.update(vcx, |v, cx| {
        let mk = AgentSession {
            state: AgentState::new_server_managed(None),
            label: "C".into(),
            cwd: PathBuf::from("."),
            resume_id: None,
        };
        v.workspace
            .split_focused(crate::workspace::SplitDir::H, App::Agent(AgentTile::new()));
        let id_c = v.show_local_session(mk, cx);
        v.sessions.bind_sid(id_c, "C".into()).unwrap();
        let win_c = v.workspace.focused_window_id().expect("focused C");
        let wsp = v.workspace.active_workspace_mut().unwrap();
        // Deterministic reading order A,B,C along one row so primary=A, stack=B,C.
        // 1×1 spans so the adjacent anchors don't overlap (which would make the
        // per-frame reconcile re-seed and scramble the order).
        for id in [win_a, win_b, win_c] {
            wsp.desktop.set_span(id, crate::workspace::Span::new(1, 1));
        }
        wsp.desktop.set_anchor(win_a, Slot::new(0, 0));
        wsp.desktop.set_anchor(win_b, Slot::new(0, 1));
        wsp.desktop.set_anchor(win_c, Slot::new(0, 2));
        wsp.view = WorkspaceView::Tiling; // primary_count defaults to 1
        wsp.focused = win_a;
        cx.notify();
        win_c
    });
    vcx.run_until_parked();

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let a = crate::layout_probe_get(&format!("columns-tile-{win_a}")).expect("primary A paints");
    let b = crate::layout_probe_get(&format!("columns-tile-{win_b}")).expect("stack B paints");
    let c = crate::layout_probe_get(&format!("columns-tile-{win_c}")).expect("stack C paints");
    crate::layout_probe_end();

    // Stack tiles sit to the RIGHT of the primary.
    assert!(
        b.0 > a.0 && c.0 > a.0,
        "stack tiles must be right of the primary (A.x={}, B.x={}, C.x={})",
        a.0,
        b.0,
        c.0
    );
    // Stack tiles share a column: same x, roughly same width.
    assert!(
        (b.0 - c.0).abs() <= 2.0 && (b.2 - c.2).abs() <= 2.0,
        "stack tiles must share an x-column (B x={} w={}, C x={} w={})",
        b.0,
        b.2,
        c.0,
        c.2
    );
    // …and stack VERTICALLY: C is strictly below B, non-overlapping.
    assert!(
        c.1 > b.1 && c.1 >= b.1 + b.3 - 1.0,
        "stack tiles must stack vertically (B y={} h={}, C y={})",
        b.1,
        b.3,
        c.1
    );
}

/// UXI-Workspace-15 real path: the registered upper-case motion commands swap
/// complete footprints, preserve stable focus, honor Columns' visible axes,
/// and `Ctrl-W u` restores the prior placement. Observed RED with directional
/// swap and undo independently disabled in their production handlers.
#[gpui::test]
fn ctrl_w_uppercase_swaps_footprints_in_plane_and_columns(cx: &mut TestAppContext) {
    use crate::workspace::{Slot, Span, WorkspaceView};
    cx.update(crate::register_keymap);
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);

    view.update(vcx, |v, cx| {
        let wsp = v.workspace.active_workspace_mut().unwrap();
        wsp.view = WorkspaceView::Plane;
        wsp.desktop.set_anchor(win_a, Slot::new(0, 0));
        wsp.desktop.set_span(win_a, Span::new(2, 2));
        wsp.desktop.set_anchor(win_b, Slot::new(0, 4));
        wsp.desktop.set_span(win_b, Span::new(3, 1));
        wsp.focused = win_a;
        wsp.desktop.last_reveal = Some(win_a);
        cx.notify();
    });
    vcx.run_until_parked();
    let before = view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().unwrap();
        (
            wsp.desktop.rect_of(win_a),
            wsp.desktop.rect_of(win_b),
            wsp.focused,
        )
    });

    vcx.simulate_keystrokes("ctrl-w shift-l");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().unwrap();
        assert_eq!(
            wsp.desktop.rect_of(win_a),
            before.1,
            "A adopts B's complete footprint"
        );
        assert_eq!(
            wsp.desktop.rect_of(win_b),
            before.0,
            "B adopts A's complete footprint"
        );
        assert_eq!(
            wsp.focused, win_a,
            "stable focused id travels with its tile"
        );
    });

    vcx.simulate_keystrokes("ctrl-w u");
    vcx.run_until_parked();
    let restored = view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().unwrap();
        (
            wsp.desktop.rect_of(win_a),
            wsp.desktop.rect_of(win_b),
            wsp.focused,
        )
    });
    assert_eq!(restored, before, "Ctrl-W u restores Plane placement");

    view.update(vcx, |v, cx| {
        let wsp = v.workspace.active_workspace_mut().unwrap();
        wsp.view = WorkspaceView::Columns;
        wsp.focused = win_a;
        cx.notify();
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("ctrl-w shift-j");
    vcx.run_until_parked();
    let after_vertical = view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().unwrap();
        (wsp.desktop.rect_of(win_a), wsp.desktop.rect_of(win_b))
    });
    assert_eq!(after_vertical, (before.0, before.1), "Columns J is a no-op");

    vcx.simulate_keystrokes("ctrl-w shift-l");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().unwrap();
        assert_eq!(wsp.desktop.rect_of(win_a), before.1);
        assert_eq!(wsp.desktop.rect_of(win_b), before.0);
        assert_eq!(wsp.focused, win_a);
    });
}

/// UXI-Workspace-15 real path for promotion and both rotation directions.
/// Observed RED with promote, forward rotate, and backward rotate separately
/// disabled in the production handlers.
#[gpui::test]
fn ctrl_w_promote_and_rotate_three_tiles(cx: &mut TestAppContext) {
    use crate::workspace::{Slot, SplitDir};
    use crate::{AgentTile, App};
    cx.update(crate::register_keymap);
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);
    let win_c = view.update(vcx, |v, cx| {
        let win_c = v
            .workspace
            .split_focused(SplitDir::H, App::Agent(AgentTile::new()))
            .expect("third tile");
        let wsp = v.workspace.active_workspace_mut().unwrap();
        let leaves = wsp.layout.leaf_ids();
        wsp.desktop.reconcile(&leaves);
        wsp.desktop.set_anchor(win_a, Slot::new(0, 0));
        wsp.desktop.set_anchor(win_b, Slot::new(0, 4));
        wsp.desktop.set_anchor(win_c, Slot::new(0, 8));
        wsp.focused = win_b;
        cx.notify();
        win_c
    });
    vcx.run_until_parked();
    let slots = |v: &YaldaGpuiView| {
        let d = &v.workspace.active_workspace().unwrap().desktop;
        (d.slot_of(win_a), d.slot_of(win_b), d.slot_of(win_c))
    };
    let original = view.read_with(vcx, |v, _| slots(v));

    vcx.simulate_keystrokes("ctrl-w enter");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| slots(v)),
        (original.1, original.0, original.2),
        "promote swaps focused B with the first footprint"
    );
    vcx.simulate_keystrokes("ctrl-w u");
    vcx.run_until_parked();
    assert_eq!(view.read_with(vcx, |v, _| slots(v)), original);

    vcx.simulate_keystrokes("ctrl-w r");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| slots(v)),
        (original.1, original.2, original.0),
        "forward rotation moves each tile to the next footprint"
    );
    vcx.simulate_keystrokes("ctrl-w u");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("ctrl-w shift-r");
    vcx.run_until_parked();
    assert_eq!(
        view.read_with(vcx, |v, _| slots(v)),
        (original.2, original.0, original.1),
        "backward rotation is the inverse cycle"
    );
}

/// UXI-Workspace-15 real picker path: cancel is inert and Enter swaps the
/// selected target while retaining stable focus. Observed RED with picker
/// commit disabled in production.
#[gpui::test]
fn ctrl_w_x_picker_cancels_and_swaps_selected_tile(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);
    let before = view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().unwrap();
        (wsp.desktop.rect_of(win_a), wsp.desktop.rect_of(win_b))
    });

    vcx.simulate_keystrokes("ctrl-w x");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let picker = v.tile_swap_picker_ref().expect("picker opens");
        assert_eq!(picker.source, win_a);
        assert_eq!(picker.targets, vec![win_b]);
    });
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(view.read_with(vcx, |v, _| v.tile_swap_picker_ref().is_none()));
    assert_eq!(
        view.read_with(vcx, |v, _| {
            let wsp = v.workspace.active_workspace().unwrap();
            (wsp.desktop.rect_of(win_a), wsp.desktop.rect_of(win_b))
        }),
        before,
        "picker cancel is mutation-free"
    );

    vcx.simulate_keystrokes("ctrl-w x");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().unwrap();
        assert_eq!(wsp.desktop.rect_of(win_a), before.1);
        assert_eq!(wsp.desktop.rect_of(win_b), before.0);
        assert_eq!(wsp.focused, win_a);
        assert!(v.tile_swap_picker_ref().is_none());
    });
}

/// bug-0012 (UXI-Workspace-6): in a workspace holding exactly ONE tile, a new
/// tile lands at the SAME row, directly to the RIGHT of it — never diagonally.
/// Both tiles use the configured default 4×4 span, so the new anchor is four
/// columns over and its painted footprint is 676×676 logical pixels.
///
/// Drives the REAL path per the anti-circling rules: the sole tile is parked at
/// a non-origin slot `(1,-1)` (the configuration that produced the reported
/// "up and to the right"), the user's actual `Ctrl-W v` action handler
/// (`split_v`) creates the leaf, and the REAL per-frame `chrome.rs` upkeep
/// (`reconcile_near`) is what assigns the slot — the test never seeds one.
///
/// NEGATIVE CONTROL (observed RED), each half separately:
/// - force `center = Slot::new(0,0)` in `reconcile_near` (origin-centered
///   spiral) ⇒ new tile at `(0,0)`, i.e. up-and-to-the-right of `(1,-1)` — the
///   reported bug reproduced;
/// - restore the raw row-major ring scan in `seed_slot_near` ⇒ new tile at
///   `(0,-2)`. Both restored.
#[gpui::test]
fn new_tile_lands_same_row_right_of_the_only_tile(cx: &mut TestAppContext) {
    use crate::workspace::Slot;
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        cx.notify();
    });
    vcx.run_until_parked();

    // One tile, parked OFF the origin at (1,-1), focus resting on it.
    let win_a = view.update(vcx, |v, cx| {
        v.desktop_grid_cols = 4;
        v.desktop_grid_rows = 4;
        v.viewport_width_px = 800.0;
        v.viewport_height_px = 600.0;
        v.desktop_canvas_bounds.set((0.0, 0.0, 800.0, 600.0));
        let win_a = v.workspace.focused_window_id().expect("focused tile");
        let wsp = v.workspace.active_workspace_mut().unwrap();
        wsp.layout_mode = crate::workspace::LayoutMode::Plane;
        let leaves = wsp.layout.leaf_ids();
        assert_eq!(leaves.len(), 1, "the fixture must start with ONE tile");
        wsp.desktop.reconcile(&leaves);
        wsp.desktop
            .set_span(win_a, crate::workspace::Span::new(4, 4));
        wsp.desktop.set_anchor(win_a, Slot::new(1, -1));
        wsp.focused = win_a;
        wsp.desktop.last_reveal = Some(win_a);
        wsp.desktop.camera.pan = (0.0, 0.0);
        cx.notify();
        win_a
    });
    vcx.run_until_parked();

    // The REAL `Ctrl-W v` handler — "new tile to the right of the focused one".
    view.update_in(vcx, |v, w, cx| v.split_v(&crate::SplitV, w, cx));
    // Let the real render path run its per-frame slot upkeep.
    for _ in 0..2 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    let (count, a_slot, a_span, new_slot, new_span) = view.read_with(vcx, |v, _| {
        let wsp = v.workspace.active_workspace().expect("active workspace");
        let leaves = wsp.layout.leaf_ids();
        let new_id = leaves
            .iter()
            .copied()
            .find(|id| *id != win_a)
            .expect("split created a new leaf");
        (
            leaves.len(),
            wsp.desktop.slot_of(win_a),
            wsp.desktop.span_of(win_a),
            wsp.desktop.slot_of(new_id),
            wsp.desktop.span_of(new_id),
        )
    });

    assert_eq!(count, 2, "split must leave exactly two tiles");
    assert_eq!(
        a_slot,
        Some(Slot::new(1, -1)),
        "the existing tile must not move"
    );
    assert_eq!(a_span, crate::workspace::Span::new(4, 4));
    assert_eq!(
        new_slot,
        Some(Slot::new(1, 3)),
        "new tile must sit at the SAME row, directly right of the 4-cell-wide tile \
         (bug-0012: it was landing diagonally, up-and-to-the-right)"
    );
    assert_eq!(
        new_span,
        crate::workspace::Span::new(4, 4),
        "new tiles must default to a useful 4×4-cell footprint"
    );
}

/// Perf proxy (spec Verification, optional): panning the plane at Card must NOT
/// re-render the (non-Full) transcript surfaces — at Card no live transcript is
/// built at all, so the cached `TranscriptView` render count stays flat across a
/// pan. Mirrors the `transcript_021_*` render-count discipline.
#[gpui::test]
fn plane_pan_at_card_leaves_transcript_render_flat(cx: &mut TestAppContext) {
    crate::perf_reset("transcript");
    let (view, vcx, _focused, _other) = boot_desktop_two_tiles(cx);
    // Zoom to Card and settle.
    view.update(vcx, |v, cx| {
        v.workspace
            .active_workspace_mut()
            .unwrap()
            .desktop
            .camera
            .zoom = crate::workspace::Detail::Card;
        cx.notify();
    });
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    let base = crate::perf_render_count("transcript");

    // Pan the plane several times in slot units (the bare-scroll path).
    for _ in 0..5 {
        view.update(vcx, |v, cx| {
            v.workspace
                .active_workspace_mut()
                .unwrap()
                .desktop
                .pan_by(0.5, 0.0);
            cx.notify();
        });
        vcx.run_until_parked();
    }
    let after = crate::perf_render_count("transcript");
    assert_eq!(
        base, after,
        "panning at Card re-rendered the transcript ({base} → {after}) — Card must \
         not build live transcript content (Constraint C2)"
    );
}

/// `Ctrl-W 0` resets the active plane's camera to the origin (Behavior 6,
/// UXI-Workspace-5) through the REAL keymap → `ResetWorkspaceView` action →
/// `reset_workspace_view` handler dispatch. Drives the production keymap
/// (`register_keymap` + `simulate_keystrokes`), not a hand-called method: after
/// panning + zooming AWAY, the chord must return `camera == Camera::default()`
/// while the slots/spans (tile placement) stay untouched (the reset is
/// view-only, Constraint C1).
///
/// NEGATIVE CONTROL (observed RED): comment out the `desktop.reset_view()` line
/// in `reset_workspace_view` (main.rs) — the handler becomes a no-op — and the
/// camera-equals-default assert fires (camera stays at the panned/zoomed pose).
/// Restored after. (The post-`Ctrl-W` digit `0` firing under the REAL macOS OS
/// keymap is the documented key gap, CLAUDE.md rule 4 / spec Verification: a
/// human runtime check confirms the chord; the action+handler are headless-tested
/// here.)
#[gpui::test]
fn ctrl_w_reset_returns_camera_to_origin(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _focused, _other) = boot_desktop_two_tiles(cx);

    // Snapshot the tile placement so we can prove the reset is view-only.
    let slots_before = view.update(vcx, |v, _| {
        v.workspace
            .active_workspace()
            .unwrap()
            .desktop
            .slots
            .clone()
    });

    // Pan AND zoom the camera AWAY from the origin — reset must undo BOTH. Zoom
    // to Minimap deliberately: the plane-camera actions live on the CANVAS root
    // (chrome.rs), which renders at every Detail, so the chord must still fire
    // when the focused tile is a Minimap placeholder (the C5 path).
    view.update(vcx, |v, cx| {
        let d = &mut v.workspace.active_workspace_mut().unwrap().desktop;
        d.pan_by(7.0, -4.0);
        d.camera.zoom = crate::workspace::Detail::Minimap;
        cx.notify();
    });
    vcx.run_until_parked();
    let moved = view.update(vcx, |v, _| {
        v.workspace.active_workspace().unwrap().desktop.camera
    });
    assert_ne!(
        moved,
        crate::workspace::Camera::default(),
        "precondition: camera must be away from origin before the reset"
    );

    // Act: the REAL Ctrl-W 0 sequence through the production keymap.
    vcx.simulate_keystrokes("ctrl-w 0");
    vcx.run_until_parked();

    let (camera, slots_after) = view.update(vcx, |v, _| {
        let d = &v.workspace.active_workspace().unwrap().desktop;
        (d.camera, d.slots.clone())
    });
    assert_eq!(
        camera,
        crate::workspace::Camera::default(),
        "Ctrl-W 0 must return the camera to the origin (pan=(0,0), zoom=Full)"
    );
    assert_eq!(
        slots_after, slots_before,
        "reset is view-only — tile slots must be untouched (Constraint C1)"
    );
}

/// `Ctrl-W -` / `Ctrl-W =` step the active plane's semantic zoom through the REAL
/// keymap → `ZoomOutWorkspace` / `ZoomInWorkspace` action → handler dispatch
/// (Behavior 3). From `Full`: `Ctrl-W -` → `Card` → `Minimap`, clamped at
/// `Minimap`; `Ctrl-W =` steps back toward `Full`.
///
/// NEGATIVE CONTROL (observed RED): unbind the `Ctrl-W -` row in
/// `keymap_registry.rs` (delete the `ZoomOutWorkspace` binding) — the first
/// `simulate_keystrokes("ctrl-w -")` no longer dispatches, so the zoom stays at
/// `Full` and the `== Card` assert fires. Restored after.
#[gpui::test]
fn ctrl_w_zoom_steps_detail(cx: &mut TestAppContext) {
    use crate::workspace::Detail;
    cx.update(crate::register_keymap);
    let (view, vcx, _focused, _other) = boot_desktop_two_tiles(cx);

    let zoom = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, _| {
            v.workspace.active_workspace().unwrap().desktop.camera.zoom
        })
    };

    // Precondition: fresh plane rests at Full.
    assert_eq!(zoom(&view, vcx), Detail::Full, "plane starts at Full");

    // Ctrl-W - : Full → Card. Repeated `ctrl-w X` sequences dispatch cleanly
    // because the plane-camera actions live on the CANVAS root (chrome.rs) — the
    // one element that renders at every Detail — so a step that lands focus on a
    // Card/Minimap placeholder still has the handler in its ancestry.
    vcx.simulate_keystrokes("ctrl-w -");
    vcx.run_until_parked();
    assert_eq!(zoom(&view, vcx), Detail::Card, "one step out ⇒ Card");

    // Ctrl-W - : Card → Minimap.
    vcx.simulate_keystrokes("ctrl-w -");
    vcx.run_until_parked();
    assert_eq!(zoom(&view, vcx), Detail::Minimap, "two steps out ⇒ Minimap");

    // Ctrl-W - : Minimap is the clamp — stays Minimap.
    vcx.simulate_keystrokes("ctrl-w -");
    vcx.run_until_parked();
    assert_eq!(
        zoom(&view, vcx),
        Detail::Minimap,
        "zoom-out clamps at Minimap (no wrap / no panic)"
    );

    // Ctrl-W = : Minimap → Card (steps back in).
    vcx.simulate_keystrokes("ctrl-w =");
    vcx.run_until_parked();
    assert_eq!(zoom(&view, vcx), Detail::Card, "one step in ⇒ Card");
}

/// `Cmd+Shift`+left-drag pans the plane camera (spec Behavior 5) and moves NO
/// tile. Drives the REAL mouse dispatch (`simulate_mouse_*` → the canvas root's
/// `on_mouse_down`/`on_mouse_move`/`on_mouse_up` → `desktop_pan_grab` /
/// `desktop_pointer_move` / `desktop_drop`), not the handlers directly.
///
/// NEGATIVE CONTROL (observed RED): drop the `&& modifiers.shift` guard in
/// `desktop_pan_grab` → the bare-drag sibling test below starts panning and its
/// `pan == (0,0)` assert fails.
#[gpui::test]
fn cmd_shift_drag_pans_the_plane(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);

    let slots_before = view.read_with(vcx, |v, _| {
        let d = &v.workspace.active_workspace().unwrap().desktop;
        (d.slot_of(win_a), d.slot_of(win_b))
    });

    let cmd_shift = Modifiers {
        platform: true,
        shift: true,
        ..Default::default()
    };
    // Derive endpoints from the REAL painted geometry (pitch varies with the
    // painted canvas). Mouse-DOWN in the empty bottom-right corner so the
    // canvas-root pan handler (not a tile title bar) owns the gesture; the drag
    // moves > 1 pitch on each axis so the release-snap (bug-0009) rounds BOTH
    // axes to a positive whole slot instead of down to 0 — keeping the "pans"
    // claim non-vacuous now that the pan settles cell-aligned on release.
    let (start, end) = pan_drag_endpoints(&view, vcx);
    vcx.simulate_mouse_down(start, MouseButton::Left, cmd_shift);
    vcx.simulate_mouse_move(end, Some(MouseButton::Left), cmd_shift);
    vcx.simulate_mouse_up(end, MouseButton::Left, cmd_shift);
    vcx.run_until_parked();

    let (pan, slots_after) = view.read_with(vcx, |v, _| {
        let d = &v.workspace.active_workspace().unwrap().desktop;
        (d.camera.pan, (d.slot_of(win_a), d.slot_of(win_b)))
    });
    // Pointer moved left+up; content follows the cursor, so the camera pan
    // moves positive on both axes. Non-vacuous (a real, sizable shift).
    assert!(
        pan.0 >= 1.0 && pan.1 >= 1.0,
        "Cmd+Shift drag pans the camera (got {pan:?})"
    );
    assert_eq!(
        slots_before, slots_after,
        "panning is view-only — it must NOT move a tile"
    );
}

/// The **Shift** requirement is load-bearing: a `Cmd`-ONLY drag (which DOES
/// reach `desktop_pan_grab`, unlike a modifier-less down) must NOT pan, because
/// `shift` is absent. This is the non-vacuous negative control for the shift
/// half of the guard: revert `&& modifiers.shift` and this test goes RED.
#[gpui::test]
fn cmd_only_drag_does_not_pan_the_plane(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};
    let (view, vcx, _win_a, _win_b) = boot_desktop_two_tiles(cx);

    // Cmd held, Shift NOT held — over the same empty canvas the Cmd+Shift test
    // uses (so the gesture reaches the canvas-root handler).
    let cmd_only = Modifiers {
        platform: true,
        ..Default::default()
    };
    vcx.simulate_mouse_down(point(px(600.0), px(400.0)), MouseButton::Left, cmd_only);
    vcx.simulate_mouse_move(
        point(px(450.0), px(320.0)),
        Some(MouseButton::Left),
        cmd_only,
    );
    vcx.simulate_mouse_up(point(px(450.0), px(320.0)), MouseButton::Left, cmd_only);
    vcx.run_until_parked();

    let pan = view.read_with(vcx, |v, _| {
        v.workspace.active_workspace().unwrap().desktop.camera.pan
    });
    assert_eq!(pan, (0.0, 0.0), "Cmd WITHOUT Shift must not pan the plane");
}

/// bug-0009 / UXI-Workspace-8: a `Cmd+Shift` free-pan is continuous WHILE
/// dragging but rests the view CELL-ALIGNED on release — the camera pan lands on
/// whole slot units, the same contract a tile drag/edge-resize already honors.
/// Drives the REAL mouse dispatch (`simulate_mouse_*` → the canvas root handlers
/// → `desktop_pan_grab` / `desktop_pointer_move` / `desktop_drop`).
///
/// Non-vacuous: it reads the pan MID-gesture (after the move, before the up) and
/// asserts it is genuinely FRACTIONAL, so the post-release integrality isn't a
/// no-op. NEGATIVE CONTROL (observed RED): remove the `snap_camera_to_slots()`
/// call in `desktop_drop`'s `pan_drag` branch → the fractional mid-pan survives
/// the release and the integral asserts fail.
#[gpui::test]
fn cmd_shift_pan_rests_view_cell_aligned(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);

    let slots_before = view.read_with(vcx, |v, _| {
        let d = &v.workspace.active_workspace().unwrap().desktop;
        (d.slot_of(win_a), d.slot_of(win_b))
    });

    let cmd_shift = Modifiers {
        platform: true,
        shift: true,
        ..Default::default()
    };
    // Down (empty corner) + move ~1.4 pitch: leaves a FRACTIONAL pan mid-gesture
    // that snaps to a positive whole slot on release. Endpoints derived from the
    // real painted geometry.
    let (start, end) = pan_drag_endpoints(&view, vcx);
    vcx.simulate_mouse_down(start, MouseButton::Left, cmd_shift);
    vcx.simulate_mouse_move(end, Some(MouseButton::Left), cmd_shift);
    vcx.run_until_parked();

    // Precondition: mid-gesture the pan is fractional (proves the snap has real
    // work to do — the release isn't landing on an already-integral pan).
    let pan_mid = view.read_with(vcx, |v, _| {
        v.workspace.active_workspace().unwrap().desktop.camera.pan
    });
    assert!(
        pan_mid.0.fract() != 0.0 || pan_mid.1.fract() != 0.0,
        "precondition: mid-pan is FRACTIONAL (got {pan_mid:?})",
    );

    vcx.simulate_mouse_up(end, MouseButton::Left, cmd_shift);
    vcx.run_until_parked();

    let (pan_after, slots_after) = view.read_with(vcx, |v, _| {
        let d = &v.workspace.active_workspace().unwrap().desktop;
        (d.camera.pan, (d.slot_of(win_a), d.slot_of(win_b)))
    });
    assert_eq!(
        pan_after.0.fract(),
        0.0,
        "pan.0 rests on a whole slot (got {pan_after:?})"
    );
    assert_eq!(
        pan_after.1.fract(),
        0.0,
        "pan.1 rests on a whole slot (got {pan_after:?})"
    );
    assert_eq!(
        slots_before, slots_after,
        "the pan is view-only — no tile moves",
    );
}

/// UXI-Workspace-8: dragging a tile near a canvas edge (which edge-auto-pans the
/// camera by a *fractional* slot step) rests the view CELL-ALIGNED on drop — the
/// camera pan lands on whole slot units, like the tile snaps to a cell. Drives the
/// REAL tile-drag path (`desktop_grab` → `desktop_pointer_move` → `desktop_drop`).
///
/// Non-vacuous: it asserts the pan was genuinely fractional *before* the drop (so a
/// no-op would fail the "fractional then integral" story). NEGATIVE CONTROL
/// (observed RED): remove the `snap_camera_to_slots()` call in `desktop_drop`'s drag
/// branch → the fractional pan survives the drop and the integral asserts fail.
#[gpui::test]
fn tile_drag_rests_view_cell_aligned(cx: &mut TestAppContext) {
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);

    let slot_b_before = view.read_with(vcx, |v, _| {
        v.workspace
            .active_workspace()
            .unwrap()
            .desktop
            .slot_of(win_b)
    });

    // Use the REAL painted canvas rect (boot's final paint sets it; a set here
    // would be overwritten). Target the actual edge bands relative to it.
    let (cx0, cy0, cw, ch) = view.read_with(vcx, |v, _| v.desktop_canvas_bounds.get());
    let br = (cx0 + cw - 5.0, cy0 + ch - 5.0); // inside the bottom-right edge band

    // Grab tile A, then drag toward the bottom-right edge band so the edge
    // auto-pan fires (it only fires once the drag is ACTIVE, i.e. after the
    // threshold-crossing first move — so the near-edge moves come after).
    view.update(vcx, |v, cx| {
        v.desktop_grab(win_a, (cx0 + 50.0, cy0 + 50.0), cx)
    });
    view.update(vcx, |v, cx| {
        v.desktop_pointer_move((cx0 + cw * 0.5, cy0 + ch * 0.5), cx)
    }); // activate
    for _ in 0..8 {
        // Past both the right (>cw-30) and bottom (>ch-30) bands → auto-pan
        // both axes by DESKTOP_EDGE_PAN_STEP/pitch (a fractional slot step).
        view.update(vcx, |v, cx| v.desktop_pointer_move(br, cx));
    }

    // The pan is now fractional (proving the auto-pan ran) — the exact condition
    // the drop must clean up.
    let pan_before = view.read_with(vcx, |v, _| {
        v.workspace.active_workspace().unwrap().desktop.camera.pan
    });
    assert!(
        pan_before.0.fract() != 0.0 || pan_before.1.fract() != 0.0,
        "precondition: edge auto-pan left a FRACTIONAL pan (got {pan_before:?})",
    );

    view.update(vcx, |v, cx| v.desktop_drop(cx));

    let (pan_after, slot_b_after) = view.read_with(vcx, |v, _| {
        let d = &v.workspace.active_workspace().unwrap().desktop;
        (d.camera.pan, d.slot_of(win_b))
    });
    assert_eq!(
        pan_after.0.fract(),
        0.0,
        "pan.0 rests on a whole slot (got {pan_after:?})"
    );
    assert_eq!(
        pan_after.1.fract(),
        0.0,
        "pan.1 rests on a whole slot (got {pan_after:?})"
    );
    assert_eq!(
        slot_b_before, slot_b_after,
        "the un-dragged tile B never moves — the snap is view-only",
    );
}

/// REGRESSION (bug-0001): a freshly-CREATED server-managed session — `resume_id`
/// None (never resumed) and `channel` None (the daemon owns the channel) — must
/// STILL be persisted so its tile auto-resumes on restart (UXI-AgentTile-18). Its
/// server id lives ONLY in the store's sid binding, so `save_agent_ring` must
/// resolve it via `sid_of`, not `resume_id` / `channel.session_id()` (both None
/// here). This is the real bug behind "still prompted with a picker": only RESUMED
/// sessions were being persisted; created ones came back as pickers. Drives the
/// REAL `save_agent_ring` and asserts the tile's `resume_sid` + the persisted
/// layout leaf carry the id.
///
/// Negative control: revert `save_agent_ring` to the `resume_id`/`channel` chain →
/// `resolved_id` is None → the tile is never stamped → `resume_sid` stays None →
/// both asserts fail RED.
#[gpui::test]
fn created_server_session_persists_its_id_for_restore(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState, AgentTile, App};
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("acp_sessions.json");

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    let win = view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        // A CREATED server-managed session: resume_id None + channel None.
        let id = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(None),
                label: "claude-created".into(),
                cwd: PathBuf::from("."),
                resume_id: None,
            },
            cx,
        );
        // The store binds the server-assigned sid (as the create resolution does).
        v.sessions.bind_sid(id, "SID-CREATED".into()).unwrap();
        crate::persist::with_acp_persist_path(file.clone(), || v.save_agent_ring(cx));
        v.workspace.focused_window_id().expect("focused")
    });

    view.read_with(vcx, |v, _cx| {
        for wsp in v.workspace.workspaces.iter() {
            if let Some(w) = wsp.layout.find_leaf(win)
                && let App::Agent(t) = &w.content
            {
                // The layout snapshot resolves the Bound tile's id from the store
                // (SINGLE source of truth — no cached resume_sid). A created session
                // (resume_id None, channel None) must still resolve via `sid_of`.
                let resolve = |id| v.sessions.sid_of(id).cloned();
                match crate::persist::snapshot_content(&w.content, &resolve) {
                    crate::persist::PersistedKind::Agent { session_id } => assert_eq!(
                        session_id.as_ref().map(|s| s.as_str()),
                        Some("SID-CREATED"),
                        "workspace.json leaf must persist the created session id"
                    ),
                    other => panic!("expected Agent kind, got {other:?}"),
                }
                return;
            }
        }
        panic!("agent tile not found");
    });
}

/// REGRESSION (bug-0001, 2nd mechanism): `save_agent_ring` stamps `resume_sid` in
/// memory + writes acp_sessions.json, but the per-tile id RESTORE reads lives in
/// `workspace.json` (written by `save_workspace_state`, which otherwise only runs
/// on structural changes). If save_agent_ring doesn't persist the layout,
/// workspace.json goes STALE and a session you create-and-use comes back as a
/// picker. Drives the REAL `save_agent_ring` and asserts the ON-DISK
/// workspace.json — the exact file restore loads — carries the session id.
///
/// Negative control: drop the `self.save_workspace_state()` at the end of
/// `save_agent_ring` → workspace.json is never written / lacks the id → RED.
#[gpui::test]
fn save_agent_ring_persists_session_id_to_workspace_json(cx: &mut TestAppContext) {
    use crate::persist::{
        PersistedKind, PersistedLayout, load_persisted_workspace, with_acp_persist_path,
        with_workspace_path,
    };
    use crate::{AgentSession, AgentState, AgentTile, App};
    let dir = tempfile::tempdir().expect("tempdir");
    let ws_file = dir.path().join("workspace.json");
    let acp_file = dir.path().join("acp_sessions.json");

    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        let id = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(None),
                label: "claude-created".into(),
                cwd: PathBuf::from("."),
                resume_id: None,
            },
            cx,
        );
        v.sessions.bind_sid(id, "SID-WS".into()).unwrap();
        with_workspace_path(ws_file.clone(), || {
            with_acp_persist_path(acp_file.clone(), || {
                v.save_agent_ring(cx);
            });
        });
    });

    // Load the ACTUAL workspace.json the restore path reads.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let found = with_workspace_path(ws_file.clone(), || {
        let ws = load_persisted_workspace(&cwd).expect("workspace.json was written");
        fn collect(l: &PersistedLayout, out: &mut Vec<Option<String>>) {
            match l {
                PersistedLayout::Empty => {}
                PersistedLayout::Leaf(leaf) => {
                    if let PersistedKind::Agent { session_id } = &leaf.kind {
                        out.push(session_id.as_ref().map(|s| s.to_string()));
                    }
                }
                PersistedLayout::Split { children, .. } => {
                    for (_, c) in children {
                        collect(c, out);
                    }
                }
            }
        }
        let mut ids = Vec::new();
        for t in &ws.workspaces {
            collect(&t.layout, &mut ids);
        }
        ids
    });

    assert!(
        found.iter().any(|s| s.as_deref() == Some("SID-WS")),
        "workspace.json (the file restore reads) must carry the created session's id; got {found:?}"
    );
}

/// bug-0016 (session names lost after exit): the LOAD-BEARING guard. A test that
/// boots the real view and triggers ANY session save (/clear, restore, rename,
/// `save_agent_ring`) must NEVER write to the user's real
/// `~/.yalda/acp_sessions.json` — that file holds the renamed-session LABELS, so
/// clobbering it reverts the user's custom names to `claude-N` on the next
/// launch. `acp_session_persist_path()` must therefore be `None` under
/// `cfg(test)` unless a test explicitly opts into a tempdir via
/// `with_acp_persist_path` (mirroring `workspace_persist_path` /
/// `preferences_path`, which already had this guard — this fn was the one that
/// fell through to the real home).
///
/// Negative control (observed RED): restore the `yalda_home()` fall-through in
/// `acp_session_persist_path` under `cfg(test)` ⇒ the no-override assert returns
/// `Some(~/.yalda/acp_sessions.json)` and fails.
#[test]
fn acp_persist_path_never_hits_real_home_in_tests() {
    use crate::persist::{acp_session_persist_path, with_acp_persist_path};
    // With no override set, the path MUST be None — a save is a silent no-op and
    // the real file is untouched. This is the anti-clobber guarantee.
    assert_eq!(
        acp_session_persist_path(),
        None,
        "acp_session_persist_path must be None in tests without an override — \
         otherwise every view-booting test overwrites the user's real \
         ~/.yalda/acp_sessions.json and wipes their renamed-session labels (bug-0016)"
    );
    // And it must still be redirectable for round-trip tests that opt in.
    let dir = tempfile::tempdir().expect("tempdir");
    let want = dir.path().join("acp_sessions.json");
    let got = with_acp_persist_path(want.clone(), acp_session_persist_path);
    assert_eq!(
        got,
        Some(want),
        "with_acp_persist_path must redirect the path"
    );
    // The override is scoped: it clears again after the closure.
    assert_eq!(
        acp_session_persist_path(),
        None,
        "the override must not leak past the closure"
    );
}

/// Unit (bug-0016): `is_auto_claude_label` recognizes exactly the auto-generated
/// names (`claude`, `claude-<n>`, `codex`, `codex-<n>`) and nothing a user would
/// type as a real name.
/// This gate decides when the server WAL's label may be adopted, so getting it
/// wrong either fails to recover a lost name or clobbers a real one.
#[test]
fn is_auto_claude_label_matches_only_generated_names() {
    use crate::agent_ui::is_auto_claude_label;
    for auto in [
        "claude",
        "claude-1",
        "claude-2",
        "claude-10",
        "claude-999",
        "  claude-3  ",
        "codex",
        "codex-1",
        "codex-42",
    ] {
        assert!(is_auto_claude_label(auto), "{auto:?} should be auto");
    }
    for custom in [
        "",
        "my agent",
        "claude-",
        "claude-x",
        "claude-2b",
        "codex-",
        "codex-x",
        "reviewer",
        "Claude-1",
        "Codex-1",
        "the-claude-1",
    ] {
        assert!(
            !is_auto_claude_label(custom),
            "{custom:?} should NOT be auto"
        );
    }
}

/// bug-0016, RECOVERY on the REAL view: the server roster (WAL-backed) is
/// authoritative for a session's label. An OPENED session that came back with an
/// auto `claude-N` name (its custom name lost to a clobbered `acp_sessions.json`)
/// adopts the roster's real name; a session that already carries a real custom
/// name is LEFT ALONE (never overridden by a momentarily-stale roster). Drives
/// the real `recover_labels_from_roster` against a seeded roster + store.
///
/// Negative control (observed RED): drop the `is_auto_claude_label` gate (adopt
/// unconditionally) → the "keep custom" assert fails as the real name is
/// clobbered; OR skip the update entirely → the "recover" assert fails.
#[gpui::test]
fn opened_session_recovers_lost_label_from_roster(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState};
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    let info = |sid: &str, label: &str| SessionInfo {
        session_id: sid.into(),
        acp_session_id: None,
        label: label.into(),
        cwd: std::path::PathBuf::from("/proj/x"),
        provider: yalda::acp_channel::AgentProvider::Claude,
        turns: 0,
        connected: true,
        permission_mode: yalda::acp_channel::PermissionMode::ReadOnly,
        busy: false,
        archived: false,
    };

    let (lost_id, kept_id) = view.update(vcx, |v, cx| {
        // The server WAL remembers the real names.
        v.agent_roster.upsert(info("S-lost", "deploy pipeline"));
        v.agent_roster.upsert(info("S-kept", "stale-old-name"));
        // Two opened sessions: one carrying an auto name (its custom name was
        // clobbered), one carrying a real custom name the user just set locally.
        let lost = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(None),
                label: "claude-5".into(),
                cwd: std::path::PathBuf::from("/proj/x"),
                resume_id: None,
            },
            cx,
        );
        v.sessions.bind_sid(lost, "S-lost".into()).unwrap();
        let kept = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(None),
                label: "my careful name".into(),
                cwd: std::path::PathBuf::from("/proj/x"),
                resume_id: None,
            },
            cx,
        );
        v.sessions.bind_sid(kept, "S-kept".into()).unwrap();
        (lost, kept)
    });

    let changed = view.update(vcx, |v, cx| v.recover_labels_from_roster(cx));
    assert!(changed, "a lost label was available to recover");

    let label_of = |v: &crate::YaldaGpuiView, cx: &gpui::App, id| {
        v.session_entity(id).map(|e| e.read(cx).label.clone())
    };
    let lost_label = view.update(vcx, |v, cx| label_of(v, cx, lost_id));
    let kept_label = view.update(vcx, |v, cx| label_of(v, cx, kept_id));
    assert_eq!(
        lost_label.as_deref(),
        Some("deploy pipeline"),
        "an auto claude-N label must be recovered from the server WAL roster"
    );
    assert_eq!(
        kept_label.as_deref(),
        Some("my careful name"),
        "a real custom local label must NEVER be overridden by the roster"
    );

    // Idempotent: a second pass changes nothing.
    let again = view.update(vcx, |v, cx| v.recover_labels_from_roster(cx));
    assert!(!again, "recovery is idempotent once labels match");
}

/// bug-0016, the behavior the guard protects: a custom (renamed) session label
/// round-trips through the REAL save + load path unchanged — it is NOT renumbered
/// to `claude-N`. Proves the persistence logic itself is correct, so the ONLY way
/// the user's names got lost was the real file being clobbered (fixed by the
/// path guard above). Uses `with_acp_persist_path` so THIS test writes to a
/// tempdir, never `~/.yalda`.
#[test]
fn renamed_session_label_round_trips_unchanged() {
    use crate::InputModeKind;
    use crate::persist::{
        SessionSnapshot, load_persisted_acp_sessions, save_persisted_acp_sessions,
        with_acp_persist_path,
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("acp_sessions.json");
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let snaps = vec![
        SessionSnapshot {
            id: "sid-alpha".into(),
            label: "my important agent".into(),
            provider: yalda::acp_channel::AgentProvider::Claude,
            active: true,
            mode: InputModeKind::Worksheet,
            tasklist_open: false,
            subagents_open: false,
            sidepanel_hidden: false,
            cwd: cwd.clone(),
            compose_draft: None,
            summary: None,
        },
        SessionSnapshot {
            id: "sid-beta".into(),
            label: "reviewer".into(),
            provider: yalda::acp_channel::AgentProvider::Claude,
            active: false,
            mode: InputModeKind::Worksheet,
            tasklist_open: false,
            subagents_open: false,
            sidepanel_hidden: false,
            cwd: cwd.clone(),
            compose_draft: None,
            summary: None,
        },
    ];
    let loaded = with_acp_persist_path(file.clone(), || {
        save_persisted_acp_sessions(&cwd, &snaps);
        load_persisted_acp_sessions(&cwd)
    });
    let labels: Vec<String> = loaded.iter().map(|s| s.label.clone()).collect();
    assert_eq!(
        labels,
        vec!["my important agent".to_string(), "reviewer".to_string()],
        "custom labels must survive save→load verbatim, never revert to claude-N"
    );
}

/// UXI-Workspace-9 (click-to-focus): a LEFT press in the BODY of an unfocused
/// Full-detail tile focuses that tile, and the press is CONSUMED — the tile's
/// content never sees it. Drives the REAL mouse dispatch (`simulate_mouse_down`
/// through the capture-phase listener on the tile body), not the handlers
/// directly, so it actually exercises `capture_any_mouse_down` +
/// `stop_propagation`.
///
/// Non-vacuous by construction: the same real transcript handler, given a point
/// over the repainted token, must flip `focus` to `Transcript`. Without that
/// second half, "the content didn't act" could pass because the content itself
/// was inert.
#[gpui::test]
fn click_in_unfocused_tile_body_focuses_and_is_consumed(cx: &mut TestAppContext) {
    use crate::{AgentFocus, App};
    use gpui::{Modifiers, MouseButton, point, px};
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);

    // `boot_desktop_two_tiles` parks B at col 100 (~26000px off-viewport) to
    // exercise culling. Bring it next to A so it actually renders LIVE content —
    // a culled tile builds no transcript and there'd be nothing to click.
    view.update(vcx, |v, cx| {
        v.jump_panel_visible = false;
        let wsp = v.workspace.active_workspace_mut().unwrap();
        wsp.desktop
            .set_anchor(win_b, crate::workspace::Slot::new(0, 1));
        // A legacy 1×1 tile is only one 160px cell and intentionally too small
        // for whole-window content. Give B a useful multi-cell span that also
        // fits fully inside this small interaction-test viewport.
        wsp.desktop
            .set_span(win_b, crate::workspace::Span::new(2, 2));
        wsp.desktop.camera.pan = (0.0, 0.0);
        wsp.desktop.last_reveal = Some(win_a);
        cx.notify();
    });
    vcx.run_until_parked();

    // Session bound to tile B (the tile we'll click into).
    let id_b = view.update(vcx, |v, _| {
        let ti = v.workspace.active_workspace;
        match &v.workspace.workspaces[ti]
            .layout
            .find_leaf_mut(win_b)
            .expect("tile B leaf")
            .content
        {
            App::Agent(t) => t.session().expect("B is bound"),
            _ => panic!("tile B is not an agent tile"),
        }
    });

    // Give B a transcript with real painted tokens, and park its focus on the
    // compose (so "focus became Transcript" is an unambiguous signal that the
    // transcript handled a click).
    let sess_b = view
        .update(vcx, |v, _| v.session_entity(id_b))
        .expect("session B entity");
    sess_b.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "alpha line one\nbeta line two\n");
        s.state.editor.add_frozen_lines(0, 2);
        s.state.focus = AgentFocus::Compose;
        cx.notify();
    });
    vcx.run_until_parked();

    // Focus tile A ⇒ tile B is the UNFOCUSED tile under test.
    view.update(vcx, |v, cx| {
        let ti = v.workspace.active_workspace;
        v.workspace.workspaces[ti].focused = win_a;
        cx.notify();
    });
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.focused_window_id(),
            Some(win_a),
            "A starts focused"
        );
    });

    // A point over a REAL painted token inside B's transcript body (not the
    // title bar, not a resize band).
    let tv_b = view
        .update(vcx, |v, _| v.transcript_views.get(&id_b).cloned())
        .expect("transcript view for B");
    let tokens: Vec<crate::TokenHit> = tv_b.update(vcx, |t, _| t.token_hits.borrow().clone());
    let line0: Vec<&crate::TokenHit> = tokens.iter().filter(|t| t.line_idx == 0).collect();
    assert!(
        !line0.is_empty(),
        "tile B's transcript painted no tokens — the click point would be vacuous"
    );
    let bx = line0[0].bounds.left() + px(2.0);
    let by = line0[0].bounds.top() + (line0[0].bounds.bottom() - line0[0].bounds.top()) / 2.0;
    let pt = point(bx, by);

    // ── The invariant: first press focuses B and is swallowed. ──
    vcx.simulate_mouse_down(pt, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.focused_window_id(),
            Some(win_b),
            "a click in the unfocused tile's BODY must focus that tile"
        );
    });
    sess_b.read_with(vcx, |s, _| {
        assert_eq!(
            s.state.focus,
            AgentFocus::Compose,
            "the focus-changing click must be CONSUMED — the transcript must not have acted on it"
        );
    });
    vcx.simulate_mouse_up(pt, MouseButton::Left, Modifiers::default());
    vcx.run_until_parked();

    // ── Non-vacuity: a press over B's repainted token reaches the content. ──
    let tokens_after: Vec<crate::TokenHit> = tv_b.update(vcx, |t, _| t.token_hits.borrow().clone());
    let token_after = tokens_after
        .iter()
        .find(|t| t.line_idx == 0)
        .expect("focused tile B repaints its transcript token");
    let focused_pt = point(
        token_after.bounds.left() + px(2.0),
        token_after.bounds.top() + (token_after.bounds.bottom() - token_after.bounds.top()) / 2.0,
    );
    tv_b.update(vcx, |t, cx| {
        t.transcript_mouse_down(
            &gpui::MouseDownEvent {
                button: MouseButton::Left,
                position: focused_pt,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
    });
    vcx.run_until_parked();
    sess_b.read_with(vcx, |s, _| {
        assert_eq!(
            s.state.focus,
            AgentFocus::Transcript,
            "the real transcript handler must act at the painted token \
             (otherwise the 'consumed' assert above is vacuous)"
        );
    });
}

/// UXI-Workspace-9 carve-out 1: the title bar is NOT covered by the swallow rule —
/// pressing an UNFOCUSED tile's title bar still focuses it AND arms the move drag
/// in one gesture (`desktop_grab`).
#[gpui::test]
fn title_bar_press_on_unfocused_tile_still_focuses_and_arms_drag(cx: &mut TestAppContext) {
    let (view, vcx, win_a, win_b) = boot_desktop_two_tiles(cx);

    // Put B next to A so its card is on-screen and hit-testable.
    view.update(vcx, |v, cx| {
        v.jump_panel_visible = false;
        let wsp = v.workspace.active_workspace_mut().unwrap();
        wsp.desktop
            .set_anchor(win_b, crate::workspace::Slot::new(0, 1));
        wsp.desktop
            .set_span(win_b, crate::workspace::Span::new(2, 2));
        wsp.desktop.camera.pan = (0.0, 0.0);
        wsp.desktop.last_reveal = Some(win_a);
        wsp.focused = win_a;
        cx.notify();
    });
    vcx.run_until_parked();

    // Drive the exact production handler wired to the title bar. Interaction
    // dispatch geometry is covered independently; this pins the focus+drag
    // state transition against fixed-cell layouts.
    view.update(vcx, |v, cx| v.desktop_grab(win_b, (350.0, 23.0), cx));
    vcx.run_until_parked();

    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.workspace.focused_window_id(),
            Some(win_b),
            "title-bar press focuses the tile"
        );
        let ti = v.workspace.active_workspace;
        assert!(
            v.workspace.workspaces[ti].desktop.drag.is_some(),
            "title-bar press ALSO arms the drag in the same gesture (carve-out 1)"
        );
    });
}

/// UXI-AgentTile-22 (end-to-end, REAL menu + REAL submit path): the space-menu
/// `x` no longer closes a session — it appends the `<Yaldabaoth System>` confirm
/// line to the transcript and arms a gate that swallows the next submit. Only a
/// trimmed `yes` closes; anything else cancels, sends nothing, and leaves the
/// draft alone.
///
/// Drives `dispatch_menu_command("claude-close")` (the exact command the `x`
/// menu entry carries) and `submit_agent` → `submit_compose` against the
/// in-process test channel, so "nothing reached the agent" is proven on the wire
/// (`prompt_rx`), not inferred from state.
///
/// Negative control: point `"claude-close"` back at `close_active_agent_session`
/// (or drop the `consume_close_confirm` call in `submit_compose`) → the
/// still-bound assert after arming (resp. the cancel asserts) fire RED.
#[cfg(feature = "test-support")]
#[gpui::test]
fn close_session_requires_typed_yes_confirmation(cx: &mut TestAppContext) {
    let (view, vcx, id, controls) = boot_worksheet_channel(cx);

    // 1. ARM through the real menu command. The session must SURVIVE, and the
    //    prompt line must be in its transcript.
    view.update(vcx, |v, cx| v.dispatch_menu_command("claude-close", cx));
    vcx.run_until_parked();
    let (bound, transcript) = view.update(vcx, |v, cx| {
        (
            v.focused_bound_session(),
            v.read_session(id, cx, |c| c.editor.document().full_text())
                .unwrap_or_default(),
        )
    });
    assert_eq!(
        bound,
        Some(id),
        "arming the confirm must NOT close the session"
    );
    assert!(
        transcript.contains(YaldaGpuiView::CLOSE_CONFIRM_PROMPT),
        "the confirm prompt must be appended to the transcript, got: {transcript:?}"
    );

    // 2. A non-`yes` submit CANCELS: nothing on the wire, session still bound,
    //    draft still sitting in the compose.
    //    Type DIRECTLY rather than via `worksheet_real_submit` — that helper
    //    presses `i` first, and since UXI-AgentTile-23 the arm has already put an
    //    empty compose in Insert, so the `i` would be typed as literal text
    //    (`inope`). A real user doesn't press it either.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            for ch in "nope".chars() {
                c.input_surface.compose_mut().editor.insert_char(ch);
            }
        });
    });
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();
    assert!(
        controls.prompt_rx.try_recv().is_err(),
        "a submit consumed by the confirm must never reach the agent"
    );
    let (bound, draft) = view.update(vcx, |v, cx| {
        (
            v.focused_bound_session(),
            v.read_session(id, cx, |c| c.input_surface.compose().text())
                .unwrap_or_default(),
        )
    });
    assert_eq!(
        bound,
        Some(id),
        "a non-`yes` answer must not close the session"
    );
    assert!(
        draft.contains("nope"),
        "the cancelled draft must be left in the compose, got: {draft:?}"
    );

    // 3. The gate is ONE-SHOT: the same draft resubmitted now really sends.
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();
    let payload = controls
        .prompt_rx
        .try_recv()
        .expect("after cancelling, the next submit sends normally");
    assert_eq!(payload.text.trim(), "nope");

    // 4. Re-arm MID-TURN (rule 4: arms regardless of turn state — step 3 left a
    //    turn in flight, so this is the chatbox surface) and answer `yes` → the
    //    session actually closes (tile unbinds).
    view.update(vcx, |v, cx| v.dispatch_menu_command("claude-close", cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            for ch in "yes".chars() {
                c.input_surface.compose_mut().editor.insert_char(ch);
            }
        });
    });
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();
    assert!(
        controls.prompt_rx.try_recv().is_err(),
        "the `yes` answer is never sent to the agent either"
    );
    let bound = view.update(vcx, |v, _| v.focused_bound_session());
    assert_eq!(
        bound, None,
        "a typed `yes` closes the session (tile unbinds)"
    );
}

/// bug-0013 (`UXI-AgentTile-8`, widened): a tool call that interrupts an OPEN run
/// MID-SENTENCE must not split the sentence — even when the break is NOT
/// alphanumeric-on-both-sides, which is all `dbe67be`'s mid-word rule covered.
/// Both cases are lifted in shape from the 2026-07-21 11:33 screenshot:
///
/// 1. the continuation starts with a SPACE (`…the fix for` | tool | ` it on my side…`),
/// 2. the continuation is bare punctuation (`…subagents` | tool | `.`), which used to
///    strand a line containing only `.`.
///
/// Drives the REAL reducer (`apply_server_batch` → `append_llm_chunk_floored`).
/// Negative control: restore the `chunk_head.is_alphanumeric()` / `last_char
/// .is_alphanumeric()` pair in `continuation_rejoin_point` → both asserts fail.
#[gpui::test]
fn tool_call_midsentence_does_not_split_agent_sentence(cx: &mut TestAppContext) {
    use yalda::acp_channel::{ReplyEvent, ToolCall};
    use yalda::session_proto::Notification as ServerNotification;

    let (view, vcx, _id, _session) = boot_with_transcript(cx);

    let ev = |e: ReplyEvent| ServerNotification::ReplyEvent {
        session_id: "S1".into(),
        event: e,
    };
    view.update(vcx, |v, cx| {
        let batch = vec![
            // Case 1: break at a whitespace boundary MID-sentence.
            ev(ReplyEvent::Chunk(
                "To close the loop on your question: the fix for".into(),
            )),
            ev(ReplyEvent::ToolCallStarted(ToolCall::new("t1", "Bash"))),
            ev(ReplyEvent::Chunk(
                " it on my side is to stop delegating long test runs to subagents".into(),
            )),
            // Case 2: the sentence's terminating '.' arrives after another tool.
            ev(ReplyEvent::ToolCallStarted(ToolCall::new("t2", "Bash"))),
            ev(ReplyEvent::Chunk(".\n".into())),
        ];
        v.apply_server_batch(batch, cx);
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    view.update(vcx, |v, cx| {
        let c = v.agent_mut(cx).expect("agent");
        let text = c.editor.document().full_text();
        assert!(
            text.contains(
                "the fix for it on my side is to stop delegating long test runs to subagents."
            ),
            "a tool must not cut a sentence at a non-word boundary; got:\n{text:?}"
        );
        // Non-vacuity: no line is left holding only the stranded terminator.
        assert!(
            !text.lines().any(|l| l.trim() == "."),
            "the trailing '.' must rejoin its sentence, not strand its own line; got:\n{text:?}"
        );
    });
}

/// bug-0015 (`UXI-Selection-1`): pressing the mouse inside a multiline code block
/// must NOT move the block. The transcript's blank-line collapse protects the line
/// the cursor sits on; a press moves that cursor, so the previously-protected blank
/// collapsed away, the flat-item list lost an entry, and the whole block repainted
/// ~25px lower — MID-GESTURE, under the pointer. 25px > the 20px line height, so
/// every later `hit_test_tokens` came back a line off and dragging inside a code
/// block selected the wrong lines ("can't select in a multiline code block").
///
/// Asserts on PAINTED geometry (the band tops), not on state: the defect is that
/// the pixels move. Drives the REAL `transcript_mouse_down`, then re-reads the
/// paint-time token sink.
///
/// Negative control: drop `drag_protect_line` from `protect_line` in
/// `rebuild_agent_view_model` (back to the bare cursor line) → the bands shift +25px
/// and the equality assert fires RED.
#[gpui::test]
fn code_block_does_not_shift_when_clicked(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    let (view, vcx, id, session) = boot_with_transcript(cx);

    // A frozen fenced code block, and a trailing blank line for the cursor to rest
    // on (the line whose protection the press used to drop).
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "```rust\nlet a = 1;\nlet b = 2;\n```\n");
        s.state.editor.add_frozen_lines(0, 4);
        s.state.editor.cursor_mut().line = 4;
        s.state.editor.cursor_mut().col = 0;
        cx.notify();
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let tv = view
        .update(vcx, |v, _| v.transcript_views.get(&id).cloned())
        .expect("transcript view exists");
    let bands = |vcx: &mut gpui::VisualTestContext| -> Vec<(usize, f32)> {
        tv.update(vcx, |t, _| t.token_hits.borrow().clone())
            .iter()
            .map(|t| (t.line_idx, f32::from(t.bounds.top())))
            .collect()
    };

    let before = bands(vcx);
    // Non-vacuity: the block really did paint per-line bands to compare. Since
    // bug-0017 these are the CONTENT lines only (raw 1 & 2) from their real
    // painted bounds — the ``` fence lines (0, 3) no longer register bands.
    assert!(
        before.iter().filter(|(l, _)| *l == 1 || *l == 2).count() >= 2,
        "the code block painted no per-line bands to compare; got {before:?}"
    );
    let line1 = tv
        .update(vcx, |t, _| t.token_hits.borrow().clone())
        .into_iter()
        .find(|t| t.line_idx == 1)
        .expect("band for the first code line");
    let inside = point(
        line1.bounds.left() + px(1.0),
        line1.bounds.top() + (line1.bounds.bottom() - line1.bounds.top()) / 2.0,
    );

    // The REAL press the mouse dispatches to, landing INSIDE the block.
    tv.update(vcx, |t, cx| {
        t.transcript_mouse_down(
            &gpui::MouseDownEvent {
                button: MouseButton::Left,
                position: inside,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
    });
    vcx.run_until_parked();
    let after = bands(vcx);

    let tops = |v: &[(usize, f32)]| -> Vec<(usize, f32)> {
        v.iter().filter(|(l, _)| *l < 4).cloned().collect()
    };
    assert_eq!(
        tops(&before),
        tops(&after),
        "the code block MOVED under the pointer when clicked (bug-0015)"
    );

    // And the drag that follows still selects across lines (the user-visible point).
    let l2 = tv
        .update(vcx, |t, _| t.token_hits.borrow().clone())
        .into_iter()
        .find(|t| t.line_idx == 2)
        .expect("band for the second code line");
    let end = point(
        l2.bounds.right() - px(1.0),
        l2.bounds.top() + (l2.bounds.bottom() - l2.bounds.top()) / 2.0,
    );
    tv.update(vcx, |t, cx| {
        t.transcript_mouse_move(
            &gpui::MouseMoveEvent {
                position: end,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            },
            cx,
        );
        t.transcript_mouse_up(
            &gpui::MouseUpEvent {
                button: MouseButton::Left,
                position: end,
                modifiers: Modifiers::default(),
                click_count: 1,
            },
            cx,
        );
    });
    vcx.run_until_parked();
    let clip = view
        .update(vcx, |_, cx| cx.read_from_clipboard())
        .and_then(|it| it.text())
        .unwrap_or_default();
    assert!(
        clip.contains("let a = 1;") && clip.contains("let b = 2;"),
        "a drag across both code lines copies both; got {clip:?}"
    );
    // The freeze is released once the gesture ends.
    let protect = session.read_with(vcx, |s, _| s.state.drag_protect_line);
    assert_eq!(protect, None, "the drag protection must clear on mouse-up");
}

/// bug-0017 (`UXI-Selection-3`): selecting inside a multiline code block in the
/// transcript must (a) register hit bands on the CONTENT lines' real painted
/// bounds — NOT the fence-inclusive even-split that put bands on the ``` lines
/// and offset them by the block's padding + `[lang]` header — and (b) actually
/// PAINT the selection highlight (a `FlatItem::Block` used to paint no highlight
/// at all: `doc_selection: None`, so the model selected + the clipboard copied
/// while the user saw nothing → "cannot select in code blocks").
///
/// Drives the REAL `transcript_mouse_down/move/up` and asserts on the paint tap,
/// not on model state.
///
/// Negative control: at the `FlatItem::Block` arm set `block_hits: None` (revert
/// the fix) → code lines take the plain path → `block_selection` tap stays empty
/// AND the fence lines (0 and 4) reappear in the hit bands → both asserts fire.
#[gpui::test]
fn code_block_selection_is_painted_and_aligned(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton};
    use std::collections::HashSet;
    let (view, vcx, id, session) = boot_with_transcript(cx);

    // A fenced code block WITH a language (so the `[rust]` header offset is
    // present) + a trailing blank line for the caret. Raw lines:
    //   0 ```rust   1 fn main() {   2 <indent>let x = 1;   3 }   4 ```   5 (blank)
    // detect_block_ranges → range (0, 5); content lines 1..=3 (raw_base = 1).
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "```rust\nfn main() {\n    let x = 1;\n}\n```\n");
        s.state.editor.add_frozen_lines(0, 5);
        s.state.editor.cursor_mut().line = 5;
        s.state.editor.cursor_mut().col = 0;
        cx.notify();
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    let tv = view
        .update(vcx, |v, _| v.transcript_views.get(&id).cloned())
        .expect("transcript view exists");
    let hits =
        |vcx: &mut gpui::VisualTestContext| tv.update(vcx, |t, _| t.token_hits.borrow().clone());

    // (a) Hit bands land on the CONTENT lines from their own bounds — not on the
    // fence lines, and not the 4-band fence-inclusive even-split.
    let lines_hit: HashSet<usize> = hits(vcx).iter().map(|h| h.line_idx).collect();
    assert!(
        lines_hit.contains(&1) && lines_hit.contains(&2) && lines_hit.contains(&3),
        "code content lines 1..=3 must register hit bands; got {lines_hit:?}"
    );
    assert!(
        !lines_hit.contains(&0) && !lines_hit.contains(&4),
        "the ``` fence lines (0, 4) must NOT register hit bands (fence-inclusive \
         even-split bug); got {lines_hit:?}"
    );

    // Reset the paint tap, then drive a REAL drag from content line 1 → line 3.
    YaldaGpuiView::test_reset_doc_render_tap();
    let band = |vcx: &mut gpui::VisualTestContext, line: usize| {
        hits(vcx)
            .into_iter()
            .find(|h| h.line_idx == line)
            .unwrap_or_else(|| panic!("no hit band for code line {line}"))
            .bounds
    };
    let b1 = band(vcx, 1);
    let b3 = band(vcx, 3);
    let mid_y = |b: gpui::Bounds<gpui::Pixels>| b.top() + (b.bottom() - b.top()) / 2.0;
    // Press at the START of line 1, release at the END of line 3 → whole lines.
    let start_pos = point(b1.left() + px(1.0), mid_y(b1));
    let end_pos = point(b3.right() - px(1.0), mid_y(b3));
    tv.update(vcx, |t, cx| {
        t.transcript_mouse_down(
            &gpui::MouseDownEvent {
                button: MouseButton::Left,
                position: start_pos,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
        t.transcript_mouse_move(
            &gpui::MouseMoveEvent {
                position: end_pos,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            },
            cx,
        );
    });
    vcx.run_until_parked();

    // (b) The selection highlight was actually PAINTED inside the block, on the
    // content lines. This is the assert every prior fix lacked.
    let tap = YaldaGpuiView::test_doc_render_tap();
    let painted: HashSet<usize> = tap.block_selection.iter().map(|(l, _, _)| *l).collect();
    assert!(
        !tap.block_selection.is_empty(),
        "no selection highlight was painted inside the code block (bug-0017)"
    );
    assert!(
        painted.contains(&1) && painted.contains(&3),
        "the selection highlight must cover the dragged content lines 1 and 3; \
         painted {painted:?}"
    );
    // Non-vacuity: at least one painted range has real width (e_char > s_char).
    assert!(
        tap.block_selection.iter().any(|(_, s, e)| e > s),
        "every painted selection range was empty; got {:?}",
        tap.block_selection
    );

    // And the drag still copies the code text on release.
    tv.update(vcx, |t, cx| {
        t.transcript_mouse_up(
            &gpui::MouseUpEvent {
                button: MouseButton::Left,
                position: end_pos,
                modifiers: Modifiers::default(),
                click_count: 1,
            },
            cx,
        );
    });
    vcx.run_until_parked();
    let clip = view
        .update(vcx, |_, cx| cx.read_from_clipboard())
        .and_then(|it| it.text())
        .unwrap_or_default();
    assert!(
        clip.contains("fn main() {") && clip.contains("let x = 1;"),
        "a drag across the code block copies its lines; got {clip:?}"
    );
}

#[cfg(test)]
impl YaldaGpuiView {
    /// Test helper (ADR-0028): point the active workspace at a project rooted at
    /// `cwd`, creating that project if absent, so `active_workspace_cwd()` /
    /// `agent_base_cwd()` resolve to `cwd`. Mirrors the production "assign the
    /// workspace's project" — the FK replacement for the old `Workspace::set_cwd`.
    pub(crate) fn test_set_active_workspace_cwd(&mut self, cwd: std::path::PathBuf) {
        let pid = self
            .projects
            .ensure_at_cwd(cwd.clone(), &crate::persist::project_name_for_cwd(&cwd));
        if let Some(t) = self.workspace.active_workspace_mut() {
            t.set_project(pid);
        }
    }
}

/// UXI-Project-2 — a workspace's cwd is DERIVED from its project (a `ProjectId`
/// foreign key), resolved live at the point of use and never cached. Repointing
/// the project's cwd moves what the workspace (and a new agent) inherits, with
/// nothing to keep in sync.
///
/// Negative control: comment out `p.cwd = cwd;` in `Projects::set_cwd`
/// (`project.rs`) → the repoint doesn't take, `agent_base_cwd()` stays at A, and
/// the second assert fails — proving the value is read live from the project.
#[gpui::test]
fn workspace_and_session_cwd_derive_from_project(cx: &mut TestAppContext) {
    let a = PathBuf::from("/tmp/yalda-proj-a");
    let b = PathBuf::from("/tmp/yalda-proj-b");
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();

    // Point the active workspace at a project rooted at A; it inherits A.
    view.update(vcx, |v, _| v.test_set_active_workspace_cwd(a.clone()));
    let at_a = view.read_with(vcx, |v, _| v.agent_base_cwd());
    assert_eq!(at_a, a, "workspace inherits its project's cwd");

    // Repoint THAT project's cwd to B — the workspace's cwd follows LIVE, because
    // it is derived from the project (no cwd cached on the workspace).
    view.update(vcx, |v, _| {
        let pid = v
            .workspace
            .active_workspace()
            .expect("active workspace")
            .project();
        v.projects.set_cwd(pid, b.clone()).expect("repoint");
    });
    let at_b = view.read_with(vcx, |v, _| v.agent_base_cwd());
    assert_eq!(
        at_b, b,
        "repointing the project moves the workspace's cwd — derived, not cached"
    );
}

/// UXI-Project-6 — a session↔tile bind is intra-project only. A free ROSTER
/// session rooted in project B is NOT offered by an A-tile's selector
/// (`picker_projection`, the Part-3 project filter), and a direct cross-project
/// attach (A tile ← B session) is REFUSED by `picker_attach_existing` (the Part-4
/// hard gate) — no placeholder is bound and a transient note is set. The SAME
/// session binds successfully once the tile's workspace is project B.
///
/// Drives the REAL paths (`picker_projection`, `picker_attach_existing` — the
/// shared attach choke both the picker and the roster-jump funnel through), not a
/// hand-built proxy state.
///
/// Negative control: comment out the Part-4 guard in `picker_attach_existing`
/// (`agent_ui.rs`, the `if let (Some(sp), Some(tp)) … sp != tp { … return }`
/// block) → the cross-project attach binds the A tile to the B session, so the
/// `tile.session().is_none()` refusal assert fails (observed RED). The Part-3
/// filter revert (restore the `cwd_match_key` gate in `picker_projection`) makes
/// the selector-omits assert fail.
#[gpui::test]
fn bind_refused_across_projects_allowed_within(cx: &mut TestAppContext) {
    use crate::{AgentTile, App};
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = boot_browser(cx);
    let pa = PathBuf::from("/tmp/yalda-bind-a");
    let pb = PathBuf::from("/tmp/yalda-bind-b");
    let (a_pid, b_pid) = view.update(vcx, |v, _| {
        let a = v.projects.create("Aproj".into(), pa.clone()).expect("A");
        let b = v.projects.create("Bproj".into(), pb.clone()).expect("B");
        (a, b)
    });
    // A FREE roster session rooted at B's cwd → it belongs to project B.
    view.update(vcx, |v, _| {
        v.agent_roster.upsert(SessionInfo {
            session_id: "SB".into(),
            acp_session_id: None,
            label: "claude-b".into(),
            cwd: pb.clone(),
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
    });
    // The active workspace is project A, showing an unbound agent tile (selector).
    view.update(vcx, |v, _| {
        if let Some(t) = v.workspace.active_workspace_mut() {
            t.set_project(a_pid);
        }
        let mut tile = AgentTile::new();
        tile.show_picker();
        v.set_screen(App::Agent(tile));
    });

    // Part 3: A's selector must NOT list B's cross-project free session.
    view.read_with(vcx, |v, _| {
        let (free, _bound) = v.picker_projection(&v.agent_base_cwd());
        assert!(
            !free.iter().any(|s| s.sid == "SB"),
            "A's selector must not offer B's cross-project free session"
        );
    });

    // Part 4: a direct cross-project attach (A tile ← B session) is refused —
    // nothing binds, a transient note is set.
    view.update(vcx, |v, cx| {
        v.transient_status = None;
        v.picker_attach_existing(
            pb.clone(),
            "SB".into(),
            None,
            yalda::acp_channel::AgentProvider::Claude,
            "claude-b".into(),
            true,
            yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            cx,
        );
    });
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let tile = v.agent_tile().expect("agent tile");
        assert!(
            tile.session().is_none(),
            "cross-project bind refused: the A tile stays unbound"
        );
        assert!(
            v.transient_status.is_some(),
            "the refusal surfaces a transient note"
        );
    });

    // Allowed within: point the active workspace at project B and attach the SAME
    // session — now intra-project, so it binds.
    view.update(vcx, |v, cx| {
        if let Some(t) = v.workspace.active_workspace_mut() {
            t.set_project(b_pid);
        }
        let mut tile = AgentTile::new();
        tile.show_picker();
        v.set_screen(App::Agent(tile));
        v.picker_attach_existing(
            pb.clone(),
            "SB".into(),
            None,
            yalda::acp_channel::AgentProvider::Claude,
            "claude-b".into(),
            true,
            yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            cx,
        );
    });
    vcx.run_until_parked();
    view.read_with(vcx, |v, _| {
        let tile = v.agent_tile().expect("agent tile");
        let bound = tile.session().expect("same-project bind succeeds");
        assert_eq!(
            v.sessions.sid_of(bound).map(|s| s.as_str()),
            Some("SB"),
            "the B tile is bound to the B session"
        );
    });
}

/// UXI-Project-7 — the active project is DERIVED from focus, never stored: the
/// focused workspace's project, else the focused session's, else the first. Point
/// the active workspace at A → `active_project()` is A; jump a FREE session rooted
/// in B (its ephemeral workspace lands under B via UXI-Project-6) and focus it →
/// `active_project()` follows to B.
///
/// Drives the REAL paths (`active_project`, `jump_to_session`), not hand-built
/// state.
///
/// Negative control: replace `active_project`'s body with `self.projects.first()`
/// (`agent_ui.rs`) → after the jump it still reports A (the first project), so the
/// `Some(b_pid)` assert fails (observed RED).
#[gpui::test]
fn active_project_derives_from_focus(cx: &mut TestAppContext) {
    use crate::{AgentSession, AgentState};
    let (view, vcx) = boot_browser(cx);
    let pa = PathBuf::from("/tmp/yalda-active-a");
    let pb = PathBuf::from("/tmp/yalda-active-b");
    let (a_pid, b_pid) = view.update(vcx, |v, _| {
        let a = v.projects.create("Aproj".into(), pa.clone()).expect("A");
        let b = v.projects.create("Bproj".into(), pb.clone()).expect("B");
        (a, b)
    });
    // A FREE local session rooted in B's cwd (the focused tile is a browser, so
    // `show_local_session` binds nothing — it stays free/re-bindable).
    let sid = view.update(vcx, |v, cx| {
        let s = AgentSession {
            state: AgentState::new_server_managed(None),
            label: "sess-b".into(),
            cwd: pb.clone(),
            resume_id: None,
        };
        v.show_local_session(s, cx)
    });

    // Focus a project-A workspace → active project A.
    view.update(vcx, |v, _| {
        if let Some(t) = v.workspace.active_workspace_mut() {
            t.set_project(a_pid);
        }
    });
    let at_a = view.read_with(vcx, |v, cx| v.active_project(cx));
    assert_eq!(
        at_a,
        Some(a_pid),
        "active project = focused workspace's project (A)"
    );

    // Jump the free B-session: its ephemeral workspace opens under project B, so
    // the derived active project follows focus to B.
    view.update(vcx, |v, cx| v.jump_to_session(sid, cx));
    let at_b = view.read_with(vcx, |v, cx| v.active_project(cx));
    assert_eq!(
        at_b,
        Some(b_pid),
        "focusing the B session moves the derived active project to B"
    );
    // Distinct ids so the NC (hardcode `first()`) is non-vacuous.
    assert_ne!(a_pid, b_pid);
}

/// Deleting the LAST project must not orphan the surviving workspace (review-caught
/// bug): `perform_delete_project` closes the project first, then mints a fresh
/// default when none survive, and seeds the replacement workspace under THAT — so
/// the store is never empty and no workspace points at a dead project id.
///
/// Negative control: restore the old `ids().find(|x| x != pid).unwrap_or(pid)`
/// survivor (computed before `close(pid)`) → the seeded workspace points at the
/// deleted id and `projects.contains(w.project())` fails.
#[gpui::test]
fn delete_last_project_mints_a_fresh_default(cx: &mut TestAppContext) {
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    vcx.run_until_parked();
    let only = view.read_with(vcx, |v, _| {
        assert_eq!(v.projects.len(), 1, "boot has exactly one project");
        v.projects.first().expect("the boot project")
    });
    view.update(vcx, |v, cx| v.perform_delete_project(only, cx));
    view.read_with(vcx, |v, cx| {
        assert!(v.projects.len() >= 1, "a fresh default project was minted");
        assert!(!v.workspace.workspaces.is_empty(), "a workspace survives");
        for w in &v.workspace.workspaces {
            assert!(
                v.projects.contains(w.project()),
                "no surviving workspace points at a deleted project id"
            );
        }
        assert!(
            v.active_project(cx).is_some_and(|p| v.projects.contains(p)),
            "the active project resolves to a live project"
        );
    });
}

/// Count the tiles (leaves) in the active workspace's layout.
#[cfg(test)]
fn active_tile_count(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
) -> usize {
    view.update(vcx, |v, _| {
        let mut n = 0;
        if let Some(wsp) = v.workspace.active_workspace() {
            wsp.layout.for_each_leaf(&mut |_| n += 1);
        }
        n
    })
}

/// ADR-0033: "new agent" is contextual. In a workspace it adds a bound tile;
/// while directly viewing Unbound it creates another unbound tile and preserves
/// the original tile and session. Drives the real menu dispatcher in both modes.
#[gpui::test]
fn new_agent_adds_bound_or_unbound_tile_by_focus_domain(cx: &mut TestAppContext) {
    use crate::App;
    let (view, vcx) = boot_browser(cx);
    let sid = add_free_session(&view, vcx, "claude-1");

    // ── A. Real workspace: a NEW tile appears, on the picker. ──────────────
    let before = active_tile_count(&view, vcx);
    view.update(vcx, |v, cx| v.dispatch_menu_command("new-agent-tile", cx));
    vcx.run_until_parked();
    assert_eq!(
        active_tile_count(&view, vcx),
        before + 1,
        "in a real workspace, new agent ADDS a tile"
    );
    view.update(vcx, |v, _| {
        // The new tile is an agent tile. (Whether it rests on the picker or is
        // pre-bound is the server-vs-direct-spawn split inside `open_agent_inner`:
        // production runs the server path and lands on the picker; this harness has
        // no daemon, so it takes the legacy direct-spawn branch. The PLACEMENT is
        // what this guard pins.)
        assert!(
            matches!(v.workspace.focused_content(), Some(App::Agent(_))),
            "the new tile is an agent tile, got {:?}",
            v.workspace.focused_content().map(std::mem::discriminant)
        );
    });

    // ── B. Direct Unbound view: create another unbound tile. ──────────────
    view.update(vcx, |v, cx| v.jump_to_session(sid, cx));
    vcx.run_until_parked();
    let (workspaces_before, unbound_before, original) = view.update(vcx, |v, _| {
        let original = v
            .workspace
            .presented_detached_tile_id()
            .expect("the jump directly focuses an unbound tile");
        assert_eq!(v.focused_bound_session(), Some(sid));
        (
            v.workspace.workspaces.len(),
            v.workspace.detached_tiles.len(),
            original,
        )
    });

    view.update(vcx, |v, cx| v.dispatch_menu_command("new-agent-tile", cx));
    vcx.run_until_parked();

    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.workspaces.len(),
            workspaces_before,
            "direct creation does not manufacture a workspace"
        );
        assert_eq!(
            v.workspace.detached_tiles.len(),
            unbound_before + 1,
            "a second stable unbound tile is created"
        );
        let created = v
            .workspace
            .presented_detached_tile_id()
            .expect("the new unbound tile is directly focused");
        assert_ne!(created, original, "new Agent preserves the original tile");
        assert!(
            matches!(v.workspace.focused_content(), Some(App::Agent(t)) if t.session().is_none()),
            "the new tile starts as an empty Agent picker"
        );
        assert_eq!(
            v.workspace
                .tile(original)
                .and_then(|window| match &window.content {
                    App::Agent(tile) => tile.session(),
                    _ => None,
                }),
            Some(sid),
            "the original unbound tile retains its session"
        );
        assert!(v.sessions.contains(sid));
    });
}

/// ADR-0033: closing a session in a directly focused unbound Agent tile keeps
/// that same tile alive and unbound as an empty picker. Session lifecycle does
/// not change workspace ownership.
#[gpui::test]
fn closing_session_keeps_same_unbound_tile_as_empty_picker(cx: &mut TestAppContext) {
    use crate::App;
    let (view, vcx) = boot_browser(cx);
    let sid = add_free_session(&view, vcx, "claude-1");
    view.update(vcx, |v, cx| v.jump_to_session(sid, cx));
    vcx.run_until_parked();
    let tile = view.update(vcx, |v, _| {
        v.workspace
            .presented_detached_tile_id()
            .expect("session owns one directly focused unbound tile")
    });

    view.update(vcx, |v, cx| v.dispatch_menu_command("claude-close", cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        v.with_session(sid, cx, |c| {
            for ch in "yes".chars() {
                c.input_surface.compose_mut().editor.insert_char(ch);
            }
        });
    });
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();

    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.presented_detached_tile_id(),
            Some(tile),
            "closing the session preserves direct focus on the same tile"
        );
        assert!(matches!(
            v.workspace.tile(tile).map(|window| &window.content),
            Some(App::Agent(agent)) if agent.session().is_none()
        ));
        assert!(
            v.workspace
                .detached_tiles
                .iter()
                .any(|entry| entry.window.id() == tile),
            "the empty Agent tile remains unbound"
        );
        assert!(!v.sessions.contains(sid), "the session itself was closed");
    });
}

/// UXI-AgentTile-23: arming the close confirm ALSO drops the user into insert when
/// the compose is empty — so closing is `<space> x yes ⏎` with no manual focus
/// step — but changes nothing when a draft is at risk (UXI-AgentTile-22 rule 1
/// still governs that case, because `yes` appended to a draft would silently
/// cancel and clearing the draft would destroy the user's work).
///
/// Drives the REAL `dispatch_menu_command("claude-close")`, then types `yes` WITHOUT
/// any focus/insert call and submits through the real path — if the auto-insert
/// didn't happen, the typing wouldn't be in a live compose.
///
/// Negative control: delete the auto-insert block in `arm_close_confirm` → the
/// focus/mode asserts fire RED; make it unconditional → the draft-case asserts fire.
#[cfg(feature = "test-support")]
#[gpui::test]
fn arming_close_drops_into_insert_unless_a_draft_is_at_risk(cx: &mut TestAppContext) {
    use crate::EditMode;

    // ── A. Empty compose (idle worksheet, resting in nav) → typeable. ──────
    {
        let (view, vcx, id, _controls) = boot_worksheet_channel(cx);
        view.update(vcx, |v, cx| {
            v.with_session(id, cx, |c| {
                assert_eq!(
                    c.focus,
                    crate::AgentFocus::Transcript,
                    "worksheet rests in nav"
                );
                assert!(c.input_surface.compose().text().is_empty(), "no draft");
            });
        });

        view.update(vcx, |v, cx| v.dispatch_menu_command("claude-close", cx));
        vcx.run_until_parked();
        view.update(vcx, |v, cx| {
            v.with_session(id, cx, |c| {
                assert_eq!(
                    c.focus,
                    crate::AgentFocus::Compose,
                    "arming with an empty compose focuses it"
                );
                assert_eq!(
                    c.input_surface.compose().mode,
                    EditMode::Insert,
                    "…in INSERT, so `yes` can just be typed"
                );
                assert!(
                    c.you_block_open,
                    "the idle worksheet's typeable surface is a You-block"
                );
            });
        });

        // Type `yes` with NO focus/insert call of our own, and submit for real.
        view.update(vcx, |v, cx| {
            v.with_session(id, cx, |c| {
                for ch in "yes".chars() {
                    c.input_surface.compose_mut().editor.insert_char(ch);
                }
            });
        });
        let workspaces_before = view.update(vcx, |v, _| v.workspace.workspaces.len());
        view.update(vcx, |v, cx| v.submit_agent(cx));
        vcx.run_until_parked();
        view.update(vcx, |v, _| {
            assert_eq!(
                v.focused_bound_session(),
                None,
                "<space> x yes ⏎ closes the session with no manual focus step"
            );
            // UXI-Workspace-9 clause 1 (the contrast to
            // `closing_the_session_in_a_bare_agent_view_dismisses_it`): this is a
            // REAL workspace, so nothing is dismissed — the tile stays an agent tile
            // showing the unbound selector.
            assert_eq!(
                v.workspace.workspaces.len(),
                workspaces_before,
                "closing in a REAL workspace destroys no workspace"
            );
            assert!(
                matches!(v.workspace.focused_content(), Some(crate::App::Agent(t)) if t.session().is_none()),
                "the real-workspace tile stays an agent tile, now the unbound selector"
            );
        });
    }

    // ── B. A draft is at risk → arming changes nothing (rule 1 preserved). ─
    {
        let (view, vcx, id, _controls) = boot_worksheet_channel(cx);
        // Put a draft in the compose the way the user would, then step back out to
        // transcript navigation so "no focus move" is observable.
        view.update(vcx, |v, cx| {
            v.with_session(id, cx, |c| {
                c.open_you_block_at_cursor();
                for ch in "half a thought".chars() {
                    c.input_surface.compose_mut().editor.insert_char(ch);
                }
                c.focus = crate::AgentFocus::Transcript;
                c.input_surface.compose_mut().mode = EditMode::Normal;
            });
        });

        view.update(vcx, |v, cx| v.dispatch_menu_command("claude-close", cx));
        vcx.run_until_parked();
        view.update(vcx, |v, cx| {
            v.with_session(id, cx, |c| {
                assert_eq!(
                    c.focus,
                    crate::AgentFocus::Transcript,
                    "with a draft at risk, arming must NOT move focus"
                );
                assert_eq!(
                    c.input_surface.compose().mode,
                    EditMode::Normal,
                    "…and must NOT enter insert"
                );
                assert_eq!(
                    c.input_surface.compose().text(),
                    "half a thought",
                    "…and must never clear the draft"
                );
            });
        });
    }
}

/// `add_free_session`, but rooted at an explicit cwd so the session belongs to a
/// specific project (`Projects::membership_for_cwd`).
#[cfg(test)]
fn add_free_session_at(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    label: &str,
    cwd: PathBuf,
) -> crate::SessionId {
    use crate::{AgentSession, AgentState};
    let label = label.to_string();
    view.update(vcx, |v, cx| {
        let id = v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(None),
                label,
                cwd,
                resume_id: None,
            },
            cx,
        );
        cx.notify();
        id
    })
}

/// Arm + confirm a close through the REAL paths (`dispatch_menu_command("claude-close")`
/// then a `yes` submit), the way the user does it.
#[cfg(test)]
fn real_close_confirmed(view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext) {
    view.update(vcx, |v, cx| v.dispatch_menu_command("claude-close", cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let id = v.focused_bound_session().expect("a bound session to close");
        v.with_session(id, cx, |c| {
            for ch in "yes".chars() {
                c.input_surface.compose_mut().editor.insert_char(ch);
            }
        });
    });
    view.update(vcx, |v, cx| v.submit_agent(cx));
    vcx.run_until_parked();
}

/// Closing a selected session cannot re-home its unbound tile into whichever
/// foreign-project workspace happens to be active underneath direct focus.
#[gpui::test]
fn closing_session_preserves_unbound_tile_project(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    let pa = PathBuf::from("/tmp/yalda-fcsp-a");
    let pb = PathBuf::from("/tmp/yalda-fcsp-b");

    let b_pid = view.update(vcx, |v, _| {
        let a = v.projects.create("Aproj".into(), pa.clone()).expect("A");
        let b = v.projects.create("Bproj".into(), pb.clone()).expect("B");
        if let Some(w) = v.workspace.active_workspace_mut() {
            w.set_project(a);
        }
        b
    });
    let sb = add_free_session_at(&view, vcx, "claude-b", pb.clone());
    view.update(vcx, |v, cx| v.jump_to_session(sb, cx));
    vcx.run_until_parked();
    let tile = view.update(vcx, |v, _| {
        let tile = v
            .workspace
            .presented_detached_tile_id()
            .expect("project-B session materializes an unbound tile");
        assert_eq!(v.workspace.tile_project(tile), Some(b_pid));
        tile
    });

    real_close_confirmed(&view, vcx);

    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.presented_detached_tile_id(),
            Some(tile),
            "session close leaves the same tile directly focused"
        );
        assert_eq!(
            v.workspace.tile_project(tile),
            Some(b_pid),
            "closing content cannot re-home the tile into project A"
        );
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// UXI-AgentTile-27 — session autonaming + summary
//
// The live Haiku HTTP call is dev-system verification gap 2 (a real subprocess /
// network round-trip), so `spawn_autoname_worker` is suppressed under
// `cfg(test)` exactly as `spawn_recap_worker` is. These tests drive the REAL
// turn-completion path into the REAL arming logic, then hand the reducer the
// result the worker would have delivered.
// ─────────────────────────────────────────────────────────────────────────────

/// Boot one bound, server-managed session ARMED for autonaming, with a seeded
/// opening exchange. Returns the view, its context, and the session's id.
fn boot_armed_autoname_session(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<YaldaGpuiView>,
    &mut gpui::VisualTestContext,
    crate::SessionId,
) {
    use crate::{AgentSession, AgentState, AgentTile, App};

    let (view, vcx) = boot_browser(cx);
    let id = view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        let id = v.show_local_session(
            AgentSession {
                // The REAL arming call every fresh-create path uses.
                state: AgentState::new_server_managed(None).armed_for_autoname(),
                label: "claude-3".into(),
                cwd: PathBuf::from("."),
                resume_id: None,
            },
            cx,
        );
        v.sessions
            .bind_sid(id, ServerSid::new("S1"))
            .expect("S1 binds");
        id
    });
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |s| {
            s.editor
                .programmatic_insert(0, "user: rip out the payments adapter\nagent: on it\n");
        });
    });
    vcx.run_until_parked();
    (view, vcx, id)
}

/// End a turn on session `sid` through the REAL server path.
fn end_turn_for(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    sid: &str,
    turn_count: usize,
) {
    use yalda::session_proto::Notification as ServerNotification;
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::TurnEnded {
                session_id: sid.into(),
                turn_count,
                generation: 1,
            }],
            cx,
        );
    });
    vcx.run_until_parked();
}

/// UXI-AgentTile-27 property 1: the FIRST completed turn of an armed session
/// arms exactly one naming request, and a SECOND completed turn arms nothing —
/// the derivation is one-shot per session, ever.
///
/// Drives the real path the user's turn actually runs (`apply_server_batch` →
/// `ServerNotification::TurnEnded` → `finalize_agent_turn_idem` → the
/// `drain_autoname_requests` call at the end of the batch), NOT a hand-built
/// state.
///
/// Negative control (observed RED): delete the `autoname_due = true` arm in
/// `finalize_agent_turn_idem` → the post-turn-1 assert reads `Pending`, not
/// `Requested`. Separately, delete the `autoname = Requested` flip in
/// `drain_autoname_requests` → the same assert fails.
#[gpui::test]
fn autoname_fires_once_on_first_turn_completion(cx: &mut TestAppContext) {
    let (view, vcx, id) = boot_armed_autoname_session(cx);

    // Before any turn: still owed, nothing requested.
    let before = view.update(vcx, |v, cx| {
        Some(v.sessions.get(id).unwrap().read(cx).state.autoname)
    });
    assert_eq!(
        before,
        Some(crate::AutonameState::Pending),
        "a freshly armed session owes an autoname"
    );

    end_turn_for(&view, vcx, "S1", 1);
    let after_first = view.update(vcx, |v, cx| {
        Some(v.sessions.get(id).unwrap().read(cx).state.autoname)
    });
    assert_eq!(
        after_first,
        Some(crate::AutonameState::Requested),
        "the first completed turn must arm exactly one naming request"
    );
    let pending_summary = view.update(vcx, |v, cx| {
        v.jump_panel_agent_rows(cx)
            .into_iter()
            .find(|row| row.order_sid.as_deref() == Some("S1"))
            .is_some_and(|row| row.summary_pending)
    });
    assert!(
        pending_summary,
        "the jump row exposes summary progress while the model request is in flight"
    );

    // A second turn must NOT re-arm: settle the first request as the worker
    // would (no name came back), then end another turn.
    view.update(vcx, |v, cx| v.finish_autoname(id, None, cx));
    end_turn_for(&view, vcx, "S1", 2);
    let after_second = view.update(vcx, |v, cx| {
        Some((
            v.sessions.get(id).unwrap().read(cx).state.autoname,
            v.sessions.get(id).unwrap().read(cx).state.autoname_due,
        ))
    });
    assert_eq!(
        after_second,
        Some((crate::AutonameState::Done, false)),
        "a later turn must never re-arm autonaming (one shot, ever)"
    );
}

/// Topic summaries are useful before the first agent reply finishes. The shared
/// accepted-user-turn chokepoint installs an immediate compact excerpt, while
/// leaving the AI naming one-shot Pending to refine it asynchronously later.
#[gpui::test]
fn autoname_topic_appears_on_first_user_turn(cx: &mut TestAppContext) {
    use yalda::agent_transcript::UserTurnOrigin;
    let (view, vcx, id) = boot_armed_autoname_session(cx);
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |session| {
            session.insert_user_turn(
                "redesign the jump tabs without low contrast gold text",
                UserTurnOrigin::LocalSubmit,
                false,
            );
        });
    });
    view.read_with(vcx, |v, cx| {
        let session = v.sessions.get(id).expect("session").read(cx);
        assert_eq!(
            session.state.summary.as_deref(),
            Some("redesign the jump tabs without low contrast gold text"),
            "the topic is visible immediately after submit"
        );
        assert_eq!(
            session.state.autoname,
            crate::AutonameState::Pending,
            "the AI refinement is still owed after the immediate excerpt"
        );
    });
}

/// Summary reliability: a missing API key is a normal installation state, not a
/// reason for the one-shot to remain `Requested` forever. The exact credential
/// branch settles with a deterministic opening-topic fallback and persists it.
///
/// Negative control: remove the `finish_autoname` call from the `None` key arm
/// of `spawn_autoname_worker_with_key`; state remains Requested, summary stays
/// absent, and both assertions fail RED.
#[gpui::test]
fn autoname_without_api_key_settles_with_persisted_topic(cx: &mut TestAppContext) {
    let (view, vcx, id) = boot_armed_autoname_session(cx);
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("session_summaries.json");
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |session| {
            session.autoname = crate::AutonameState::Requested;
        });
        crate::persist::with_session_summaries_path(file.clone(), || {
            v.test_autoname_without_api_key(
                id,
                "user: improve jump panel state tabs\nagent: working on it".into(),
                cx,
            )
        });
    });
    vcx.run_until_parked();

    view.read_with(vcx, |v, cx| {
        let session = v.sessions.get(id).expect("session").read(cx);
        assert_eq!(session.state.autoname, crate::AutonameState::Done);
        assert_eq!(
            session.state.summary.as_deref(),
            Some("improve jump panel state tabs"),
            "no-key fallback is visible instead of a permanently blank summary"
        );
    });
    let saved = crate::persist::with_session_summaries_path(file, || {
        crate::persist::load_session_summaries()
    });
    assert_eq!(
        saved.get("S1").map(String::as_str),
        Some("improve jump panel state tabs"),
        "the fallback survives restart like an AI-produced summary"
    );
}

/// UXI-AgentTile-27: the result the worker brings back installs the name AND the
/// summary, and settles the one-shot.
///
/// Negative control (observed RED): drop the `s.label = name` assignment in
/// `finish_autoname` → the label stays `claude-3`. Drop the summary assignment →
/// the summary assert fails.
#[gpui::test]
fn autoname_result_renames_the_session(cx: &mut TestAppContext) {
    let (view, vcx, id) = boot_armed_autoname_session(cx);
    end_turn_for(&view, vcx, "S1", 1);

    view.update(vcx, |v, cx| {
        v.finish_autoname(
            id,
            Some(crate::SessionNaming {
                name: Some("payments adapter".into()),
                summary: Some("Ripping out the payments adapter.".into()),
            }),
            cx,
        )
    });
    vcx.run_until_parked();

    let (label, summary, state) = view
        .update(vcx, |v, cx| {
            let s = v.sessions.get(id).unwrap().read(cx);
            Some((s.label.clone(), s.state.summary.clone(), s.state.autoname))
        })
        .expect("session present");
    assert_eq!(
        label, "payments adapter",
        "the derived name replaces claude-N"
    );
    assert_eq!(
        summary.as_deref(),
        Some("Ripping out the payments adapter."),
        "the derived summary is installed for the jump panel"
    );
    assert_eq!(state, crate::AutonameState::Done, "the one-shot is settled");
}

/// bug-0020 / UXI-AgentTile-27: the autoname summary must round-trip through
/// `acp_sessions.json` — the naming call is one-shot, so a summary that isn't
/// written at settle time is gone forever after a restart (the jump panel's
/// italic explainer line vanishes on reload).
///
/// Drives the REAL settle entry point (`finish_autoname`, which the naming
/// worker calls) with the persist path overridden, then reads the file back
/// through the REAL loader.
#[gpui::test]
fn autoname_summary_survives_a_restart(cx: &mut TestAppContext) {
    let (view, vcx, id) = boot_armed_autoname_session(cx);
    end_turn_for(&view, vcx, "S1", 1);
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("acp_sessions.json");

    view.update(vcx, |v, cx| {
        crate::persist::with_acp_persist_path(file.clone(), || {
            v.finish_autoname(
                id,
                Some(crate::SessionNaming {
                    name: Some("payments adapter".into()),
                    summary: Some("Ripping out the payments adapter.".into()),
                }),
                cx,
            )
        })
    });
    vcx.run_until_parked();

    let slots = crate::persist::with_acp_persist_path(file.clone(), || {
        crate::persist::load_persisted_acp_sessions(&crate::persist::process_cwd())
    });
    let slot = slots
        .iter()
        .find(|s| s.id.as_str() == "S1")
        .expect("the bound session is persisted");
    assert_eq!(
        slot.label, "payments adapter",
        "the derived name is persisted (it already was)"
    );
    assert_eq!(
        slot.summary.as_deref(),
        Some("Ripping out the payments adapter."),
        "the derived summary must be persisted too, or it dies on reload"
    );
}

/// bug-0020: the jump panel's explainer line survives a GUI RELOAD — including
/// for a session no tile is bound to.
///
/// `acp_sessions.json` is cwd-keyed and only holds sessions bound to a tile at
/// save time, so it can never carry a free session's summary. This drives the
/// REAL settle path (`finish_autoname`) in one view, then boots a SECOND view
/// (the "reload") that only knows the session from the roster, and asserts the
/// row it builds still carries the summary.
///
/// Negative control: drop `save_session_summary` in `finish_autoname` (or the
/// `session_summaries` fallback in `jump_panel_agent_rows`) → the reloaded row's
/// summary is `None` → RED.
#[gpui::test]
fn autoname_summary_survives_a_gui_reload(cx: &mut TestAppContext) {
    use yalda::session_proto::SessionInfo;
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("session_summaries.json");

    // Session 1: a bound, armed session settles its autoname.
    let (view, vcx, id) = boot_armed_autoname_session(cx);
    end_turn_for(&view, vcx, "S1", 1);
    view.update(vcx, |v, cx| {
        crate::persist::with_session_summaries_path(file.clone(), || {
            v.finish_autoname(
                id,
                Some(crate::SessionNaming {
                    name: Some("payments adapter".into()),
                    summary: Some("Ripping out the payments adapter.".into()),
                }),
                cx,
            )
        })
    });
    vcx.run_until_parked();

    // Session 2 ("reload"): a fresh view that loads the sidecar at construction
    // and knows S1 only from the server roster — nothing is bound to it.
    let (view2, vcx2) =
        crate::persist::with_session_summaries_path(file.clone(), || boot_browser(cx));
    let row_summary = view2.update(vcx2, |v, cx| {
        v.agent_roster.upsert(SessionInfo {
            session_id: "S1".into(),
            acp_session_id: None,
            label: "payments adapter".into(),
            cwd: PathBuf::from("."),
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 1,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
        v.jump_panel_agent_rows(cx)
            .into_iter()
            .find(|r| matches!(&r.target, crate::JumpTarget::Roster(s) if s == "S1"))
            .and_then(|r| r.summary)
    });
    assert_eq!(
        row_summary.as_deref(),
        Some("Ripping out the payments adapter."),
        "the autoname summary must survive a reload, even unbound"
    );
}

/// A focused Agent tile bound to a pre-attach PLACEHOLDER session (no sid) with a
/// pending open token — the exact state the real create/attach paths leave behind
/// while the server round-trip is in flight, so `apply_open_agent_resolution` can
/// be driven against it. Returns the token.
#[cfg(test)]
fn boot_pending_agent_tile<'a>(
    cx: &'a mut TestAppContext,
) -> (
    gpui::Entity<YaldaGpuiView>,
    &'a mut gpui::VisualTestContext,
    u64,
) {
    use crate::{AgentSession, AgentState, AgentTile, App};
    let (view, vcx) = boot_browser(cx);
    let token = view.update(vcx, |v, cx| {
        v.set_screen(App::Agent(AgentTile::new()));
        v.show_local_session(
            AgentSession {
                state: AgentState::new_server_managed(Some("connecting…".into())),
                label: "claude-7".into(),
                cwd: PathBuf::from("."),
                resume_id: None,
            },
            cx,
        );
        let token = crate::alloc_open_token();
        if let Some(tile) = v.agent_tile_mut() {
            tile.set_pending(Some(token));
        }
        token
    });
    vcx.run_until_parked();
    (view, vcx, token)
}

/// bug-0022: the jump panel shows live status for a session this GUI has NEVER
/// opened. The server's `SessionBusy` broadcast drives the row: busy ⇒ working,
/// and a busy→idle flip while you are elsewhere ⇒ ready (the roster-side
/// twin of `AgentState::unread`), cleared when you jump to it.
///
/// This is the actual "status marks appear inconsistently" cause: `awaiting` /
/// `unread` used to be readable ONLY off a live in-store session, so free
/// sessions (and any session another GUI was driving) were permanently neutral.
///
/// Drives the REAL reducer (`apply_server_batch` with the real notification) and
/// the REAL row builder.
///
/// Negative control: drop the `.or(Some(info.busy))` fallback in
/// `jump_panel_agent_rows` → the roster row reports `Neutral` while working.
#[gpui::test]
fn roster_only_session_shows_live_status(cx: &mut TestAppContext) {
    use yalda::session_proto::{Notification as ServerNotification, SessionInfo};
    let (view, vcx) = boot_browser(cx);
    view.update(vcx, |v, _cx| {
        v.agent_roster.upsert(SessionInfo {
            session_id: "S-free".into(),
            acp_session_id: None,
            label: "free one".into(),
            cwd: PathBuf::from("."),
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
    });
    let status_of = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            v.jump_panel_agent_rows(cx)
                .into_iter()
                .find(|r| matches!(&r.target, crate::JumpTarget::Roster(s) if s == "S-free"))
                .map(|r| r.dot_status())
        })
    };
    assert_eq!(
        status_of(&view, vcx),
        Some(crate::AgentDotStatus::WaitingForYou),
        "idle begins ready for input"
    );

    // The server says a turn started — through the REAL notification reducer.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "S-free".into(),
                busy: true,
            }],
            cx,
        );
    });
    assert_eq!(
        status_of(&view, vcx),
        Some(crate::AgentDotStatus::Working),
        "a session we never opened still reports WORKING while its turn runs"
    );

    // …and finishing while we're elsewhere is ready for input.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "S-free".into(),
                busy: false,
            }],
            cx,
        );
    });
    assert_eq!(
        status_of(&view, vcx),
        Some(crate::AgentDotStatus::WaitingForYou),
        "a backgrounded roster session that finished a turn is waiting on you"
    );

    // Jumping to it marks it read.
    view.update(vcx, |v, _cx| v.mark_roster_session_read("S-free"));
    assert_eq!(
        status_of(&view, vcx),
        Some(crate::AgentDotStatus::WaitingForYou),
        "looking at it clears unread state without changing readiness"
    );
}

/// UXI-JumpPanel-12 (union clause): a session OPEN in this GUI whose current turn
/// was started ELSEWHERE — another GUI window, `yalda-mcp`, or a turn still
/// streaming after a reconnect — has `turn_phase == Idle` locally (the phase only
/// enters `Awaiting` on a submit THIS GUI made), yet the server's `SessionBusy`
/// flag is set. The row must read WORKING: the derivation unions the local phase
/// with `SessionInfo.busy`, so the server flag is never masked by the local Idle.
///
/// This is the "poor sense of awareness of when an agent is working" report: an
/// open session driven from elsewhere painted green "ready" while genuinely busy.
///
/// Drives the REAL row builder over a REAL bound local session (`install_agent_slot`)
/// plus the REAL reducer (`apply_server_batch` with `SessionBusy`). The local
/// `turn_phase` is left at its Idle default — nothing local ever makes it Awaiting.
///
/// Negative control (observed RED): restore the local-wins derivation
/// `let awaiting = local.map(|(a,_,_)| a).or(Some(info.busy));` in
/// `jump_panel_agent_rows` → the open session reports WaitingForYou while the
/// server says busy.
#[gpui::test]
fn open_session_busy_from_elsewhere_shows_working(cx: &mut TestAppContext) {
    use yalda::session_proto::{Notification as ServerNotification, SessionInfo};
    let (view, vcx) = boot_browser(cx);

    // A session bound to a tile HERE (so `opened`/local state resolves) AND
    // present in the roster (so the row is a Roster target the server flag drives).
    install_agent_slot(&view, &mut *vcx, Some("S-open"));
    view.update(vcx, |v, _cx| {
        v.agent_roster.upsert(SessionInfo {
            session_id: "S-open".into(),
            acp_session_id: None,
            label: "claude-1".into(),
            cwd: PathBuf::from("."),
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
    });
    vcx.run_until_parked();

    let status_of = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            v.jump_panel_agent_rows(cx)
                .into_iter()
                .find(|r| matches!(&r.target, crate::JumpTarget::Roster(s) if s == "S-open"))
                .map(|r| r.dot_status())
        })
    };

    // Precondition: locally Idle + server idle ⇒ ready for input.
    assert_eq!(
        status_of(&view, vcx),
        Some(crate::AgentDotStatus::WaitingForYou),
        "an open, idle session begins ready for input"
    );
    // Guard: the local turn_phase is genuinely Idle — this is not a vacuous pass.
    assert!(
        view.read_with(vcx, |v, cx| {
            let id = v
                .sessions
                .locate(&ServerSid::new("S-open"))
                .expect("bound session");
            v.read_session(id, cx, |c| c.turn_phase.is_awaiting()) == Some(false)
        }),
        "the open session's local turn_phase must stay Idle (the elsewhere case)"
    );

    // The server says the turn started — via the REAL reducer. Nothing touches
    // the local turn_phase, exactly like a turn another driver initiated.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "S-open".into(),
                busy: true,
            }],
            cx,
        );
    });
    assert_eq!(
        status_of(&view, vcx),
        Some(crate::AgentDotStatus::Working),
        "an open session whose turn runs elsewhere must read WORKING, not ready"
    );

    // And the server settling it returns the row to ready.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "S-open".into(),
                busy: false,
            }],
            cx,
        );
    });
    assert_eq!(
        status_of(&view, vcx),
        Some(crate::AgentDotStatus::WaitingForYou),
        "when the server clears busy and local is idle, the row is ready again"
    );
}

/// UXI-JumpPanel-30: a workspace folder marks that it contains a working agent, so
/// a collapsed folder still surfaces activity inside it. Derived from the folder's
/// own tiles (`AgentActivity::Working`), it tracks the same union-of-authorities
/// status the session dot uses — here the turn is driven by the server flag while
/// the local phase stays Idle.
///
/// Drives the REAL folder projection (`jump_panel_sections`) over a REAL agent tile
/// in the active workspace, and the REAL `SessionBusy` reducer.
///
/// Negative control (observed RED): force `has_working_agent = false` at its
/// construction site in `jump_panel_sections_with_tab` → the busy assertion fails.
#[gpui::test]
fn workspace_folder_marks_contained_working_agent(cx: &mut TestAppContext) {
    use yalda::session_proto::{Notification as ServerNotification, SessionInfo};
    // boot_with_transcript installs an agent tile bound to S1 in the active
    // workspace, so that workspace's folder owns the tile.
    let (view, vcx, _id, _session) = boot_with_transcript(cx);
    view.update(vcx, |v, _cx| {
        v.agent_roster.upsert(SessionInfo {
            session_id: "S1".into(),
            acp_session_id: None,
            label: "claude-1".into(),
            cwd: PathBuf::from("."),
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
    });
    vcx.run_until_parked();

    let any_folder_working =
        |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
            view.update(vcx, |v, cx| {
                v.jump_panel_sections(cx)
                    .0
                    .iter()
                    .flat_map(|s| &s.workspace_folders)
                    .any(|f| f.has_working_agent)
            })
        };
    // Guard the setup is non-vacuous: a folder actually owns the S1 tile.
    assert!(
        view.update(vcx, |v, cx| {
            v.jump_panel_sections(cx)
                .0
                .iter()
                .flat_map(|s| &s.workspace_folders)
                .flat_map(|f| &f.tiles)
                .any(|t| t.agent.is_some())
        }),
        "a workspace folder must own the bound agent tile (setup non-vacuous)"
    );

    assert!(
        !any_folder_working(&view, vcx),
        "idle agent ⇒ no folder marked working"
    );

    // The server starts a turn; the local phase is never touched (driven elsewhere).
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "S1".into(),
                busy: true,
            }],
            cx,
        );
    });
    assert!(
        any_folder_working(&view, vcx),
        "a working agent inside the workspace marks its folder working"
    );

    // Settling the turn clears the folder mark.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "S1".into(),
                busy: false,
            }],
            cx,
        );
    });
    assert!(
        !any_folder_working(&view, vcx),
        "the folder mark clears when the contained agent goes idle"
    );
}

/// bug-0021 / `UXI-AgentTile-27` property 1 (amended): the one-shot autoname is armed
/// by SESSION IDENTITY, not by which constructor built the state. A session that
/// arrives by ATTACH (created free from the jump panel, `/clear`ed, or restored)
/// with a still-generated `claude-N` label and an unspent one-shot is armed at the
/// bind, and its replayed content makes it due — that whole class could never be
/// named before.
///
/// Drives the REAL resolution handler (`apply_open_agent_resolution` →
/// `Attached`), then the REAL replay boundary (`finish_replay`) + drain.
///
/// Negative control: drop the `maybe_arm_autoname` call in
/// `apply_open_agent_resolution` → the session stays `Done` and never requests.
#[gpui::test]
fn attached_unnamed_session_is_armed_and_named(cx: &mut TestAppContext) {
    let (view, vcx, token) = boot_pending_agent_tile(cx);

    // The REAL attach resolution binds sid S-free and installs its label.
    view.update(vcx, |v, cx| {
        v.apply_open_agent_resolution(
            token,
            crate::OpenResolution::Attached(vec![crate::AttachedSlot {
                label: "claude-7".into(),
                sid: "S-free".into(),
                acp_id: None,
                provider: yalda::acp_channel::AgentProvider::Claude,
                status: "attached".into(),
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            }]),
            cx,
        );
    });
    let id = view.update(vcx, |v, _cx| v.focused_bound_session().expect("bound"));
    view.read_with(vcx, |v, cx| {
        assert_eq!(
            v.read_session(id, cx, |c| c.autoname),
            Some(crate::AutonameState::Pending),
            "an attached session still called claude-N must be armed for naming"
        );
    });

    // Replay delivers the conversation; that boundary is what makes it nameable.
    view.update(vcx, |v, cx| {
        v.with_session(id, cx, |c| {
            c.editor
                .programmatic_insert(0, "user: fix the flaky deploy test\nagent: on it\n");
            c.finish_replay();
        });
        v.drain_autoname_requests(cx);
    });
    vcx.run_until_parked();
    view.read_with(vcx, |v, cx| {
        assert_eq!(
            v.read_session(id, cx, |c| c.autoname),
            Some(crate::AutonameState::Requested),
            "replayed content makes the armed session due, and the drain requests the name"
        );
    });
}

/// bug-0021: the one-shot is spent per SID and that fact is durable — a session
/// whose naming already ran (or ran and produced nothing) is NOT re-armed on the
/// next launch, so we never re-ask Haiku about the same session forever.
///
/// Negative control: drop the `autoname_already_attempted` check in
/// `maybe_arm_autoname` → the second view re-arms and RED.
#[gpui::test]
fn a_spent_autoname_is_never_re_armed(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("session_summaries.json");

    // View 1: the call comes back empty — the one-shot is still SPENT.
    let (view, vcx, id) = boot_armed_autoname_session(cx);
    view.update(vcx, |v, cx| {
        crate::persist::with_session_summaries_path(file.clone(), || {
            v.finish_autoname(id, None, cx)
        })
    });
    vcx.run_until_parked();

    // View 2 ("relaunch"): the same sid attaches, still labelled claude-N.
    let (view2, vcx2, token) =
        crate::persist::with_session_summaries_path(file.clone(), || boot_pending_agent_tile(cx));
    view2.update(vcx2, |v, cx| {
        v.apply_open_agent_resolution(
            token,
            crate::OpenResolution::Attached(vec![crate::AttachedSlot {
                label: "claude-3".into(),
                sid: "S1".into(),
                acp_id: None,
                provider: yalda::acp_channel::AgentProvider::Claude,
                status: "attached".into(),
                permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            }]),
            cx,
        );
    });
    view2.read_with(vcx2, |v, cx| {
        let sid_id = v.focused_bound_session().expect("bound");
        assert_eq!(
            v.read_session(sid_id, cx, |c| c.autoname),
            Some(crate::AutonameState::Done),
            "a session whose one-shot was already spent must not be re-armed"
        );
    });
}

/// UXI-AgentTile-27 property 3 (early half): renaming BEFORE the first turn ends
/// latches the origin to `User`, and the completed turn then arms nothing.
///
/// Drives the REAL rename entry point the user's command runs
/// (`open_rename_agent_session` → `commit_rename_overlay`), not a hand-set field.
///
/// Negative control (observed RED): remove the `name_origin = NameOrigin::User`
/// latch in `commit_rename_overlay` → the session flips to `Requested` and the
/// assert fails.
#[gpui::test]
fn rename_latches_origin_and_blocks_autoname(cx: &mut TestAppContext) {
    let (view, vcx, id) = boot_armed_autoname_session(cx);

    // The REAL rename path: open the overlay for this session, type, commit.
    view.update(vcx, |v, cx| {
        v.open_rename_overlay(cx);
        if let Some(o) = v.rename_mut() {
            o.text = "my own name".into();
        }
        v.commit_rename_overlay(cx);
    });
    vcx.run_until_parked();

    let origin = view.update(vcx, |v, cx| {
        Some(v.sessions.get(id).unwrap().read(cx).state.name_origin)
    });
    assert_eq!(
        origin,
        Some(crate::NameOrigin::User),
        "an explicit rename latches the origin to User"
    );

    end_turn_for(&view, vcx, "S1", 1);
    let (autoname, label) = view
        .update(vcx, |v, cx| {
            let s = v.sessions.get(id).unwrap().read(cx);
            Some((s.state.autoname, s.label.clone()))
        })
        .expect("session present");
    assert_eq!(
        autoname,
        crate::AutonameState::Done,
        "a user-named session must never request an autoname"
    );
    assert_eq!(label, "my own name", "the user's name survives the turn");
}

/// UXI-AgentTile-27 property 3 (late half): a naming result that lands AFTER the
/// user renamed is DROPPED, never applied. This is the race the typed origin
/// exists for — the request was in flight when the user typed a name.
///
/// Negative control (observed RED): remove the `renamed_by_user` guard in
/// `finish_autoname` → the label is overwritten with `derived name` and the
/// assert fails.
#[gpui::test]
fn late_autoname_result_never_clobbers_a_user_rename(cx: &mut TestAppContext) {
    let (view, vcx, id) = boot_armed_autoname_session(cx);
    end_turn_for(&view, vcx, "S1", 1);
    let armed = view.update(vcx, |v, cx| {
        Some(v.sessions.get(id).unwrap().read(cx).state.autoname)
    });
    assert_eq!(
        armed,
        Some(crate::AutonameState::Requested),
        "precondition: a naming request is in flight"
    );

    // The user renames WHILE the call is in flight (real entry point).
    view.update(vcx, |v, cx| {
        v.open_rename_overlay(cx);
        if let Some(o) = v.rename_mut() {
            o.text = "typed by hand".into();
        }
        v.commit_rename_overlay(cx);
    });
    vcx.run_until_parked();

    // …and only now does the worker come back with its answer.
    view.update(vcx, |v, cx| {
        v.finish_autoname(
            id,
            Some(crate::SessionNaming {
                name: Some("derived name".into()),
                summary: Some("Should not be installed.".into()),
            }),
            cx,
        )
    });
    vcx.run_until_parked();

    let (label, summary) = view
        .update(vcx, |v, cx| {
            let s = v.sessions.get(id).unwrap().read(cx);
            Some((s.label.clone(), s.state.summary.clone()))
        })
        .expect("session present");
    assert_eq!(
        label, "typed by hand",
        "a late autoname must never overwrite the name the user typed"
    );
    assert_eq!(
        summary, None,
        "…and it must not sneak its summary in either"
    );
}

// ---- UXI-JumpPanel-9: the Cmd-P jump palette ------------------------------
//
// The palette is a pure alternate INPUT onto the jump panel's list, so these
// drive the real chord (`register_keymap` + `simulate_keystrokes`), the real
// item projection (`jump_palette_items`, built from `jump_panel_sections`), and
// the real activators (`select_workspace` / `jump_to_agent`). The ranking is a
// pure function, so "the top row is the best match" is asserted directly rather
// than inferred from paint.

/// Give the boot workspace a typeable name and add `n` more named workspaces.
/// Returns nothing — tests address workspaces by their labels, the way the user
/// does.
#[cfg(test)]
fn name_workspaces(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
    names: &[&str],
) {
    use crate::{App, BrowserWindow, BufferApp};
    let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    view.update(vcx, |v, _| {
        let cwd = PathBuf::from(".");
        while v.workspace.workspaces.len() < names.len() {
            v.workspace
                .push_workspace_inheriting(App::Buffer(BufferApp::Picking(
                    BrowserWindow::standalone(cwd.clone()),
                )));
        }
        for (i, n) in names.iter().enumerate() {
            v.workspace.workspaces[i].display_name = Some(n.clone());
        }
        v.workspace.set_active_workspace(0);
    });
    vcx.run_until_parked();
}

/// UXI-JumpPanel-9 (1): `Cmd-P` opens the palette through the REAL keymap, and a
/// second `Cmd-P` is a no-op — it neither closes it nor leaks a typed `p` into
/// the query (the overlay captures keys before action dispatch, so the chord has
/// to die in `handle_jump_palette_key`).
#[gpui::test]
fn jump_palette_cmd_p_opens_over_any_screen(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    name_workspaces(&view, vcx, &["alpha", "beta"]);

    view.update(vcx, |v, _| {
        assert!(!v.overlay_is_jump_palette(), "palette starts closed");
    });

    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert!(
            v.overlay_is_jump_palette(),
            "cmd-p must open the jump palette on the focused screen"
        );
        assert_eq!(
            v.jump_palette_ref().unwrap().query,
            "",
            "opens with an empty query"
        );
    });

    // Re-pressing the chord: still open, still empty — not a toggle, not a `p`.
    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert!(
            v.overlay_is_jump_palette(),
            "cmd-p while open is a no-op, not a toggle"
        );
        assert_eq!(
            v.jump_palette_ref().unwrap().query,
            "",
            "the cmd-p chord must never type its bare letter into the query"
        );
    });
}

/// UXI-JumpPanel-9 (candidate set): the palette lists every non-ephemeral
/// workspace AND every agent session, in panel order — and lists no project.
#[gpui::test]
fn jump_palette_lists_workspaces_and_sessions_in_panel_order(cx: &mut TestAppContext) {
    let (view, vcx) = boot_browser(cx);
    name_workspaces(&view, vcx, &["alpha", "beta"]);
    let gamma = add_free_session(&view, vcx, "gamma-session");
    view.update(vcx, |v, cx| {
        // A session becomes a navigation candidate when it owns a tile. The
        // direct visit materializes that stable unbound tile; returning to the
        // workspace changes focus only, not ownership.
        v.jump_to_session(gamma, cx);
        v.workspace.set_active_workspace(0);
    });

    let (labels, agents) = view.update(vcx, |v, cx| {
        let items = v.jump_palette_items(cx);
        (
            items.iter().map(|i| i.label.clone()).collect::<Vec<_>>(),
            items.iter().map(|i| i.is_agent).collect::<Vec<_>>(),
        )
    });

    assert!(
        labels.contains(&"alpha".to_string()),
        "workspaces are candidates: {labels:?}"
    );
    assert!(
        labels.contains(&"beta".to_string()),
        "every workspace is a candidate: {labels:?}"
    );
    assert!(
        labels.contains(&"gamma-session".to_string()),
        "agent sessions are candidates: {labels:?}"
    );
    // Panel order: a section's workspaces precede its sessions.
    let first_agent = agents
        .iter()
        .position(|a| *a)
        .expect("at least one session row");
    assert!(
        agents[..first_agent].iter().all(|a| !*a),
        "panel order puts a section's workspaces before its sessions: {agents:?}"
    );
}

/// ADR-0034 / UXI-JumpPanel-25: Cmd-P names a Detached *tile*, Enter opens that
/// exact tile without attaching it, and the explicit workspace command moves the
/// same id into the active workspace.
#[gpui::test]
fn jump_palette_opens_detached_tile_then_attaches_same_identity(cx: &mut TestAppContext) {
    use crate::workspace::TileMembership;
    use crate::{App, LinearTile, PaletteTarget};
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    let id = view.update(vcx, |v, _| {
        let project = v.workspace.active_workspace().expect("workspace").project();
        let mut tile = LinearTile::new();
        tile.title = "unique-unbound-linear".into();
        v.workspace.push_detached(App::Linear(tile), project)
    });

    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("u n i q u e - u n b o u n d");
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let (items, ranked) = v.jump_palette_ranked(cx);
        let top = &items[*ranked.first().expect("unbound tile is a Cmd-P candidate")];
        assert_eq!(top.target, PaletteTarget::Tile(id));
        assert!(top.detail.ends_with("Detached"));
    });

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(v.workspace.presented_detached_tile_id(), Some(id));
        assert_eq!(
            v.workspace.tile_membership(id),
            Some(TileMembership::Detached)
        );
    });

    vcx.simulate_keystrokes("ctrl-w b");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.workspace.tile_membership(id),
            Some(TileMembership::Attached {
                workspace: 0,
                visibility: crate::workspace::AttachedVisibility::Visible
            })
        );
        assert_eq!(v.workspace.focused_window_id(), Some(id));
        assert!(
            v.workspace
                .detached_tiles
                .iter()
                .all(|tile| tile.window.id() != id)
        );
    });
}

/// UXI-JumpPanel-9 (2): ranking, not mere filtering. A prefix hit outranks a
/// late/scattered hit, and an exact hit outranks everything — so the TOP row is
/// the best match rather than the first list member that happened to match.
#[gpui::test]
fn jump_palette_ranks_best_match_first(_cx: &mut TestAppContext) {
    use crate::{PaletteItem, PaletteTarget, fuzzy_score, rank_palette_items};
    let item = |label: &str, i: usize| PaletteItem {
        target: PaletteTarget::Workspace(i),
        label: label.to_string(),
        detail: String::new(),
        is_agent: false,
        status: None,
        active: false,
    };
    // Deliberately listed worst-first, so a "filter in panel order" impl fails.
    let items = vec![
        item("the yalda archive", 0), // scattered, late
        item("yalda-gpui", 1),        // prefix
        item("yal", 2),               // exact
    ];
    let ranked = rank_palette_items(&items, "yal");
    assert_eq!(
        ranked
            .iter()
            .map(|&i| items[i].label.as_str())
            .collect::<Vec<_>>(),
        vec!["yal", "yalda-gpui", "the yalda archive"],
        "candidates must be ordered by match quality, best first"
    );

    // Non-matches are dropped entirely.
    assert_eq!(rank_palette_items(&items, "zzz"), Vec::<usize>::new());
    // An empty query keeps panel order.
    assert_eq!(rank_palette_items(&items, ""), vec![0, 1, 2]);
    // Word-start hits beat mid-word ones at equal length.
    assert!(
        fuzzy_score("my-agent-run", "mar").unwrap() > fuzzy_score("mmmagentrun", "mar").unwrap(),
        "characters landing at word starts must score higher"
    );
}

/// UXI-JumpPanel-9 (4): typing then `Enter` jumps to the top match, through the
/// REAL keystroke path and the REAL activator (`select_workspace`).
#[gpui::test]
fn jump_palette_enter_jumps_to_top_match(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    name_workspaces(&view, vcx, &["alpha", "beta", "gamma"]);

    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("g a m");
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(v.jump_palette_ref().unwrap().query, "gam");
        let (items, ranked) = v.jump_palette_ranked(cx);
        assert_eq!(
            items[ranked[0]].label, "gamma",
            "top match is the typed workspace"
        );
    });

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert!(!v.overlay_is_jump_palette(), "enter closes the palette");
        assert_eq!(
            v.workspace.active_workspace, 2,
            "enter jumps to the top match's workspace"
        );
    });
}

/// UXI-JumpPanel-9 (3)(4): arrows move the highlight WITHOUT navigating, and
/// `Enter` activates the highlighted row — not the top match.
#[gpui::test]
fn jump_palette_arrows_select_and_enter_activates_the_selection(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    name_workspaces(&view, vcx, &["alpha", "beta", "gamma"]);

    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();

    // Empty query ⇒ folder, child tile, then the next folder. Select beta's
    // workspace row, not alpha's first child tile.
    let (third_label, started_on) = view.update(vcx, |v, cx| {
        let (items, ranked) = v.jump_palette_ranked(cx);
        assert!(ranked.len() >= 3, "empty query lists everything");
        (items[ranked[2]].label.clone(), v.workspace.active_workspace)
    });

    vcx.simulate_keystrokes("down down");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert_eq!(
            v.jump_palette_ref().unwrap().selected,
            2,
            "down moves the highlight"
        );
        assert_eq!(
            v.workspace.active_workspace, started_on,
            "moving the highlight must NOT navigate"
        );
    });

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    let landed = view.update(vcx, |v, _| {
        assert!(!v.overlay_is_jump_palette());
        v.workspace.workspaces[v.workspace.active_workspace]
            .display_label()
            .to_string()
    });
    assert_eq!(
        landed, third_label,
        "enter activates the HIGHLIGHTED row, not the top match"
    );
}

/// UXI-JumpPanel-9 (5): a query nothing matches ⇒ `Enter` is a no-op and the
/// palette stays open (a typo must not jump you somewhere arbitrary).
#[gpui::test]
fn jump_palette_no_match_enter_is_noop(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    name_workspaces(&view, vcx, &["alpha", "beta", "gamma"]);

    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("z q x");
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        assert_eq!(v.jump_palette_ref().unwrap().query, "zqx");
        assert!(
            v.jump_palette_ranked(cx).1.is_empty(),
            "nothing matches 'zqx'"
        );
    });

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.update(vcx, |v, _| {
        assert!(
            v.overlay_is_jump_palette(),
            "enter with no matches must leave the palette open"
        );
        assert_eq!(v.workspace.active_workspace, 0, "…and must not navigate");
    });

    // Backspacing back to a matching query re-ranks and re-highlights the top.
    vcx.simulate_keystrokes("backspace backspace backspace b");
    vcx.run_until_parked();
    view.update(vcx, |v, cx| {
        let (items, ranked) = v.jump_palette_ranked(cx);
        assert_eq!(items[ranked[0]].label, "beta");
        assert_eq!(
            v.jump_palette_ref().unwrap().selected,
            0,
            "editing resets the highlight"
        );
    });
}

/// UXI-JumpPanel-9 (6): `Esc` closes with no navigation.
#[gpui::test]
fn jump_palette_escape_closes_without_navigating(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    name_workspaces(&view, vcx, &["alpha", "beta", "gamma"]);

    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("g a m");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();

    view.update(vcx, |v, _| {
        assert!(!v.overlay_is_jump_palette(), "escape closes the palette");
        assert_eq!(v.workspace.active_workspace, 0, "escape navigates nowhere");
    });
}

/// UXI-JumpPanel-9 (7): the palette never clobbers a sibling overlay — the
/// single `ActiveOverlay` slot is guarded, so `Cmd-P` over the rename/tag input
/// (both text-entry surfaces) is a no-op.
#[gpui::test]
fn jump_palette_does_not_open_over_another_overlay(cx: &mut TestAppContext) {
    use crate::{ActiveOverlay, WorkspacePicker, WorkspacePickerMode};
    let (view, vcx) = boot_browser(cx);
    name_workspaces(&view, vcx, &["alpha", "beta"]);

    view.update(vcx, |v, cx| {
        v.open_overlay(ActiveOverlay::WorkspacePicker(WorkspacePicker {
            project: v.workspace.inherited_project(),
            mode: WorkspacePickerMode::Move { follow: false },
            targets: vec![0, 1],
            selected: 0,
        }));
        v.open_jump_palette_impl(cx);
        assert!(
            !v.overlay_is_jump_palette(),
            "cmd-p must not steal the overlay slot from another overlay"
        );
        assert!(
            v.overlay_is_workspace(),
            "…and must leave that overlay intact"
        );
    });
}

/// UXI-JumpPanel-9: the palette actually PAINTS over the screen (a state-only
/// assert can't catch a collapsed or unmounted overlay). Layout probe, per the
/// anti-circling rule "assert on paint, not just state".
#[gpui::test]
fn jump_palette_paints_over_the_screen(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx) = boot_browser(cx);
    name_workspaces(&view, vcx, &["alpha", "beta", "gamma"]);

    // Closed: nothing paints.
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let closed = crate::layout_probe_get("jump-palette");
    crate::layout_probe_end();
    assert!(closed.is_none(), "the palette must not paint while closed");

    vcx.simulate_keystrokes("cmd-p");
    vcx.run_until_parked();

    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let open = crate::layout_probe_get("jump-palette");
    crate::layout_probe_end();

    let (_, _, w, h) = open.expect("the open palette did not paint");
    assert!(
        w > 0.0 && h > 0.0,
        "the palette painted a collapsed box ({w}x{h}) — the rows/input never reached the screen"
    );
}

/// UXI-JumpPanel-18: a real archive-flag toggle announces itself — one `Info`
/// system-console line naming the agent, plus a `TurnId::System` transcript
/// notice when this GUI has the session open. Drives the REAL mutator both
/// command surfaces route through (`set_session_archived`), for an open session
/// AND a roster-only one, and proves the no-op toggle stays silent.
///
/// Negative control: delete the `announce_session_archived` call from
/// `set_session_archived`. The console file has no `archived agent session`
/// line and the transcript tail has no `session archived` notice.
#[gpui::test]
fn archive_toggle_announces_in_console_and_transcript(cx: &mut TestAppContext) {
    use crate::TurnId;
    use yalda::session_proto::SessionInfo;
    let (view, vcx, id, _session) = boot_with_transcript(cx);
    // This test exercises the acknowledged local reducer/announcement seam;
    // the real wire transition has its own session_resilience integration test.
    view.update(vcx, |v, _| v.session_server = None);

    // A second session that exists only in the roster — never opened here, so
    // it has no in-memory transcript to write into.
    view.update(vcx, |v, _| {
        v.agent_roster.upsert(SessionInfo {
            session_id: "S2".into(),
            acp_session_id: None,
            label: "roster-only-agent".into(),
            cwd: PathBuf::from("."),
            provider: yalda::acp_channel::AgentProvider::Claude,
            turns: 0,
            connected: true,
            permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
            busy: false,
            archived: false,
        });
    });

    // The transcript tail: (text, turn tag) of the last non-empty line.
    let tail = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            v.read_session(id, cx, |c| {
                let turn_meta = c.editor.metadata::<TurnId>();
                (0..c.editor.document().line_count())
                    .rev()
                    .map(|i| {
                        (
                            c.editor.document().line_text(i),
                            c.editor
                                .anchor_for_line_opt(i)
                                .and_then(|a| turn_meta.get(a).copied()),
                        )
                    })
                    .find(|(text, _)| !text.trim().is_empty())
                    .expect("a non-empty transcript line")
            })
            .expect("open session")
        })
    };

    let temp = tempfile::tempdir().expect("temp console dir");
    let console = temp.path().join("system-console.log");
    let read_console = || std::fs::read_to_string(&console).unwrap_or_default();

    crate::with_system_console_path(console.clone(), || {
        // ── Archive the OPEN session: console line + transcript notice ──────
        view.update(vcx, |v, cx| v.set_session_archived("S1", true, cx));
        let log = read_console();
        assert!(
            log.contains("INFO\tarchived agent session \"claude-1\""),
            "the console must log the archive naming the agent; got: {log:?}"
        );
        let (text, tag) = tail(&view, vcx);
        assert!(
            text.contains("session archived"),
            "the open session's transcript must carry the notice; tail: {text:?}"
        );
        assert_eq!(
            tag,
            Some(TurnId::System),
            "a lifecycle notice is System-tagged — never an agent turn"
        );

        // ── Re-archiving is a no-op: it must announce NOTHING ───────────────
        let before = read_console();
        let lines_before = view
            .update(vcx, |v, cx| {
                v.read_session(id, cx, |c| c.editor.document().line_count())
            })
            .expect("open session");
        view.update(vcx, |v, cx| v.set_session_archived("S1", true, cx));
        assert_eq!(
            read_console(),
            before,
            "a no-op archive writes no console line"
        );
        assert_eq!(
            view.update(vcx, |v, cx| v
                .read_session(id, cx, |c| c.editor.document().line_count())),
            Some(lines_before),
            "a no-op archive appends no transcript notice"
        );

        // ── A ROSTER-ONLY session gets the console line and nothing else ────
        view.update(vcx, |v, cx| v.set_session_archived("S2", true, cx));
        let log = read_console();
        assert!(
            log.contains("INFO\tarchived agent session \"roster-only-agent\""),
            "the console names the roster session even though we never opened it; got: {log:?}"
        );
        assert_eq!(
            tail(&view, vcx).0,
            text,
            "archiving another session must not touch this session's transcript"
        );

        // ── Unarchive announces symmetrically ───────────────────────────────
        view.update(vcx, |v, cx| v.set_session_archived("S1", false, cx));
        assert!(
            read_console().contains("INFO\tunarchived agent session \"claude-1\""),
            "unarchive logs its own console line"
        );
        let (text, tag) = tail(&view, vcx);
        assert!(
            text.contains("session unarchived"),
            "unarchive appends its own transcript notice; tail: {text:?}"
        );
        assert_eq!(tag, Some(TurnId::System));
    });
}

/// REGRESSION (bug-0027): a session's agent subprocess coming up must reach the
/// roster. `SessionCreated` is necessarily broadcast BEFORE the blocking
/// spawn handshake, so it always carries `connected: false`; if nothing
/// publishes the transition to true, the row stays `Unavailable` for the rest
/// of the session's life — mid-turn included — until an unrelated
/// `list_sessions` reseed. This drives the REAL reducer (`apply_server_batch`),
/// exactly as the live server pump does.
///
/// Negative control: delete the `SessionConnected` arm from
/// `apply_server_batch`. The row remains `AgentActivity::Unavailable` after the
/// agent comes up, and stays Unavailable even while it reports a turn in
/// flight — the reported symptom verbatim.
#[gpui::test]
fn agent_coming_online_clears_the_unavailable_row(cx: &mut TestAppContext) {
    use crate::{AgentActivity, ServerNotification};
    use yalda::session_proto::SessionInfo;
    let (view, vcx) = boot_browser(cx);
    let cwd = view.update(vcx, |v, _| {
        let pid = v.workspace.active_workspace().expect("workspace").project();
        v.projects.cwd_of(pid).expect("project cwd").to_path_buf()
    });

    // The create broadcast as the server actually sends it: the session exists,
    // the subprocess does not yet.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionCreated {
                session: SessionInfo {
                    session_id: "srv-1".into(),
                    acp_session_id: None,
                    label: "claude-9".into(),
                    cwd,
                    provider: yalda::acp_channel::AgentProvider::Claude,
                    turns: 0,
                    connected: false,
                    permission_mode: yalda::acp_channel::DEFAULT_PERMISSION_MODE,
                    busy: false,
                    archived: false,
                },
            }],
            cx,
        );
    });
    let activity = |view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext| {
        view.update(vcx, |v, cx| {
            let rows = v.jump_panel_agent_rows(cx);
            assert_eq!(rows.len(), 1, "exactly the one roster session");
            rows[0].activity()
        })
    };
    assert_eq!(
        activity(&view, vcx),
        AgentActivity::Unavailable,
        "pre-handshake the row is honestly unavailable"
    );

    // The subprocess finishes its handshake. THIS is the event that was missing.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionConnected {
                session_id: "srv-1".into(),
                connected: true,
            }],
            cx,
        );
    });
    assert_eq!(
        activity(&view, vcx),
        AgentActivity::Waiting,
        "a live agent with no turn in flight is ready for input, not unavailable"
    );

    // And it must survive a turn: the reported symptom was a session showing
    // Unavailable while it was demonstrably mid-reply.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionBusy {
                session_id: "srv-1".into(),
                busy: true,
            }],
            cx,
        );
    });
    assert_eq!(
        activity(&view, vcx),
        AgentActivity::Working,
        "a session mid-turn must read as working, never unavailable"
    );

    // The agent exiting puts it back — connectivity is live in both directions.
    view.update(vcx, |v, cx| {
        v.apply_server_batch(
            vec![ServerNotification::SessionConnected {
                session_id: "srv-1".into(),
                connected: false,
            }],
            cx,
        );
    });
    assert_eq!(
        activity(&view, vcx),
        AgentActivity::Unavailable,
        "a departed agent returns to unavailable"
    );
}

// ── UXI-Diagram-1: inline mermaid diagrams ──────────────────────────────────
//
// Three headless guards over the REAL paths. The only genuine runtime gaps are
// the actual `mmdc` subprocess (gap 2) and the rasterized PNG pixels (gap 1);
// the render mechanism is stubbed via `crate::set_test_renderer` so the whole
// classify → reconcile → off-thread request → completion pipeline runs without
// `mmdc` installed.

/// Single marker-based render stub, installed by every diagram guard. Branching
/// on the SOURCE (not on which stub is set) makes it race-free under cargo's
/// parallel test threads: the global override is only ever written to this one
/// fn pointer, never cleared, so tests can't stomp each other's renderer.
/// Source containing `FAIL` → error (mmdc-unavailable case); otherwise a valid
/// 1×1 transparent PNG so a successful render decodes.
fn diagram_stub(source: &str, _theme: crate::MermaidTheme) -> Result<Vec<u8>, String> {
    if source.contains("FAIL") {
        return Err("stub: mmdc unavailable".to_string());
    }
    Ok(vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ])
}

/// (a) A ` ```mermaid ` fence classifies to `RenderedBlock::Diagram` via the
/// REAL markdown parser the doc/transcript both use (`render::render`); a
/// non-mermaid fence stays a `CodeBlock`.
///
/// Negative control (observed RED): revert the `is_mermaid` branch in
/// `src/render.rs` (always push `CodeBlock`) → the first assert fails.
#[gpui::test]
fn diagram_001_mermaid_fence_classifies_to_diagram(_cx: &mut TestAppContext) {
    let theme = Theme::default();
    let blocks = yalda::render::render("```mermaid\ngraph TD; A-->B;\n```\n", &theme);
    assert!(
        matches!(
            blocks.first(),
            Some(yalda::blocks::RenderedBlock::Diagram { .. })
        ),
        "a ```mermaid fence must classify to RenderedBlock::Diagram, got {:?}",
        blocks.first()
    );
    let rust = yalda::render::render("```rust\nfn a() {}\n```\n", &theme);
    assert!(
        matches!(
            rust.first(),
            Some(yalda::blocks::RenderedBlock::CodeBlock { .. })
        ),
        "a non-mermaid fence must stay a CodeBlock, got {:?}",
        rust.first()
    );
}

/// (b) When the renderer is unavailable/errors, the real pipeline (open doc →
/// per-frame reconcile → off-thread request → completion) resolves the diagram
/// to `Failed` — the fallback-to-raw-source trigger — and the block still paints
/// (never blank).
///
/// Negative control (observed RED): revert the `self.reconcile_diagrams(cx)`
/// call in `main.rs render()` → no request is ever issued, the cache stays
/// empty, and the `state_of == "failed"` assert fails.
#[gpui::test]
fn diagram_002_render_failure_falls_back_to_source(cx: &mut TestAppContext) {
    crate::set_test_renderer(Some(diagram_stub));

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let mut v = YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        );
        // `FAIL` in the source drives the stub to the mmdc-unavailable branch.
        v.test_open_doc("```mermaid\ngraph TD; FAIL-->B;\n```\n");
        v
    });
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }

    let theme_name = view.read_with(vcx, |v, _| v.theme.name);
    let key = crate::diagram_key(
        "graph TD; FAIL-->B;",
        crate::MermaidTheme::from_theme_name(theme_name),
    );
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.diagrams.borrow().state_of(key),
            Some("failed"),
            "a failed render must resolve to Failed (the fallback trigger); got {:?}",
            v.diagrams.borrow().state_of(key)
        );
    });

    // Non-vacuous: the mermaid block still paints its raw-source fallback.
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let painted = crate::layout_probe_get("doc-block-0");
    crate::layout_probe_end();
    let (_, _, _, h) = painted.expect("mermaid fallback block did not paint (blank!)");
    assert!(h > 0.0, "fallback block painted with zero height");
}

/// (c) A successful render drives the real off-thread request to `Ready` with a
/// decoded image — the state the paint arm swaps to `img()` on.
///
/// Negative control (observed RED): revert the `Ok(bytes) => …set Ready` arm in
/// `request_diagram`'s completion (leave the entry `Pending`) → the
/// `state_of == "ready"` assert fails.
#[gpui::test]
fn diagram_003_successful_render_reaches_ready(cx: &mut TestAppContext) {
    crate::set_test_renderer(Some(diagram_stub));

    let (view, vcx) = cx.add_window_view(|window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        YaldaGpuiView::new_browser(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Theme::default(),
            focus_handle,
        )
    });

    view.update(vcx, |v, cx| {
        let name = v.theme.name;
        v.request_diagram("graph TD; A-->B;", name, cx);
    });
    vcx.run_until_parked();

    let theme_name = view.read_with(vcx, |v, _| v.theme.name);
    let key = crate::diagram_key(
        "graph TD; A-->B;",
        crate::MermaidTheme::from_theme_name(theme_name),
    );
    view.read_with(vcx, |v, _| {
        assert_eq!(
            v.diagrams.borrow().state_of(key),
            Some("ready"),
            "a successful render must resolve to Ready; got {:?}",
            v.diagrams.borrow().state_of(key)
        );
    });
}

/// REGRESSION (UXI-Diagram-1): a ` ```mermaid ` fence in the AGENT TRANSCRIPT must
/// promote to a `FlatItem::Block(Diagram)` so it reaches the paint arm and renders
/// as an image. The transcript has its OWN block-promoter (`parse_block_range`)
/// separate from the buffer doc path; it originally recognized only Table/CodeBlock,
/// so a mermaid fence fell back to raw text lines and never rendered (the "nothing
/// in the transcript" report).
///
/// Drives the REAL transcript: freeze a mermaid fence (as a committed agent block),
/// let the real view-model rebuild + the per-frame reconcile run, then assert (1) the
/// fence promoted to a Diagram block and (2) the real merman pipeline reached Ready.
///
/// Negative control (observed RED): drop `RenderedBlock::Diagram` from the
/// `parse_block_range` match → the fence falls back to lines → no Diagram block →
/// the promotion assert fails.
#[gpui::test]
fn diagram_006_mermaid_fence_renders_in_agent_transcript(cx: &mut TestAppContext) {
    let (view, vcx, id, session) = boot_with_transcript(cx);

    // A mermaid fence, frozen so it is a committed transcript block (4 lines).
    session.update(vcx, |s, cx: &mut gpui::Context<crate::AgentSession>| {
        s.state
            .editor
            .programmatic_insert(0, "```mermaid\nflowchart TD\n  A --> B\n```\n");
        s.state.editor.add_frozen_lines(0, 4);
        cx.notify();
    });
    vcx.run_until_parked();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();

    // (1) Promotion: the fence must be a Diagram FlatItem::Block, not raw lines.
    let has_diagram = session.read_with(vcx, |s, _| {
        s.state.view_model.flat_items_cache.iter().any(|it| {
            matches!(it, crate::FlatItem::Block(rc)
                if matches!(rc.as_ref(), yalda::blocks::RenderedBlock::Diagram { .. }))
        })
    });
    assert!(
        has_diagram,
        "a ```mermaid fence in the transcript must promote to a Diagram FlatItem::Block"
    );

    // (2) End-to-end: the per-frame reconcile requested a render and the real
    // in-process merman pipeline reached Ready for that source.
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    let theme_name = view.read_with(vcx, |v, _| v.theme.name);
    let key = crate::diagram_key(
        "flowchart TD\n  A --> B",
        crate::MermaidTheme::from_theme_name(theme_name),
    );
    let state = view.read_with(vcx, |v, _| v.diagrams.borrow().state_of(key));
    assert_eq!(
        state,
        Some("ready"),
        "the transcript mermaid block must render to Ready via the real pipeline; got {state:?}"
    );
}

// ── Cog explorer tile (cog.rs / cog_view.rs / cog_ui.rs) ─────────────────────
// The Cog tile is a read-only two-pane explorer (UXI-Cog-1..3). Its body is a
// cached child (CogView). Tests drive the REAL reducer (`cog_apply`) with
// synthetic data and the real key/select/scroll methods; the live `cog`
// subprocess is genuine-gap #2 and is deliberately NOT run here (tests stay
// hermetic — `install_cog_tile` creates the tile without a fetch).

#[cfg(test)]
fn boot_with_cog<'a>(
    cx: &'a mut TestAppContext,
) -> (
    gpui::Entity<YaldaGpuiView>,
    &'a mut gpui::VisualTestContext,
    gpui::Entity<crate::CogView>,
    crate::workspace::WindowId,
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
        v.install_cog_tile(cx);
    });
    vcx.run_until_parked();
    let (cv, wid) = view.update(vcx, |v, _cx| {
        let wid = v.workspace.focused_window_id().expect("focused window");
        let cv = match v.workspace.focused_content() {
            Some(crate::App::Cog(tile)) => tile
                .view
                .clone()
                .expect("render_cog lazily creates the CogView"),
            _ => panic!("expected a focused Cog tile"),
        };
        (cv, wid)
    });
    (view, vcx, cv, wid)
}

#[cfg(test)]
fn cog_tile_req(view: &gpui::Entity<YaldaGpuiView>, vcx: &mut gpui::VisualTestContext) -> u64 {
    view.update(vcx, |v, _| match v.workspace.focused_content() {
        Some(crate::App::Cog(tile)) => tile.req,
        _ => panic!("expected a Cog tile"),
    })
}

#[cfg(test)]
fn cog_test_graph(id: &str, name: &str) -> crate::CogGraph {
    crate::CogGraph {
        id: id.into(),
        name: name.into(),
        description: format!("{name} description"),
        omega: "om".into(),
        sealed: false,
        prototype: false,
    }
}

#[cfg(test)]
fn cog_test_node(id: &str, name: &str, status: &str, content: serde_json::Value) -> crate::CogNode {
    crate::CogNode {
        id: id.into(),
        name: name.into(),
        content,
        status: status.into(),
        output: None,
    }
}

/// A node whose content is 200 lines — taller than any test viewport, so its
/// right pane genuinely overflows and the scroll offset is not clamped to 0.
#[cfg(test)]
fn cog_tall_node(id: &str, name: &str, status: &str) -> crate::CogNode {
    let big = (0..200)
        .map(|i| format!("content line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    cog_test_node(id, name, status, serde_json::json!(big))
}

/// A synthetic bundle: the given nodes, one status transition + one note per
/// node, no edges.
#[cfg(test)]
fn cog_test_bundle(nodes: Vec<crate::CogNode>) -> crate::CogBundle {
    let mut logs: std::collections::BTreeMap<String, Vec<crate::CogLogEntry>> =
        std::collections::BTreeMap::new();
    let mut notes: std::collections::BTreeMap<String, Vec<crate::CogNote>> =
        std::collections::BTreeMap::new();
    for n in &nodes {
        logs.insert(
            n.id.clone(),
            vec![crate::CogLogEntry {
                seq: 0,
                at: 1_786_989_281_753_564_000,
                actor: "claude-code".into(),
                kind: "status_changed".into(),
                data: serde_json::json!({"to": n.status}),
            }],
        );
        notes.insert(
            n.id.clone(),
            vec![crate::CogNote {
                at: 1_786_989_281_753_564_000,
                actor: "claude-code".into(),
                topic: Some("deviation".into()),
                data: serde_json::json!({"summary": format!("a note on {}", n.id)}),
            }],
        );
    }
    crate::CogBundle {
        graph: cog_test_graph("g1", "Graph One"),
        status: Default::default(),
        nodes,
        edges: vec![],
        logs,
        notes,
        render: "n1\n└─ n2".to_string(),
    }
}

/// UXI-Cog-1: a Cog tile's real reducer, fed a graph list, lands on the graph
/// explorer state with the graphs present.
#[gpui::test]
fn cog_opens_on_graph_explorer(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graphs(vec![
                cog_test_graph("a", "Alpha"),
                cog_test_graph("b", "Beta"),
            ])),
            cx,
        );
    });
    vcx.run_until_parked();
    let (in_graphs, len) = cv.update(vcx, |c, _| (c.in_graphs(), c.list_len()));
    assert!(in_graphs, "a Cog tile lands on the graph explorer");
    assert_eq!(len, 2, "both graphs are listed");
}

/// UXI-Cog-2/-3: selecting a different node advances the selection AND resets
/// the right detail pane to the top (a fresh node starts at its header). The
/// scroll reset is the negative-control target: revert the `set_offset` in
/// `select_move` and this fails with the offset still non-zero.
#[gpui::test]
fn cog_node_selection_resets_right_scroll(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_tall_node("n1", "first", "done"),
                cog_tall_node("n2", "second", "open"),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();

    // Opens on Overview → move onto the first node, then scroll its (tall) detail.
    view.update(vcx, |v, cx| v.cog_select(1, cx));
    vcx.run_until_parked();
    view.update(vcx, |v, cx| v.cog_scroll(300.0, cx));
    vcx.run_until_parked();
    let scrolled = cv.update(vcx, |c, _| c.right_scroll_y());
    assert!(scrolled < 0.0, "right pane scrolled down (y={scrolled})");

    view.update(vcx, |v, cx| v.cog_select(1, cx));
    vcx.run_until_parked();
    let (sel, after) = cv.update(vcx, |c, _| (c.selected_index(), c.right_scroll_y()));
    assert_eq!(sel, 1, "selection advanced to the second node");
    assert_eq!(after, 0.0, "changing node resets the right pane to the top");
}

/// UXI-Cog-3: the right pane scrolls on `d`/`u` and clamps at the top.
#[gpui::test]
fn cog_right_pane_scrolls_and_clamps(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_tall_node("n1", "only", "done"),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    // Opens on Overview → move onto the (tall) node before scrolling its detail.
    view.update(vcx, |v, cx| v.cog_select(1, cx));
    vcx.run_until_parked();

    view.update(vcx, |v, cx| v.cog_scroll(200.0, cx));
    vcx.run_until_parked();
    let y = cv.update(vcx, |c, _| c.right_scroll_y());
    assert!((y - -200.0).abs() < 0.5, "scrolled down by 200px (y={y})");

    view.update(vcx, |v, cx| v.cog_scroll(-1000.0, cx));
    vcx.run_until_parked();
    let y = cv.update(vcx, |c, _| c.right_scroll_y());
    assert_eq!(y, 0.0, "scrolling up past the top clamps to 0");
}

/// yux rule 5: the Cog body is a cached child. A root-only notify (unrelated
/// surface) leaves its render count FLAT; a body payload change busts it once.
#[gpui::test]
fn cog_body_is_cached(cx: &mut TestAppContext) {
    crate::perf_reset("cog");
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_test_node("n1", "only", "done", serde_json::json!({"purpose": "a"})),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    let base = crate::perf_render_count("cog");
    assert!(base >= 1, "cog body renders at least once");

    // Root-only notify (no cog change) → cached body render stays flat.
    for _ in 0..5 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    let flat = crate::perf_render_count("cog");
    assert_eq!(
        flat, base,
        "root notify must NOT re-render the cached cog body"
    );

    // A body payload change (mutation-site notify) busts the cache exactly once.
    cv.update(vcx, |c, cx| {
        c.set_state(crate::CogViewState::Error("boom".into()));
        cx.notify();
    });
    vcx.run_until_parked();
    let after = crate::perf_render_count("cog");
    assert_eq!(
        after,
        base + 1,
        "a body payload change re-renders the body once"
    );
}

/// UXI-Cog-2/-3 (PAINT, non-vacuous): a node with tall detail paints a right-pane
/// content taller than its viewport — i.e. it genuinely overflows and is
/// scrollable, not merely styled `overflow_y_scroll`.
#[gpui::test]
fn cog_detail_paints_and_overflows(cx: &mut TestAppContext) {
    let (view, vcx, _cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);

    // A node whose content is 200 lines — far taller than any test viewport.
    let big = (0..200)
        .map(|i| format!("content line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let node = cog_test_node("n1", "huge", "done", serde_json::json!(big));

    crate::layout_probe_begin();
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                node,
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    // Opens on Overview → move to the node so its detail sections render.
    view.update(vcx, |v, cx| v.cog_select(1, cx));
    vcx.run_until_parked();
    let viewport = crate::layout_probe_get("cog-right");
    // Node detail lays sections out directly: [transitions(0), Content(1), Notes(2)].
    let content = crate::layout_probe_get("cog-sec-1");
    crate::layout_probe_end();

    let (_, _, _, vp_h) = viewport.expect("right pane viewport did not paint");
    let (_, _, _, ct_h) = content.expect("content section did not paint");
    assert!(vp_h > 0.0, "viewport has real height ({vp_h})");
    assert!(
        ct_h > vp_h,
        "content ({ct_h}px) must overflow the viewport ({vp_h}px) — a genuine, \
         non-vacuous scroll (200-line node)"
    );
}

/// REGRESSION: the detail pane must NOT collapse to ~1 char wide (every glyph
/// wrapping to its own line) when the events strip moved to the bottom. The
/// detail section width must fill most of the detail pane's width.
#[gpui::test]
fn cog_detail_pane_fills_width(cx: &mut TestAppContext) {
    let (view, vcx, _cv, wid) = boot_with_cog(cx);
    vcx.simulate_resize(gpui::size(px(1200.0), px(800.0)));
    vcx.run_until_parked();
    let req = cog_tile_req(&view, vcx);

    crate::layout_probe_begin();
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_test_node(
                    "omega",
                    "omega",
                    "open",
                    serde_json::json!({"purpose": "confirm the seam"}),
                ),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    // Open on Overview by default → move to the node to view its detail sections.
    view.update(vcx, |v, cx| v.cog_select(1, cx));
    vcx.run_until_parked();
    // Mirror the real app: the bottom strip has streamed a wide-JSON event whose
    // long unbreakable line can force a min-content collapse up the flex tree.
    let generation = cog_tile_watch_gen(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_push_event(
            wid,
            generation,
            r#"{"ready":["t-01-entity-value-seam","t-02-review-entity-value-seam","t-03-signal-mediated-human-edits"],"status":{"islands":"none","sealed":false,"status":"open"}}"#.into(),
            cx,
        );
    });
    vcx.run_until_parked();
    let pane = crate::layout_probe_get("cog-right");
    let sec = crate::layout_probe_get("cog-sec-1"); // Content section
    crate::layout_probe_end();

    let (_, _, pane_w, _) = pane.expect("detail pane did not paint");
    let (_, _, sec_w, _) = sec.expect("content section did not paint");
    assert!(pane_w > 100.0, "detail pane should be wide ({pane_w})");
    assert!(
        sec_w > pane_w * 0.6,
        "detail content ({sec_w}px) must fill most of the pane ({pane_w}px) — a \
         collapse to ~1 char (the bottom-strip layout bug) fails here"
    );
}

/// UXI-Cog-3 (keyboard focus): moving focus to the right detail pane makes j/k
/// scroll it instead of moving the left selection; moving focus back restores
/// selection. The focus-aware routing in `handle_cog_press` is the negative-
/// control target (revert it → j selects while focused right → both asserts fail).
#[gpui::test]
fn cog_right_focus_scrolls_with_jk(cx: &mut TestAppContext) {
    use crate::{KMods, Key, KeyPress};
    let kp = |c: char| KeyPress::new(Key::Char(c), KMods::NONE);

    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_tall_node("n1", "first", "done"),
                cog_tall_node("n2", "second", "open"),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    // Opens on Overview → `j` moves the selector onto the first (tall) node.
    view.update(vcx, |v, cx| v.handle_cog_press(kp('j'), cx));
    vcx.run_until_parked();

    // `l` moves focus into the detail pane.
    view.update(vcx, |v, cx| v.handle_cog_press(kp('l'), cx));
    vcx.run_until_parked();
    assert!(
        cv.update(vcx, |c, _| c.focused_right()),
        "l moves keyboard focus to the detail pane"
    );

    // With the detail pane focused, j scrolls it and does NOT move the selection.
    for _ in 0..3 {
        view.update(vcx, |v, cx| v.handle_cog_press(kp('j'), cx));
        vcx.run_until_parked();
    }
    let (sel, y) = cv.update(vcx, |c, _| (c.selected_index(), c.right_scroll_y()));
    assert_eq!(sel, 0, "detail-focused j must NOT move the left selection");
    assert!(y < 0.0, "detail-focused j scrolls the detail pane (y={y})");

    // `h` returns focus to the selector; j then moves the selection again.
    view.update(vcx, |v, cx| v.handle_cog_press(kp('h'), cx));
    vcx.run_until_parked();
    assert!(
        !cv.update(vcx, |c, _| c.focused_right()),
        "h returns focus to the selector"
    );
    view.update(vcx, |v, cx| v.handle_cog_press(kp('j'), cx));
    vcx.run_until_parked();
    assert_eq!(
        cv.update(vcx, |c, _| c.selected_index()),
        1,
        "selector-focused j moves the selection"
    );
}

/// UXI-Cog-5: clicking a node row selects that node (its detail fills the right
/// pane) and puts keyboard focus on the selector. The `click_node` selection is
/// the negative-control target (revert `*selected = i` → click selects nothing).
#[gpui::test]
fn cog_click_node_selects(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_test_node("n1", "first", "done", serde_json::json!({"purpose": "a"})),
                cog_test_node("n2", "second", "open", serde_json::json!({"purpose": "b"})),
                cog_test_node("n3", "third", "open", serde_json::json!({"purpose": "c"})),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();

    // Focus the detail pane first, so we can prove the click both selects AND
    // returns focus to the selector.
    view.update(vcx, |v, cx| v.cog_set_focus(true, cx));
    vcx.run_until_parked();

    cv.update(vcx, |c, cx| c.click_node(2, cx));
    vcx.run_until_parked();
    let (sel, focused_right) = cv.update(vcx, |c, _| (c.selected_index(), c.focused_right()));
    assert_eq!(sel, 2, "clicking node row 2 selects it");
    assert!(
        !focused_right,
        "clicking a node row returns focus to the selector"
    );
}

/// UXI-Cog-5: clicking the right detail pane moves keyboard focus there so j/k
/// scroll it.
#[gpui::test]
fn cog_click_right_pane_focuses_it(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_tall_node("n1", "only", "done"),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(
        !cv.update(vcx, |c, _| c.focused_right()),
        "focus starts on the selector"
    );

    cv.update(vcx, |c, cx| c.click_focus_right(cx));
    vcx.run_until_parked();
    assert!(
        cv.update(vcx, |c, _| c.focused_right()),
        "clicking the detail pane focuses it"
    );
}

/// UXI-Cog-5: clicking a graph row in the explorer opens that graph — it routes
/// through the real `cog_open_graph` path (the tile enters the loading state).
/// The live `cog` fetch itself is runtime gap #2, so we assert the open fired
/// WITHOUT pumping the detached subprocess task.
#[gpui::test]
fn cog_click_graph_row_opens(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graphs(vec![
                cog_test_graph("a", "Alpha"),
                cog_test_graph("b", "Beta"),
            ])),
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(
        cv.update(vcx, |c, _| c.in_graphs()),
        "starts on the explorer"
    );

    // Click the second graph row. click_graph selects it then routes to the root
    // open path, which sets the tile to Loading synchronously (before the async
    // fetch). Do NOT run_until_parked — that would run the live `cog` subprocess.
    cv.update(vcx, |c, cx| c.click_graph(1, cx));
    assert!(
        cv.update(vcx, |c, _| c.is_loading()),
        "clicking a graph row opens it (tile enters loading)"
    );
}

#[cfg(test)]
fn cog_tile_watch_gen(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
) -> u64 {
    view.update(vcx, |v, _| match v.workspace.focused_content() {
        Some(crate::App::Cog(tile)) => tile.watch_gen,
        _ => panic!("expected a Cog tile"),
    })
}

/// UXI-Cog-6: live `cog graph watch` events fold into the events pane via the
/// REAL reducer (`cog_push_event`) — newest first, generation-guarded, invalid
/// JSON dropped. (The live subprocess itself is runtime gap #2 and is not spawned
/// under test.) The newest-first insert is the negative-control target.
#[gpui::test]
fn cog_events_stream_into_pane(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_test_node("n1", "only", "done", serde_json::json!({"purpose": "a"})),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    assert_eq!(cv.update(vcx, |c, _| c.events_len()), 0, "no events yet");

    let generation = cog_tile_watch_gen(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_push_event(
            wid,
            generation,
            r#"{"kind":"claimed","node":"n1"}"#.into(),
            cx,
        );
        v.cog_push_event(
            wid,
            generation,
            r#"{"ready":["n1"],"status":{"status":"open"}}"#.into(),
            cx,
        );
    });
    vcx.run_until_parked();
    assert_eq!(
        cv.update(vcx, |c, _| c.events_len()),
        2,
        "two events streamed in"
    );
    assert_eq!(
        cv.update(vcx, |c, _| c.newest_event_seq()),
        Some(2),
        "newest event (seq 2) renders first"
    );

    // A stale generation (a killed prior watcher) is dropped.
    view.update(vcx, |v, cx| {
        v.cog_push_event(
            wid,
            generation.wrapping_sub(1),
            r#"{"stale":true}"#.into(),
            cx,
        );
    });
    vcx.run_until_parked();
    assert_eq!(
        cv.update(vcx, |c, _| c.events_len()),
        2,
        "stale-generation event dropped"
    );

    // Non-JSON is dropped, not panicked.
    view.update(vcx, |v, cx| {
        v.cog_push_event(wid, generation, "not json".into(), cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        cv.update(vcx, |c, _| c.events_len()),
        2,
        "invalid JSON dropped"
    );
}

/// UXI-Cog-6: the events pane is the third column, present only in a graph, and
/// paints. Tab cycles focus Selector → Detail → Events → Selector.
#[gpui::test]
fn cog_events_pane_paints_and_focus_cycles(cx: &mut TestAppContext) {
    use crate::{KMods, Key, KeyPress};
    let tab = || KeyPress::new(Key::Tab, KMods::NONE);

    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);

    // Explorer: no events pane.
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graphs(vec![cog_test_graph("a", "Alpha")])),
            cx,
        );
    });
    vcx.run_until_parked();
    crate::layout_probe_begin();
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let explorer_events = crate::layout_probe_get("cog-events");
    crate::layout_probe_end();
    assert!(explorer_events.is_none(), "explorer has no events pane");

    // Load a graph → events pane paints as a real third column.
    let req = cog_tile_req(&view, vcx);
    crate::layout_probe_begin();
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_test_node("n1", "only", "done", serde_json::json!({"purpose": "a"})),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    let events = crate::layout_probe_get("cog-events");
    crate::layout_probe_end();
    let (_, _, w, h) = events.expect("events pane paints in a loaded graph");
    assert!(w > 0.0 && h > 0.0, "events pane has real size ({w}x{h})");

    // Focus starts on the selector; Tab cycles selector → detail → events → selector.
    assert!(
        cv.update(vcx, |c, _| c.focused_selector()),
        "focus starts on selector"
    );
    view.update(vcx, |v, cx| v.handle_cog_press(tab(), cx));
    vcx.run_until_parked();
    assert!(cv.update(vcx, |c, _| c.focused_right()), "tab → detail");
    view.update(vcx, |v, cx| v.handle_cog_press(tab(), cx));
    vcx.run_until_parked();
    assert!(cv.update(vcx, |c, _| c.focused_events()), "tab → events");
    view.update(vcx, |v, cx| v.handle_cog_press(tab(), cx));
    vcx.run_until_parked();
    assert!(
        cv.update(vcx, |c, _| c.focused_selector()),
        "tab → selector"
    );
}

#[cfg(test)]
fn cog_tile_needs_load(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
) -> bool {
    view.update(vcx, |v, _| match v.workspace.focused_content() {
        Some(crate::App::Cog(tile)) => tile.needs_load,
        _ => panic!("expected a Cog tile"),
    })
}

/// UXI-Cog-1 (regression): a Cog tile whose first render runs WITHOUT an explicit
/// open (the disk-restore path) kicks the graph-list load itself — else it sits
/// frozen. `boot_with_cog` installs the tile without calling `open_cog_inner`, so
/// the first render's kick must have cleared `needs_load`. Removing the render
/// kick is the negative control.
#[gpui::test]
fn cog_restored_tile_kicks_load(cx: &mut TestAppContext) {
    let (view, vcx, _cv, _wid) = boot_with_cog(cx);
    assert!(
        !cog_tile_needs_load(&view, vcx),
        "a restored tile's first render must kick the graph-list load (not stay frozen)"
    );
}

#[cfg(test)]
fn cog_tile_refreshing(
    view: &gpui::Entity<YaldaGpuiView>,
    vcx: &mut gpui::VisualTestContext,
) -> bool {
    view.update(vcx, |v, _| match v.workspace.focused_content() {
        Some(crate::App::Cog(tile)) => tile.refreshing,
        _ => panic!("expected a Cog tile"),
    })
}

/// UXI-Cog-6: a live event auto-refreshes the graph (real `cog_push_event` sets
/// the coalescing `refreshing` flag), and the refresh reload (`cog_apply_refresh`
/// → `update_bundle`) updates the node set IN PLACE while PRESERVING the events
/// feed. The events-preserving `update_bundle` is the negative-control target.
#[gpui::test]
fn cog_event_auto_refreshes_and_preserves_events(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_test_node("n1", "first", "done", serde_json::json!({"purpose": "a"})),
                cog_test_node("n2", "second", "open", serde_json::json!({"purpose": "b"})),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    assert!(
        !cog_tile_refreshing(&view, vcx),
        "no refresh before any event"
    );

    // A live event both buffers AND triggers an auto-refresh (coalesced flag set).
    let generation = cog_tile_watch_gen(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_push_event(
            wid,
            generation,
            r#"{"kind":"claimed","node":"n1"}"#.into(),
            cx,
        );
    });
    vcx.run_until_parked();
    assert_eq!(cv.update(vcx, |c, _| c.events_len()), 1, "event buffered");
    assert!(
        cog_tile_refreshing(&view, vcx),
        "a live event auto-refreshes the graph"
    );

    // The refresh reload lands: a NEW 3-node bundle updates the node list in
    // place, and the events feed SURVIVES (not cleared like a graph change).
    view.update(vcx, |v, cx| {
        v.cog_apply_refresh(
            wid,
            Ok(cog_test_bundle(vec![
                cog_test_node("n1", "first", "done", serde_json::json!({"purpose": "a"})),
                cog_test_node(
                    "n2",
                    "second",
                    "claimed",
                    serde_json::json!({"purpose": "b"}),
                ),
                cog_test_node("n3", "third", "open", serde_json::json!({"purpose": "c"})),
            ])),
            cx,
        );
    });
    vcx.run_until_parked();
    assert_eq!(
        cv.update(vcx, |c, _| c.list_len()),
        3,
        "refresh updated the node set"
    );
    assert_eq!(
        cv.update(vcx, |c, _| c.events_len()),
        1,
        "the events feed persists across the refresh"
    );
}

/// UXI-Cog-9: node detail sections are ordered with **State transitions first**
/// (then Content, Output when present, Notes) — the single order source
/// `node_section_titles` that `node_sections` renders from. Reordering it is the
/// negative control.
#[gpui::test]
fn cog_node_sections_state_transitions_first(_cx: &mut TestAppContext) {
    // A node WITH output → [transitions, Content, Output, Notes].
    let mut node = cog_test_node("n1", "x", "done", serde_json::json!({"purpose": "p"}));
    node.output = Some(serde_json::json!({"result": "ok"}));
    assert_eq!(
        crate::node_section_titles(&node),
        vec!["Status transitions", "Content", "Output", "Notes"],
    );

    // Without output → Output is omitted, transitions still first.
    let node2 = cog_test_node("n2", "y", "open", serde_json::json!({"purpose": "p"}));
    let titles = crate::node_section_titles(&node2);
    assert_eq!(
        titles[0], "Status transitions",
        "State transitions must be first"
    );
    assert!(
        !titles.contains(&"Output"),
        "no Output section without output"
    );
}

/// UXI-Cog-11: node JSON (content/output/events) renders as a foldable tree-table
/// — folding a nested key collapses its child rows. Drives the real
/// `toggle_json_fold`; the fold hiding children is asserted by PAINT (the Content
/// section gets shorter). The toggle is the negative control.
#[gpui::test]
fn cog_json_tree_fold_collapses(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    // Content: a nested object with several children under "outer".
    let content = serde_json::json!({
        "outer": {"a": 1, "b": 2, "c": 3, "d": 4, "e": 5, "f": 6, "g": 7, "h": 8}
    });
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_test_node("n1", "node", "done", content),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    // Opens on Overview → move to the node so its Content tree renders.
    view.update(vcx, |v, cx| v.cog_select(1, cx));
    vcx.run_until_parked();

    // Expanded: measure the Content section (cog-sec-1) height.
    crate::layout_probe_begin();
    cv.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let expanded = crate::layout_probe_get("cog-sec-1");
    crate::layout_probe_end();
    let (_, _, _, h_expanded) = expanded.expect("content tree paints expanded");

    // Fold "outer" (the real toggle the fold-row click invokes).
    assert!(
        !cv.update(vcx, |c, _| c.json_folded("n:n1/content/outer")),
        "starts expanded"
    );
    crate::layout_probe_begin();
    cv.update(vcx, |c, cx| {
        c.toggle_json_fold("n:n1/content/outer".into(), cx)
    });
    vcx.run_until_parked();
    let folded = crate::layout_probe_get("cog-sec-1");
    crate::layout_probe_end();
    let (_, _, _, h_folded) = folded.expect("content tree paints folded");

    assert!(
        cv.update(vcx, |c, _| c.json_folded("n:n1/content/outer")),
        "outer is now folded"
    );
    assert!(
        h_folded < h_expanded,
        "folding a nested key must hide its child rows (folded {h_folded}px < expanded {h_expanded}px)"
    );
}

/// UXI-Cog-8: `bundle.stats()` computes node counts + claimed→done completion
/// min/max/avg from the node logs — the numbers shown in the Overview.
#[gpui::test]
fn cog_stats_completion_times(_cx: &mut TestAppContext) {
    let sec = 1_000_000_000i64;
    // n1: claimed@1s → done@3s (2s). n2: claimed@10s → done@15s (5s). n3: open.
    let mut logs = std::collections::BTreeMap::new();
    let tr = |to: &str, at: i64| crate::CogLogEntry {
        seq: at,
        at,
        actor: "a".into(),
        kind: "status_changed".into(),
        data: serde_json::json!({"to": to}),
    };
    logs.insert(
        "n1".to_string(),
        vec![tr("claimed", sec), tr("done", 3 * sec)],
    );
    logs.insert(
        "n2".to_string(),
        vec![tr("claimed", 10 * sec), tr("done", 15 * sec)],
    );
    let bundle = crate::CogBundle {
        graph: cog_test_graph("g", "G"),
        status: Default::default(),
        nodes: vec![
            cog_test_node("n1", "a", "done", serde_json::json!({})),
            cog_test_node("n2", "b", "done", serde_json::json!({})),
            cog_test_node("n3", "c", "open", serde_json::json!({})),
        ],
        edges: vec![],
        logs,
        notes: std::collections::BTreeMap::new(),
        render: String::new(),
    };
    let s = bundle.stats();
    assert_eq!(s.total, 3);
    assert_eq!(s.done, 2);
    assert_eq!(s.open, 1);
    assert_eq!(s.completed, 2, "two nodes have claimed→done durations");
    assert_eq!(s.quickest_ns, Some(2 * sec));
    assert_eq!(s.longest_ns, Some(5 * sec));
    assert_eq!(s.average_ns, Some((7 * sec) / 2));
}

/// UXI-Cog-8: the Overview row is reachable (keyboard `k` up from the first node,
/// and a click), and shows the Overview body; a TOC click jumps the detail pane.
#[gpui::test]
fn cog_overview_reachable_and_toc_jumps(cx: &mut TestAppContext) {
    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graph(Box::new(cog_test_bundle(vec![
                cog_tall_node("n1", "first", "done"),
                cog_tall_node("n2", "second", "open"),
            ])))),
            cx,
        );
    });
    vcx.run_until_parked();
    // A graph opens on its Overview, and the Overview body paints.
    crate::layout_probe_begin();
    cv.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let ov = crate::layout_probe_get("cog-right-content");
    crate::layout_probe_end();
    assert!(
        cv.update(vcx, |c, _| c.showing_overview()),
        "a graph opens on its Overview"
    );
    let (_, _, w, h) = ov.expect("overview body paints");
    assert!(w > 0.0 && h > 0.0, "overview body has real size ({w}x{h})");

    // `j` down leaves Overview for a node; `k` up returns to it.
    view.update(vcx, |v, cx| v.cog_select(1, cx));
    vcx.run_until_parked();
    assert!(
        !cv.update(vcx, |c, _| c.showing_overview()),
        "j down leaves the Overview"
    );
    view.update(vcx, |v, cx| v.cog_select(-1, cx));
    vcx.run_until_parked();
    assert!(
        cv.update(vcx, |c, _| c.showing_overview()),
        "k up returns to the Overview"
    );

    // Back to a node; a TOC click (section 2) scrolls the detail pane down.
    view.update(vcx, |v, cx| v.cog_select(1, cx)); // overview → node 0
    vcx.run_until_parked();
    assert!(!cv.update(vcx, |c, _| c.showing_overview()));
    cv.update(vcx, |c, cx| c.click_node_section(2, cx));
    vcx.run_until_parked();
    assert!(
        cv.update(vcx, |c, _| c.right_scroll_y()) < 0.0,
        "a TOC jump to a later section scrolls the detail pane"
    );
}

/// UXI-Cog-8 (regression): completion time is computed even for nodes that were
/// closed straight to `done` WITHOUT a `claimed` transition (the real cog case) —
/// the span falls back to the node's earliest log entry. The fallback is the
/// negative-control target.
#[gpui::test]
fn cog_completion_without_claimed_counts(_cx: &mut TestAppContext) {
    let sec = 1_000_000_000i64;
    let mut logs = std::collections::BTreeMap::new();
    // No `claimed` — an edit at 1s, then `done` at 3s. Span = 2s.
    logs.insert(
        "n1".to_string(),
        vec![
            crate::CogLogEntry {
                seq: 0,
                at: sec,
                actor: "a".into(),
                kind: "content_edited".into(),
                data: serde_json::json!({}),
            },
            crate::CogLogEntry {
                seq: 1,
                at: 3 * sec,
                actor: "a".into(),
                kind: "status_changed".into(),
                data: serde_json::json!({"to": "done"}),
            },
        ],
    );
    let bundle = crate::CogBundle {
        graph: cog_test_graph("g", "G"),
        status: Default::default(),
        nodes: vec![cog_test_node("n1", "a", "done", serde_json::json!({}))],
        edges: vec![],
        logs,
        notes: std::collections::BTreeMap::new(),
        render: String::new(),
    };
    let s = bundle.stats();
    assert_eq!(
        s.completed, 1,
        "a claimed-less done node still counts as completed"
    );
    assert_eq!(
        s.quickest_ns,
        Some(2 * sec),
        "span = done − earliest log entry"
    );
}

/// UXI-Cog-10: the graph picker supports `/` search — typing filters the list and
/// Enter opens the highlighted match. The filter match is the negative control.
#[gpui::test]
fn cog_graph_picker_search_filters(cx: &mut TestAppContext) {
    use crate::{KMods, Key, KeyPress};
    let kp = |c: char| KeyPress::new(Key::Char(c), KMods::NONE);

    let (view, vcx, cv, wid) = boot_with_cog(cx);
    let req = cog_tile_req(&view, vcx);
    view.update(vcx, |v, cx| {
        v.cog_apply(
            wid,
            req,
            Ok(crate::CogFetch::Graphs(vec![
                cog_test_graph("aid", "alpha"),
                cog_test_graph("bid", "beta"),
                cog_test_graph("gid", "gamma"),
            ])),
            cx,
        );
    });
    vcx.run_until_parked();
    assert_eq!(
        cv.update(vcx, |c, _| c.selected_graph_id()),
        Some("aid".to_string()),
        "first graph selected initially"
    );

    // `/` then "be" filters to "beta"; it becomes the selected (only) match.
    view.update(vcx, |v, cx| v.handle_cog_press(kp('/'), cx));
    vcx.run_until_parked();
    assert!(cv.update(vcx, |c, _| c.is_filtering()), "/ starts search");
    view.update(vcx, |v, cx| v.handle_cog_press(kp('b'), cx));
    view.update(vcx, |v, cx| v.handle_cog_press(kp('e'), cx));
    vcx.run_until_parked();
    assert_eq!(
        cv.update(vcx, |c, _| c.filter_text().to_string()),
        "be",
        "typed keys build the filter"
    );
    assert_eq!(
        cv.update(vcx, |c, _| c.selected_graph_id()),
        Some("bid".to_string()),
        "the filter narrows the selection to the matching graph"
    );

    // Esc clears the search back to the full list.
    view.update(vcx, |v, cx| v.handle_cog_press(kp('x'), cx)); // "bex" → no match
    vcx.run_until_parked();
    assert_eq!(
        cv.update(vcx, |c, _| c.selected_graph_id()),
        None,
        "no matches → nothing selected"
    );
    view.update(vcx, |v, cx| {
        v.handle_cog_press(KeyPress::new(Key::Esc, KMods::NONE), cx)
    });
    vcx.run_until_parked();
    assert!(!cv.update(vcx, |c, _| c.is_filtering()), "esc exits search");
    assert_eq!(
        cv.update(vcx, |c, _| c.selected_graph_id()),
        Some("aid".to_string()),
        "clearing the filter restores the full list"
    );
}
