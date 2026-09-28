use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::keys::KeyPress;
use crate::line_input::{LineEdit, LineInput};
use crate::worktree;

const MAX_SEARCH_RESULTS: usize = 200;
const MAX_SEARCH_DEPTH: usize = 8;

/// Directory names the recursive fuzzy find never descends into — build output,
/// dependency caches, VCS metadata. Descending into `target/` (a Rust build dir
/// with tens of thousands of files) was the bulk of the "slow + finds too much"
/// problem: the search walked all of it and matched on the full path. These are
/// matched by exact directory name (case-insensitive), independent of the
/// dotfile rule (`target`/`node_modules` are not dotfiles).
const IGNORED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".cache",
    "vendor",
    ".idea",
    ".gradle",
    "DerivedData",
];

/// Fuzzy subsequence score of `query` against `text` (both compared
/// lowercased). Returns `Some(score)` — higher is better — when every char of
/// `query` appears in `text` in order, else `None`. Contiguous runs and matches
/// at a word boundary (string start or just after a separator) score higher, so
/// `fb` ranks `file_browser.rs` above an incidental `…f…b…` scatter. This is the
/// core of the finder: matching a *subsequence of the filename* (not a substring
/// of the whole path) is what stops every parent-directory component from
/// producing a hit.
pub fn fuzzy_score(text: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0i32;
    let mut hi = 0usize;
    let mut last_match: Option<usize> = None;
    for &nc in &needle {
        let mut found = false;
        while hi < hay.len() {
            if hay[hi] == nc {
                score += 1;
                if last_match == Some(hi.wrapping_sub(1)) {
                    score += 4; // contiguous with the previous matched char
                }
                let boundary = hi == 0 || matches!(hay[hi - 1], '/' | '_' | '-' | '.' | ' ');
                if boundary {
                    score += 6; // start of a word / path component
                }
                last_match = Some(hi);
                hi += 1;
                found = true;
                break;
            }
            hi += 1;
        }
        if !found {
            return None;
        }
    }
    // Prefer denser matches: a short name beats a long one at equal match shape.
    score -= hay.len() as i32 / 16;
    Some(score)
}

thread_local! {
    /// Per-thread count of recursive filesystem walks ([`search`] calls). A
    /// perf probe for A8: the UI-thread keystroke path must never walk
    /// synchronously, so a test reads this before/after a keystroke on the
    /// thread that handled it. Thread-local so parallel tests don't race.
    static RECURSIVE_WALKS: Cell<usize> = const { Cell::new(0) };
}

/// How many recursive walks ([`search`]) have run on the calling thread.
pub fn recursive_walk_count() -> usize {
    RECURSIVE_WALKS.with(Cell::get)
}

/// Process-wide generation counter for recursive searches. Globally unique (not
/// per browser) so a completed search can be offered to every live browser and
/// only the one that issued it — and only if nothing re-filtered since — takes it.
static NEXT_SEARCH_GEN: AtomicU64 = AtomicU64::new(1);

/// A recursive search the browser wants run off the UI thread (A8). Produced by
/// the keystroke path ([`FileBrowser::filter_key`]) and handed out by
/// [`FileBrowser::take_pending_search`]; run it with [`search`] anywhere, then
/// fold the result back with [`FileBrowser::apply_search_results`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub generation: u64,
    pub dir: PathBuf,
    pub query: String,
    pub show_hidden: bool,
}

impl SearchRequest {
    /// Run the recursive walk for this request (blocking — call it on a
    /// background thread from a UI).
    pub fn run(&self) -> Vec<BrowserEntry> {
        search(&self.dir, &self.query, self.show_hidden)
    }
}

/// The recursive fuzzy find under `dir`: walk (depth-capped, result-capped,
/// skipping [`IGNORED_DIRS`]) and rank by fuzzy score of the filename (or whole
/// relative path for a `/` query). Pure w.r.t. the browser — touches only the
/// filesystem — so it can run on any thread. Each row's `name` is the path
/// relative to `dir`.
pub fn search(dir: &Path, query: &str, show_hidden: bool) -> Vec<BrowserEntry> {
    RECURSIVE_WALKS.with(|c| c.set(c.get() + 1));
    let query = query.to_lowercase();
    let mut results = Vec::new();
    FileBrowser::search_recursive(dir, dir, &query, &mut results, 0, show_hidden);
    rank_results(&mut results, &query);
    results
}

/// Rank search rows by fuzzy score of the filename (DESC — higher is better),
/// then shorter path, then alphabetical. `search_target` picks the same field
/// the matcher matched on, so ranking and inclusion agree. `query` lowercased.
fn rank_results(results: &mut [BrowserEntry], query: &str) {
    results.sort_by(|a, b| {
        let sa = fuzzy_score(FileBrowser::search_target(&a.name, query), query).unwrap_or(i32::MIN);
        let sb = fuzzy_score(FileBrowser::search_target(&b.name, query), query).unwrap_or(i32::MIN);
        sb.cmp(&sa)
            .then_with(|| a.name.len().cmp(&b.name.len()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Name,
    DateDesc,
    DateAsc,
}

impl SortOrder {
    pub fn cycle(self) -> Self {
        match self {
            SortOrder::Name => SortOrder::DateDesc,
            SortOrder::DateDesc => SortOrder::DateAsc,
            SortOrder::DateAsc => SortOrder::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortOrder::Name => "name",
            SortOrder::DateDesc => "date \u{2193}",
            SortOrder::DateAsc => "date \u{2191}",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
}

/// Transient state for an in-progress rename of the selected entry.
pub struct RenameState {
    /// The edited name (seeded with the entry's current name).
    pub input: LineInput,
    /// Last failed-commit message, shown inline until the user edits again.
    pub error: Option<String>,
}

/// Transient state for the worktree-picker overlay inside the file browser.
pub struct WorktreeMode {
    pub worktrees: Vec<worktree::Worktree>,
    pub selected: usize,
}

impl WorktreeMode {
    pub fn move_down(&mut self) {
        let len = self.worktrees.len();
        if len > 0 {
            self.selected = (self.selected + 1) % len;
        }
    }

    pub fn move_up(&mut self) {
        let len = self.worktrees.len();
        if len == 0 {
            return;
        }
        if self.selected == 0 {
            self.selected = len - 1;
        } else {
            self.selected -= 1;
        }
    }
}

pub struct FileBrowser {
    #[allow(dead_code)]
    root: PathBuf,
    current_dir: PathBuf,
    entries: Vec<BrowserEntry>,
    selected: usize,
    filter: LineInput,
    filtered_indices: Vec<usize>,
    /// Recursive search results (populated when filter is non-empty). While a
    /// deferred search is in flight this holds the provisional shallow
    /// (current-dir) matches.
    search_results: Vec<BrowserEntry>,
    /// Generation of the current `(dir, filter, show_hidden)` search. Bumped on
    /// every rebuild so an in-flight result for an older query is dropped.
    search_gen: u64,
    /// A deferred recursive search not yet handed to an executor.
    pending_search: Option<SearchRequest>,
    /// True from a deferred rebuild until its results are applied.
    search_in_flight: bool,
    pub filter_mode: bool,
    pub show_hidden: bool,
    pub sort_order: SortOrder,
    /// When `Some`, the browser shows a worktree-picker overlay instead of
    /// the normal directory listing.
    pub worktree_mode: Option<WorktreeMode>,
    /// When `Some`, the selected entry is being renamed in place.
    pub rename: Option<RenameState>,
}

impl FileBrowser {
    pub fn new(start_dir: PathBuf) -> Self {
        let mut browser = Self {
            root: start_dir.clone(),
            current_dir: start_dir,
            entries: Vec::new(),
            selected: 0,
            filter: LineInput::new(),
            filtered_indices: Vec::new(),
            search_results: Vec::new(),
            search_gen: 0,
            pending_search: None,
            search_in_flight: false,
            filter_mode: false,
            show_hidden: false,
            sort_order: SortOrder::Name,
            worktree_mode: None,
            rename: None,
        };
        browser.refresh();
        browser
    }

    pub fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    pub fn entries(&self) -> &[BrowserEntry] {
        &self.entries
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, idx: usize) {
        let max = self.visible_entries().len().saturating_sub(1);
        self.selected = idx.min(max);
    }

    /// Get entries visible after filtering.
    pub fn visible_entries(&self) -> Vec<&BrowserEntry> {
        if self.filter.is_empty() {
            self.entries.iter().collect()
        } else {
            self.search_results.iter().collect()
        }
    }

    /// Get the currently selected entry.
    pub fn selected_entry(&self) -> Option<&BrowserEntry> {
        let visible = self.visible_entries();
        visible.get(self.selected).copied()
    }

    pub fn move_down(&mut self) {
        let len = self.visible_entries().len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + 1) % len;
    }

    pub fn move_up(&mut self) {
        let len = self.visible_entries().len();
        if len == 0 {
            return;
        }
        if self.selected == 0 {
            self.selected = len - 1;
        } else {
            self.selected -= 1;
        }
    }

    /// Enter the selected entry. Returns Some(path) if a file was selected (to open),
    /// or None if a directory was entered.
    pub fn enter_selected(&mut self) -> Option<PathBuf> {
        let entry = self.selected_entry()?.clone();
        if entry.is_dir {
            self.current_dir = entry.path;
            self.selected = 0;
            self.clear_filter();
            self.refresh();
            None
        } else {
            Some(entry.path)
        }
    }

    /// Navigate to parent directory, landing the cursor on the directory we
    /// just came from (not the top) so `l`/`h` in-and-out keeps your place.
    /// No-op at filesystem root.
    pub fn go_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            let came_from = self.current_dir.clone();
            self.current_dir = parent.to_path_buf();
            self.selected = 0;
            self.clear_filter();
            self.refresh();
            self.select_path(&came_from);
        }
    }

    /// Move the cursor onto the visible entry whose path equals `path` (the file
    /// or directory you just left). No-op while filtering, or if `path` is not a
    /// row in the current directory. Used by `go_parent` (land on the child dir)
    /// and by the buffer→browser open (land on the open file).
    pub fn select_path(&mut self, path: &Path) {
        if !self.filter.is_empty() {
            return;
        }
        if let Some(idx) = self.entries.iter().position(|e| e.name != ".." && e.path == path) {
            self.selected = idx;
        }
    }

    /// Set the filter programmatically and run the recursive search
    /// synchronously (a blocking walk — not for the UI keystroke path, which
    /// goes through [`Self::filter_key`] and defers the walk).
    pub fn set_filter(&mut self, text: &str) {
        self.filter.set_text(text);
        self.rebuild_filtered(false);
        self.selected = 0;
    }

    /// Route an editing key to the filter field; an edit re-filters and
    /// resets the selection to the first match. A8: this is the keystroke path,
    /// so it NEVER walks the filesystem recursively — it shows the shallow
    /// (current-dir) matches immediately and queues a [`SearchRequest`]
    /// (collect it with [`Self::take_pending_search`], run it off-thread, and
    /// fold it back with [`Self::apply_search_results`]).
    pub fn filter_key(&mut self, press: &KeyPress) -> LineEdit {
        let edit = self.filter.handle(press);
        if edit.edited() {
            self.rebuild_filtered(true);
            self.selected = 0;
        }
        edit
    }

    /// Hand out the queued deferred recursive search, if any (A8).
    pub fn take_pending_search(&mut self) -> Option<SearchRequest> {
        self.pending_search.take()
    }

    /// True while a deferred recursive search for the current query has not
    /// yet been applied.
    pub fn search_in_flight(&self) -> bool {
        self.search_in_flight
    }

    /// Whether a search result of `generation` is current for this browser
    /// (i.e. [`Self::apply_search_results`] would take it).
    pub fn accepts_search(&self, generation: u64) -> bool {
        generation == self.search_gen && !self.filter.is_empty()
    }

    /// Fold a completed recursive search back in. Returns `false` (and changes
    /// nothing) when `generation` is not this browser's current search — the
    /// query / dir / hidden-toggle changed since it was issued, or it belongs to
    /// another browser. Keeps the selected row selected when it survives.
    pub fn apply_search_results(&mut self, generation: u64, results: Vec<BrowserEntry>) -> bool {
        if !self.accepts_search(generation) {
            return false;
        }
        let keep = self.selected_entry().map(|e| e.path.clone());
        self.search_results = results;
        self.search_in_flight = false;
        self.selected = keep
            .and_then(|p| self.search_results.iter().position(|e| e.path == p))
            .unwrap_or(0);
        true
    }

    /// Run any queued deferred search right here, blocking (for non-UI
    /// consumers that want `filter_key` to behave synchronously).
    pub fn run_pending_search_blocking(&mut self) {
        if let Some(req) = self.take_pending_search() {
            let results = req.run();
            self.apply_search_results(req.generation, results);
        }
    }

    /// The filter field (text + caret) for rendering.
    pub fn filter_input(&self) -> &LineInput {
        &self.filter
    }

    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.filter_mode = false;
        self.rebuild_filtered(false);
        self.selected = 0;
    }

    pub fn filter_text(&self) -> &str {
        self.filter.text()
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refresh();
        self.selected = 0;
    }

    pub fn cycle_sort(&mut self) {
        self.sort_order = self.sort_order.cycle();
        self.refresh();
        self.selected = 0;
    }

    /// Set the sort order directly (used to seed a freshly-opened picker from
    /// the tile's remembered order). No-op if already that order.
    pub fn set_sort_order(&mut self, order: SortOrder) {
        if self.sort_order == order {
            return;
        }
        self.sort_order = order;
        self.refresh();
        self.selected = 0;
    }

    /// Reload `entries` from disk, then rebuild the derived filtered/search
    /// lists so they always reflect the current `(entries, filter_text)`.
    fn refresh(&mut self) {
        self.entries = Self::list_directory(&self.current_dir, self.show_hidden, self.sort_order);
        // A re-list while filtering (hidden toggle, sort, rename) happens on the
        // UI thread too — defer its recursive walk like a keystroke (A8).
        self.rebuild_filtered(true);
    }

    fn list_directory(dir: &Path, show_hidden: bool, sort_order: SortOrder) -> Vec<BrowserEntry> {
        let read_dir = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files unless toggled on
            if !show_hidden && name.starts_with('.') {
                continue;
            }

            let path = entry.path();

            // Follow symlinks — check the resolved metadata
            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue, // broken symlink — skip
            };

            let is_dir = metadata.is_dir();
            let size = if metadata.is_file() {
                Some(metadata.len())
            } else {
                None
            };
            let modified = metadata.modified().ok();
            let browser_entry = BrowserEntry {
                name,
                is_dir,
                path,
                size,
                modified,
            };

            if is_dir {
                dirs.push(browser_entry);
            } else {
                files.push(browser_entry);
            }
        }

        // Sort each group
        let sort_entries = |entries: &mut Vec<BrowserEntry>| match sort_order {
            SortOrder::Name => {
                entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            SortOrder::DateDesc => {
                entries.sort_by(|a, b| b.modified.cmp(&a.modified));
            }
            SortOrder::DateAsc => {
                entries.sort_by(|a, b| a.modified.cmp(&b.modified));
            }
        };
        sort_entries(&mut dirs);
        sort_entries(&mut files);

        // Parent entry first, then directories, then files
        let mut result = Vec::new();
        if let Some(parent) = dir.parent() {
            result.push(BrowserEntry {
                name: "..".to_string(),
                is_dir: true,
                path: parent.to_path_buf(),
                size: None,
                modified: None,
            });
        }
        result.extend(dirs);
        result.extend(files);
        result
    }

    /// Single source of truth for the two derived lists. Rebuilds both
    /// `filtered_indices` (indices into `entries` whose name matches the
    /// filter) and `search_results` (recursive matches) from the current
    /// `(entries, filter_text)`. Call this at every filter/dir-change site.
    ///
    /// `defer` (A8): when true the recursive walk is NOT run here — the
    /// shallow current-dir matches become the provisional `search_results`
    /// and a [`SearchRequest`] is queued; when false the walk runs inline.
    /// Either way the generation bumps, so any in-flight result is stale.
    fn rebuild_filtered(&mut self, defer: bool) {
        self.filtered_indices.clear();
        self.search_results.clear();
        self.pending_search = None;
        self.search_in_flight = false;
        self.search_gen = NEXT_SEARCH_GEN.fetch_add(1, Ordering::Relaxed);
        if self.filter.is_empty() {
            return;
        }
        let query = self.filter.text().to_lowercase();

        // Shallow filter over the current directory's entries.
        self.filtered_indices = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();

        if defer {
            // Provisional rows: the depth-0 slice of what the recursive walk
            // will find (same matcher + ranking), from the already-listed
            // entries — no filesystem access.
            let mut shallow: Vec<BrowserEntry> = self
                .entries
                .iter()
                .filter(|e| e.name != "..")
                .filter(|e| fuzzy_score(Self::search_target(&e.name, &query), &query).is_some())
                .cloned()
                .collect();
            rank_results(&mut shallow, &query);
            self.search_results = shallow;
            self.search_in_flight = true;
            self.pending_search = Some(SearchRequest {
                generation: self.search_gen,
                dir: self.current_dir.clone(),
                query,
                show_hidden: self.show_hidden,
            });
        } else {
            self.search_results = search(&self.current_dir, &query, self.show_hidden);
        }
    }

    /// Which string a `search_results` row (whose `name` is a path relative to
    /// `current_dir`) is fuzzy-matched/ranked against: the whole relative path
    /// when the query looks path-like (contains `/`), otherwise just the
    /// filename. Filename-only matching is what keeps the finder from lighting
    /// up on every parent-directory component.
    fn search_target<'a>(relative: &'a str, query: &str) -> &'a str {
        if query.contains('/') {
            relative
        } else {
            relative.rsplit('/').next().unwrap_or(relative)
        }
    }

    // ── Worktree mode ────────────────────────────────────────────

    /// Navigate the browser to an arbitrary directory.
    pub fn navigate_to(&mut self, dir: PathBuf) {
        self.current_dir = dir;
        self.selected = 0;
        self.clear_filter();
        self.refresh();
    }

    // ── Rename ───────────────────────────────────────────────────

    /// Begin renaming the selected entry. No-op while filtering or in
    /// worktree mode, and never on the `..` parent row.
    pub fn begin_rename(&mut self) {
        if self.worktree_mode.is_some() || self.filter_mode {
            return;
        }
        if let Some(e) = self.selected_entry()
            && e.name != ".."
        {
            self.rename = Some(RenameState {
                input: LineInput::with_text(e.name.clone()),
                error: None,
            });
        }
    }

    /// Abandon an in-progress rename.
    pub fn cancel_rename(&mut self) {
        self.rename = None;
    }

    /// Route an editing key to the rename field; an edit clears any prior
    /// error.
    pub fn rename_key(&mut self, press: &KeyPress) -> LineEdit {
        let Some(r) = &mut self.rename else {
            return LineEdit::Unhandled;
        };
        let edit = r.input.handle(press);
        if edit.edited() {
            r.error = None;
        }
        edit
    }

    /// Commit the in-progress rename via `fs::rename`. On a filesystem error
    /// or name conflict the rename stays open with the message stashed in
    /// `RenameState::error`; on success (or a no-op rename) it closes.
    pub fn commit_rename(&mut self) {
        let new_name = match &self.rename {
            Some(r) => r.input.text().trim().to_string(),
            None => return,
        };
        let entry = match self.selected_entry() {
            Some(e) => e.clone(),
            None => {
                self.rename = None;
                return;
            }
        };
        // Empty or unchanged → treat as cancel.
        if new_name.is_empty() || new_name == entry.name {
            self.rename = None;
            return;
        }
        if new_name.contains('/') || new_name.contains('\\') {
            self.set_rename_error("name cannot contain a path separator");
            return;
        }
        let dest = self.current_dir.join(&new_name);
        if dest.exists() {
            self.set_rename_error(&format!("\"{new_name}\" already exists"));
            return;
        }
        match fs::rename(&entry.path, &dest) {
            Ok(()) => {
                self.rename = None;
                self.refresh();
                // Keep the renamed entry selected if we can find it again.
                if let Some(idx) = self.entries.iter().position(|e| e.name == new_name) {
                    self.selected = idx;
                }
            }
            Err(e) => self.set_rename_error(&format!("rename failed: {e}")),
        }
    }

    fn set_rename_error(&mut self, msg: &str) {
        if let Some(r) = &mut self.rename {
            r.error = Some(msg.to_string());
        }
    }

    /// Enter worktree selection mode.
    pub fn enter_worktree_mode(&mut self) {
        let mut wts = worktree::list_worktrees(&self.current_dir);
        worktree::mark_current(&mut wts, &self.current_dir);
        let selected = worktree::best_match_index(&wts, &self.current_dir);
        self.worktree_mode = Some(WorktreeMode {
            worktrees: wts,
            selected,
        });
    }

    /// Exit worktree mode without selecting.
    pub fn exit_worktree_mode(&mut self) {
        self.worktree_mode = None;
    }

    /// Select the current worktree and navigate to it. Returns true if a
    /// worktree was selected (the browser's `current_dir` was changed).
    pub fn select_worktree(&mut self) -> bool {
        let wm = match self.worktree_mode.take() {
            Some(wm) => wm,
            None => return false,
        };
        let wt = match wm.worktrees.get(wm.selected) {
            Some(wt) => wt,
            None => return false,
        };
        let target_root = wt.path.clone();

        // Try to preserve the relative subdirectory the user was browsing.
        let relative_suffix = self.relative_suffix_in_current_worktree(&wm.worktrees);
        let mut dest = target_root.clone();
        if let Some(suffix) = relative_suffix {
            let candidate = target_root.join(&suffix);
            if candidate.is_dir() {
                dest = candidate;
            }
        }
        self.navigate_to(dest);
        true
    }

    /// Find the relative path from the best-matching worktree root to
    /// `current_dir`. Returns `None` if no worktree contains `current_dir`
    /// or if `current_dir` is exactly at the worktree root.
    fn relative_suffix_in_current_worktree(
        &self,
        worktrees: &[worktree::Worktree],
    ) -> Option<PathBuf> {
        let current_wt = worktrees
            .iter()
            .filter(|wt| self.current_dir.starts_with(&wt.path))
            .max_by_key(|wt| wt.path.as_os_str().len())?;
        let suffix = self.current_dir.strip_prefix(&current_wt.path).ok()?;
        if suffix.as_os_str().is_empty() {
            None
        } else {
            Some(suffix.to_path_buf())
        }
    }

    fn search_recursive(
        base: &Path,
        dir: &Path,
        query: &str,
        results: &mut Vec<BrowserEntry>,
        depth: usize,
        show_hidden: bool,
    ) {
        if depth > MAX_SEARCH_DEPTH || results.len() >= MAX_SEARCH_RESULTS {
            return;
        }

        let read_dir = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };

        for entry in read_dir.flatten() {
            if results.len() >= MAX_SEARCH_RESULTS {
                return;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }

            let path = entry.path();
            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let is_dir = metadata.is_dir();

            // Show relative path from the current directory
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .display()
                .to_string();

            // Fuzzy-match the FILENAME (or the whole relative path for a
            // path-like query), not a substring of the full path — so a query
            // matches names, not every ancestor directory it happens to sit
            // under. `query` is already lowercased by the caller.
            if fuzzy_score(Self::search_target(&relative, query), query).is_some() {
                let size = if metadata.is_file() {
                    Some(metadata.len())
                } else {
                    None
                };
                let modified = metadata.modified().ok();
                results.push(BrowserEntry {
                    name: relative,
                    is_dir,
                    path: path.clone(),
                    size,
                    modified,
                });
            }

            // Never descend into build output / dependency caches / VCS dirs —
            // the main cause of the finder being slow and swamped.
            if is_dir && !IGNORED_DIRS.iter().any(|d| d.eq_ignore_ascii_case(&name)) {
                Self::search_recursive(base, &path, query, results, depth + 1, show_hidden);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn fuzzy_score_requires_subsequence_and_ranks_boundaries() {
        // Non-subsequence → no match.
        assert!(fuzzy_score("readme.md", "zzz").is_none());
        // Subsequence match.
        assert!(fuzzy_score("file_browser.rs", "fb").is_some());
        // A boundary/prefix match outranks a scattered one.
        let boundary = fuzzy_score("file_browser.rs", "fb").unwrap();
        let scattered = fuzzy_score("affable_number.rs", "fb").unwrap();
        assert!(
            boundary > scattered,
            "boundary match ({boundary}) must outrank scattered ({scattered})"
        );
        // Empty query matches everything (browsing, not filtering).
        assert_eq!(fuzzy_score("anything", ""), Some(0));
    }

    #[test]
    fn search_skips_ignored_dirs_and_matches_filename_not_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A source file we WANT to find.
        touch(&root.join("src/widget.rs"));
        // Build output that must be ignored even though its path contains "src".
        touch(&root.join("target/debug/build/src_generated_widget.rs"));
        touch(&root.join("node_modules/pkg/widget.js"));
        // A file whose PARENT dir matches the query but whose NAME does not —
        // must NOT appear (the "finds too much" regression).
        touch(&root.join("widgetry/notes.txt"));

        let mut fb = FileBrowser::new(root.to_path_buf());
        fb.set_filter("widget");
        let names: Vec<String> = fb
            .visible_entries()
            .iter()
            .map(|e| e.name.replace('\\', "/"))
            .collect();

        assert!(
            names.iter().any(|n| n == "src/widget.rs"),
            "the real source file must be found: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with("target/")),
            "target/ must be skipped: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with("node_modules/")),
            "node_modules/ must be skipped: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "widgetry/notes.txt"),
            "a file matched only via its parent-dir name must NOT appear: {names:?}"
        );
    }

    #[test]
    fn go_parent_lands_on_the_child_dir_just_left() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        // Parent holds several dirs so the target isn't accidentally at index 0
        // (after the `..` row). Names chosen so "child" sorts late.
        for d in ["aaa", "bbb", "child", "zzz"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let mut fb = FileBrowser::new(root.join("child"));
        fb.go_parent();
        let sel = fb.selected_entry().expect("a selection after go_parent");
        assert_eq!(
            sel.path,
            root.join("child"),
            "cursor must land on the directory we came from, not the top"
        );
    }

    #[test]
    fn select_path_lands_on_the_named_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        for f in ["a.md", "target.md", "z.md"] {
            touch(&root.join(f));
        }
        let mut fb = FileBrowser::new(root.clone());
        fb.select_path(&root.join("target.md"));
        let sel = fb.selected_entry().expect("a selection");
        assert_eq!(sel.path, root.join("target.md"), "cursor lands on the file we came from");
    }

    fn type_char(fb: &mut FileBrowser, c: char) {
        use crate::keys::{Key, Modifiers};
        let edit = fb.filter_key(&KeyPress::new(Key::Char(c), Modifiers::NONE));
        assert!(edit.edited(), "typing {c:?} edits the filter");
    }

    /// A8: the filter KEYSTROKE path never walks the tree synchronously — it
    /// shows shallow matches and queues a request; running + applying it lands
    /// the nested match. Negative control: make `filter_key` call
    /// `rebuild_filtered(false)` → the walk counter moves on the keystroke.
    #[test]
    fn filter_key_defers_recursive_walk_and_applies_results() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("zeta.md"));
        touch(&root.join("deep/er/zebra.md"));
        let mut fb = FileBrowser::new(root.to_path_buf());
        fb.filter_mode = true;

        let before = recursive_walk_count();
        type_char(&mut fb, 'z');
        assert_eq!(
            recursive_walk_count(),
            before,
            "a filter keystroke must not run the recursive walk synchronously"
        );
        let names: Vec<String> = fb.visible_entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["zeta.md".to_string()], "shallow matches show immediately");
        assert!(fb.search_in_flight());

        let req = fb.take_pending_search().expect("a deferred search is queued");
        assert_eq!(req.query, "z");
        let results = req.run();
        assert_eq!(recursive_walk_count(), before + 1);
        assert!(fb.apply_search_results(req.generation, results));
        assert!(!fb.search_in_flight());
        let names: Vec<String> = fb
            .visible_entries()
            .iter()
            .map(|e| e.name.replace('\\', "/"))
            .collect();
        assert!(
            names.iter().any(|n| n == "deep/er/zebra.md"),
            "the nested match lands after apply: {names:?}"
        );
    }

    /// A8: results for a superseded query are dropped. Negative control: remove
    /// the `generation != self.search_gen` check → the stale `z` results land.
    #[test]
    fn stale_generation_search_results_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("deep/zebra.md"));
        touch(&root.join("deep/zq.md"));
        let mut fb = FileBrowser::new(root.to_path_buf());
        fb.filter_mode = true;
        type_char(&mut fb, 'z');
        let stale = fb.take_pending_search().unwrap();
        type_char(&mut fb, 'q');
        let fresh = fb.take_pending_search().unwrap();
        assert_ne!(stale.generation, fresh.generation);

        let stale_results = stale.run();
        assert!(!stale_results.is_empty());
        assert!(
            !fb.apply_search_results(stale.generation, stale_results),
            "a stale generation must be rejected"
        );
        assert!(
            fb.visible_entries().is_empty(),
            "stale results must not replace the current (zq) provisional rows"
        );
        assert!(fb.apply_search_results(fresh.generation, fresh.run()));
        let names: Vec<String> = fb
            .visible_entries()
            .iter()
            .map(|e| e.name.replace('\\', "/"))
            .collect();
        assert_eq!(names, vec!["deep/zq.md".to_string()]);
    }
}
