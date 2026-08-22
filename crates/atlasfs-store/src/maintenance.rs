use crate::{
    ChunkedFile, Directory, Error, Inode, NodeKind, ObjectId, ObjectKind, ObjectStore, Repository,
    Result, Root, Snapshot,
};
use std::{collections::BTreeSet, fmt::Write as _, fs::OpenOptions, io::Write, path::PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct StorageUsage {
    pub logical_bytes: u64,
    pub physical_bytes: u64,
    pub reachable_bytes: u64,
    pub unreachable_bytes: u64,
    pub total_objects: u64,
    pub reachable_objects: u64,
    pub unreachable_objects: u64,
    pub unique_chunks: u64,
    pub unique_chunk_bytes: u64,
    pub metadata_bytes: u64,
    pub deduplication_ratio: f64,
    pub snapshot_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GcReport {
    pub objects_considered: u64,
    pub unreachable_objects: u64,
    pub bytes_reclaimable: u64,
    pub objects_removed: u64,
    pub bytes_reclaimed: u64,
    pub dry_run: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexReport {
    pub reachable_objects: u64,
    pub path_entries: u64,
    pub history_entries: u64,
    pub reachability_path: PathBuf,
    pub path_index_path: PathBuf,
    pub history_index_path: PathBuf,
}

impl Repository {
    /// Computes logical, physical, deduplication, and reachability statistics.
    /// # Errors
    /// Returns an error when references or objects are unreadable or corrupt.
    pub fn storage_usage(&self) -> Result<StorageUsage> {
        let snapshots = self.snapshots()?;
        let reachable = reachable_objects(self, &snapshots)?;
        let all = self.store().object_ids()?;
        let physical_bytes = sum_sizes(self.store(), &all)?;
        let reachable_bytes = sum_sizes(self.store(), reachable.iter())?;
        let (logical_bytes, chunks) = current_namespace(self.store(), self.head())?;
        let unique_chunk_bytes = chunks.iter().try_fold(0_u64, |total, id| {
            let object = self.store().get(*id)?;
            total
                .checked_add(object.payload().len() as u64)
                .ok_or(Error::InvalidObject("storage usage overflow"))
        })?;
        let reachable_chunk_storage = reachable.iter().try_fold(0_u64, |total, id| {
            if self.store().get(*id)?.kind() == ObjectKind::Chunk {
                total
                    .checked_add(self.store().encoded_size(*id)?)
                    .ok_or(Error::InvalidObject("storage usage overflow"))
            } else {
                Ok(total)
            }
        })?;
        let total_objects = all.len() as u64;
        let reachable_objects = reachable.len() as u64;
        Ok(StorageUsage {
            logical_bytes,
            physical_bytes,
            reachable_bytes,
            unreachable_bytes: physical_bytes.saturating_sub(reachable_bytes),
            total_objects,
            reachable_objects,
            unreachable_objects: total_objects.saturating_sub(reachable_objects),
            unique_chunks: chunks.len() as u64,
            unique_chunk_bytes,
            metadata_bytes: reachable_bytes.saturating_sub(reachable_chunk_storage),
            deduplication_ratio: ratio(logical_bytes, unique_chunk_bytes),
            snapshot_count: snapshots.len() as u64,
        })
    }

    /// Rebuilds the durable reachability index from authoritative references.
    /// # Errors
    /// Returns an error when locking, traversal, or atomic publication fails.
    pub fn rebuild_reachability_index(&mut self) -> Result<IndexReport> {
        let _guard = self.begin_write()?;
        let snapshots = self.snapshots()?;
        let reachable = reachable_objects(self, &snapshots)?;
        let mut reachability = format!("atlasfs-reachability 1\nhead={}\n", self.head());
        for id in &reachable {
            writeln!(reachability, "{id}").expect("writing to String cannot fail");
        }
        let (paths, histories) = namespace_indexes(self.store(), self.head())?;
        let mut path_data = String::from("atlasfs-paths 1\n");
        let mut history_data = String::from("atlasfs-history 1\n");
        for (path, inode) in &paths {
            writeln!(path_data, "{}\t{inode}", encode_path(path))
                .expect("writing to String cannot fail");
            let mut current = Some(*inode);
            while let Some(id) = current {
                writeln!(history_data, "{}\t{id}", encode_path(path))
                    .expect("writing to String cannot fail");
                current = Inode::load(id, self.store())?.previous;
            }
        }
        let reachability_path = publish_index(self, "reachability-v1", reachability.as_bytes())?;
        let path_index_path = publish_index(self, "paths-v1", path_data.as_bytes())?;
        let history_index_path = publish_index(self, "history-v1", history_data.as_bytes())?;
        Ok(IndexReport {
            reachable_objects: reachable.len() as u64,
            path_entries: paths.len() as u64,
            history_entries: histories,
            reachability_path,
            path_index_path,
            history_index_path,
        })
    }

    /// Finds unreachable objects and optionally removes them under the writer lock.
    /// # Errors
    /// Returns an error when locking, traversal, sizing, or durable retirement fails.
    pub fn garbage_collect(&mut self, dry_run: bool) -> Result<GcReport> {
        let _guard = self.begin_write()?;
        let snapshots = self.snapshots()?;
        let reachable = reachable_objects(self, &snapshots)?;
        let all = self.store().object_ids()?;
        let unreachable: Vec<_> = all
            .iter()
            .copied()
            .filter(|id| !reachable.contains(id))
            .collect();
        let bytes_reclaimable = sum_sizes(self.store(), &unreachable)?;
        let mut bytes_reclaimed = 0;
        if !dry_run {
            for id in &unreachable {
                bytes_reclaimed += self.store().retire(*id)?;
            }
        }
        Ok(GcReport {
            objects_considered: all.len() as u64,
            unreachable_objects: unreachable.len() as u64,
            bytes_reclaimable,
            objects_removed: if dry_run { 0 } else { unreachable.len() as u64 },
            bytes_reclaimed,
            dry_run,
        })
    }
}

fn publish_index(repository: &Repository, name: &str, data: &[u8]) -> Result<PathBuf> {
    let destination = repository.path().join("index").join(name);
    let temporary = repository.path().join("tmp").join(format!("{name}.tmp"));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|source| Error::io("create index", &temporary, source))?;
    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|source| Error::io("write index", &temporary, source))?;
    drop(file);
    atlasfs_platform::publish(&temporary, &destination, true)
        .map_err(|source| Error::io("publish index", &destination, source))?;
    Ok(destination)
}

fn namespace_indexes(
    store: &ObjectStore,
    root: ObjectId,
) -> Result<(Vec<(String, ObjectId)>, u64)> {
    let root = Root::load(root, store)?;
    let mut stack = vec![(String::from("/"), root.root_inode)];
    let mut paths = Vec::new();
    let mut histories = 0_u64;
    while let Some((path, id)) = stack.pop() {
        paths.push((path.clone(), id));
        let inode = Inode::load(id, store)?;
        let mut previous = inode.previous;
        histories += 1;
        while let Some(old) = previous {
            let version = Inode::load(old, store)?;
            histories = histories
                .checked_add(1)
                .ok_or(Error::InvalidObject("history index overflow"))?;
            previous = version.previous;
        }
        if inode.kind == NodeKind::Directory {
            for (name, child) in Directory::load(inode.content, store)?
                .entries()
                .iter()
                .rev()
            {
                let child_path = if path == "/" {
                    format!("/{name}")
                } else {
                    format!("{path}/{name}")
                };
                stack.push((child_path, *child));
            }
        }
    }
    paths.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok((paths, histories))
}

fn encode_path(path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(path.len() * 2);
    for byte in path.as_bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[allow(clippy::cast_precision_loss)]
fn ratio(logical_bytes: u64, unique_chunk_bytes: u64) -> f64 {
    if unique_chunk_bytes == 0 {
        1.0
    } else {
        logical_bytes as f64 / unique_chunk_bytes as f64
    }
}

fn reachable_objects(
    repository: &Repository,
    snapshots: &[crate::SnapshotInfo],
) -> Result<BTreeSet<ObjectId>> {
    let mut stack = vec![repository.head()];
    stack.extend(snapshots.iter().map(|snapshot| snapshot.object));
    let mut marked = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !marked.insert(id) {
            continue;
        }
        match repository.store().get(id)?.kind() {
            ObjectKind::Chunk => {}
            ObjectKind::FileManifest => stack.extend(
                ChunkedFile::load(id, repository.store())?
                    .chunks
                    .into_iter()
                    .map(|chunk| chunk.id),
            ),
            ObjectKind::Directory => stack.extend(
                Directory::load(id, repository.store())?
                    .entries()
                    .values()
                    .copied(),
            ),
            ObjectKind::Inode => {
                let inode = Inode::load(id, repository.store())?;
                stack.push(inode.content);
                stack.extend(inode.previous);
            }
            ObjectKind::Root => {
                let root = Root::load(id, repository.store())?;
                stack.push(root.root_inode);
                stack.extend(root.previous);
            }
            ObjectKind::Snapshot => stack.push(Snapshot::load(id, repository.store())?.root),
        }
    }
    Ok(marked)
}

fn current_namespace(store: &ObjectStore, root: ObjectId) -> Result<(u64, BTreeSet<ObjectId>)> {
    let root = Root::load(root, store)?;
    let mut stack = vec![root.root_inode];
    let mut visited = BTreeSet::new();
    let mut chunks = BTreeSet::new();
    let mut logical = 0_u64;
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let inode = Inode::load(id, store)?;
        match inode.kind {
            NodeKind::Directory => {
                stack.extend(Directory::load(inode.content, store)?.entries().values());
            }
            NodeKind::File => {
                let manifest = ChunkedFile::load(inode.content, store)?;
                logical = logical
                    .checked_add(manifest.logical_length)
                    .ok_or(Error::InvalidObject("logical size overflow"))?;
                chunks.extend(manifest.chunks.into_iter().map(|chunk| chunk.id));
            }
        }
    }
    Ok((logical, chunks))
}

fn sum_sizes<'a>(store: &ObjectStore, ids: impl IntoIterator<Item = &'a ObjectId>) -> Result<u64> {
    ids.into_iter().try_fold(0_u64, |total, id| {
        total
            .checked_add(store.encoded_size(*id)?)
            .ok_or(Error::InvalidObject("storage usage overflow"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Object, repository::test_directory};

    #[test]
    fn indexes_usage_and_gc_share_authoritative_reachability() {
        let directory = test_directory("maintenance");
        let mut repository = Repository::init(directory.path()).unwrap();
        let orphan = repository
            .store()
            .put(&Object::new(ObjectKind::Chunk, b"orphan".to_vec()).unwrap())
            .unwrap();
        assert_eq!(repository.storage_usage().unwrap().unreachable_objects, 1);

        let index = repository.rebuild_reachability_index().unwrap();
        assert!(index.reachability_path.is_file());
        assert!(index.path_index_path.is_file());
        assert!(index.history_index_path.is_file());
        assert_eq!(index.path_entries, 1);

        let preview = repository.garbage_collect(true).unwrap();
        assert_eq!(preview.unreachable_objects, 1);
        assert_eq!(preview.objects_removed, 0);
        assert!(repository.store().get(orphan).is_ok());

        let collected = repository.garbage_collect(false).unwrap();
        assert_eq!(collected.objects_removed, 1);
        assert!(repository.store().get(orphan).is_err());
        assert!(repository.store().get(repository.head()).is_ok());
        assert_eq!(repository.storage_usage().unwrap().unreachable_objects, 0);
    }
}
