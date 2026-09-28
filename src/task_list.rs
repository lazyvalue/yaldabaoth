//! GFM task-list checkboxes in markdown source (`UXI-Buffer-12`): where each
//! `[ ]` / `[x]` marker lives in the text, the rendered-tree address of each
//! task item, and which one a keyboard toggle targets.
//!
//! The markers come from the SAME parser the renderer uses
//! ([`crate::parse::parse_with_offsets`]), so "is this a task item" never
//! disagrees with what the Doc view paints: a `- [ ]` inside a fenced code
//! block, an indented code block or a table cell is not a marker here either.
//! Nested items, `*` / `-` / `+` / `1.` / `1)` bullets and `[X]` are all just
//! what the parser reports.

use std::ops::Range;

use pulldown_cmark::Event;

use crate::blocks::RenderedBlock;

/// One task checkbox in the source: `bytes` covers exactly the three bytes
/// `[ ]` / `[x]` / `[X]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMarker {
    pub bytes: Range<usize>,
    pub checked: bool,
}

impl TaskMarker {
    /// Byte offset of the state character between the brackets (the one byte
    /// a toggle rewrites).
    pub fn state_byte(&self) -> usize {
        self.bytes.start + 1
    }

    /// The state character a toggle writes: `x` for an open box, a space for
    /// a done one (`[X]` unchecks to `[ ]` too).
    pub fn toggled_char(&self) -> char {
        if self.checked { ' ' } else { 'x' }
    }
}

/// Every task marker in `markdown`, in document order.
pub fn task_markers(markdown: &str) -> Vec<TaskMarker> {
    let bytes = markdown.as_bytes();
    crate::parse::parse_with_offsets(markdown)
        .filter_map(|(ev, range)| match ev {
            Event::TaskListMarker(checked) => {
                // Normalize to the bracket triple (defensive: the parser's
                // range is the marker itself, but never trust a width).
                let start = (range.start..range.end.max(range.start + 1))
                    .find(|&i| bytes.get(i) == Some(&b'['))?;
                (bytes.get(start + 2) == Some(&b']')).then_some(TaskMarker {
                    bytes: start..start + 3,
                    checked,
                })
            }
            _ => None,
        })
        .collect()
}

/// The task markers inside `span` (a top-level block's source byte range).
pub fn task_markers_in(markdown: &str, span: &Range<usize>) -> Vec<TaskMarker> {
    task_markers(markdown)
        .into_iter()
        .filter(|m| m.bytes.start >= span.start && m.bytes.end <= span.end)
        .collect()
}

/// The structural path (relative to the top-level `block`) of every task item
/// in it, in document order — the same addressing the Doc view names its
/// checkboxes by (`md-task-<block>.<path…>`): a list item is `[item]`, a block
/// inside an item's content is `[item, content]`, a blockquote / footnote
/// child is `[child]`. Parallel to [`task_markers_in`] for that block's span.
pub fn task_item_paths(block: &RenderedBlock) -> Vec<Vec<usize>> {
    fn walk(block: &RenderedBlock, prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        match block {
            RenderedBlock::List { items, .. } => {
                for (i, item) in items.iter().enumerate() {
                    prefix.push(i);
                    if item.checked.is_some() {
                        out.push(prefix.clone());
                    }
                    for (bi, b) in item.content.iter().enumerate() {
                        prefix.push(bi);
                        walk(b, prefix, out);
                        prefix.pop();
                    }
                    prefix.pop();
                }
            }
            RenderedBlock::BlockQuote { blocks } | RenderedBlock::Footnote { blocks, .. } => {
                for (i, b) in blocks.iter().enumerate() {
                    prefix.push(i);
                    walk(b, prefix, out);
                    prefix.pop();
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(block, &mut Vec::new(), &mut out);
    out
}

/// Which of a block's task markers the keyboard toggle flips: the FIRST open
/// one (so repeated presses work down the list, checking items off in order);
/// when every task is done, the LAST one (so the gesture is reversible
/// without a mouse). `None` when the block has no task.
pub fn key_toggle_target(markers: &[TaskMarker]) -> Option<usize> {
    markers
        .iter()
        .position(|m| !m.checked)
        .or_else(|| markers.len().checked_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(md: &str, m: &TaskMarker) -> String {
        md[m.bytes.clone()].to_string()
    }

    #[test]
    fn finds_markers_with_exact_bracket_ranges() {
        let md = "- [ ] a\n- [x] b\n- plain\n* [X] c\n+ [ ] d\n1. [ ] e\n2) [x] f\n";
        let ms = task_markers(md);
        let got: Vec<(String, bool)> = ms.iter().map(|m| (at(md, m), m.checked)).collect();
        assert_eq!(
            got,
            vec![
                ("[ ]".into(), false),
                ("[x]".into(), true),
                ("[X]".into(), true),
                ("[ ]".into(), false),
                ("[ ]".into(), false),
                ("[x]".into(), true),
            ]
        );
    }

    #[test]
    fn nested_items_are_in_document_order() {
        let md = "- [ ] parent\n  - [x] child\n    - [ ] grandchild\n- [ ] sibling\n";
        let ms = task_markers(md);
        assert_eq!(ms.len(), 4);
        let lines: Vec<usize> = ms
            .iter()
            .map(|m| md[..m.bytes.start].matches('\n').count())
            .collect();
        assert_eq!(lines, vec![0, 1, 2, 3]);
        assert_eq!(
            ms.iter().map(|m| m.checked).collect::<Vec<_>>(),
            vec![false, true, false, false]
        );
    }

    #[test]
    fn code_and_non_items_are_not_markers() {
        let md = "```\n- [ ] in fence\n```\n\n    - [ ] indented code\n\nText [ ] here\n\n- [ ]\n";
        // A bare `- [ ]` with no text is not a task in GFM (pulldown agrees).
        let ms = task_markers(md);
        assert!(ms.is_empty(), "{ms:?}");
    }

    #[test]
    fn markers_in_span_and_blockquotes() {
        let md = "- [ ] one\n\n> - [x] quoted\n\ntext\n\n- [ ] two\n";
        let q = md.find("> ").unwrap();
        let q_end = md.find("\n\ntext").unwrap();
        let ms = task_markers_in(md, &(q..q_end));
        assert_eq!(ms.len(), 1);
        assert_eq!(at(md, &ms[0]), "[x]");
        assert!(ms[0].checked);
    }

    #[test]
    fn toggled_char_flips_state() {
        let open = TaskMarker {
            bytes: 2..5,
            checked: false,
        };
        let done = TaskMarker {
            bytes: 2..5,
            checked: true,
        };
        assert_eq!(open.toggled_char(), 'x');
        assert_eq!(done.toggled_char(), ' ');
        assert_eq!(open.state_byte(), 3);
    }

    #[test]
    fn key_target_is_first_open_else_last() {
        let m = |c| TaskMarker {
            bytes: 0..3,
            checked: c,
        };
        assert_eq!(key_toggle_target(&[]), None);
        assert_eq!(key_toggle_target(&[m(true), m(false), m(false)]), Some(1));
        assert_eq!(key_toggle_target(&[m(true), m(true)]), Some(1));
        assert_eq!(key_toggle_target(&[m(false)]), Some(0));
    }

    #[test]
    fn item_paths_parallel_the_markers() {
        let theme = crate::theme::Theme::default();
        let md = "* [ ] parent\n  * [x] child\n* plain\n  * [ ] under plain\n";
        let r = crate::render::render_mapped(md, &theme);
        assert_eq!(r.blocks.len(), 1);
        let paths = task_item_paths(&r.blocks[0]);
        assert_eq!(paths, vec![vec![0], vec![0, 1, 0], vec![1, 1, 0]]);
        assert_eq!(paths.len(), task_markers_in(md, &r.spans[0].bytes).len());

        let md = "> - [ ] a\n> - [x] b\n";
        let r = crate::render::render_mapped(md, &theme);
        assert_eq!(task_item_paths(&r.blocks[0]), vec![vec![0, 0], vec![0, 1]]);
    }
}
