//! Optional, capability-gated supervision for Cog runtime delivery.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::SessionManager;
use yalda::acp_channel::AgentProvider;
use yalda::cog_runtime::{
    ActivationStatus, CapabilityStatus, ClaimBatch, CogClient, CoordinatorConfig, DecimalU64,
    DeliveryCoordinator, DeliveryJournal, OpaqueId, ProtocolOne, ProtocolUuid,
    ProviderDeliveryResult, ProviderKind, RuntimeRoute, UreqTransport,
};

const DEFAULT_HOST_LEASE_SECONDS: u64 = 60;
const DEFAULT_ATTEMPT_LEASE_SECONDS: u64 = 60;
const DEFAULT_MAX_ATTEMPTS: u64 = 4;
const DEFAULT_MAX_ENTRIES: u64 = 100;
const DEFAULT_MAX_CONTENT_BYTES: u64 = 1_000_000;
const CLAIM_REVALIDATE: Duration = Duration::from_secs(5);
const CAPABILITY_REVALIDATE: Duration = Duration::from_secs(5 * 60);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

type Coordinator = DeliveryCoordinator<UreqTransport>;
type SharedCoordinator = Arc<Mutex<Coordinator>>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(rename = "schema_version")]
    _schema_version: ProtocolOne,
    cog_url: String,
    host_id: OpaqueId,
    #[serde(default)]
    allow_takeover: bool,
    addresses: Vec<AddressConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddressConfig {
    address_id: OpaqueId,
    yalda_session_id: String,
    provider: ProviderKind,
}

#[derive(Debug, Clone)]
struct LoadedConfig {
    cog_url: String,
    journal_path: PathBuf,
    wake_cursor_path: PathBuf,
    coordinator: CoordinatorConfig,
}

impl ConfigFile {
    fn load(path: &Path) -> Result<LoadedConfig, String> {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("reading {} failed: {error}", path.display()))?;
        let parsed: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parsing {} failed: {error}", path.display()))?;
        parsed.validate(path)
    }

    fn validate(self, path: &Path) -> Result<LoadedConfig, String> {
        if self.addresses.is_empty() {
            return Err("addresses must select at least one Cog address".into());
        }
        let mut address_ids = BTreeSet::new();
        let mut session_ids = BTreeSet::new();
        let mut routes = Vec::with_capacity(self.addresses.len());
        for address in self.addresses {
            if address.yalda_session_id.trim().is_empty()
                || address.yalda_session_id.trim() != address.yalda_session_id
            {
                return Err(
                    "yalda_session_id must be nonempty with no surrounding whitespace".into(),
                );
            }
            if !address_ids.insert(address.address_id.clone()) {
                return Err(format!("duplicate Cog address_id {}", address.address_id));
            }
            if !session_ids.insert(address.yalda_session_id.clone()) {
                return Err(format!(
                    "multiple Cog addresses map to Yalda session {}",
                    address.yalda_session_id
                ));
            }
            routes.push(RuntimeRoute {
                address_id: address.address_id,
                server_session_id: address.yalda_session_id,
                provider: address.provider,
            });
        }
        let journal_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("cog-runtime-journal.ndjson");
        let wake_cursor_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("cog-runtime-wake.cursor");
        Ok(LoadedConfig {
            cog_url: self.cog_url,
            journal_path,
            wake_cursor_path,
            coordinator: CoordinatorConfig {
                host_id: self.host_id,
                instance_id: ProtocolUuid::from_uuid(uuid::Uuid::new_v4()),
                routes,
                host_lease_seconds: DecimalU64(DEFAULT_HOST_LEASE_SECONDS),
                attempt_lease_seconds: DecimalU64(DEFAULT_ATTEMPT_LEASE_SECONDS),
                max_attempts: DecimalU64(DEFAULT_MAX_ATTEMPTS),
                max_entries: DecimalU64(DEFAULT_MAX_ENTRIES),
                max_content_bytes: DecimalU64(DEFAULT_MAX_CONTENT_BYTES),
                // Selecting an address in this file is the explicit operator
                // authorization to CAS its durable owner to this host.
                reconcile_ownership: true,
                takeover_live_host: self.allow_takeover,
            },
        })
    }
}

pub(super) struct RuntimeAdapterHandle {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RuntimeAdapterHandle {
    pub(super) async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let mut task = self.task;
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut task)
            .await
            .is_err()
        {
            tracing::error!("Cog runtime adapter shutdown timed out; aborting supervisor");
            task.abort();
        }
    }
}

pub(super) fn spawn_if_configured(manager: Arc<SessionManager>) -> Option<RuntimeAdapterHandle> {
    let path = configured_path()?;
    let config = match ConfigFile::load(&path) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "Cog runtime adapter inactive: invalid configuration");
            return None;
        }
    };
    let (shutdown, shutdown_rx) = watch::channel(false);
    let task = tokio::spawn(run_supervisor(config, manager, shutdown_rx));
    Some(RuntimeAdapterHandle { shutdown, task })
}

fn configured_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("YALDA_COG_RUNTIME_CONFIG") {
        return Some(PathBuf::from(path));
    }
    yalda::paths::yalda_home()
        .map(|home| home.join("cog-runtime.json"))
        .filter(|path| path.exists())
}

async fn run_supervisor(
    config: LoadedConfig,
    manager: Arc<SessionManager>,
    mut shutdown: watch::Receiver<bool>,
) {
    if let Err(error) = validate_session_routes(&config, &manager).await {
        tracing::error!(%error, "Cog runtime adapter inactive: route validation failed");
        return;
    }

    let build_config = config.clone();
    let coordinator = match tokio::task::spawn_blocking(move || {
        let transport = UreqTransport::new(&build_config.cog_url)
            .map_err(|error| format!("creating Cog transport failed: {error}"))?;
        read_wake_cursor(&build_config.wake_cursor_path)?;
        let journal = DeliveryJournal::open(&build_config.journal_path)
            .map_err(|error| format!("opening Cog delivery journal failed: {error}"))?;
        Ok::<_, String>(Arc::new(Mutex::new(DeliveryCoordinator::new(
            CogClient::new(transport),
            build_config.coordinator,
            journal,
        ))))
    })
    .await
    {
        Ok(Ok(coordinator)) => coordinator,
        Ok(Err(error)) => {
            tracing::error!(%error, "Cog runtime adapter inactive: initialization failed");
            return;
        }
        Err(error) => {
            tracing::error!(%error, "Cog runtime adapter inactive: initialization task failed");
            return;
        }
    };

    let mut retry = Duration::from_secs(1);
    loop {
        if *shutdown.borrow() {
            shutdown_coordinator(&coordinator).await;
            return;
        }
        match coordinator_call(&coordinator, |coordinator| coordinator.activate()).await {
            Ok(ActivationStatus::Active {
                host_fence,
                eligible_addresses,
            }) => {
                tracing::info!(
                    host_fence = %host_fence,
                    eligible_addresses = eligible_addresses.len(),
                    "Cog runtime adapter active"
                );
                let shutdown_requested = run_active(
                    Arc::clone(&coordinator),
                    Arc::clone(&manager),
                    config.coordinator.host_lease_seconds.0,
                    config.coordinator.attempt_lease_seconds.0,
                    config.coordinator.max_attempts.0 as usize,
                    config.wake_cursor_path.clone(),
                    &mut shutdown,
                )
                .await;
                if shutdown_requested {
                    return;
                }
                retry = Duration::from_secs(1);
            }
            Ok(ActivationStatus::InertCapabilitiesUnavailable) => {
                tracing::info!(
                    revalidate_seconds = CAPABILITY_REVALIDATE.as_secs(),
                    "Cog runtime adapter inert: runtime-delivery capabilities unavailable"
                );
                if wait_or_shutdown(CAPABILITY_REVALIDATE, &mut shutdown).await {
                    shutdown_coordinator(&coordinator).await;
                    return;
                }
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    retry_seconds = retry.as_secs(),
                    "Cog runtime adapter activation failed closed"
                );
                if wait_or_shutdown(retry, &mut shutdown).await {
                    shutdown_coordinator(&coordinator).await;
                    return;
                }
                retry = (retry * 2).min(MAX_RETRY_BACKOFF);
            }
        }
    }
}

async fn validate_session_routes(
    config: &LoadedConfig,
    manager: &SessionManager,
) -> Result<(), String> {
    let sessions = manager.send_list_sessions().await;
    for route in &config.coordinator.routes {
        let session = sessions
            .iter()
            .find(|session| session.session_id == route.server_session_id)
            .ok_or_else(|| {
                format!(
                    "Cog address {} references missing Yalda session {}",
                    route.address_id, route.server_session_id
                )
            })?;
        if session.archived {
            return Err(format!(
                "Cog address {} references archived Yalda session {}",
                route.address_id, route.server_session_id
            ));
        }
        let expected = match route.provider {
            ProviderKind::Codex => AgentProvider::Codex,
            ProviderKind::Claude => AgentProvider::Claude,
        };
        if session.provider != expected {
            return Err(format!(
                "Cog address {} expects {:?}, but Yalda session {} uses {:?}",
                route.address_id, route.provider, route.server_session_id, session.provider
            ));
        }
    }
    Ok(())
}

async fn run_active(
    coordinator: SharedCoordinator,
    manager: Arc<SessionManager>,
    host_lease_seconds: u64,
    attempt_lease_seconds: u64,
    max_attempts: usize,
    wake_cursor_path: PathBuf,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    let (wake_tx, mut wake_rx) = mpsc::unbounded_channel();
    let wake_task = spawn_wake_reader(Arc::clone(&coordinator), wake_cursor_path, wake_tx);
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    let mut claim_tick = tokio::time::interval(CLAIM_REVALIDATE);
    claim_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut host_tick = tokio::time::interval(renewal_interval(host_lease_seconds));
    host_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut attempt_tick = tokio::time::interval(renewal_interval(attempt_lease_seconds));
    attempt_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut capability_tick = tokio::time::interval(CAPABILITY_REVALIDATE);
    capability_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Tokio intervals fire once immediately. Consume those setup ticks so lease
    // renewal and bounded revalidation begin at their documented intervals.
    claim_tick.tick().await;
    host_tick.tick().await;
    attempt_tick.tick().await;
    capability_tick.tick().await;
    let mut claim_requested = true;
    let mut capabilities_compatible = true;
    let mut host_healthy = true;
    let mut shutdown_requested = false;

    loop {
        if claim_requested && capabilities_compatible && host_healthy {
            claim_requested = false;
            match coordinator_call(&coordinator, |coordinator| coordinator.claim_available()).await
            {
                Ok(batch) => {
                    let remaining_due = batch.remaining_due;
                    let dispatched = batch.dispatches.len();
                    dispatch_batch(batch, &manager, &result_tx);
                    if remaining_due
                        && dispatched > 0
                        && coordinator_inflight(&coordinator).await < max_attempts
                    {
                        claim_requested = true;
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "Cog runtime authoritative claim failed");
                }
            }
            if claim_requested {
                continue;
            }
        }

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    shutdown_requested = true;
                    break;
                }
            }
            Some(_) = wake_rx.recv() => {
                claim_requested = true;
            }
            _ = claim_tick.tick() => {
                claim_requested = capabilities_compatible && host_healthy;
            }
            _ = host_tick.tick() => {
                match coordinator_call(&coordinator, |coordinator| coordinator.renew_host()).await {
                    Ok(()) => host_healthy = true,
                    Err(error) => {
                        host_healthy = false;
                        claim_requested = false;
                        tracing::error!(%error, "Cog runtime host lease renewal failed closed");
                    }
                }
            }
            _ = attempt_tick.tick() => {
                if let Err(error) = coordinator_call(&coordinator, |coordinator| {
                    coordinator.renew_attempts()?;
                    coordinator.retry_terminal_mutations()
                }).await {
                    tracing::error!(%error, "Cog runtime attempt maintenance failed closed");
                }
            }
            _ = capability_tick.tick() => {
                match coordinator_call(&coordinator, |coordinator| coordinator.revalidate_capabilities()).await {
                    Ok(CapabilityStatus::Compatible) => {
                        capabilities_compatible = true;
                        claim_requested = host_healthy;
                    }
                    Ok(CapabilityStatus::Unavailable) => {
                        capabilities_compatible = false;
                        claim_requested = false;
                        tracing::warn!("Cog runtime capabilities were withdrawn; pausing new claims");
                    }
                    Err(error) => {
                        capabilities_compatible = false;
                        claim_requested = false;
                        tracing::error!(%error, "Cog runtime capability revalidation failed closed");
                    }
                }
            }
            Some((attempt_id, result)) = result_rx.recv() => {
                let logged_id = attempt_id.clone();
                if let Err(error) = coordinator_call(&coordinator, move |coordinator| {
                    coordinator.record_provider_result(&attempt_id, result)
                }).await {
                    tracing::error!(attempt_id = %logged_id, %error, "Cog runtime provider result is durable but terminal mutation needs retry");
                }
                claim_requested = true;
            }
        }

        if (!capabilities_compatible || !host_healthy)
            && coordinator_inflight(&coordinator).await == 0
        {
            break;
        }
    }

    wake_task.abort();
    while let Ok((attempt_id, result)) = result_rx.try_recv() {
        let _ = coordinator_call(&coordinator, move |coordinator| {
            coordinator.record_provider_result(&attempt_id, result)
        })
        .await;
    }
    match coordinator_call(&coordinator, |coordinator| coordinator.shutdown()).await {
        Ok(()) => tracing::info!("Cog runtime adapter released attempts and host lease"),
        Err(error) => tracing::error!(%error, "Cog runtime adapter graceful release failed"),
    }
    shutdown_requested
}

fn dispatch_batch(
    batch: ClaimBatch,
    manager: &Arc<SessionManager>,
    result_tx: &mpsc::UnboundedSender<(OpaqueId, ProviderDeliveryResult)>,
) {
    if batch.remaining_incompatible {
        tracing::warn!("Cog runtime claim reports incompatible pending delivery");
    }
    for action in batch.dispatches {
        let manager = Arc::clone(manager);
        let result_tx = result_tx.clone();
        tokio::spawn(async move {
            let result = manager.send_cog_delivery(action.request).await;
            let _ = result_tx.send((action.attempt_id, result));
        });
    }
}

fn spawn_wake_reader(
    coordinator: SharedCoordinator,
    cursor_path: PathBuf,
    wake_tx: mpsc::UnboundedSender<DecimalU64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_event_id = match read_wake_cursor(&cursor_path) {
            Ok(cursor) => cursor,
            Err(error) => {
                tracing::error!(%error, "Cog runtime wake cursor is invalid; wake stream disabled");
                return;
            }
        };
        let mut retry = Duration::from_secs(1);
        loop {
            let opened = coordinator_call(&coordinator, move |coordinator| {
                coordinator.open_wakes(last_event_id)
            })
            .await;
            let mut stream = match opened {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, retry_seconds = retry.as_secs(), "Cog runtime wake connection failed");
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(MAX_RETRY_BACKOFF);
                    continue;
                }
            };
            retry = Duration::from_secs(1);
            let tx = wake_tx.clone();
            let persist_path = cursor_path.clone();
            match tokio::task::spawn_blocking(move || {
                let mut cursor = stream.last_event_id();
                while let Some(wake) = stream.next_wake()? {
                    cursor = Some(wake.wake_id);
                    persist_wake_cursor(&persist_path, wake.wake_id).map_err(|error| {
                        yalda::cog_runtime::ClientError::Sse(format!(
                            "persisting Cog wake cursor failed: {error}"
                        ))
                    })?;
                    if tx.send(wake.wake_id).is_err() {
                        break;
                    }
                }
                Ok::<_, yalda::cog_runtime::ClientError>(cursor)
            })
            .await
            {
                Ok(Ok(cursor)) => last_event_id = cursor,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "Cog runtime wake stream ended with an error");
                    last_event_id = match read_wake_cursor(&cursor_path) {
                        Ok(cursor) => cursor,
                        Err(error) => {
                            tracing::error!(%error, "Cog runtime wake cursor became invalid; wake stream disabled");
                            return;
                        }
                    };
                }
                Err(error) => {
                    tracing::warn!(%error, "Cog runtime wake reader task failed");
                }
            }
            tokio::time::sleep(retry).await;
        }
    })
}

fn read_wake_cursor(path: &Path) -> Result<Option<DecimalU64>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading {} failed: {error}", path.display())),
    };
    let value = text
        .strip_suffix('\n')
        .unwrap_or(&text)
        .parse::<u64>()
        .map_err(|error| format!("decoding {} failed: {error}", path.display()))?;
    if value.to_string() != text.trim_end_matches('\n') {
        return Err(format!(
            "{} does not contain a canonical decimal wake id",
            path.display()
        ));
    }
    Ok(Some(DecimalU64(value)))
}

fn persist_wake_cursor(path: &Path, cursor: DecimalU64) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("cursor.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    writeln!(file, "{cursor}")?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

async fn shutdown_coordinator(coordinator: &SharedCoordinator) {
    match coordinator_call(coordinator, |coordinator| coordinator.shutdown()).await {
        Ok(()) => {}
        Err(error) => tracing::error!(%error, "Cog runtime adapter graceful release failed"),
    }
}

async fn coordinator_inflight(coordinator: &SharedCoordinator) -> usize {
    coordinator_call(coordinator, |coordinator| {
        Ok::<_, yalda::cog_runtime::CoordinatorError>(coordinator.inflight_len())
    })
    .await
    .unwrap_or(usize::MAX)
}

async fn coordinator_call<R, F>(coordinator: &SharedCoordinator, call: F) -> Result<R, String>
where
    R: Send + 'static,
    F: FnOnce(&mut Coordinator) -> Result<R, yalda::cog_runtime::CoordinatorError> + Send + 'static,
{
    let coordinator = Arc::clone(coordinator);
    tokio::task::spawn_blocking(move || {
        let mut coordinator = coordinator
            .lock()
            .map_err(|_| "Cog runtime coordinator mutex was poisoned".to_string())?;
        call(&mut coordinator).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Cog runtime coordinator task failed: {error}"))?
}

fn renewal_interval(lease_seconds: u64) -> Duration {
    Duration::from_secs((lease_seconds / 2).max(1))
}

async fn wait_or_shutdown(duration: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<LoadedConfig, String> {
        let file: ConfigFile = serde_json::from_str(text).map_err(|error| error.to_string())?;
        file.validate(Path::new("/tmp/yalda-test/cog-runtime.json"))
    }

    #[test]
    fn explicit_routes_produce_a_fresh_fenced_runtime_configuration() {
        let first = parse(
            r#"{
            "schema_version":"1",
            "cog_url":"http://127.0.0.1:7666",
            "host_id":"yalda-session-server",
            "allow_takeover":false,
            "addresses":[{
                "address_id":"address-1",
                "yalda_session_id":"session-1",
                "provider":"codex"
            }]
        }"#,
        )
        .unwrap();
        let second = parse(
            r#"{
            "schema_version":"1",
            "cog_url":"http://127.0.0.1:7666",
            "host_id":"yalda-session-server",
            "addresses":[{
                "address_id":"address-1",
                "yalda_session_id":"session-1",
                "provider":"codex"
            }]
        }"#,
        )
        .unwrap();
        assert_ne!(
            first.coordinator.instance_id,
            second.coordinator.instance_id
        );
        assert!(first.coordinator.reconcile_ownership);
        assert!(!first.coordinator.takeover_live_host);
        assert_eq!(
            first.journal_path,
            Path::new("/tmp/yalda-test/cog-runtime-journal.ndjson")
        );
        assert_eq!(
            first.wake_cursor_path,
            Path::new("/tmp/yalda-test/cog-runtime-wake.cursor")
        );
    }

    #[test]
    fn duplicate_address_or_session_mappings_fail_before_activation() {
        let duplicate_address = r#"{
            "schema_version":"1", "cog_url":"http://127.0.0.1:7666", "host_id":"h",
            "addresses":[
                {"address_id":"a","yalda_session_id":"s1","provider":"codex"},
                {"address_id":"a","yalda_session_id":"s2","provider":"claude"}
            ]
        }"#;
        assert!(
            parse(duplicate_address)
                .unwrap_err()
                .contains("duplicate Cog address_id")
        );

        let duplicate_session = duplicate_address.replace(
            "\"address_id\":\"a\",\"yalda_session_id\":\"s2\"",
            "\"address_id\":\"b\",\"yalda_session_id\":\"s1\"",
        );
        assert!(
            parse(&duplicate_session)
                .unwrap_err()
                .contains("multiple Cog addresses")
        );
    }

    #[test]
    fn empty_routes_and_unknown_configuration_are_rejected() {
        assert!(
            parse(
                r#"{
            "schema_version":"1", "cog_url":"http://127.0.0.1:7666", "host_id":"h",
            "addresses":[]
        }"#
            )
            .unwrap_err()
            .contains("at least one")
        );
        assert!(
            serde_json::from_str::<ConfigFile>(
                r#"{
            "schema_version":"1", "cog_url":"http://127.0.0.1:7666", "host_id":"h",
            "addresses":[], "silently_ignored":true
        }"#
            )
            .is_err()
        );
    }

    #[test]
    fn renewal_occurs_no_later_than_half_the_lease() {
        assert_eq!(renewal_interval(60), Duration::from_secs(30));
        assert_eq!(renewal_interval(1), Duration::from_secs(1));
    }

    #[test]
    fn wake_cursor_is_fsynced_and_replayed_losslessly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wake.cursor");
        assert_eq!(read_wake_cursor(&path).unwrap(), None);
        persist_wake_cursor(&path, DecimalU64(9_007_199_254_740_993)).unwrap();
        assert_eq!(
            read_wake_cursor(&path).unwrap(),
            Some(DecimalU64(9_007_199_254_740_993))
        );
        std::fs::write(&path, "01\n").unwrap();
        assert!(read_wake_cursor(&path).is_err());
    }
}
