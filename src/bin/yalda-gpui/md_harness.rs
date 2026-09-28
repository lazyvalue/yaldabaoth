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
