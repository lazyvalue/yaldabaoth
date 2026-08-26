//! Fsync-before-ack append-only recovery journal for runtime delivery.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::wire::{
    DecimalU64, FailureClass, OpaqueId, ProtocolUuid, ProviderReceipt, Sha256Digest, Timestamp,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalRecord {
    pub schema_version: JournalSchema,
    pub seq: DecimalU64,
    pub at: Timestamp,
    pub attempt_id: OpaqueId,
    pub delivery_key: OpaqueId,
    pub payload_digest: Sha256Digest,
    pub address_id: OpaqueId,
    pub host_fence: DecimalU64,
    pub owner_generation: DecimalU64,
    pub attempt_fence: DecimalU64,
    pub state: JournalState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalSchema {
    #[serde(rename = "1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalState {
    Claimed,
    DispatchStarted {
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
    CogCompleted,
    CogFailed,
    Released {
        reason: String,
    },
    TerminalObserved {
        reason: String,
    },
}

impl JournalState {
    pub fn is_locally_terminal(&self) -> bool {
        matches!(
            self,
            Self::CogCompleted | Self::CogFailed | Self::TerminalObserved { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JournalSnapshot {
    pub next_seq: u64,
    pub attempts: BTreeMap<OpaqueId, JournalRecord>,
}

pub struct DeliveryJournal {
    path: PathBuf,
    file: File,
    next_seq: u64,
    attempts: BTreeMap<OpaqueId, JournalRecord>,
}

impl DeliveryJournal {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;
        let (snapshot, valid_len) = replay(&mut file)?;
        if file.metadata()?.len() != valid_len {
            file.set_len(valid_len)?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            path,
            file,
            next_seq: snapshot.next_seq,
            attempts: snapshot.attempts,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> JournalSnapshot {
        JournalSnapshot {
            next_seq: self.next_seq,
            attempts: self.attempts.clone(),
        }
    }

    pub fn latest(&self, attempt_id: &OpaqueId) -> Option<&JournalRecord> {
        self.attempts.get(attempt_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &mut self,
        at: Timestamp,
        attempt_id: OpaqueId,
        delivery_key: OpaqueId,
        payload_digest: Sha256Digest,
        address_id: OpaqueId,
        host_fence: DecimalU64,
        owner_generation: DecimalU64,
        attempt_fence: DecimalU64,
        state: JournalState,
    ) -> io::Result<JournalRecord> {
        let record = JournalRecord {
            schema_version: JournalSchema::V1,
            seq: DecimalU64(self.next_seq),
            at,
            attempt_id,
            delivery_key,
            payload_digest,
            address_id,
            host_fence,
            owner_generation,
            attempt_fence,
            state,
        };
        validate_transition(self.attempts.get(&record.attempt_id), &record)?;
        let mut line = serde_json::to_vec(&record)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.sync_data()?;
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Cog journal sequence exhausted"))?;
        self.attempts
            .insert(record.attempt_id.clone(), record.clone());
        Ok(record)
    }
}

fn replay(file: &mut File) -> io::Result<(JournalSnapshot, u64)> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut attempts = BTreeMap::new();
    let mut expected_seq = 1_u64;
    let mut valid_len = 0_u64;
    let mut reader = BufReader::new(bytes.as_slice());
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        let terminated = line.last() == Some(&b'\n');
        if !terminated {
            break;
        }
        line.pop();
        if line.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Cog delivery journal contains an empty record",
            ));
        }
        let record: JournalRecord = serde_json::from_slice(&line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if record.seq.0 != expected_seq {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Cog delivery journal expected seq {expected_seq}, got {}",
                    record.seq
                ),
            ));
        }
        validate_transition(attempts.get(&record.attempt_id), &record)?;
        expected_seq = expected_seq
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Cog journal sequence exhausted"))?;
        attempts.insert(record.attempt_id.clone(), record);
        valid_len = valid_len.saturating_add(read as u64);
    }
    Ok((
        JournalSnapshot {
            next_seq: expected_seq,
            attempts,
        },
        valid_len,
    ))
}

fn validate_transition(previous: Option<&JournalRecord>, next: &JournalRecord) -> io::Result<()> {
    let Some(previous) = previous else {
        if matches!(next.state, JournalState::Claimed) {
            return Ok(());
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "first Cog journal state for an attempt must be claimed",
        ));
    };
    if previous.delivery_key != next.delivery_key
        || previous.payload_digest != next.payload_digest
        || previous.address_id != next.address_id
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Cog journal attempt identity changed across transitions",
        ));
    }
    if previous.state.is_locally_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Cog journal transition follows a terminal state",
        ));
    }
    let valid = matches!(
        (&previous.state, &next.state),
        (JournalState::Claimed, JournalState::DispatchStarted { .. })
            | (JournalState::Claimed, JournalState::Claimed)
            | (JournalState::Claimed, JournalState::Released { .. })
            | (JournalState::Claimed, JournalState::TerminalObserved { .. })
            | (JournalState::DispatchStarted { .. }, JournalState::Claimed)
            | (JournalState::Released { .. }, JournalState::Claimed)
            | (
                JournalState::DispatchStarted { .. },
                JournalState::ProviderSucceeded { .. }
            )
            | (
                JournalState::DispatchStarted { .. },
                JournalState::ProviderFailed { .. }
            )
            | (
                JournalState::DispatchStarted { .. },
                JournalState::Released { .. }
            )
            | (
                JournalState::DispatchStarted { .. },
                JournalState::TerminalObserved { .. }
            )
            | (
                JournalState::Released { .. },
                JournalState::TerminalObserved { .. }
            )
            | (
                JournalState::ProviderSucceeded { .. },
                JournalState::CogCompleted
            )
            | (
                JournalState::ProviderSucceeded { .. },
                JournalState::TerminalObserved { .. }
            )
            | (JournalState::ProviderFailed { .. }, JournalState::CogFailed)
            | (
                JournalState::ProviderFailed { .. },
                JournalState::TerminalObserved { .. }
            )
    );
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid Cog journal transition {:?} -> {:?}",
                previous.state, next.state
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp() -> Timestamp {
        Timestamp::new("2026-08-25T18:02:03.123456789Z").unwrap()
    }

    fn append_claim(journal: &mut DeliveryJournal) {
        journal
            .append(
                timestamp(),
                OpaqueId::new("attempt").unwrap(),
                OpaqueId::new("delivery").unwrap(),
                Sha256Digest::new("a".repeat(64)).unwrap(),
                OpaqueId::new("address").unwrap(),
                DecimalU64(1),
                DecimalU64(1),
                DecimalU64(1),
                JournalState::Claimed,
            )
            .unwrap();
    }

    #[test]
    fn append_fsync_and_reopen_preserve_latest_attempt_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.ndjson");
        {
            let mut journal = DeliveryJournal::open(&path).unwrap();
            append_claim(&mut journal);
            journal
                .append(
                    timestamp(),
                    OpaqueId::new("attempt").unwrap(),
                    OpaqueId::new("delivery").unwrap(),
                    Sha256Digest::new("a".repeat(64)).unwrap(),
                    OpaqueId::new("address").unwrap(),
                    DecimalU64(1),
                    DecimalU64(1),
                    DecimalU64(1),
                    JournalState::DispatchStarted {
                        idempotency_key: ProtocolUuid::from_uuid(uuid::Uuid::nil()),
                    },
                )
                .unwrap();
        }
        let journal = DeliveryJournal::open(&path).unwrap();
        assert_eq!(journal.snapshot().next_seq, 3);
        assert!(matches!(
            journal
                .latest(&OpaqueId::new("attempt").unwrap())
                .unwrap()
                .state,
            JournalState::DispatchStarted { .. }
        ));
    }

    #[test]
    fn only_a_torn_final_record_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.ndjson");
        let mut journal = DeliveryJournal::open(&path).unwrap();
        append_claim(&mut journal);
        drop(journal);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"schema_version\":").unwrap();
        file.sync_data().unwrap();
        drop(file);

        let mut reopened = DeliveryJournal::open(&path).unwrap();
        assert_eq!(reopened.snapshot().next_seq, 2);
        reopened
            .append(
                timestamp(),
                OpaqueId::new("attempt").unwrap(),
                OpaqueId::new("delivery").unwrap(),
                Sha256Digest::new("a".repeat(64)).unwrap(),
                OpaqueId::new("address").unwrap(),
                DecimalU64(1),
                DecimalU64(1),
                DecimalU64(1),
                JournalState::Released {
                    reason: "recovered".into(),
                },
            )
            .unwrap();
        drop(reopened);
        assert_eq!(DeliveryJournal::open(&path).unwrap().snapshot().next_seq, 3);
    }
}
