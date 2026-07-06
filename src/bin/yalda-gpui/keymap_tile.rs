//! `KeymapTile` — the `App::Keymap` tile's data: a thin holder for its cached
//! body view (`KeymapView`, `keymap_view.rs`). The bindings themselves live on
//! the root (`YaldaGpuiView::keymap_registry`); this tile just presents them and
//! routes keys (see `keymap_ui.rs`). Mirrors `LinearTile`'s tile/view split.

use super::*;

pub(crate) struct KeymapTile {
    /// The cached body — lazily built on first render (and re-created on restore,
    /// which has no `cx`). Owned here, so it drops when the tile closes.
    pub(crate) view: Option<Entity<KeymapView>>,
}

impl KeymapTile {
    pub(crate) fn new() -> Self {
        KeymapTile { view: None }
    }

    /// Tab / window title — a fixed label; the sheet is not document-backed.
    pub(crate) fn title(&self) -> String {
        "Keybindings".to_string()
    }
}
