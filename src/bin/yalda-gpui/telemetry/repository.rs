use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DISTRIBUTION_LIMIT: usize = 12;
const PATH_LIST_LIMIT: usize = 32;
const LARGE_FILE_LIMIT: usize = 20;
const CHURN_LIMIT: usize = 20;
const HISTORY_COMMIT_LIMIT: usize = 500;
const MAX_DISPLAY_PATH_CHARS: usize = 240;
const MAX_DISTRIBUTION_LABEL_CHARS: usize = 80;
const MAX_ERROR_CHARS: usize = 240;
const MAX_LINE_COUNT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RepositoryScan {
    Ready(RepositorySnapshot),
    NotGit { cwd: PathBuf },
    CommandError(RepositoryCommandError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepositoryCommandError {
    pub(crate) operation: RepositoryOperation,
    pub(crate) detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepositoryOperation {
    ResolveRoot,
    ListTrackedFiles,
    ReadStatus,
    ReadHead,
    ReadHistory,
    CountHistory,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RepositorySnapshot {
    pub(crate) root: PathBuf,
    pub(crate) head: Option<String>,
    pub(crate) tracked_dirty: bool,
    pub(crate) tracked_files: usize,
    pub(crate) source_files: usize,
    pub(crate) top_level: CountProjection,
    pub(crate) extensions: CountProjection,
    pub(crate) instruction_files: PathProjection,
    pub(crate) workspace_manifests: PathProjection,
    pub(crate) large_source_files: LargeFileProjection,
    pub(crate) recent_churn: ChurnProjection,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct NamedCount {
    pub(crate) label: String,
    pub(crate) count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CountProjection {
    pub(crate) distinct: usize,
    pub(crate) items: Vec<NamedCount>,
}

impl CountProjection {
    pub(crate) fn omitted(&self) -> usize {
        self.distinct.saturating_sub(self.items.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PathProjection {
    pub(crate) total: usize,
    pub(crate) items: Vec<String>,
}

impl PathProjection {
    pub(crate) fn omitted(&self) -> usize {
        self.total.saturating_sub(self.items.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct LargeTrackedFile {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    /// Physical lines. `None` means the file exceeded the bounded read limit
    /// or changed while the scan ran.
    pub(crate) lines: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct LargeFileProjection {
    pub(crate) source_files: usize,
    pub(crate) items: Vec<LargeTrackedFile>,
}

impl LargeFileProjection {
    pub(crate) fn omitted(&self) -> usize {
        self.source_files.saturating_sub(self.items.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ChurnProjection {
    pub(crate) commit_limit: usize,
    pub(crate) commits_scanned: usize,
    pub(crate) distinct_paths: usize,
    pub(crate) items: Vec<NamedCount>,
}

impl ChurnProjection {
    pub(crate) fn omitted(&self) -> usize {
        self.distinct_paths.saturating_sub(self.items.len())
    }
}

#[derive(Clone, Copy)]
struct ProjectionLimits {
    distribution: usize,
    paths: usize,
    large_files: usize,
    churn: usize,
}

impl Default for ProjectionLimits {
    fn default() -> Self {
        Self {
            distribution: DISTRIBUTION_LIMIT,
            paths: PATH_LIST_LIMIT,
            large_files: LARGE_FILE_LIMIT,
            churn: CHURN_LIMIT,
        }
    }
}

#[derive(Debug)]
struct TrackedProjection {
    tracked_files: usize,
    source_files: usize,
    top_level: CountProjection,
    extensions: CountProjection,
    instruction_files: PathProjection,
    workspace_manifests: PathProjection,
}

struct LargeFileCandidate {
    full_path: PathBuf,
    display_path: String,
    bytes: u64,
}

pub(crate) fn scan_repository(cwd: &Path) -> RepositoryScan {
    if !cwd.is_dir() {
        return RepositoryScan::CommandError(RepositoryCommandError {
            operation: RepositoryOperation::ResolveRoot,
            detail: "the project directory is unavailable".to_string(),
        });
    }

    let root_output = match run_git(
        cwd,
        RepositoryOperation::ResolveRoot,
        &["rev-parse", "--show-toplevel"],
    ) {
        Ok(output) => output,
        Err(error) => return RepositoryScan::CommandError(error),
    };
    if !root_output.status.success() {
        if is_not_git_error(&root_output) {
            return RepositoryScan::NotGit {
                cwd: cwd.to_path_buf(),
            };
        }
        return RepositoryScan::CommandError(failed_command(
            RepositoryOperation::ResolveRoot,
            &root_output,
        ));
    }

    let root_text = String::from_utf8_lossy(&root_output.stdout);
    let root = PathBuf::from(root_text.trim_end_matches(['\r', '\n']));
    if root.as_os_str().is_empty() {
        return RepositoryScan::CommandError(RepositoryCommandError {
            operation: RepositoryOperation::ResolveRoot,
            detail: "git returned no repository root".to_string(),
        });
    }

    let tracked_output = match successful_git(
        &root,
        RepositoryOperation::ListTrackedFiles,
        &["ls-files", "-z"],
    ) {
        Ok(output) => output,
        Err(error) => return RepositoryScan::CommandError(error),
    };
    let limits = ProjectionLimits::default();
    let tracked = project_tracked_paths(&tracked_output.stdout, limits);
    let large_source_files = find_large_source_files(&root, &tracked_output.stdout, limits);

    let status_output = match successful_git(
        &root,
        RepositoryOperation::ReadStatus,
        &["status", "--porcelain=v1", "-z", "--untracked-files=no"],
    ) {
        Ok(output) => output,
        Err(error) => return RepositoryScan::CommandError(error),
    };

    let head_output = match run_git(
        &root,
        RepositoryOperation::ReadHead,
        &["rev-parse", "--verify", "--quiet", "HEAD"],
    ) {
        Ok(output) => output,
        Err(error) => return RepositoryScan::CommandError(error),
    };
    let head = if head_output.status.success() {
        Some(
            String::from_utf8_lossy(&head_output.stdout)
                .trim()
                .chars()
                .take(64)
                .collect(),
        )
    } else if head_output.status.code() == Some(1) {
        None
    } else {
        return RepositoryScan::CommandError(failed_command(
            RepositoryOperation::ReadHead,
            &head_output,
        ));
    };

    let recent_churn = if head.is_some() {
        match scan_history(&root, limits) {
            Ok(churn) => churn,
            Err(error) => return RepositoryScan::CommandError(error),
        }
    } else {
        ChurnProjection {
            commit_limit: HISTORY_COMMIT_LIMIT,
            commits_scanned: 0,
            distinct_paths: 0,
            items: Vec::new(),
        }
    };

    RepositoryScan::Ready(RepositorySnapshot {
        root,
        head,
        tracked_dirty: !status_output.stdout.is_empty(),
        tracked_files: tracked.tracked_files,
        source_files: tracked.source_files,
        top_level: tracked.top_level,
        extensions: tracked.extensions,
        instruction_files: tracked.instruction_files,
        workspace_manifests: tracked.workspace_manifests,
        large_source_files,
        recent_churn,
    })
}

fn scan_history(
    root: &Path,
    limits: ProjectionLimits,
) -> Result<ChurnProjection, RepositoryCommandError> {
    let limit = HISTORY_COMMIT_LIMIT.to_string();
    let history_output = successful_git(
        root,
        RepositoryOperation::ReadHistory,
        &[
            "log",
            "--name-only",
            "--format=",
            "-z",
            "-n",
            limit.as_str(),
            "HEAD",
            "--",
        ],
    )?;
    let count_output = successful_git(
        root,
        RepositoryOperation::CountHistory,
        &[
            "rev-list",
            "--count",
            &format!("--max-count={HISTORY_COMMIT_LIMIT}"),
            "HEAD",
        ],
    )?;
    let commits_scanned = String::from_utf8_lossy(&count_output.stdout)
        .trim()
        .parse()
        .map_err(|_| RepositoryCommandError {
            operation: RepositoryOperation::CountHistory,
            detail: "git returned an invalid history count".to_string(),
        })?;
    let mut churn = project_churn(&history_output.stdout, limits.churn);
    churn.commit_limit = HISTORY_COMMIT_LIMIT;
    churn.commits_scanned = commits_scanned;
    Ok(churn)
}

fn run_git(
    cwd: &Path,
    operation: RepositoryOperation,
    args: &[&str],
) -> Result<Output, RepositoryCommandError> {
    Command::new("git")
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|error| RepositoryCommandError {
            operation,
            detail: bounded_text(&error.to_string(), MAX_ERROR_CHARS),
        })
}

fn successful_git(
    cwd: &Path,
    operation: RepositoryOperation,
    args: &[&str],
) -> Result<Output, RepositoryCommandError> {
    let output = run_git(cwd, operation, args)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(failed_command(operation, &output))
    }
}

fn failed_command(operation: RepositoryOperation, output: &Output) -> RepositoryCommandError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.lines().next().unwrap_or("git command failed");
    RepositoryCommandError {
        operation,
        detail: bounded_text(detail, MAX_ERROR_CHARS),
    }
}

fn is_not_git_error(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stderr)
        .to_ascii_lowercase()
        .contains("not a git repository")
}

fn project_tracked_paths(bytes: &[u8], limits: ProjectionLimits) -> TrackedProjection {
    let mut tracked_files = 0;
    let mut source_files = 0;
    let mut top_level = HashMap::new();
    let mut extensions = HashMap::new();
    let mut instruction_files = Vec::new();
    let mut instruction_total = 0;
    let mut workspace_manifests = Vec::new();
    let mut manifest_total = 0;

    for path in nul_records(bytes) {
        tracked_files += 1;
        let display_path = bounded_path(path);
        let top = path.split('/').next().filter(|_| path.contains('/'));
        *top_level
            .entry(bounded_text(
                top.unwrap_or("(root)"),
                MAX_DISTRIBUTION_LABEL_CHARS,
            ))
            .or_insert(0) += 1;

        let extension = extension_of(path)
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| "(none)".to_string());
        *extensions.entry(extension.clone()).or_insert(0) += 1;
        if is_source_extension(&extension) {
            source_files += 1;
        }

        if is_instruction_file(path) {
            instruction_total += 1;
            push_lexical_bounded(&mut instruction_files, display_path.clone(), limits.paths);
        }
        if is_workspace_manifest(path) {
            manifest_total += 1;
            push_lexical_bounded(&mut workspace_manifests, display_path, limits.paths);
        }
    }

    TrackedProjection {
        tracked_files,
        source_files,
        top_level: project_counts(top_level, limits.distribution),
        extensions: project_counts(extensions, limits.distribution),
        instruction_files: PathProjection {
            total: instruction_total,
            items: instruction_files,
        },
        workspace_manifests: PathProjection {
            total: manifest_total,
            items: workspace_manifests,
        },
    }
}

fn project_churn(bytes: &[u8], limit: usize) -> ChurnProjection {
    let mut counts = HashMap::new();
    for path in nul_records(bytes) {
        *counts.entry(path.to_string()).or_insert(0) += 1;
    }
    let mut projected = project_counts(counts, limit);
    for item in &mut projected.items {
        item.label = bounded_path(&item.label);
    }
    ChurnProjection {
        commit_limit: 0,
        commits_scanned: 0,
        distinct_paths: projected.distinct,
        items: projected.items,
    }
}

fn find_large_source_files(
    root: &Path,
    tracked: &[u8],
    limits: ProjectionLimits,
) -> LargeFileProjection {
    let mut source_files = 0;
    let mut candidates = Vec::new();
    for path in nul_records(tracked) {
        let Some(extension) = extension_of(path) else {
            continue;
        };
        if !is_source_extension(extension) {
            continue;
        }
        source_files += 1;

        let full_path = root.join(path);
        let Ok(metadata) = full_path.symlink_metadata() else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let candidate = LargeFileCandidate {
            full_path,
            display_path: bounded_path(path),
            bytes: metadata.len(),
        };
        push_large_candidate(&mut candidates, candidate, limits.large_files);
    }

    candidates.sort_by(|left, right| large_candidate_rank(right, left));
    let items = candidates
        .into_iter()
        .map(|candidate| LargeTrackedFile {
            path: candidate.display_path,
            bytes: candidate.bytes,
            lines: count_lines_bounded(&candidate.full_path, candidate.bytes),
        })
        .collect();

    LargeFileProjection {
        source_files,
        items,
    }
}

fn push_large_candidate(
    candidates: &mut Vec<LargeFileCandidate>,
    candidate: LargeFileCandidate,
    limit: usize,
) {
    if limit == 0 {
        return;
    }
    if candidates.len() < limit {
        candidates.push(candidate);
        return;
    }
    let Some((smallest_index, smallest)) = candidates
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| large_candidate_rank(left, right))
    else {
        return;
    };
    if large_candidate_rank(&candidate, smallest).is_gt() {
        candidates[smallest_index] = candidate;
    }
}

fn large_candidate_rank(left: &LargeFileCandidate, right: &LargeFileCandidate) -> Ordering {
    left.bytes
        .cmp(&right.bytes)
        // For equal sizes, a lexically earlier path has the higher rank.
        .then_with(|| right.display_path.cmp(&left.display_path))
}

fn count_lines_bounded(path: &Path, expected_bytes: u64) -> Option<usize> {
    if expected_bytes > MAX_LINE_COUNT_BYTES {
        return None;
    }
    let file = File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(expected_bytes as usize + 1);
    file.take(MAX_LINE_COUNT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_LINE_COUNT_BYTES {
        return None;
    }
    let newline_count = bytes.iter().filter(|byte| **byte == b'\n').count();
    Some(newline_count + usize::from(!bytes.is_empty() && bytes.last() != Some(&b'\n')))
}

fn nul_records(bytes: &[u8]) -> impl Iterator<Item = &str> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| std::str::from_utf8(record).unwrap_or("<non-utf8-path>"))
}

fn project_counts(counts: HashMap<String, usize>, limit: usize) -> CountProjection {
    let distinct = counts.len();
    let mut items: Vec<_> = counts
        .into_iter()
        .map(|(label, count)| NamedCount { label, count })
        .collect();
    items.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.label.cmp(&right.label))
    });
    items.truncate(limit);
    CountProjection { distinct, items }
}

fn push_lexical_bounded(items: &mut Vec<String>, value: String, limit: usize) {
    items.push(value);
    items.sort();
    items.dedup();
    items.truncate(limit);
}

fn bounded_path(path: &str) -> String {
    bounded_text(path, MAX_DISPLAY_PATH_CHARS)
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    let mut result: String = value
        .chars()
        .filter_map(|character| {
            if character == '\n' || character == '\r' || character == '\0' {
                Some(' ')
            } else if character.is_control() {
                None
            } else {
                Some(character)
            }
        })
        .take(max_chars)
        .collect();
    if value.chars().count() > max_chars {
        result.push('…');
    }
    result
}

fn extension_of(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    let (_, extension) = name.rsplit_once('.')?;
    (!extension.is_empty()).then_some(extension)
}

fn is_source_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "rs" | "py"
            | "pyi"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "cxx"
            | "hpp"
            | "swift"
            | "rb"
            | "php"
            | "ex"
            | "exs"
            | "erl"
            | "fs"
            | "fsx"
            | "cs"
            | "scala"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "sql"
            | "tf"
            | "hcl"
            | "lua"
            | "r"
            | "dart"
            | "vue"
            | "svelte"
    )
}

fn is_instruction_file(path: &str) -> bool {
    matches!(
        path.rsplit('/').next(),
        Some("AGENTS.md" | "CLAUDE.md" | "GEMINI.md" | ".cursorrules")
    ) || path == ".github/copilot-instructions.md"
}

fn is_workspace_manifest(path: &str) -> bool {
    matches!(
        path.rsplit('/').next(),
        Some(
            "Cargo.toml"
                | "pyproject.toml"
                | "package.json"
                | "pnpm-workspace.yaml"
                | "go.mod"
                | "go.work"
                | "Package.swift"
                | "Gemfile"
                | "pom.xml"
                | "build.gradle"
                | "build.gradle.kts"
                | "settings.gradle"
                | "settings.gradle.kts"
                | "deno.json"
                | "deno.jsonc"
                | "bun.lock"
                | "composer.json"
                | "mix.exs"
                | "WORKSPACE"
                | "MODULE.bazel"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_limits() -> ProjectionLimits {
        ProjectionLimits {
            distribution: 2,
            paths: 2,
            large_files: 2,
            churn: 2,
        }
    }

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .status()
            .expect("git must run");
        assert!(status.success(), "git command failed: {args:?}");
    }

    #[test]
    fn tracked_projection_is_deterministic_and_bounded() {
        let paths = b"src/main.rs\0src/lib.rs\0tests/main.rs\0docs/guide.md\0Cargo.toml\0\
                      nested/CLAUDE.md\0AGENTS.md\0web/package.json\0scripts/run.sh\0";
        let projection = project_tracked_paths(paths, test_limits());

        assert_eq!(projection.tracked_files, 9);
        assert_eq!(projection.source_files, 4);
        assert_eq!(projection.top_level.items.len(), 2);
        assert_eq!(projection.top_level.items[0].label, "(root)");
        assert_eq!(projection.top_level.items[0].count, 2);
        assert_eq!(projection.top_level.items[1].label, "src");
        assert_eq!(projection.top_level.items[1].count, 2);
        assert!(projection.top_level.omitted() > 0);
        assert_eq!(projection.extensions.items.len(), 2);
        assert_eq!(projection.instruction_files.total, 2);
        assert_eq!(
            projection.instruction_files.items,
            vec!["AGENTS.md", "nested/CLAUDE.md"]
        );
        assert_eq!(projection.workspace_manifests.total, 2);
        assert_eq!(projection.workspace_manifests.items.len(), 2);
    }

    #[test]
    fn churn_projection_counts_touches_and_bounds_rows() {
        let history = b"src/main.rs\0src/lib.rs\0src/main.rs\0README.md\0README.md\0README.md\0";
        let projection = project_churn(history, test_limits().churn);

        assert_eq!(projection.distinct_paths, 3);
        assert_eq!(projection.items.len(), 2);
        assert_eq!(
            projection.items,
            vec![
                NamedCount {
                    label: "README.md".to_string(),
                    count: 3,
                },
                NamedCount {
                    label: "src/main.rs".to_string(),
                    count: 2,
                },
            ]
        );
        assert_eq!(projection.omitted(), 1);
    }

    #[test]
    fn scan_never_copies_file_content_into_snapshot() {
        let directory = tempfile::tempdir().expect("tempdir");
        git(directory.path(), &["init", "--quiet"]);
        fs::create_dir(directory.path().join("src")).expect("source directory");
        let secret = "do-not-copy-this-secret-payload";
        fs::write(directory.path().join("src/private.rs"), secret).expect("source file");
        let long_relative = format!(
            "src/{}/{}/{}/large.rs",
            "a".repeat(78),
            "b".repeat(78),
            "c".repeat(78)
        );
        let long_path = directory.path().join(&long_relative);
        fs::create_dir_all(long_path.parent().expect("long path parent"))
            .expect("long source directory");
        fs::write(&long_path, "line one\nline two\nline three\n").expect("long source file");
        fs::write(directory.path().join("AGENTS.md"), "private instructions")
            .expect("instruction file");
        git(directory.path(), &["add", "."]);

        let RepositoryScan::Ready(snapshot) = scan_repository(directory.path()) else {
            panic!("initialized repository must scan");
        };

        assert_eq!(snapshot.tracked_files, 3);
        assert_eq!(snapshot.source_files, 2);
        let long_item = snapshot
            .large_source_files
            .items
            .iter()
            .find(|item| item.path.ends_with('…'))
            .expect("long display path is bounded");
        assert_eq!(long_item.lines, Some(3));
        assert!(!format!("{snapshot:?}").contains(secret));
        assert!(!format!("{snapshot:?}").contains("private instructions"));
    }

    #[test]
    fn large_file_candidate_selection_keeps_only_the_highest_ranked_items() {
        let mut candidates = Vec::new();
        for (path, bytes) in [("c.rs", 30), ("b.rs", 10), ("a.rs", 30), ("d.rs", 20)] {
            push_large_candidate(
                &mut candidates,
                LargeFileCandidate {
                    full_path: PathBuf::from(path),
                    display_path: path.to_string(),
                    bytes,
                },
                2,
            );
        }
        candidates.sort_by(|left, right| large_candidate_rank(right, left));

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.display_path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.rs", "c.rs"]
        );
    }

    #[test]
    fn non_git_directory_has_an_explicit_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            scan_repository(directory.path()),
            RepositoryScan::NotGit { .. }
        ));
    }

    #[test]
    fn missing_directory_is_a_command_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        let missing = directory.path().join("missing");
        assert!(matches!(
            scan_repository(&missing),
            RepositoryScan::CommandError(RepositoryCommandError {
                operation: RepositoryOperation::ResolveRoot,
                ..
            })
        ));
    }

    #[test]
    fn qualify_real_repository_from_environment() {
        let Some(path) = std::env::var_os("YALDA_REPOSITORY_QUALIFY") else {
            return;
        };
        let RepositoryScan::Ready(snapshot) = scan_repository(Path::new(&path)) else {
            panic!("qualification repository must scan");
        };
        assert!(snapshot.tracked_files > 0);
        assert!(!snapshot.top_level.items.is_empty());
        assert!(!snapshot.extensions.items.is_empty());
        assert!(!snapshot.workspace_manifests.items.is_empty());
        eprintln!("repository qualification: {snapshot:#?}");
    }
}
