use std::{fmt, io, path::PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    RepositoryAlreadyExists(PathBuf),
    RepositoryNotFound(PathBuf),
    RepositoryBusy(PathBuf),
    UnsupportedFormat(u16),
    UnsupportedFeature(String),
    InvalidObject(&'static str),
    InvalidObjectId(String),
    ObjectMissing(String),
    ObjectCorrupt(String),
    ReferenceCorrupt(String),
    JournalCorrupt(String),
    TransactionConflict,
    ObjectKindMismatch {
        expected: u8,
        actual: u8,
    },
    ResourceLimit {
        requested: u64,
        maximum: u64,
    },
}

impl Error {
    pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} failed for {}: {source}",
                path.display()
            ),
            Self::RepositoryAlreadyExists(path) => {
                write!(formatter, "repository already exists at {}", path.display())
            }
            Self::RepositoryNotFound(path) => {
                write!(formatter, "repository not found at {}", path.display())
            }
            Self::RepositoryBusy(path) => {
                write!(formatter, "repository writer is busy at {}", path.display())
            }
            Self::UnsupportedFormat(version) => {
                write!(formatter, "unsupported object format version {version}")
            }
            Self::UnsupportedFeature(feature) => {
                write!(formatter, "unsupported required feature: {feature}")
            }
            Self::InvalidObject(reason) => write!(formatter, "invalid object: {reason}"),
            Self::InvalidObjectId(value) => write!(formatter, "invalid object ID: {value}"),
            Self::ObjectMissing(id) => write!(formatter, "object is missing: {id}"),
            Self::ObjectCorrupt(id) => write!(formatter, "object failed verification: {id}"),
            Self::ReferenceCorrupt(reference) => {
                write!(formatter, "reference is corrupt: {reference}")
            }
            Self::JournalCorrupt(reason) => write!(formatter, "journal is corrupt: {reason}"),
            Self::TransactionConflict => {
                formatter.write_str("repository HEAD changed during mutation")
            }
            Self::ObjectKindMismatch { expected, actual } => write!(
                formatter,
                "object kind mismatch: expected {expected}, found {actual}"
            ),
            Self::ResourceLimit { requested, maximum } => write!(
                formatter,
                "object payload of {requested} bytes exceeds maximum of {maximum} bytes"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
