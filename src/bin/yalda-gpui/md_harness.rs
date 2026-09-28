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
