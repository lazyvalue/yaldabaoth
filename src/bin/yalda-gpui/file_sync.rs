//! Buffer ⇄ disk synchronisation (`UXI-Buffer-4` … `UXI-Buffer-7`).
//!
//! Every file open in the buffer pool is watched (its PARENT directory is
//! watched non-recursively, so an external editor's atomic temp-file + rename
//! write — which replaces the inode — is still seen). The OS watcher runs on
//! `notify`'s own thread and only forwards paths that belong to the pool into
//! an unbounded channel; a gpui task on the view (`start_file_sync`) awaits
//! that channel, debounces a burst, reads the changed files on the background
//! executor, and hands `(path, contents)` to [`YaldaGpuiView::apply_disk_changes`]
//! on the UI thread. Nothing here blocks paint.
//!
//! Policy (all keyed by the pool's canonical path):
//! - **clean buffer + external change** → reload silently, every view of the
//!   file keeps its caret line/col (clamped) and scroll;
//! - **dirty buffer + external change** → never clobbered; the path enters the
//!   conflict set (shown in the tile status bar) until the user picks
//!   *keep mine* (next save overwrites) or *reload theirs*;
//! - **autosave** — dirty, non-conflicted buffers are written ~1s after the last
//!   edit and when their tile loses focus, atomically (temp + rename in the same
//!   dir, `Document::save_to`);
//! - **self-write echo** — the hash of the content we last loaded/wrote is kept
//!   per path; a watcher event whose disk content hashes the same is our own
//!   write (or a no-op touch) and is ignored.

use super::*;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Idle time after the last edit before dirty buffers autosave.
pub(crate) const AUTOSAVE_DELAY: Duration = Duration::from_millis(1000);
/// Coalescing window for a burst of watcher events (an editor's write is
/// typically create-temp + write + rename: several events for one change).
pub(crate) const WATCH_DEBOUNCE: Duration = Duration::from_millis(100);

/// Content hash used for self-write echo suppression.
pub(crate) fn content_hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Forward one raw watcher path into the view's channel iff it names a file
/// currently open in the pool. Shared by the OS watcher callback and the test
/// injection seam, so both take the exact same filter.
fn forward_event(tracked: &Mutex<HashSet<PathBuf>>, tx: &UnboundedSender<PathBuf>, path: PathBuf) {
    let hit = tracked.lock().map(|t| t.contains(&path)).unwrap_or(false);
    if hit {
        let _ = tx.unbounded_send(path);
    }
}

/// Build the real OS watcher whose callback forwards pool-file events.
pub(crate) fn spawn_os_watcher(
    tracked: Arc<Mutex<HashSet<PathBuf>>>,
    tx: UnboundedSender<PathBuf>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::EventKind;
    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        // Access(Open/Read) events are not changes; everything else
        // (create / modify / rename-to / remove) may be.
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        for p in ev.paths {
            forward_event(&tracked, &tx, p);
        }
    })
}

/// Per-view file-sync state. Owned by `YaldaGpuiView`; mutated only from event
/// contexts (observer / pump / command handlers), never from render.
pub(crate) struct FileSync {
    /// Pool files currently watched (shared with the watcher thread's filter).
    tracked: Arc<Mutex<HashSet<PathBuf>>>,
    /// Parent dirs registered with the OS watcher.
    watched_dirs: HashSet<PathBuf>,
    /// Hash of the content we last loaded from / wrote to disk, per path.
    disk_hash: HashMap<PathBuf, u64>,
    /// Paths whose dirty buffer diverged from an external change on disk.
    conflicts: HashSet<PathBuf>,
    tx: UnboundedSender<PathBuf>,
    rx: Option<UnboundedReceiver<PathBuf>>,
    watcher: Option<notify::RecommendedWatcher>,
    /// Whether to create a real OS watcher. Off under `cfg(test)`: the harness
    /// injects events through `test_inject_fs_event` (same filter + pump) so
    /// no test depends on inotify/FSEvents timing.
    use_os_watcher: bool,
    /// Pending autosave (dropping it cancels — that is the debounce).
    autosave_task: Option<Task<()>>,
    /// Fingerprint of the dirty set the pending autosave was armed for.
    autosave_fingerprint: u64,
    pub(crate) autosave_delay: Duration,
    /// Focused tile + its buffer path at the last observation (focus-loss save).
    last_focus: Option<(workspace::WindowId, Option<PathBuf>)>,
    started: bool,
}

impl Default for FileSync {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Self {
            tracked: Arc::new(Mutex::new(HashSet::new())),
            watched_dirs: HashSet::new(),
            disk_hash: HashMap::new(),
            conflicts: HashSet::new(),
            tx,
            rx: Some(rx),
            watcher: None,
            use_os_watcher: !cfg!(test),
            autosave_task: None,
            autosave_fingerprint: 0,
            autosave_delay: AUTOSAVE_DELAY,
            last_focus: None,
            started: false,
        }
    }
}

impl FileSync {
    pub(crate) fn is_conflicted(&self, path: &Path) -> bool {
        self.conflicts.contains(path)
    }

    #[cfg(test)]
    pub(crate) fn is_tracked(&self, path: &Path) -> bool {
        self.tracked.lock().map(|t| t.contains(path)).unwrap_or(false)
    }

    /// Record that `text` is what is now on disk at `path` (after a load or
    /// our own write) — the echo-suppression key.
    pub(crate) fn note_disk_content(&mut self, path: &Path, text: &str) {
        self.disk_hash.insert(path.to_path_buf(), content_hash(text));
    }

    /// Reconcile the watch set to the pool's current files. New files get
    /// their disk hash seeded from the (clean) core text; closed files are
    /// forgotten and their dirs unwatched once empty.
    fn reconcile(&mut self, pool: &[(PathBuf, SharedCoreSnapshot)]) {
        let want: HashSet<PathBuf> = pool.iter().map(|(p, _)| p.clone()).collect();
        let mut tracked = match self.tracked.lock() {
            Ok(t) => t,
            Err(_) => return,
        };
        if *tracked == want {
            return;
        }
        for (p, snap) in pool {
            if !tracked.contains(p) && !self.disk_hash.contains_key(p) {
                if let Some(h) = snap.clean_hash {
                    self.disk_hash.insert(p.clone(), h);
                } else if let Ok(t) = std::fs::read_to_string(p) {
                    self.disk_hash.insert(p.clone(), content_hash(&t));
                }
            }
        }
        *tracked = want.clone();
        drop(tracked);
        self.disk_hash.retain(|p, _| want.contains(p));
        self.conflicts.retain(|p| want.contains(p));
        let want_dirs: HashSet<PathBuf> = want
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        if self.use_os_watcher && self.watcher.is_none() && !want_dirs.is_empty() {
            match spawn_os_watcher(self.tracked.clone(), self.tx.clone()) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => {
                    eprintln!("[file-sync] cannot start file watcher: {e}");
                    self.use_os_watcher = false;
                }
            }
        }
        if let Some(w) = self.watcher.as_mut() {
            use notify::Watcher;
            for d in self.watched_dirs.difference(&want_dirs) {
                let _ = w.unwatch(d);
            }
            for d in want_dirs.difference(&self.watched_dirs) {
                if let Err(e) = w.watch(d, notify::RecursiveMode::NonRecursive) {
                    eprintln!("[file-sync] cannot watch {}: {e}", d.display());
                }
            }
        }
        self.watched_dirs = want_dirs;
    }
}

/// What the observer needs from one pooled core, snapshotted without holding
/// the `RefCell` borrow.
struct SharedCoreSnapshot {
    /// `Some(hash)` when the core is clean (its text == disk at load).
    clean_hash: Option<u64>,
}

/// Visit every `EditState` bound to `core` anywhere in the workspace (all
/// workspaces, hidden tiles, and an Edit parked under a picker).
fn for_each_edit_of(
    frame: &mut workspace::Frame<App>,
    core: &workspace::SharedCore,
    f: &mut impl FnMut(&mut EditState),
) {
    fn visit(app: &mut BufferApp, core: &workspace::SharedCore, f: &mut impl FnMut(&mut EditState)) {
        match app {
            BufferApp::Editing(e) if Rc::ptr_eq(&e.editor.core, core) => f(e),
            BufferApp::Picking(b) => {
                if let Some(u) = b.underlying.as_deref_mut() {
                    visit(u, core, f);
                }
            }
            _ => {}
        }
    }
    for wsp in frame.workspaces.iter_mut() {
        wsp.for_each_attached_window_mut(&mut |w| {
            if let App::Buffer(b) = &mut w.content {
                visit(b, core, f);
            }
        });
    }
}

impl YaldaGpuiView {
    /// Start disk sync for this view: the watcher-event pump, and a
    /// self-observer that (on every notify) reconciles the watch set to the
    /// pool, re-arms the autosave debounce when the dirty set changed, and
    /// autosaves a buffer whose tile just lost focus. Called once from
    /// `main()`; idempotent.
    pub(crate) fn start_file_sync(&mut self, cx: &mut Context<Self>) {
        if self.file_sync.started {
            return;
        }
        self.file_sync.started = true;
        if let Some(mut rx) = self.file_sync.rx.take() {
            cx.spawn(async move |this, cx| {
                while let Some(first) = rx.next().await {
                    // Debounce: let the rest of the burst land, then drain.
                    cx.background_executor().timer(WATCH_DEBOUNCE).await;
                    let mut paths: Vec<PathBuf> = vec![first];
                    while let Ok(p) = rx.try_recv() {
                        if !paths.contains(&p) {
                            paths.push(p);
                        }
                    }
                    // Disk reads off the UI thread.
                    let reads = cx
                        .background_executor()
                        .spawn(async move {
                            paths
                                .into_iter()
                                .map(|p| {
                                    let t = std::fs::read_to_string(&p).ok();
                                    (p, t)
                                })
                                .collect::<Vec<_>>()
                        })
                        .await;
                    if this
                        .update(cx, |v, cx| v.apply_disk_changes(reads, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.observe_self(|v, cx| v.file_sync_observe(cx)).detach();
        // Seed the watch set for buffers opened before sync started.
        self.file_sync_observe(cx);
    }

    /// Test seam: deliver a watcher event for `path` through the same filter
    /// and channel the OS watcher uses (then the real debounce/read/apply pump).
    #[cfg(test)]
    pub(crate) fn test_inject_fs_event(&self, path: PathBuf) {
        forward_event(&self.file_sync.tracked, &self.file_sync.tx, path);
    }

    fn pool_snapshot(&self) -> Vec<(PathBuf, SharedCoreSnapshot)> {
        self.workspace
            .file_buffers
            .values()
            .map(|b| {
                let core = b.core.borrow();
                let doc = core.document();
                let clean_hash = (!doc.is_modified()).then(|| content_hash(&doc.full_text()));
                (b.canonical_path.clone(), SharedCoreSnapshot { clean_hash })
            })
            .collect()
    }

    /// The pooled core for a canonical path, if open.
    fn pooled_core(&self, path: &Path) -> Option<workspace::SharedCore> {
        let id = *self.workspace.path_index.get(path)?;
        self.workspace.buffer_core(id)
    }

    /// Canonical path of the focused tile's file buffer, if any.
    pub(crate) fn focused_buffer_path(&self) -> Option<PathBuf> {
        match self.workspace.focused_content() {
            Some(App::Buffer(BufferApp::Editing(e))) => {
                Some(e.editor.core.borrow().document().file_path.clone())
            }
            Some(App::Buffer(BufferApp::Viewing(d))) => d
                .source
                .as_ref()
                .map(|s| s.core.borrow().document().file_path.clone()),
            _ => None,
        }
    }

    /// True if `path` has an unresolved disk conflict (status bar badge).
    pub(crate) fn buffer_has_disk_conflict(&self, path: &Path) -> bool {
        self.file_sync.is_conflicted(path)
    }

    /// The self-observer body (runs in effect flush, never during render).
    fn file_sync_observe(&mut self, cx: &mut Context<Self>) {
        // 1. Watch set ⇐ pool.
        let snap = self.pool_snapshot();
        self.file_sync.reconcile(&snap);

        // 2. Focus-loss autosave: the previously focused tile's buffer.
        let now_focus = self
            .workspace
            .focused_window_id()
            .map(|id| (id, self.focused_buffer_path()));
        let prev = self.file_sync.last_focus.clone();
        if now_focus.as_ref().map(|f| f.0) != prev.as_ref().map(|f| f.0) {
            self.file_sync.last_focus = now_focus;
            if let Some((_, Some(prev_path))) = prev {
                self.autosave_path(&prev_path);
            }
        }

        // 3. Autosave debounce: re-arm when the dirty set (paths × edit_seq) moved.
        let mut dirty: Vec<(PathBuf, u64)> = self
            .workspace
            .file_buffers
            .values()
            .filter(|b| !self.file_sync.is_conflicted(&b.canonical_path))
            .filter_map(|b| {
                let core = b.core.borrow();
                let doc = core.document();
                doc.is_modified()
                    .then(|| (b.canonical_path.clone(), doc.edit_seq()))
            })
            .collect();
        dirty.sort();
        let fp = if dirty.is_empty() {
            0
        } else {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            dirty.hash(&mut h);
            h.finish() | 1
        };
        if fp == self.file_sync.autosave_fingerprint {
            return;
        }
        self.file_sync.autosave_fingerprint = fp;
        if fp == 0 {
            self.file_sync.autosave_task = None;
            return;
        }
        let delay = self.file_sync.autosave_delay;
        self.file_sync.autosave_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |v, cx| {
                // Detach (not drop) our own handle: we are running inside it.
                if let Some(t) = v.file_sync.autosave_task.take() {
                    t.detach();
                }
                v.autosave_dirty_buffers(cx);
            });
        }));
    }

    /// Write every dirty, non-conflicted pooled buffer to disk.
    pub(crate) fn autosave_dirty_buffers(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.workspace.path_index.keys().cloned().collect();
        let mut wrote = false;
        for p in paths {
            wrote |= self.autosave_path(&p);
        }
        if wrote {
            cx.notify();
        }
    }

    /// Autosave one path if it is dirty and not in conflict. Returns whether
    /// it wrote.
    fn autosave_path(&mut self, path: &Path) -> bool {
        if self.file_sync.is_conflicted(path) {
            return false;
        }
        let Some(core) = self.pooled_core(path) else {
            return false;
        };
        if !core.borrow().document().is_modified() {
            return false;
        }
        match self.write_core_to_disk(&core) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[file-sync] autosave {} failed: {e}", path.display());
                false
            }
        }
    }

    /// THE write path for a file buffer (manual Ctrl-S and autosave): atomic
    /// temp + rename (`Document::save_to`), then record the written content's
    /// hash so the watcher echo of our own write is ignored.
    pub(crate) fn write_core_to_disk(&mut self, core: &workspace::SharedCore) -> std::io::Result<()> {
        core.borrow_mut().save()?;
        let (path, text) = {
            let c = core.borrow();
            (c.document().file_path.clone(), c.document().full_text())
        };
        self.file_sync.note_disk_content(&path, &text);
        Ok(())
    }

    /// After an explicit reload replaced `core` with the disk text: record it
    /// as the known disk content and drop any conflict for the path.
    pub(crate) fn note_core_matches_disk(&mut self, core: &workspace::SharedCore) {
        let (path, text) = {
            let c = core.borrow();
            (c.document().file_path.clone(), c.document().full_text())
        };
        self.file_sync.note_disk_content(&path, &text);
        self.file_sync.conflicts.remove(&path);
    }

    /// Apply a debounced batch of `(path, disk contents)` (the pump's output).
    pub(crate) fn apply_disk_changes(
        &mut self,
        reads: Vec<(PathBuf, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for (path, disk) in reads {
            // Deleted / unreadable: nothing to reload (a rename-into-place
            // arrives as its own event). Leave the buffer alone.
            let Some(disk) = disk else { continue };
            let h = content_hash(&disk);
            if self.file_sync.disk_hash.get(&path) == Some(&h) {
                continue; // our own write echo, or a no-op touch
            }
            let Some(core) = self.pooled_core(&path) else { continue };
            self.file_sync.disk_hash.insert(path.clone(), h);
            let (modified, same) = {
                let c = core.borrow();
                (c.document().is_modified(), c.document().full_text() == disk)
            };
            if same {
                // Disk caught up with the buffer (e.g. identical external write).
                changed |= self.file_sync.conflicts.remove(&path);
                continue;
            }
            if modified {
                changed |= self.file_sync.conflicts.insert(path);
            } else {
                self.reload_core_preserving_carets(&core, disk);
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Replace `core`'s text with `text` (bumping the content generation —
    /// UXI-Buffer-3) and clamp every bound Edit view's caret to the new text,
    /// keeping its line/col where it still exists. Scroll is untouched (the
    /// list reconciles by splice, so the viewport stays anchored).
    fn reload_core_preserving_carets(&mut self, core: &workspace::SharedCore, text: String) {
        let path = core.borrow().document().file_path.clone();
        self.file_sync.note_disk_content(&path, &text);
        self.file_sync.conflicts.remove(&path);
        core.borrow_mut().replace_text(text, path);
        let (line_count, line_lens): (usize, Vec<usize>) = {
            let c = core.borrow();
            let d = c.document();
            let n = d.line_count().max(1);
            (n, (0..n).map(|l| d.line_len_chars(l)).collect())
        };
        for_each_edit_of(&mut self.workspace, core, &mut |e| {
            let cur = e.editor.cursor();
            let line = cur.line.min(line_count - 1);
            let col = cur.col.min(line_lens.get(line).copied().unwrap_or(0));
            e.editor.set_cursor(line, col);
            e.editor.clear_selection();
        });
    }

    /// `space k` — keep my buffer: clear the conflict; the next save (manual
    /// or the autosave this re-arms) overwrites the disk version.
    pub(crate) fn disk_conflict_keep_mine(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.focused_buffer_path()
            && self.file_sync.conflicts.remove(&p)
        {
            cx.notify();
        }
    }

    /// `space R` — reload theirs: discard the buffer's edits and take the disk
    /// version, carets preserved (clamped).
    pub(crate) fn disk_conflict_reload_theirs(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.focused_buffer_path() else { return };
        let Some(core) = self.pooled_core(&p) else { return };
        match std::fs::read_to_string(&p) {
            Ok(text) => {
                self.reload_core_preserving_carets(&core, text);
                cx.notify();
            }
            Err(e) => eprintln!("[file-sync] reload {} failed: {e}", p.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real OS watcher glue: a write to a tracked file in a watched dir is
    /// forwarded; an untracked sibling (e.g. our own `.x.tmp`) is filtered.
    /// Bounded wait (5s) — the only test that touches real inotify/FSEvents.
    #[test]
    fn os_watcher_forwards_only_tracked_files() {
        use notify::Watcher;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("a.md");
        std::fs::write(&file, "one").unwrap();
        let tracked = Arc::new(Mutex::new(HashSet::from([file.clone()])));
        let (tx, mut rx) = unbounded();
        let mut w = spawn_os_watcher(tracked, tx).unwrap();
        w.watch(&root, notify::RecursiveMode::NonRecursive).unwrap();
        std::fs::write(root.join("other.tmp"), "noise").unwrap();
        std::fs::write(&file, "two").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while std::time::Instant::now() < deadline {
            while let Ok(p) = rx.try_recv() {
                got.push(p);
            }
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(got.contains(&file), "tracked write forwarded: {got:?}");
        assert!(got.iter().all(|p| p == &file), "untracked paths filtered: {got:?}");
    }
}
