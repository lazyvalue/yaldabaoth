//! Markdown images (`RenderedBlock::Image`): source resolution, the
//! width-fitting image element, and the HTTP client gpui's image loader uses
//! for `http(s)` sources.
//!
//! `gpui::img` sizes a loaded image to its intrinsic pixel size and fixes BOTH
//! dimensions, so a `max_w_full` clamp squashes a wide picture instead of
//! scaling it. [`MdImage`] instead asks taffy for the available width (a
//! measured leaf) and returns `min(intrinsic, available)` with the height kept
//! in proportion. Loading goes through gpui's own asset cache
//! (`ImgResourceLoader`, the loader `img` uses): off-thread, cached per source,
//! and it re-notifies the rendering view when the bytes land — so this module
//! never notifies (never from render). While loading, or when the file is
//! missing / undecodable / the URL fails, the alt text is painted instead.

use super::*;
use gpui::http_client::{self, AsyncBody, HttpClient, Url};
use gpui::{AvailableSpace, Corners, ImgResourceLoader, RenderImage, Resource, SharedUri, Size};
use std::path::Path;
use std::sync::Arc;

/// Where an image destination loads from. `http(s)://` → a URI (fetched by
/// the app's HTTP client); `file://`, absolute, and relative paths → a file,
/// relative ones resolved against the document's directory (`doc_dir`).
/// `None` when it cannot be resolved (relative with no `doc_dir`, `data:`
/// URIs, empty) — the caller then shows the alt text. Pure: no filesystem
/// access (this runs on the render path).
pub(crate) fn image_resource(url: &str, doc_dir: Option<&Path>) -> Option<Resource> {
    let mut t = url.trim();
    if t.starts_with('<') && t.ends_with('>') && t.len() >= 2 {
        t = &t[1..t.len() - 1];
    }
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(Resource::Uri(SharedUri::from(t.to_string())));
    }
    if lower.contains("://") || lower.starts_with("data:") {
        // Only `file://` among the remaining schemes loads locally.
        let rest = t.strip_prefix("file://")?;
        t = rest.strip_prefix("localhost").unwrap_or(rest);
    }
    if let Some(end) = t.find(['#', '?']) {
        t = &t[..end];
    }
    let decoded = percent_decode_path(t);
    let path = Path::new(&decoded);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        doc_dir?.join(path)
    };
    Some(Resource::Path(Arc::from(full.as_path())))
}

/// Display size of an `(iw, ih)` image given at most `max_w` of width:
/// never wider than intrinsic, never wider than `max_w`, aspect preserved.
pub(crate) fn fit_image(iw: f32, ih: f32, max_w: Option<f32>) -> (f32, f32) {
    if iw <= 0.0 || ih <= 0.0 {
        return (0.0, 0.0);
    }
    let w = max_w.map_or(iw, |m| iw.min(m.max(0.0)));
    (w, ih * w / iw)
}

/// Intrinsic display size of a decoded image in logical pixels. SVGs are
/// rasterized by gpui at `SMOOTH_SVG_SCALE_FACTOR`, so their buffers are that
/// many times larger than their nominal size.
const SVG_RASTER_SCALE: f32 = 2.0; // gpui's (crate-private) SMOOTH_SVG_SCALE_FACTOR

fn intrinsic_size(data: &RenderImage, resource: &Resource) -> (f32, f32) {
    let s = data.size(0);
    let is_svg = match resource {
        Resource::Path(p) => p.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")),
        Resource::Uri(u) => u.as_ref().to_ascii_lowercase().ends_with(".svg"),
        Resource::Embedded(_) => false,
    };
    let div = if is_svg { SVG_RASTER_SCALE } else { 1.0 };
    (s.width.0 as f32 / div, s.height.0 as f32 / div)
}

/// The image element of a markdown `Image` block. See the module docs.
pub(crate) struct MdImage {
    resource: Resource,
    fallback: AnyElement,
    /// Layout-probe tag (tests only; `None` in production).
    probe: Option<String>,
}

impl MdImage {
    pub(crate) fn new(resource: Resource, fallback: AnyElement, probe: Option<String>) -> Self {
        Self {
            resource,
            fallback,
            probe,
        }
    }
}

impl IntoElement for MdImage {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for MdImage {
    /// The decoded image, or `None` while the fallback stands in.
    type RequestLayoutState = Option<Arc<RenderImage>>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut GpuiApp,
    ) -> (LayoutId, Self::RequestLayoutState) {
        match window.use_asset::<ImgResourceLoader>(&self.resource, cx) {
            Some(Ok(data)) => {
                let (iw, ih) = intrinsic_size(&data, &self.resource);
                let layout = window.request_measured_layout(
                    gpui::Style::default(),
                    move |known: Size<Option<Pixels>>, avail: Size<AvailableSpace>, _, _| {
                        let max_w = known.width.map(f32::from).or(match avail.width {
                            AvailableSpace::Definite(w) => Some(f32::from(w)),
                            // A replaced element with `max-width: 100%` adds
                            // nothing to its container's min-content width.
                            AvailableSpace::MinContent => Some(0.0),
                            AvailableSpace::MaxContent => None,
                        });
                        let (w, h) = fit_image(iw, ih, max_w);
                        gpui::size(px(w), px(h))
                    },
                );
                (layout, Some(data))
            }
            // Loading, or failed: the alt text stands in.
            _ => (self.fallback.request_layout(window, cx), None),
        }
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        image: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut GpuiApp,
    ) {
        if image.is_none() {
            self.fallback.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        image: &mut Self::RequestLayoutState,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut GpuiApp,
    ) {
        let Some(data) = image.take() else {
            self.fallback.paint(window, cx);
            return;
        };
        // The box may be stretched wider than the image (block layout); the
        // picture itself is fitted to it, left-aligned.
        let (iw, ih) = intrinsic_size(&data, &self.resource);
        let (w, h) = fit_image(iw, ih, Some(f32::from(bounds.size.width)));
        let rect = Bounds::new(bounds.origin, gpui::size(px(w), px(h)));
        if let Err(e) = window.paint_image(rect, Corners::default(), data, 0, false) {
            tracing::debug!("markdown image paint failed: {e}");
        }
        if let Some(tag) = &self.probe {
            // The laid-out box too: it must hug the picture (no blank band).
            layout_probe_record_dyn(
                &format!("{tag}-box"),
                (
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                ),
            );
            layout_probe_record_dyn(
                tag,
                (
                    f32::from(rect.origin.x),
                    f32::from(rect.origin.y),
                    f32::from(rect.size.width),
                    f32::from(rect.size.height),
                ),
            );
        }
    }
}

/// gpui's image loader fetches `http(s)` images through the app's
/// `HttpClient`; gpui's default is a null client that fails every request.
/// This one runs a blocking `ureq` GET on its own thread (never the UI
/// thread) and hands the body back. Images only: bodies are capped.
pub(crate) struct UreqImageClient;

/// Largest image body fetched (bytes).
const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

impl HttpClient for UreqImageClient {
    fn type_name(&self) -> &'static str {
        "UreqImageClient"
    }

    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: http_client::Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, http_client::Result<http_client::Response<AsyncBody>>>
    {
        let uri = req.uri().to_string();
        let method = req.method().clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<(u16, Vec<u8>), String> {
                if method != http_client::Method::GET {
                    return Err(format!("unsupported method {method}"));
                }
                let resp = match ureq::get(&uri).call() {
                    Ok(r) => r,
                    Err(ureq::Error::Status(_, r)) => r,
                    Err(e) => return Err(e.to_string()),
                };
                let status = resp.status();
                let mut body = Vec::new();
                use std::io::Read;
                resp.into_reader()
                    .take(MAX_IMAGE_BYTES)
                    .read_to_end(&mut body)
                    .map_err(|e| e.to_string())?;
                Ok((status, body))
            })();
            let _ = tx.send(result);
        });
        Box::pin(async move {
            let (status, body) = rx
                .await
                .map_err(|_| http_client::anyhow!("image fetch thread dropped"))?
                .map_err(|e| http_client::anyhow!("image fetch failed: {e}"))?;
            Ok(http_client::Response::builder()
                .status(status)
                .body(AsyncBody::from(body))?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_resource_resolves_relative_to_the_doc_dir() {
        let dir = Path::new("/notes/sub");
        let path = |r: Option<Resource>| match r {
            Some(Resource::Path(p)) => p.to_path_buf(),
            other => panic!("expected a path, got {other:?}"),
        };
        assert_eq!(
            path(image_resource("img/a b.png", Some(dir))),
            dir.join("img/a b.png")
        );
        assert_eq!(
            path(image_resource("img/a%20b.png#frag", Some(dir))),
            dir.join("img/a b.png")
        );
        assert_eq!(
            path(image_resource("/abs/x.png", None)),
            Path::new("/abs/x.png")
        );
        assert_eq!(
            path(image_resource("file:///abs/y.png", None)),
            Path::new("/abs/y.png")
        );
        assert!(matches!(
            image_resource("https://example.com/p.png", None),
            Some(Resource::Uri(_))
        ));
        assert!(
            image_resource("rel.png", None).is_none(),
            "relative needs a doc dir"
        );
        assert!(image_resource("data:image/png;base64,AAAA", Some(dir)).is_none());
        assert!(image_resource("  ", Some(dir)).is_none());
    }

    #[test]
    fn fit_image_caps_width_and_keeps_aspect() {
        assert_eq!(fit_image(400.0, 200.0, Some(1000.0)), (400.0, 200.0));
        assert_eq!(fit_image(400.0, 200.0, Some(100.0)), (100.0, 50.0));
        assert_eq!(fit_image(400.0, 200.0, None), (400.0, 200.0));
        assert_eq!(fit_image(0.0, 200.0, Some(100.0)), (0.0, 0.0));
    }
}
