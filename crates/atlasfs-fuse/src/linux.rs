use super::{FuseAdapter, OpenMode};
use atlasfs_core::{Actor, Error, NodeKind, Timestamp, VirtualFileSystem, VirtualPath};
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use libc::{EACCES, EEXIST, EINVAL, EIO, EISDIR, ENOENT, ENOSYS, ENOTDIR, ENOTEMPTY};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::Path,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const TTL: Duration = Duration::from_secs(1);

struct LinuxFuse {
    adapter: FuseAdapter,
    paths: Mutex<BTreeMap<u64, VirtualPath>>,
    next_inode: u64,
}

pub(super) fn mount(repository: &Path, mountpoint: &Path) -> std::io::Result<()> {
    if !mountpoint.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "mountpoint is not a directory",
        ));
    }
    let adapter =
        FuseAdapter::open(repository).map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut paths = BTreeMap::new();
    paths.insert(1, VirtualPath::parse("/").expect("root path is valid"));
    let filesystem = LinuxFuse {
        adapter,
        paths: Mutex::new(paths),
        next_inode: 2,
    };
    fuser::mount2(
        filesystem,
        mountpoint,
        &[
            MountOption::RW,
            MountOption::FSName("atlasfs".to_owned()),
            MountOption::DefaultPermissions,
            MountOption::NoDev,
            MountOption::NoSuid,
        ],
    )
}

impl LinuxFuse {
    fn vfs(&self) -> Result<VirtualFileSystem, i32> {
        VirtualFileSystem::open(&self.adapter.repository).map_err(errno)
    }
    fn actor(request: &Request<'_>) -> Actor {
        Actor::new(request.uid(), request.gid(), Vec::new())
    }
    fn path(&self, inode: u64) -> Result<VirtualPath, i32> {
        self.paths
            .lock()
            .map_err(|_| EIO)?
            .get(&inode)
            .cloned()
            .ok_or(ENOENT)
    }
    fn child(&self, parent: u64, name: &OsStr) -> Result<VirtualPath, i32> {
        let name = name.to_str().ok_or(EINVAL)?;
        let parent = self.path(parent)?;
        VirtualPath::parse(&if parent.is_root() {
            format!("/{name}")
        } else {
            format!("{parent}/{name}")
        })
        .map_err(errno)
    }
    fn remember(&mut self, path: VirtualPath) -> Result<u64, i32> {
        let mut paths = self.paths.lock().map_err(|_| EIO)?;
        if let Some((inode, _)) = paths.iter().find(|(_, value)| **value == path) {
            return Ok(*inode);
        }
        let inode = self.next_inode;
        self.next_inode = self.next_inode.checked_add(1).ok_or(EIO)?;
        paths.insert(inode, path);
        Ok(inode)
    }
    fn attr(&self, inode: u64, path: &VirtualPath, actor: &Actor) -> Result<FileAttr, i32> {
        let stat = self.vfs()?.stat(path, actor).map_err(errno)?;
        Ok(FileAttr {
            ino: inode,
            size: stat.size,
            blocks: stat.size.div_ceil(512),
            atime: timestamp(stat.created),
            mtime: timestamp(stat.modified),
            ctime: timestamp(stat.changed),
            crtime: timestamp(stat.created),
            kind: match stat.kind {
                NodeKind::File => FileType::RegularFile,
                NodeKind::Directory => FileType::Directory,
            },
            perm: u16::try_from(stat.mode).map_err(|_| EIO)?,
            nlink: if stat.kind == NodeKind::Directory {
                2
            } else {
                1
            },
            uid: stat.owner,
            gid: stat.group,
            rdev: 0,
            blksize: 65_536,
            flags: 0,
        })
    }
    fn flush_handle(&self, handle: u64, actor: &Actor) -> Result<(), i32> {
        let mut handles = self.adapter.handles.lock().map_err(|_| EIO)?;
        handles
            .flush(handle, &mut self.vfs()?, actor)
            .map_err(errno)
    }
}

impl Filesystem for LinuxFuse {
    fn lookup(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let result = self
            .child(parent, name)
            .and_then(|path| {
                let inode = self.remember(path.clone())?;
                Ok((inode, path))
            })
            .and_then(|(inode, path)| self.attr(inode, &path, &Self::actor(req)));
        match result {
            Ok(attr) => reply.entry(&TTL, &attr, 0),
            Err(error) => reply.error(error),
        }
    }
    fn getattr(&mut self, req: &Request<'_>, inode: u64, _handle: Option<u64>, reply: ReplyAttr) {
        match self
            .path(inode)
            .and_then(|path| self.attr(inode, &path, &Self::actor(req)))
        {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(error) => reply.error(error),
        }
    }
    fn readdir(
        &mut self,
        req: &Request<'_>,
        inode: u64,
        _handle: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let result = self.path(inode).and_then(|path| {
            self.vfs()?
                .list(&path, &Self::actor(req))
                .map(|entries| (path, entries))
                .map_err(errno)
        });
        let Ok((parent, entries)) = result else {
            reply.error(result.unwrap_err());
            return;
        };
        let mut rows = vec![
            (inode, FileType::Directory, ".".to_owned()),
            (1, FileType::Directory, "..".to_owned()),
        ];
        for name in entries {
            let Ok(path) = VirtualPath::parse(&if parent.is_root() {
                format!("/{name}")
            } else {
                format!("{parent}/{name}")
            }) else {
                reply.error(EIO);
                return;
            };
            let Ok(child) = self.remember(path.clone()) else {
                reply.error(EIO);
                return;
            };
            let Ok(stat) = self
                .vfs()
                .and_then(|vfs| vfs.stat(&path, &Self::actor(req)).map_err(errno))
            else {
                reply.error(EIO);
                return;
            };
            rows.push((
                child,
                if stat.kind == NodeKind::Directory {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                },
                name,
            ));
        }
        let Ok(offset) = usize::try_from(offset.max(0)) else {
            reply.error(EINVAL);
            return;
        };
        for (index, (child, kind, name)) in rows.into_iter().enumerate().skip(offset) {
            let Ok(next) = i64::try_from(index + 1) else {
                reply.error(EIO);
                return;
            };
            if reply.add(child, next, kind, name) {
                break;
            }
        }
        reply.ok();
    }
    fn open(&mut self, req: &Request<'_>, inode: u64, flags: i32, reply: ReplyOpen) {
        let mode = match flags & libc::O_ACCMODE {
            libc::O_RDONLY => OpenMode::ReadOnly,
            libc::O_WRONLY => OpenMode::WriteOnly,
            libc::O_RDWR => OpenMode::ReadWrite,
            _ => {
                reply.error(EINVAL);
                return;
            }
        };
        let result = self.path(inode).and_then(|path| {
            let vfs = self.vfs()?;
            self.adapter
                .handles
                .lock()
                .map_err(|_| EIO)?
                .open(&vfs, path, &Self::actor(req), mode)
                .map_err(errno)
        });
        match result {
            Ok(handle) => reply.opened(handle, 0),
            Err(error) => reply.error(error),
        }
    }
    fn read(
        &mut self,
        _req: &Request<'_>,
        _inode: u64,
        handle: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _owner: Option<u64>,
        reply: ReplyData,
    ) {
        let result = u64::try_from(offset)
            .map_err(|_| EINVAL)
            .and_then(|offset| {
                self.adapter
                    .handles
                    .lock()
                    .map_err(|_| EIO)?
                    .read(handle, offset, size)
                    .map_err(errno)
            });
        match result {
            Ok(data) => reply.data(&data),
            Err(error) => reply.error(error),
        }
    }
    fn write(
        &mut self,
        _req: &Request<'_>,
        _inode: u64,
        handle: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let result = u64::try_from(offset)
            .map_err(|_| EINVAL)
            .and_then(|offset| {
                self.adapter
                    .handles
                    .lock()
                    .map_err(|_| EIO)?
                    .write(handle, offset, data)
                    .map_err(errno)
            });
        match result {
            Ok(count) => reply.written(count),
            Err(error) => reply.error(error),
        }
    }
    fn flush(
        &mut self,
        req: &Request<'_>,
        _inode: u64,
        handle: u64,
        _owner: u64,
        reply: ReplyEmpty,
    ) {
        match self.flush_handle(handle, &Self::actor(req)) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn fsync(
        &mut self,
        req: &Request<'_>,
        _inode: u64,
        handle: u64,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        match self.flush_handle(handle, &Self::actor(req)) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn release(
        &mut self,
        req: &Request<'_>,
        _inode: u64,
        handle: u64,
        _flags: i32,
        _owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let result = self
            .adapter
            .handles
            .lock()
            .map_err(|_| EIO)
            .and_then(|mut handles| {
                handles
                    .release(handle, &mut self.vfs()?, &Self::actor(req))
                    .map_err(errno)
            });
        match result {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(error),
        }
    }
    fn create(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let result = self.child(parent, name).and_then(|path| {
            let actor = Self::actor(req);
            let mut vfs = self.vfs()?;
            vfs.create_file(&path, &[], mode & !umask & 0o777, &actor)
                .map_err(errno)?;
            let inode = self.remember(path.clone())?;
            let attr = self.attr(inode, &path, &actor)?;
            let handle = self
                .adapter
                .handles
                .lock()
                .map_err(|_| EIO)?
                .open(
                    &vfs,
                    path,
                    &actor,
                    if flags & libc::O_ACCMODE == libc::O_RDWR {
                        OpenMode::ReadWrite
                    } else {
                        OpenMode::WriteOnly
                    },
                )
                .map_err(errno)?;
            Ok((attr, handle))
        });
        match result {
            Ok((attr, handle)) => reply.created(&TTL, &attr, 0, handle, 0),
            Err(error) => reply.error(error),
        }
    }
    fn mkdir(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let result = self.child(parent, name).and_then(|path| {
            self.vfs()?
                .create_directory(&path, mode & !umask & 0o777, &Self::actor(req))
                .map_err(errno)?;
            let inode = self.remember(path.clone())?;
            self.attr(inode, &path, &Self::actor(req))
        });
        match result {
            Ok(attr) => reply.entry(&TTL, &attr, 0),
            Err(error) => reply.error(error),
        }
    }
    fn unlink(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let result = self.child(parent, name).and_then(|path| {
            self.vfs()?
                .remove(&path, &Self::actor(req))
                .map_err(errno)?;
            self.adapter
                .handles
                .lock()
                .map_err(|_| EIO)?
                .unlinked(&path);
            Ok(())
        });
        mutate_empty(result, reply);
    }
    fn rmdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        mutate_empty(
            self.child(parent, name)
                .and_then(|path| self.vfs()?.remove(&path, &Self::actor(req)).map_err(errno)),
            reply,
        );
    }
    fn rename(
        &mut self,
        req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        if flags != 0 {
            reply.error(ENOSYS);
            return;
        }
        let result = self.child(parent, name).and_then(|source| {
            self.child(new_parent, new_name).and_then(|destination| {
                self.vfs()?
                    .rename(&source, &destination, &Self::actor(req))
                    .map_err(errno)?;
                self.adapter
                    .handles
                    .lock()
                    .map_err(|_| EIO)?
                    .renamed(&source, &destination)
                    .map_err(errno)
            })
        });
        mutate_empty(result, reply);
    }
    fn access(&mut self, req: &Request<'_>, inode: u64, _mask: i32, reply: ReplyEmpty) {
        mutate_empty(
            self.path(inode).and_then(|path| {
                self.vfs()?
                    .stat(&path, &Self::actor(req))
                    .map(|_| ())
                    .map_err(errno)
            }),
            reply,
        );
    }
}

fn mutate_empty(result: Result<(), i32>, reply: ReplyEmpty) {
    match result {
        Ok(()) => reply.ok(),
        Err(error) => reply.error(error),
    }
}
fn timestamp(value: Timestamp) -> SystemTime {
    UNIX_EPOCH
        + Duration::new(
            u64::try_from(value.seconds.max(0)).unwrap_or(0),
            value.nanoseconds,
        )
}
#[allow(clippy::needless_pass_by_value)]
fn errno(error: Error) -> i32 {
    match error {
        Error::NotFound(_) => ENOENT,
        Error::AlreadyExists(_) => EEXIST,
        Error::NotDirectory(_) => ENOTDIR,
        Error::IsDirectory(_) => EISDIR,
        Error::DirectoryNotEmpty(_) => ENOTEMPTY,
        Error::PermissionDenied(_) => EACCES,
        Error::InvalidPath(_) | Error::InvalidOperation(_) => EINVAL,
        _ => EIO,
    }
}
