#![doc = "Versioned virtual filesystem semantics for `AtlasFS`."]

mod error;
mod path;
mod permission;
mod vfs;

pub use atlasfs_store::{
    BackupReport, GcReport, IndexReport, SnapshotInfo, StorageUsage, VerificationReport,
};
pub use error::{Error, Result};
pub use path::VirtualPath;
pub use permission::Actor;
pub use vfs::{DiffEntry, DiffKind, HistoryEntry, NodeStat, VirtualFileSystem};

pub use atlasfs_store::{DEFAULT_CHUNK_SIZE, NodeKind, OBJECT_FORMAT_VERSION, Timestamp, UniqueId};
