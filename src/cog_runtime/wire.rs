//! Exact data model for `application/vnd.cog.runtime-delivery.v1+json`.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

pub const PROTOCOL_VERSION: &str = "1";
pub const MEDIA_TYPE: &str = "application/vnd.cog.runtime-delivery.v1+json";
pub const SSE_MEDIA_TYPE: &str = "text/event-stream";
pub const REQUIRED_FEATURES: &[&str] = &[
    "durable-owner",
    "host-fence",
    "attempt-fence",
    "attempt-renew",
    "capacity-claim",
    "completion-receipt",
    "wake-sse",
    "source-cursor-vector-v1",
];

/// A v1 U64 is always a canonical decimal JSON string, never a JSON number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DecimalU64(pub u64);

impl fmt::Display for DecimalU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for DecimalU64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = DecimalU64;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a canonical decimal u64 JSON string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.is_empty()
                    || (value.len() > 1 && value.starts_with('0'))
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(E::custom("U64 must match ^(0|[1-9][0-9]*)$"));
                }
                value
                    .parse::<u64>()
                    .map(DecimalU64)
                    .map_err(|_| E::custom("U64 is out of range"))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// A non-empty opaque identifier. No format or normalization is inferred.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueId(String);

impl OpaqueId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.is_empty() {
            Err("opaque id must not be empty".into())
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OpaqueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for OpaqueId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Opaque endpoint-scoped pagination cursor.
pub type PageCursor = OpaqueId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(Self(value))
        } else {
            Err("SHA-256 must be 64 lowercase hexadecimal characters".into())
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// Lowercase RFC4122 hyphenated UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProtocolUuid(uuid::Uuid);

impl ProtocolUuid {
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }
}

impl fmt::Display for ProtocolUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl Serialize for ProtocolUuid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ProtocolUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let parsed = uuid::Uuid::parse_str(&raw).map_err(de::Error::custom)?;
        if raw != parsed.hyphenated().to_string() {
            return Err(de::Error::custom(
                "UUID must be lowercase RFC4122 hyphenated form",
            ));
        }
        Ok(Self(parsed))
    }
}

/// UTC RFC3339 with exactly nine fractional digits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Timestamp(String);

impl Timestamp {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_timestamp(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

fn validate_timestamp(value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() != 30
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[29] != b'Z'
        || !bytes
            .iter()
            .enumerate()
            .filter(|(i, _)| ![4, 7, 10, 13, 16, 19, 29].contains(i))
            .all(|(_, b)| b.is_ascii_digit())
    {
        return Err("timestamp must be UTC RFC3339 with exactly nine fractional digits".into());
    }
    let part =
        |a: usize, b: usize| -> u32 { value[a..b].parse::<u32>().expect("digits validated") };
    let year = part(0, 4);
    let month = part(5, 7);
    let day = part(8, 10);
    let hour = part(11, 13);
    let minute = part(14, 16);
    let second = part(17, 19);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    if day == 0 || day > max_day || hour > 23 || minute > 59 || second > 59 {
        return Err("timestamp contains an out-of-range calendar field".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Codex,
    Claude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Mail,
    Chat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CursorOwner {
    Mail,
    Chat { chat_id: OpaqueId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPoint {
    pub owner: CursorOwner,
    pub position: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorVector {
    pub kind: SourceVectorLiteral,
    pub points: Vec<CursorPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceVectorLiteral {
    #[serde(rename = "source_vector")]
    SourceVector,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorAdvance {
    pub owner: CursorOwner,
    pub before: DecimalU64,
    pub through: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedReference {
    pub kind: String,
    pub id: OpaqueId,
    pub state: ReferenceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceState {
    Live,
    Tombstoned,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryEntry {
    pub event_id: DecimalU64,
    pub source_kind: SourceKind,
    pub source_id: OpaqueId,
    pub source_name: String,
    pub topic_addresses: Vec<String>,
    pub entry_id: OpaqueId,
    pub from: OpaqueId,
    pub audit_actor: String,
    pub at: Timestamp,
    pub content: serde_json::Value,
    pub content_size_bytes: DecimalU64,
    pub references: Vec<ResolvedReference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptCommon {
    pub attempt_id: OpaqueId,
    pub delivery_key: OpaqueId,
    pub payload_digest: Sha256Digest,
    pub attempt_fence: DecimalU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_id: Option<OpaqueId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_fence: Option<DecimalU64>,
    pub address_id: OpaqueId,
    pub owner_generation: DecimalU64,
    pub created_at: Timestamp,
    pub cursor_before: CursorVector,
    pub cursor_through: CursorVector,
    pub advances: Vec<CursorAdvance>,
    pub oversize: bool,
    pub entries: Vec<DeliveryEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptView {
    #[serde(flatten)]
    pub common: AttemptCommon,
    pub status: AttemptStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttemptStatus {
    Claimed {
        instance_id: ProtocolUuid,
        claimed_at: Timestamp,
        lease_expires_at: Timestamp,
    },
    Recoverable {
        available_at: Timestamp,
        cause: RecoverableCause,
        #[serde(skip_serializing_if = "Option::is_none")]
        superseded_claim: Option<SupersededClaim>,
    },
    RetryWait {
        failure: FailureReceipt,
    },
    Blocked {
        failure: FailureReceipt,
    },
    Completed {
        completion: CompletionReceipt,
    },
    Superseded {
        supersession: SupersessionReceipt,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverableCause {
    Released,
    HostFenceChanged,
    AttemptExpired,
    OwnerTransferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum DeliveryOwner {
    Cogd {
        owner_generation: DecimalU64,
    },
    External {
        host_id: OpaqueId,
        owner_generation: DecimalU64,
    },
}

impl DeliveryOwner {
    pub fn generation(&self) -> DecimalU64 {
        match self {
            Self::Cogd { owner_generation }
            | Self::External {
                owner_generation, ..
            } => *owner_generation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupersededClaim {
    pub owner: DeliveryOwner,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_id: Option<OpaqueId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_fence: Option<DecimalU64>,
    pub attempt_fence: DecimalU64,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderReceipt {
    pub kind: ProviderKind,
    pub session_id: OpaqueId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<OpaqueId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionReceipt {
    pub receipt_id: OpaqueId,
    pub attempt_id: OpaqueId,
    pub idempotency_key: ProtocolUuid,
    pub request_digest: Sha256Digest,
    pub address_id: OpaqueId,
    pub cursor_before: CursorVector,
    pub cursor_after: CursorVector,
    pub provider: ProviderReceipt,
    pub completed_at: Timestamp,
    pub audit_event_id: DecimalU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    ProviderUnavailable,
    ProviderRejected,
    Timeout,
    Cancelled,
    UnsupportedContractValue,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureReceipt {
    pub failure_id: OpaqueId,
    pub attempt_id: OpaqueId,
    pub idempotency_key: ProtocolUuid,
    pub request_digest: Sha256Digest,
    pub address_id: OpaqueId,
    pub attempt_fence: DecimalU64,
    pub class: FailureClass,
    pub retryable: bool,
    pub message: String,
    pub failed_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<Timestamp>,
    pub audit_event_id: DecimalU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupersessionReason {
    ChatLeft,
    AddressRetired,
    OperatorSkip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupersessionReceipt {
    pub receipt_id: OpaqueId,
    pub attempt_id: OpaqueId,
    pub reason: SupersessionReason,
    pub at: Timestamp,
    pub cursor_before: CursorVector,
    pub cursor_after: CursorVector,
    pub audit_event_id: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLease {
    pub host_id: OpaqueId,
    pub instance_id: ProtocolUuid,
    pub host_fence: DecimalU64,
    pub protocol_version: ProtocolOne,
    pub source_kinds: Vec<SourceKind>,
    pub provider_kinds: Vec<ProviderKind>,
    pub lease_expires_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolOne {
    #[serde(rename = "1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitRange {
    pub min: DecimalU64,
    pub max: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityLimits {
    pub host_lease_seconds: LimitRange,
    pub attempt_lease_seconds: LimitRange,
    pub max_claim_attempts: DecimalU64,
    pub max_claim_entries: DecimalU64,
    pub max_claim_content_bytes: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub schema_version: ProtocolOne,
    pub protocol_versions: Vec<String>,
    pub source_kinds: Vec<SourceKind>,
    pub provider_kinds: Vec<ProviderKind>,
    pub features: Vec<String>,
    pub limits: CapabilityLimits,
    pub server_time: Timestamp,
}

impl Capabilities {
    pub fn compatibility_error(&self, providers: &[ProviderKind]) -> Option<String> {
        if !self.protocol_versions.iter().any(|v| v == PROTOCOL_VERSION) {
            return Some("protocol version 1 is not advertised".into());
        }
        for source in [SourceKind::Mail, SourceKind::Chat] {
            if !self.source_kinds.contains(&source) {
                return Some(format!("required source kind {source:?} is missing"));
            }
        }
        for provider in providers {
            if !self.provider_kinds.contains(provider) {
                return Some(format!("configured provider {provider:?} is missing"));
            }
        }
        for feature in REQUIRED_FEATURES {
            if !self.features.iter().any(|f| f == feature) {
                return Some(format!("required feature {feature} is missing"));
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLeaseResponse {
    pub schema_version: ProtocolOne,
    pub lease: HostLease,
    pub live: bool,
    pub server_time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLeaseReleaseResponse {
    pub schema_version: ProtocolOne,
    pub host_id: OpaqueId,
    pub instance_id: ProtocolUuid,
    pub host_fence: DecimalU64,
    pub released_at: Timestamp,
    pub server_time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryOwnerResponse {
    pub schema_version: ProtocolOne,
    pub address_id: OpaqueId,
    pub owner: DeliveryOwner,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transferred_attempt_id: Option<OpaqueId>,
    pub server_time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptMutationResponse {
    pub schema_version: ProtocolOne,
    pub attempt: AttemptView,
    pub idempotent_replay: bool,
    pub server_time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptResponse {
    pub schema_version: ProtocolOne,
    pub attempt: AttemptView,
    pub server_time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptListResponse {
    pub schema_version: ProtocolOne,
    pub attempts: Vec<AttemptView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<PageCursor>,
    pub server_time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimResponse {
    pub schema_version: ProtocolOne,
    pub attempts: Vec<AttemptView>,
    pub remaining_due: bool,
    pub remaining_incompatible: bool,
    pub server_time: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseAcquireRequest {
    pub instance_id: ProtocolUuid,
    pub protocol_version: ProtocolOne,
    pub source_kinds: Vec<SourceKind>,
    pub provider_kinds: Vec<ProviderKind>,
    pub lease_seconds: DecimalU64,
    pub takeover: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_host_fence: Option<DecimalU64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRenewRequest {
    pub instance_id: ProtocolUuid,
    pub host_fence: DecimalU64,
    pub lease_seconds: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseReleaseRequest {
    pub instance_id: ProtocolUuid,
    pub host_fence: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryOwnerPutRequest {
    pub owner: DeliveryOwnerSelection,
    pub expected_owner_generation: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum DeliveryOwnerSelection {
    Cogd,
    External { host_id: OpaqueId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRequest {
    pub instance_id: ProtocolUuid,
    pub host_fence: DecimalU64,
    pub available_addresses: Vec<OpaqueId>,
    pub max_attempts: DecimalU64,
    pub max_entries: DecimalU64,
    pub max_content_bytes: DecimalU64,
    pub attempt_lease_seconds: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptFenceRequest {
    pub instance_id: ProtocolUuid,
    pub host_fence: DecimalU64,
    pub owner_generation: DecimalU64,
    pub attempt_fence: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRenewRequest {
    #[serde(flatten)]
    pub fences: AttemptFenceRequest,
    pub lease_seconds: DecimalU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptReleaseRequest {
    #[serde(flatten)]
    pub fences: AttemptFenceRequest,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptCompleteRequest {
    #[serde(flatten)]
    pub fences: AttemptFenceRequest,
    pub idempotency_key: ProtocolUuid,
    pub provider: ProviderReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptFailRequest {
    #[serde(flatten)]
    pub fences: AttemptFenceRequest,
    pub idempotency_key: ProtocolUuid,
    pub class: FailureClass,
    pub retryable: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub details: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    InvalidCapacity,
    InvalidLeaseSeconds,
    HostNotFound,
    AddressNotFound,
    AttemptNotFound,
    HostLeaseConflict,
    StaleHostFence,
    StaleOwnerGeneration,
    StaleAttemptFence,
    AttemptLeaseExpired,
    AttemptAlreadyTerminal,
    IdempotencyConflict,
    CursorConflict,
    RetiredAddress,
    UnsupportedContractValue,
    ProtocolVersionUnsupported,
    TemporarilyUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryWake {
    pub schema_version: ProtocolOne,
    pub wake_id: DecimalU64,
    pub host_id: OpaqueId,
    pub due_since: Timestamp,
    pub due_addresses: Vec<OpaqueId>,
    pub reason: WakeReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WakeReason {
    Mail,
    Chat,
    Both,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_u64_is_always_a_lossless_canonical_string() {
        let value: DecimalU64 = serde_json::from_str(r#""18446744073709551615""#).unwrap();
        assert_eq!(value.0, u64::MAX);
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            r#""18446744073709551615""#
        );
        for invalid in ["0", r#""01""#, r#""+1""#, r#""18446744073709551616""#] {
            if invalid == "0" {
                assert!(serde_json::from_str::<DecimalU64>(invalid).is_err());
            } else {
                assert!(
                    serde_json::from_str::<DecimalU64>(invalid).is_err(),
                    "{invalid}"
                );
            }
        }
        assert_eq!(serde_json::from_str::<DecimalU64>(r#""0""#).unwrap().0, 0);
    }

    #[test]
    fn timestamps_require_nine_digits_and_real_calendar_fields() {
        assert!(Timestamp::new("2026-08-25T18:02:03.123456789Z").is_ok());
        assert!(Timestamp::new("2026-02-29T18:02:03.123456789Z").is_err());
        assert!(Timestamp::new("2024-02-29T18:02:03.123456789Z").is_ok());
        assert!(Timestamp::new("2026-08-25T18:02:03.123Z").is_err());
        assert!(Timestamp::new("2026-08-25T18:02:60.123456789Z").is_err());
    }

    #[test]
    fn status_and_error_unions_fail_closed() {
        assert!(serde_json::from_str::<AttemptStatus>(r#"{"kind":"future"}"#).is_err());
        assert!(serde_json::from_str::<ErrorCode>(r#""future_error""#).is_err());
        assert_eq!(
            serde_json::from_str::<SupersessionReason>(r#""address_retired""#).unwrap(),
            SupersessionReason::AddressRetired
        );
    }

    #[test]
    fn opaque_types_reject_empty_or_noncanonical_values() {
        assert!(serde_json::from_str::<OpaqueId>(r#""""#).is_err());
        assert!(serde_json::from_str::<Sha256Digest>(&format!("\"{}\"", "a".repeat(64))).is_ok());
        assert!(serde_json::from_str::<Sha256Digest>(&format!("\"{}\"", "A".repeat(64))).is_err());
        assert!(
            serde_json::from_str::<ProtocolUuid>(r#""550E8400-E29B-41D4-A716-446655440000""#)
                .is_err()
        );
    }
}
