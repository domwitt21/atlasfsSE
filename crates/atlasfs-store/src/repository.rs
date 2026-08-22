use crate::{
    Directory, Error, Inode, NodeKind, ObjectId, ObjectKind, ObjectStore, Result, Root, Snapshot,
    Timestamp, UniqueId,
    journal::{
        Journal, RecordKind, begin_payload, checkpoint_payload, commit_payload, one_id_payload,
    },
};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
const FORMAT_MARKER: &[u8] = b"atlasfs 1\n";
const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupReport {
    pub destination: PathBuf,
    pub objects: u64,
    pub bytes: u64,
    pub head: ObjectId,
}

#[derive(Debug)]
pub struct Repository {
    path: PathBuf,
    store: ObjectStore,
    head: ObjectId,
}

#[derive(Debug)]
pub struct WriterGuard {
    _file: File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotInfo {
    pub name: String,
    pub object: ObjectId,
    pub root: ObjectId,
    pub created: Timestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitStage {
    BeginDurable,
    ObjectsDurable,
    CommitDurable,
    HeadPublished,
    CompleteDurable,
}

impl Repository {
    /// Atomically creates a new repository directory structure.
    ///
    /// # Errors
    /// Returns an error if the destination exists or required storage operations fail.
    pub fn init(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            return Err(Error::RepositoryAlreadyExists(path.to_path_buf()));
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|source| Error::io("create repository parent", parent, source))?;
        let token = unique_token();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("atlasfs");
        let staging = parent.join(format!(".{name}.init-{token}.tmp"));
        let result = (|| {
            for relative in [
                "objects",
                "refs/snapshots",
                "journal",
                "index",
                "locks",
                "tmp",
            ] {
                fs::create_dir_all(staging.join(relative)).map_err(|source| {
                    Error::io(
                        "create repository directory",
                        staging.join(relative),
                        source,
                    )
                })?;
            }
            write_new(&staging.join("FORMAT"), FORMAT_MARKER)?;
            let config = format!(
                "chunk_size={}\noptional_features=\nrepository_uuid={token}\nrequired_features=\n",
                crate::DEFAULT_CHUNK_SIZE
            );
            write_new(&staging.join("config"), config.as_bytes())?;
            let store = ObjectStore::open(&staging)?;
            let directory_id = Directory::empty().store(&store)?;
            let timestamp = Timestamp::now()?;
            let inode_id = Inode {
                kind: NodeKind::Directory,
                node_id: UniqueId::new(),
                previous: None,
                mode: 0o755,
                owner: 0,
                group: 0,
                created: timestamp,
                modified: timestamp,
                changed: timestamp,
                accessed: timestamp,
                content: directory_id,
            }
            .store(&store)?;
            let root_id = Root {
                root_inode: inode_id,
                generation: 0,
                previous: None,
                transaction: UniqueId::new(),
                committed: timestamp,
            }
            .store(&store)?;
            write_new(
                &staging.join("refs/HEAD"),
                format!("{root_id}\n").as_bytes(),
            )?;
            atlasfs_platform::publish(&staging, path, false)
                .map_err(|source| Error::io("publish repository", path, source))?;
            Ok(())
        })();
        if result.is_err() && staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        result?;
        Self::open(path)
    }

    /// Opens and validates an existing repository.
    ///
    /// # Errors
    /// Returns an error when the repository is absent, unreadable, or unsupported.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let marker = fs::read(path.join("FORMAT"))
            .map_err(|source| Error::io("read format marker", path.join("FORMAT"), source))?;
        if marker != FORMAT_MARKER {
            return Err(Error::UnsupportedFormat(0));
        }
        validate_config(&path)?;
        let store = ObjectStore::open(&path)?;
        let head = read_head(&path)?;
        let root = store.get(head)?;
        if root.kind() != ObjectKind::Root {
            return Err(Error::ReferenceCorrupt(
                "HEAD does not name a root object".to_owned(),
            ));
        }
        let mut repository = Self { path, store, head };
        repository.recover()?;
        Ok(repository)
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub const fn store(&self) -> &ObjectStore {
        &self.store
    }
    #[must_use]
    pub const fn head(&self) -> ObjectId {
        self.head
    }

    /// Creates an atomic, verified, directly reopenable repository backup.
    /// # Errors
    /// Returns an error if locking, verification, copying, or publication fails.
    pub fn backup(&mut self, destination: impl AsRef<Path>) -> Result<BackupReport> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(Error::RepositoryAlreadyExists(destination.to_path_buf()));
        }
        let _guard = self.begin_write()?;
        let verified = self.store.verify_all()?;
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|source| Error::io("create backup parent", parent, source))?;
        let name = destination
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("atlasfs-backup");
        let staging = parent.join(format!(".{name}.backup-{}.tmp", unique_token()));
        let result = (|| {
            for relative in [
                "objects",
                "refs/snapshots",
                "journal",
                "index",
                "locks",
                "tmp",
            ] {
                fs::create_dir_all(staging.join(relative)).map_err(|source| {
                    Error::io("create backup directory", staging.join(relative), source)
                })?;
            }
            copy_file(&self.path.join("FORMAT"), &staging.join("FORMAT"))?;
            copy_file(&self.path.join("config"), &staging.join("config"))?;
            copy_tree(&self.path.join("objects"), &staging.join("objects"))?;
            copy_tree(&self.path.join("refs"), &staging.join("refs"))?;
            let journal = self.path.join("journal/0000000000000001.log");
            if journal.exists() {
                copy_file(&journal, &staging.join("journal/0000000000000001.log"))?;
            }
            let manifest = format!(
                "atlasfs-backup 1\nhead={}\nobjects={}\nbytes={}\n",
                self.head, verified.objects_checked, verified.bytes_checked
            );
            write_new(&staging.join("BACKUP"), manifest.as_bytes())?;
            atlasfs_platform::publish(&staging, destination, false)
                .map_err(|source| Error::io("publish repository backup", destination, source))
        })();
        if result.is_err() && staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        result?;
        Repository::open(destination)?;
        Ok(BackupReport {
            destination: destination.to_path_buf(),
            objects: verified.objects_checked,
            bytes: verified.bytes_checked,
            head: self.head,
        })
    }

    /// Acquires the cross-process writer lock and refreshes `HEAD`.
    /// # Errors
    /// Returns [`Error::RepositoryBusy`] if another writer owns the lock.
    pub fn begin_write(&mut self) -> Result<WriterGuard> {
        let lock_path = self.path.join("locks/writer.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|source| Error::io("open writer lock", &lock_path, source))?;
        file.try_lock_exclusive().map_err(|source| {
            if lock_is_contended(&source) {
                Error::RepositoryBusy(self.path.clone())
            } else {
                Error::io("acquire writer lock", &lock_path, source)
            }
        })?;
        self.head = read_head(&self.path)?;
        Ok(WriterGuard { _file: file })
    }

    /// Commits a durable root through the checksummed journal.
    /// # Errors
    /// Returns an error for stale state, malformed roots, journal failure, or publication failure.
    pub fn commit_root(
        &mut self,
        guard: &WriterGuard,
        expected: ObjectId,
        new_head: ObjectId,
    ) -> Result<()> {
        self.commit_root_with_hook(guard, expected, new_head, |_| Ok(()))
    }

    fn commit_root_with_hook(
        &mut self,
        _guard: &WriterGuard,
        expected: ObjectId,
        new_head: ObjectId,
        mut hook: impl FnMut(CommitStage) -> Result<()>,
    ) -> Result<()> {
        if self.head != expected {
            return Err(Error::TransactionConflict);
        }
        let root = Root::load(new_head, &self.store)?;
        if root.previous != Some(expected) {
            return Err(Error::TransactionConflict);
        }
        let journal = Journal::open(&self.path);
        journal.append(
            root.transaction,
            RecordKind::Begin,
            &begin_payload(expected, root.generation),
        )?;
        hook(CommitStage::BeginDurable)?;
        journal.append(
            root.transaction,
            RecordKind::ObjectsDurable,
            &one_id_payload(new_head),
        )?;
        hook(CommitStage::ObjectsDurable)?;
        journal.append(
            root.transaction,
            RecordKind::Commit,
            &commit_payload(expected, new_head),
        )?;
        hook(CommitStage::CommitDurable)?;
        self.publish_head_raw(expected, new_head)?;
        hook(CommitStage::HeadPublished)?;
        journal.append(
            root.transaction,
            RecordKind::Complete,
            &one_id_payload(new_head),
        )?;
        hook(CommitStage::CompleteDurable)?;
        Ok(())
    }

    /// Appends a checkpoint describing the fully resolved journal prefix.
    /// # Errors
    /// Returns an error when locking, scanning, or journal publication fails.
    pub fn checkpoint(&mut self) -> Result<u64> {
        let guard = self.begin_write()?;
        let journal = Journal::open(&self.path);
        let scan = journal.scan()?;
        let resolved = scan.records.last().map_or(0, |record| record.sequence);
        let sequence = journal.append(
            UniqueId::new(),
            RecordKind::Checkpoint,
            &checkpoint_payload(resolved, self.head),
        )?;
        drop(guard);
        Ok(sequence)
    }

    /// Creates a named immutable snapshot of the current root.
    /// # Errors
    /// Returns an error for an invalid/existing name, locking, or durable publication failure.
    pub fn create_snapshot(&mut self, name: &str) -> Result<SnapshotInfo> {
        validate_snapshot_name(name)?;
        let guard = self.begin_write()?;
        let snapshot = Snapshot {
            root: self.head,
            created: Timestamp::now()?,
        };
        let object = snapshot.store(&self.store)?;
        let encoded = encode_snapshot_name(name);
        let destination = self.path.join("refs/snapshots").join(encoded);
        if destination.exists() {
            return Err(Error::ReferenceCorrupt(format!(
                "snapshot already exists: {name}"
            )));
        }
        let temp = self
            .path
            .join("tmp")
            .join(format!("snapshot-{}.tmp", unique_token()));
        write_new(&temp, format!("{object}\n").as_bytes())?;
        atlasfs_platform::publish(&temp, &destination, false)
            .map_err(|source| Error::io("publish snapshot reference", &destination, source))?;
        drop(guard);
        Ok(SnapshotInfo {
            name: name.to_owned(),
            object,
            root: snapshot.root,
            created: snapshot.created,
        })
    }

    /// Returns one named snapshot.
    /// # Errors
    /// Returns an error for invalid names, missing/corrupt references, or invalid objects.
    pub fn snapshot(&self, name: &str) -> Result<SnapshotInfo> {
        validate_snapshot_name(name)?;
        let path = self
            .path
            .join("refs/snapshots")
            .join(encode_snapshot_name(name));
        if !path.is_file() {
            return Err(Error::ReferenceCorrupt(format!(
                "snapshot not found: {name}"
            )));
        }
        read_snapshot_info(name.to_owned(), &path, &self.store)
    }

    /// Lists named snapshots in canonical name order.
    /// # Errors
    /// Returns an error for unreadable or corrupt snapshot references.
    pub fn snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let directory = self.path.join("refs/snapshots");
        let mut values = Vec::new();
        for entry in fs::read_dir(&directory)
            .map_err(|source| Error::io("list snapshots", &directory, source))?
        {
            let entry =
                entry.map_err(|source| Error::io("read snapshot entry", &directory, source))?;
            if !entry.path().is_file() {
                return Err(Error::ReferenceCorrupt(
                    "non-file snapshot reference".to_owned(),
                ));
            }
            let encoded = entry.file_name().to_string_lossy().into_owned();
            let name = decode_snapshot_name(&encoded)?;
            values.push(read_snapshot_info(name, &entry.path(), &self.store)?);
        }
        values.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
        Ok(values)
    }

    /// Deletes a named snapshot reference without deleting immutable objects.
    /// # Errors
    /// Returns an error for invalid/missing names, locking, or publication failure.
    pub fn delete_snapshot(&mut self, name: &str) -> Result<()> {
        validate_snapshot_name(name)?;
        let guard = self.begin_write()?;
        let source = self
            .path
            .join("refs/snapshots")
            .join(encode_snapshot_name(name));
        if !source.is_file() {
            return Err(Error::ReferenceCorrupt(format!(
                "snapshot not found: {name}"
            )));
        }
        let retired = self
            .path
            .join("tmp")
            .join(format!("deleted-snapshot-{}.tmp", unique_token()));
        atlasfs_platform::publish(&source, &retired, false)
            .map_err(|error| Error::io("retire snapshot reference", &source, error))?;
        fs::remove_file(&retired)
            .map_err(|error| Error::io("remove retired snapshot reference", &retired, error))?;
        drop(guard);
        Ok(())
    }

    /// Repairs a torn tail and completes committed root publications.
    /// # Errors
    /// Returns an error for conflicts, corruption, locking, or publication failure.
    pub fn recover(&mut self) -> Result<()> {
        let journal = Journal::open(&self.path);
        let scan = journal.scan()?;
        let states = journal.states()?;
        if !scan.torn_tail
            && !states
                .values()
                .any(|state| state.committed && !state.completed && !state.aborted)
        {
            return Ok(());
        }
        let guard = self.begin_write()?;
        if scan.torn_tail {
            journal.truncate(scan.valid_length)?;
        }
        for (transaction, state) in states {
            if !state.committed || state.completed || state.aborted {
                continue;
            }
            let parent = state.parent.ok_or_else(|| {
                Error::JournalCorrupt("committed transaction lacks parent".to_owned())
            })?;
            let proposed = state.proposed.ok_or_else(|| {
                Error::JournalCorrupt("committed transaction lacks proposed root".to_owned())
            })?;
            Root::load(proposed, &self.store)?;
            if self.head == parent {
                self.publish_head_raw(parent, proposed)?;
            } else if self.head != proposed {
                return Err(Error::TransactionConflict);
            }
            journal.append(transaction, RecordKind::Complete, &one_id_payload(proposed))?;
        }
        drop(guard);
        Ok(())
    }

    fn publish_head_raw(&mut self, expected: ObjectId, new_head: ObjectId) -> Result<()> {
        if self.head != expected {
            return Err(Error::TransactionConflict);
        }
        if self.store.get(new_head)?.kind() != ObjectKind::Root {
            return Err(Error::ReferenceCorrupt(
                "new HEAD does not name a root object".to_owned(),
            ));
        }
        let temp = self
            .path
            .join("tmp")
            .join(format!("head-{}.tmp", unique_token()));
        write_new(&temp, format!("{new_head}\n").as_bytes())?;
        atlasfs_platform::publish(&temp, &self.path.join("refs/HEAD"), true).map_err(|source| {
            Error::io(
                "publish HEAD reference",
                self.path.join("refs/HEAD"),
                source,
            )
        })?;
        self.head = new_head;
        Ok(())
    }
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn read_head(path: &Path) -> Result<ObjectId> {
    let head_text = fs::read_to_string(path.join("refs/HEAD"))
        .map_err(|source| Error::io("read HEAD reference", path.join("refs/HEAD"), source))?;
    head_text
        .strip_suffix('\n')
        .ok_or_else(|| Error::ReferenceCorrupt("HEAD".to_owned()))?
        .parse::<ObjectId>()
        .map_err(|_| Error::ReferenceCorrupt("HEAD".to_owned()))
}

fn validate_snapshot_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || name.bytes().any(|byte| matches!(byte, 0 | b'/' | b'\\'))
    {
        return Err(Error::ReferenceCorrupt(format!(
            "invalid snapshot name: {name}"
        )));
    }
    Ok(())
}
fn encode_snapshot_name(name: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    output
}
fn decode_snapshot_name(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(Error::ReferenceCorrupt(
                    "invalid snapshot filename encoding".to_owned(),
                ));
            }
            let high = hex(bytes[index + 1])?;
            let low = hex(bytes[index + 2])?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    let name = String::from_utf8(output)
        .map_err(|_| Error::ReferenceCorrupt("snapshot name is not UTF-8".to_owned()))?;
    validate_snapshot_name(&name)?;
    if encode_snapshot_name(&name) != value {
        return Err(Error::ReferenceCorrupt(
            "noncanonical snapshot filename".to_owned(),
        ));
    }
    Ok(name)
}
fn hex(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::ReferenceCorrupt(
            "invalid snapshot percent encoding".to_owned(),
        )),
    }
}
fn read_snapshot_info(name: String, path: &Path, store: &ObjectStore) -> Result<SnapshotInfo> {
    let text = fs::read_to_string(path)
        .map_err(|source| Error::io("read snapshot reference", path, source))?;
    let object = text
        .strip_suffix('\n')
        .ok_or_else(|| Error::ReferenceCorrupt(name.clone()))?
        .parse::<ObjectId>()
        .map_err(|_| Error::ReferenceCorrupt(name.clone()))?;
    let snapshot = Snapshot::load(object, store)?;
    Root::load(snapshot.root, store)?;
    Ok(SnapshotInfo {
        name,
        object,
        root: snapshot.root,
        created: snapshot.created,
    })
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| Error::io("create repository file", path, source))?;
    file.write_all(bytes)
        .map_err(|source| Error::io("write repository file", path, source))?;
    file.sync_all()
        .map_err(|source| Error::io("flush repository file", path, source))
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    let mut input =
        File::open(source).map_err(|error| Error::io("open backup source", source, error))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| Error::io("create backup file", destination, error))?;
    io::copy(&mut input, &mut output)
        .map_err(|error| Error::io("copy backup file", destination, error))?;
    output
        .sync_all()
        .map_err(|error| Error::io("flush backup file", destination, error))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in
        fs::read_dir(source).map_err(|error| Error::io("list backup source", source, error))?
    {
        let entry = entry.map_err(|error| Error::io("read backup entry", source, error))?;
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            fs::create_dir_all(&target)
                .map_err(|error| Error::io("create backup subtree", &target, error))?;
            copy_tree(&entry.path(), &target)?;
        } else if entry.path().is_file() {
            copy_file(&entry.path(), &target)?;
        } else {
            return Err(Error::InvalidObject("unsupported repository entry type"));
        }
    }
    Ok(())
}

fn validate_config(repository: &Path) -> Result<()> {
    let path = repository.join("config");
    let text = fs::read_to_string(&path)
        .map_err(|source| Error::io("read repository config", &path, source))?;
    if !text.ends_with('\n') || text.contains('\r') {
        return Err(Error::ReferenceCorrupt(
            "noncanonical repository config".to_owned(),
        ));
    }
    let mut values = std::collections::BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| Error::ReferenceCorrupt("malformed repository config".to_owned()))?;
        if values.insert(key, value).is_some() {
            return Err(Error::ReferenceCorrupt(
                "duplicate repository config key".to_owned(),
            ));
        }
    }
    if values.keys().copied().collect::<Vec<_>>()
        != [
            "chunk_size",
            "optional_features",
            "repository_uuid",
            "required_features",
        ]
    {
        return Err(Error::ReferenceCorrupt(
            "repository config keys are not canonical".to_owned(),
        ));
    }
    if values["chunk_size"] != crate::DEFAULT_CHUNK_SIZE.to_string() {
        return Err(Error::UnsupportedFeature("chunk_size".to_owned()));
    }
    if !values["required_features"].is_empty() {
        return Err(Error::UnsupportedFeature(
            values["required_features"].to_owned(),
        ));
    }
    let uuid = values["repository_uuid"];
    if uuid.len() != 32
        || !uuid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::ReferenceCorrupt(
            "invalid repository UUID".to_owned(),
        ));
    }
    let mut canonical = String::new();
    for (key, value) in &values {
        writeln!(canonical, "{key}={value}").expect("writing to String cannot fail");
    }
    if canonical != text {
        return Err(Error::ReferenceCorrupt(
            "repository config is not canonical".to_owned(),
        ));
    }
    Ok(())
}

fn unique_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    hasher.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    let digest = hasher.finalize();
    let mut token = String::with_capacity(32);
    for byte in &digest[..16] {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    token
}

#[cfg(test)]
pub(crate) struct TestDirectory(PathBuf);
#[cfg(test)]
impl TestDirectory {
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}
#[cfg(test)]
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[cfg(test)]
pub(crate) fn test_directory(label: &str) -> TestDirectory {
    TestDirectory(std::env::temp_dir().join(format!("atlasfs-{label}-{}", unique_token())))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initializes_and_reopens_repository() {
        let directory = test_directory("repository");
        let repository = Repository::init(directory.path()).unwrap();
        assert_eq!(repository.path(), directory.path());
        assert_eq!(
            repository.store().get(repository.head()).unwrap().kind(),
            ObjectKind::Root
        );
        for relative in [
            "FORMAT",
            "config",
            "objects",
            "refs/snapshots",
            "journal",
            "index",
            "locks",
            "tmp",
        ] {
            assert!(
                directory.path().join(relative).exists(),
                "missing {relative}"
            );
        }
        drop(repository);
        let reopened = Repository::open(directory.path()).unwrap();
        assert_eq!(reopened.store().verify_all().unwrap().objects_checked, 3);
    }
    #[test]
    fn refuses_to_replace_existing_path() {
        let directory = test_directory("existing");
        fs::create_dir_all(directory.path()).unwrap();
        assert!(matches!(
            Repository::init(directory.path()),
            Err(Error::RepositoryAlreadyExists(_))
        ));
    }

    #[test]
    fn backup_is_atomic_verified_and_reopenable() {
        let directory = test_directory("backup-source");
        let destination = directory.path().with_extension("backup");
        let mut repository = Repository::init(directory.path()).unwrap();
        let expected = repository.head();
        let report = repository.backup(&destination).unwrap();
        assert_eq!(report.head, expected);
        assert_eq!(Repository::open(&destination).unwrap().head(), expected);
        assert!(destination.join("BACKUP").is_file());
        fs::remove_dir_all(destination).unwrap();
    }

    #[test]
    fn repository_config_rejects_unknown_required_features() {
        let directory = test_directory("unsupported-feature");
        let repository = Repository::init(directory.path()).unwrap();
        drop(repository);
        let config_path = directory.path().join("config");
        let config = fs::read_to_string(&config_path).unwrap();
        fs::write(
            &config_path,
            config.replace("required_features=\n", "required_features=future-mode\n"),
        )
        .unwrap();
        assert!(matches!(
            Repository::open(directory.path()),
            Err(Error::UnsupportedFeature(_))
        ));
    }

    #[test]
    fn writer_lock_rejects_competing_writer() {
        let directory = test_directory("writer-lock");
        let mut first = Repository::init(directory.path()).unwrap();
        let guard = first.begin_write().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "repository::tests::writer_lock_child_probe"])
            .env("ATLASFS_LOCK_PROBE", directory.path())
            .status()
            .unwrap();
        assert!(status.success());
        drop(guard);
        first.begin_write().unwrap();
    }

    #[test]
    fn writer_lock_child_probe() {
        let Some(path) = std::env::var_os("ATLASFS_LOCK_PROBE") else {
            return;
        };
        let mut repository = Repository::open(path).unwrap();
        assert!(matches!(
            repository.begin_write(),
            Err(Error::RepositoryBusy(_))
        ));
    }

    #[test]
    fn recovery_rolls_forward_committed_root() {
        let directory = test_directory("recover-before-head");
        let mut repository = Repository::init(directory.path()).unwrap();
        let guard = repository.begin_write().unwrap();
        let parent = repository.head();
        let old_root = Root::load(parent, repository.store()).unwrap();
        let transaction = UniqueId::new();
        let proposed = Root {
            root_inode: old_root.root_inode,
            generation: 1,
            previous: Some(parent),
            transaction,
            committed: Timestamp::now().unwrap(),
        }
        .store(repository.store())
        .unwrap();
        let journal = Journal::open(repository.path());
        journal
            .append(transaction, RecordKind::Begin, &begin_payload(parent, 1))
            .unwrap();
        journal
            .append(
                transaction,
                RecordKind::ObjectsDurable,
                &one_id_payload(proposed),
            )
            .unwrap();
        journal
            .append(
                transaction,
                RecordKind::Commit,
                &commit_payload(parent, proposed),
            )
            .unwrap();
        drop(guard);
        drop(repository);

        let recovered = Repository::open(directory.path()).unwrap();
        assert_eq!(recovered.head(), proposed);
        let state = Journal::open(recovered.path())
            .states()
            .unwrap()
            .remove(&transaction)
            .unwrap();
        assert!(state.completed);
    }

    #[test]
    fn recovery_completes_published_root_and_checkpoint_appends() {
        let directory = test_directory("recover-after-head");
        let mut repository = Repository::init(directory.path()).unwrap();
        let guard = repository.begin_write().unwrap();
        let parent = repository.head();
        let old_root = Root::load(parent, repository.store()).unwrap();
        let transaction = UniqueId::new();
        let proposed = Root {
            root_inode: old_root.root_inode,
            generation: 1,
            previous: Some(parent),
            transaction,
            committed: Timestamp::now().unwrap(),
        }
        .store(repository.store())
        .unwrap();
        let journal = Journal::open(repository.path());
        journal
            .append(transaction, RecordKind::Begin, &begin_payload(parent, 1))
            .unwrap();
        journal
            .append(
                transaction,
                RecordKind::ObjectsDurable,
                &one_id_payload(proposed),
            )
            .unwrap();
        journal
            .append(
                transaction,
                RecordKind::Commit,
                &commit_payload(parent, proposed),
            )
            .unwrap();
        repository.publish_head_raw(parent, proposed).unwrap();
        drop(guard);
        drop(repository);

        let mut recovered = Repository::open(directory.path()).unwrap();
        assert_eq!(recovered.head(), proposed);
        let before = Journal::open(recovered.path())
            .scan()
            .unwrap()
            .records
            .len();
        recovered.checkpoint().unwrap();
        let after = Journal::open(recovered.path())
            .scan()
            .unwrap()
            .records
            .len();
        assert_eq!(after, before + 1);
    }

    #[test]
    fn commit_stage_faults_recover_to_old_or_new_root() {
        for (index, stage) in [
            CommitStage::BeginDurable,
            CommitStage::ObjectsDurable,
            CommitStage::CommitDurable,
            CommitStage::HeadPublished,
            CommitStage::CompleteDurable,
        ]
        .into_iter()
        .enumerate()
        {
            let directory = test_directory(&format!("commit-fault-{index}"));
            let mut repository = Repository::init(directory.path()).unwrap();
            let guard = repository.begin_write().unwrap();
            let parent = repository.head();
            let old_root = Root::load(parent, repository.store()).unwrap();
            let proposed_root = Root {
                root_inode: old_root.root_inode,
                generation: 1,
                previous: Some(parent),
                transaction: UniqueId::new(),
                committed: Timestamp::now().unwrap(),
            };
            let proposed = proposed_root.store(repository.store()).unwrap();
            let result = repository.commit_root_with_hook(&guard, parent, proposed, |observed| {
                if observed == stage {
                    Err(Error::InvalidObject("injected commit interruption"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err());
            drop(guard);
            drop(repository);
            let recovered = Repository::open(directory.path()).unwrap();
            let expected = match stage {
                CommitStage::BeginDurable | CommitStage::ObjectsDurable => parent,
                CommitStage::CommitDurable
                | CommitStage::HeadPublished
                | CommitStage::CompleteDurable => proposed,
            };
            assert_eq!(
                recovered.head(),
                expected,
                "unexpected root after {stage:?}"
            );
        }
    }
}
