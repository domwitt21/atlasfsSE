use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::{Error, Object, ObjectId, ObjectKind, ObjectStore, Result};

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UniqueId([u8; 16]);

impl UniqueId {
    #[must_use]
    pub fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| value.as_nanos());
        let mut hash = Sha256::new();
        hash.update(nanos.to_be_bytes());
        hash.update(std::process::id().to_be_bytes());
        hash.update(ID_COUNTER.fetch_add(1, Ordering::Relaxed).to_be_bytes());
        let digest = hash.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Self(bytes)
    }
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl Default for UniqueId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

impl Timestamp {
    /// Returns the current wall-clock timestamp.
    /// # Errors
    /// Returns an error if the system clock cannot be represented by format version 1.
    pub fn now() -> Result<Self> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::InvalidObject("system clock predates Unix epoch"))?;
        Ok(Self {
            seconds: i64::try_from(duration.as_secs())
                .map_err(|_| Error::InvalidObject("system time is out of range"))?,
            nanoseconds: duration.subsec_nanos(),
        })
    }
    fn encode(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.seconds.to_be_bytes());
        output.extend_from_slice(&self.nanoseconds.to_be_bytes());
    }
    fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let value = Self {
            seconds: reader.i64()?,
            nanoseconds: reader.u32()?,
        };
        if value.nanoseconds > 999_999_999 {
            return Err(Error::InvalidObject("timestamp nanoseconds out of range"));
        }
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum NodeKind {
    File = 1,
    Directory = 2,
}

impl TryFrom<u8> for NodeKind {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::File),
            2 => Ok(Self::Directory),
            _ => Err(Error::InvalidObject("unknown inode kind")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    pub name: String,
    pub inode: ObjectId,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Directory {
    entries: BTreeMap<String, ObjectId>,
}

impl Directory {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<String, ObjectId> {
        &self.entries
    }
    #[must_use]
    pub fn get(&self, name: &str) -> Option<ObjectId> {
        self.entries.get(name).copied()
    }
    pub fn insert(&mut self, name: String, inode: ObjectId) -> Option<ObjectId> {
        self.entries.insert(name, inode)
    }
    pub fn remove(&mut self, name: &str) -> Option<ObjectId> {
        self.entries.remove(name)
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Stores this canonical directory.
    /// # Errors
    /// Returns an error for invalid names, excessive entries, or storage failure.
    pub fn store(&self, store: &ObjectStore) -> Result<ObjectId> {
        let count = u32::try_from(self.entries.len())
            .map_err(|_| Error::InvalidObject("too many directory entries"))?;
        let mut payload = Vec::new();
        payload.extend_from_slice(&count.to_be_bytes());
        for (name, inode) in &self.entries {
            validate_name(name)?;
            let bytes = name.as_bytes();
            let length =
                u32::try_from(bytes.len()).map_err(|_| Error::InvalidObject("name too long"))?;
            payload.extend_from_slice(&length.to_be_bytes());
            payload.extend_from_slice(bytes);
            payload.extend_from_slice(inode.as_bytes());
        }
        store.put(&Object::new(ObjectKind::Directory, payload)?)
    }

    /// Loads and validates a directory object.
    /// # Errors
    /// Returns an error when the object is missing, corrupt, the wrong kind, or noncanonical.
    pub fn load(id: ObjectId, store: &ObjectStore) -> Result<Self> {
        let object = store.get(id)?;
        if object.kind() != ObjectKind::Directory {
            return Err(kind_mismatch(ObjectKind::Directory, object.kind()));
        }
        let mut reader = Reader::new(object.payload());
        let count = usize::try_from(reader.u32()?)
            .map_err(|_| Error::InvalidObject("directory count unsupported"))?;
        let mut entries = BTreeMap::new();
        let mut previous: Option<Vec<u8>> = None;
        for _ in 0..count {
            let length = usize::try_from(reader.u32()?)
                .map_err(|_| Error::InvalidObject("name length unsupported"))?;
            let raw = reader.take(length)?;
            let name = std::str::from_utf8(raw)
                .map_err(|_| Error::InvalidObject("directory name is not UTF-8"))?
                .to_owned();
            validate_name(&name)?;
            if previous.as_deref().is_some_and(|old| old >= raw) {
                return Err(Error::InvalidObject(
                    "directory entries are not strictly ordered",
                ));
            }
            previous = Some(raw.to_vec());
            let inode = ObjectId::from_bytes(reader.array()?);
            entries.insert(name, inode);
        }
        reader.finish()?;
        Ok(Self { entries })
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.bytes().any(|byte| matches!(byte, 0 | b'/' | b'\\'))
    {
        return Err(Error::InvalidObject("invalid directory entry name"));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inode {
    pub kind: NodeKind,
    pub node_id: UniqueId,
    pub previous: Option<ObjectId>,
    pub mode: u32,
    pub owner: u32,
    pub group: u32,
    pub created: Timestamp,
    pub modified: Timestamp,
    pub changed: Timestamp,
    pub accessed: Timestamp,
    pub content: ObjectId,
}

impl Inode {
    /// Stores this inode version.
    /// # Errors
    /// Returns an error when metadata is invalid or storage fails.
    pub fn store(&self, store: &ObjectStore) -> Result<ObjectId> {
        if self.mode > 0o777 {
            return Err(Error::InvalidObject("mode contains unsupported bits"));
        }
        let mut payload = Vec::new();
        payload.push(self.kind as u8);
        payload.extend_from_slice(self.node_id.as_bytes());
        encode_optional_id(self.previous, &mut payload);
        payload.extend_from_slice(&self.mode.to_be_bytes());
        payload.extend_from_slice(&self.owner.to_be_bytes());
        payload.extend_from_slice(&self.group.to_be_bytes());
        self.created.encode(&mut payload);
        self.modified.encode(&mut payload);
        self.changed.encode(&mut payload);
        self.accessed.encode(&mut payload);
        payload.extend_from_slice(self.content.as_bytes());
        store.put(&Object::new(ObjectKind::Inode, payload)?)
    }
    /// Loads and validates an inode object.
    /// # Errors
    /// Returns an error when the object is missing, corrupt, the wrong kind, or malformed.
    pub fn load(id: ObjectId, store: &ObjectStore) -> Result<Self> {
        let object = store.get(id)?;
        if object.kind() != ObjectKind::Inode {
            return Err(kind_mismatch(ObjectKind::Inode, object.kind()));
        }
        let mut reader = Reader::new(object.payload());
        let kind = NodeKind::try_from(reader.u8()?)?;
        let node_id = UniqueId::from_bytes(reader.array()?);
        let previous = decode_optional_id(&mut reader)?;
        let mode = reader.u32()?;
        if mode > 0o777 {
            return Err(Error::InvalidObject("mode contains unsupported bits"));
        }
        let value = Self {
            kind,
            node_id,
            previous,
            mode,
            owner: reader.u32()?,
            group: reader.u32()?,
            created: Timestamp::decode(&mut reader)?,
            modified: Timestamp::decode(&mut reader)?,
            changed: Timestamp::decode(&mut reader)?,
            accessed: Timestamp::decode(&mut reader)?,
            content: ObjectId::from_bytes(reader.array()?),
        };
        reader.finish()?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Root {
    pub root_inode: ObjectId,
    pub generation: u64,
    pub previous: Option<ObjectId>,
    pub transaction: UniqueId,
    pub committed: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub root: ObjectId,
    pub created: Timestamp,
}

impl Snapshot {
    /// Stores this immutable snapshot descriptor.
    /// # Errors
    /// Returns an error when object publication fails.
    pub fn store(&self, store: &ObjectStore) -> Result<ObjectId> {
        let mut payload = Vec::with_capacity(44);
        payload.extend_from_slice(self.root.as_bytes());
        self.created.encode(&mut payload);
        store.put(&Object::new(ObjectKind::Snapshot, payload)?)
    }
    /// Loads and validates a snapshot descriptor.
    /// # Errors
    /// Returns an error when the object is missing, corrupt, the wrong kind, or malformed.
    pub fn load(id: ObjectId, store: &ObjectStore) -> Result<Self> {
        let object = store.get(id)?;
        if object.kind() != ObjectKind::Snapshot {
            return Err(kind_mismatch(ObjectKind::Snapshot, object.kind()));
        }
        let mut reader = Reader::new(object.payload());
        let value = Self {
            root: ObjectId::from_bytes(reader.array()?),
            created: Timestamp::decode(&mut reader)?,
        };
        reader.finish()?;
        Ok(value)
    }
}

impl Root {
    /// Stores this committed-root object.
    /// # Errors
    /// Returns an error when encoding or storage fails.
    pub fn store(&self, store: &ObjectStore) -> Result<ObjectId> {
        let mut payload = Vec::new();
        payload.extend_from_slice(self.root_inode.as_bytes());
        payload.extend_from_slice(&self.generation.to_be_bytes());
        encode_optional_id(self.previous, &mut payload);
        payload.extend_from_slice(self.transaction.as_bytes());
        self.committed.encode(&mut payload);
        store.put(&Object::new(ObjectKind::Root, payload)?)
    }
    /// Loads and validates a committed-root object.
    /// # Errors
    /// Returns an error when the object is missing, corrupt, the wrong kind, or malformed.
    pub fn load(id: ObjectId, store: &ObjectStore) -> Result<Self> {
        let object = store.get(id)?;
        if object.kind() != ObjectKind::Root {
            return Err(kind_mismatch(ObjectKind::Root, object.kind()));
        }
        let mut reader = Reader::new(object.payload());
        let value = Self {
            root_inode: ObjectId::from_bytes(reader.array()?),
            generation: reader.u64()?,
            previous: decode_optional_id(&mut reader)?,
            transaction: UniqueId::from_bytes(reader.array()?),
            committed: Timestamp::decode(&mut reader)?,
        };
        reader.finish()?;
        Ok(value)
    }
}

fn kind_mismatch(expected: ObjectKind, actual: ObjectKind) -> Error {
    Error::ObjectKindMismatch {
        expected: expected as u8,
        actual: actual as u8,
    }
}
fn encode_optional_id(id: Option<ObjectId>, output: &mut Vec<u8>) {
    match id {
        Some(value) => {
            output.push(1);
            output.extend_from_slice(value.as_bytes());
        }
        None => output.push(0),
    }
}
fn decode_optional_id(reader: &mut Reader<'_>) -> Result<Option<ObjectId>> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(ObjectId::from_bytes(reader.array()?))),
        _ => Err(Error::InvalidObject("invalid optional object ID tag")),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(Error::InvalidObject("length overflow"))?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(Error::InvalidObject("truncated payload"))?;
        self.position = end;
        Ok(value)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| Error::InvalidObject("truncated fixed-width value"))
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.array()?))
    }
    fn finish(&self) -> Result<()> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::InvalidObject("trailing payload bytes"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repository;
    #[test]
    fn typed_objects_round_trip() {
        let temp = crate::repository::test_directory("typed");
        let repository = Repository::init(temp.path()).unwrap();
        let root = Root::load(repository.head(), repository.store()).unwrap();
        let inode = Inode::load(root.root_inode, repository.store()).unwrap();
        let directory = Directory::load(inode.content, repository.store()).unwrap();
        assert_eq!(root.generation, 0);
        assert_eq!(inode.kind, NodeKind::Directory);
        assert!(directory.is_empty());
    }
    #[test]
    fn directory_rejects_invalid_name() {
        let temp = crate::repository::test_directory("bad-name");
        let repository = Repository::init(temp.path()).unwrap();
        let mut directory = Directory::empty();
        directory.insert("..".to_owned(), repository.head());
        assert!(directory.store(repository.store()).is_err());
    }
}
