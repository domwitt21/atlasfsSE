#![doc = "Content-addressed object storage for `AtlasFS`."]

mod chunk;
mod error;
mod fs_object;
mod journal;
mod maintenance;
mod object;
mod repository;
mod store;

pub use chunk::{ChunkRef, ChunkedFile, chunk_reader, read_chunked_file};
pub use error::{Error, Result};
pub use fs_object::{
    Directory, DirectoryEntry, Inode, NodeKind, Root, Snapshot, Timestamp, UniqueId,
};
pub use maintenance::{GcReport, IndexReport, StorageUsage};
pub use object::{MAX_PAYLOAD_SIZE, Object, ObjectId, ObjectKind};
pub use repository::{BackupReport, Repository, SnapshotInfo, WriterGuard};
pub use store::{ObjectStore, VerificationReport};

/// Current repository object-format version.
pub const OBJECT_FORMAT_VERSION: u16 = 1;

/// Fixed chunk size used by the initial repository format.
pub const DEFAULT_CHUNK_SIZE: usize = 65_536;
