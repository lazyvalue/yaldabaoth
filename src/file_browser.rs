use std::fs;
use std::path::{Path, PathBuf};

use crate::worktree;

const MAX_SEARCH_RESULTS: usize = 200;
const MAX_SEARCH_DEPTH: usize = 8;

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
    pub input: String,
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
    filter_text: String,
    filtered_indices: Vec<usize>,
    /// Recursive search results (populated when filter is non-empty).
    search_results: Vec<BrowserEntry>,
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
            filter_text: String::new(),
            filtered_indices: Vec::new(),
            search_results: Vec::new(),
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
        if self.filter_text.is_empty() {
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

    /// Navigate to parent directory. No-op at filesystem root.
    pub fn go_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.selected = 0;
            self.clear_filter();
            self.refresh();
        }
    }

    pub fn set_filter(&mut self, text: &str) {
        self.filter_text = text.to_string();
        self.rebuild_filtered();
        self.selected = 0;
    }

    pub fn clear_filter(&mut self) {
        self.filter_text.clear();
        self.filter_mode = false;
        self.rebuild_filtered();
        self.selected = 0;
    }

    pub fn filter_text(&self) -> &str {
        &self.filter_text
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

    /// Reload `entries` from disk, then rebuild the derived filtered/search
    /// lists so they always reflect the current `(entries, filter_text)`.
    fn refresh(&mut self) {
        self.entries = Self::list_directory(&self.current_dir, self.show_hidden, self.sort_order);
        self.rebuild_filtered();
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
    fn rebuild_filtered(&mut self) {
        self.filtered_indices.clear();
        self.search_results.clear();
        if self.filter_text.is_empty() {
            return;
        }
        let query = self.filter_text.to_lowercase();

        // Shallow filter over the current directory's entries.
        self.filtered_indices = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();

        // Recursive search results.
        Self::search_recursive(
            &self.current_dir,
            &self.current_dir,
            &query,
            &mut self.search_results,
            0,
            self.show_hidden,
        );
        // Sort by match quality: exact filename > starts-with > shorter path > alphabetical
        let q = query.clone();
        self.search_results.sort_by(|a, b| {
            fn score(name: &str, query: &str) -> u8 {
                let lower = name.to_lowercase();
                // Extract just the filename from the relative path
                let filename = name.rsplit('/').next().unwrap_or(name).to_lowercase();
                if filename == query {
                    0 // exact filename match
                } else if filename.starts_with(query) {
                    1 // filename starts with query
                } else if lower == query {
                    2 // exact path match
                } else if filename.contains(query) {
                    3 // filename contains query
                } else {
                    4 // path contains query
                }
            }
            let sa = score(&a.name, &q);
            let sb = score(&b.name, &q);
            sa.cmp(&sb)
                .then_with(|| a.name.len().cmp(&b.name.len()))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
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
                input: e.name.clone(),
                error: None,
            });
        }
    }

    /// Abandon an in-progress rename.
    pub fn cancel_rename(&mut self) {
        self.rename = None;
    }

    /// Push a character into the rename buffer (clears any prior error).
    pub fn rename_push(&mut self, c: char) {
        if let Some(r) = &mut self.rename {
            r.input.push(c);
            r.error = None;
        }
    }

    /// Delete the last character of the rename buffer.
    pub fn rename_backspace(&mut self) {
        if let Some(r) = &mut self.rename {
            r.input.pop();
            r.error = None;
        }
    }

    /// Commit the in-progress rename via `fs::rename`. On a filesystem error
    /// or name conflict the rename stays open with the message stashed in
    /// `RenameState::error`; on success (or a no-op rename) it closes.
    pub fn commit_rename(&mut self) {
        let new_name = match &self.rename {
            Some(r) => r.input.trim().to_string(),
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

            if relative.to_lowercase().contains(query) {
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

            if is_dir {
                Self::search_recursive(base, &path, query, results, depth + 1, show_hidden);
            }
        }
    }
}
