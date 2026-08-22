use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{Error, ObjectId, Result, UniqueId};

const MAGIC: &[u8; 8] = b"ATLASJNL";
const VERSION: u16 = 1;
const HEADER_LENGTH: usize = 40;
const CHECKSUM_LENGTH: usize = 32;
const SEGMENT_NAME: &str = "0000000000000001.log";
const MAX_SEGMENT_SIZE: u64 = 256 * 1024 * 1024;
const MAX_RECORD_PAYLOAD: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum RecordKind {
    Begin = 1,
    ObjectsDurable = 2,
    Commit = 3,
    Complete = 4,
    Abort = 5,
    Checkpoint = 6,
}

impl TryFrom<u8> for RecordKind {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Begin),
            2 => Ok(Self::ObjectsDurable),
            3 => Ok(Self::Commit),
            4 => Ok(Self::Complete),
            5 => Ok(Self::Abort),
            6 => Ok(Self::Checkpoint),
            _ => Err(Error::JournalCorrupt("unknown record kind".to_owned())),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Record {
    pub sequence: u64,
    pub transaction: UniqueId,
    pub kind: RecordKind,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct Scan {
    pub records: Vec<Record>,
    pub valid_length: u64,
    pub torn_tail: bool,
}

#[derive(Debug)]
pub(crate) struct Journal {
    path: PathBuf,
}

impl Journal {
    pub fn open(repository: &Path) -> Self {
        Self {
            path: repository.join("journal").join(SEGMENT_NAME),
        }
    }

    pub fn append(&self, transaction: UniqueId, kind: RecordKind, payload: &[u8]) -> Result<u64> {
        if payload.len() > MAX_RECORD_PAYLOAD {
            return Err(Error::ResourceLimit {
                requested: payload.len() as u64,
                maximum: MAX_RECORD_PAYLOAD as u64,
            });
        }
        let scan = self.scan()?;
        if scan.torn_tail {
            self.truncate(scan.valid_length)?;
        }
        let sequence = scan
            .records
            .last()
            .map_or(1, |record| record.sequence.saturating_add(1));
        if sequence == u64::MAX {
            return Err(Error::JournalCorrupt("sequence overflow".to_owned()));
        }
        let payload_length = u32::try_from(payload.len())
            .map_err(|_| Error::JournalCorrupt("record payload too large".to_owned()))?;
        let mut encoded = Vec::with_capacity(HEADER_LENGTH + payload.len() + CHECKSUM_LENGTH);
        encoded.extend_from_slice(MAGIC);
        encoded.extend_from_slice(&VERSION.to_be_bytes());
        encoded.push(kind as u8);
        encoded.push(0);
        encoded.extend_from_slice(&sequence.to_be_bytes());
        encoded.extend_from_slice(transaction.as_bytes());
        encoded.extend_from_slice(&payload_length.to_be_bytes());
        encoded.extend_from_slice(payload);
        let checksum = Sha256::digest(&encoded);
        encoded.extend_from_slice(&checksum);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| Error::io("open journal segment", &self.path, source))?;
        file.write_all(&encoded)
            .map_err(|source| Error::io("append journal record", &self.path, source))?;
        file.sync_all()
            .map_err(|source| Error::io("flush journal record", &self.path, source))?;
        Ok(sequence)
    }

    pub fn scan(&self) -> Result<Scan> {
        if !self.path.exists() {
            return Ok(Scan {
                records: Vec::new(),
                valid_length: 0,
                torn_tail: false,
            });
        }
        let segment_size = fs::metadata(&self.path)
            .map_err(|source| Error::io("inspect journal segment", &self.path, source))?
            .len();
        if segment_size > MAX_SEGMENT_SIZE {
            return Err(Error::ResourceLimit {
                requested: segment_size,
                maximum: MAX_SEGMENT_SIZE,
            });
        }
        let mut bytes = Vec::new();
        File::open(&self.path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|source| Error::io("read journal segment", &self.path, source))?;
        let mut records = Vec::new();
        let mut position = 0_usize;
        let mut expected_sequence = 1_u64;
        while position < bytes.len() {
            if bytes.len() - position < HEADER_LENGTH {
                return Ok(torn(records, position));
            }
            let header = &bytes[position..position + HEADER_LENGTH];
            if &header[..8] != MAGIC {
                return Err(Error::JournalCorrupt(format!(
                    "invalid magic at offset {position}"
                )));
            }
            let version = u16::from_be_bytes(
                header[8..10]
                    .try_into()
                    .map_err(|_| Error::JournalCorrupt("invalid version".to_owned()))?,
            );
            if version != VERSION {
                return Err(Error::JournalCorrupt(format!(
                    "unsupported version {version}"
                )));
            }
            let kind = RecordKind::try_from(header[10])?;
            if header[11] != 0 {
                return Err(Error::JournalCorrupt(
                    "reserved byte is not zero".to_owned(),
                ));
            }
            let sequence = u64::from_be_bytes(
                header[12..20]
                    .try_into()
                    .map_err(|_| Error::JournalCorrupt("invalid sequence".to_owned()))?,
            );
            if sequence != expected_sequence {
                return Err(Error::JournalCorrupt(format!(
                    "expected sequence {expected_sequence}, found {sequence}"
                )));
            }
            let mut transaction_bytes = [0_u8; 16];
            transaction_bytes.copy_from_slice(&header[20..36]);
            let transaction = UniqueId::from_bytes(transaction_bytes);
            let payload_length =
                usize::try_from(u32::from_be_bytes(header[36..40].try_into().map_err(
                    |_| Error::JournalCorrupt("invalid payload length".to_owned()),
                )?))
                .map_err(|_| Error::JournalCorrupt("payload length unsupported".to_owned()))?;
            let record_length = HEADER_LENGTH
                .checked_add(payload_length)
                .and_then(|value| value.checked_add(CHECKSUM_LENGTH))
                .ok_or_else(|| Error::JournalCorrupt("record length overflow".to_owned()))?;
            if bytes.len() - position < record_length {
                return Ok(torn(records, position));
            }
            let checksum_start = position + HEADER_LENGTH + payload_length;
            let expected = Sha256::digest(&bytes[position..checksum_start]);
            if expected.as_slice() != &bytes[checksum_start..checksum_start + CHECKSUM_LENGTH] {
                if position + record_length < bytes.len() {
                    return Err(Error::JournalCorrupt(format!(
                        "checksum failure before journal tail at offset {position}"
                    )));
                }
                return Ok(torn(records, position));
            }
            records.push(Record {
                sequence,
                transaction,
                kind,
                payload: bytes[position + HEADER_LENGTH..checksum_start].to_vec(),
            });
            position += record_length;
            expected_sequence = expected_sequence
                .checked_add(1)
                .ok_or_else(|| Error::JournalCorrupt("sequence overflow".to_owned()))?;
        }
        Ok(Scan {
            records,
            valid_length: position as u64,
            torn_tail: false,
        })
    }

    pub fn truncate(&self, length: u64) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|source| Error::io("open journal for repair", &self.path, source))?;
        file.set_len(length)
            .map_err(|source| Error::io("truncate torn journal tail", &self.path, source))?;
        file.seek(SeekFrom::Start(length))
            .map_err(|source| Error::io("seek repaired journal", &self.path, source))?;
        file.sync_all()
            .map_err(|source| Error::io("flush repaired journal", &self.path, source))
    }

    pub fn states(&self) -> Result<BTreeMap<UniqueId, TransactionState>> {
        let scan = self.scan()?;
        let mut states = BTreeMap::new();
        for record in scan.records {
            let state = states
                .entry(record.transaction)
                .or_insert_with(TransactionState::default);
            match record.kind {
                RecordKind::Begin => {
                    require_len(&record.payload, 40)?;
                    state.parent = Some(id(&record.payload[..32]));
                    state.generation = Some(u64::from_be_bytes(
                        record.payload[32..40]
                            .try_into()
                            .map_err(|_| Error::JournalCorrupt("invalid generation".to_owned()))?,
                    ));
                }
                RecordKind::ObjectsDurable => {
                    require_len(&record.payload, 32)?;
                    state.proposed = Some(id(&record.payload));
                }
                RecordKind::Commit => {
                    require_len(&record.payload, 64)?;
                    state.parent = Some(id(&record.payload[..32]));
                    state.proposed = Some(id(&record.payload[32..]));
                    state.committed = true;
                }
                RecordKind::Complete => {
                    require_len(&record.payload, 32)?;
                    state.completed = true;
                    state.proposed = Some(id(&record.payload));
                }
                RecordKind::Abort => state.aborted = true,
                RecordKind::Checkpoint => {}
            }
        }
        Ok(states)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TransactionState {
    pub parent: Option<ObjectId>,
    pub proposed: Option<ObjectId>,
    pub generation: Option<u64>,
    pub committed: bool,
    pub completed: bool,
    pub aborted: bool,
}

pub(crate) fn begin_payload(parent: ObjectId, generation: u64) -> Vec<u8> {
    let mut value = Vec::with_capacity(40);
    value.extend_from_slice(parent.as_bytes());
    value.extend_from_slice(&generation.to_be_bytes());
    value
}
pub(crate) fn one_id_payload(value: ObjectId) -> Vec<u8> {
    value.as_bytes().to_vec()
}
pub(crate) fn commit_payload(parent: ObjectId, proposed: ObjectId) -> Vec<u8> {
    let mut value = Vec::with_capacity(64);
    value.extend_from_slice(parent.as_bytes());
    value.extend_from_slice(proposed.as_bytes());
    value
}
pub(crate) fn checkpoint_payload(sequence: u64, root: ObjectId) -> Vec<u8> {
    let mut value = Vec::with_capacity(40);
    value.extend_from_slice(&sequence.to_be_bytes());
    value.extend_from_slice(root.as_bytes());
    value
}
fn id(bytes: &[u8]) -> ObjectId {
    let mut value = [0_u8; 32];
    value.copy_from_slice(bytes);
    ObjectId::from_bytes(value)
}
fn require_len(bytes: &[u8], expected: usize) -> Result<()> {
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(Error::JournalCorrupt(
            "record payload length mismatch".to_owned(),
        ))
    }
}

fn torn(records: Vec<Record>, position: usize) -> Scan {
    Scan {
        records,
        valid_length: u64::try_from(position).unwrap_or(u64::MAX),
        torn_tail: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repository;
    #[test]
    fn records_round_trip_and_torn_tail_is_identified() {
        let temp = crate::repository::test_directory("journal");
        let repository = Repository::init(temp.path()).unwrap();
        let journal = Journal::open(repository.path());
        let transaction = UniqueId::new();
        journal
            .append(
                transaction,
                RecordKind::Begin,
                &begin_payload(repository.head(), 1),
            )
            .unwrap();
        journal
            .append(
                transaction,
                RecordKind::Commit,
                &commit_payload(repository.head(), repository.head()),
            )
            .unwrap();
        let scan = journal.scan().unwrap();
        assert_eq!(scan.records.len(), 2);
        assert!(!scan.torn_tail);
        let mut file = OpenOptions::new().append(true).open(&journal.path).unwrap();
        file.write_all(b"torn").unwrap();
        file.sync_all().unwrap();
        let scan = journal.scan().unwrap();
        assert!(scan.torn_tail);
        assert_eq!(scan.records.len(), 2);
        journal.truncate(scan.valid_length).unwrap();
        assert!(!journal.scan().unwrap().torn_tail);
    }

    #[test]
    fn oversized_segment_is_rejected_before_allocation() {
        let directory = crate::repository::test_directory("large-journal");
        fs::create_dir_all(directory.path().join("journal")).unwrap();
        let journal = Journal::open(directory.path());
        let file = File::create(&journal.path).unwrap();
        file.set_len(MAX_SEGMENT_SIZE + 1).unwrap();
        assert!(matches!(journal.scan(), Err(Error::ResourceLimit { .. })));
    }
}
