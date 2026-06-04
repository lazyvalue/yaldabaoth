use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    /// Branch name, or "(detached)" / "(bare)" for special states.
    pub label: String,
    /// True if `current_dir` is inside this worktree's path.
    pub is_current: bool,
}

/// List git worktrees by running `git worktree list --porcelain`.
/// Returns an empty vec if not in a git repo or git is unavailable.
pub fn list_worktrees() -> Vec<Worktree> {
    let output = match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    parse_porcelain(&text)
}

fn parse_porcelain(text: &str) -> Vec<Worktree> {
    let mut result = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut label: Option<String> = None;
    let mut is_bare = false;

    // Records are separated by blank lines. Append an empty sentinel so the
    // last record is flushed even if the output doesn't end with a newline.
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(p) = path.take() {
                let lbl = if is_bare {
                    "(bare)".to_string()
                } else {
                    label.take().unwrap_or_else(|| "(detached)".to_string())
                };
                result.push(Worktree {
                    path: p,
                    label: lbl,
                    is_current: false,
                });
            }
            is_bare = false;
            label = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            // A new record may start without a preceding blank line in some
            // git versions — flush any in-progress record first.
            if let Some(p) = path.take() {
                let lbl = if is_bare {
                    "(bare)".to_string()
                } else {
                    label.take().unwrap_or_else(|| "(detached)".to_string())
                };
                result.push(Worktree {
                    path: p,
                    label: lbl,
                    is_current: false,
                });
                is_bare = false;
            }
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
            label = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            // Fallback for refs that aren't under refs/heads/
            label = Some(rest.to_string());
        } else if line == "bare" {
            is_bare = true;
        }
        // "detached" lines are handled implicitly — no branch → "(detached)"
    }
    result
}

/// Set `is_current` on worktrees whose path is a prefix of `current_dir`.
pub fn mark_current(worktrees: &mut [Worktree], current_dir: &Path) {
    for wt in worktrees.iter_mut() {
        wt.is_current = current_dir.starts_with(&wt.path);
    }
}

/// Index of the worktree that best matches `current_dir` (longest prefix).
/// Returns 0 if nothing matches.
pub fn best_match_index(worktrees: &[Worktree], current_dir: &Path) -> usize {
    worktrees
        .iter()
        .enumerate()
        .filter(|(_, wt)| current_dir.starts_with(&wt.path))
        .max_by_key(|(_, wt)| wt.path.as_os_str().len())
        .map(|(i, _)| i)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_porcelain() {
        let input = "\
worktree /home/user/repo
branch refs/heads/main

worktree /home/user/repo-feat
branch refs/heads/feat-x

";
        let wts = parse_porcelain(input);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].label, "main");
        assert_eq!(wts[0].path, PathBuf::from("/home/user/repo"));
        assert_eq!(wts[1].label, "feat-x");
    }

    #[test]
    fn parse_detached_and_bare() {
        let input = "\
worktree /repo
bare

worktree /repo-detached
HEAD abc123
detached

";
        let wts = parse_porcelain(input);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].label, "(bare)");
        assert_eq!(wts[1].label, "(detached)");
    }

    #[test]
    fn best_match_longest_prefix() {
        let wts = vec![
            Worktree { path: PathBuf::from("/repo"), label: "main".into(), is_current: false },
            Worktree { path: PathBuf::from("/repo/.claude/worktrees/feat"), label: "feat".into(), is_current: false },
        ];
        assert_eq!(best_match_index(&wts, Path::new("/repo/.claude/worktrees/feat/src")), 1);
        assert_eq!(best_match_index(&wts, Path::new("/repo/src")), 0);
        assert_eq!(best_match_index(&wts, Path::new("/elsewhere")), 0);
    }
}
