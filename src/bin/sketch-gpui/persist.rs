//! Durable on-disk state: sketch-home paths, client id, preferences,
//! workspace snapshot/restore (tabs/splits/rails), persisted ACP session
//! slots, and session-server launch/attach helpers. Extracted verbatim
//! from main.rs (split-gpui-main).

use super::*;

/// Path to the JSON file that maps cwd → list of ACP session slots. Lives
/// next to `debug.log` so all sketch-managed transient state stays in one
/// place.
pub(crate) fn acp_session_persist_path() -> Option<PathBuf> {
    sketch::paths::sketch_home().map(|d| d.join("acp_sessions.json"))
}

/// Path to the one-line UUID file holding this GUI install's STABLE client id
/// (spec phase 4). Sibling to `acp_session_persist_path` under
/// `~/.sketch/`. Chosen over `config.kdl` (this is an implementation
/// detail, not user-facing) and `preferences.json` (no JSON restructure; `rm`
/// to reset).
pub(crate) fn client_id_path() -> Option<PathBuf> {
    sketch::paths::sketch_home().map(|d| d.join("client_id"))
}

/// Load (or first-time generate) this GUI's stable `client_id`. The lease model
/// keys ownership on this id so a restart/reconnect resumes with zero
/// contention.
///
/// A per-process `SKETCH_CLIENT_ID` override WINS, so a blue-green *candidate*
/// (launched with a fresh per-process UUID) is a DISTINCT client from the
/// original — it lands as Observer while the original's lease is live, then
/// Promotes under its own id. This is load-bearing: if the candidate read the
/// on-disk id it would impersonate the original and steal the lease.
///
/// Production (no env) reads/creates the persistent file, so a normal restart
/// resumes the lease.
pub(crate) fn load_or_create_client_id() -> String {
    if let Ok(env_id) = std::env::var("SKETCH_CLIENT_ID") {
        let t = env_id.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(path) = client_id_path() {
        if let Ok(id) = std::fs::read_to_string(&path) {
            let t = id.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(&path, &id);
        id
    } else {
        // No cache dir: ephemeral per-process id (rare; lease still works within
        // this process's lifetime, just doesn't survive a restart).
        uuid::Uuid::new_v4().to_string()
    }
}

/// Sketch's process cwd, with a safe fallback. Used both as the default
/// per-session cwd for new agent slots (spec-agent-cwd.md §1) and as the
/// top-level key in `acp_sessions.json` / `workspace.json`.
pub(crate) fn process_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

/// Canonicalize a path for resume-matching, falling back to the path verbatim
/// when it can't be resolved (e.g. it no longer exists). Both the stored
/// session cwd and the current cwd go through this before comparison, so a
/// symlinked / non-normalized launch directory still matches its saved session
/// instead of silently falling into the "create a fresh session" branch (which
/// is what made a resumed session look like it was "replaced" by a new one).
/// Comparing raw-vs-raw on a canonicalize failure preserves the old exact-match
/// behavior with no regression.
pub(crate) fn cwd_match_key(p: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// THE canonical on-disk key for a cwd (ADR-0010 / D5). Every cwd-keyed map in
/// our persistence files (`workspace.json`, the ACP-session file) keys through
/// this, so a symlinked / non-normalized / `/tmp`-vs-`/private/tmp` launch dir
/// resolves to the SAME string the entry was saved under — instead of silently
/// missing and resurrecting the workspace/session as empty. Mirrors
/// [`cwd_match_key`] (the resume *filter* key); this is its on-disk twin.
/// Falls back to the raw spelling when `canonicalize` fails (deleted path), so
/// a transient stat failure never regresses to never-matching.
pub(crate) fn persist_cwd_key(cwd: &std::path::Path) -> String {
    cwd_match_key(cwd).to_string_lossy().into_owned()
}

/// Attach to a server session and learn our role in ONE deterministic round
/// trip (spec phase 4 — replaces the old `attach_owner_with_retry` race).
///
/// The lease keys on this GUI's STABLE `client_id` (already set on the client),
/// so a returning instance presents the SAME id as any lingering lease and the
/// server's same-`client_id` branch RESUMES on the first attempt — regardless
/// of whether the prior socket's EOF has been processed, and regardless of
/// expiry (live → renew, expired → re-grant). There is nothing to wait out, so
/// the 8×300ms retry + "already own" error-string sniffing is gone.
///
/// Returns `Ok(true)` when this attach was granted drive rights (the lease) and
/// `Ok(false)` when it downgraded to Observer (a different live client holds the
/// lease, or `want_owner` is false). The role comes straight from the response's
/// `driver` flag, never from an inferred error.
///
/// Still runs on the background executor (never the paint thread).
///
/// Named `attach_for_role` (not `_with_retry`): there is no retry loop anymore —
/// it is a single deterministic attach whose Owner/Observer outcome the caller
/// records onto the slot's `is_driver`.
pub(crate) fn attach_for_role(
    handle: &sketch::session_client::SessionServerHandle,
    sid: &str,
    want_owner: bool,
) -> Result<bool, String> {
    let mode = if want_owner {
        AttachMode::Owner
    } else {
        AttachMode::Observer
    };
    handle.attach(sid, mode).map_err(|e| e.to_string())
}

/// Whether this process was launched as a build-loop candidate.
pub(crate) fn is_candidate_launch() -> bool {
    std::env::var("SKETCH_CANDIDATE").as_deref() == Ok("1")
}

/// Connect to the session server, the default model: a persistent server owns
/// the agent subprocesses so sessions survive GUI restarts/crashes, and the
/// GUI auto-launches a detached one if none is running. Set
/// `SKETCH_SESSION_SERVER=0` to force the legacy in-process direct-spawn path.
/// Returns `None` when disabled, or when the connection/launch fails (falls
/// back to direct spawning so the GUI still starts).
pub(crate) fn connect_session_server() -> Option<SessionServerClient> {
    if std::env::var("SKETCH_SESSION_SERVER").as_deref() == Ok("0") {
        eprintln!("[sketch-gpui] session server disabled (SKETCH_SESSION_SERVER=0); direct spawn");
        return None;
    }
    match SessionServerClient::connect() {
        Ok(client) => {
            // Install the stable lease identity (phase 4) right after connect so
            // every attach / heartbeat / gated action carries it. Survives
            // in-place reconnect (the client re-applies it onto the rebuilt
            // struct) AND app restart (persisted to ~/.sketch/client_id).
            client.set_client_id(load_or_create_client_id());
            eprintln!("[sketch-gpui] connected to session server");
            Some(client)
        }
        Err(e) => {
            eprintln!(
                "[sketch-gpui] session server connect failed: {e}; falling back to direct spawn"
            );
            None
        }
    }
}

/// Resolve a user-typed path argument to an absolute directory, per
/// spec-agent-cwd.md §2: expand a leading `~`, canonicalize when the
/// directory exists, fall back to process-cwd-relative resolution with
/// `.`/`..` collapsed otherwise, then validate that the result names a
/// directory. Returns the absolute path on success, or an error string
/// suitable for a footer hint on failure.
pub(crate) fn resolve_agent_cwd_arg(arg: &str) -> Result<PathBuf, String> {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return Err("missing path argument".into());
    }
    // 1) Tilde expansion. `~` or `~/...` → $HOME/.... `~user/...` is not
    //    supported in v1 — sketch is single-user.
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let expanded: PathBuf = if trimmed == "~" {
        match home {
            Some(h) => h,
            None => return Err("$HOME not set, cannot expand ~".into()),
        }
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        match home {
            Some(h) => h.join(rest),
            None => return Err("$HOME not set, cannot expand ~".into()),
        }
    } else {
        PathBuf::from(trimmed)
    };

    // 2) Canonicalize when possible, else fall back to cwd-relative with
    //    `.`/`..` collapsed (same pattern as `Workspace::canonical_key`).
    let resolved = match std::fs::canonicalize(&expanded) {
        Ok(c) => c,
        Err(_) => {
            let abs = if expanded.is_absolute() {
                expanded
            } else {
                process_cwd().join(&expanded)
            };
            let mut out = PathBuf::new();
            for comp in abs.components() {
                match comp {
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => out.push(other.as_os_str()),
                }
            }
            out
        }
    };

    // 3) Validate.
    if !resolved.is_dir() {
        return Err(format!("not a directory: {}", resolved.display()));
    }
    Ok(resolved)
}

/// Shorten an absolute path for display in the Status Strip
/// (spec-agent-cwd.md §6): replace a `$HOME` prefix with `~`, then if the
/// result is longer than 32 characters elide the middle so the leading and
/// trailing segments survive.
pub(crate) fn shorten_cwd_for_display(cwd: &std::path::Path) -> String {
    let raw = cwd.display().to_string();
    let shortened = if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home).display().to_string();
        if let Some(rest) = raw.strip_prefix(&home) {
            if rest.is_empty() {
                "~".to_string()
            } else if rest.starts_with('/') {
                format!("~{}", rest)
            } else {
                raw
            }
        } else {
            raw
        }
    } else {
        raw
    };
    if shortened.chars().count() <= 32 {
        return shortened;
    }
    // Keep leading two and trailing two segments. If we can't get that many
    // segments, fall back to leading-truncation with a `…` prefix.
    let parts: Vec<&str> = shortened.split('/').collect();
    if parts.len() >= 4 {
        let head = parts[..2].join("/");
        let tail = parts[parts.len() - 2..].join("/");
        return format!("{}/…/{}", head, tail);
    }
    // Few segments but very long names: leading-truncate.
    let chars: Vec<char> = shortened.chars().collect();
    let keep_tail = 30;
    if chars.len() > keep_tail + 1 {
        let tail: String = chars[chars.len() - keep_tail..].iter().collect();
        format!("…{}", tail)
    } else {
        shortened
    }
}

/// Path to the JSON file that maps cwd → workspace snapshot (tabs + layout
/// tree). Companion to acp_sessions.json; cleared by clearing cache_dir.
pub(crate) fn workspace_persist_path() -> Option<PathBuf> {
    sketch::paths::sketch_home().map(|d| d.join("workspace.json"))
}

/// Path to the JSON file holding app-managed runtime preferences (theme
/// choice, eventually other "View" menu state). Kept separate from the
/// user-edited `~/.config/sketch/config.kdl` so the menu-driven theme
/// switcher doesn't have to rewrite a hand-curated config file. On launch
/// preferences override the config's theme — if the user picked a theme
/// from the menu, that's what they expect next time, regardless of what
/// the kdl says.
pub(crate) fn preferences_path() -> Option<PathBuf> {
    sketch::paths::sketch_home().map(|d| d.join("preferences.json"))
}

/// Where the agent info bar sits relative to the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AgentStatusPosition {
    Top,
    #[default]
    Bottom,
}

impl AgentStatusPosition {
    pub(crate) fn toggle(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "top" => Self::Top,
            _ => Self::Bottom,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Preferences {
    /// Kebab-case theme identifier — `ThemeName::as_kebab()` /
    /// `ThemeName::parse()`. `None` means "no app-managed override; use
    /// the value from config.kdl (or the built-in default)."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) theme: Option<String>,
    /// Agent info bar placement: "top" or "bottom".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_status_position: Option<String>,
    /// Document text-zoom factor (`Cmd-=`/`Cmd--`/`Cmd-0`). `None` means "no
    /// saved zoom; start at 1.0." Clamped to `[MIN_TEXT_SCALE, MAX_TEXT_SCALE]`
    /// on load so a hand-edited file can't push the body off-screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text_scale: Option<f32>,
}

pub(crate) fn load_preferences() -> Preferences {
    let Some(path) = preferences_path() else {
        return Preferences::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Preferences::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Best-effort write. Silently no-ops on any I/O / serialization failure —
/// preference persistence is a convenience, not a correctness boundary.
pub(crate) fn save_preferences(prefs: &Preferences) {
    let Some(path) = preferences_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(prefs) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Serializable shadow of `WindowContent` for spec-tabs-and-splits.md
/// Behavior 23. Doc/Edit persist their file path; Browser its current_dir;
/// Claude its session_id (or `None` if not yet attached). Window-local view
/// state (scroll, cursor) is intentionally NOT persisted (Constraint §4).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub(crate) enum PersistedKind {
    Doc {
        path: PathBuf,
    },
    Edit {
        path: PathBuf,
    },
    Browser {
        dir: PathBuf,
    },
    /// JSON tag stays as "claude" so saved layouts from earlier builds load
    /// without migration; the in-memory variant is `Agent` to match the rest
    /// of the rename pass (spec-agent-window.md).
    #[serde(rename = "claude")]
    Agent {
        session_id: Option<String>,
    },
}

/// One leaf in a persisted layout. Carries the (stable) window id so
/// `focused_window` references survive restore.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedLeaf {
    pub(crate) id: workspace::WindowId,
    #[serde(flatten)]
    pub(crate) kind: PersistedKind,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistedLayout {
    Leaf(PersistedLeaf),
    Split {
        dir: workspace::SplitDir,
        children: Vec<(f32, PersistedLayout)>,
    },
}

/// Persisted rail kind tag (spec-rail.md §14). Outline rails persist only
/// their kind — the heading list re-derives on restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistedRailKind {
    FileBrowser,
    Outline,
}

/// Persisted per-tab rail (spec-rail.md §14). Optional on `PersistedTab` so
/// snapshots written before rails existed still load (serde default → `None`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedRail {
    pub(crate) kind: PersistedRailKind,
    #[serde(default)]
    pub(crate) side: workspace::RailSide,
    /// Column width in px. Older/partial entries default to the standard width.
    #[serde(default = "default_rail_width")]
    pub(crate) width: f32,
    /// File-browser rail: directory it was rooted at. Absent for outline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<PathBuf>,
    /// Leaf the rail is pinned to. Defaults to 0 for old snapshots (will be
    /// overridden by the tab's focused_window on restore).
    #[serde(default)]
    pub(crate) pinned_to: workspace::WindowId,
}

pub(crate) fn default_rail_width() -> f32 {
    workspace::RAIL_DEFAULT_WIDTH
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedTab {
    pub(crate) auto_name: String,
    pub(crate) display_name: Option<String>,
    pub(crate) focused_window: workspace::WindowId,
    pub(crate) layout: PersistedLayout,
    /// Optional rail (spec-rail.md §14). Absent in old snapshots → no rail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rail: Option<PersistedRail>,
    // Layout patterns: per-tab layout mode + master-stack params
    #[serde(default)]
    pub(crate) layout_mode: workspace::LayoutMode,
    #[serde(default = "default_master_ratio")]
    pub(crate) master_ratio: f32,
    #[serde(default = "default_master_count")]
    pub(crate) master_count: usize,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub(crate) tag_view: std::collections::BTreeSet<String>,
}

pub(crate) fn default_master_ratio() -> f32 {
    0.6
}
pub(crate) fn default_master_count() -> usize {
    1
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedWorkspace {
    pub(crate) tabs: Vec<PersistedTab>,
    pub(crate) active_tab: usize,
    // Layout patterns: workspace-global marks
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) marks: HashMap<char, workspace::WindowId>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) tag_shortcuts: HashMap<char, String>,
    // Buffer-level tags (keyed by canonical path string)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) buffer_tags: HashMap<String, Vec<String>>,
}

/// Snapshot a live `WindowContent` into its persisted shadow. Returns `None`
/// for content kinds that aren't worth persisting (e.g., an unattached
/// transient state we'd lose nothing by skipping).
pub(crate) fn snapshot_content(content: &WindowContent) -> PersistedKind {
    match content {
        WindowContent::Doc(d) => PersistedKind::Doc {
            path: PathBuf::from(d.file_label.as_ref()),
        },
        WindowContent::Edit(e) => PersistedKind::Edit {
            path: PathBuf::from(e.file_label.as_ref()),
        },
        WindowContent::Browser(b) => PersistedKind::Browser {
            dir: b.fb.current_dir().to_path_buf(),
        },
        WindowContent::Agent(ring) => {
            // Use the active session's id if any. Multi-session restore is
            // handled by the existing ACP persistence path; this is just
            // enough to know "this slot had a Claude session" so on restore
            // we can spawn the ring shell.
            let session_id = ring
                .slots
                .first()
                .and_then(|s| s.state.channel.as_ref())
                .and_then(|c| c.session_id().map(|s| s.to_string()));
            PersistedKind::Agent { session_id }
        }
    }
}

/// Snapshot a live `Layout<WindowContent>` into its persisted shadow.
pub(crate) fn snapshot_layout(layout: &workspace::Layout<WindowContent>) -> PersistedLayout {
    match layout {
        workspace::Layout::Empty => PersistedLayout::Leaf(PersistedLeaf {
            id: 0,
            kind: PersistedKind::Browser {
                dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            },
        }),
        workspace::Layout::Leaf(win) => PersistedLayout::Leaf(PersistedLeaf {
            id: win.id,
            kind: snapshot_content(&win.content),
        }),
        workspace::Layout::Split { dir, children } => PersistedLayout::Split {
            dir: *dir,
            children: children
                .iter()
                .map(|(w, c)| (*w, snapshot_layout(c)))
                .collect(),
        },
    }
}

/// Snapshot a live rail into its persisted shadow (spec-rail.md §14).
pub(crate) fn snapshot_rail(rail: &workspace::RailState) -> PersistedRail {
    match &rail.content {
        workspace::RailContent::FileBrowser(fb) => PersistedRail {
            kind: PersistedRailKind::FileBrowser,
            side: rail.side,
            width: rail.width_px,
            cwd: Some(fb.current_dir().to_path_buf()),
            pinned_to: rail.pinned_to,
        },
        workspace::RailContent::Outline(_) => PersistedRail {
            kind: PersistedRailKind::Outline,
            side: rail.side,
            width: rail.width_px,
            cwd: None,
            pinned_to: rail.pinned_to,
        },
    }
}

/// Reconstruct a live rail from its persisted shadow (spec-rail.md §14). The
/// restored rail is unfocused (focus stays on the content leaf on restore).
/// `fallback_pin` is used when the snapshot predates the `pinned_to` field.
pub(crate) fn restore_rail(p: PersistedRail, fallback_pin: workspace::WindowId) -> workspace::RailState {
    let content = match p.kind {
        PersistedRailKind::FileBrowser => {
            let dir = match p.cwd {
                Some(d) if d.is_dir() => d,
                _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            };
            workspace::RailContent::FileBrowser(FileBrowser::new(dir))
        }
        PersistedRailKind::Outline => {
            workspace::RailContent::Outline(workspace::OutlineState::new())
        }
    };
    let pinned_to = if p.pinned_to != 0 {
        p.pinned_to
    } else {
        fallback_pin
    };
    workspace::RailState {
        content,
        side: p.side,
        width_px: p.width,
        focused: false,
        pinned_to,
    }
}

/// Reconstruct a persisted layout into live `WindowContent`, opening any
/// file-backed leaves through `ws`'s buffer pool so two restored views of the
/// same file share one core. Returns the live layout plus the max window id
/// seen (so the caller can advance the id allocator past restored ids).
/// Returns (layout, max_window_id, agent_leaf_ids).
pub(crate) fn restore_layout(
    ws: &mut workspace::Workspace<WindowContent>,
    theme: &Theme,
    layout: PersistedLayout,
) -> (
    workspace::Layout<WindowContent>,
    workspace::WindowId,
    Vec<workspace::WindowId>,
) {
    match layout {
        PersistedLayout::Leaf(leaf) => {
            let id = leaf.id;
            let is_agent = matches!(&leaf.kind, PersistedKind::Agent { .. });
            let content = restore_content(ws, theme, leaf.kind);
            let agents = if is_agent { vec![id] } else { vec![] };
            (
                workspace::Layout::Leaf(workspace::Window { id, content }),
                id,
                agents,
            )
        }
        PersistedLayout::Split { dir, children } => {
            let mut max_id: workspace::WindowId = 0;
            let mut agents = Vec::new();
            let mut restored_children = Vec::with_capacity(children.len());
            for (w, child) in children {
                let (sub, sub_max, sub_agents) = restore_layout(ws, theme, child);
                if sub_max > max_id {
                    max_id = sub_max;
                }
                agents.extend(sub_agents);
                restored_children.push((w, sub));
            }
            (
                workspace::Layout::Split {
                    dir,
                    children: restored_children,
                },
                max_id,
                agents,
            )
        }
    }
}

pub(crate) fn restore_content(
    ws: &mut workspace::Workspace<WindowContent>,
    theme: &Theme,
    kind: PersistedKind,
) -> WindowContent {
    match kind {
        PersistedKind::Doc { path } => {
            let label: SharedString = path.display().to_string().into();
            // 5c: restore the Doc bound to its pooled core (shared text/undo +
            // live tracking). Fall back to a Browser if the file vanished since
            // it was persisted (mirrors the Edit restore path).
            match ws.open_and_retain(&path) {
                Ok((id, core)) => {
                    let blocks =
                        render_with_wiki(&core.borrow().document().full_text(), theme, Some(&path));
                    WindowContent::Doc(DocState {
                        blocks,
                        file_label: label,
                        cursor_block: 0,
                        list_state: DocState::new_list_state(0),
                        list_item_count: std::cell::Cell::new(0),
                        blocks_seq: 0,
                        blocks_snapshot: RefCell::new(None),
                        last_cursor_block: std::cell::Cell::new(None),
                        source: Some(DocSource::new(id, core)),
                    })
                }
                Err(_) => WindowContent::Browser(BrowserWindow::standalone(
                    path.parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from(".")),
                )),
            }
        }
        PersistedKind::Edit { path } => {
            let label: SharedString = path.display().to_string().into();
            // Restore through the pool — a second restored Edit view of the
            // same file binds to the same shared core.
            match ws.open_and_retain(&path) {
                Ok((id, core)) => WindowContent::Edit(EditState::new(
                    SharedEditor::new(id, core),
                    label,
                    EditView::Code,
                )),
                Err(_) => WindowContent::Browser(BrowserWindow::standalone(
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                )),
            }
        }
        PersistedKind::Browser { dir } => {
            let dir = if dir.is_dir() {
                dir
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            };
            WindowContent::Browser(BrowserWindow::standalone(dir))
        }
        PersistedKind::Agent { .. } => {
            // Claude restore is its own subsystem (acp_sessions.json +
            // open_agent_inner). Replace with a Browser stub here so the
            // tab survives; user can re-attach via the existing Claude
            // commands.
            WindowContent::Browser(BrowserWindow::standalone(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            ))
        }
    }
}

/// Snapshot a live workspace into a fully serializable shape.
pub(crate) fn snapshot_workspace(ws: &workspace::Workspace<WindowContent>) -> PersistedWorkspace {
    PersistedWorkspace {
        tabs: ws
            .tabs
            .iter()
            .map(|t| PersistedTab {
                auto_name: t.auto_name.clone(),
                display_name: t.display_name.clone(),
                focused_window: t.focused,
                layout: snapshot_layout(&t.layout),
                rail: t.rail.as_ref().map(snapshot_rail),
                layout_mode: t.layout_mode,
                master_ratio: t.master_ratio,
                master_count: t.master_count,
                tag_view: t.tag_view.clone(),
            })
            .collect(),
        active_tab: ws.active_tab,
        marks: ws.marks.all_marks().into_iter().collect(),
        tag_shortcuts: ws.tag_shortcuts.clone(),
        buffer_tags: {
            let mut bt = HashMap::new();
            for buf in ws.file_buffers.values() {
                if !buf.tags.is_empty() {
                    bt.insert(
                        buf.canonical_path.display().to_string(),
                        buf.tags.iter().cloned().collect(),
                    );
                }
            }
            bt
        },
    }
}

/// Best-effort write of the workspace snapshot for `cwd`. Silently no-ops
/// on any I/O / serialization failure (Behavior 23: best-effort + silent).
pub(crate) fn save_persisted_workspace(cwd: &std::path::Path, ws: &workspace::Workspace<WindowContent>) {
    let Some(path) = workspace_persist_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Read-modify-write so other cwds in the file aren't clobbered (Constraint
    // §11 / multi-session §15: last-writer-wins).
    let mut map: serde_json::Map<String, serde_json::Value> = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let snap = snapshot_workspace(ws);
    if let Ok(v) = serde_json::to_value(&snap) {
        // Drop any entry saved under the old raw spelling so the file doesn't
        // accumulate a canonical + raw duplicate for the same dir (ADR-0010:
        // the next save rewrites canonical — this is that rewrite).
        map.remove(&cwd.display().to_string());
        map.insert(persist_cwd_key(cwd), v);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&map) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Read the persisted workspace for `cwd`. Returns `None` if no file, no
/// entry, or unparseable — the caller treats these as "no saved state,
/// bootstrap fresh" (Behavior 24).
pub(crate) fn load_persisted_workspace(cwd: &std::path::Path) -> Option<PersistedWorkspace> {
    let path = workspace_persist_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&bytes).ok()?;
    // Canonical key first; lazy fallback to the old raw spelling for entries
    // saved before D5 (ADR-0010 — adopt on read, next save rewrites canonical).
    let entry = map
        .get(&persist_cwd_key(cwd))
        .or_else(|| map.get(&cwd.display().to_string()))?;
    serde_json::from_value(entry.clone()).ok()
}

/// One restored session slot. Order in the returned `Vec` matches the
/// saved ring order; reboot rebuilds the ring in this same order.
/// `mode`, `tasklist_open`, and `subagents_open` are spec §35 additions;
/// older files (without these keys) deserialize with defaults
/// (Chatbox, false, false). Older sketch binaries reading newer files
/// silently drop the unknown keys (downgrade contract, §35).
/// `cwd` is a spec-agent-cwd.md §5 addition; `None` (absence in JSON)
/// resolves to the process cwd at restore time per §1.
#[derive(Debug, Clone)]
pub(crate) struct PersistedSlot {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) active: bool,
    pub(crate) mode: InputModeKind,
    pub(crate) tasklist_open: bool,
    pub(crate) subagents_open: bool,
    pub(crate) cwd: Option<PathBuf>,
}

/// Load the persisted slot list for `cwd`. Returns an empty vec if no
/// file, no entry, or unparseable input — all of which the caller treats
/// as "no saved state, open a fresh claude-1". Migrates the legacy
/// `{cwd: <string-id>}` shape on the fly to a one-element list labelled
/// `"claude-1"`.
pub(crate) fn load_persisted_acp_sessions(cwd: &std::path::Path) -> Vec<PersistedSlot> {
    let Some(path) = acp_session_persist_path() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    // Canonical key first; lazy fallback to the old raw spelling (ADR-0010).
    let raw = cwd.to_string_lossy();
    let Some(entry) = json
        .get(persist_cwd_key(cwd))
        .or_else(|| json.get(raw.as_ref()))
    else {
        return Vec::new();
    };
    // Legacy single-string shape: synthesize a one-slot list with the
    // spec-§35 defaults for the missing fields.
    if let Some(id) = entry.as_str() {
        return vec![PersistedSlot {
            id: id.to_string(),
            label: "claude-1".into(),
            active: true,
            mode: InputModeKind::Chatbox,
            tasklist_open: false,
            subagents_open: false,
            cwd: None,
        }];
    }
    let Some(arr) = entry.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            let obj = v.as_object()?;
            let id = obj.get("id")?.as_str()?.to_string();
            let label = obj
                .get("label")
                .and_then(|s| s.as_str())
                .unwrap_or("claude")
                .to_string();
            let active = obj.get("active").and_then(|b| b.as_bool()).unwrap_or(false);
            // Spec §35 additions. Missing keys default per the same
            // table (chatbox, false, false). Unknown mode strings fall
            // back to Chatbox.
            let mode = obj
                .get("mode")
                .and_then(|m| m.as_str())
                .map(|s| match s {
                    "worksheet" => InputModeKind::Worksheet,
                    _ => InputModeKind::Chatbox,
                })
                .unwrap_or(InputModeKind::Chatbox);
            let tasklist_open = obj
                .get("tasklist_open")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            let subagents_open = obj
                .get("subagents_open")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            // spec-agent-cwd.md §5: optional per-slot cwd. Absent (old
            // file, or pre-spec save) is loaded as None so the restore
            // path can fall back to process cwd per §1.
            let cwd = obj.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from);
            Some(PersistedSlot {
                id,
                label,
                active,
                mode,
                tasklist_open,
                subagents_open,
                cwd,
            })
        })
        .collect()
}

/// Classify an attach error string as the PERMANENT "session is gone" case.
/// The session-server actor returns `no such session: <id>` for a lookup miss
/// (every `.ok_or_else(|| format!("no such session: {session_id}"))` site in
/// `sketch-session-server/main.rs`). That means the persisted id outlived the
/// server's WAL — the slot can never reattach and must be dropped, not retried.
/// Matched case-insensitively via `.contains` so wrapping/prefixing (e.g. the
/// io::Error round-trip) can't hide it. Transient errors ("disconnected",
/// write/read failures) deliberately do NOT match.
pub(crate) fn is_session_gone_error(e: &str) -> bool {
    e.to_ascii_lowercase().contains("no such session")
}

/// Forget the saved ACP session list for `cwd`. Used by `claude-clear` so
/// the next attach hits `session/new` instead of resuming the cleared
/// sessions.
pub(crate) fn forget_persisted_acp_sessions(cwd: &std::path::Path) {
    let Some(path) = acp_session_persist_path() else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    if let Some(obj) = json.as_object_mut() {
        // Clear both spellings so a pre-D5 raw entry can't linger (ADR-0010).
        obj.remove(persist_cwd_key(cwd).as_str());
        obj.remove(cwd.to_string_lossy().as_ref());
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, serialized);
    }
}

/// Remove specific session ids from the persisted ACP-session file, across
/// EVERY cwd key (the file is `{ cwd_key: [ {id, ...}, ... ] }`). Used when an
/// attach reports a session the server no longer has: re-saving the live rings
/// can't be relied on to drop the id (a single-slot ring that empties no longer
/// holds an Agent ring to re-save, and a stale id in a non-active tab is never
/// walked), so we scrub by id here. A cwd whose array becomes empty has its key
/// removed. Best-effort.
pub(crate) fn forget_persisted_acp_session_ids(ids: &[String]) {
    if ids.is_empty() {
        return;
    }
    let Some(path) = acp_session_persist_path() else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    let dead: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    if let Some(obj) = json.as_object_mut() {
        for v in obj.values_mut() {
            if let Some(arr) = v.as_array_mut() {
                arr.retain(|entry| {
                    entry
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(|id| !dead.contains(id))
                        .unwrap_or(true)
                });
            }
        }
        // Drop now-empty cwd arrays so the file doesn't accumulate dead keys.
        obj.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, serialized);
    }
}

/// Persist the ring's slots for `cwd` so the next sketch run can resume
/// every session in the ring, not just the active one. Best-effort writes
/// — failures (no cache dir, permissions, malformed prior file) silently
/// bail. Per-slot id resolution honors the resume_id stability rule: if a
/// slot was restored with a `resume_id`, that id is what gets persisted
/// (even if `session/load` failed and the slot fell back to a fresh
/// `session/new`). Slots without an id (pending attach or attach failed
/// outright) are skipped.
///
/// Concurrent sketch instances on the same `cwd`: last-writer-wins. Each
/// call does a read-modify-write of the file, replacing only the cwd
/// entry; other cwds are preserved.
pub(crate) fn save_persisted_acp_sessions(cwd: &std::path::Path, ring: &AgentRing) {
    let Some(path) = acp_session_persist_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let active_index = ring.active;
    let entries: Vec<serde_json::Value> = ring
        .slots
        .iter()
        .enumerate()
        .filter_map(|(i, slot)| {
            // resume_id wins over channel id: if we were trying to resume,
            // keep retrying the original id even when load fell back.
            let id = slot
                .resume_id
                .clone()
                .or_else(|| slot.state.channel.as_ref().and_then(|c| c.session_id()))?;
            let mut obj = serde_json::Map::new();
            obj.insert("id".into(), serde_json::Value::String(id));
            obj.insert(
                "label".into(),
                serde_json::Value::String(slot.label.clone()),
            );
            if i == active_index {
                obj.insert("active".into(), serde_json::Value::Bool(true));
            }
            // Spec §35: persist input mode and sidepane state per slot.
            // Older sketch binaries reading this file ignore the unknown
            // keys (serde's standard behavior); no migration needed.
            let mode_str = match slot.state.input_surface.mode() {
                InputModeKind::Worksheet => "worksheet",
                InputModeKind::Chatbox => "chatbox",
            };
            obj.insert(
                "mode".into(),
                serde_json::Value::String(mode_str.to_string()),
            );
            obj.insert(
                "tasklist_open".into(),
                serde_json::Value::Bool(slot.state.tasklist_open),
            );
            obj.insert(
                "subagents_open".into(),
                serde_json::Value::Bool(slot.state.subagents_open),
            );
            // spec-agent-cwd.md §5: persist the slot's working directory.
            // Lossy on non-UTF8 paths (Constraint §11) — same as the
            // top-level `cwd` key in this file. Acceptable on macOS where
            // APFS enforces UTF8-encodable names.
            obj.insert(
                "cwd".into(),
                serde_json::Value::String(slot.cwd.display().to_string()),
            );
            Some(serde_json::Value::Object(obj))
        })
        .collect();

    let mut json = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !json.is_object() {
        json = serde_json::json!({});
    }
    if let Some(obj) = json.as_object_mut() {
        if entries.is_empty() {
            // Don't leave a stale list behind if nothing is persistable
            // (e.g., user closed all sessions but reboot hasn't fired yet).
            obj.remove(persist_cwd_key(cwd).as_str());
            obj.remove(cwd.to_string_lossy().as_ref());
        } else {
            // Clear the old raw spelling, then write under the canonical key
            // (ADR-0010: next save rewrites canonical).
            obj.remove(cwd.to_string_lossy().as_ref());
            obj.insert(persist_cwd_key(cwd), serde_json::Value::Array(entries));
        }
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, serialized);
    }
}

// Test-only counter: incremented every time `render_agent` rebuilds the
// memoized view-model (flat_items + gutter). A fingerprint hit must leave
// this unchanged. Asserted by `view_model_memoization_fast_skip`.
#[cfg(test)]
thread_local! {
    pub(crate) static VIEW_MODEL_REBUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
