//! Headless guards for markdown viewing/editing (graph 4f1): source mapping,
//! outline rail, view⇄edit position, Doc rendering. Drives the real
//! `YaldaGpuiView` like `verify_harness.rs`; kept separate so this project's
//! tests don't churn that file.

use crate::verify_harness::hermetic_browser_view;
use crate::{App, BufferApp, YaldaGpuiView};
use gpui::{Entity, TestAppContext, VisualTestContext};
use std::path::PathBuf;

/// A unique temp dir for one test.
pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yalda-md-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Boot a hermetic view and open `markdown` from a real file through the real
/// `open_file` path, splash skipped and a frame painted.
pub(crate) fn boot_doc<'a>(
    cx: &'a mut TestAppContext,
    tag: &str,
    markdown: &str,
) -> (Entity<YaldaGpuiView>, &'a mut VisualTestContext, PathBuf) {
    let dir = temp_dir(tag);
    let file = dir.join("doc.md");
    std::fs::write(&file, markdown).expect("write fixture");
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    view.update(vcx, |v, _| {
        assert!(v.open_file(file.clone()), "open fixture");
        v.splash_until = None;
    });
    for _ in 0..2 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    (view, vcx, file)
}

/// A file opened into the Doc view carries a source span per rendered block,
/// pointing at that block's markdown. Negative control: build the Doc with
/// `render_with_wiki` (unmapped) in `open_file`'s doc constructor → spans empty.
#[gpui::test]
fn opened_doc_blocks_are_source_mapped(cx: &mut TestAppContext) {
    let md = "# Title\n\nPara\n\n- a\n- b\n\n## Sub\n";
    let (view, vcx, _file) = boot_doc(cx, "mapped", md);
    let (n_blocks, spans) = view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Viewing(d))) => (d.blocks.len(), d.spans.clone()),
        _ => panic!("expected a Doc"),
    });
    assert_eq!(n_blocks, 4);
    assert_eq!(spans.len(), n_blocks, "every block mapped");
    let starts: Vec<usize> = spans.iter().map(|s| s.lines.start).collect();
    assert_eq!(starts, vec![0, 2, 4, 7]);
}

// ── Outline rail (UXI-Rail-1..5) ─────────────────────────────────────────────

const SECTIONS_MD: &str = "# Alpha\n\npara a1\n\npara a2\n\n# Beta\n\npara b1\n\n```sh\n# not a heading\n```\n\npara b2\n\n## Beta sub\n\npara bs\n\n# Gamma\n\npara g1\n";

fn frames(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext) {
    for _ in 0..2 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
}

/// (entries, selected, current, rail focused)
fn outline_state(
    view: &Entity<YaldaGpuiView>,
    vcx: &mut VisualTestContext,
) -> (Vec<String>, usize, Option<usize>, bool) {
    view.read_with(vcx, |v, _| {
        let r = v
            .workspace
            .active_workspace()
            .and_then(|t| t.rail.as_ref())
            .expect("rail open");
        match &r.content {
            crate::workspace::RailContent::Outline(o) => (
                o.entries.iter().map(|e| e.text.clone()).collect(),
                o.selected,
                o.current,
                r.focused,
            ),
            _ => panic!("expected outline rail"),
        }
    })
}

fn doc_cursor(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext) -> (usize, usize) {
    view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Viewing(d))) => {
            (d.cursor_block, d.list.state().logical_scroll_top().item_ix)
        }
        _ => panic!("expected a Doc"),
    })
}

/// The outline lists real headings only (a `#` line in a code fence is code),
/// opens with its selection on the section you're in, previews as you move
/// through it, jumps (heading to the top) and returns focus on Enter, and then
/// follows the document cursor. Drives real keystrokes end to end.
///
/// Negative controls (each observed RED): drop the `|| first` sync in
/// `OutlineState::track_current` → opens at 0 not 1; drop
/// `outline_preview_selected()` from `rail_down` → cursor stays on block 5;
/// drop `track_current` from `refresh_outline_rail` → current stays stale.
#[gpui::test]
fn outline_rail_selection_tracks_and_jumps(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let md = format!("{SECTIONS_MD}{}", "\nfiller paragraph\n".repeat(30));
    let (view, vcx, _file) = boot_doc(cx, "outline-track", &md);
    vcx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(300.0)));

    // Into section "Beta": block 5 is the code fence under it.
    for _ in 0..5 {
        vcx.simulate_keystrokes("j");
    }
    frames(&view, vcx);
    assert_eq!(doc_cursor(&view, vcx).0, 5);

    vcx.simulate_keystrokes("cmd-shift-o");
    frames(&view, vcx);
    let (entries, selected, current, focused) = outline_state(&view, vcx);
    assert_eq!(entries, vec!["Alpha", "Beta", "Beta sub", "Gamma"], "fenced `#` is not a heading");
    assert!(focused, "opening the outline focuses it");
    assert_eq!(current, Some(1));
    assert_eq!(selected, 1, "the outline opens on the section you are in");

    // j in the rail previews: the doc follows the selection.
    vcx.simulate_keystrokes("j");
    frames(&view, vcx);
    assert_eq!(outline_state(&view, vcx).1, 2);
    assert_eq!(doc_cursor(&view, vcx).0, 7, "doc cursor follows the rail selection");

    // Enter jumps (heading scrolled to the top) and hands focus back.
    vcx.simulate_keystrokes("enter");
    frames(&view, vcx);
    let (_, selected, current, focused) = outline_state(&view, vcx);
    assert!(!focused, "Enter returns focus to the document");
    assert_eq!((selected, current), (2, Some(2)));
    assert_eq!(doc_cursor(&view, vcx), (7, 7), "heading is the top visible block");

    // Now the document has focus: moving its cursor moves "you are here".
    vcx.simulate_keystrokes("j j");
    frames(&view, vcx);
    assert_eq!(doc_cursor(&view, vcx).0, 9);
    let (_, selected, current, _) = outline_state(&view, vcx);
    assert_eq!(current, Some(3), "current section follows the doc cursor");
    assert_eq!(selected, 3, "unfocused rail selection follows too");

    // The rail doesn't wrap: k at the top stays at the top.
    vcx.simulate_keystrokes("cmd-shift-o cmd-shift-o");
    frames(&view, vcx);
    for _ in 0..6 {
        vcx.simulate_keystrokes("k");
    }
    frames(&view, vcx);
    assert_eq!(outline_state(&view, vcx).1, 0, "no wrap-around past the first heading");
}

/// A long outline in a short window keeps the selected row painted inside the
/// rail (the old fixed 40-row window ignored the rail's real height and let the
/// selection fall off the bottom).
///
/// Negative control (observed RED): remove the `scroll_to_item` reveal in the
/// outline render → the highlighted row is never painted.
#[gpui::test]
fn outline_rail_selected_row_stays_painted_in_short_window(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let md: String = (0..60).map(|i| format!("# Heading {i}\n\ntext {i}\n\n")).collect();
    let (view, vcx, _file) = boot_doc(cx, "outline-short", &md);
    vcx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(300.0)));
    vcx.simulate_keystrokes("cmd-shift-o");
    frames(&view, vcx);
    for _ in 0..50 {
        vcx.simulate_keystrokes("j");
    }
    frames(&view, vcx);
    assert_eq!(outline_state(&view, vcx).1, 50);

    crate::layout_probe_begin();
    frames(&view, vcx);
    let list = crate::layout_probe_get("outline-rail-list");
    let row = crate::layout_probe_get("outline-highlighted-row");
    crate::layout_probe_end();
    let (_, ly, _, lh) = list.expect("outline list painted");
    assert!(lh < 60.0 * 16.0, "non-vacuous: 60 rows cannot fit in {lh}px");
    let (_, ry, _, rh) = row.expect("selected outline row painted");
    assert!(ry >= ly && ry + rh <= ly + lh + 0.5, "row {ry}+{rh} inside list {ly}+{lh}");
}

// ---------------------------------------------------------------------------
// View ⇄ Edit position (UXI-Buffer-8/9/10)
// ---------------------------------------------------------------------------

/// `n` one-line paragraphs separated by blank lines: block `i` is source line
/// `2 * i`. Long enough that the rendered Doc and the raw Edit list both
/// overflow the test viewport (asserted non-vacuously in each test).
fn long_doc(n: usize) -> String {
    (0..n)
        .map(|i| format!("Paragraph number {i} of the position fixture."))
        .collect::<Vec<_>>()
        .join("\n\n")
        + "\n"
}

/// Force frames until any post-landing settle passes have run.
fn paint(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext) {
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
}

/// `(cursor_block, top item, doc list viewport (top, bottom))` of the focused Doc.
fn doc_pos(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext) -> (usize, usize, (f32, f32)) {
    view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Viewing(d))) => {
            let vp = d.list.state().viewport_bounds();
            (
                d.cursor_block,
                d.list.state().logical_scroll_top().item_ix,
                (f32::from(vp.top()), f32::from(vp.bottom())),
            )
        }
        _ => panic!("expected a Doc"),
    })
}

/// `(caret line, caret col, top item, edit list viewport (top, bottom))` of the focused Edit.
fn edit_pos(
    view: &Entity<YaldaGpuiView>,
    vcx: &mut VisualTestContext,
) -> (usize, usize, usize, (f32, f32)) {
    view.read_with(vcx, |v, cx| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Editing(e))) => {
            let c = e.editor.cursor();
            let list = &e.body.as_ref().expect("edit body built").read(cx).list;
            let vp = list.state().viewport_bounds();
            (
                c.line,
                c.col,
                list.state().logical_scroll_top().item_ix,
                (f32::from(vp.top()), f32::from(vp.bottom())),
            )
        }
        _ => panic!("expected an Edit view"),
    })
}

/// Paint one frame with the layout probe on and return `tag`'s painted rect.
fn probe(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext, tag: &str) -> Option<(f32, f32, f32, f32)> {
    crate::layout_probe_begin();
    // The Edit body is a cached child: a root notify alone replays its last
    // paint (no probes recorded), so invalidate it too.
    view.update(vcx, |v, cx| {
        if let Some(App::Buffer(BufferApp::Editing(e))) = v.workspace.focused_content()
            && let Some(body) = &e.body
        {
            body.update(cx, |_, bcx| bcx.notify());
        }
        cx.notify();
    });
    vcx.run_until_parked();
    let r = crate::layout_probe_get(tag);
    crate::layout_probe_end();
    r
}

fn assert_painted_inside(r: Option<(f32, f32, f32, f32)>, vp: (f32, f32), what: &str) {
    let (_, y, _, h) = r.unwrap_or_else(|| panic!("{what} was not painted (off-screen)"));
    assert!(
        y >= vp.0 - 0.5 && y + h <= vp.1 + 0.5,
        "{what} painted at y={y}..{} outside the viewport {vp:?}",
        y + h
    );
}

/// UXI-Buffer-8: Ctrl-E from a Doc whose cursor is deep in the file lands the
/// Edit caret at the focused block's first source line, painted inside the
/// edit viewport, with the Doc's top block's first line at the top.
///
/// Negative control (observed RED): without the landing in `enter_edit_with`
/// the caret is (0, 0) and the edit list shows line 0.
#[gpui::test]
fn doc_to_edit_lands_caret_on_focused_block(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _file) = boot_doc(cx, "d2e", &long_doc(150));
    for _ in 0..40 {
        vcx.simulate_keystrokes("j");
    }
    paint(&view, vcx);
    let (cursor_block, top_block, _) = doc_pos(&view, vcx);
    assert_eq!(cursor_block, 40);
    assert!(top_block > 0, "non-vacuous: the doc must have scrolled (top={top_block})");

    vcx.simulate_keystrokes("ctrl-e");
    paint(&view, vcx);
    let (line, col, top_line, vp) = edit_pos(&view, vcx);
    assert_eq!((line, col), (80, 0), "caret at block 40's first source line");
    // Raw rows (line + blank per block) are taller than the rendered blocks, so
    // the doc's top line can't stay on top AND keep the caret visible: the
    // landing starts at the mapped top and settles only as far as the caret.
    assert!(
        top_line >= 2 * top_block && top_line <= 80,
        "edit top {top_line} between the mapped top {} and the caret",
        2 * top_block
    );
    assert!(
        probe(&view, vcx, "code-line-0").is_none(),
        "non-vacuous: line 0 is scrolled out of the edit viewport"
    );
    assert_painted_inside(probe(&view, vcx, "code-line-80"), vp, "caret line 80");
}

/// UXI-Buffer-8 (top mapping): when the caret fits below it, the Doc's top
/// visible block's first source line becomes the top visible edit line.
///
/// Negative control (observed RED): without the landing the edit top is 0.
#[gpui::test]
fn doc_to_edit_keeps_top_block_on_top_when_caret_fits(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _file) = boot_doc(cx, "d2e-top", &long_doc(150));
    for _ in 0..40 {
        vcx.simulate_keystrokes("j");
    }
    paint(&view, vcx);
    let (_, top_block, _) = doc_pos(&view, vcx);
    // Walk the cursor back up to 3 blocks under the top (no doc scroll).
    for _ in 0..(40 - (top_block + 3)) {
        vcx.simulate_keystrokes("k");
    }
    paint(&view, vcx);
    let (cursor_block, top_again, _) = doc_pos(&view, vcx);
    assert_eq!((cursor_block, top_again), (top_block + 3, top_block));
    assert!(top_block > 0, "non-vacuous: the doc must have scrolled");

    vcx.simulate_keystrokes("ctrl-e");
    paint(&view, vcx);
    let (line, _, top_line, vp) = edit_pos(&view, vcx);
    assert_eq!(line, 2 * cursor_block);
    assert_eq!(top_line, 2 * top_block, "edit top line = the doc's top block's first line");
    assert_painted_inside(probe(&view, vcx, &format!("code-line-{line}")), vp, "caret line");
}

/// UXI-Buffer-9: Ctrl-V from Edit lands the Doc cursor on the block holding the
/// caret line, painted inside the doc viewport.
///
/// Negative control (observed RED): without the landing in `back_to_doc` the
/// cursor is block 0 and block 50 is never painted.
#[gpui::test]
fn edit_to_doc_lands_cursor_on_caret_block(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _file) = boot_doc(cx, "e2d", &long_doc(150));
    vcx.simulate_keystrokes("ctrl-e");
    paint(&view, vcx);
    for _ in 0..101 {
        vcx.simulate_keystrokes("j");
    }
    paint(&view, vcx);
    let (line, _, edit_top, _) = edit_pos(&view, vcx);
    assert_eq!(line, 101, "caret moved by real keystrokes (blank line after block 50)");
    assert!(edit_top > 0, "non-vacuous: the edit view must have scrolled");

    vcx.simulate_keystrokes("ctrl-v");
    paint(&view, vcx);
    let (cursor_block, top_block, vp) = doc_pos(&view, vcx);
    assert_eq!(cursor_block, 50, "cursor = the block holding caret line 101");
    assert_eq!(top_block, edit_top / 2, "doc top block = the block holding the top edit line");
    assert!(
        probe(&view, vcx, "doc-block-0").is_none(),
        "non-vacuous: block 0 is scrolled out of the doc viewport"
    );
    assert_painted_inside(probe(&view, vcx, "doc-block-50"), vp, "cursor block 50");
}

/// UXI-Buffer-10: a Doc→Edit→Doc round trip with no edit and no caret motion
/// returns to the same cursor block AND the same top block.
///
/// Negative control (observed RED): without the `doc_return` restore the top
/// block drifts / the cursor resets.
#[gpui::test]
fn doc_edit_doc_round_trip_keeps_place(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _file) = boot_doc(cx, "rt", &long_doc(150));
    for _ in 0..40 {
        vcx.simulate_keystrokes("j");
    }
    paint(&view, vcx);
    let before = doc_pos(&view, vcx);
    assert!(before.1 > 0, "non-vacuous: the doc must have scrolled");

    vcx.simulate_keystrokes("ctrl-e");
    paint(&view, vcx);
    vcx.simulate_keystrokes("ctrl-v");
    paint(&view, vcx);
    let after = doc_pos(&view, vcx);
    assert_eq!((after.0, after.1), (before.0, before.1), "same cursor block + top block");
    assert_painted_inside(probe(&view, vcx, "doc-block-40"), after.2, "cursor block 40");
}
