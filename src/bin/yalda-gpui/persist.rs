//! Durable on-disk state: yalda-home paths, client id, preferences,
//! workspace snapshot/restore (workspaces/splits/rails), persisted ACP session
//! slots, and session-server launch/attach helpers. Extracted verbatim
//! from main.rs (split-gpui-main).

use super::*;

/// Path to the JSON file that maps cwd → list of ACP session slots. Lives
/// next to `debug.log` so all yalda-managed transient state stays in one
/// place.
pub(crate) fn acp_session_persist_path() -> Option<PathBuf> {
    // Fail safe in test builds — NEVER touch the user's real
    // `~/.yalda/acp_sessions.json` (bug-0016: this is where renamed-session
    // LABELS live, so a test that clobbered it wiped the user's custom names,
    // which reverted to `claude-N` on the next launch). `save_agent_ring`
    // fires from `/clear`, restore, rename, and ~every session mutation, so any
    // test that boots the real view and triggers one would otherwise overwrite
    // it. Round-trip tests opt in via `with_acp_persist_path`; every other test
    // gets `None` → save/load is a no-op. Mirrors `workspace_persist_path` /
    // `preferences_path`, which already had this guard — this fn was the one
    // that was missed (it fell through to `yalda_home()` under `cfg(test)`).
    #[cfg(test)]
    {
        return ACP_PERSIST_PATH_OVERRIDE.with(|c| c.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|d| d.join("acp_sessions.json"))
    }
}

/// Test-only seam: redirect the ACP-session persistence file to a tempdir so a
/// save→restore round-trip test never touches the user's real
/// `~/.yalda/acp_sessions.json`. Thread-local, so parallel tests don't collide.
#[cfg(test)]
thread_local! {
    pub(crate) static ACP_PERSIST_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_acp_persist_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    ACP_PERSIST_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(path));
    let r = f();
    ACP_PERSIST_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    r
}

/// Path to the id-keyed session-summary sidecar (`bug-0020`).
///
/// The autoname SUMMARY cannot live only in `acp_sessions.json`: that file is
/// keyed by cwd and only ever holds the sessions **bound to a tile** at save
/// time, so every free session's summary died on restart (and the jump panel
/// lists free sessions too). This file is a flat `{server session id → summary}`
/// map — the same durability the LABEL gets from the server WAL, for the one
/// piece of session metadata the server doesn't know about.
///
/// Same `cfg(test)` fail-safe as [`acp_session_persist_path`]: `None` under test
/// unless a test opts in via [`with_session_summaries_path`].
pub(crate) fn session_summaries_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return SUMMARIES_PATH_OVERRIDE.with(|c| c.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|d| d.join("session_summaries.json"))
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static SUMMARIES_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_session_summaries_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    SUMMARIES_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(path));
    let r = f();
    SUMMARIES_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    r
}

/// Load the whole `sid → summary` map (`bug-0020`). Missing/unparseable file =>
/// empty map; a summary is a nicety, never a reason to fail a boot.
pub(crate) fn load_session_summaries() -> std::collections::HashMap<String, String> {
    let Some(path) = session_summaries_path() else {
        return std::collections::HashMap::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return std::collections::HashMap::new();
    };
    serde_json::from_slice::<std::collections::HashMap<String, String>>(&bytes).unwrap_or_default()
}

/// Record one session's summary durably (`bug-0020`). Read-modify-write so
/// concurrent yalda instances only clobber the key they touched (last-writer-
/// wins per session, matching the ACP-slot file). Best-effort.
pub(crate) fn save_session_summary(sid: &ServerSid, summary: &str) {
    write_summary_entry(sid, summary.trim());
}

/// Record that the one-shot autoname has been SPENT for `sid` without producing
/// a summary (`bug-0021`): an empty-string entry. The autoname arm is keyed by
/// session identity now, not by which constructor built the state, so "have we
/// already tried this session?" has to outlive the process — otherwise every
/// launch re-asks Haiku about the same nameless session.
///
/// Never downgrades a real summary to empty.
pub(crate) fn mark_autoname_attempted(sid: &ServerSid) {
    if load_session_summaries()
        .get(sid.as_str())
        .is_some_and(|s| !s.is_empty())
    {
        return;
    }
    write_summary_entry(sid, "");
}

/// Has the one-shot autoname already been spent for `sid` (`bug-0021`)? True for
/// both outcomes — a summary landed, or the attempt produced nothing.
pub(crate) fn autoname_already_attempted(
    map: &std::collections::HashMap<String, String>,
    sid: &ServerSid,
) -> bool {
    map.contains_key(sid.as_str())
}

fn write_summary_entry(sid: &ServerSid, value: &str) {
    let Some(path) = session_summaries_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut map = load_session_summaries();
    map.insert(sid.to_string(), value.to_string());
    if let Ok(serialized) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, serialized);
    }
}

/// Drop summaries for sessions that are gone (`bug-0020`) — called from the same
/// place the dead persisted ids are scrubbed, so the sidecar can't grow forever.
pub(crate) fn forget_session_summaries(sids: &[String]) {
    let Some(path) = session_summaries_path() else {
        return;
    };
    let mut map = load_session_summaries();
    let before = map.len();
    map.retain(|k, _| !sids.iter().any(|s| s == k));
    if map.len() == before {
        return;
    }
    if let Ok(serialized) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, serialized);
    }
}

// ── Session tags sidecar (`session_tags.json`, UXI-JumpPanel-20) ────────────
// The same durability the summary gets: a flat `{server session id → [tag]}`
// map, keyed by server sid so a free session's tags survive restart (the
// cwd-keyed `acp_sessions.json` only holds tile-bound sessions). The GUI owns
// the canonical in-memory copy (`YaldaGpuiView::session_tags`); this is its
// on-disk twin, written whole from that copy on every tag edit.

/// Path to the tags sidecar. `None` under test unless a test opts in via
/// [`with_session_tags_path`] (same fail-safe as the summaries sidecar).
pub(crate) fn session_tags_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return TAGS_PATH_OVERRIDE.with(|c| c.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|d| d.join("session_tags.json"))
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static TAGS_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_session_tags_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    TAGS_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(path));
    let r = f();
    TAGS_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    r
}

/// Load the whole `sid → [tag]` map. Missing/unparseable => empty map; tags are
/// a nicety, never a reason to fail a boot.
pub(crate) fn load_session_tags() -> std::collections::HashMap<String, Vec<String>> {
    let Some(path) = session_tags_path() else {
        return std::collections::HashMap::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return std::collections::HashMap::new();
    };
    serde_json::from_slice::<std::collections::HashMap<String, Vec<String>>>(&bytes)
        .unwrap_or_default()
}

/// Persist the whole tags map from the GUI's canonical in-memory copy.
/// Best-effort; entries with no tags are elided to keep the file tidy.
pub(crate) fn save_session_tags(map: &std::collections::HashMap<String, Vec<String>>) {
    let Some(path) = session_tags_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let pruned: std::collections::BTreeMap<&String, &Vec<String>> =
        map.iter().filter(|(_, v)| !v.is_empty()).collect();
    if let Ok(serialized) = serde_json::to_string_pretty(&pruned) {
        let _ = std::fs::write(&path, serialized);
    }
}

/// Yalda's process cwd, with a safe fallback. Used both as the default
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

/// Attach the single client to a server session (strict 1:1) in one round trip.
/// The server replays the full event log before the response, so the pump picks
/// up the entire transcript on its first drain cycle. Runs on the background
/// executor (never the paint thread).
pub(crate) fn attach_session(
    handle: &yalda::session_client::SessionServerHandle,
    sid: &str,
) -> Result<(), String> {
    handle.attach(sid).map_err(|e| e.to_string())
}

/// Connect to the session server, the default model: a persistent server owns
/// the agent subprocesses so sessions survive GUI restarts/crashes, and the
/// GUI auto-launches a detached one if none is running. Set
/// `YALDA_SESSION_SERVER=0` to force the legacy in-process direct-spawn path.
/// Returns `None` when disabled, or when the connection/launch fails (falls
/// back to direct spawning so the GUI still starts).
/// Test-only hermetic seam: when set on the current thread, `connect_session_
/// server` returns `None` so a headless view never reaches out to whatever
/// `yalda-session-server` happens to be running on the dev box. Set/cleared via
/// [`with_no_session_server`] around a view construction.
#[cfg(test)]
thread_local! {
    pub(crate) static FORCE_NO_SESSION_SERVER: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Run `f` with the session server forced OFF on this thread (test-only). The
/// flag is restored afterward so the seam is scoped to the construction.
#[cfg(test)]
pub(crate) fn with_no_session_server<R>(f: impl FnOnce() -> R) -> R {
    FORCE_NO_SESSION_SERVER.with(|c| c.set(true));
    let r = f();
    FORCE_NO_SESSION_SERVER.with(|c| c.set(false));
    r
}

/// Test seam: force `clear_agent_session` down the SERVER branch (the real
/// client/server `/clear` path — placeholder + async `apply_open_agent_resolution`
/// bind) even though no live server is present under `cfg(test)`. Without this the
/// harness only ever exercises the legacy direct-spawn else-branch, so the real
/// user path (which the 7×-recurring "/clear worksheet invisible" bug lives on)
/// was never headlessly reproduced. `spawn_create_agent_session` bails gracefully
/// when `session_server` is `None`, leaving the placeholder bound with its
/// `pending_open_token` — exactly the real mid-open state the test then resolves.
#[cfg(test)]
thread_local! {
    pub(crate) static FORCE_SERVER_CLEAR_BRANCH: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Whether `clear_agent_session` should take the server branch on this thread.
/// Always `false` outside tests.
pub(crate) fn force_server_clear_branch() -> bool {
    #[cfg(test)]
    {
        return FORCE_SERVER_CLEAR_BRANCH.with(|c| c.get());
    }
    #[cfg(not(test))]
    false
}

/// Run `f` with the server `/clear` branch forced ON this thread (test-only).
#[cfg(test)]
pub(crate) fn with_server_clear_branch<R>(f: impl FnOnce() -> R) -> R {
    FORCE_SERVER_CLEAR_BRANCH.with(|c| c.set(true));
    let r = f();
    FORCE_SERVER_CLEAR_BRANCH.with(|c| c.set(false));
    r
}

/// Test seam: allow the real roster-row jump path to construct and bind its
/// local attachment placeholder while the hermetic harness has no live session
/// server. The eventual attach task still bails on `session_server == None`;
/// this only reaches the synchronous `jump_to_roster_session` →
/// `picker_attach_existing` identity handoff that production runs before it.
#[cfg(test)]
thread_local! {
    pub(crate) static FORCE_SERVER_ROSTER_JUMP_BRANCH: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

pub(crate) fn force_server_roster_jump_branch() -> bool {
    #[cfg(test)]
    {
        return FORCE_SERVER_ROSTER_JUMP_BRANCH.with(|c| c.get());
    }
    #[cfg(not(test))]
    false
}

#[cfg(test)]
pub(crate) fn with_server_roster_jump_branch<R>(f: impl FnOnce() -> R) -> R {
    FORCE_SERVER_ROSTER_JUMP_BRANCH.with(|c| c.set(true));
    let r = f();
    FORCE_SERVER_ROSTER_JUMP_BRANCH.with(|c| c.set(false));
    r
}

pub(crate) fn connect_session_server() -> Option<SessionServerClient> {
    // NOTE (clear-worksheet-invisible critique R4): under `cfg(test)` this still
    // falls through to a LIVE `SessionServerClient::connect()` unless a test wraps
    // itself in `with_no_session_server`. That is a real hygiene gap — a synthetic
    // sid can issue a real `attach` and get unbound — but forcing None under test
    // breaks steering tests that (fragilely) rely on the live connection. The
    // proper fix is a `test-support` in-process session-server seam; tracked as a
    // follow-up, NOT bundled with the `/clear` fix.
    #[cfg(test)]
    if FORCE_NO_SESSION_SERVER.with(|c| c.get()) {
        return None;
    }
    if std::env::var("YALDA_SESSION_SERVER").as_deref() == Ok("0") {
        eprintln!("[yalda-gpui] session server disabled (YALDA_SESSION_SERVER=0); direct spawn");
        return None;
    }
    match SessionServerClient::connect() {
        Ok(client) => {
            eprintln!("[yalda-gpui] connected to session server");
            Some(client)
        }
        Err(e) => {
            eprintln!(
                "[yalda-gpui] session server connect failed: {e}; falling back to direct spawn"
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
    //    supported in v1 — yalda is single-user.
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
    //    `.`/`..` collapsed (same pattern as `Frame::canonical_key`).
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

/// Path to the JSON file that maps cwd → workspace snapshot (workspaces + layout
/// tree). Companion to acp_sessions.json; cleared by clearing cache_dir.
pub(crate) fn workspace_persist_path() -> Option<PathBuf> {
    // Fail safe in test builds (same rationale as `preferences_path`): NEVER
    // touch the user's real `~/.yalda/workspace.json`. `save_workspace_state`
    // fires from ~75 action handlers (open / split / close / focus), so any
    // test that dispatches one of those actions would otherwise overwrite the
    // user's real workspace/split layout. Round-trip tests opt in via
    // `with_workspace_path`; everything else gets `None` → save is a no-op.
    #[cfg(test)]
    {
        return WS_PATH_OVERRIDE.with(|c| c.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|d| d.join("workspace.json"))
    }
}

/// Test-only seam mirroring `with_acp_persist_path` / `with_preferences_path`:
/// redirect the workspace snapshot file to a tempdir. Unset (the default) ⇒
/// `workspace_persist_path()` is `None` and `save_workspace_state` no-ops.
#[cfg(test)]
thread_local! {
    pub(crate) static WS_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn with_workspace_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    WS_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(path));
    let r = f();
    WS_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    r
}

/// Path to the JSON file holding app-managed runtime preferences (theme
/// choice, eventually other "View" menu state). Kept separate from the
/// user-edited `~/.config/yalda/config.kdl` so the menu-driven theme
/// switcher doesn't have to rewrite a hand-curated config file. On launch
/// preferences override the config's theme — if the user picked a theme
/// from the menu, that's what they expect next time, regardless of what
/// the kdl says.
pub(crate) fn preferences_path() -> Option<PathBuf> {
    // Fail safe in test builds: NEVER touch the user's real
    // `~/.yalda/preferences.json`. A test that genuinely exercises
    // preference persistence opts in via `with_preferences_path`; every other
    // test that merely triggers `save_settings()` as a side-effect (e.g. a
    // theme/zoom render-cache test calling `set_theme`) gets `None`, so the
    // write is a no-op instead of clobbering real user preferences. Mirrors
    // `acp_session_persist_path`'s `ACP_PERSIST_PATH_OVERRIDE` seam.
    #[cfg(test)]
    {
        return PREFS_PATH_OVERRIDE.with(|c| c.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|d| d.join("preferences.json"))
    }
}

/// Where user keybinding overrides are stored (the rebindable-keymap tile,
/// `keymap_registry.rs`). Only the diffs from the default table are written.
/// Same fail-safe seam as [`preferences_path`]: under `cfg(test)` this returns
/// `None` unless a test opts in via [`with_keymap_overrides_path`], so a rebind
/// exercised in a headless test never clobbers the user's real overrides.
pub(crate) fn keymap_overrides_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return KEYMAP_OVERRIDES_PATH_OVERRIDE.with(|c| c.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|d| d.join("keymap-overrides.json"))
    }
}

/// Test-only seam mirroring [`PREFS_PATH_OVERRIDE`] for keymap overrides.
#[cfg(test)]
thread_local! {
    pub(crate) static KEYMAP_OVERRIDES_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn with_keymap_overrides_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    KEYMAP_OVERRIDES_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(path));
    let r = f();
    KEYMAP_OVERRIDES_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    r
}

/// Test-only seam: redirect the preferences file to a tempdir so a save→load
/// round-trip test never touches the user's real `~/.yalda/preferences.json`.
/// When unset (the default in any test), `preferences_path()` returns `None`
/// and a `save_preferences` call is a silent no-op. Thread-local, so parallel
/// tests don't collide.
#[cfg(test)]
thread_local! {
    pub(crate) static PREFS_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn with_preferences_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    PREFS_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(path));
    let r = f();
    PREFS_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    r
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Preferences {
    /// Kebab-case theme identifier — `ThemeName::as_kebab()` /
    /// `ThemeName::parse()`. `None` means "no app-managed override; use
    /// the value from config.kdl (or the built-in default)."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) theme: Option<String>,
    /// Document text-zoom factor (`Cmd-=`/`Cmd--`/`Cmd-0`). `None` means "no
    /// saved zoom; start at 1.0." Clamped to `[MIN_TEXT_SCALE, MAX_TEXT_SCALE]`
    /// on load so a hand-edited file can't push the body off-screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text_scale: Option<f32>,
    /// Last outer-window size in logical pixels. Position and maximized/fullscreen
    /// state are intentionally not persisted; the OS remains responsible for
    /// placing the restored window on an available display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) window_width_px: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) window_height_px: Option<f32>,
    /// Default span assigned to new desktop tiles, in fixed cells
    /// (spec-desktop-mode.md Behavior 6). Existing tile spans are unaffected.
    /// One global setting; clamped on use, not on load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) desktop_grid_cols: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) desktop_grid_rows: Option<u32>,
    /// Version of the built-in default tile span. Migrations update shipped
    /// defaults without overriding choices explicitly saved afterward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) desktop_grid_defaults_version: Option<u8>,
    /// Jump-panel visibility (jump-panel; `cmd-j` / `?` menu). `None` means
    /// "no saved preference; show it" (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_panel_visible: Option<bool>,
    /// User's drag-reordered order of jump-panel cwd group headers
    /// (jump-reorder). Ordered list of cwd display keys
    /// (`shorten_cwd_for_display`); groups render in this order, any group not
    /// listed sorts after them alphabetically. `None`/absent = alphabetical
    /// (the default). Rewritten wholesale on a cwd-header drop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_cwd_order: Option<Vec<String>>,
    /// User's drag-reordered order of jump-panel sessions WITHIN their cwd group
    /// (jump-reorder). Ordered list of server sids; within a cwd group sessions
    /// sort by their index here (unlisted sids sort after, keeping label order).
    /// A session never crosses cwd groups (the drop is cwd-gated), so one global
    /// list suffices. `None`/absent = by-label (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_session_order: Option<Vec<String>>,
    /// Local projection/cache of server session ids in durable cold archive.
    /// The WAL-backed server bit is authoritative; this preserves navigation
    /// before the first roster seed and migrates preferences from older builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_archived_sessions: Option<Vec<String>>,
    /// Folded jump-panel projects, keyed by durable project name (ProjectId is
    /// runtime-local). Absent means every project starts expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_folded_projects: Option<Vec<String>>,
    /// Folded workspace folders, keyed by a durable project/auto-name pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_folded_workspaces: Option<Vec<String>>,
    /// User's drag-reordered jump-panel WORKSPACE folder order
    /// (UXI-JumpPanel-29). Each entry is the same durable project/immutable
    /// auto-name composite used by `jump_folded_workspaces`. Reordering is
    /// project-gated; unlisted workspaces retain frame order after listed ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_workspace_order: Option<Vec<String>>,
    /// User's drag-reordered order of jump-panel tag folders, per project
    /// (UXI-JumpPanel-21). `project name → [tag]`; folders render in this order,
    /// any tag not listed sorts after alphabetically. Tags are project-scoped, so
    /// the order is keyed by durable project name (like `jump_folded_projects`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_tag_order: Option<std::collections::HashMap<String, Vec<String>>>,
    /// Folded jump-panel tag folders, keyed by `"{project}\u{1f}{tag}"` composite
    /// (UXI-JumpPanel-21). Absent means every folder starts expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_folded_tags: Option<Vec<String>>,
    /// User's drag-reordered order of jump-panel TILE rows within a workspace
    /// folder (UXI-JumpPanel-28). One global ordered list of durable `WindowId`s;
    /// within each workspace folder tiles sort by their index here, any tile not
    /// listed sorts after in layout-traversal order. A tile drag is folder-gated,
    /// so one global list suffices. `None`/absent = layout order (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_tile_order: Option<Vec<workspace::WindowId>>,
    /// User's drag-reordered order of Detached tile rows (UXI-JumpPanel-28).
    /// Kept separate from `jump_tile_order` so rebuilding the complete attached
    /// order cannot erase the Detached presentation order (and vice versa).
    /// Drops are project/tag-group gated; one global durable `WindowId` rank is
    /// sufficient. `None`/absent = alphabetical order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jump_detached_tile_order: Option<Vec<workspace::WindowId>>,
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

// ── Projects (ADR-0028, docs/components/project.md, UXI-Project-8) ───────────

/// Path to the JSON file holding the Project registry (ADR-0028). **Global**,
/// not cwd-keyed — projects are the top of the hierarchy, so one file holds them
/// all. Same fail-safe seam as [`preferences_path`]: under `cfg(test)` returns
/// `None` unless a test opts in via [`with_projects_path`], so a headless test
/// never touches the user's real `~/.yalda/projects.json`.
pub(crate) fn projects_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return PROJECTS_PATH_OVERRIDE.with(|c| c.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|d| d.join("projects.json"))
    }
}

/// Test-only seam mirroring [`PREFS_PATH_OVERRIDE`] for the projects file.
#[cfg(test)]
thread_local! {
    pub(crate) static PROJECTS_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn with_projects_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    PROJECTS_PATH_OVERRIDE.with(|c| *c.borrow_mut() = Some(path));
    let r = f();
    PROJECTS_PATH_OVERRIDE.with(|c| *c.borrow_mut() = None);
    r
}

/// On-disk shadow of one [`Project`]. `params` defaults empty so a file written
/// before a future key existed loads cleanly; serde ignores unknown fields, so a
/// *newer* file never resets the store (the migration discipline of
/// `UXI-Workspace-7`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedProject {
    pub(crate) name: String,
    pub(crate) cwd: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) params: std::collections::BTreeMap<String, String>,
}

/// The `projects.json` root.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedProjects {
    #[serde(default)]
    pub(crate) projects: Vec<PersistedProject>,
}

/// Load the persisted project registry, or `None` when the file is absent — the
/// signal that triggers a one-time cwd→project migration ([`migrate_cwds_to_projects`]).
pub(crate) fn load_persisted_projects() -> Option<PersistedProjects> {
    let path = projects_path()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Best-effort write of the live [`Projects`] store. No-ops on any failure.
pub(crate) fn save_persisted_projects(projects: &Projects) {
    let Some(path) = projects_path() else {
        return;
    };
    let doc = PersistedProjects {
        projects: projects
            .iter()
            .map(|(_, p)| PersistedProject {
                name: p.name.clone(),
                cwd: p.cwd.display().to_string(),
                params: p.params.clone(),
            })
            .collect(),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Rebuild a live [`Projects`] store from a persisted doc. Tolerant of a
/// malformed/hand-edited file: a duplicate **name** folds to the existing
/// project, a duplicate **cwd** folds to whatever roots there — the loader never
/// panics and never drops a record's params.
pub(crate) fn projects_from_persisted(doc: &PersistedProjects) -> Projects {
    let mut ps = Projects::new();
    for pp in &doc.projects {
        let cwd = PathBuf::from(&pp.cwd);
        let id = match ps.create(pp.name.clone(), cwd.clone()) {
            Ok(id) => id,
            // Fold a corrupt duplicate onto the existing project rather than fail.
            Err(CreateError::DuplicateName) => match ps.by_name(&pp.name) {
                Some(id) => id,
                None => continue,
            },
            Err(CreateError::DuplicateCwd(id)) => id,
        };
        if let Some(p) = ps.get_mut(id) {
            p.params = pp.params.clone();
        }
    }
    ps
}

/// The project name a cwd migrates to (ADR-0028 §7, `UXI-Project-8`): the last
/// path component with its first letter capitalized. Basename `yaldabaoth` →
/// `Yaldabaoth`, `fulcrum` → `Fulcrum` — the user's two named projects fall out
/// of this general rule, and any other cwd gets its own basename-derived name.
/// Total: an empty/rootless path falls back to `"Project"`.
pub(crate) fn project_name_for_cwd(cwd: &std::path::Path) -> String {
    let base = cwd
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Project".to_string());
    let mut chars = base.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Project".to_string(),
    }
}

/// Migrate the cwds found across persisted workspaces + sessions into a named
/// [`Projects`] store (ADR-0028 §7). Each **distinct canonical** cwd becomes a
/// project named by [`project_name_for_cwd`]; a name clash folds via
/// `get_or_create` (a documented limitation — two distinct dirs sharing a
/// basename share a project; not expected in practice, where live state is under
/// the two known cwds). **Total + panic-proof**: every cwd yields a project and
/// nothing is dropped, so an old snapshot with no `projects.json` never loses
/// data.
/// Build the Project registry at boot (ADR-0028 "projects before workspace"):
/// load `projects.json` if present, else migrate from `seed_cwds` (the cwds found
/// across persisted workspaces + sessions). Then ensure a project exists at
/// `primary` (the root workspace's cwd) and return the store plus that project's
/// id, persisting the (possibly newly-migrated) store. Under `cfg(test)` the
/// load/save no-op (path seam), so a boot migrates purely in memory.
pub(crate) fn boot_projects(
    primary: &std::path::Path,
    seed_cwds: impl IntoIterator<Item = PathBuf>,
) -> (Projects, ProjectId) {
    let mut projects = match load_persisted_projects() {
        Some(doc) if !doc.projects.is_empty() => projects_from_persisted(&doc),
        _ => migrate_cwds_to_projects(seed_cwds),
    };
    let pid = projects.ensure_at_cwd(primary.to_path_buf(), &project_name_for_cwd(primary));
    save_persisted_projects(&projects);
    (projects, pid)
}

pub(crate) fn migrate_cwds_to_projects(cwds: impl IntoIterator<Item = PathBuf>) -> Projects {
    let mut ps = Projects::new();
    for cwd in cwds {
        // `ensure_at_cwd` dedups on the canonical key (so `/tmp` vs `/private/tmp`
        // don't split) and uniquifies the basename-derived name on a clash, so
        // both uniqueness invariants stay total.
        let name = project_name_for_cwd(&cwd);
        ps.ensure_at_cwd(cwd, &name);
    }
    ps
}

/// Serializable shadow of `App` for spec-tiles-and-apps.md (ADR-0019).
///
/// The tag set is `{buffer{mode}, agent}` (was `{doc, edit, browser, claude}`).
/// A `Buffer` tile persists its mode (`viewing`/`editing`/`picking`) plus the
/// payload that mode needs: a file `path` for viewing/editing, a `dir` for
/// picking. An `Agent` tile persists its session_id (or `None` if not yet
/// attached). Window-local view state (scroll, cursor) and the `underlying`
/// stashes are intentionally NOT persisted — no on-disk layout can encode an
/// agent-behind-picker (B7). There is no schema version field: the load path
/// (`restore_kind`) already discards an entry that fails to deserialize via
/// `serde_json::from_value(...).ok()` and falls back to defaults, so a stale
/// `workspace.json` from an older build silently re-opens at defaults.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub(crate) enum PersistedKind {
    /// A Buffer tile in one of its three modes.
    Buffer { mode: PersistedBufferMode },
    /// JSON tag stays as "claude" so the ACP-session side-channel keys line up;
    /// the in-memory variant is `Agent` to match the rename pass.
    /// `session_id` is a `ServerSid`, but its `#[serde(transparent)]` makes the
    /// on-disk shape a bare string — byte-identical to the pre-newtype
    /// `Option<String>`, so old `workspace.json` files load and new ones don't
    /// change shape.
    #[serde(rename = "claude")]
    Agent { session_id: Option<ServerSid> },
    /// A Linear tile. The loaded issue/project isn't persisted — restore opens
    /// an empty Linear tile and the user re-enters the identifier (the data is
    /// remote and cheap to re-fetch).
    #[serde(rename = "linear")]
    Linear {},
    /// A Cog explorer tile. Only semantic navigation/display state is stored;
    /// every remote payload is re-fetched on restore. `state` defaults so the
    /// historical `{ "kind": "cog", "data": {} }` shape stays readable.
    #[serde(rename = "cog")]
    Cog {
        #[serde(default, skip_serializing_if = "CogRemembered::is_default")]
        state: CogRemembered,
    },
    /// The keybindings reference tile — stateless (it reads the live registry),
    /// so restore just opens a fresh one.
    #[serde(rename = "keymap")]
    Keymap {},
    /// The global Agent Stats singleton. Its tab and snapshots are transient;
    /// persistence restores one openable shell tile and refreshes live data.
    #[serde(rename = "agent_stats")]
    AgentStats {},
}

/// Persisted shadow of `BufferApp`'s mode (B1). `viewing`/`editing` carry the
/// file path; `picking` carries the browser's current dir.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub(crate) enum PersistedBufferMode {
    Viewing { path: PathBuf },
    Editing { path: PathBuf },
    Picking { dir: PathBuf },
}

/// One leaf in a persisted layout. Carries the (stable) window id so
/// `focused_window` references survive restore.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedLeaf {
    pub(crate) id: workspace::WindowId,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub(crate) tags: workspace::TagSet,
    #[serde(flatten)]
    pub(crate) kind: PersistedKind,
}

/// One tile outside every workspace (ADR-0033). Project is persisted by cwd,
/// matching workspace membership's self-healing project migration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedDetachedTile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) project_cwd: Option<String>,
    pub(crate) tile: PersistedLeaf,
}

/// One tile that remains attached to a workspace but is excluded from its
/// visible layout (ADR-0034). The footprint is a best-effort restoration hint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedHiddenTile {
    pub(crate) tile: PersistedLeaf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) previous_placement: Option<PersistedPlacement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedPlacement {
    pub(crate) row: i32,
    pub(crate) col: i32,
    pub(crate) rows: u32,
    pub(crate) cols: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "tile")]
pub(crate) enum PersistedSoloPresentation {
    Detached(workspace::WindowId),
    HiddenAttached(workspace::WindowId),
}

/// Reserve one restored tile's stable identities. Rejects duplicate
/// WindowIds and duplicate durable Agent sids, rolling back the id reservation
/// when the sid conflicts.
pub(crate) fn accept_tile_restore(
    id: workspace::WindowId,
    agent_sid: Option<&ServerSid>,
    placed_ids: &mut std::collections::HashSet<workspace::WindowId>,
    placed_agent_sids: &mut std::collections::HashSet<String>,
) -> bool {
    if !placed_ids.insert(id) {
        return false;
    }
    if let Some(sid) = agent_sid
        && !placed_agent_sids.insert(sid.to_string())
    {
        placed_ids.remove(&id);
        return false;
    }
    true
}

/// The durable identity reserved for one accepted persisted tile. Keeping the
/// Agent case typed prevents restore callers from separately (and
/// inconsistently) deciding whether a leaf participates in the global
/// one-session/one-tile invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistedTileIdentity {
    NonAgent,
    Agent(ServerSid),
}

/// Atomically classify and reserve a persisted leaf. `None` means either its
/// stable WindowId or its durable Agent sid is already owned elsewhere.
pub(crate) fn reserve_persisted_leaf(
    leaf: &PersistedLeaf,
    placed_ids: &mut std::collections::HashSet<workspace::WindowId>,
    placed_agent_sids: &mut std::collections::HashSet<String>,
) -> Option<PersistedTileIdentity> {
    let identity = match &leaf.kind {
        PersistedKind::Agent {
            session_id: Some(sid),
        } => PersistedTileIdentity::Agent(sid.clone()),
        _ => PersistedTileIdentity::NonAgent,
    };
    let sid = match &identity {
        PersistedTileIdentity::Agent(sid) => Some(sid),
        PersistedTileIdentity::NonAgent => None,
    };
    accept_tile_restore(leaf.id, sid, placed_ids, placed_agent_sids).then_some(identity)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistedLayout {
    Empty,
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

/// Persisted per-workspace rail (spec-rail.md §14). Optional on `PersistedWorkspace` so
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
    /// overridden by the workspace's focused_window on restore).
    #[serde(default)]
    pub(crate) pinned_to: workspace::WindowId,
}

pub(crate) fn default_rail_width() -> f32 {
    workspace::RAIL_DEFAULT_WIDTH
}

/// Persisted shadow of a plane's [`workspace::Camera`]
/// (`spec-infinite-plane-workspace.md` D4): pan (slot units) + semantic-zoom
/// Detail. `zoom` deserializes through `Detail`'s HAND-ROLLED
/// `Deserialize` (workspace.rs), which falls back to `Full` on any unknown
/// string — so a `zoom` value from a newer binary DEGRADES the camera to
/// origin-detail rather than failing the parse and dropping the whole snapshot
/// (the loader's "failed parse ⇒ discard" rule). That fallback lives in
/// `Detail`, so a plain `#[derive(Deserialize)]` here is safe: it can never
/// hard-error on the zoom field. `pan` is a pair of `f32`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedCamera {
    pub(crate) pan: (f32, f32),
    pub(crate) zoom: workspace::Detail,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedWorkspace {
    pub(crate) auto_name: String,
    pub(crate) display_name: Option<String>,
    pub(crate) focused_window: workspace::WindowId,
    /// Attached tiles excluded from the visible layout. Absent in snapshots
    /// written before ADR-0034, where every attached tile was visible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) hidden_tiles: Vec<PersistedHiddenTile>,
    pub(crate) layout: PersistedLayout,
    /// Optional rail (spec-rail.md §14). Absent in old snapshots → no rail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rail: Option<PersistedRail>,
    // Layout patterns: per-workspace layout mode + primary-stack params
    #[serde(default)]
    pub(crate) layout_mode: workspace::LayoutMode,
    // `alias` keeps pre-rename snapshots (which wrote `master_ratio`/
    // `master_count`) loading after the master→primary rename.
    #[serde(default = "default_primary_ratio", alias = "master_ratio")]
    pub(crate) primary_ratio: f32,
    #[serde(default = "default_primary_count", alias = "master_count")]
    pub(crate) primary_count: usize,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub(crate) tag_view: std::collections::BTreeSet<String>,
    /// Desktop-mode slot assignments (spec-desktop-mode.md Behavior 7),
    /// keyed by the same stable per-leaf `WindowId` the layout snapshot
    /// uses — NOT positional, so a mismatched entry degrades to
    /// reconciliation instead of scrambling the arrangement. Absent in old
    /// snapshots → seed on the first desktop render. `(id, row, col)`; the
    /// row/col are SIGNED (`i32`) on the infinite plane
    /// (`spec-infinite-plane-workspace.md` D4) — old snapshots stored
    /// non-negative `u32` values, which deserialize as the same positive
    /// signed coordinates (the old top-right quadrant), so they load
    /// transparently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) desktop_slots: Vec<(workspace::WindowId, i32, i32)>,
    /// Desktop tile spans (spec-desktop-mode.md Behavior 4b), keyed by the
    /// same `WindowId`. Parallel to `desktop_slots` and holds only non-default
    /// (≠ 1 × 1) tiles, so older snapshots (no field) load every tile at
    /// 1 × 1 and span-free arrangements omit it entirely. `(id, rows, cols)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) desktop_spans: Vec<(workspace::WindowId, u32, u32)>,
    /// The plane's persisted camera (`spec-infinite-plane-workspace.md`
    /// Behavior 7 / D4): pan + semantic-zoom Detail, so a workspace reopens
    /// exactly where the view was left. Absent in old snapshots → restored as
    /// `Camera::default()` (origin, `Full`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) camera: Option<PersistedCamera>,
    /// The workspace's tile arrangement (`UXI-Workspace-14`): `"columns"` (default)
    /// or `"plane"`. Absent in old snapshots → `Columns`; an unknown value from a
    /// newer binary degrades to `Columns` (the hand-rolled `WorkspaceView`
    /// deserialize) rather than dropping the whole snapshot.
    #[serde(default)]
    pub(crate) view: workspace::WorkspaceView,
    /// The workspace's working directory (ADR-0023). `None` in old snapshots
    /// (pre-typed-cwd) → migrated from `legacy_kv["cwd"]` on restore, else the
    /// process dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<String>,
    /// Legacy per-workspace registry (the old `kv` that held `"cwd"`). Read on
    /// restore for back-compat with pre-ADR-0023 snapshots; never written again
    /// (cwd is now a typed field), so it disappears from snapshots over time.
    #[serde(default, rename = "kv", skip_serializing)]
    pub(crate) legacy_kv: HashMap<String, String>,
}

pub(crate) fn default_primary_ratio() -> f32 {
    0.6
}
pub(crate) fn default_primary_count() -> usize {
    1
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedFrame {
    // On-disk keys stay `tabs`/`active_tab` (pre-T007 format) so existing
    // `workspace.json` files keep loading after the Tab→Workspace rename. The
    // Rust field names are the new vocabulary; serde bridges to the old keys.
    #[serde(rename = "tabs")]
    pub(crate) workspaces: Vec<PersistedWorkspace>,
    #[serde(rename = "active_tab")]
    pub(crate) active_workspace: usize,
    /// Stable tiles outside every workspace. Absent in legacy snapshots.
    #[serde(
        default,
        alias = "unbound_tiles",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(crate) detached_tiles: Vec<PersistedDetachedTile>,
    /// Temporary presentation of a tile whose normal owner does not paint it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) solo_presentation: Option<PersistedSoloPresentation>,
    /// Legacy direct-Unbound focus; restored only when it names a Detached tile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) direct_unbound: Option<workspace::WindowId>,
    /// Legacy scratchpad MRU, retained only for additive deserialization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) scratchpad: Vec<workspace::WindowId>,
    /// False/missing means import the legacy session/buffer tag sidecars into
    /// tile-local tags once. New snapshots always write true.
    #[serde(default)]
    pub(crate) tile_tags_migrated: bool,
    // Layout patterns: workspace-global marks
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) marks: HashMap<char, workspace::WindowId>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) tag_shortcuts: HashMap<char, String>,
    // Buffer-level tags (keyed by canonical path string)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) buffer_tags: HashMap<String, Vec<String>>,
}

/// Typed account of corruption removed before a persisted frame can become a
/// live ownership graph.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PersistedAgentOwnershipRepair {
    pub(crate) cleared_attached_duplicates: usize,
    pub(crate) removed_detached_duplicates: usize,
}

impl PersistedAgentOwnershipRepair {
    pub(crate) fn changed(self) -> bool {
        self.cleared_attached_duplicates != 0 || self.removed_detached_duplicates != 0
    }
}

#[derive(Debug)]
struct PersistedAgentCandidate {
    sid: String,
    id: workspace::WindowId,
    project_cwd: PathBuf,
    attached: bool,
    order: usize,
}

fn collect_persisted_agent_candidates(
    layout: &PersistedLayout,
    project_cwd: &std::path::Path,
    order: &mut usize,
    out: &mut Vec<PersistedAgentCandidate>,
) {
    match layout {
        PersistedLayout::Empty => {}
        PersistedLayout::Leaf(PersistedLeaf {
            id,
            kind: PersistedKind::Agent {
                session_id: Some(sid),
            },
            ..
        }) if !sid.as_str().is_empty() => {
            out.push(PersistedAgentCandidate {
                sid: sid.to_string(),
                id: *id,
                project_cwd: project_cwd.to_path_buf(),
                attached: true,
                order: *order,
            });
            *order += 1;
        }
        PersistedLayout::Leaf(_) => *order += 1,
        PersistedLayout::Split { children, .. } => {
            for (_, child) in children {
                collect_persisted_agent_candidates(child, project_cwd, order, out);
            }
        }
    }
}

fn clear_noncanonical_bound_agent_sids(
    layout: &mut PersistedLayout,
    canonical: &HashMap<String, usize>,
    order: &mut usize,
) -> usize {
    match layout {
        PersistedLayout::Empty => 0,
        PersistedLayout::Leaf(PersistedLeaf {
            kind: PersistedKind::Agent { session_id },
            ..
        }) => {
            let candidate_order = *order;
            *order += 1;
            let duplicate = session_id.as_ref().is_some_and(|sid| {
                canonical
                    .get(sid.as_str())
                    .is_some_and(|canonical_order| *canonical_order != candidate_order)
            });
            if duplicate {
                *session_id = None;
                1
            } else {
                0
            }
        }
        PersistedLayout::Leaf(_) => {
            *order += 1;
            0
        }
        PersistedLayout::Split { children, .. } => children
            .iter_mut()
            .map(|(_, child)| clear_noncanonical_bound_agent_sids(child, canonical, order))
            .sum(),
    }
}
fn collect_hidden_agent_candidate(
    hidden: &PersistedHiddenTile,
    project_cwd: &std::path::Path,
    order: usize,
    out: &mut Vec<PersistedAgentCandidate>,
) {
    if let PersistedKind::Agent {
        session_id: Some(sid),
    } = &hidden.tile.kind
        && !sid.as_str().is_empty()
    {
        out.push(PersistedAgentCandidate {
            sid: sid.to_string(),
            id: hidden.tile.id,
            project_cwd: project_cwd.to_path_buf(),
            attached: true,
            order,
        });
    }
}

fn clear_noncanonical_hidden_agent_sid(
    hidden: &mut PersistedHiddenTile,
    canonical: &HashMap<String, usize>,
    order: usize,
) -> usize {
    let PersistedKind::Agent { session_id } = &mut hidden.tile.kind else {
        return 0;
    };
    let duplicate = session_id.as_ref().is_some_and(|sid| {
        canonical
            .get(sid.as_str())
            .is_some_and(|canonical_order| *canonical_order != order)
    });
    if duplicate {
        *session_id = None;
        1
    } else {
        0
    }
}

/// Heal duplicate durable Agent identities before constructing live tiles.
/// Session cwd is authoritative for project membership; within that project a
/// attached tile wins over a Detached tile, then stable id/order break ties.
pub(crate) fn heal_persisted_agent_ownership(
    frame: &mut PersistedFrame,
    authoritative_cwds: &HashMap<String, PathBuf>,
    fallback_cwd: &std::path::Path,
) -> PersistedAgentOwnershipRepair {
    let mut candidates = Vec::new();
    let mut order = 0;
    for persisted in &frame.workspaces {
        let project_cwd = persisted
            .cwd
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| persisted.legacy_kv.get("cwd").map(PathBuf::from))
            .unwrap_or_else(|| fallback_cwd.to_path_buf());
        collect_persisted_agent_candidates(
            &persisted.layout,
            &project_cwd,
            &mut order,
            &mut candidates,
        );
        for hidden in &persisted.hidden_tiles {
            collect_hidden_agent_candidate(hidden, &project_cwd, order, &mut candidates);
            order += 1;
        }
    }
    for persisted in &frame.detached_tiles {
        let PersistedKind::Agent {
            session_id: Some(sid),
        } = &persisted.tile.kind
        else {
            order += 1;
            continue;
        };
        if !sid.as_str().is_empty() {
            candidates.push(PersistedAgentCandidate {
                sid: sid.to_string(),
                id: persisted.tile.id,
                project_cwd: persisted
                    .project_cwd
                    .as_deref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| fallback_cwd.to_path_buf()),
                attached: false,
                order,
            });
        }
        order += 1;
    }

    let mut canonical: HashMap<String, usize> = HashMap::new();
    let mut rank: HashMap<String, (u8, u8, workspace::WindowId, usize)> = HashMap::new();
    for candidate in candidates {
        let correct_project = authoritative_cwds
            .get(&candidate.sid)
            .is_some_and(|cwd| cwd_match_key(cwd) == cwd_match_key(&candidate.project_cwd));
        let candidate_rank = (
            u8::from(!correct_project),
            u8::from(!candidate.attached),
            candidate.id,
            candidate.order,
        );
        if rank
            .get(&candidate.sid)
            .is_none_or(|current| candidate_rank < *current)
        {
            rank.insert(candidate.sid.clone(), candidate_rank);
            canonical.insert(candidate.sid, candidate.order);
        }
    }

    let mut repair = PersistedAgentOwnershipRepair::default();
    let mut repair_order = 0;
    for persisted in &mut frame.workspaces {
        repair.cleared_attached_duplicates += clear_noncanonical_bound_agent_sids(
            &mut persisted.layout,
            &canonical,
            &mut repair_order,
        );
        for hidden in &mut persisted.hidden_tiles {
            repair.cleared_attached_duplicates +=
                clear_noncanonical_hidden_agent_sid(hidden, &canonical, repair_order);
            repair_order += 1;
        }
    }
    frame.detached_tiles.retain(|persisted| {
        let candidate_order = repair_order;
        repair_order += 1;
        let keep = match &persisted.tile.kind {
            PersistedKind::Agent {
                session_id: Some(sid),
            } if !sid.as_str().is_empty() => canonical
                .get(sid.as_str())
                .is_none_or(|canonical_order| *canonical_order == candidate_order),
            _ => true,
        };
        if !keep {
            repair.removed_detached_duplicates += 1;
        }
        keep
    });
    repair
}

/// Resolve a bound tile's local `SessionId` to its durable server id — the store's
/// `sid_of`, passed in so the (cx-free) snapshot has the SINGLE source of truth for
/// which session occupies a tile (ADR-0026: no `resume_sid` cache to drift).
pub(crate) type SidResolver<'a> = &'a dyn Fn(SessionId) -> Option<ServerSid>;

/// Snapshot a live `App` into its persisted shadow. Returns `None`
/// for content kinds that aren't worth persisting (e.g., an unattached
/// transient state we'd lose nothing by skipping).
pub(crate) fn snapshot_content(content: &App, resolve: SidResolver) -> PersistedKind {
    match content {
        App::Buffer(BufferApp::Viewing(d)) => PersistedKind::Buffer {
            mode: PersistedBufferMode::Viewing {
                path: PathBuf::from(d.file_label.as_ref()),
            },
        },
        App::Buffer(BufferApp::Editing(e)) => PersistedKind::Buffer {
            mode: PersistedBufferMode::Editing {
                path: PathBuf::from(e.file_label.as_ref()),
            },
        },
        App::Buffer(BufferApp::Picking(b)) => PersistedKind::Buffer {
            mode: PersistedBufferMode::Picking {
                dir: b.fb.current_dir().to_path_buf(),
            },
        },
        App::Agent(tile) => {
            // Persist WHICH session occupies this tile (identity), so restore
            // rebinds each tile to its OWN session (UXI-AgentTile-18) instead of
            // zipping sessions to tiles by index. The id is resolved from the store
            // via `resolve` (single source of truth — a `Bound` tile has no cached
            // copy). `None` (Selecting, or the store lacks a sid) ⇒ restore shows
            // the selector for that tile.
            PersistedKind::Agent {
                session_id: tile.remembered_sid(resolve),
            }
        }
        App::Linear(_tile) => PersistedKind::Linear {},
        App::Cog(tile) => PersistedKind::Cog {
            state: tile.remembered(),
        },
        App::Keymap(_tile) => PersistedKind::Keymap {},
        App::AgentStats => PersistedKind::AgentStats {},
    }
}

/// Snapshot a live `Layout<App>` into its persisted shadow.
pub(crate) fn snapshot_layout(
    layout: &workspace::Layout<App>,
    resolve: SidResolver,
) -> PersistedLayout {
    match layout {
        workspace::Layout::Empty => PersistedLayout::Empty,
        workspace::Layout::Leaf(win) => PersistedLayout::Leaf(PersistedLeaf {
            id: win.id(),
            tags: win.tags.clone(),
            kind: snapshot_content(&win.content, resolve),
        }),
        workspace::Layout::Split { dir, children } => PersistedLayout::Split {
            dir: *dir,
            children: children
                .iter()
                .map(|(w, c)| (*w, snapshot_layout(c, resolve)))
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
pub(crate) fn restore_rail(
    p: PersistedRail,
    fallback_pin: workspace::WindowId,
) -> workspace::RailState {
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

/// Reconstruct a persisted layout into live `App`, opening any
/// file-backed leaves through `ws`'s buffer pool so two restored views of the
/// same file share one core. Returns the live layout plus the max window id
/// seen (so the caller can advance the id allocator past restored ids).
/// Returns (layout, max_window_id, agent_leaves). Each agent leaf carries its
/// persisted session id (`Option<String>`) so restore rebinds each tile to its
/// OWN session by identity (UXI-AgentTile-18), not by index.
pub(crate) fn restore_layout(
    ws: &mut workspace::Frame<App>,
    theme: &Theme,
    layout: PersistedLayout,
    project: ProjectId,
) -> (
    workspace::Layout<App>,
    workspace::WindowId,
    Vec<(workspace::WindowId, Option<ServerSid>)>,
) {
    match layout {
        PersistedLayout::Empty => (workspace::Layout::Empty, 0, Vec::new()),
        PersistedLayout::Leaf(leaf) => {
            let (window, agent_sid) = restore_leaf(ws, theme, leaf, project);
            let id = window.id();
            let agents = agent_sid.map(|sid| vec![(id, sid)]).unwrap_or_default();
            (workspace::Layout::Leaf(window), id, agents)
        }
        PersistedLayout::Split { dir, children } => {
            let mut max_id: workspace::WindowId = 0;
            let mut agents = Vec::new();
            let mut restored_children = Vec::with_capacity(children.len());
            for (w, child) in children {
                let (sub, sub_max, sub_agents) = restore_layout(ws, theme, child, project);
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

/// Restore one persisted tile while preserving its stable id and tile-local
/// tags. Shared by workspace layouts and the Detached collection.
pub(crate) fn restore_leaf(
    ws: &mut workspace::Frame<App>,
    theme: &Theme,
    leaf: PersistedLeaf,
    project: ProjectId,
) -> (workspace::Window<App>, Option<Option<ServerSid>>) {
    let id = leaf.id;
    let tags = leaf.tags;
    let agent_sid = match &leaf.kind {
        PersistedKind::Agent { session_id } => Some(session_id.clone()),
        _ => None,
    };
    let content = restore_content(ws, theme, leaf.kind);
    let mut window = workspace::Window::new(id, project, content);
    window.tags = tags;
    (window, agent_sid)
}

pub(crate) fn restore_content(
    ws: &mut workspace::Frame<App>,
    theme: &Theme,
    kind: PersistedKind,
) -> App {
    match kind {
        PersistedKind::Buffer {
            mode: PersistedBufferMode::Viewing { path },
        } => {
            let label: SharedString = path.display().to_string().into();
            // 5c: restore the Doc bound to its pooled core (shared text/undo +
            // live tracking). Fall back to a Browser if the file vanished since
            // it was persisted (mirrors the Edit restore path).
            match ws.open_and_retain(&path) {
                Ok((id, core)) => {
                    let blocks =
                        render_with_wiki(&core.borrow().document().full_text(), theme, Some(&path));
                    App::Buffer(BufferApp::Viewing(DocState::viewing(
                        blocks,
                        label,
                        Some(DocSource::new(id, core)),
                    )))
                }
                Err(_) => App::Buffer(BufferApp::Picking(BrowserWindow::standalone(
                    path.parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from(".")),
                ))),
            }
        }
        PersistedKind::Buffer {
            mode: PersistedBufferMode::Editing { path },
        } => {
            let label: SharedString = path.display().to_string().into();
            // Restore through the pool — a second restored Edit view of the
            // same file binds to the same shared core.
            match ws.open_and_retain(&path) {
                Ok((id, core)) => App::Buffer(BufferApp::Editing(EditState::new(
                    SharedEditor::new(id, core),
                    label,
                    EditView::Code,
                ))),
                Err(_) => App::Buffer(BufferApp::Picking(BrowserWindow::standalone(
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                ))),
            }
        }
        PersistedKind::Buffer {
            mode: PersistedBufferMode::Picking { dir },
        } => {
            let dir = if dir.is_dir() {
                dir
            } else {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            };
            App::Buffer(BufferApp::Picking(BrowserWindow::standalone(dir)))
        }
        PersistedKind::Agent { .. } => {
            // Claude restore is its own subsystem (acp_sessions.json +
            // open_agent_inner). Replace with a Browser stub here so the
            // workspace survives; user can re-attach via the existing Claude
            // commands.
            App::Buffer(BufferApp::Picking(BrowserWindow::standalone(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            )))
        }
        PersistedKind::Linear {} => App::Linear(LinearTile::new()),
        PersistedKind::Cog { state } => App::Cog(CogTile::restored(state)),
        PersistedKind::Keymap {} => App::Keymap(KeymapTile::new()),
        PersistedKind::AgentStats {} => App::AgentStats,
    }
}

/// Snapshot a live workspace into a fully serializable shape.
pub(crate) fn snapshot_workspace(
    ws: &workspace::Frame<App>,
    projects: &Projects,
    resolve: SidResolver,
) -> PersistedFrame {
    // Ephemeral virtual workspaces (ADR-0021) are transient and never persisted;
    // they're always the last wsp, so filtering them keeps the remaining indices
    // contiguous. `active_workspace` is clamped into the surviving range so a restore
    // never points past the saved list.
    let non_ephemeral = ws.workspaces.iter().filter(|t| !t.ephemeral).count();
    PersistedFrame {
        workspaces: ws
            .workspaces
            .iter()
            .filter(|t| !t.ephemeral)
            .map(|t| PersistedWorkspace {
                auto_name: t.auto_name.clone(),
                display_name: t.display_name.clone(),
                focused_window: t.focused,
                hidden_tiles: t
                    .hidden_tiles
                    .iter()
                    .map(|hidden| PersistedHiddenTile {
                        tile: PersistedLeaf {
                            id: hidden.window.id(),
                            tags: hidden.window.tags.clone(),
                            kind: snapshot_content(&hidden.window.content, resolve),
                        },
                        previous_placement: hidden.previous_placement.map(|(slot, span)| {
                            PersistedPlacement {
                                row: slot.row,
                                col: slot.col,
                                rows: span.rows,
                                cols: span.cols,
                            }
                        }),
                    })
                    .collect(),
                layout: snapshot_layout(&t.layout, resolve),
                rail: t.rail.as_ref().map(snapshot_rail),
                layout_mode: t.layout_mode,
                primary_ratio: t.primary_ratio,
                primary_count: t.primary_count,
                tag_view: t.tag_view.clone(),
                desktop_slots: t
                    .desktop
                    .slots
                    .iter()
                    // Slots are signed on the plane (D4); persist row/col
                    // directly as `i32` — negative anchors round-trip.
                    .map(|&(id, s)| (id, s.row, s.col))
                    .collect(),
                desktop_spans: t
                    .desktop
                    .spans
                    .iter()
                    .map(|(&id, sp)| (id, sp.rows, sp.cols))
                    .collect(),
                // The plane's camera (pan + semantic-zoom Detail), so the view
                // reopens where it was left (D4 / Behavior 7).
                camera: Some(PersistedCamera {
                    pan: t.desktop.camera.pan,
                    zoom: t.desktop.camera.zoom,
                }),
                // The tile arrangement (UXI-Workspace-14), so a columns
                // workspace reopens in columns.
                view: t.view,
                // Persist the workspace's PROJECT cwd (ADR-0028): the cwd lives on
                // the project now, so we resolve `t.project()` through the store.
                // Restore re-points the workspace at whatever project roots this
                // cwd (self-heal); project *names* survive via `projects.json`.
                cwd: projects
                    .cwd_of(t.project())
                    .map(|p| p.display().to_string()),
                legacy_kv: HashMap::new(),
            })
            .collect(),
        active_workspace: ws.active_workspace.min(non_ephemeral.saturating_sub(1)),
        detached_tiles: ws
            .detached_tiles
            .iter()
            .map(|tile| PersistedDetachedTile {
                project_cwd: projects
                    .cwd_of(tile.project())
                    .map(|path| path.display().to_string()),
                tile: PersistedLeaf {
                    id: tile.window.id(),
                    tags: tile.window.tags.clone(),
                    kind: snapshot_content(&tile.window.content, resolve),
                },
            })
            .collect(),
        solo_presentation: ws.presented_tile().map(|presentation| match presentation {
            workspace::SoloPresentation::Detached(id) => PersistedSoloPresentation::Detached(id),
            workspace::SoloPresentation::HiddenAttached(id) => {
                PersistedSoloPresentation::HiddenAttached(id)
            }
        }),
        direct_unbound: ws.presented_detached_tile_id(),
        // ADR-0034 removed scratchpad membership. Keep the legacy field empty
        // for additive snapshot compatibility until the schema version retires it.
        scratchpad: Vec::new(),
        tile_tags_migrated: true,
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
pub(crate) fn save_persisted_workspace(
    cwd: &std::path::Path,
    ws: &workspace::Frame<App>,
    projects: &Projects,
    resolve: SidResolver,
) {
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
    let snap = snapshot_workspace(ws, projects, resolve);
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
pub(crate) fn load_persisted_workspace(cwd: &std::path::Path) -> Option<PersistedFrame> {
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
/// (Chatbox, false, false). Older yalda binaries reading newer files
/// silently drop the unknown keys (downgrade contract, §35).
/// `cwd` is a spec-agent-cwd.md §5 addition; `None` (absence in JSON)
/// resolves to the process cwd at restore time per §1.
#[derive(Debug, Clone)]
pub(crate) struct PersistedSlot {
    pub(crate) id: ServerSid,
    pub(crate) label: String,
    /// Backend identity for the restored tile (bug-0024). `None` means this is
    /// an older snapshot; restore may consult the authoritative server roster
    /// and the live roster reconciliation will fill it once startup completes.
    pub(crate) provider: Option<yalda::acp_channel::AgentProvider>,
    pub(crate) active: bool,
    pub(crate) mode: InputModeKind,
    pub(crate) tasklist_open: bool,
    pub(crate) subagents_open: bool,
    /// Force-hidden sidepanel (UXI-AgentTile-20). Missing key ⇒ `false` (shown),
    /// same migration as the §35 flags above.
    pub(crate) sidepanel_hidden: bool,
    pub(crate) cwd: Option<PathBuf>,
    /// The unsent compose draft (Model C — `design-c.md` §4.4). `None`/absent =
    /// no draft. Seeded into the compose buffer on restore so a draft survives an
    /// app restart (it already survives a reconnect, since replay rebuilds only
    /// the transcript, not the compose).
    pub(crate) compose_draft: Option<String>,
    /// The autonamer's summary (`UXI-AgentTile-27`). Absent (old file, or a
    /// session that never got one) => `None`. Persisted so the jump panel's
    /// italic summary line survives a restart — the naming call is one-shot per
    /// session and is never re-run to rebuild it.
    pub(crate) summary: Option<String>,
    /// Whether the session finished a turn the user hasn't looked at yet
    /// (`AgentState::unread`). Persisted so the jump panel's unread dot survives
    /// a restart — otherwise every restored session comes back read. Absent (old
    /// file, or a read session) => `false`, same downgrade contract as above.
    pub(crate) unread: bool,
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
            // Wire boundary: the legacy single-string id becomes a `ServerSid`.
            id: ServerSid::new(id),
            label: "claude-1".into(),
            provider: None,
            active: true,
            mode: InputModeKind::Chatbox,
            tasklist_open: false,
            subagents_open: false,
            sidepanel_hidden: false,
            cwd: None,
            compose_draft: None,
            summary: None,
            unread: false,
        }];
    }
    let Some(arr) = entry.as_array() else {
        return Vec::new();
    };
    let mut slots: Vec<PersistedSlot> = arr
        .iter()
        .filter_map(|v| {
            let obj = v.as_object()?;
            // Wire boundary: the persisted id string becomes a `ServerSid`.
            let id = ServerSid::new(obj.get("id")?.as_str()?);
            // A MISSING label loads as empty (not bare "claude") so the dedupe pass
            // below always assigns it a numbered `claude-N` — bug-0005 (two sessions
            // named "claude" after restore).
            let label = obj
                .get("label")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let provider = obj
                .get("provider")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok());
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
            // UXI-AgentTile-20: force-hidden sidepanel. Absent (old file) => false.
            let sidepanel_hidden = obj
                .get("sidepanel_hidden")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            // spec-agent-cwd.md §5: optional per-slot cwd. Absent (old
            // file, or pre-spec save) is loaded as None so the restore
            // path can fall back to process cwd per §1.
            let cwd = obj.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from);
            // Model C: the unsent compose draft. Absent (old file) => None.
            let compose_draft = obj
                .get("compose_draft")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
            // UXI-AgentTile-27: the autoname summary. Absent (old file) => None.
            let summary = obj
                .get("summary")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
            // Jump-panel unread dot. Absent (old file, or a read session) => false.
            let unread = obj.get("unread").and_then(|b| b.as_bool()).unwrap_or(false);
            Some(PersistedSlot {
                id,
                label,
                provider,
                active,
                mode,
                tasklist_open,
                subagents_open,
                sidepanel_hidden,
                cwd,
                compose_draft,
                summary,
                unread,
            })
        })
        .collect();
    // bug-0005: no two restored sessions in one cwd may share a label. A
    // missing/empty (loaded as "") or duplicate label is reassigned to the next
    // free `claude-N`; valid distinct labels are left untouched.
    dedupe_slot_labels(&mut slots);
    slots
}

/// Return a unique session label given the names already in use: `desired` verbatim
/// when it is non-empty and free, otherwise the smallest free `claude-N` (bug-0005).
/// Does NOT mutate `used` — the caller records the result.
pub(crate) fn unique_label(desired: &str, used: &std::collections::HashSet<String>) -> String {
    if !desired.trim().is_empty() && !used.contains(desired) {
        return desired.to_string();
    }
    (1..)
        .map(|n| format!("claude-{n}"))
        .find(|l| !used.contains(l))
        .expect("infinite range always yields a free label")
}

/// Ensure every slot's `label` is unique within the cwd (bug-0005 — "two sessions
/// named 'claude' after restore"). Processed in ring order: the FIRST occurrence of
/// a valid label keeps it; an empty or already-seen label is reassigned to the
/// smallest free `claude-N`. A no-op when the labels are already distinct + non-empty.
pub(crate) fn dedupe_slot_labels(slots: &mut [PersistedSlot]) {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in slots.iter_mut() {
        s.label = unique_label(&s.label, &used);
        used.insert(s.label.clone());
    }
}

/// Classify an attach error string as the PERMANENT "session is gone" case.
/// The session-server actor returns `no such session: <id>` for a lookup miss
/// (every `.ok_or_else(|| format!("no such session: {session_id}"))` site in
/// `yalda-session-server/main.rs`). That means the persisted id outlived the
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
/// holds an Agent ring to re-save, and a stale id in a non-active workspace is never
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

/// Persist the ring's slots for `cwd` so the next yalda run can resume
/// every session in the ring, not just the active one. Best-effort writes
/// — failures (no cache dir, permissions, malformed prior file) silently
/// bail. Per-slot id resolution honors the resume_id stability rule: if a
/// slot was restored with a `resume_id`, that id is what gets persisted
/// (even if `session/load` failed and the slot fell back to a fresh
/// `session/new`). Slots without an id (pending attach or attach failed
/// outright) are skipped.
///
/// Concurrent yalda instances on the same `cwd`: last-writer-wins. Each
/// call does a read-modify-write of the file, replacing only the cwd
/// entry; other cwds are preserved.
/// A persistable snapshot of one bound agent session (spec-agent-session-
/// ownership.md). Gathered by `YaldaGpuiView::save_agent_ring` from the tiles'
/// bound sessions in the store, then written by `save_persisted_acp_sessions`.
pub(crate) struct SessionSnapshot {
    pub(crate) id: ServerSid,
    pub(crate) label: String,
    /// Backend identity is tile-visible state, so it must survive the interval
    /// between synchronous restore and the async roster seed (bug-0024).
    pub(crate) provider: yalda::acp_channel::AgentProvider,
    pub(crate) active: bool,
    pub(crate) mode: InputModeKind,
    pub(crate) tasklist_open: bool,
    pub(crate) subagents_open: bool,
    /// Force-hidden sidepanel (UXI-AgentTile-20).
    pub(crate) sidepanel_hidden: bool,
    pub(crate) cwd: PathBuf,
    /// The unsent compose draft (Model C — `design-c.md` §4.4). `None`/empty is
    /// not written; a non-empty draft round-trips through `compose_draft`.
    pub(crate) compose_draft: Option<String>,
    /// The autonamer's summary (`UXI-AgentTile-27`); `None`/empty is not written.
    pub(crate) summary: Option<String>,
    /// Jump-panel unread state (`AgentState::unread`). Only written when true,
    /// same downgrade contract as `sidepanel_hidden`.
    pub(crate) unread: bool,
}

pub(crate) fn save_persisted_acp_sessions(cwd: &std::path::Path, snaps: &[SessionSnapshot]) {
    let Some(path) = acp_session_persist_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let entries: Vec<serde_json::Value> = snaps
        .iter()
        .map(|snap| {
            let mut obj = serde_json::Map::new();
            // Wire boundary: write the `ServerSid` back out as a bare string.
            obj.insert("id".into(), serde_json::Value::String(snap.id.to_string()));
            obj.insert(
                "label".into(),
                serde_json::Value::String(snap.label.clone()),
            );
            obj.insert(
                "provider".into(),
                serde_json::Value::String(
                    match snap.provider {
                        yalda::acp_channel::AgentProvider::Claude => "claude",
                        yalda::acp_channel::AgentProvider::Codex => "codex",
                    }
                    .to_string(),
                ),
            );
            if snap.active {
                obj.insert("active".into(), serde_json::Value::Bool(true));
            }
            // Spec §35: persist input mode and sidebar state per session.
            let mode_str = match snap.mode {
                InputModeKind::Worksheet => "worksheet",
                InputModeKind::Chatbox => "chatbox",
            };
            obj.insert(
                "mode".into(),
                serde_json::Value::String(mode_str.to_string()),
            );
            obj.insert(
                "tasklist_open".into(),
                serde_json::Value::Bool(snap.tasklist_open),
            );
            obj.insert(
                "subagents_open".into(),
                serde_json::Value::Bool(snap.subagents_open),
            );
            // UXI-AgentTile-20: persist force-hidden sidepanel per session. Only
            // write when true — an absent key restores as shown (false), matching
            // the §35 fields' downgrade contract.
            if snap.sidepanel_hidden {
                obj.insert("sidepanel_hidden".into(), serde_json::Value::Bool(true));
            }
            // spec-agent-cwd.md §5: persist the session's working directory.
            obj.insert(
                "cwd".into(),
                serde_json::Value::String(snap.cwd.display().to_string()),
            );
            // Model C: persist a non-empty compose draft so it survives restart.
            if let Some(draft) = snap.compose_draft.as_ref().filter(|d| !d.is_empty()) {
                obj.insert(
                    "compose_draft".into(),
                    serde_json::Value::String(draft.clone()),
                );
            }
            // UXI-AgentTile-27: persist the autoname summary so the jump panel's
            // italic line survives a restart. Same downgrade contract as the
            // fields above — only written when present.
            if let Some(summary) = snap.summary.as_ref().filter(|s| !s.is_empty()) {
                obj.insert("summary".into(), serde_json::Value::String(summary.clone()));
            }
            // Jump-panel unread dot: only write when unread, so an old binary
            // reading a read session sees the absent-key default (false).
            if snap.unread {
                obj.insert("unread".into(), serde_json::Value::Bool(true));
            }
            serde_json::Value::Object(obj)
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

/// Private `.env` credential used by session autonaming. Unlike ordinary
/// `.env` entries it is never copied into Yalda's process environment, so
/// Claude/MCP subprocesses cannot accidentally treat it as their auth mode.
static DOTENV_ANTHROPIC_API_KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub(crate) fn dotenv_anthropic_api_key() -> Option<String> {
    DOTENV_ANTHROPIC_API_KEY.get().cloned()
}

pub(crate) fn is_private_dotenv_key(key: &str) -> bool {
    key == "ANTHROPIC_API_KEY"
}

/// Load `.env` from the current directory (walking up to the filesystem root).
/// Ordinary entries enter the process environment for compatibility.
/// `ANTHROPIC_API_KEY` is retained privately for autonaming instead of exported.
///
/// Deliberately tiny — no new dependency for `KEY=value`. Three rules:
/// **real environment variables always win** (a `.env` never overrides what the
/// launching shell exported), the first `.env` found walking up wins, and any
/// malformed line is skipped rather than failing the load. `export KEY=v` and
/// surrounding quotes are tolerated because that's how people actually write
/// these files.
pub(crate) fn load_dotenv() {
    let Ok(mut dir) = std::env::current_dir() else {
        return;
    };
    let path = loop {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            break candidate;
        }
        if !dir.pop() {
            return;
        }
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return;
    };
    for (key, value) in parse_dotenv(&contents) {
        if is_private_dotenv_key(&key) {
            if std::env::var_os(&key).is_none() && !value.trim().is_empty() {
                let _ = DOTENV_ANTHROPIC_API_KEY.set(value);
            }
            continue;
        }
        // SAFETY: single-threaded startup, before any thread is spawned.
        if std::env::var_os(&key).is_none() {
            unsafe { std::env::set_var(&key, &value) };
        }
    }
}

/// Pure `KEY=value` parser for [`load_dotenv`], split out so it is unit-testable
/// without touching the process environment or the filesystem.
pub(crate) fn parse_dotenv(contents: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim();
        // Strip one layer of matching quotes, if present.
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        out.push((key.to_string(), value.to_string()));
    }
    out
}
