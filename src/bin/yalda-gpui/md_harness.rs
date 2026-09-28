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
    // The Edit and Doc bodies are cached children: a root notify alone
    // replays their last paint (no probes recorded), so invalidate them too.
    view.update(vcx, |v, cx| {
        if let Some(App::Buffer(BufferApp::Editing(e))) = v.workspace.focused_content()
            && let Some(body) = &e.body
        {
            body.update(cx, |_, bcx| bcx.notify());
        }
        v.test_notify_doc_bodies(cx);
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

// ---------------------------------------------------------------------------
// render-fixes (graph 4f1): GFM constructs painted in the Doc view.
// ---------------------------------------------------------------------------

/// Force `n` frames (the real render + paint path), re-running the cached Doc
/// body too. The body re-renders on its own inputs; one input the headless
/// harness can't deliver is an image finishing its decode — gpui re-notifies
/// the painting view via `on_next_frame`, which the test platform never fires
/// (`TestWindow::on_request_frame` is a no-op). Invalidating the body here
/// stands in for that frame callback.
fn frames_n(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext, n: usize) {
    for _ in 0..n {
        view.update(vcx, |v, cx| v.test_notify_doc_bodies(cx));
        vcx.run_until_parked();
    }
}

/// Boot a Doc with the layout probe recording from the first frame.
fn boot_probed<'a>(
    cx: &'a mut TestAppContext,
    tag: &str,
    markdown: &str,
) -> (Entity<YaldaGpuiView>, &'a mut VisualTestContext, PathBuf) {
    crate::layout_probe_begin();
    let (view, vcx, file) = boot_doc(cx, tag, markdown);
    frames_n(&view, vcx, 1);
    (view, vcx, file)
}

fn probe_get(tag: &str) -> Option<(f32, f32, f32, f32)> {
    crate::layout_probe_get(tag)
}

/// A solid-color RGB PNG of `w`×`h` (stored-deflate zlib, no compressor dep).
fn png(w: u32, h: u32) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in bytes {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend(crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::new();
    for _ in 0..h {
        raw.push(0u8); // filter: none
        for _ in 0..w {
            raw.extend_from_slice(&[0x20, 0x80, 0xc0]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = b.len() as u16;
        z.extend(len.to_le_bytes());
        z.extend((!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Task items paint a checkbox (addressable as `md-task-<block>.<item…>`)
/// instead of a bullet; plain items don't. Negative control: in
/// `list_item_element`, prefer `item.marker` over `item.checked` (the old
/// order) → no checkbox paints.
#[gpui::test]
fn task_list_items_paint_checkboxes(cx: &mut TestAppContext) {
    let md = "- [ ] todo\n- [x] done\n- plain\n\n* [ ] parent\n  * [x] child\n";
    let (_view, _vcx, _file) = boot_probed(cx, "tasks", md);
    for tag in ["md-task-0.0", "md-task-0.1", "md-task-1.0", "md-task-1.0.1.0"] {
        let (_, _, w, h) = probe_get(tag).unwrap_or_else(|| panic!("{tag} checkbox painted"));
        assert!(w > 0.0 && h > 0.0, "{tag} has area: {w}x{h}");
    }
    assert!(probe_get("md-task-0.2").is_none(), "a plain item has no checkbox");
    // The box sits left of the item text, inside the block's column.
    let (bx, _, bw, _) = probe_get("md-task-0.0").unwrap();
    let (cx0, _, cw, _) = probe_get("doc-block-inner-0").expect("block column");
    assert!(bx >= cx0 && bx + bw < cx0 + cw);
    crate::layout_probe_end();
}

/// An image alone in its paragraph paints the picture: a wide one fitted to
/// the column with its aspect kept, a small one at its own size; a missing
/// file falls back to the alt text. Negative control: drop the lone-image
/// lift in `render.rs`'s `Paragraph` arm → no image paints.
#[gpui::test]
fn images_paint_fitted_to_the_column(cx: &mut TestAppContext) {
    let dir = temp_dir("images");
    std::fs::create_dir_all(dir.join("img")).unwrap();
    std::fs::write(dir.join("img/wide.png"), png(3000, 300)).unwrap();
    std::fs::write(dir.join("small.png"), png(40, 20)).unwrap();
    let md = "![wide](img/wide.png)\n\n![small](small.png)\n\n![gone](missing.png)\n";
    let (view, vcx, _file) = boot_probed(cx, "images", md);
    // Decoding is off-thread; the loader re-notifies. Pump until it lands.
    for _ in 0..20 {
        if probe_get("md-image-0").is_some() && probe_get("md-image-1").is_some() {
            break;
        }
        frames_n(&view, vcx, 1);
    }
    let (_, _, col_w, _) = probe_get("doc-block-inner-0").expect("block column");
    let (_, _, w, h) = probe_get("md-image-0").expect("wide image painted");
    assert!(3000.0 > col_w, "non-vacuous: the image is wider than the column");
    assert!(w <= col_w + 0.5 && w > 0.0, "fitted: {w} ≤ column {col_w}");
    assert!((h - w / 10.0).abs() < 1.0, "aspect kept: {w}x{h}");
    let (_, _, _, box_h) = probe_get("md-image-0-box").expect("image box");
    assert!((box_h - h).abs() < 1.0, "the laid-out box is the fitted height: {box_h} vs {h}");
    let (_, _, sw, sh) = probe_get("md-image-1").expect("small image painted");
    assert_eq!((sw, sh), (40.0, 20.0), "never upscaled");
    assert!(probe_get("md-image-alt-2").is_some(), "missing file shows its alt text");
    assert!(probe_get("md-image-2").is_none());
    crate::layout_probe_end();
}

/// A hard break (two trailing spaces / backslash) paints the next text on its
/// own line; a soft break doesn't. Negative control: collapse `HardBreak` to
/// a space in `collect_inline` → one painted line.
#[gpui::test]
fn hard_breaks_paint_separate_lines(cx: &mut TestAppContext) {
    let md = "first  \nsecond\\\nthird\nsame line\n";
    let (view, vcx, _file) = boot_doc(cx, "hardbreak", md);
    let ys: Vec<Option<f32>> = view.read_with(vcx, |v, _| {
        let lls = v.focused_doc_line_layouts();
        let layouts = lls.borrow();
        (0..4)
            .map(|li| {
                layouts
                    .get(&(0, li))
                    .and_then(|l| l.bounds().origin.y.into())
                    .map(f32::from)
            })
            .collect()
    });
    assert!(ys[0].is_some() && ys[1].is_some() && ys[2].is_some(), "three lines: {ys:?}");
    assert!(ys[3].is_none(), "the soft break stays inline: {ys:?}");
    assert!(ys[0] < ys[1] && ys[1] < ys[2], "stacked top to bottom: {ys:?}");
}

/// Column alignment (`:--` / `:-:` / `--:`) places each cell's text. Negative
/// control: make `table_element` ignore `alignments` (always Left) → the
/// centered / right texts hug their columns' left edges.
#[gpui::test]
fn table_columns_honor_alignment(cx: &mut TestAppContext) {
    let md = "| Left | Center | Right |\n|:--|:-:|--:|\n| a | b | c |\n";
    let (_view, _vcx, _file) = boot_probed(cx, "tablealign", md);
    let (x0, _, w, _) = probe_get("doc-block-inner-0").expect("block column");
    // The table starts after the column's left padding (`pl_3`).
    let (x0, w) = (x0 + 12.0, w - 12.0);
    let col = w / 3.0;
    let (ax, _, aw, _) = probe_get("md-cell-0.1.0").expect("a");
    let (bx, _, bw, _) = probe_get("md-cell-0.1.1").expect("b");
    let (cx_, _, cw, _) = probe_get("md-cell-0.1.2").expect("c");
    assert!(aw < col / 2.0, "non-vacuous: cell text narrower than its column");
    assert!(ax - x0 < 16.0, "left: hugs the left edge ({ax} vs {x0})");
    let b_mid = bx + bw / 2.0;
    let col_mid = x0 + col * 1.5;
    assert!((b_mid - col_mid).abs() < 4.0, "center: {b_mid} ≈ {col_mid}");
    assert!(x0 + w - (cx_ + cw) < 16.0, "right: hugs the right edge");
    assert!(cx_ > x0 + 2.0 * col + col / 2.0, "right text in the right half of its column");
    crate::layout_probe_end();
}

/// Footnote definitions paint as their own de-emphasized block; references
/// stay inline. Negative control: drop `ENABLE_FOOTNOTES` → no footnote block.
#[gpui::test]
fn footnote_definitions_paint_as_blocks(cx: &mut TestAppContext) {
    let md = "Claim[^1].\n\n[^1]: The source.\n";
    let (view, vcx, _file) = boot_probed(cx, "footnotes", md);
    let para = view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Viewing(d))) => match &d.blocks[0] {
            yalda::blocks::RenderedBlock::Paragraph { lines } => lines[0].text_content(),
            other => panic!("paragraph first: {other:?}"),
        },
        _ => panic!("expected a Doc"),
    });
    assert_eq!(para, "Claim¹.");
    let (_, _, w, h) = probe_get("md-footnote-1").expect("footnote block painted");
    assert!(w > 0.0 && h > 0.0);
    crate::layout_probe_end();
}

// ---------------------------------------------------------------------------
// doc-view (graph 4f1): the Doc body is a cached yux component (`DocView`).
// ---------------------------------------------------------------------------

/// Boot `(Doc of left, Edit of right)` side by side in one workspace through
/// the real paths: `open_file` (Doc in place of the browser), then
/// `make_doc_content` and `split_focused` (a pooled tile of `right`, focused),
/// then `enter_edit_with` (toggled into Edit, Insert). `same_file` makes
/// `right` the SAME file as `left` (one pooled core).
fn boot_doc_beside_edit<'a>(
    cx: &'a mut TestAppContext,
    tag: &str,
    left_md: &str,
    right_md: &str,
    same_file: bool,
) -> (Entity<YaldaGpuiView>, &'a mut VisualTestContext) {
    cx.update(crate::register_keymap);
    let dir = temp_dir(tag);
    let left = dir.join("left.md");
    let right = if same_file { left.clone() } else { dir.join("right.md") };
    std::fs::write(&left, left_md).expect("write left");
    if !same_file {
        std::fs::write(&right, right_md).expect("write right");
    }
    let (view, vcx) = cx.add_window_view(hermetic_browser_view);
    view.update(vcx, |v, cx| {
        v.splash_until = None;
        assert!(v.open_file(left.clone()), "open the Doc");
        let content = v.make_doc_content(&right).expect("doc content");
        v.workspace
            .split_focused(crate::workspace::SplitDir::V, content)
            .expect("split");
        v.enter_edit_with(crate::EditView::Code, cx);
        v.edit_mut().expect("edit tile").mode = crate::EditMode::Insert;
        cx.notify();
    });
    for _ in 0..3 {
        view.update(vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    (view, vcx)
}

/// The painted text of every line the unfocused Doc tile's body registered in
/// its hit-test sink — filled at PAINT time, so this is what the user sees.
fn painted_doc_text(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext) -> String {
    view.read_with(vcx, |v, _| {
        let mut out = Vec::new();
        v.workspace.workspaces[v.workspace.active_workspace].for_each_attached_window(
            &mut |w| {
                if let App::Buffer(BufferApp::Viewing(d)) = &w.content {
                    let ll = d.line_layouts.borrow();
                    let mut keys: Vec<_> = ll.keys().copied().collect();
                    keys.sort();
                    for k in keys {
                        out.push(ll[&k].text().to_string());
                    }
                }
            },
        );
        out.join("\n")
    })
}

/// yux rule 5 render-count guard for the cached Doc body. A Doc of file A
/// beside an Edit of file B: typing in B (real keystrokes through the keymap
/// and `handle_edit_key`, each a root notify) must leave the Doc body's render
/// count FLAT; the global inputs — zoom (`cmd-=`) and theme (`ToggleTheme`) —
/// must each re-render it.
///
/// Negative controls (observed RED): make the root-observe filter in
/// `DocView::new` notify unconditionally → the flat assert fails; drop
/// `text_scale_bits` / `theme` from `DocSeqs::of` (default them) → the zoom /
/// theme bust asserts fail.
#[gpui::test]
fn doc_body_is_render_flat_while_another_file_is_edited(cx: &mut TestAppContext) {
    let left: String = (0..30).map(|i| format!("Left paragraph {i}.\n\n")).collect();
    let (view, vcx) = boot_doc_beside_edit(cx, "docview-flat", &left, "right side\n", false);
    let base = crate::perf_render_count("doc-body");
    assert!(base >= 1, "the Doc body must have rendered (live beside the Edit)");
    assert!(painted_doc_text(&view, vcx).contains("Left paragraph 0."), "the Doc painted");

    for key in ["q", "u", "x"] {
        vcx.simulate_keystrokes(key);
        vcx.run_until_parked();
    }
    let typed = view.update(vcx, |v, _| v.edit_mut().expect("edit").editor.full_text());
    assert!(typed.contains("qux"), "non-vacuous: the keystrokes landed in B: {typed:?}");
    let after = crate::perf_render_count("doc-body");
    assert_eq!(
        after, base,
        "typing in another file's Edit tile must not re-render the cached Doc body ({base} → {after})"
    );

    vcx.simulate_keystrokes("cmd-=");
    vcx.run_until_parked();
    let zoomed = crate::perf_render_count("doc-body");
    assert!(zoomed > after, "zoom must re-render the Doc body ({after} → {zoomed})");

    vcx.dispatch_action(crate::ToggleTheme);
    vcx.run_until_parked();
    let themed = crate::perf_render_count("doc-body");
    assert!(themed > zoomed, "a theme change must re-render the Doc body ({zoomed} → {themed})");
}

/// A Doc and an Edit tile of the SAME pooled file: typing in the Edit tile
/// (real keystrokes) re-derives the Doc's blocks on the effect path (the
/// root's self-observe → `refresh_painted_docs`) and the cached Doc body
/// repaints them — asserted on the text the Doc's body actually PAINTED (its
/// paint-time hit-test sink), not on the model.
///
/// Negative controls (observed RED): make `ensure_doc_refresh_hook` a no-op →
/// the Doc keeps painting the old text; drop `blocks_ptr`/`blocks_seq`/
/// `source_seq` from `DocSeqs::of` → the body never re-renders, same failure.
#[gpui::test]
fn doc_body_repaints_a_same_file_sibling_edit(cx: &mut TestAppContext) {
    let (view, vcx) =
        boot_doc_beside_edit(cx, "docview-same", "alpha paragraph\n", "", true);
    assert!(painted_doc_text(&view, vcx).contains("alpha paragraph"), "the Doc painted");
    let before = crate::perf_render_count("doc-body");
    for key in ["q", "u", "x"] {
        vcx.simulate_keystrokes(key);
        vcx.run_until_parked();
    }
    let typed = view.update(vcx, |v, _| v.edit_mut().expect("edit").editor.full_text());
    assert!(typed.contains("qux"), "non-vacuous: the keystrokes landed: {typed:?}");
    assert!(
        crate::perf_render_count("doc-body") > before,
        "the sibling edit must re-render the Doc body"
    );
    let painted = painted_doc_text(&view, vcx);
    assert!(
        painted.contains("qux"),
        "the Doc must PAINT the sibling Edit tile's text, not stale content: {painted:?}"
    );
}
