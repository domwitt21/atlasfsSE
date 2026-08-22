use std::collections::BTreeSet;
use std::{io::Cursor, path::Path};

use crate::{
    Actor, Error, Result, VirtualPath,
    permission::{EXECUTE, READ, WRITE},
};
use atlasfs_store::{
    ChunkedFile, Directory, Inode, NodeKind, ObjectId, Repository, Root, Timestamp, UniqueId,
    WriterGuard, chunk_reader, read_chunked_file,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeStat {
    pub object_id: ObjectId,
    pub node_id: UniqueId,
    pub kind: NodeKind,
    pub mode: u32,
    pub owner: u32,
    pub group: u32,
    pub created: Timestamp,
    pub modified: Timestamp,
    pub changed: Timestamp,
    pub size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub object_id: ObjectId,
    pub modified: Timestamp,
    pub changed: Timestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffKind {
    Added,
    Removed,
    ContentModified,
    MetadataModified,
    TypeChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffEntry {
    pub path: String,
    pub kind: DiffKind,
}

#[derive(Debug)]
pub struct VirtualFileSystem {
    repository: Repository,
}

impl VirtualFileSystem {
    /// Initializes a new virtual filesystem repository.
    /// # Errors
    /// Returns an error when repository initialization fails.
    pub fn init(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            repository: Repository::init(path)?,
        })
    }
    /// Opens an existing virtual filesystem repository.
    /// # Errors
    /// Returns an error when repository validation fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            repository: Repository::open(path)?,
        })
    }
    #[must_use]
    pub fn head(&self) -> ObjectId {
        self.repository.head()
    }
    /// Returns the current committed generation.
    /// # Errors
    /// Returns an error when the root object cannot be validated.
    pub fn generation(&self) -> Result<u64> {
        Ok(self.root()?.generation)
    }

    /// Reports repository storage use and deduplication effectiveness.
    /// # Errors
    /// Returns an error when canonical objects or references cannot be validated.
    pub fn storage_usage(&self) -> Result<atlasfs_store::StorageUsage> {
        Ok(self.repository.storage_usage()?)
    }

    /// Rebuilds disposable path, history, and reachability indexes.
    /// # Errors
    /// Returns an error when locking, traversal, or index publication fails.
    pub fn rebuild_indexes(&mut self) -> Result<atlasfs_store::IndexReport> {
        Ok(self.repository.rebuild_reachability_index()?)
    }

    /// Reports or removes objects unreachable from `HEAD` and snapshots.
    /// # Errors
    /// Returns an error when locking, traversal, or object retirement fails.
    pub fn garbage_collect(&mut self, dry_run: bool) -> Result<atlasfs_store::GcReport> {
        Ok(self.repository.garbage_collect(dry_run)?)
    }

    /// Verifies every object envelope and SHA-256 identity in the repository.
    /// # Errors
    /// Returns an error when enumeration fails or any object is corrupt.
    pub fn verify(&self) -> Result<atlasfs_store::VerificationReport> {
        Ok(self.repository.store().verify_all()?)
    }

    /// Replays and resolves the crash-recovery journal.
    /// # Errors
    /// Returns an error when the journal or its referenced objects are invalid.
    pub fn recover(&mut self) -> Result<()> {
        Ok(self.repository.recover()?)
    }

    /// Creates a verified, atomic repository backup.
    /// # Errors
    /// Returns an error when locking, verification, copying, or publication fails.
    pub fn backup(&mut self, destination: impl AsRef<Path>) -> Result<atlasfs_store::BackupReport> {
        Ok(self.repository.backup(destination)?)
    }

    /// Creates a named point-in-time snapshot.
    /// # Errors
    /// Returns an error for invalid/existing names, locking, or publication failure.
    pub fn create_snapshot(&mut self, name: &str) -> Result<atlasfs_store::SnapshotInfo> {
        Ok(self.repository.create_snapshot(name)?)
    }
    /// Lists all named snapshots.
    /// # Errors
    /// Returns an error when snapshot references are unreadable or corrupt.
    pub fn snapshots(&self) -> Result<Vec<atlasfs_store::SnapshotInfo>> {
        Ok(self.repository.snapshots()?)
    }
    /// Deletes a named snapshot reference.
    /// # Errors
    /// Returns an error when the name is invalid/missing or durable removal fails.
    pub fn delete_snapshot(&mut self, name: &str) -> Result<()> {
        Ok(self.repository.delete_snapshot(name)?)
    }
    /// Restores a snapshot tree as a new journaled root generation.
    /// # Errors
    /// Returns an error when the snapshot is invalid, locking fails, or commit fails.
    pub fn restore_snapshot(&mut self, name: &str) -> Result<()> {
        let guard = self.repository.begin_write()?;
        let snapshot = self.repository.snapshot(name)?;
        let selected = Root::load(snapshot.root, self.repository.store())?;
        let current = self.root()?;
        let timestamp = Timestamp::now()?;
        self.commit(&current, selected.root_inode, timestamp, &guard)
    }

    /// Reads a file from a named read-only snapshot.
    /// # Errors
    /// Returns an error for invalid snapshots, paths, permissions, or objects.
    pub fn read_snapshot_file(
        &self,
        snapshot: &str,
        path: &VirtualPath,
        actor: &Actor,
    ) -> Result<Vec<u8>> {
        let info = self.repository.snapshot(snapshot)?;
        let root = Root::load(info.root, self.repository.store())?;
        let (_, inode) = self.resolve_in(root.root_inode, path.components(), actor)?;
        if inode.kind != NodeKind::File {
            return Err(Error::IsDirectory(path.to_string()));
        }
        Self::require(actor, &inode, READ, path)?;
        let file = ChunkedFile::load(inode.content, self.repository.store())?;
        let mut output = Vec::new();
        read_chunked_file(&file, self.repository.store(), &mut output)?;
        Ok(output)
    }

    /// Compares two named snapshot trees.
    /// # Errors
    /// Returns an error when either snapshot or any reachable object is invalid.
    pub fn diff_snapshots(&self, left: &str, right: &str) -> Result<Vec<DiffEntry>> {
        let left = Root::load(
            self.repository.snapshot(left)?.root,
            self.repository.store(),
        )?;
        let right = Root::load(
            self.repository.snapshot(right)?.root,
            self.repository.store(),
        )?;
        let mut output = Vec::new();
        self.diff_nodes(
            "/",
            Some(left.root_inode),
            Some(right.root_inode),
            &mut output,
        )?;
        Ok(output)
    }

    /// Returns metadata for a path.
    /// # Errors
    /// Returns an error if traversal or storage validation fails.
    pub fn stat(&self, path: &VirtualPath, actor: &Actor) -> Result<NodeStat> {
        let (id, inode) = self.resolve(path, actor)?;
        self.stat_inode(id, &inode)
    }

    /// Reads a complete virtual file.
    /// # Errors
    /// Returns an error if traversal, permissions, storage, or validation fails.
    pub fn read_file(&self, path: &VirtualPath, actor: &Actor) -> Result<Vec<u8>> {
        let (_, inode) = self.resolve(path, actor)?;
        if inode.kind != NodeKind::File {
            return Err(Error::IsDirectory(path.to_string()));
        }
        Self::require(actor, &inode, READ, path)?;
        let file = ChunkedFile::load(inode.content, self.repository.store())?;
        let capacity = usize::try_from(file.logical_length)
            .map_err(|_| Error::InvalidOperation("file is too large for memory read"))?;
        let mut output = Vec::with_capacity(capacity);
        read_chunked_file(&file, self.repository.store(), &mut output)?;
        Ok(output)
    }

    /// Lists a directory in canonical byte order.
    /// # Errors
    /// Returns an error if traversal, permissions, or validation fails.
    pub fn list(&self, path: &VirtualPath, actor: &Actor) -> Result<Vec<String>> {
        let (_, inode) = self.resolve(path, actor)?;
        if inode.kind != NodeKind::Directory {
            return Err(Error::NotDirectory(path.to_string()));
        }
        Self::require(actor, &inode, READ | EXECUTE, path)?;
        Ok(Directory::load(inode.content, self.repository.store())?
            .entries()
            .keys()
            .cloned()
            .collect())
    }

    /// Creates a file containing `data`.
    /// # Errors
    /// Returns an error if the destination exists or validation, permissions, or publication fails.
    pub fn create_file(
        &mut self,
        path: &VirtualPath,
        data: &[u8],
        mode: u32,
        actor: &Actor,
    ) -> Result<ObjectId> {
        let guard = self.repository.begin_write()?;
        let file = chunk_reader(&mut Cursor::new(data), self.repository.store())?;
        let content = file.store(self.repository.store())?;
        self.create_node(path, NodeKind::File, content, mode, actor, &guard)
    }

    /// Creates an empty directory.
    /// # Errors
    /// Returns an error if the destination exists or validation, permissions, or publication fails.
    pub fn create_directory(
        &mut self,
        path: &VirtualPath,
        mode: u32,
        actor: &Actor,
    ) -> Result<ObjectId> {
        let guard = self.repository.begin_write()?;
        let content = Directory::empty().store(self.repository.store())?;
        self.create_node(path, NodeKind::Directory, content, mode, actor, &guard)
    }

    /// Replaces the complete contents of a file.
    /// # Errors
    /// Returns an error if the path is not a writable file or publication fails.
    pub fn write_file(
        &mut self,
        path: &VirtualPath,
        data: &[u8],
        actor: &Actor,
    ) -> Result<ObjectId> {
        let guard = self.repository.begin_write()?;
        let root = self.root()?;
        let (old_id, mut inode) = self.resolve_in(root.root_inode, path.components(), actor)?;
        if inode.kind != NodeKind::File {
            return Err(Error::IsDirectory(path.to_string()));
        }
        Self::require(actor, &inode, WRITE, path)?;
        let timestamp = Timestamp::now()?;
        let file = chunk_reader(&mut Cursor::new(data), self.repository.store())?;
        inode.content = file.store(self.repository.store())?;
        inode.previous = Some(old_id);
        inode.modified = timestamp;
        inode.changed = timestamp;
        let new_id = inode.store(self.repository.store())?;
        let new_root_inode =
            self.replace_inode(root.root_inode, path.components(), new_id, timestamp)?;
        self.commit(&root, new_root_inode, timestamp, &guard)?;
        Ok(new_id)
    }

    /// Removes a file or empty directory.
    /// # Errors
    /// Returns an error for root, missing/nonempty nodes, permissions, or publication failures.
    pub fn remove(&mut self, path: &VirtualPath, actor: &Actor) -> Result<()> {
        self.remove_inner(path, actor, false)
    }

    /// Removes a file or an entire directory subtree.
    /// # Errors
    /// Returns an error for root, missing nodes, permissions, or publication failures.
    pub fn remove_recursive(&mut self, path: &VirtualPath, actor: &Actor) -> Result<()> {
        self.remove_inner(path, actor, true)
    }

    /// Changes a node's permission bits, creating a new inode version.
    /// # Errors
    /// Returns an error unless the actor owns the node or is root, or publication fails.
    pub fn chmod(&mut self, path: &VirtualPath, mode: u32, actor: &Actor) -> Result<ObjectId> {
        let guard = self.repository.begin_write()?;
        if mode > 0o777 {
            return Err(Error::InvalidOperation("mode contains unsupported bits"));
        }
        let root = self.root()?;
        let (old_id, mut inode) = self.resolve_in(root.root_inode, path.components(), actor)?;
        if actor.user != 0 && actor.user != inode.owner {
            return Err(Error::PermissionDenied(path.to_string()));
        }
        let timestamp = Timestamp::now()?;
        inode.mode = mode;
        inode.previous = Some(old_id);
        inode.changed = timestamp;
        let new_id = inode.store(self.repository.store())?;
        let new_root_inode =
            self.replace_inode(root.root_inode, path.components(), new_id, timestamp)?;
        self.commit(&root, new_root_inode, timestamp, &guard)?;
        Ok(new_id)
    }

    fn remove_inner(&mut self, path: &VirtualPath, actor: &Actor, recursive: bool) -> Result<()> {
        let guard = self.repository.begin_write()?;
        let (parent_path, name) = path.split_parent()?;
        let root = self.root()?;
        let (_, parent) = self.resolve_in(root.root_inode, parent_path, actor)?;
        Self::require_directory(&parent, path)?;
        Self::require(actor, &parent, WRITE | EXECUTE, path)?;
        let directory = Directory::load(parent.content, self.repository.store())?;
        let child_id = directory
            .get(name)
            .ok_or_else(|| Error::NotFound(path.to_string()))?;
        let child = Inode::load(child_id, self.repository.store())?;
        if !recursive
            && child.kind == NodeKind::Directory
            && !Directory::load(child.content, self.repository.store())?.is_empty()
        {
            return Err(Error::DirectoryNotEmpty(path.to_string()));
        }
        let timestamp = Timestamp::now()?;
        let new_root_inode =
            self.rewrite_entry(root.root_inode, parent_path, name, None, timestamp)?;
        self.commit(&root, new_root_inode, timestamp, &guard)
    }

    /// Atomically renames a node without replacing an existing destination.
    /// # Errors
    /// Returns an error for invalid relationships, conflicts, permissions, or publication failures.
    pub fn rename(
        &mut self,
        source: &VirtualPath,
        destination: &VirtualPath,
        actor: &Actor,
    ) -> Result<()> {
        let guard = self.repository.begin_write()?;
        let (source_parent, source_name) = source.split_parent()?;
        let (destination_parent, destination_name) = destination.split_parent()?;
        if destination.starts_with(source) {
            return Err(Error::InvalidOperation("cannot move a node into itself"));
        }
        let root = self.root()?;
        let (_, source_parent_inode) = self.resolve_in(root.root_inode, source_parent, actor)?;
        Self::require(actor, &source_parent_inode, WRITE | EXECUTE, source)?;
        let source_id = Directory::load(source_parent_inode.content, self.repository.store())?
            .get(source_name)
            .ok_or_else(|| Error::NotFound(source.to_string()))?;
        let (_, destination_parent_inode) =
            self.resolve_in(root.root_inode, destination_parent, actor)?;
        Self::require(
            actor,
            &destination_parent_inode,
            WRITE | EXECUTE,
            destination,
        )?;
        if Directory::load(destination_parent_inode.content, self.repository.store())?
            .get(destination_name)
            .is_some()
        {
            return Err(Error::AlreadyExists(destination.to_string()));
        }
        let timestamp = Timestamp::now()?;
        let without_source =
            self.rewrite_entry(root.root_inode, source_parent, source_name, None, timestamp)?;
        let with_destination = self.rewrite_entry(
            without_source,
            destination_parent,
            destination_name,
            Some(source_id),
            timestamp,
        )?;
        self.commit(&root, with_destination, timestamp, &guard)
    }

    /// Returns the immutable inode-version chain for a path, newest first.
    /// # Errors
    /// Returns an error when traversal, permissions, or validation fails.
    pub fn history(&self, path: &VirtualPath, actor: &Actor) -> Result<Vec<HistoryEntry>> {
        let (mut id, inode) = self.resolve(path, actor)?;
        Self::require(actor, &inode, READ, path)?;
        let mut history = Vec::new();
        let mut current = inode;
        loop {
            history.push(HistoryEntry {
                object_id: id,
                modified: current.modified,
                changed: current.changed,
            });
            match current.previous {
                Some(previous) => {
                    id = previous;
                    current = Inode::load(previous, self.repository.store())?;
                }
                None => break,
            }
        }
        Ok(history)
    }

    fn create_node(
        &mut self,
        path: &VirtualPath,
        kind: NodeKind,
        content: ObjectId,
        mode: u32,
        actor: &Actor,
        guard: &WriterGuard,
    ) -> Result<ObjectId> {
        if mode > 0o777 {
            return Err(Error::InvalidOperation("mode contains unsupported bits"));
        }
        let timestamp = Timestamp::now()?;
        let (parent_path, name) = path.split_parent()?;
        let root = self.root()?;
        let (_, parent) = self.resolve_in(root.root_inode, parent_path, actor)?;
        Self::require_directory(&parent, path)?;
        Self::require(actor, &parent, WRITE | EXECUTE, path)?;
        if Directory::load(parent.content, self.repository.store())?
            .get(name)
            .is_some()
        {
            return Err(Error::AlreadyExists(path.to_string()));
        }
        let inode = Inode {
            kind,
            node_id: UniqueId::new(),
            previous: None,
            mode,
            owner: actor.user,
            group: actor.primary_group,
            created: timestamp,
            modified: timestamp,
            changed: timestamp,
            accessed: timestamp,
            content,
        };
        let inode_id = inode.store(self.repository.store())?;
        let new_root_inode = self.rewrite_entry(
            root.root_inode,
            parent_path,
            name,
            Some(inode_id),
            timestamp,
        )?;
        self.commit(&root, new_root_inode, timestamp, guard)?;
        Ok(inode_id)
    }

    fn root(&self) -> Result<Root> {
        Ok(Root::load(self.repository.head(), self.repository.store())?)
    }
    fn resolve(&self, path: &VirtualPath, actor: &Actor) -> Result<(ObjectId, Inode)> {
        self.resolve_in(self.root()?.root_inode, path.components(), actor)
    }
    fn resolve_in(
        &self,
        root_inode: ObjectId,
        components: &[String],
        actor: &Actor,
    ) -> Result<(ObjectId, Inode)> {
        let mut id = root_inode;
        let mut inode = Inode::load(id, self.repository.store())?;
        let mut traversed = Vec::new();
        for component in components {
            let current_path = VirtualPath {
                components: traversed.clone(),
            };
            Self::require_directory(&inode, &current_path)?;
            Self::require(actor, &inode, EXECUTE, &current_path)?;
            let directory = Directory::load(inode.content, self.repository.store())?;
            traversed.push(component.clone());
            id = directory.get(component).ok_or_else(|| {
                Error::NotFound(
                    VirtualPath {
                        components: traversed.clone(),
                    }
                    .to_string(),
                )
            })?;
            inode = Inode::load(id, self.repository.store())?;
        }
        Ok((id, inode))
    }
    fn require(actor: &Actor, inode: &Inode, bits: u32, path: &VirtualPath) -> Result<()> {
        if actor.permits(inode, bits) {
            Ok(())
        } else {
            Err(Error::PermissionDenied(path.to_string()))
        }
    }
    fn require_directory(inode: &Inode, path: &VirtualPath) -> Result<()> {
        if inode.kind == NodeKind::Directory {
            Ok(())
        } else {
            Err(Error::NotDirectory(path.to_string()))
        }
    }
    fn stat_inode(&self, id: ObjectId, inode: &Inode) -> Result<NodeStat> {
        let size = match inode.kind {
            NodeKind::File => {
                ChunkedFile::load(inode.content, self.repository.store())?.logical_length
            }
            NodeKind::Directory => u64::try_from(
                Directory::load(inode.content, self.repository.store())?
                    .entries()
                    .len(),
            )
            .map_err(|_| Error::InvalidOperation("directory size overflow"))?,
        };
        Ok(NodeStat {
            object_id: id,
            node_id: inode.node_id,
            kind: inode.kind,
            mode: inode.mode,
            owner: inode.owner,
            group: inode.group,
            created: inode.created,
            modified: inode.modified,
            changed: inode.changed,
            size,
        })
    }

    fn replace_inode(
        &self,
        current_id: ObjectId,
        path: &[String],
        replacement: ObjectId,
        timestamp: Timestamp,
    ) -> Result<ObjectId> {
        if path.is_empty() {
            return Ok(replacement);
        }
        let mut current = Inode::load(current_id, self.repository.store())?;
        if current.kind != NodeKind::Directory {
            return Err(Error::NotDirectory(format!("/{}", path.join("/"))));
        }
        let mut directory = Directory::load(current.content, self.repository.store())?;
        let child = directory
            .get(&path[0])
            .ok_or_else(|| Error::NotFound(format!("/{}", path.join("/"))))?;
        let rewritten = self.replace_inode(child, &path[1..], replacement, timestamp)?;
        directory.insert(path[0].clone(), rewritten);
        current.content = directory.store(self.repository.store())?;
        current.previous = Some(current_id);
        current.modified = timestamp;
        current.changed = timestamp;
        Ok(current.store(self.repository.store())?)
    }
    fn rewrite_entry(
        &self,
        current_id: ObjectId,
        directory_path: &[String],
        name: &str,
        replacement: Option<ObjectId>,
        timestamp: Timestamp,
    ) -> Result<ObjectId> {
        let (directory_id, mut inode) =
            self.resolve_in(current_id, directory_path, &Actor::root())?;
        let mut directory = Directory::load(inode.content, self.repository.store())?;
        match replacement {
            Some(id) => {
                directory.insert(name.to_owned(), id);
            }
            None => {
                if directory.remove(name).is_none() {
                    return Err(Error::NotFound(name.to_owned()));
                }
            }
        }
        inode.content = directory.store(self.repository.store())?;
        inode.previous = Some(directory_id);
        inode.modified = timestamp;
        inode.changed = timestamp;
        let replacement_id = inode.store(self.repository.store())?;
        self.replace_inode(current_id, directory_path, replacement_id, timestamp)
    }
    fn commit(
        &mut self,
        previous: &Root,
        root_inode: ObjectId,
        timestamp: Timestamp,
        guard: &WriterGuard,
    ) -> Result<()> {
        let expected = self.repository.head();
        let generation = previous
            .generation
            .checked_add(1)
            .ok_or(Error::GenerationOverflow)?;
        let root = Root {
            root_inode,
            generation,
            previous: Some(expected),
            transaction: UniqueId::new(),
            committed: timestamp,
        };
        let new_head = root.store(self.repository.store())?;
        self.repository.commit_root(guard, expected, new_head)?;
        Ok(())
    }

    fn diff_nodes(
        &self,
        path: &str,
        left: Option<ObjectId>,
        right: Option<ObjectId>,
        output: &mut Vec<DiffEntry>,
    ) -> Result<()> {
        if left == right {
            return Ok(());
        }
        match (left, right) {
            (None, Some(_)) => output.push(DiffEntry {
                path: path.to_owned(),
                kind: DiffKind::Added,
            }),
            (Some(_), None) => output.push(DiffEntry {
                path: path.to_owned(),
                kind: DiffKind::Removed,
            }),
            (Some(left_id), Some(right_id)) => {
                let left_inode = Inode::load(left_id, self.repository.store())?;
                let right_inode = Inode::load(right_id, self.repository.store())?;
                if left_inode.kind != right_inode.kind {
                    output.push(DiffEntry {
                        path: path.to_owned(),
                        kind: DiffKind::TypeChanged,
                    });
                    return Ok(());
                }
                match left_inode.kind {
                    NodeKind::File => {
                        let kind = if left_inode.content == right_inode.content {
                            DiffKind::MetadataModified
                        } else {
                            DiffKind::ContentModified
                        };
                        output.push(DiffEntry {
                            path: path.to_owned(),
                            kind,
                        });
                    }
                    NodeKind::Directory => {
                        if left_inode.mode != right_inode.mode
                            || left_inode.owner != right_inode.owner
                            || left_inode.group != right_inode.group
                        {
                            output.push(DiffEntry {
                                path: path.to_owned(),
                                kind: DiffKind::MetadataModified,
                            });
                        }
                        let left_dir =
                            Directory::load(left_inode.content, self.repository.store())?;
                        let right_dir =
                            Directory::load(right_inode.content, self.repository.store())?;
                        let names: BTreeSet<&String> = left_dir
                            .entries()
                            .keys()
                            .chain(right_dir.entries().keys())
                            .collect();
                        for name in names {
                            let child_path = if path == "/" {
                                format!("/{name}")
                            } else {
                                format!("{path}/{name}")
                            };
                            self.diff_nodes(
                                &child_path,
                                left_dir.get(name),
                                right_dir.get(name),
                                output,
                            )?;
                        }
                    }
                }
            }
            (None, None) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "atlasfs-core-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn path(value: &str) -> VirtualPath {
        VirtualPath::parse(value).unwrap()
    }

    #[test]
    fn file_directory_history_and_rename_work() {
        let temp = Temp::new();
        let root = Actor::root();
        let mut vfs = VirtualFileSystem::init(temp.path()).unwrap();
        assert_eq!(vfs.generation().unwrap(), 0);
        vfs.create_directory(&path("/docs"), 0o755, &root).unwrap();
        let first = vfs
            .create_file(&path("/docs/readme"), b"one", 0o644, &root)
            .unwrap();
        assert_eq!(vfs.read_file(&path("/docs/readme"), &root).unwrap(), b"one");
        let first_stat = vfs.stat(&path("/docs/readme"), &root).unwrap();
        assert_eq!(first_stat.object_id, first);
        assert_eq!(first_stat.size, 3);
        vfs.write_file(&path("/docs/readme"), b"version two", &root)
            .unwrap();
        let second_stat = vfs.stat(&path("/docs/readme"), &root).unwrap();
        assert_eq!(first_stat.node_id, second_stat.node_id);
        assert_ne!(first_stat.object_id, second_stat.object_id);
        assert_eq!(vfs.history(&path("/docs/readme"), &root).unwrap().len(), 2);
        vfs.rename(&path("/docs/readme"), &path("/README"), &root)
            .unwrap();
        assert!(matches!(
            vfs.stat(&path("/docs/readme"), &root),
            Err(Error::NotFound(_))
        ));
        assert_eq!(
            vfs.read_file(&path("/README"), &root).unwrap(),
            b"version two"
        );
        assert_eq!(vfs.list(&path("/"), &root).unwrap(), ["README", "docs"]);
        assert_eq!(vfs.generation().unwrap(), 4);
        drop(vfs);
        let reopened = VirtualFileSystem::open(temp.path()).unwrap();
        assert_eq!(
            reopened.read_file(&path("/README"), &root).unwrap(),
            b"version two"
        );
    }

    #[test]
    fn permissions_and_removal_rules_are_enforced() {
        let temp = Temp::new();
        let root = Actor::root();
        let owner = Actor::new(1000, 100, Vec::new());
        let stranger = Actor::new(2000, 200, Vec::new());
        let mut vfs = VirtualFileSystem::init(temp.path()).unwrap();
        vfs.create_directory(&path("/home"), 0o777, &root).unwrap();
        vfs.create_directory(&path("/home/private"), 0o700, &owner)
            .unwrap();
        vfs.create_file(&path("/home/private/data"), b"secret", 0o600, &owner)
            .unwrap();
        assert!(matches!(
            vfs.read_file(&path("/home/private/data"), &stranger),
            Err(Error::PermissionDenied(_))
        ));
        assert!(matches!(
            vfs.remove(&path("/home/private"), &owner),
            Err(Error::DirectoryNotEmpty(_))
        ));
        vfs.chmod(&path("/home/private/data"), 0o604, &owner)
            .unwrap();
        assert!(
            matches!(
                vfs.read_file(&path("/home/private/data"), &stranger),
                Err(Error::PermissionDenied(_))
            ),
            "directory traversal must still fail"
        );
        vfs.remove_recursive(&path("/home/private"), &owner)
            .unwrap();
        assert!(matches!(
            vfs.stat(&path("/home/private"), &root),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn snapshots_preserve_diff_restore_and_delete() {
        let temp = Temp::new();
        let root = Actor::root();
        let mut vfs = VirtualFileSystem::init(temp.path()).unwrap();
        vfs.create_file(&path("/data"), b"version one", 0o644, &root)
            .unwrap();
        let first = vfs.create_snapshot("release one").unwrap();
        vfs.write_file(&path("/data"), b"version two", &root)
            .unwrap();
        vfs.create_file(&path("/added"), b"new", 0o644, &root)
            .unwrap();
        vfs.create_snapshot("release-two").unwrap();
        assert_eq!(
            vfs.read_snapshot_file("release one", &path("/data"), &root)
                .unwrap(),
            b"version one"
        );
        let diff = vfs.diff_snapshots("release one", "release-two").unwrap();
        assert_eq!(
            diff,
            vec![
                DiffEntry {
                    path: "/added".to_owned(),
                    kind: DiffKind::Added
                },
                DiffEntry {
                    path: "/data".to_owned(),
                    kind: DiffKind::ContentModified
                }
            ]
        );
        let generation = vfs.generation().unwrap();
        vfs.restore_snapshot("release one").unwrap();
        assert_eq!(vfs.generation().unwrap(), generation + 1);
        assert_eq!(
            vfs.read_file(&path("/data"), &root).unwrap(),
            b"version one"
        );
        assert!(matches!(
            vfs.stat(&path("/added"), &root),
            Err(Error::NotFound(_))
        ));
        assert_eq!(vfs.snapshots().unwrap().len(), 2);
        vfs.delete_snapshot("release one").unwrap();
        assert_eq!(vfs.snapshots().unwrap().len(), 1);
        assert_eq!(first.name, "release one");
    }

    #[test]
    fn analytics_measure_current_deduplication_and_indexes_rebuild() {
        let temp = Temp::new();
        let root = Actor::root();
        let mut vfs = VirtualFileSystem::init(temp.path()).unwrap();
        vfs.create_file(&path("/one"), b"same bytes", 0o644, &root)
            .unwrap();
        vfs.create_file(&path("/two"), b"same bytes", 0o644, &root)
            .unwrap();

        let usage = vfs.storage_usage().unwrap();
        assert_eq!(usage.logical_bytes, 20);
        assert_eq!(usage.unique_chunks, 1);
        assert_eq!(usage.unique_chunk_bytes, 10);
        assert!((usage.deduplication_ratio - 2.0).abs() < f64::EPSILON);
        assert!(usage.metadata_bytes > 0);

        let indexes = vfs.rebuild_indexes().unwrap();
        assert_eq!(indexes.path_entries, 3);
        assert!(indexes.history_entries >= 3);
        assert_eq!(vfs.garbage_collect(true).unwrap().objects_removed, 0);
    }

    #[test]
    fn deterministic_randomized_versions_survive_reopen_and_verify() {
        let temp = Temp::new();
        let root = Actor::root();
        let target = path("/randomized");
        let mut vfs = VirtualFileSystem::init(temp.path()).unwrap();
        vfs.create_file(&target, &[], 0o644, &root).unwrap();
        let mut state = 0x4d59_5df4_d0f3_3173_u64;
        let mut expected = Vec::new();
        for iteration in 0..100 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let length = usize::try_from(state % 4096).unwrap();
            expected = (0..length)
                .map(|index| (state.wrapping_add(index as u64) & 0xff) as u8)
                .collect();
            vfs.write_file(&target, &expected, &root).unwrap();
            if iteration % 25 == 0 {
                vfs.create_snapshot(&format!("random-{iteration}")).unwrap();
            }
        }
        drop(vfs);
        let reopened = VirtualFileSystem::open(temp.path()).unwrap();
        assert_eq!(reopened.read_file(&target, &root).unwrap(), expected);
        assert!(reopened.verify().unwrap().objects_checked > 100);
    }
}
