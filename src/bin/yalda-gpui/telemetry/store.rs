//! Durable, privacy-safe operational telemetry.
//!
//! The store deliberately persists only the bounded aggregate snapshots in
//! this module's sibling collectors. It never accepts transcript text, prompts,
//! tool inputs/outputs, or source contents. Mutation is in-memory only; callers
//! can clone a dirty store and write it on a background executor.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{FleetMetricSnapshot, RepositoryScan, RepositorySnapshot};

pub(crate) const TELEMETRY_STORE_VERSION: u32 = 1;
pub(crate) const AGENT_HISTORY_LIMIT: usize = 512;
pub(crate) const REPOSITORY_LIMIT: usize = 64;
pub(crate) const AGENT_SAMPLE_INTERVAL_MILLIS: u64 = 30_000;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct AgentFleetObservation {
    pub(crate) captured_at_unix_ms: u64,
    pub(crate) snapshot: FleetMetricSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RepositoryObservation {
    pub(crate) captured_at_unix_ms: u64,
    pub(crate) snapshot: RepositorySnapshot,
}

/// The complete bounded on-disk document. Clone is intentional: the GUI owns
/// and mutates one copy, then hands a clone to a background executor for save.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TelemetryStore {
    version: u32,
    #[serde(default)]
    agent_history: Vec<AgentFleetObservation>,
    #[serde(default)]
    repositories: BTreeMap<String, RepositoryObservation>,
}

impl Default for TelemetryStore {
    fn default() -> Self {
        Self {
            version: TELEMETRY_STORE_VERSION,
            agent_history: Vec::new(),
            repositories: BTreeMap::new(),
        }
    }
}

impl TelemetryStore {
    /// Load from Yalda's durable home. Missing, corrupt, unreadable, or unknown
    /// versions all fail closed to an empty current-version store.
    pub(crate) fn load() -> Self {
        let Some(path) = telemetry_store_path() else {
            return Self::default();
        };
        Self::load_path(&path)
    }

    /// Atomically replace the durable document. This performs blocking I/O and
    /// is intended for a background executor, never a GPUI render or streaming
    /// event path.
    pub(crate) fn save(&self) -> io::Result<()> {
        let Some(path) = telemetry_store_path() else {
            return Ok(());
        };
        self.save_path(&path)
    }

    pub(crate) fn version(&self) -> u32 {
        self.version
    }

    pub(crate) fn agent_history(&self) -> &[AgentFleetObservation] {
        &self.agent_history
    }

    pub(crate) fn latest_agent(&self) -> Option<&AgentFleetObservation> {
        self.agent_history.last()
    }

    pub(crate) fn repositories(&self) -> impl Iterator<Item = (&str, &RepositoryObservation)> {
        self.repositories
            .iter()
            .map(|(root, observation)| (root.as_str(), observation))
    }

    pub(crate) fn latest_repository(&self, root: &Path) -> Option<&RepositoryObservation> {
        let key = repository_root_key(root);
        self.repositories.get(&key).or_else(|| {
            let requested = Path::new(&key);
            self.repositories
                .iter()
                .filter(|(candidate, _)| requested.starts_with(Path::new(candidate.as_str())))
                .max_by_key(|(candidate, _)| candidate.len())
                .map(|(_, observation)| observation)
        })
    }

    /// Record a fleet observation without doing I/O. Exact duplicates are
    /// dropped. Ordinary metric churn is sampled at most every 30 seconds;
    /// agent membership/state transitions bypass the interval so lifecycle
    /// boundaries remain visible. `true` means the durable document changed.
    pub(crate) fn record_agent(
        &mut self,
        captured_at_unix_ms: u64,
        snapshot: FleetMetricSnapshot,
    ) -> bool {
        if let Some(previous) = self.agent_history.last() {
            if captured_at_unix_ms < previous.captured_at_unix_ms {
                return false;
            }
            if previous.snapshot == snapshot {
                return false;
            }
            let interval_elapsed = captured_at_unix_ms.saturating_sub(previous.captured_at_unix_ms)
                >= AGENT_SAMPLE_INTERVAL_MILLIS;
            if !interval_elapsed && !agent_lifecycle_changed(&previous.snapshot, &snapshot) {
                return false;
            }
        }

        self.agent_history.push(AgentFleetObservation {
            captured_at_unix_ms,
            snapshot,
        });
        if self.agent_history.len() > AGENT_HISTORY_LIMIT {
            let overflow = self.agent_history.len() - AGENT_HISTORY_LIMIT;
            self.agent_history.drain(..overflow);
        }
        true
    }

    /// Store the latest successful analysis for a generic normalized repository
    /// root. Non-git and command-error results remain transient UI states and do
    /// not erase the last successful analysis. `true` means the store changed.
    pub(crate) fn record_repository(
        &mut self,
        captured_at_unix_ms: u64,
        scan: &RepositoryScan,
    ) -> bool {
        let RepositoryScan::Ready(snapshot) = scan else {
            return false;
        };
        let key = repository_root_key(&snapshot.root);
        if self
            .repositories
            .get(&key)
            .is_some_and(|previous| captured_at_unix_ms < previous.captured_at_unix_ms)
        {
            return false;
        }

        let mut snapshot = snapshot.clone();
        snapshot.root = PathBuf::from(&key);
        let next = RepositoryObservation {
            captured_at_unix_ms,
            snapshot,
        };
        if self.repositories.get(&key) == Some(&next) {
            return false;
        }
        self.repositories.insert(key, next);
        self.enforce_repository_limit();
        true
    }

    fn enforce_repository_limit(&mut self) {
        while self.repositories.len() > REPOSITORY_LIMIT {
            let oldest = self
                .repositories
                .iter()
                .min_by_key(|(root, observation)| (observation.captured_at_unix_ms, root.as_str()))
                .map(|(root, _)| root.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.repositories.remove(&oldest);
        }
    }

    fn load_path(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(mut store) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::default();
        };
        if store.version != TELEMETRY_STORE_VERSION {
            return Self::default();
        }
        store
            .agent_history
            .sort_by_key(|item| item.captured_at_unix_ms);
        if store.agent_history.len() > AGENT_HISTORY_LIMIT {
            let overflow = store.agent_history.len() - AGENT_HISTORY_LIMIT;
            store.agent_history.drain(..overflow);
        }
        store.enforce_repository_limit();
        store
    }

    fn save_path(&self, path: &Path) -> io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "telemetry path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temp_path = atomic_temp_path(path);

        let write_result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp_path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp_path, path)?;
            // Best-effort directory fsync makes the rename durable on filesystems
            // that support opening directories. The data file is already safe if
            // this platform does not.
            let _ = File::open(parent).and_then(|directory| directory.sync_all());
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }
}

pub(crate) fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Default path under Yalda's durable (not purgeable cache) home.
pub(crate) fn telemetry_store_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return TELEMETRY_PATH_OVERRIDE.with(|path| path.borrow().clone());
    }
    #[cfg(not(test))]
    {
        yalda::paths::yalda_home().map(|home| home.join("telemetry").join("v1.json"))
    }
}

#[cfg(test)]
thread_local! {
    static TELEMETRY_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_telemetry_store_path<R>(path: PathBuf, f: impl FnOnce() -> R) -> R {
    TELEMETRY_PATH_OVERRIDE.with(|override_path| {
        let previous = override_path.replace(Some(path));
        let result = f();
        override_path.replace(previous);
        result
    })
}

pub(crate) fn repository_root_key(root: &Path) -> String {
    let absolute = fs::canonicalize(root).unwrap_or_else(|_| {
        if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("/"))
                .join(root)
        }
    });
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized.to_string_lossy().into_owned()
}

fn agent_lifecycle_changed(before: &FleetMetricSnapshot, after: &FleetMetricSnapshot) -> bool {
    if (
        before.working,
        before.ready,
        before.archived,
        before.unavailable,
    ) != (
        after.working,
        after.ready,
        after.archived,
        after.unavailable,
    ) || before.agents.len() != after.agents.len()
    {
        return true;
    }
    let before_states: BTreeMap<_, _> = before
        .agents
        .iter()
        .map(|agent| (agent.row_id.as_str(), agent.state))
        .collect();
    let after_states: BTreeMap<_, _> = after
        .agents
        .iter()
        .map(|agent| (agent.row_id.as_str(), agent.state))
        .collect();
    before_states != after_states
}

fn atomic_temp_path(path: &Path) -> PathBuf {
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("telemetry.json");
    path.with_file_name(format!(".{name}.tmp.{}.{}", std::process::id(), sequence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentMetricSnapshot, AgentMetricState, ChurnProjection, ContextOccupancy, CountProjection,
        FleetMetricAverages, LargeFileProjection, MetricAverage, PathProjection,
    };

    fn average(value: f64) -> MetricAverage {
        MetricAverage {
            sum: Some(value),
            mean: Some(value),
            denominator: 1,
            population: 1,
        }
    }

    fn fleet(state: AgentMetricState, tools: usize) -> FleetMetricSnapshot {
        let (working, ready, archived, unavailable) = match state {
            AgentMetricState::Working => (1, 0, 0, 0),
            AgentMetricState::Ready => (0, 1, 0, 0),
            AgentMetricState::Archived => (0, 0, 1, 0),
            AgentMetricState::Unavailable => (0, 0, 0, 1),
        };
        FleetMetricSnapshot {
            agents: vec![AgentMetricSnapshot {
                row_id: "agent-1".into(),
                session_id: Some("session-1".into()),
                label: "Agent one".into(),
                provider: None,
                model: Some("test-model".into()),
                state,
                settled_turns: Some(3),
                tool_total: Some(tools),
                tool_failures: Some(1),
                context: Some(ContextOccupancy {
                    used: 25,
                    capacity: 100,
                }),
                cost_usd: Some(1.25),
                current_turn_elapsed: None,
                loaded: true,
            }],
            working,
            ready,
            archived,
            unavailable,
            averages: FleetMetricAverages {
                settled_turns: average(3.0),
                tool_total: average(tools as f64),
                tool_failures: average(1.0),
                context_percent: average(25.0),
                cost_usd: average(1.25),
                current_turn_elapsed_secs: average(0.0),
            },
        }
    }

    fn repository(root: PathBuf, tracked_files: usize) -> RepositoryScan {
        RepositoryScan::Ready(RepositorySnapshot {
            root,
            head: Some("abc123".into()),
            tracked_dirty: false,
            tracked_files,
            source_files: 2,
            top_level: CountProjection {
                distinct: 0,
                items: Vec::new(),
            },
            extensions: CountProjection {
                distinct: 0,
                items: Vec::new(),
            },
            instruction_files: PathProjection {
                total: 0,
                items: Vec::new(),
            },
            workspace_manifests: PathProjection {
                total: 0,
                items: Vec::new(),
            },
            large_source_files: LargeFileProjection {
                source_files: 2,
                items: Vec::new(),
            },
            recent_churn: ChurnProjection {
                commit_limit: 500,
                commits_scanned: 1,
                distinct_paths: 0,
                items: Vec::new(),
            },
        })
    }

    #[test]
    fn save_load_reconstructs_timestamped_agent_and_repository_observations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("telemetry.json");
        let repository_root = temp.path().join("repo");
        fs::create_dir(&repository_root).unwrap();

        with_telemetry_store_path(path.clone(), || {
            let mut store = TelemetryStore::default();
            assert!(store.record_agent(1_000, fleet(AgentMetricState::Working, 2)));
            assert!(store.record_repository(2_000, &repository(repository_root.clone(), 7)));
            store.save().unwrap();

            let restored = TelemetryStore::load();
            assert_eq!(restored, store);
            assert_eq!(restored.latest_agent().unwrap().captured_at_unix_ms, 1_000);
            let restored_repository = restored.latest_repository(&repository_root).unwrap();
            assert_eq!(restored_repository.captured_at_unix_ms, 2_000);
            assert_eq!(restored_repository.snapshot.tracked_files, 7);
            assert_eq!(restored.version(), TELEMETRY_STORE_VERSION);

            let entries: Vec<_> = fs::read_dir(path.parent().unwrap())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(entries.len(), 1, "atomic temp file is not left behind");
        });
    }

    #[test]
    fn test_build_never_uses_the_real_telemetry_path_without_an_override() {
        assert_eq!(telemetry_store_path(), None);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("telemetry.json");
        with_telemetry_store_path(path.clone(), || {
            assert_eq!(telemetry_store_path(), Some(path));
        });
        assert_eq!(telemetry_store_path(), None);
    }

    #[test]
    fn agent_history_is_coalesced_and_retained_at_a_fixed_bound() {
        let mut store = TelemetryStore::default();
        let first = fleet(AgentMetricState::Working, 1);
        assert!(store.record_agent(0, first.clone()));
        assert!(!store.record_agent(1, first), "exact duplicate is ignored");
        assert!(
            !store.record_agent(2, fleet(AgentMetricState::Working, 2)),
            "ordinary streaming metric churn is throttled"
        );
        assert!(
            store.record_agent(3, fleet(AgentMetricState::Ready, 2)),
            "lifecycle transition bypasses throttle"
        );
        assert!(
            store.record_agent(4, fleet(AgentMetricState::Unavailable, 2)),
            "becoming unavailable bypasses throttle"
        );
        assert!(
            store.record_agent(5, fleet(AgentMetricState::Archived, 2)),
            "unavailable-to-archived remains a durable lifecycle boundary"
        );

        for index in 1..=(AGENT_HISTORY_LIMIT + 9) {
            let timestamp = 5 + index as u64 * AGENT_SAMPLE_INTERVAL_MILLIS;
            assert!(store.record_agent(timestamp, fleet(AgentMetricState::Ready, index + 10)));
        }
        assert_eq!(store.agent_history().len(), AGENT_HISTORY_LIMIT);
        assert_eq!(
            store.latest_agent().unwrap().snapshot.agents[0].tool_total,
            Some(AGENT_HISTORY_LIMIT + 19)
        );
    }

    #[test]
    fn repository_entries_are_latest_by_normalized_root_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = TelemetryStore::default();
        let primary = temp.path().join("primary");
        fs::create_dir(&primary).unwrap();
        assert!(store.record_repository(10, &repository(primary.join("."), 1)));
        assert!(store.record_repository(11, &repository(primary.clone(), 2)));
        assert_eq!(
            store
                .latest_repository(&primary)
                .unwrap()
                .snapshot
                .tracked_files,
            2
        );
        let nested_project = primary.join("services").join("api");
        fs::create_dir_all(&nested_project).unwrap();
        assert_eq!(
            store
                .latest_repository(&nested_project)
                .unwrap()
                .snapshot
                .tracked_files,
            2,
            "a project cwd below a repository root restores that repository"
        );

        for index in 0..=REPOSITORY_LIMIT {
            let root = temp.path().join(format!("repository-{index}"));
            assert!(store.record_repository(index as u64 + 100, &repository(root, index)));
        }
        assert_eq!(store.repositories().count(), REPOSITORY_LIMIT);
        assert!(
            store.latest_repository(&primary).is_none(),
            "oldest repository is evicted deterministically"
        );
    }

    #[test]
    fn missing_corrupt_and_unknown_versions_fail_safe() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("telemetry.json");
        with_telemetry_store_path(path.clone(), || {
            assert_eq!(TelemetryStore::load(), TelemetryStore::default());
            fs::write(&path, b"not json").unwrap();
            assert_eq!(TelemetryStore::load(), TelemetryStore::default());
            fs::write(
                &path,
                br#"{"version":999,"agent_history":[],"repositories":{}}"#,
            )
            .unwrap();
            assert_eq!(TelemetryStore::load(), TelemetryStore::default());
        });
    }

    #[test]
    fn v1_documents_without_archived_count_remain_readable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("telemetry.json");
        with_telemetry_store_path(path.clone(), || {
            let mut store = TelemetryStore::default();
            assert!(store.record_agent(1_000, fleet(AgentMetricState::Ready, 2)));
            let mut document = serde_json::to_value(&store).unwrap();
            document["agent_history"][0]["snapshot"]
                .as_object_mut()
                .unwrap()
                .remove("archived");
            fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

            let restored = TelemetryStore::load();
            let snapshot = &restored.latest_agent().unwrap().snapshot;
            assert_eq!(snapshot.ready, 1);
            assert_eq!(snapshot.archived, 0);
            assert_eq!(snapshot.agents[0].state, AgentMetricState::Ready);
        });
    }

    #[test]
    fn failed_repository_scan_does_not_erase_last_success() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let mut store = TelemetryStore::default();
        assert!(store.record_repository(1, &repository(root.clone(), 9)));
        assert!(!store.record_repository(2, &RepositoryScan::NotGit { cwd: root.clone() }));
        assert_eq!(
            store
                .latest_repository(&root)
                .unwrap()
                .snapshot
                .tracked_files,
            9
        );
    }

    #[test]
    fn persisted_repository_analysis_never_contains_source_contents() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "--quiet"])
            .status()
            .unwrap();
        assert!(status.success());
        fs::write(root.join("private.rs"), "unique-source-secret-7823").unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "private.rs"])
            .status()
            .unwrap();
        assert!(status.success());
        let scan = crate::scan_repository(&root);
        let path = temp.path().join("telemetry.json");

        with_telemetry_store_path(path.clone(), || {
            let mut store = TelemetryStore::default();
            assert!(store.record_repository(1, &scan));
            store.save().unwrap();
        });
        let bytes = fs::read_to_string(path).unwrap();
        assert!(!bytes.contains("unique-source-secret-7823"));
        assert!(bytes.contains("private.rs"), "metadata path remains useful");
    }
}
