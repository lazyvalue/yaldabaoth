//! Release-mode perf gate (verification-harness gap #3, `docs/dev-system.md`).
//!
//! Render-COUNT is already a CI proxy, but it's a proxy and debug masks real
//! wins. This benchmarks the two lib hot paths a realistic transcript / document
//! actually runs — `render::render` (pulldown-cmark → styled blocks, incl.
//! syntect code-block highlighting) and `md_highlight::highlight_markdown_lines_syn`
//! (the edit-view per-line highlighter) — over a realistic-size markdown doc,
//! optimized. Run: `cargo bench --bench render_bench`.
//!
//! Using it as a GATE: criterion writes `target/criterion/<id>/` and reports a
//! `change: [-x% +y%] (p = …)` line vs the prior run; a regression run prints
//! `Performance has regressed`. CI can save a baseline (`--save-baseline main`)
//! and fail on `cargo bench -- --baseline main` regression. This file provides
//! the measurement; wiring the CI threshold is the remaining step.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::hint::black_box;
use yalda::highlight::Highlighter;
use yalda::theme::Theme;

/// Build a realistic markdown document of roughly `paras` paragraphs with a
/// fenced code block every 6th block — mirrors a long agent transcript / note
/// (prose + headings + code), the shape the render/highlight cost scales with.
fn synthetic_markdown(paras: usize) -> String {
    let mut s = String::with_capacity(paras * 120);
    for i in 0..paras {
        if i % 12 == 0 {
            s.push_str(&format!("## Section {}\n\n", i / 12));
        }
        if i % 6 == 0 {
            s.push_str("```rust\n");
            s.push_str("fn demo(x: usize) -> usize {\n    let y = x * 2;\n    y + 1\n}\n");
            s.push_str("```\n\n");
        } else {
            s.push_str(&format!(
                "Paragraph {i} with some **bold**, *italic*, and `inline code` plus a \
                 [link](https://example.com) to exercise inline parsing and styling.\n\n"
            ));
        }
    }
    s
}

fn bench_render(c: &mut Criterion) {
    let theme = Theme::default();
    let hl = Highlighter::new();
    let mut group = c.benchmark_group("render");
    for &paras in &[200usize, 1000] {
        let md = synthetic_markdown(paras);
        let lines: Vec<String> = md.lines().map(|l| l.to_string()).collect();
        group.bench_with_input(BenchmarkId::new("render_with_highlighter", paras), &md, |b, md| {
            b.iter(|| {
                let blocks = yalda::render::render_with_highlighter(black_box(md), &theme, &hl);
                black_box(blocks.len())
            })
        });
        group.bench_with_input(
            BenchmarkId::new("highlight_markdown_lines_syn", paras),
            &lines,
            |b, lines| {
                b.iter(|| {
                    let segs = yalda::md_highlight::highlight_markdown_lines_syn(
                        black_box(lines),
                        &theme,
                        &hl,
                    );
                    black_box(segs.len())
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
