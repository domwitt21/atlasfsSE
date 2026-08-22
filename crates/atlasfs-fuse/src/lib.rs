#![doc = "Optional Linux `FUSE` adapter for `AtlasFS`."]

use atlasfs_core::{Actor, Error, VirtualFileSystem, VirtualPath};
use std::{collections::BTreeMap, path::Path, sync::Mutex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

#[derive(Debug)]
struct OpenHandle {
    path: Option<VirtualPath>,
    mode: OpenMode,
    data: Vec<u8>,
    dirty: bool,
}

/// Implements POSIX-style open-handle buffering over atomic `AtlasFS` versions.
#[derive(Debug)]
pub struct HandleTable {
    next: u64,
    handles: BTreeMap<u64, OpenHandle>,
}

impl Default for HandleTable {
    fn default() -> Self {
        Self {
            next: 1,
            handles: BTreeMap::new(),
        }
    }
}

impl HandleTable {
    /// Opens a path and snapshots its current contents into a handle.
    /// # Errors
    /// Returns an error for missing paths, permission failures, or handle exhaustion.
    pub fn open(
        &mut self,
        vfs: &VirtualFileSystem,
        path: VirtualPath,
        actor: &Actor,
        mode: OpenMode,
    ) -> atlasfs_core::Result<u64> {
        let data = vfs.read_file(&path, actor)?;
        let handle = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(Error::InvalidOperation("file handle space exhausted"))?;
        self.handles.insert(
            handle,
            OpenHandle {
                path: Some(path),
                mode,
                data,
                dirty: false,
            },
        );
        Ok(handle)
    }

    /// Reads a bounded range from the handle's stable open-time view.
    /// # Errors
    /// Returns an error for an invalid handle or numeric range overflow.
    pub fn read(&self, handle: u64, offset: u64, size: u32) -> atlasfs_core::Result<Vec<u8>> {
        let entry = self
            .handles
            .get(&handle)
            .ok_or(Error::InvalidOperation("invalid file handle"))?;
        if entry.mode == OpenMode::WriteOnly {
            return Err(Error::PermissionDenied(handle_path(entry)));
        }
        let start = usize::try_from(offset)
            .map_err(|_| Error::InvalidOperation("read offset unsupported"))?;
        if start >= entry.data.len() {
            return Ok(Vec::new());
        }
        let end = start.saturating_add(size as usize).min(entry.data.len());
        Ok(entry.data[start..end].to_vec())
    }

    /// Applies an in-memory write; publication occurs on flush, fsync, or release.
    /// # Errors
    /// Returns an error for a read-only handle or unsupported range.
    pub fn write(&mut self, handle: u64, offset: u64, data: &[u8]) -> atlasfs_core::Result<u32> {
        let entry = self
            .handles
            .get_mut(&handle)
            .ok_or(Error::InvalidOperation("invalid file handle"))?;
        if entry.mode == OpenMode::ReadOnly {
            return Err(Error::PermissionDenied(handle_path(entry)));
        }
        let start = usize::try_from(offset)
            .map_err(|_| Error::InvalidOperation("write offset unsupported"))?;
        let end = start
            .checked_add(data.len())
            .ok_or(Error::InvalidOperation("write range overflow"))?;
        if end > entry.data.len() {
            entry.data.resize(end, 0);
        }
        entry.data[start..end].copy_from_slice(data);
        entry.dirty = true;
        u32::try_from(data.len()).map_err(|_| Error::InvalidOperation("write is too large"))
    }

    /// Truncates or extends an open file with zeroes.
    /// # Errors
    /// Returns an error for a read-only or invalid handle.
    pub fn truncate(&mut self, handle: u64, size: u64) -> atlasfs_core::Result<()> {
        let entry = self
            .handles
            .get_mut(&handle)
            .ok_or(Error::InvalidOperation("invalid file handle"))?;
        if entry.mode == OpenMode::ReadOnly {
            return Err(Error::PermissionDenied(handle_path(entry)));
        }
        let size =
            usize::try_from(size).map_err(|_| Error::InvalidOperation("file size unsupported"))?;
        entry.data.resize(size, 0);
        entry.dirty = true;
        Ok(())
    }

    /// Atomically publishes dirty handle contents as a new file version.
    /// # Errors
    /// Returns an error if the handle or core commit is invalid.
    pub fn flush(
        &mut self,
        handle: u64,
        vfs: &mut VirtualFileSystem,
        actor: &Actor,
    ) -> atlasfs_core::Result<()> {
        let entry = self
            .handles
            .get_mut(&handle)
            .ok_or(Error::InvalidOperation("invalid file handle"))?;
        if entry.dirty {
            if let Some(path) = &entry.path {
                vfs.write_file(path, &entry.data, actor)?;
            }
            entry.dirty = false;
        }
        Ok(())
    }

    /// Flushes and closes a handle.
    /// # Errors
    /// Returns an error when publication fails or the handle is invalid.
    pub fn release(
        &mut self,
        handle: u64,
        vfs: &mut VirtualFileSystem,
        actor: &Actor,
    ) -> atlasfs_core::Result<()> {
        self.flush(handle, vfs, actor)?;
        self.handles
            .remove(&handle)
            .ok_or(Error::InvalidOperation("invalid file handle"))?;
        Ok(())
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn renamed(
        &mut self,
        source: &VirtualPath,
        destination: &VirtualPath,
    ) -> atlasfs_core::Result<()> {
        for entry in self.handles.values_mut() {
            let Some(path) = &entry.path else { continue };
            if path.starts_with(source) {
                let suffix = &path.components()[source.components().len()..];
                let mut value = destination.to_string();
                for component in suffix {
                    if value != "/" {
                        value.push('/');
                    }
                    value.push_str(component);
                }
                entry.path = Some(VirtualPath::parse(&value)?);
            }
        }
        Ok(())
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn unlinked(&mut self, path: &VirtualPath) {
        for entry in self.handles.values_mut() {
            if entry
                .path
                .as_ref()
                .is_some_and(|open| open.starts_with(path))
            {
                entry.path = None;
            }
        }
    }
}

fn handle_path(entry: &OpenHandle) -> String {
    entry
        .path
        .as_ref()
        .map_or_else(|| "<unlinked>".to_owned(), ToString::to_string)
}

#[derive(Debug)]
pub struct FuseAdapter {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    repository: std::path::PathBuf,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    handles: Mutex<HandleTable>,
}

impl FuseAdapter {
    /// Opens the repository used by a mount adapter.
    /// # Errors
    /// Returns an error when the repository is invalid.
    pub fn open(repository: impl AsRef<Path>) -> atlasfs_core::Result<Self> {
        let repository = repository.as_ref().to_path_buf();
        VirtualFileSystem::open(&repository)?;
        Ok(Self {
            repository,
            handles: Mutex::new(HandleTable::default()),
        })
    }
}

#[cfg(target_os = "linux")]
mod linux;

/// Mounts a repository at a Linux mountpoint until unmounted.
/// # Errors
/// Returns an error for invalid repositories, mountpoints, or kernel FUSE failures.
#[cfg(target_os = "linux")]
pub fn mount(repository: impl AsRef<Path>, mountpoint: impl AsRef<Path>) -> std::io::Result<()> {
    linux::mount(repository.as_ref(), mountpoint.as_ref())
}

/// Reports that FUSE mounting is Linux-only.
/// # Errors
/// Always returns [`std::io::ErrorKind::Unsupported`] on non-Linux targets.
#[cfg(not(target_os = "linux"))]
pub fn mount(_repository: impl AsRef<Path>, _mountpoint: impl AsRef<Path>) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "AtlasFS FUSE integration requires Linux",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn handles_hold_stable_views_and_commit_on_fsync() {
        let directory = std::env::temp_dir().join(format!(
            "atlas-fuse-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let actor = Actor::root();
        let path = VirtualPath::parse("/file").unwrap();
        let mut vfs = VirtualFileSystem::init(&directory).unwrap();
        vfs.create_file(&path, b"abcdef", 0o644, &actor).unwrap();
        let mut handles = HandleTable::default();
        let reader = handles
            .open(&vfs, path.clone(), &actor, OpenMode::ReadOnly)
            .unwrap();
        let writer = handles
            .open(&vfs, path.clone(), &actor, OpenMode::ReadWrite)
            .unwrap();
        handles.write(writer, 2, b"XYZ").unwrap();
        assert_eq!(vfs.read_file(&path, &actor).unwrap(), b"abcdef");
        handles.flush(writer, &mut vfs, &actor).unwrap();
        assert_eq!(vfs.read_file(&path, &actor).unwrap(), b"abXYZf");
        assert_eq!(handles.read(reader, 0, 20).unwrap(), b"abcdef");
        handles.release(reader, &mut vfs, &actor).unwrap();
        handles.release(writer, &mut vfs, &actor).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn handles_follow_rename_and_do_not_recreate_unlinked_files() {
        let directory = std::env::temp_dir().join(format!(
            "atlas-fuse-rename-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let actor = Actor::root();
        let source = VirtualPath::parse("/source").unwrap();
        let destination = VirtualPath::parse("/destination").unwrap();
        let mut vfs = VirtualFileSystem::init(&directory).unwrap();
        vfs.create_file(&source, b"one", 0o644, &actor).unwrap();
        let mut handles = HandleTable::default();
        let handle = handles
            .open(&vfs, source.clone(), &actor, OpenMode::ReadWrite)
            .unwrap();
        vfs.rename(&source, &destination, &actor).unwrap();
        handles.renamed(&source, &destination).unwrap();
        handles.write(handle, 0, b"two").unwrap();
        handles.flush(handle, &mut vfs, &actor).unwrap();
        assert_eq!(vfs.read_file(&destination, &actor).unwrap(), b"two");
        vfs.remove(&destination, &actor).unwrap();
        handles.unlinked(&destination);
        handles.write(handle, 0, b"gone").unwrap();
        handles.release(handle, &mut vfs, &actor).unwrap();
        assert!(matches!(
            vfs.stat(&destination, &actor),
            Err(Error::NotFound(_))
        ));
        fs::remove_dir_all(directory).unwrap();
    }
}
