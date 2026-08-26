//! Deterministic lease/claim/dispatch coordinator for Cog runtime delivery.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;

use chrono::Utc;

use super::journal::{DeliveryJournal, JournalRecord, JournalState};
use super::provider::{DeliveryEnvelope, ProviderDeliveryRequest, ProviderDeliveryResult};
use super::transport::{CapabilityProbe, ClientError, CogClient, CogRuntimeTransport, WakeStream};
use super::wire::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRoute {
    pub address_id: OpaqueId,
    pub server_session_id: String,
    pub provider: ProviderKind,
}

#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    pub host_id: OpaqueId,
    pub instance_id: ProtocolUuid,
    pub routes: Vec<RuntimeRoute>,
    pub host_lease_seconds: DecimalU64,
    pub attempt_lease_seconds: DecimalU64,
    pub max_attempts: DecimalU64,
    pub max_entries: DecimalU64,
    pub max_content_bytes: DecimalU64,
    /// Explicit operator authorization to CAS current owners to this host.
    pub reconcile_ownership: bool,
    /// Explicit operator authorization to take over another live host instance.
    pub takeover_live_host: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActivationStatus {
    Active {
        host_fence: DecimalU64,
        eligible_addresses: Vec<OpaqueId>,
    },
    InertCapabilitiesUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityStatus {
    Compatible,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchAction {
    pub attempt_id: OpaqueId,
    pub request: ProviderDeliveryRequest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimBatch {
    pub dispatches: Vec<DispatchAction>,
    pub remaining_due: bool,
    pub remaining_incompatible: bool,
}

#[derive(Debug)]
pub enum CoordinatorError {
    Client(ClientError),
    Journal(io::Error),
    Contract(String),
    Inactive,
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => error.fmt(f),
            Self::Journal(error) => error.fmt(f),
            Self::Contract(message) => f.write_str(message),
            Self::Inactive => f.write_str("Cog runtime coordinator is inactive"),
        }
    }
}

impl std::error::Error for CoordinatorError {}

impl From<ClientError> for CoordinatorError {
    fn from(value: ClientError) -> Self {
        Self::Client(value)
    }
}

impl From<io::Error> for CoordinatorError {
    fn from(value: io::Error) -> Self {
        Self::Journal(value)
    }
}

#[derive(Debug, Clone)]
enum InflightPhase {
    AwaitingProvider {
        idempotency_key: ProtocolUuid,
    },
    ProviderSucceeded {
        idempotency_key: ProtocolUuid,
        provider: ProviderReceipt,
    },
    ProviderFailed {
        idempotency_key: ProtocolUuid,
        class: FailureClass,
        retryable: bool,
        message: String,
    },
}

#[derive(Debug, Clone)]
struct InflightAttempt {
    attempt: AttemptView,
    phase: InflightPhase,
}

pub struct DeliveryCoordinator<T> {
    client: CogClient<T>,
    config: CoordinatorConfig,
    journal: DeliveryJournal,
    host_fence: Option<DecimalU64>,
    eligible: BTreeMap<OpaqueId, RuntimeRoute>,
    inflight: BTreeMap<OpaqueId, InflightAttempt>,
}

impl<T: CogRuntimeTransport> DeliveryCoordinator<T> {
    pub fn new(client: CogClient<T>, config: CoordinatorConfig, journal: DeliveryJournal) -> Self {
        Self {
            client,
            config,
            journal,
            host_fence: None,
            eligible: BTreeMap::new(),
            inflight: BTreeMap::new(),
        }
    }

    pub fn journal(&self) -> &DeliveryJournal {
        &self.journal
    }

    pub fn host_fence(&self) -> Option<DecimalU64> {
        self.host_fence
    }

    pub fn inflight_len(&self) -> usize {
        self.inflight.len()
    }

    pub fn activate(&mut self) -> Result<ActivationStatus, CoordinatorError> {
        match self.revalidate_capabilities()? {
            CapabilityStatus::Unavailable => {
                return Ok(ActivationStatus::InertCapabilitiesUnavailable);
            }
            CapabilityStatus::Compatible => {}
        }

        let current = match self.client.get_host_lease(&self.config.host_id) {
            Ok(response) => Some(response),
            Err(error) if api_code(&error) == Some(ErrorCode::HostNotFound) => None,
            Err(error) => return Err(error.into()),
        };
        let (takeover, expected_host_fence) = match current {
            None => (false, None),
            Some(response)
                if response.live && response.lease.instance_id == self.config.instance_id =>
            {
                (false, Some(response.lease.host_fence))
            }
            Some(response) if response.live => {
                if !self.config.takeover_live_host {
                    return Err(CoordinatorError::Contract(
                        "another live runtime host instance owns the lease; takeover is not authorized"
                            .into(),
                    ));
                }
                (true, Some(response.lease.host_fence))
            }
            Some(response) => (false, Some(response.lease.host_fence)),
        };
        let lease = self.client.acquire_host_lease(
            &self.config.host_id,
            &LeaseAcquireRequest {
                instance_id: self.config.instance_id,
                protocol_version: ProtocolOne::V1,
                source_kinds: vec![SourceKind::Mail, SourceKind::Chat],
                provider_kinds: self.configured_providers(),
                lease_seconds: self.config.host_lease_seconds,
                takeover,
                expected_host_fence,
            },
        )?;
        self.host_fence = Some(lease.lease.host_fence);
        self.reconcile_routes()?;
        self.validate_open_attempt_history()?;
        self.reconcile_journal_terminals()?;
        Ok(ActivationStatus::Active {
            host_fence: lease.lease.host_fence,
            eligible_addresses: self.eligible.keys().cloned().collect(),
        })
    }

    /// Re-negotiate the live contract without making a host, owner, attempt, or
    /// journal mutation. Supervisors use this periodically so a withdrawn or
    /// incompatible capability stops new delivery instead of relying on a
    /// process restart.
    pub fn revalidate_capabilities(&self) -> Result<CapabilityStatus, CoordinatorError> {
        let capabilities = match self.client.probe_capabilities()? {
            CapabilityProbe::Unavailable => return Ok(CapabilityStatus::Unavailable),
            CapabilityProbe::Available(capabilities) => capabilities,
        };
        let providers = self.configured_providers();
        if let Some(error) = capabilities.compatibility_error(&providers) {
            return Err(CoordinatorError::Contract(format!(
                "Cog runtime capabilities are incompatible: {error}"
            )));
        }
        self.validate_limits(&capabilities)?;
        Ok(CapabilityStatus::Compatible)
    }

    pub fn renew_host(&mut self) -> Result<(), CoordinatorError> {
        let host_fence = self.host_fence.ok_or(CoordinatorError::Inactive)?;
        let response = self.client.renew_host_lease(
            &self.config.host_id,
            &LeaseRenewRequest {
                instance_id: self.config.instance_id,
                host_fence,
                lease_seconds: self.config.host_lease_seconds,
            },
        )?;
        self.host_fence = Some(response.lease.host_fence);
        Ok(())
    }

    /// Open the advisory resumable wake stream. Each frame (and each reconnect)
    /// must be followed by [`claim_available`](Self::claim_available), which is
    /// the authoritative capacity decision.
    pub fn open_wakes(
        &self,
        last_event_id: Option<DecimalU64>,
    ) -> Result<WakeStream, CoordinatorError> {
        let host_fence = self.host_fence.ok_or(CoordinatorError::Inactive)?;
        Ok(self.client.open_wakes(
            &self.config.host_id,
            self.config.instance_id,
            host_fence,
            last_event_id,
        )?)
    }

    pub fn claim_available(&mut self) -> Result<ClaimBatch, CoordinatorError> {
        let host_fence = self.host_fence.ok_or(CoordinatorError::Inactive)?;
        let active_addresses: BTreeSet<&OpaqueId> = self
            .inflight
            .values()
            .map(|attempt| &attempt.attempt.common.address_id)
            .collect();
        let available_addresses: Vec<OpaqueId> = self
            .eligible
            .keys()
            .filter(|address| !active_addresses.contains(address))
            .cloned()
            .collect();
        let remaining_capacity = self
            .config
            .max_attempts
            .0
            .saturating_sub(self.inflight.len() as u64);
        let response = self.client.claim(
            &self.config.host_id,
            &ClaimRequest {
                instance_id: self.config.instance_id,
                host_fence,
                available_addresses,
                max_attempts: DecimalU64(remaining_capacity),
                max_entries: self.config.max_entries,
                max_content_bytes: self.config.max_content_bytes,
                attempt_lease_seconds: self.config.attempt_lease_seconds,
            },
        )?;

        let mut dispatches = Vec::new();
        for attempt in response.attempts {
            if let Some(action) = self.admit_claimed_attempt(attempt)? {
                dispatches.push(action);
            }
        }
        Ok(ClaimBatch {
            dispatches,
            remaining_due: response.remaining_due,
            remaining_incompatible: response.remaining_incompatible,
        })
    }

    pub fn record_provider_result(
        &mut self,
        attempt_id: &OpaqueId,
        result: ProviderDeliveryResult,
    ) -> Result<(), CoordinatorError> {
        let inflight = self
            .inflight
            .get_mut(attempt_id)
            .ok_or_else(|| CoordinatorError::Contract(format!("unknown attempt {attempt_id}")))?;
        let idempotency_key = match inflight.phase {
            InflightPhase::AwaitingProvider { idempotency_key } => idempotency_key,
            _ => {
                return Err(CoordinatorError::Contract(
                    "provider result was already recorded".into(),
                ));
            }
        };
        match result {
            ProviderDeliveryResult::Succeeded(provider) => {
                append_attempt_state(
                    &mut self.journal,
                    &inflight.attempt,
                    JournalState::ProviderSucceeded {
                        idempotency_key,
                        provider: provider.clone(),
                    },
                )?;
                inflight.phase = InflightPhase::ProviderSucceeded {
                    idempotency_key,
                    provider,
                };
            }
            ProviderDeliveryResult::Failed(failure) => {
                append_attempt_state(
                    &mut self.journal,
                    &inflight.attempt,
                    JournalState::ProviderFailed {
                        idempotency_key,
                        class: failure.class,
                        retryable: failure.retryable,
                        message: failure.message.clone(),
                    },
                )?;
                inflight.phase = InflightPhase::ProviderFailed {
                    idempotency_key,
                    class: failure.class,
                    retryable: failure.retryable,
                    message: failure.message,
                };
            }
        }
        self.flush_terminal(attempt_id)
    }

    pub fn retry_terminal_mutations(&mut self) -> Result<(), CoordinatorError> {
        let ids: Vec<OpaqueId> = self
            .inflight
            .iter()
            .filter(|(_, attempt)| !matches!(attempt.phase, InflightPhase::AwaitingProvider { .. }))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.flush_terminal(&id)?;
        }
        Ok(())
    }

    pub fn renew_attempts(&mut self) -> Result<(), CoordinatorError> {
        let host_fence = self.host_fence.ok_or(CoordinatorError::Inactive)?;
        let ids: Vec<OpaqueId> = self.inflight.keys().cloned().collect();
        for id in ids {
            let Some(inflight) = self.inflight.get(&id).cloned() else {
                continue;
            };
            let request = AttemptRenewRequest {
                fences: fences(&inflight.attempt, self.config.instance_id, host_fence),
                lease_seconds: self.config.attempt_lease_seconds,
            };
            match self.client.renew_attempt(&id, &request) {
                Ok(response) => {
                    if let Some(current) = self.inflight.get_mut(&id) {
                        current.attempt = response.attempt;
                    }
                }
                Err(error) if is_attempt_terminal_error(&error) => {
                    self.observe_terminal(&id, format!("attempt renewal fenced: {error}"))?;
                }
                Err(error)
                    if matches!(
                        api_code(&error),
                        Some(ErrorCode::AttemptLeaseExpired | ErrorCode::StaleAttemptFence)
                    ) =>
                {
                    if matches!(inflight.phase, InflightPhase::AwaitingProvider { .. }) {
                        self.release_local(&id, format!("claim lost: {error}"))?;
                    } else {
                        // Preserve the durable terminal provider result as the
                        // latest journal state. A later stable reclaim retries
                        // the identical completion/failure without dispatch.
                        self.inflight.remove(&id);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub fn shutdown(&mut self) -> Result<(), CoordinatorError> {
        let Some(host_fence) = self.host_fence else {
            return Ok(());
        };
        self.retry_terminal_mutations()?;
        let ids: Vec<OpaqueId> = self.inflight.keys().cloned().collect();
        for id in ids {
            let Some(inflight) = self.inflight.get(&id).cloned() else {
                continue;
            };
            let response = self.client.release_attempt(
                &id,
                &AttemptReleaseRequest {
                    fences: fences(&inflight.attempt, self.config.instance_id, host_fence),
                    reason: "Yaldabaoth session server shutting down".into(),
                },
            );
            match response {
                Ok(_) => self.release_local(&id, "graceful shutdown".into())?,
                Err(error) if is_attempt_terminal_error(&error) => {
                    self.observe_terminal(&id, format!("shutdown release fenced: {error}"))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        self.client.release_host_lease(
            &self.config.host_id,
            &LeaseReleaseRequest {
                instance_id: self.config.instance_id,
                host_fence,
            },
        )?;
        self.host_fence = None;
        Ok(())
    }

    fn configured_providers(&self) -> Vec<ProviderKind> {
        let mut providers = Vec::new();
        for route in &self.config.routes {
            if !providers.contains(&route.provider) {
                providers.push(route.provider);
            }
        }
        providers
    }

    fn validate_limits(&self, capabilities: &Capabilities) -> Result<(), CoordinatorError> {
        let limits = &capabilities.limits;
        for (name, value, range) in [
            (
                "host_lease_seconds",
                self.config.host_lease_seconds,
                &limits.host_lease_seconds,
            ),
            (
                "attempt_lease_seconds",
                self.config.attempt_lease_seconds,
                &limits.attempt_lease_seconds,
            ),
        ] {
            if value < range.min || value > range.max {
                return Err(CoordinatorError::Contract(format!(
                    "configured {name} {value} is outside {}..={}",
                    range.min, range.max
                )));
            }
        }
        if self.config.max_attempts > capabilities.limits.max_claim_attempts
            || self.config.max_entries > capabilities.limits.max_claim_entries
            || self.config.max_content_bytes > capabilities.limits.max_claim_content_bytes
        {
            return Err(CoordinatorError::Contract(
                "configured claim capacity exceeds Cog capability limits".into(),
            ));
        }
        Ok(())
    }

    fn reconcile_routes(&mut self) -> Result<(), CoordinatorError> {
        self.eligible.clear();
        let mut seen_sessions = BTreeSet::new();
        for route in self.config.routes.clone() {
            if !seen_sessions.insert(route.server_session_id.clone()) {
                return Err(CoordinatorError::Contract(
                    "multiple Cog routes map to the same Yalda provider session".into(),
                ));
            }
            let current = match self.client.get_delivery_owner(&route.address_id) {
                Ok(response) => response,
                Err(error)
                    if matches!(
                        api_code(&error),
                        Some(ErrorCode::RetiredAddress | ErrorCode::AddressNotFound)
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let already_owned = matches!(
                &current.owner,
                DeliveryOwner::External { host_id, .. } if host_id == &self.config.host_id
            );
            let owner = if already_owned {
                current
            } else if self.config.reconcile_ownership {
                match self.client.put_delivery_owner(
                    &route.address_id,
                    &DeliveryOwnerPutRequest {
                        owner: DeliveryOwnerSelection::External {
                            host_id: self.config.host_id.clone(),
                        },
                        expected_owner_generation: current.owner.generation(),
                    },
                ) {
                    Ok(response) => response,
                    Err(error) if api_code(&error) == Some(ErrorCode::RetiredAddress) => continue,
                    Err(error) => return Err(error.into()),
                }
            } else {
                continue;
            };
            if matches!(
                &owner.owner,
                DeliveryOwner::External { host_id, .. } if host_id == &self.config.host_id
            ) {
                self.eligible.insert(route.address_id.clone(), route);
            }
        }
        Ok(())
    }

    fn validate_open_attempt_history(&self) -> Result<(), CoordinatorError> {
        let mut after = None;
        loop {
            let page = self
                .client
                .list_open_attempts(&self.config.host_id, 100, after.as_ref())?;
            for attempt in page.attempts {
                if let Some(record) = self.journal.latest(&attempt.common.attempt_id) {
                    validate_stable_identity(record, &attempt)?;
                }
            }
            let Some(next) = page.next_page else {
                break;
            };
            after = Some(next);
        }
        Ok(())
    }

    fn reconcile_journal_terminals(&mut self) -> Result<(), CoordinatorError> {
        let records: Vec<JournalRecord> = self
            .journal
            .snapshot()
            .attempts
            .into_values()
            .filter(|record| !record.state.is_locally_terminal())
            .collect();
        for record in records {
            let response = self.client.get_attempt(&record.attempt_id)?;
            validate_stable_identity(&record, &response.attempt)?;
            let state = match (&record.state, &response.attempt.status) {
                (JournalState::ProviderSucceeded { .. }, AttemptStatus::Completed { .. }) => {
                    Some(JournalState::CogCompleted)
                }
                (
                    JournalState::ProviderFailed { .. },
                    AttemptStatus::RetryWait { .. } | AttemptStatus::Blocked { .. },
                ) => Some(JournalState::CogFailed),
                (_, AttemptStatus::Superseded { supersession }) => {
                    Some(JournalState::TerminalObserved {
                        reason: format!("Cog attempt superseded: {:?}", supersession.reason),
                    })
                }
                (_, AttemptStatus::Completed { .. })
                | (_, AttemptStatus::RetryWait { .. })
                | (_, AttemptStatus::Blocked { .. }) => Some(JournalState::TerminalObserved {
                    reason: "Cog attempt is terminal with no matching local provider result".into(),
                }),
                _ => None,
            };
            if let Some(state) = state {
                self.journal.append(
                    now_timestamp(),
                    record.attempt_id.clone(),
                    record.delivery_key.clone(),
                    record.payload_digest.clone(),
                    record.address_id.clone(),
                    record.host_fence,
                    record.owner_generation,
                    record.attempt_fence,
                    state,
                )?;
            }
        }
        Ok(())
    }

    fn admit_claimed_attempt(
        &mut self,
        attempt: AttemptView,
    ) -> Result<Option<DispatchAction>, CoordinatorError> {
        if !matches!(attempt.status, AttemptStatus::Claimed { .. }) {
            return Err(CoordinatorError::Contract(
                "claim response contained a non-claimed attempt".into(),
            ));
        }
        let Some(route) = self.eligible.get(&attempt.common.address_id).cloned() else {
            return Ok(None);
        };
        let host_fence = self.host_fence.ok_or(CoordinatorError::Inactive)?;
        if attempt.common.host_id.as_ref() != Some(&self.config.host_id)
            || attempt.common.host_fence != Some(host_fence)
        {
            return Err(CoordinatorError::Contract(
                "claimed attempt carries a different host fence".into(),
            ));
        }

        let previous = self.journal.latest(&attempt.common.attempt_id).cloned();
        if let Some(previous) = &previous {
            validate_stable_identity(previous, &attempt)?;
            if previous.state.is_locally_terminal() {
                return Ok(None);
            }
        }
        let phase = match previous.map(|record| record.state) {
            Some(JournalState::ProviderSucceeded {
                idempotency_key,
                provider,
            }) => InflightPhase::ProviderSucceeded {
                idempotency_key,
                provider,
            },
            Some(JournalState::ProviderFailed {
                idempotency_key,
                class,
                retryable,
                message,
            }) => InflightPhase::ProviderFailed {
                idempotency_key,
                class,
                retryable,
                message,
            },
            _ => {
                append_attempt_state(&mut self.journal, &attempt, JournalState::Claimed)?;
                let idempotency_key = ProtocolUuid::from_uuid(uuid::Uuid::new_v4());
                append_attempt_state(
                    &mut self.journal,
                    &attempt,
                    JournalState::DispatchStarted { idempotency_key },
                )?;
                InflightPhase::AwaitingProvider { idempotency_key }
            }
        };
        let attempt_id = attempt.common.attempt_id.clone();
        let action =
            matches!(phase, InflightPhase::AwaitingProvider { .. }).then(|| DispatchAction {
                attempt_id: attempt_id.clone(),
                request: ProviderDeliveryRequest {
                    server_session_id: route.server_session_id,
                    provider: route.provider,
                    envelope: DeliveryEnvelope::from_attempt(&attempt),
                },
            });
        self.inflight
            .insert(attempt_id.clone(), InflightAttempt { attempt, phase });
        if action.is_none() {
            self.flush_terminal(&attempt_id)?;
        }
        Ok(action)
    }

    fn flush_terminal(&mut self, attempt_id: &OpaqueId) -> Result<(), CoordinatorError> {
        let host_fence = self.host_fence.ok_or(CoordinatorError::Inactive)?;
        let Some(inflight) = self.inflight.get(attempt_id).cloned() else {
            return Ok(());
        };
        let result = match inflight.phase {
            InflightPhase::AwaitingProvider { .. } => return Ok(()),
            InflightPhase::ProviderSucceeded {
                idempotency_key,
                provider,
            } => self.client.complete_attempt(
                attempt_id,
                &AttemptCompleteRequest {
                    fences: fences(&inflight.attempt, self.config.instance_id, host_fence),
                    idempotency_key,
                    provider,
                },
            ),
            InflightPhase::ProviderFailed {
                idempotency_key,
                class,
                retryable,
                message,
            } => self.client.fail_attempt(
                attempt_id,
                &AttemptFailRequest {
                    fences: fences(&inflight.attempt, self.config.instance_id, host_fence),
                    idempotency_key,
                    class,
                    retryable,
                    message,
                },
            ),
        };
        match result {
            Ok(response) => {
                let state = match response.attempt.status {
                    AttemptStatus::Completed { .. } => JournalState::CogCompleted,
                    AttemptStatus::RetryWait { .. } | AttemptStatus::Blocked { .. } => {
                        JournalState::CogFailed
                    }
                    _ => {
                        return Err(CoordinatorError::Contract(
                            "terminal mutation returned a nonterminal attempt".into(),
                        ));
                    }
                };
                append_attempt_state(&mut self.journal, &inflight.attempt, state)?;
                self.inflight.remove(attempt_id);
                Ok(())
            }
            Err(error) if is_attempt_terminal_error(&error) => {
                self.observe_terminal(attempt_id, format!("terminal mutation fenced: {error}"))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn observe_terminal(
        &mut self,
        attempt_id: &OpaqueId,
        reason: String,
    ) -> Result<(), CoordinatorError> {
        if let Some(inflight) = self.inflight.remove(attempt_id) {
            append_attempt_state(
                &mut self.journal,
                &inflight.attempt,
                JournalState::TerminalObserved { reason },
            )?;
        }
        Ok(())
    }

    fn release_local(
        &mut self,
        attempt_id: &OpaqueId,
        reason: String,
    ) -> Result<(), CoordinatorError> {
        if let Some(inflight) = self.inflight.remove(attempt_id) {
            append_attempt_state(
                &mut self.journal,
                &inflight.attempt,
                JournalState::Released { reason },
            )?;
        }
        Ok(())
    }
}

fn append_attempt_state(
    journal: &mut DeliveryJournal,
    attempt: &AttemptView,
    state: JournalState,
) -> Result<(), CoordinatorError> {
    let common = &attempt.common;
    let host_fence = common
        .host_fence
        .ok_or_else(|| CoordinatorError::Contract("claimed attempt has no host fence".into()))?;
    journal.append(
        now_timestamp(),
        common.attempt_id.clone(),
        common.delivery_key.clone(),
        common.payload_digest.clone(),
        common.address_id.clone(),
        host_fence,
        common.owner_generation,
        common.attempt_fence,
        state,
    )?;
    Ok(())
}

fn now_timestamp() -> Timestamp {
    Timestamp::new(Utc::now().format("%Y-%m-%dT%H:%M:%S.%9fZ").to_string())
        .expect("chrono emits exact UTC nanosecond timestamps")
}

fn fences(
    attempt: &AttemptView,
    instance_id: ProtocolUuid,
    host_fence: DecimalU64,
) -> AttemptFenceRequest {
    AttemptFenceRequest {
        instance_id,
        host_fence,
        owner_generation: attempt.common.owner_generation,
        attempt_fence: attempt.common.attempt_fence,
    }
}

fn validate_stable_identity(
    record: &JournalRecord,
    attempt: &AttemptView,
) -> Result<(), CoordinatorError> {
    let common = &attempt.common;
    if record.delivery_key != common.delivery_key
        || record.payload_digest != common.payload_digest
        || record.address_id != common.address_id
    {
        Err(CoordinatorError::Contract(format!(
            "stable attempt {} changed immutable identity",
            common.attempt_id
        )))
    } else {
        Ok(())
    }
}

fn api_code(error: &ClientError) -> Option<ErrorCode> {
    match error {
        ClientError::Api { error, .. } => Some(error.code),
        _ => None,
    }
}

fn is_attempt_terminal_error(error: &ClientError) -> bool {
    matches!(
        api_code(error),
        Some(ErrorCode::AttemptAlreadyTerminal | ErrorCode::RetiredAddress)
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::cog_runtime::transport::{
        HttpRequest, HttpResponse, StreamResponse, TransportError,
    };

    #[derive(Default)]
    struct ScriptedTransport {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    }

    impl ScriptedTransport {
        fn push_json(&self, status: u16, value: serde_json::Value) {
            self.responses.lock().unwrap().push_back(Ok(HttpResponse {
                status,
                headers: BTreeMap::from([("content-type".into(), MEDIA_TYPE.into())]),
                body: serde_json::to_vec(&value).unwrap(),
            }));
        }

        fn push_transport_error(&self, message: &str) {
            self.responses
                .lock()
                .unwrap()
                .push_back(Err(TransportError(message.into())));
        }
    }

    impl CogRuntimeTransport for ScriptedTransport {
        fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(TransportError("no scripted response".into())))
        }

        fn open_stream(&self, _request: HttpRequest) -> Result<StreamResponse, TransportError> {
            Ok(StreamResponse {
                status: 200,
                headers: BTreeMap::from([("content-type".into(), SSE_MEDIA_TYPE.into())]),
                reader: Box::new(std::io::BufReader::new(Cursor::new(Vec::new()))),
            })
        }
    }

    fn config(routes: Vec<RuntimeRoute>) -> CoordinatorConfig {
        CoordinatorConfig {
            host_id: OpaqueId::new("yalda-host").unwrap(),
            instance_id: ProtocolUuid::from_uuid(uuid::Uuid::nil()),
            routes,
            host_lease_seconds: DecimalU64(60),
            attempt_lease_seconds: DecimalU64(60),
            max_attempts: DecimalU64(4),
            max_entries: DecimalU64(100),
            max_content_bytes: DecimalU64(1_000_000),
            reconcile_ownership: false,
            takeover_live_host: false,
        }
    }

    fn route(address: &str, session: &str) -> RuntimeRoute {
        RuntimeRoute {
            address_id: OpaqueId::new(address).unwrap(),
            server_session_id: session.into(),
            provider: ProviderKind::Codex,
        }
    }

    fn timestamp() -> Timestamp {
        Timestamp::new("2026-08-25T18:02:03.123456789Z").unwrap()
    }

    fn claimed_attempt(address: &str, fence: u64) -> AttemptView {
        AttemptView {
            common: AttemptCommon {
                attempt_id: OpaqueId::new(format!("attempt-{address}")).unwrap(),
                delivery_key: OpaqueId::new(format!("delivery-{address}")).unwrap(),
                payload_digest: Sha256Digest::new("a".repeat(64)).unwrap(),
                attempt_fence: DecimalU64(fence),
                host_id: Some(OpaqueId::new("yalda-host").unwrap()),
                host_fence: Some(DecimalU64(7)),
                address_id: OpaqueId::new(address).unwrap(),
                owner_generation: DecimalU64(3),
                created_at: timestamp(),
                cursor_before: CursorVector {
                    kind: SourceVectorLiteral::SourceVector,
                    points: Vec::new(),
                },
                cursor_through: CursorVector {
                    kind: SourceVectorLiteral::SourceVector,
                    points: Vec::new(),
                },
                advances: Vec::new(),
                oversize: false,
                entries: Vec::new(),
            },
            status: AttemptStatus::Claimed {
                instance_id: ProtocolUuid::from_uuid(uuid::Uuid::nil()),
                claimed_at: timestamp(),
                lease_expires_at: timestamp(),
            },
        }
    }

    fn delivery_entry(source_kind: SourceKind, source_id: &str, event_id: u64) -> DeliveryEntry {
        DeliveryEntry {
            event_id: DecimalU64(event_id),
            source_kind,
            source_id: OpaqueId::new(source_id).unwrap(),
            source_name: source_id.into(),
            topic_addresses: vec!["projects/cog/mail".into()],
            entry_id: OpaqueId::new(format!("entry-{event_id}")).unwrap(),
            from: OpaqueId::new("peer").unwrap(),
            audit_actor: "peer-actor".into(),
            at: timestamp(),
            content: serde_json::json!({"message":format!("entry {event_id}")}),
            content_size_bytes: DecimalU64(8),
            references: Vec::new(),
        }
    }

    fn capabilities_json() -> serde_json::Value {
        serde_json::json!({
            "schema_version":"1",
            "protocol_versions":["1"],
            "source_kinds":["mail","chat"],
            "provider_kinds":["codex","claude"],
            "features":REQUIRED_FEATURES,
            "limits":{
                "host_lease_seconds":{"min":"15","max":"300"},
                "attempt_lease_seconds":{"min":"30","max":"900"},
                "max_claim_attempts":"20",
                "max_claim_entries":"1000",
                "max_claim_content_bytes":"10000000"
            },
            "server_time":timestamp().as_str()
        })
    }

    fn lease_json(instance_id: ProtocolUuid, fence: u64, live: bool) -> serde_json::Value {
        serde_json::json!({
            "schema_version":"1",
            "lease":{
                "host_id":"yalda-host",
                "instance_id":instance_id,
                "host_fence":fence.to_string(),
                "protocol_version":"1",
                "source_kinds":["mail","chat"],
                "provider_kinds":["codex"],
                "lease_expires_at":timestamp().as_str()
            },
            "live":live,
            "server_time":timestamp().as_str()
        })
    }

    fn coordinator(
        transport: Arc<ScriptedTransport>,
        journal: DeliveryJournal,
        routes: Vec<RuntimeRoute>,
    ) -> DeliveryCoordinator<ScriptedTransport> {
        let mut coordinator = DeliveryCoordinator::new(
            CogClient::from_shared(transport),
            config(routes.clone()),
            journal,
        );
        coordinator.host_fence = Some(DecimalU64(7));
        coordinator.eligible = routes
            .into_iter()
            .map(|route| (route.address_id.clone(), route))
            .collect();
        coordinator
    }

    #[test]
    fn capability_404_is_inert_before_any_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        for _ in 0..2 {
            transport
                .responses
                .lock()
                .unwrap()
                .push_back(Ok(HttpResponse {
                    status: 404,
                    headers: BTreeMap::new(),
                    body: Vec::new(),
                }));
        }
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = DeliveryCoordinator::new(
            CogClient::from_shared(Arc::clone(&transport)),
            config(vec![route("a", "s")]),
            journal,
        );
        assert_eq!(
            coordinator.revalidate_capabilities().unwrap(),
            CapabilityStatus::Unavailable
        );
        assert_eq!(
            coordinator.activate().unwrap(),
            ActivationStatus::InertCapabilitiesUnavailable
        );
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].method,
            crate::cog_runtime::transport::HttpMethod::Get
        );
        assert_eq!(
            requests[1].path_and_query,
            "/v1/runtime-delivery/capabilities"
        );
        assert_eq!(coordinator.journal.snapshot().next_seq, 1);
    }

    #[test]
    fn fresh_activation_acquires_fence_and_requires_exact_external_owner() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        transport.push_json(200, capabilities_json());
        transport.push_json(
            404,
            serde_json::json!({"error":{
                "code":"host_not_found", "message":"missing", "retryable":false,
                "details":{}
            }}),
        );
        transport.push_json(
            200,
            lease_json(ProtocolUuid::from_uuid(uuid::Uuid::nil()), 1, true),
        );
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "address_id":"address",
                "owner":{"mode":"external","host_id":"yalda-host","owner_generation":"3"},
                "server_time":timestamp().as_str()
            }),
        );
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "attempts":[], "server_time":timestamp().as_str()
            }),
        );
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = DeliveryCoordinator::new(
            CogClient::from_shared(Arc::clone(&transport)),
            config(vec![route("address", "session")]),
            journal,
        );
        assert_eq!(
            coordinator.activate().unwrap(),
            ActivationStatus::Active {
                host_fence: DecimalU64(1),
                eligible_addresses: vec![OpaqueId::new("address").unwrap()]
            }
        );
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        let lease_body: serde_json::Value = serde_json::from_slice(&requests[2].body).unwrap();
        assert_eq!(lease_body["takeover"], false);
        assert!(lease_body.get("expected_host_fence").is_none());
        assert_eq!(
            requests[3].path_and_query,
            "/v1/addresses/address/delivery-owner"
        );
    }

    #[test]
    fn activation_renews_the_same_live_instance_without_takeover() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        transport.push_json(200, capabilities_json());
        transport.push_json(
            200,
            lease_json(ProtocolUuid::from_uuid(uuid::Uuid::nil()), 7, true),
        );
        transport.push_json(
            200,
            lease_json(ProtocolUuid::from_uuid(uuid::Uuid::nil()), 8, true),
        );
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "address_id":"address",
                "owner":{"mode":"external","host_id":"yalda-host","owner_generation":"3"},
                "server_time":timestamp().as_str()
            }),
        );
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "attempts":[], "server_time":timestamp().as_str()
            }),
        );
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = DeliveryCoordinator::new(
            CogClient::from_shared(Arc::clone(&transport)),
            config(vec![route("address", "session")]),
            journal,
        );

        assert_eq!(
            coordinator.activate().unwrap(),
            ActivationStatus::Active {
                host_fence: DecimalU64(8),
                eligible_addresses: vec![OpaqueId::new("address").unwrap()]
            }
        );
        let requests = transport.requests.lock().unwrap();
        let lease_body: serde_json::Value = serde_json::from_slice(&requests[2].body).unwrap();
        assert_eq!(lease_body["takeover"], false);
        assert_eq!(lease_body["expected_host_fence"], "7");
    }

    #[test]
    fn explicit_route_selection_cas_transfers_owner_to_the_runtime_host() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        transport.push_json(200, capabilities_json());
        transport.push_json(
            404,
            serde_json::json!({"error":{
                "code":"host_not_found", "message":"missing", "retryable":false,
                "details":{}
            }}),
        );
        transport.push_json(
            200,
            lease_json(ProtocolUuid::from_uuid(uuid::Uuid::nil()), 1, true),
        );
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "address_id":"address",
                "owner":{"mode":"cogd","owner_generation":"2"},
                "server_time":timestamp().as_str()
            }),
        );
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "address_id":"address",
                "owner":{"mode":"external","host_id":"yalda-host","owner_generation":"3"},
                "server_time":timestamp().as_str()
            }),
        );
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "attempts":[], "server_time":timestamp().as_str()
            }),
        );
        let mut runtime_config = config(vec![route("address", "session")]);
        runtime_config.reconcile_ownership = true;
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = DeliveryCoordinator::new(
            CogClient::from_shared(Arc::clone(&transport)),
            runtime_config,
            journal,
        );
        assert!(matches!(
            coordinator.activate().unwrap(),
            ActivationStatus::Active { .. }
        ));
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 6);
        assert_eq!(
            requests[4].method,
            crate::cog_runtime::transport::HttpMethod::Put
        );
        let body: serde_json::Value = serde_json::from_slice(&requests[4].body).unwrap();
        assert_eq!(body["expected_owner_generation"], "2");
        assert_eq!(
            body["owner"],
            serde_json::json!({"mode":"external","host_id":"yalda-host"})
        );
    }

    #[test]
    fn live_other_instance_requires_explicit_takeover_before_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        transport.push_json(200, capabilities_json());
        transport.push_json(
            200,
            lease_json(ProtocolUuid::from_uuid(uuid::Uuid::from_u128(1)), 9, true),
        );
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = DeliveryCoordinator::new(
            CogClient::from_shared(Arc::clone(&transport)),
            config(vec![route("address", "session")]),
            journal,
        );
        assert!(matches!(
            coordinator.activate(),
            Err(CoordinatorError::Contract(message)) if message.contains("takeover")
        ));
        assert_eq!(transport.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn claim_is_fsynced_through_dispatch_started_before_action_is_returned() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        let journal_path = dir.path().join("journal");
        let journal = DeliveryJournal::open(&journal_path).unwrap();
        let mut coordinator = coordinator(transport, journal, vec![route("address", "session")]);
        let action = coordinator
            .admit_claimed_attempt(claimed_attempt("address", 1))
            .unwrap()
            .expect("dispatch action");
        assert_eq!(action.request.server_session_id, "session");
        assert_eq!(
            action.request.envelope.attempt_id.as_str(),
            "attempt-address"
        );

        let reopened = DeliveryJournal::open(&journal_path).unwrap();
        assert_eq!(reopened.snapshot().next_seq, 3);
        assert!(matches!(
            reopened
                .latest(&OpaqueId::new("attempt-address").unwrap())
                .unwrap()
                .state,
            JournalState::DispatchStarted { .. }
        ));
    }

    #[test]
    fn lost_completion_response_reuses_identical_body_then_observes_terminal_race() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        transport.push_transport_error("response lost");
        transport.push_json(
            409,
            serde_json::json!({"error":{
                "code":"attempt_already_terminal", "message":"retired concurrently",
                "retryable":false, "details":{}
            }}),
        );
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = coordinator(
            Arc::clone(&transport),
            journal,
            vec![route("address", "session")],
        );
        let attempt = claimed_attempt("address", 1);
        let attempt_id = attempt.common.attempt_id.clone();
        coordinator
            .admit_claimed_attempt(attempt)
            .unwrap()
            .expect("dispatch");
        let receipt = ProviderReceipt {
            kind: ProviderKind::Codex,
            session_id: OpaqueId::new("provider-session").unwrap(),
            turn_id: Some(OpaqueId::new("1").unwrap()),
            metadata: None,
        };
        assert!(matches!(
            coordinator
                .record_provider_result(&attempt_id, ProviderDeliveryResult::Succeeded(receipt)),
            Err(CoordinatorError::Client(ClientError::Transport(_)))
        ));
        assert!(matches!(
            coordinator.journal.latest(&attempt_id).unwrap().state,
            JournalState::ProviderSucceeded { .. }
        ));
        coordinator.retry_terminal_mutations().unwrap();
        assert_eq!(coordinator.inflight_len(), 0);
        assert!(matches!(
            coordinator.journal.latest(&attempt_id).unwrap().state,
            JournalState::TerminalObserved { .. }
        ));
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body, requests[1].body, "idempotent retry body");
    }

    #[test]
    fn mixed_mail_chat_batch_preserves_order_and_never_submits_a_cursor_advance() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = coordinator(
            Arc::clone(&transport),
            journal,
            vec![route("address", "session")],
        );
        let mut attempt = claimed_attempt("address", 1);
        let chat_id = OpaqueId::new("chat-1").unwrap();
        attempt.common.oversize = true;
        attempt.common.cursor_before.points = vec![
            CursorPoint {
                owner: CursorOwner::Mail,
                position: DecimalU64(10),
            },
            CursorPoint {
                owner: CursorOwner::Chat {
                    chat_id: chat_id.clone(),
                },
                position: DecimalU64(20),
            },
        ];
        attempt.common.cursor_through.points = vec![
            CursorPoint {
                owner: CursorOwner::Mail,
                position: DecimalU64(11),
            },
            CursorPoint {
                owner: CursorOwner::Chat {
                    chat_id: chat_id.clone(),
                },
                position: DecimalU64(21),
            },
        ];
        attempt.common.advances = vec![
            CursorAdvance {
                owner: CursorOwner::Mail,
                before: DecimalU64(10),
                through: DecimalU64(11),
            },
            CursorAdvance {
                owner: CursorOwner::Chat { chat_id },
                before: DecimalU64(20),
                through: DecimalU64(21),
            },
        ];
        attempt.common.entries = vec![
            delivery_entry(SourceKind::Mail, "mail-1", 11),
            delivery_entry(SourceKind::Chat, "chat-1", 21),
        ];
        let attempt_id = attempt.common.attempt_id.clone();
        let cursor_before = attempt.common.cursor_before.clone();
        let cursor_after = attempt.common.cursor_through.clone();
        let completed_common = attempt.common.clone();
        let action = coordinator
            .admit_claimed_attempt(attempt)
            .unwrap()
            .expect("one coalesced provider dispatch");
        assert_eq!(action.request.envelope.entries.len(), 2);
        assert_eq!(
            action.request.envelope.entries[0].source_kind,
            SourceKind::Mail
        );
        assert_eq!(
            action.request.envelope.entries[1].source_kind,
            SourceKind::Chat
        );
        assert!(
            transport.requests.lock().unwrap().is_empty(),
            "provider dispatch alone must not acknowledge Cog or move a cursor"
        );

        let provider = ProviderReceipt {
            kind: ProviderKind::Codex,
            session_id: OpaqueId::new("provider-session").unwrap(),
            turn_id: Some(OpaqueId::new("turn-1").unwrap()),
            metadata: None,
        };
        transport.push_json(
            200,
            serde_json::to_value(AttemptMutationResponse {
                schema_version: ProtocolOne::V1,
                attempt: AttemptView {
                    common: completed_common,
                    status: AttemptStatus::Completed {
                        completion: CompletionReceipt {
                            receipt_id: OpaqueId::new("receipt-1").unwrap(),
                            attempt_id: attempt_id.clone(),
                            idempotency_key: ProtocolUuid::from_uuid(uuid::Uuid::nil()),
                            request_digest: Sha256Digest::new("b".repeat(64)).unwrap(),
                            address_id: OpaqueId::new("address").unwrap(),
                            cursor_before,
                            cursor_after,
                            provider: provider.clone(),
                            completed_at: timestamp(),
                            audit_event_id: DecimalU64(99),
                        },
                    },
                },
                idempotent_replay: false,
                server_time: timestamp(),
            })
            .unwrap(),
        );
        coordinator
            .record_provider_result(&attempt_id, ProviderDeliveryResult::Succeeded(provider))
            .unwrap();
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(body.get("cursor_before").is_none());
        assert!(body.get("cursor_after").is_none());
        assert!(body.get("advances").is_none());
        assert!(matches!(
            coordinator.journal.latest(&attempt_id).unwrap().state,
            JournalState::CogCompleted
        ));
    }

    #[test]
    fn restart_with_durable_provider_success_completes_without_redispatch() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("journal");
        let attempt = claimed_attempt("address", 1);
        let attempt_id = attempt.common.attempt_id.clone();
        {
            let mut journal = DeliveryJournal::open(&journal_path).unwrap();
            append_attempt_state(&mut journal, &attempt, JournalState::Claimed).unwrap();
            let key = ProtocolUuid::from_uuid(uuid::Uuid::nil());
            append_attempt_state(
                &mut journal,
                &attempt,
                JournalState::DispatchStarted {
                    idempotency_key: key,
                },
            )
            .unwrap();
            append_attempt_state(
                &mut journal,
                &attempt,
                JournalState::ProviderSucceeded {
                    idempotency_key: key,
                    provider: ProviderReceipt {
                        kind: ProviderKind::Codex,
                        session_id: OpaqueId::new("provider-session").unwrap(),
                        turn_id: Some(OpaqueId::new("8").unwrap()),
                        metadata: None,
                    },
                },
            )
            .unwrap();
        }
        let transport = Arc::new(ScriptedTransport::default());
        transport.push_json(
            409,
            serde_json::json!({"error":{
                "code":"attempt_already_terminal", "message":"completed before retry",
                "retryable":false, "details":{}
            }}),
        );
        let journal = DeliveryJournal::open(&journal_path).unwrap();
        let mut coordinator = coordinator(
            Arc::clone(&transport),
            journal,
            vec![route("address", "session")],
        );
        let mut reclaimed = attempt;
        reclaimed.common.attempt_fence = DecimalU64(2);
        assert!(
            coordinator
                .admit_claimed_attempt(reclaimed)
                .unwrap()
                .is_none()
        );
        assert_eq!(coordinator.inflight_len(), 0);
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
        assert!(matches!(
            coordinator.journal.latest(&attempt_id).unwrap().state,
            JournalState::TerminalObserved { .. }
        ));
    }

    #[test]
    fn restart_observes_completion_that_committed_before_response_was_lost() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("journal");
        let attempt = claimed_attempt("address", 1);
        let attempt_id = attempt.common.attempt_id.clone();
        let key = ProtocolUuid::from_uuid(uuid::Uuid::nil());
        let provider = ProviderReceipt {
            kind: ProviderKind::Codex,
            session_id: OpaqueId::new("provider-session").unwrap(),
            turn_id: Some(OpaqueId::new("8").unwrap()),
            metadata: None,
        };
        {
            let mut journal = DeliveryJournal::open(&journal_path).unwrap();
            append_attempt_state(&mut journal, &attempt, JournalState::Claimed).unwrap();
            append_attempt_state(
                &mut journal,
                &attempt,
                JournalState::DispatchStarted {
                    idempotency_key: key,
                },
            )
            .unwrap();
            append_attempt_state(
                &mut journal,
                &attempt,
                JournalState::ProviderSucceeded {
                    idempotency_key: key,
                    provider: provider.clone(),
                },
            )
            .unwrap();
        }

        let mut completed = attempt.clone();
        completed.status = AttemptStatus::Completed {
            completion: CompletionReceipt {
                receipt_id: OpaqueId::new("receipt").unwrap(),
                attempt_id: attempt_id.clone(),
                idempotency_key: key,
                request_digest: Sha256Digest::new("b".repeat(64)).unwrap(),
                address_id: OpaqueId::new("address").unwrap(),
                cursor_before: attempt.common.cursor_before.clone(),
                cursor_after: attempt.common.cursor_through.clone(),
                provider,
                completed_at: timestamp(),
                audit_event_id: DecimalU64(99),
            },
        };
        let transport = Arc::new(ScriptedTransport::default());
        transport.push_json(
            200,
            serde_json::to_value(AttemptResponse {
                schema_version: ProtocolOne::V1,
                attempt: completed,
                server_time: timestamp(),
            })
            .unwrap(),
        );
        let journal = DeliveryJournal::open(&journal_path).unwrap();
        let mut coordinator = coordinator(transport, journal, vec![route("address", "session")]);
        coordinator.reconcile_journal_terminals().unwrap();
        assert!(matches!(
            coordinator.journal.latest(&attempt_id).unwrap().state,
            JournalState::CogCompleted
        ));
        assert_eq!(coordinator.inflight_len(), 0);
    }

    #[test]
    fn claims_offer_all_eligible_routes_in_byte_order_with_bounded_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        transport.push_json(
            200,
            serde_json::json!({
                "schema_version":"1", "attempts":[], "remaining_due":false,
                "remaining_incompatible":false, "server_time":timestamp().as_str()
            }),
        );
        let routes = vec![route("z", "sz"), route("a", "sa")];
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = coordinator(Arc::clone(&transport), journal, routes);
        coordinator.claim_available().unwrap();
        let requests = transport.requests.lock().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["available_addresses"], serde_json::json!(["a", "z"]));
        assert_eq!(body["max_attempts"], "4");
    }

    #[test]
    fn unowned_or_retired_route_never_dispatches_even_if_server_returns_it() {
        let dir = tempfile::tempdir().unwrap();
        let transport = Arc::new(ScriptedTransport::default());
        let journal = DeliveryJournal::open(dir.path().join("journal")).unwrap();
        let mut coordinator = coordinator(transport, journal, vec![route("eligible", "session")]);
        assert!(
            coordinator
                .admit_claimed_attempt(claimed_attempt("retired", 1))
                .unwrap()
                .is_none()
        );
        assert_eq!(coordinator.inflight_len(), 0);
        assert_eq!(coordinator.journal.snapshot().next_seq, 1);
    }
}
