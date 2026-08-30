//! The **keybinding registry** — the single source of truth for every GPUI key
//! binding in the app. Historically the bindings were ~120 inline
//! `KeyBinding::new(...)` calls buried in `register_keymap`; that made them
//! impossible to introspect (GPUI consumes a `KeyBinding` on registration) and
//! impossible to rebind at runtime. This module lifts them into a declarative
//! table (`DEFAULT_BINDINGS`) that:
//!
//! 1. **drives the real keymap** — `register_keymap` now just builds a
//!    `KeymapRegistry` and calls [`KeymapRegistry::apply`], which clears the
//!    keymap and re-binds every entry via `App::build_action` +
//!    `KeyBinding::load`. So the table IS the live keymap, not a description of
//!    it — the reference tile reads the same data the app dispatches from.
//! 2. **is rebindable live** — [`KeymapRegistry::rebind`] mutates an entry's
//!    keystrokes; re-`apply`-ing swaps the whole keymap atomically (GPUI's
//!    `clear_key_bindings` + `bind_keys`). Overrides persist to
//!    `~/.yalda/keymap-overrides.json` and reload on the next launch.
//!
//! The `App::Keymap` tile (`keymap_view.rs`) renders this registry grouped by
//! context + theme and lets the user rebind or reset any row.

use super::*;
use serde::{Deserialize, Serialize};

/// A default binding, exactly as it ships. Ported 1:1 from the old inline
/// `register_keymap` — **order is load-bearing**: GPUI resolves same-keystroke
/// collisions by context specificity then registration order, so the reference
/// list preserves the original sequence (e.g. the global `cmd-0` ZoomReset must
/// precede the `AgentView` `cmd-0` FocusAgentPanel, as the old code documented).
struct DefaultBinding {
    /// Keystroke string in GPUI notation, e.g. `"ctrl-w s"`, `"cmd-shift-r"`.
    keys: &'static str,
    /// Action name WITHOUT the `yalda::` namespace (added when building).
    action: &'static str,
    /// GPUI key context this binding is scoped to (`None` = global).
    ctx: Option<&'static str>,
    /// Theme grouping within a context section (the "by theme" axis).
    cat: &'static str,
    /// Human description shown in the reference.
    desc: &'static str,
}

macro_rules! b {
    ($keys:literal, $action:literal, $ctx:expr, $cat:literal, $desc:literal) => {
        DefaultBinding {
            keys: $keys,
            action: $action,
            ctx: $ctx,
            cat: $cat,
            desc: $desc,
        }
    };
}

const YV: Option<&str> = Some("YaldaView");
const AV: Option<&str> = Some("AgentView");
const BV: Option<&str> = Some("BrowserView");
const RV: Option<&str> = Some("RailView");
const GLOBAL: Option<&str> = None;

/// The full default keymap. Sections mirror `register_keymap`'s three
/// `bind_keys` blocks, in order.
#[rustfmt::skip]
const DEFAULT_BINDINGS: &[DefaultBinding] = &[
    // ── Document view (YaldaView) ────────────────────────────────────────────
    b!("j",            "ScrollDown",       YV, "Navigation", "Scroll down"),
    b!("down",         "ScrollDown",       YV, "Navigation", "Scroll down"),
    b!("ctrl-n",       "ScrollDown",       YV, "Navigation", "Scroll down"),
    b!("k",            "ScrollUp",         YV, "Navigation", "Scroll up"),
    b!("up",           "ScrollUp",         YV, "Navigation", "Scroll up"),
    b!("ctrl-p",       "ScrollUp",         YV, "Navigation", "Scroll up"),
    b!("ctrl-d",       "ScrollPageDown",   YV, "Navigation", "Scroll half-page down"),
    b!("pagedown",     "ScrollPageDown",   YV, "Navigation", "Scroll page down"),
    b!("ctrl-u",       "ScrollPageUp",     YV, "Navigation", "Scroll half-page up"),
    b!("pageup",       "ScrollPageUp",     YV, "Navigation", "Scroll page up"),
    b!("l",            "CursorNextBlock",  YV, "Navigation", "Next block"),
    b!("right",        "CursorNextBlock",  YV, "Navigation", "Next block"),
    b!("h",            "CursorPrevBlock",  YV, "Navigation", "Previous block"),
    b!("left",         "CursorPrevBlock",  YV, "Navigation", "Previous block"),
    b!("g",            "CursorTop",        YV, "Navigation", "Go to top"),
    b!("shift-g",      "CursorBottom",     YV, "Navigation", "Go to bottom"),
    b!("ctrl-o",       "OpenBrowser",      YV, "Apps & files", "Open file browser"),
    b!("ctrl-e",       "EnterEdit",        YV, "Editing", "Edit — raw markdown"),
    b!("ctrl-shift-e", "EnterWp",          YV, "Editing", "Edit — word processor"),
    b!("ctrl-k",       "OpenAgent",        YV, "Apps & files", "Open agent"),
    b!("ctrl-l",       "OpenLinear",       YV, "Apps & files", "Open Linear"),
    b!("ctrl-g",       "OpenCog",          YV, "Apps & files", "Open Cog explorer"),
    b!("ctrl-shift-d", "OpenDiff",         YV, "Apps & files", "Open Diff"),
    b!("tab",          "NextBuffer",       YV, "Buffers", "Next buffer"),
    b!("shift-tab",    "PrevBuffer",       YV, "Buffers", "Previous buffer"),

    // ── Global (all screens) ─────────────────────────────────────────────────
    b!("cmd-q",             "Quit",            GLOBAL, "Application", "Quit"),
    b!("cmd-shift-ctrl-r",  "Restart",         GLOBAL, "Application", "Rebuild & restart"),
    b!("cmd-o",             "OpenBrowser",     GLOBAL, "Apps & files", "Open file browser"),
    b!("cmd-k",             "OpenAgent",       GLOBAL, "Apps & files", "Open agent"),
    b!("cmd-l",             "OpenLinear",      GLOBAL, "Apps & files", "Open Linear"),
    b!("cmd-g",             "OpenCog",         GLOBAL, "Apps & files", "Open Cog explorer"),
    b!("cmd-d",             "OpenDiff",        GLOBAL, "Apps & files", "Open Diff"),
    b!("cmd-/",             "OpenKeymap",      GLOBAL, "Apps & files", "Open keybindings reference"),
    b!("cmd-1",             "ToggleTasklist",  AV,     "Agent", "Toggle tasklist sidebar"),
    b!("cmd-2",             "ToggleSubagents", AV,     "Agent", "Toggle subagents sidebar"),
    b!("ctrl-alt-enter",    "ToggleAgentInputMode", AV, "Agent", "Toggle worksheet ⇄ message box"),
    b!("cmd-.",             "StopAgent",       AV,     "Agent", "Stop the in-flight turn"),
    b!("ctrl-tab",          "NextWorkspace",         GLOBAL, "Workspaces", "Next workspace"),
    b!("ctrl-shift-tab",    "PrevWorkspace",         GLOBAL, "Workspaces", "Previous workspace"),
    b!("cmd-shift-]",       "NextWorkspace",         GLOBAL, "Workspaces", "Next workspace"),
    b!("cmd-shift-[",       "PrevWorkspace",         GLOBAL, "Workspaces", "Previous workspace"),
    b!("cmd-shift-right",   "NextWorkspace",         GLOBAL, "Workspaces", "Next workspace"),
    b!("cmd-shift-left",    "PrevWorkspace",         GLOBAL, "Workspaces", "Previous workspace"),
    b!("ctrl-1",            "GotoWorkspace1",  GLOBAL, "Workspaces", "Go to workspace 1"),
    b!("ctrl-2",            "GotoWorkspace2",  GLOBAL, "Workspaces", "Go to workspace 2"),
    b!("ctrl-3",            "GotoWorkspace3",  GLOBAL, "Workspaces", "Go to workspace 3"),
    b!("ctrl-4",            "GotoWorkspace4",  GLOBAL, "Workspaces", "Go to workspace 4"),
    b!("ctrl-5",            "GotoWorkspace5",  GLOBAL, "Workspaces", "Go to workspace 5"),
    b!("ctrl-6",            "GotoWorkspace6",  GLOBAL, "Workspaces", "Go to workspace 6"),
    b!("ctrl-7",            "GotoWorkspace7",  GLOBAL, "Workspaces", "Go to workspace 7"),
    b!("ctrl-8",            "GotoWorkspace8",  GLOBAL, "Workspaces", "Go to workspace 8"),
    b!("ctrl-9",            "GotoWorkspace9",  GLOBAL, "Workspaces", "Go to workspace 9"),
    b!("ctrl-0",            "GotoWorkspace10", GLOBAL, "Workspaces", "Go to workspace 10"),
    b!("cmd-t",             "NewWorkspace",          GLOBAL, "Workspaces", "New workspace"),
    b!("cmd-shift-w",       "CloseWorkspace",        GLOBAL, "Workspaces", "Close workspace"),
    b!("cmd-shift-t",       "ToggleTheme",     GLOBAL, "Theme", "Toggle light / dark theme"),
    b!("ctrl-w s",          "SplitH",          GLOBAL, "Splits & focus", "Split horizontally"),
    b!("ctrl-w v",          "SplitV",          GLOBAL, "Splits & focus", "Split vertically"),
    b!("ctrl-w c",          "CloseWindow",     GLOBAL, "Splits & focus", "Close tile"),
    b!("cmd-w",             "CloseWindow",     GLOBAL, "Splits & focus", "Close tile"),
    b!("ctrl-w o",          "OnlyWindow",      GLOBAL, "Splits & focus", "Close other tiles"),
    b!("ctrl-w m",          "MoveTile",        GLOBAL, "Workspaces", "Send tile to workspace"),
    b!("ctrl-w shift-m",    "MoveTileAndFollow", GLOBAL, "Workspaces", "Send tile and follow"),
    b!("ctrl-w h",          "FocusLeft",       GLOBAL, "Splits & focus", "Focus tile left"),
    b!("ctrl-w l",          "FocusRight",      GLOBAL, "Splits & focus", "Focus tile right"),
    b!("ctrl-w k",          "FocusUp",         GLOBAL, "Splits & focus", "Focus tile up"),
    b!("ctrl-w j",          "FocusDown",       GLOBAL, "Splits & focus", "Focus tile down"),
    b!("ctrl-w w",          "FocusNext",       GLOBAL, "Splits & focus", "Focus next tile"),
    b!("ctrl-w shift-w",    "FocusPrev",       GLOBAL, "Splits & focus", "Focus previous tile"),
    b!("ctrl-w shift-h",    "SwapTileLeft",    GLOBAL, "Tile placement", "Swap tile left"),
    b!("ctrl-w shift-j",    "SwapTileDown",    GLOBAL, "Tile placement", "Swap tile down"),
    b!("ctrl-w shift-k",    "SwapTileUp",      GLOBAL, "Tile placement", "Swap tile up"),
    b!("ctrl-w shift-l",    "SwapTileRight",   GLOBAL, "Tile placement", "Swap tile right"),
    b!("ctrl-w enter",      "PromoteTile",      GLOBAL, "Tile placement", "Promote tile to first position"),
    b!("ctrl-w x",          "SwapTilePicker",  GLOBAL, "Tile placement", "Swap tile with…"),
    b!("ctrl-w r",          "RotateTilesForward",  GLOBAL, "Tile placement", "Rotate tiles forward"),
    b!("ctrl-w shift-r",    "RotateTilesBackward", GLOBAL, "Tile placement", "Rotate tiles backward"),
    b!("ctrl-w u",          "UndoArrangement", GLOBAL, "Tile placement", "Undo tile arrangement"),
    // Plane camera (spec-infinite-plane-workspace.md). `Ctrl-W`+plain-key
    // SEQUENCES (reliable on macOS; a bare `Ctrl`+digit is not). `Ctrl-W -/=` are
    // reclaimed from the retired ResizeShrink/Equalize; `Ctrl-W 0` is new.
    b!("ctrl-w -",          "ZoomOutWorkspace",   GLOBAL, "Plane", "Zoom the plane out"),
    b!("ctrl-w =",          "ZoomInWorkspace",    GLOBAL, "Plane", "Zoom the plane in"),
    b!("ctrl-w 0",          "ResetWorkspaceView", GLOBAL, "Plane", "Reset plane view to origin"),
    b!("ctrl-w a",          "ToggleWorkspaceColumns", GLOBAL, "Layout", "Cycle layout: columns / tiling / monocle"),
    b!("ctrl-w p",          "DesktopTileSize", GLOBAL, "Plane", "Set desktop tile size"),
    b!("ctrl-w t",          "TagViewChord",    GLOBAL, "Layout", "Tag: view by tag"),
    b!("ctrl-w ctrl-t",     "TagToggleChord",  GLOBAL, "Layout", "Tag: toggle tag on tile"),
    b!("ctrl-w shift-t",    "ClearTagView",    GLOBAL, "Layout", "Tag: clear tag view"),
    b!("ctrl-w b",          "AttachFocusedTile", GLOBAL, "Workspaces", "Attach tile to active workspace"),
    b!("ctrl-w shift-b",    "DetachFocusedTile", GLOBAL, "Workspaces", "Detach tile from workspace"),
    b!("ctrl-w d",          "HideFocusedTile", GLOBAL, "Workspaces", "Hide tile in its workspace"),
    b!("ctrl-w shift-d",    "UnhideFocusedTile", GLOBAL, "Workspaces", "Unhide focused hidden tile"),
    b!("ctrl-w backspace",  "WorkspaceBackAndForth", GLOBAL, "Workspaces", "Toggle previous workspace"),
    b!("ctrl-w f",          "GrowPrimaryArea", GLOBAL, "Columns", "Grow primary area"),
    b!("ctrl-w shift-f",    "ShrinkPrimaryArea", GLOBAL, "Columns", "Shrink primary area"),
    b!("ctrl-w n",          "IncreasePrimaryCount", GLOBAL, "Columns", "Increase primary tile count"),
    b!("ctrl-w shift-n",    "DecreasePrimaryCount", GLOBAL, "Columns", "Decrease primary tile count"),
    b!("cmd-=",             "ZoomIn",          GLOBAL, "Zoom", "Zoom in"),
    b!("cmd-+",             "ZoomIn",          GLOBAL, "Zoom", "Zoom in"),
    b!("cmd--",             "ZoomOut",         GLOBAL, "Zoom", "Zoom out"),
    b!("cmd-0",             "ZoomReset",       GLOBAL, "Zoom", "Reset zoom"),
    b!("cmd-0",             "FocusAgentPanel", AV,     "Agent", "Focus + widen the right sidepanel"),
    b!("cmd-c",             "CopyDocSelection", YV,    "Clipboard", "Copy doc-view selection"),
    b!("cmd-c",             "CopySelection",   GLOBAL, "Clipboard", "Copy selection"),
    b!("cmd-v",             "PasteFromClipboard", GLOBAL, "Clipboard", "Paste"),
    b!("cmd-shift-r",       "RenameWorkspace",       GLOBAL, "Workspaces", "Rename workspace"),
    b!("cmd-b",             "ToggleFileBrowserRail", GLOBAL, "Rails", "Toggle file-browser rail"),
    // AgentView-scoped cmd-b shadows the global rail toggle (same precedent as
    // cmd-0 FocusAgentPanel shadowing the global ZoomReset): hide the sidepanel.
    b!("cmd-b",             "ToggleAgentSidepanel", AV,     "Agent", "Hide / show the right sidepanel"),
    b!("cmd-shift-o",       "ToggleOutlineRail", GLOBAL, "Rails", "Toggle outline rail"),
    b!("cmd-shift-b",       "FlipRailSide",    GLOBAL, "Rails", "Flip rail to other side"),
    b!("cmd-j",             "ToggleJumpPanel", GLOBAL, "Workspaces", "Toggle jump panel"),
    b!("cmd-p",             "OpenJumpPalette", GLOBAL, "Workspaces", "Jump palette (fuzzy)"),

    // ── File browser (BrowserView) ───────────────────────────────────────────
    b!("j",       "BrowserDown",      BV, "File browser", "Move down"),
    b!("down",    "BrowserDown",      BV, "File browser", "Move down"),
    b!("ctrl-n",  "BrowserDown",      BV, "File browser", "Move down"),
    b!("k",       "BrowserUp",        BV, "File browser", "Move up"),
    b!("up",      "BrowserUp",        BV, "File browser", "Move up"),
    b!("ctrl-p",  "BrowserUp",        BV, "File browser", "Move up"),
    b!("enter",   "BrowserEnter",     BV, "File browser", "Open / enter"),
    b!("l",       "BrowserEnter",     BV, "File browser", "Open / enter"),
    b!("right",   "BrowserEnter",     BV, "File browser", "Open / enter"),
    b!("h",       "BrowserParent",    BV, "File browser", "Go to parent dir"),
    b!("left",    "BrowserParent",    BV, "File browser", "Go to parent dir"),
    b!("-",       "BrowserParent",    BV, "File browser", "Go to parent dir"),
    b!("s",       "BrowserCycleSort", BV, "File browser", "Cycle sort order"),
    b!("q",       "BrowserClose",     BV, "File browser", "Close browser"),
    b!("escape",  "BrowserClose",     BV, "File browser", "Close browser"),
    b!("w",       "BrowserWorktrees", BV, "File browser", "Show worktrees"),
    b!("/",       "BrowserFilter",    BV, "File browser", "Filter"),
    b!("r",       "BrowserRename",    BV, "File browser", "Rename"),

    // ── Rail / side column (RailView) ────────────────────────────────────────
    b!("j",       "RailDown",         RV, "Rail", "Move down"),
    b!("down",    "RailDown",         RV, "Rail", "Move down"),
    b!("ctrl-n",  "RailDown",         RV, "Rail", "Move down"),
    b!("k",       "RailUp",           RV, "Rail", "Move up"),
    b!("up",      "RailUp",           RV, "Rail", "Move up"),
    b!("ctrl-p",  "RailUp",           RV, "Rail", "Move up"),
    b!("enter",   "RailSelect",       RV, "Rail", "Select"),
    b!("escape",  "RailClose",        RV, "Rail", "Close rail"),
    b!("-",       "RailParent",       RV, "Rail", "Go to parent dir"),
    b!(".",       "RailToggleHidden", RV, "Rail", "Toggle hidden files"),
    b!("s",       "RailCycleSort",    RV, "Rail", "Cycle sort order"),
    b!("w",       "RailWorktrees",    RV, "Rail", "Show worktrees"),
    b!("/",       "RailFilter",       RV, "Rail", "Filter"),
];

/// A friendly section heading for a GPUI key context (the "by context" axis).
pub(crate) fn context_label(ctx: Option<&str>) -> &'static str {
    match ctx {
        None => "Global — every screen",
        Some("YaldaView") => "Document view",
        Some("AgentView") => "Agent",
        Some("BrowserView") => "File browser",
        Some("RailView") => "Rail (side column)",
        Some(_) => "Other",
    }
}

/// The display order of context sections in the reference.
pub(crate) const CONTEXT_ORDER: &[Option<&str>] = &[
    None,
    Some("YaldaView"),
    Some("AgentView"),
    Some("BrowserView"),
    Some("RailView"),
];

/// One live binding: its current keystrokes plus the immutable defaults it was
/// built from (so we can show "changed", reset it, and key persistence stably).
#[derive(Clone)]
pub(crate) struct BindingEntry {
    /// Stable index into `DEFAULT_BINDINGS` (identity for rebind/reset).
    pub idx: usize,
    /// Current keystrokes (an override, or the default).
    pub keystrokes: String,
    pub default_keystrokes: &'static str,
    pub action: &'static str,
    pub context: Option<&'static str>,
    pub category: &'static str,
    pub desc: &'static str,
}

impl BindingEntry {
    pub(crate) fn is_changed(&self) -> bool {
        self.keystrokes != self.default_keystrokes
    }
}

/// The live registry: every binding, in registration order. Owned by the root
/// view (`YaldaGpuiView::keymap_registry`) so it is both what `register_keymap`
/// applies at boot and what the reference tile rebinds at runtime.
pub(crate) struct KeymapRegistry {
    pub entries: Vec<BindingEntry>,
}

/// Persisted override — keyed by (action, context, default) so it survives table
/// reordering across versions; a row whose default keystrokes no longer exist is
/// silently dropped on load.
#[derive(Serialize, Deserialize)]
struct PersistedOverride {
    action: String,
    context: Option<String>,
    default_keystrokes: String,
    keystrokes: String,
}

impl KeymapRegistry {
    /// Build the registry from defaults, then fold in any persisted overrides.
    pub(crate) fn load() -> Self {
        let mut reg = Self::defaults();
        reg.apply_overrides(load_overrides());
        reg
    }

    /// The pristine registry — defaults only, no persisted overrides.
    pub(crate) fn defaults() -> Self {
        let entries = DEFAULT_BINDINGS
            .iter()
            .enumerate()
            .map(|(idx, d)| BindingEntry {
                idx,
                keystrokes: d.keys.to_string(),
                default_keystrokes: d.keys,
                action: d.action,
                context: d.ctx,
                category: d.cat,
                desc: d.desc,
            })
            .collect();
        KeymapRegistry { entries }
    }

    fn apply_overrides(&mut self, overrides: Vec<PersistedOverride>) {
        for ov in overrides {
            if let Some(e) = self.entries.iter_mut().find(|e| {
                e.action == ov.action
                    && e.context.map(str::to_string) == ov.context
                    && e.default_keystrokes == ov.default_keystrokes
            }) {
                e.keystrokes = ov.keystrokes;
            }
        }
    }

    /// Rebuild the app's entire keymap from this registry. Atomic: clears the
    /// old bindings and installs the new set in registration order (so GPUI's
    /// specificity/recency resolution is unchanged from the ported defaults).
    /// Entries whose action name or keystrokes fail to parse are skipped rather
    /// than panicking — a bad override can never brick the keymap.
    pub(crate) fn apply(&self, app: &mut GpuiApp) {
        app.clear_key_bindings();
        let mut bindings = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            if e.keystrokes.trim().is_empty() {
                continue; // an unbound row
            }
            let full = format!("yalda::{}", e.action);
            let Ok(action) = app.build_action(&full, None) else {
                continue;
            };
            let predicate = match e.context {
                Some(c) => match gpui::KeyBindingContextPredicate::parse(c) {
                    Ok(p) => Some(std::rc::Rc::new(p)),
                    Err(_) => continue,
                },
                None => None,
            };
            if let Ok(kb) = gpui::KeyBinding::load(
                &e.keystrokes,
                action,
                predicate,
                false,
                None,
                &gpui::DummyKeyboardMapper,
            ) {
                bindings.push(kb);
            }
        }
        app.bind_keys(bindings);
    }

    /// Change the keystrokes of the entry at `idx`. Returns false if the new
    /// keystrokes don't parse (caller keeps the old binding). Does NOT re-apply
    /// or persist — the caller does both after a successful edit.
    pub(crate) fn rebind(&mut self, idx: usize, keystrokes: &str) -> bool {
        let keystrokes = keystrokes.trim();
        if keystrokes.is_empty() || !keystrokes_parse(keystrokes) {
            return false;
        }
        if let Some(e) = self.entries.iter_mut().find(|e| e.idx == idx) {
            e.keystrokes = keystrokes.to_string();
            true
        } else {
            false
        }
    }

    /// Restore the entry at `idx` to its default keystrokes.
    pub(crate) fn reset(&mut self, idx: usize) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.idx == idx) {
            e.keystrokes = e.default_keystrokes.to_string();
        }
    }

    /// Restore every entry to its default.
    pub(crate) fn reset_all(&mut self) {
        for e in self.entries.iter_mut() {
            e.keystrokes = e.default_keystrokes.to_string();
        }
    }

    pub(crate) fn entry(&self, idx: usize) -> Option<&BindingEntry> {
        self.entries.iter().find(|e| e.idx == idx)
    }

    /// Other entries whose keystrokes collide with the entry at `idx` in an
    /// overlapping context (same context, or one of them is global). Intentional
    /// shadows (e.g. global `cmd-0` vs `AgentView` `cmd-0`) show up here too —
    /// the tile surfaces them as an advisory, never a hard block.
    pub(crate) fn conflicts(&self, idx: usize) -> Vec<usize> {
        let Some(target) = self.entry(idx) else {
            return Vec::new();
        };
        self.entries
            .iter()
            .filter(|e| {
                e.idx != idx
                    && !e.keystrokes.trim().is_empty()
                    && e.keystrokes == target.keystrokes
                    && contexts_overlap(e.context, target.context)
            })
            .map(|e| e.idx)
            .collect()
    }

    /// Persist the current overrides (entries differing from their default) to
    /// `~/.yalda/keymap-overrides.json`. No-op in tests / when no path.
    pub(crate) fn persist(&self) {
        let Some(path) = keymap_overrides_path() else {
            return;
        };
        let overrides: Vec<PersistedOverride> = self
            .entries
            .iter()
            .filter(|e| e.is_changed())
            .map(|e| PersistedOverride {
                action: e.action.to_string(),
                context: e.context.map(str::to_string),
                default_keystrokes: e.default_keystrokes.to_string(),
                keystrokes: e.keystrokes.clone(),
            })
            .collect();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&overrides) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// How many entries currently differ from their default.
    pub(crate) fn changed_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_changed()).count()
    }

    /// Test-only: report every table entry whose action name isn't registered,
    /// whose context predicate doesn't parse, or whose keystrokes don't parse.
    /// An empty result means `apply` would bind every row (nothing silently
    /// skipped) — the guard against a typo'd action name in `DEFAULT_BINDINGS`.
    #[cfg(test)]
    pub(crate) fn validate(&self, app: &GpuiApp) -> Vec<String> {
        let mut bad = Vec::new();
        for e in &self.entries {
            if app
                .build_action(&format!("yalda::{}", e.action), None)
                .is_err()
            {
                bad.push(format!("unknown action: {}", e.action));
            }
            if let Some(c) = e.context
                && gpui::KeyBindingContextPredicate::parse(c).is_err()
            {
                bad.push(format!("bad context: {c}"));
            }
            for chord in e.keystrokes.split_whitespace() {
                if Keystroke::parse(chord).is_err() {
                    bad.push(format!("bad keystrokes: {} ({})", e.keystrokes, e.action));
                }
            }
        }
        bad
    }
}

/// Two contexts "overlap" (can both match the same focused element) if they're
/// equal or either is global (`None`).
fn contexts_overlap(a: Option<&str>, b: Option<&str>) -> bool {
    a == b || a.is_none() || b.is_none()
}

/// Does a keystroke string parse as a valid GPUI binding? Each space-separated
/// chord must parse via `Keystroke::parse`.
fn keystrokes_parse(s: &str) -> bool {
    !s.split_whitespace().count().eq(&0)
        && s.split_whitespace()
            .all(|chord| Keystroke::parse(chord).is_ok())
}

fn load_overrides() -> Vec<PersistedOverride> {
    let Some(path) = keymap_overrides_path() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}
