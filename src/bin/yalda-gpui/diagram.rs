//! Mermaid diagram rendering + cache (`UXI-Diagram-1`).
//!
//! A ` ```mermaid ` fence parses to [`RenderedBlock::Diagram`](yalda::blocks::
//! RenderedBlock::Diagram); this module turns its source into a PNG **off the
//! paint thread** and caches the result so the shared `block_inner` paint
//! dispatch (both the agent transcript and the buffer Viewing surface) can paint
//! it inline. Rendering is **in-process** via `merman` (native Rust: parse →
//! layout → SVG → resvg raster) — no `mmdc`, no Node/Chromium, no `PATH`
//! dependency. Until the PNG is ready — or if `merman` can't render that diagram
//! type — the block paints its raw highlighted source (the paint arm handles
//! that; see `render_blocks.rs`).
//!
//! Rules honored here (see `yux/CLAUDE.md` + root `CLAUDE.md`):
//! - the CPU-bound `merman` render happens on `background_executor()`, never the
//!   paint thread;
//! - the cache is written and both surfaces invalidated from the spawn
//!   completion callback (an event context), never from a render path.

use super::*;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Which mermaid built-in theme to render with — derived from the app theme so
/// the diagram's colors match the surrounding UI. Part of the cache key, so a
/// theme switch re-renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MermaidTheme {
    Default,
    Dark,
}

impl MermaidTheme {
    pub(crate) fn from_theme_name(name: yalda::theme::ThemeName) -> Self {
        // `syntect_theme()` already classifies every app theme as `.light`/`.dark`;
        // reuse it so a new theme maps correctly without a second table.
        if name.syntect_theme().ends_with("dark") {
            Self::Dark
        } else {
            Self::Default
        }
    }

    /// The mermaid built-in theme name (merman `MermaidConfig` `theme` value).
    fn flag(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Dark => "dark",
        }
    }
}

/// The render state of one diagram, keyed in [`DiagramCache`].
pub(crate) enum DiagramRender {
    /// A background render is in flight; paint the raw source placeholder.
    Pending,
    /// A decoded PNG ready to paint via `img()`.
    Ready(Arc<gpui::Image>),
    /// merman could not render this diagram (unsupported type / parse error);
    /// paint raw source + this note.
    Failed(String),
}

/// Per-view cache of rendered mermaid diagrams, keyed by `hash(source + theme)`.
/// Owned by the root view behind an
/// `Rc<RefCell<…>>` so the paint path (via `RenderCtx`) can read it and the
/// spawn completion can write it.
#[derive(Default)]
pub(crate) struct DiagramCache {
    entries: HashMap<u64, DiagramRender>,
}

impl DiagramCache {
    pub(crate) fn get(&self, key: u64) -> Option<&DiagramRender> {
        self.entries.get(&key)
    }

    pub(crate) fn contains(&self, key: u64) -> bool {
        self.entries.contains_key(&key)
    }

    fn set(&mut self, key: u64, state: DiagramRender) {
        self.entries.insert(key, state);
    }

    /// Test-only: the render state under `key` as a stable tag, so guards can
    /// assert the async pipeline reached `pending`/`ready`/`failed` without
    /// matching the `Arc<Image>` payload.
    #[cfg(test)]
    pub(crate) fn state_of(&self, key: u64) -> Option<&'static str> {
        self.entries.get(&key).map(|s| match s {
            DiagramRender::Pending => "pending",
            DiagramRender::Ready(_) => "ready",
            DiagramRender::Failed(_) => "failed",
        })
    }
}

/// The cache key for a diagram: its source and theme. Exposed so the paint path
/// looks up the same key the request stored under. Width is deliberately NOT in
/// the key: the paint path builds its element tree before layout, so it cannot
/// know the container width to match a width-bucketed key. The rendered PNG is
/// intrinsic-size and fit to the container at paint time instead.
pub(crate) fn diagram_key(source: &str, theme: MermaidTheme) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut h);
    theme.hash(&mut h);
    h.finish()
}

// ---- renderer + test seam ------------------------------------------------

#[cfg(test)]
pub(crate) type TestRenderFn = fn(&str, MermaidTheme) -> Result<Vec<u8>, String>;

#[cfg(test)]
static TEST_RENDERER: std::sync::Mutex<Option<TestRenderFn>> = std::sync::Mutex::new(None);

/// Inject a stub renderer for headless tests (returns fixed PNG bytes or a
/// forced error) so the render path is exercised WITHOUT `mmdc` installed. Pass
/// `None` to clear.
#[cfg(test)]
pub(crate) fn set_test_renderer(f: Option<TestRenderFn>) {
    *TEST_RENDERER.lock().unwrap() = f;
}

/// Render mermaid `source` to PNG bytes. CPU-bound — MUST run on a background
/// executor, never the paint thread. Renders IN-PROCESS via `merman` (native
/// Rust: parse → layout → SVG → resvg raster), so there is no `mmdc` subprocess,
/// no Node/Chromium, and no `PATH` dependency. `merman` covers a subset of
/// mermaid's diagram types (flowchart, sequence, class, ER, XY); anything it
/// can't render returns `Ok(None)` here, which the caller surfaces as the
/// raw-source fallback.
fn render_mermaid_png(source: &str, theme: MermaidTheme) -> Result<Vec<u8>, String> {
    #[cfg(test)]
    if let Some(f) = *TEST_RENDERER.lock().unwrap() {
        return f(source, theme);
    }
    render_mermaid_png_real(source, theme)
}

/// The real (never-stubbed) merman render. Split out so tests can exercise the
/// actual engine without racing the process-global `TEST_RENDERER` seam.
fn render_mermaid_png_real(source: &str, theme: MermaidTheme) -> Result<Vec<u8>, String> {
    // The app theme drives mermaid's built-in `default`/`dark` palette so the
    // diagram matches the surrounding UI.
    let mut config = merman::MermaidConfig::empty_object();
    config.set_value("theme", serde_json::Value::from(theme.flag()));

    let renderer = merman::render::HeadlessRenderer::new().with_site_config(config);
    // scale 2.0 keeps the raster crisp on HiDPI; transparent background (None)
    // matches the block's own background.
    let raster = merman::render::raster::RasterOptions {
        scale: 2.0,
        background: None,
        jpeg_quality: 90,
    };

    match renderer.render_png_sync(source, &raster) {
        Ok(Some(bytes)) => Ok(bytes),
        // merman parsed nothing renderable (empty or an unsupported diagram type).
        Ok(None) => Err("unsupported or empty mermaid diagram".to_string()),
        Err(e) => Err(format!("mermaid render failed: {e}")),
    }
}

impl YaldaGpuiView {
    /// Non-render reconcile: find every mermaid `Diagram` block currently shown
    /// on either markdown surface — buffer `Viewing` docs and agent transcripts
    /// — and ensure a render is in flight for each. Idempotent: `request_diagram`
    /// dedups by cache key, so calling this every frame spawns a given diagram's
    /// render exactly once. Called from the top of `render()` (a mutation-only
    /// pass; it spawns off-thread and never notifies synchronously), NOT from a
    /// cached child's build path.
    pub(crate) fn reconcile_diagrams(&mut self, cx: &mut Context<Self>) {
        let mtheme = MermaidTheme::from_theme_name(self.theme.name);
        // Unique by cache key so the same diagram on two surfaces is one request.
        let mut wanted: HashMap<u64, String> = HashMap::new();

        // (a) Buffer Viewing docs: parsed blocks live directly on the root.
        for wsp in self.workspace.workspaces.iter() {
            wsp.layout.for_each_leaf(&mut |w| {
                if let App::Buffer(BufferApp::Viewing(d)) = &w.content {
                    for b in d.blocks.iter() {
                        if let RenderedBlock::Diagram { source, .. } = b {
                            wanted
                                .entry(diagram_key(source, mtheme))
                                .or_insert_with(|| source.clone());
                        }
                    }
                }
            });
        }

        // (b) Agent transcripts: parsed blocks are cached on each session's
        // view model (populated by `TranscriptView`; a one-frame lag on the
        // very first render is harmless — the completion repaints anyway).
        let ids: Vec<SessionId> = self.sessions.iter().map(|(id, _)| id).collect();
        for id in ids {
            self.read_session(id, cx, |st| {
                for item in st.view_model.flat_items_cache.iter() {
                    if let FlatItem::Block(rc) = item
                        && let RenderedBlock::Diagram { source, .. } = rc.as_ref()
                    {
                        wanted
                            .entry(diagram_key(source, mtheme))
                            .or_insert_with(|| source.clone());
                    }
                }
            });
        }

        let theme_name = self.theme.name;
        for source in wanted.into_values() {
            self.request_diagram(&source, theme_name, cx);
        }
    }

    /// Ensure a render exists for this diagram (source × theme). Called from the
    /// non-render reconcile pass — NEVER from a render path. If the key is already
    /// cached (pending/ready/failed) this is a no-op; otherwise it marks the entry
    /// `Pending` and spawns the blocking `mmdc` render on the background executor,
    /// writing the result and invalidating BOTH the doc view and every transcript
    /// view from the completion callback.
    pub(crate) fn request_diagram(&mut self, source: &str, theme: ThemeName, cx: &mut Context<Self>) {
        let mtheme = MermaidTheme::from_theme_name(theme);
        let key = diagram_key(source, mtheme);
        {
            let mut cache = self.diagrams.borrow_mut();
            if cache.contains(key) {
                return;
            }
            cache.set(key, DiagramRender::Pending);
        }
        let source = source.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { render_mermaid_png(&source, mtheme) })
                .await;
            let _ = this.update(cx, |this, cx| {
                {
                    let mut cache = this.diagrams.borrow_mut();
                    match result {
                        Ok(bytes) => cache.set(
                            key,
                            DiagramRender::Ready(Arc::new(gpui::Image::from_bytes(
                                gpui::ImageFormat::Png,
                                bytes,
                            ))),
                        ),
                        Err(err) => cache.set(key, DiagramRender::Failed(err)),
                    }
                }
                // Both markdown surfaces may show this diagram: the doc view
                // (root re-render) and any cached transcript view (pushed).
                cx.notify();
                this.notify_transcript_views(MissReason::Refresh, cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod merman_tests {
    use super::{render_mermaid_png_real, MermaidTheme};

    /// The REAL in-process merman engine renders a basic flowchart to a PNG,
    /// headlessly, with no subprocess/browser. This is what used to be the
    /// "live mmdc subprocess" runtime gap — now a plain unit test.
    ///
    /// Calls `render_mermaid_png_real` directly (not the `TEST_RENDERER`-gated
    /// wrapper) so a stub another test left installed can't intercept it.
    #[test]
    fn real_merman_renders_flowchart_to_png() {
        let bytes = render_mermaid_png_real(
            "flowchart TD\n  A[Start] --> B{Choice}\n  B -->|yes| C[Done]\n  B -->|no| A",
            MermaidTheme::Dark,
        )
        .expect("merman must render a basic flowchart");
        assert!(
            bytes.len() > 200,
            "expected a non-trivial PNG, got {} bytes",
            bytes.len()
        );
        assert_eq!(
            &bytes[..4],
            &[0x89, 0x50, 0x4E, 0x47],
            "output bytes must carry the PNG magic number"
        );
    }

    /// Unrenderable source resolves to `Err` (the raw-source fallback trigger),
    /// never a panic.
    #[test]
    fn unrenderable_source_errors_without_panic() {
        let r = render_mermaid_png_real("this is not a mermaid diagram", MermaidTheme::Default);
        assert!(
            r.is_err(),
            "unrenderable source must Err (drives fallback), got {:?}",
            r.map(|b| b.len())
        );
    }
}
