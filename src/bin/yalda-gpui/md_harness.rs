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

// ---------------------------------------------------------------------------
// checkbox-toggle (graph 4f1): UXI-Buffer-12 — toggling a task from the Doc.
// ---------------------------------------------------------------------------

/// The focused Doc's pooled source text (what autosave writes).
fn doc_source(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext) -> (String, bool) {
    view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Viewing(d))) => {
            let src = d.source.as_ref().expect("file-backed Doc");
            (src.full_text(), src.is_modified())
        }
        _ => panic!("expected a Doc"),
    })
}

/// Run `act` with a FRESH probe map, then ROOT-only frames (no forced body
/// notify): a probe recorded now proves the cached Doc body re-rendered on its
/// own inputs as a result of `act` — a cache-hit replay records none.
fn probed_after(
    view: &Entity<YaldaGpuiView>,
    vcx: &mut VisualTestContext,
    act: impl FnOnce(&mut VisualTestContext),
) {
    crate::layout_probe_begin();
    act(vcx);
    paint(view, vcx);
}

/// UXI-Buffer-12 (key): `x` on a focused task-list block checks its FIRST open
/// task through the shared buffer (dirty → autosaved to disk), the Doc PAINTS
/// the check, repeated `x` works down the list, and once every task is done
/// `x` unchecks the LAST one. The edit is one undo step in an Edit view.
///
/// Negative control (observed RED): drop the `ToggleTask` `on_action` in
/// `render_doc` (screens.rs) → the source is unchanged after `x`.
#[gpui::test]
fn x_toggles_the_focused_blocks_first_open_task(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let md = "Intro.\n\n- [x] one\n- [ ] two\n- [ ] three\n";
    let (view, vcx, file) = boot_probed(cx, "task-key", md);
    view.update(vcx, |v, cx| v.start_file_sync(cx));
    assert!(probe_get("md-task-1.1").is_some(), "the open box painted");
    assert!(probe_get("md-task-1.1-checked").is_none(), "non-vacuous: item 1 starts open");

    // `x` on a block with no task is a no-op on the text.
    vcx.simulate_keystrokes("x");
    vcx.run_until_parked();
    assert_eq!(doc_source(&view, vcx), (md.to_string(), false));

    vcx.simulate_keystrokes("j");
    paint(&view, vcx);
    probed_after(&view, vcx, |vcx| vcx.simulate_keystrokes("x"));
    let (text, dirty) = doc_source(&view, vcx);
    assert_eq!(text, "Intro.\n\n- [x] one\n- [x] two\n- [ ] three\n");
    assert!(dirty, "the toggle marks the buffer modified");
    assert!(
        probe_get("md-task-1.1-checked").is_some(),
        "the Doc body re-rendered and PAINTED item 1 checked"
    );
    assert!(probe_get("md-task-1.2-checked").is_none());

    // Autosave (UXI-Buffer-6) persists it.
    vcx.executor().advance_clock(std::time::Duration::from_millis(1500));
    vcx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text, "autosaved");

    vcx.simulate_keystrokes("x");
    vcx.run_until_parked();
    assert_eq!(doc_source(&view, vcx).0, "Intro.\n\n- [x] one\n- [x] two\n- [x] three\n");
    probed_after(&view, vcx, |vcx| vcx.simulate_keystrokes("x"));
    assert_eq!(
        doc_source(&view, vcx).0,
        "Intro.\n\n- [x] one\n- [x] two\n- [ ] three\n",
        "all done → x unchecks the last task"
    );
    assert!(probe_get("md-task-1.2-checked").is_none(), "item 2 painted open again");
    assert!(probe_get("md-task-1.2").is_some());

    // One undo step per toggle, in an Edit view of the same buffer.
    vcx.simulate_keystrokes("ctrl-e");
    vcx.run_until_parked();
    let normal =
        view.update(vcx, |v, _| v.edit_mut().expect("edit").mode == crate::EditMode::Normal);
    assert!(normal, "Edit opens in Normal");
    vcx.simulate_keystrokes("u");
    vcx.run_until_parked();
    let text = view.update(vcx, |v, _| v.edit_mut().expect("edit").editor.full_text());
    assert_eq!(text, "Intro.\n\n- [x] one\n- [x] two\n- [x] three\n", "u undid the last toggle");
    crate::layout_probe_end();
}

/// UXI-Buffer-12 (click): a real click on a painted checkbox toggles exactly
/// THAT item — nested items and `*` bullets included — both ways, and the Doc
/// PAINTS the new state. Nothing else in the source changes.
///
/// Negative control (observed RED): drop the `on_click` in
/// `task_checkbox_clickable` (render_blocks.rs) → the source is unchanged.
#[gpui::test]
fn clicking_a_checkbox_toggles_that_item(cx: &mut TestAppContext) {
    let md = "* [ ] parent\n  * [ ] child\n* [X] done\n";
    let (view, vcx, _file) = boot_probed(cx, "task-click", md);
    let click = |view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext, tag: &str| {
        let (x, y, w, h) = probe_get(tag).unwrap_or_else(|| panic!("{tag} painted"));
        let at = gpui::point(gpui::px(x + w / 2.0), gpui::px(y + h / 2.0));
        // A real click, split so the probe map is cleared between press and
        // release: the press anchors a (then empty) view selection, which
        // itself repaints the body with the PRE-toggle blocks.
        let (left, none) = (gpui::MouseButton::Left, gpui::Modifiers::default());
        vcx.simulate_mouse_down(at, left, none);
        vcx.run_until_parked();
        probed_after(view, vcx, |vcx| vcx.simulate_mouse_up(at, left, none));
    };

    click(&view, vcx, "md-task-0.0.1.0");
    assert_eq!(doc_source(&view, vcx).0, "* [ ] parent\n  * [x] child\n* [X] done\n");
    assert!(probe_get("md-task-0.0.1.0-checked").is_some(), "the child painted checked");
    assert!(probe_get("md-task-0.0-checked").is_none(), "the parent is untouched");

    click(&view, vcx, "md-task-0.1");
    assert_eq!(
        doc_source(&view, vcx).0,
        "* [ ] parent\n  * [x] child\n* [ ] done\n",
        "`[X]` unchecks to `[ ]`"
    );
    assert!(probe_get("md-task-0.1-checked").is_none(), "painted open");
    assert!(probe_get("md-task-0.1").is_some());
    crate::layout_probe_end();
}

// ---------------------------------------------------------------------------
// Reading typography (graph 4f1 typography): measure, type scale, hanging list
// markers, blockquote bar, code-block copy. Geometry in the headless harness:
// the test text system (`NoopTextSystem`) advances every glyph 0.6em, so the
// body font's `ch` at 14px is 8.4px and a 72ch measure is 604.8px.
// ---------------------------------------------------------------------------

const LONG_PARA: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.";

fn set_zoom(view: &Entity<YaldaGpuiView>, vcx: &mut VisualTestContext, scale: f32) {
    view.update(vcx, |v, cx| v.set_text_scale(scale, cx));
    frames_n(view, vcx, 2);
}

/// The Doc's text column is capped at the reading measure (72ch of the body
/// font, + the 15px cursor-bar/padding chrome) and centered in a wide tile;
/// it scales with zoom; in a narrow tile it takes the full width.
///
/// Negative control (observed RED): drop the `max_w(column_max)` wrapper in
/// `build_doc_body` → the column is the full ~1500px tile width.
#[gpui::test]
fn doc_reading_measure_caps_centers_and_scales(cx: &mut TestAppContext) {
    let md = format!("{LONG_PARA}\n\nsecond\n");
    let (view, vcx, _file) = boot_probed(cx, "measure", &md);
    vcx.simulate_resize(gpui::size(gpui::px(1600.0), gpui::px(600.0)));
    frames_n(&view, vcx, 2);
    let expect = |scale: f32| 72.0 * 0.6 * 14.0 * scale + 15.0;
    let (bx, _, bw, _) = probe_get("doc-body").expect("doc body painted");
    let (x, _, w, _) = probe_get("doc-column-0").expect("column painted");
    assert!(bw > 1000.0, "non-vacuous: a wide tile ({bw})");
    assert!((w - expect(1.0)).abs() < 1.0, "capped at the measure: {w} vs {}", expect(1.0));
    let (mid, body_mid) = (x + w / 2.0, bx + bw / 2.0);
    assert!((mid - body_mid).abs() < 1.0, "centered: column mid {mid} vs body mid {body_mid}");

    // The measure scales with zoom.
    set_zoom(&view, vcx, 1.5);
    let (_, _, w15, _) = probe_get("doc-column-0").expect("column painted at 1.5x");
    assert!((w15 - expect(1.5)).abs() < 1.0, "zoomed measure: {w15} vs {}", expect(1.5));

    // Narrow tile: full width (the body's inner width, minus its px_8).
    set_zoom(&view, vcx, 1.0);
    vcx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(600.0)));
    frames_n(&view, vcx, 2);
    let (_, _, nbw, _) = probe_get("doc-body").expect("doc body");
    let (_, _, nw, _) = probe_get("doc-column-0").expect("column");
    assert!(nw < expect(1.0), "non-vacuous: narrower than the measure");
    assert!((nw - (nbw - 64.0)).abs() < 1.0, "narrow tile: full width {nw} vs {}", nbw - 64.0);
    crate::layout_probe_end();
}

/// Hanging markers: every item of a list starts its text at the same x (one
/// gutter sized to the widest marker — `9.` vs `10.`), right of the marker,
/// and a wrapped item's continuation lines stay in the text column.
///
/// Negative control (observed RED): per-item `min_w(24)` marker (the old
/// layout) → at 2× zoom `10.` is wider than `9.` and the item texts misalign.
#[gpui::test]
fn list_markers_hang_in_a_shared_gutter(cx: &mut TestAppContext) {
    let md = format!("9. short\n10. {LONG_PARA}\n");
    let (view, vcx, _file) = boot_probed(cx, "hang", &md);
    vcx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(900.0)));
    set_zoom(&view, vcx, 2.0);
    let (t0x, _, _, _) = probe_get("md-li-0.0-text").expect("item 0 text");
    let (t1x, _, _, t1h) = probe_get("md-li-0.1-text").expect("item 1 text");
    let (m1x, _, m1w, _) = probe_get("md-li-0.1-marker").expect("item 1 marker");
    let (_, _, _, t0h) = probe_get("md-li-0.0-text").expect("item 0 text");
    assert!(t1h > 2.0 * t0h, "non-vacuous: the long item wraps ({t1h} vs one line {t0h})");
    assert!((t0x - t1x).abs() < 0.5, "item texts align: {t0x} vs {t1x}");
    assert!(t1x >= m1x + m1w, "the text (and its wrapped lines) sit right of the marker");
    crate::layout_probe_end();
}

/// A blockquote's left rule runs the full height of the quoted text.
#[gpui::test]
fn blockquote_bar_spans_the_quote(cx: &mut TestAppContext) {
    let md = format!("> {LONG_PARA}\n>\n> second quoted paragraph\n");
    let (_view, _vcx, _file) = boot_probed(cx, "quote", &md);
    let (bx, by, bw, bh) = probe_get("md-quote-bar-0").expect("bar painted");
    let (tx, ty, _, th) = probe_get("md-quote-text-0").expect("quote text painted");
    assert!(th > 100.0, "non-vacuous: the long quote wraps to several lines ({th})");
    assert!(bw >= 2.0 && bx < tx, "a visible rule left of the text: bar {bx},{by} {bw}x{bh} text {tx},{ty} h{th}");
    assert!((by - ty).abs() < 0.5 && (bh - th).abs() < 0.5, "rule spans the quote: {by}+{bh} vs {ty}+{th}");
    crate::layout_probe_end();
}

/// The copy button of a fenced code block (clicked through the real mouse
/// dispatch at its painted rect) writes the block's exact plain text to the
/// clipboard and flags it "Copied"; nested code blocks resolve by path too.
///
/// Negative control (observed RED): make `copy_doc_code_block` skip the
/// clipboard write → the clipboard stays empty.
#[gpui::test]
fn code_block_copy_button_writes_the_code(cx: &mut TestAppContext) {
    let code = "fn main() {\n    println!(\"hi\");\n}";
    let md = format!("intro\n\n```rust\n{code}\n```\n\n- item\n\n  ```\n  nested\n  ```\n");
    let (view, vcx, _file) = boot_probed(cx, "copy", &md);
    let (x, y, w, h) = probe_get("md-code-copy-1").expect("copy button painted");
    assert!(w > 0.0 && h > 0.0);
    vcx.simulate_click(gpui::point(gpui::px(x + w / 2.0), gpui::px(y + h / 2.0)), gpui::Modifiers::default());
    vcx.run_until_parked();
    let clip = view.update(vcx, |_, cx| cx.read_from_clipboard()).and_then(|c| c.text());
    assert_eq!(clip.as_deref(), Some(code), "the exact code text is on the clipboard");
    let copied = view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Viewing(d))) => d.code_copied.as_deref().cloned(),
        _ => panic!("expected a Doc"),
    });
    assert_eq!(copied, Some(vec![1]), "the button flags itself Copied");
    assert!(
        view.read_with(vcx, |v, _| v.doc_selection.is_none()),
        "the press did not start a text selection"
    );

    // A code block nested in a list item: path [2, item 0, content block 1].
    frames_n(&view, vcx, 1);
    let (x, y, w, h) = probe_get("md-code-copy-2.0.1").expect("nested copy button painted");
    vcx.simulate_click(gpui::point(gpui::px(x + w / 2.0), gpui::px(y + h / 2.0)), gpui::Modifiers::default());
    vcx.run_until_parked();
    let clip = view.update(vcx, |_, cx| cx.read_from_clipboard()).and_then(|c| c.text());
    assert_eq!(clip.as_deref(), Some("nested"));
    crate::layout_probe_end();
}

/// One type scale: a WP heading paints at the same size (line box) as the
/// Doc's heading of the same level.
///
/// Negative control (observed RED): restore WP's old 26px h1 → painted 34 vs 36.5.
#[gpui::test]
fn wp_heading_matches_doc_heading(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let md = "# Title\n\nbody\n\n## Sub\n\nmore\n";
    let (view, vcx, _file) = boot_probed(cx, "wph", md);
    let (_, _, _, d1) = probe_get("md-heading-0").expect("doc h1");
    let (_, _, _, d2) = probe_get("md-heading-2").expect("doc h2");
    crate::layout_probe_end();
    assert!((d1 - 28.0 * 1.3).abs() < 0.5, "doc h1 on the scale: {d1}");
    vcx.simulate_keystrokes("ctrl-shift-e");
    paint(&view, vcx);
    let wp1 = probe(&view, vcx, "wp-heading-0").expect("wp h1").3;
    let wp2 = probe(&view, vcx, "wp-heading-4").expect("wp h2").3;
    assert!((wp1 - d1).abs() < 0.5, "h1: WP {wp1} == Doc {d1}");
    assert!((wp2 - d2).abs() < 0.5, "h2: WP {wp2} == Doc {d2}");
}

// ---------------------------------------------------------------------------
// heading-nav (graph 4f1): `]]` / `[[` heading jumps (UXI-Buffer-13) and
// heading folds `za` / `zM` / `zR` (UXI-Buffer-14) in the Doc view.
// ---------------------------------------------------------------------------

/// `n` filler paragraphs.
fn filler(tag: &str, n: usize) -> String {
    (0..n).map(|i| format!("{tag} filler paragraph {i}.\n\n")).collect()
}

/// `(cursor_block, fold keys, hidden blocks)` of the focused Doc.
fn fold_state(
    view: &Entity<YaldaGpuiView>,
    vcx: &mut VisualTestContext,
) -> (usize, Vec<usize>, Vec<usize>) {
    view.read_with(vcx, |v, _| match v.workspace.focused_content() {
        Some(App::Buffer(BufferApp::Viewing(d))) => (
            d.cursor_block,
            d.folds.keys().copied().collect(),
            (0..d.blocks.len()).filter(|&i| d.is_block_hidden(i)).collect(),
        ),
        _ => panic!("expected a Doc"),
    })
}

/// UXI-Buffer-13: `]]` puts the cursor on the next heading (any level) and
/// scrolls it to the top of the view, painted there; `[[` from inside a
/// section goes back to that section's heading, and from a heading to the
/// previous one. Real keystrokes through the keymap.
///
/// Negative control (observed RED): unbind `] ]` in `DEFAULT_BINDINGS` → the
/// cursor stays on block 0; drop `scroll_block_to_top` from
/// `doc_heading_jump` → the heading is not the top block.
#[gpui::test]
fn heading_jumps_move_cursor_and_scroll_heading_to_top(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    // Blocks: 0 `# One`, 1..=12, 13 `## Two`, 14..=25, 26 `# Three`, 27..=38.
    let md = format!(
        "# One\n\n{}## Two\n\n{}# Three\n\n{}",
        filler("one", 12),
        filler("two", 12),
        filler("three", 12)
    );
    let (view, vcx, _file) = boot_doc(cx, "hnav", &md);
    vcx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(300.0)));
    paint(&view, vcx);

    vcx.simulate_keystrokes("] ]");
    paint(&view, vcx);
    let (cursor, top, vp) = doc_pos(&view, vcx);
    assert_eq!((cursor, top), (13, 13), "`]]` → `## Two`, scrolled to the top");
    let (_, y, _, _) = probe(&view, vcx, "doc-block-13").expect("heading painted");
    assert!((y - vp.0).abs() < 20.0, "heading painted at the viewport top: {y} vs {vp:?}");
    assert!(probe(&view, vcx, "doc-block-0").is_none(), "non-vacuous: block 0 scrolled away");

    vcx.simulate_keystrokes("] ]");
    paint(&view, vcx);
    assert_eq!(doc_pos(&view, vcx).0, 26, "`]]` → `# Three`");

    // Into the section, then `[[` returns to its heading…
    vcx.simulate_keystrokes("j j j");
    paint(&view, vcx);
    assert_eq!(doc_pos(&view, vcx).0, 29);
    vcx.simulate_keystrokes("[ [");
    paint(&view, vcx);
    assert_eq!(doc_pos(&view, vcx).0, 26, "`[[` inside a section → its heading");
    // …and from a heading to the previous one, at the top.
    vcx.simulate_keystrokes("[ [");
    paint(&view, vcx);
    let (cursor, top, _) = doc_pos(&view, vcx);
    assert_eq!((cursor, top), (13, 13), "`[[` → `## Two`, scrolled to the top");
    // Past the last heading: no move.
    vcx.simulate_keystrokes("] ] ] ] ] ]");
    paint(&view, vcx);
    assert_eq!(doc_pos(&view, vcx).0, 26, "`]]` stops at the last heading");
}

/// Blocks: 0 `# One`, 1, 2, 3 `## Two`, 4, 5 `# Three`, 6 — fits the viewport.
const FOLD_MD: &str =
    "# One\n\npara 1a\n\npara 1b\n\n## Two\n\npara 2a\n\n# Three\n\npara 3a\n";

/// UXI-Buffer-14: `za` inside a section folds it — its blocks (incl. the nested
/// `## Two`) are NOT painted, the heading paints a `… N hidden` marker, the
/// cursor lands on the heading, and `j` skips the folded blocks. `za` again
/// unfolds. The outline rail marks the folded heading.
///
/// Negative controls (observed RED): make `FoldLayout::is_hidden` always false
/// → the hidden blocks paint; revert `doc_next_block` to `cursor + 1` → `j`
/// lands on block 1.
#[gpui::test]
fn za_folds_the_section_out_of_paint_and_j_skips_it(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _file) = boot_doc(cx, "fold-za", FOLD_MD);
    paint(&view, vcx);
    for i in 0..7 {
        assert!(probe(&view, vcx, &format!("doc-block-{i}")).is_some(), "non-vacuous: block {i} painted before folding");
    }
    vcx.simulate_keystrokes("j j");
    vcx.simulate_keystrokes("z a");
    paint(&view, vcx);
    let (cursor, folds, hidden) = fold_state(&view, vcx);
    assert_eq!(cursor, 0, "folding from inside the section lands on its heading");
    assert_eq!(folds, vec![0]);
    assert_eq!(hidden, vec![1, 2, 3, 4]);
    for i in 1..=4 {
        assert!(probe(&view, vcx, &format!("doc-block-{i}")).is_none(), "block {i} folded away, not painted");
    }
    for i in [0, 5, 6] {
        assert!(probe(&view, vcx, &format!("doc-block-{i}")).is_some(), "block {i} still painted");
    }
    let (_, hy, _, hh) = probe(&view, vcx, "doc-block-0").unwrap();
    let (_, my, _, _) = probe(&view, vcx, "doc-fold-marker-0").expect("fold marker painted");
    assert!(my >= hy && my < hy + hh, "the marker trails the heading row");

    vcx.simulate_keystrokes("j");
    paint(&view, vcx);
    assert_eq!(fold_state(&view, vcx).0, 5, "`j` skips the folded section");
    vcx.simulate_keystrokes("k");
    paint(&view, vcx);
    assert_eq!(fold_state(&view, vcx).0, 0, "`k` skips it back");

    // The outline rail marks the folded heading.
    vcx.simulate_keystrokes("cmd-shift-o");
    crate::layout_probe_begin();
    frames(&view, vcx);
    let folded_row = crate::layout_probe_get("outline-folded-row");
    crate::layout_probe_end();
    assert!(folded_row.is_some(), "the outline shows the folded heading with its marker");
    vcx.simulate_keystrokes("escape");
    paint(&view, vcx);

    vcx.simulate_keystrokes("z a");
    paint(&view, vcx);
    let (_, folds, hidden) = fold_state(&view, vcx);
    assert!(folds.is_empty() && hidden.is_empty(), "`za` on a folded heading unfolds");
    assert!(probe(&view, vcx, "doc-block-2").is_some(), "unfolded block paints again");
}

/// UXI-Buffer-14: `zM` folds every section (nested ones inside their parent),
/// `zR` unfolds everything; `G` stops at the last painted block.
///
/// Negative control (observed RED): unbind `z shift-r` → the folds stay.
#[gpui::test]
fn zm_folds_all_and_zr_unfolds_all(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _file) = boot_doc(cx, "fold-zm", FOLD_MD);
    vcx.simulate_keystrokes("z shift-m");
    paint(&view, vcx);
    let (_, folds, hidden) = fold_state(&view, vcx);
    assert_eq!(folds, vec![0, 6, 10], "every heading with a section folds");
    assert_eq!(hidden, vec![1, 2, 3, 4, 6]);
    assert!(probe(&view, vcx, "doc-fold-marker-0").is_some());
    assert!(probe(&view, vcx, "doc-fold-marker-5").is_some());
    assert!(probe(&view, vcx, "doc-fold-marker-3").is_none(), "a nested fold is hidden with its parent");
    vcx.simulate_keystrokes("shift-g");
    paint(&view, vcx);
    assert_eq!(fold_state(&view, vcx).0, 5, "`G` lands on the last PAINTED block");

    vcx.simulate_keystrokes("z shift-r");
    paint(&view, vcx);
    let (_, folds, hidden) = fold_state(&view, vcx);
    assert!(folds.is_empty() && hidden.is_empty(), "`zR` unfolds everything");
    for i in 0..7 {
        assert!(probe(&view, vcx, &format!("doc-block-{i}")).is_some(), "block {i} painted after zR");
    }
}

/// UXI-Buffer-14: a fold follows its heading across a re-parse. A Doc and an
/// Edit tile of the SAME file: fold `# One` in the Doc, then type a newline
/// ABOVE it in the Edit tile (real keystrokes) — the heading moves from line 0
/// to line 1, the Doc re-derives, and the section stays folded out of paint. A
/// Doc → Edit → Doc round trip keeps the fold too.
///
/// Negative control (observed RED): make `rekey_folds` keep only folds whose
/// line still holds the heading (no identity re-key) → the fold is dropped.
#[gpui::test]
fn fold_survives_an_edit_that_moves_its_heading(cx: &mut TestAppContext) {
    let (view, vcx) = boot_doc_beside_edit(cx, "fold-edit", FOLD_MD, "", true);
    vcx.simulate_keystrokes("ctrl-w h");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("z a");
    paint(&view, vcx);
    assert_eq!(fold_state(&view, vcx).1, vec![0], "folded `# One` at line 0");

    vcx.simulate_keystrokes("ctrl-w l");
    vcx.run_until_parked();
    let caret = view.update(vcx, |v, _| v.edit_mut().expect("edit").editor.cursor());
    assert_eq!((caret.line, caret.col), (0, 0), "typing lands above the heading");
    vcx.simulate_keystrokes("enter");
    paint(&view, vcx);
    let text = view.update(vcx, |v, _| v.edit_mut().expect("edit").editor.full_text());
    assert!(text.starts_with("\n# One"), "non-vacuous: the heading moved down: {text:?}");

    vcx.simulate_keystrokes("ctrl-w h");
    paint(&view, vcx);
    let (_, folds, hidden) = fold_state(&view, vcx);
    assert_eq!(folds, vec![1], "the fold re-keyed to the heading's new line");
    assert_eq!(hidden, vec![1, 2, 3, 4]);
    assert!(probe(&view, vcx, "doc-block-2").is_none(), "still folded out of paint");
    assert!(probe(&view, vcx, "doc-fold-marker-0").is_some());

    // Doc → Edit → Doc keeps it.
    vcx.simulate_keystrokes("ctrl-e");
    paint(&view, vcx);
    vcx.simulate_keystrokes("ctrl-v");
    paint(&view, vcx);
    assert_eq!(fold_state(&view, vcx).1, vec![1], "the fold survives Doc → Edit → Doc");
    assert!(probe(&view, vcx, "doc-block-2").is_none());
}

/// yux rule 2 / UXP-3: a fold toggle is a `DocView` render input — it must
/// re-render the cached body (the cursor stays on the heading and the scroll
/// top is unchanged, so `fold_seq` is the only input that moved).
///
/// Negative control (observed RED): drop `fold_seq` from `DocSeqs::of`
/// (default it) → the body's render count stays flat and block 1 keeps painting.
#[gpui::test]
fn fold_toggle_rerenders_the_cached_doc_body(cx: &mut TestAppContext) {
    cx.update(crate::register_keymap);
    let (view, vcx, _file) = boot_doc(cx, "fold-perf", FOLD_MD);
    paint(&view, vcx);
    let before = crate::perf_render_count("doc-body");
    vcx.simulate_keystrokes("z a");
    vcx.run_until_parked();
    assert_eq!(fold_state(&view, vcx).0, 0, "cursor stayed on the heading");
    let after = crate::perf_render_count("doc-body");
    assert!(after > before, "the fold must re-render the Doc body ({before} → {after})");
    // What the body painted on its own (no forced re-render) excludes the
    // folded text — read from its paint-time hit-test sink.
    view.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    let painted = painted_doc_text(&view, vcx);
    assert!(painted.contains("One"), "non-vacuous: the heading painted: {painted:?}");
    assert!(!painted.contains("para 1a"), "folded text not painted: {painted:?}");
}

#[test]
fn outline_row_label_marks_folded_headings() {
    assert_eq!(crate::chrome::outline_row_label("Intro", true), "▸ Intro");
    assert_eq!(crate::chrome::outline_row_label("Intro", false), "Intro");
}
