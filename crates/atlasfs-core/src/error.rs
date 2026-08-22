use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Store(atlasfs_store::Error),
    InvalidPath(String),
    NotFound(String),
    AlreadyExists(String),
    NotDirectory(String),
    IsDirectory(String),
    DirectoryNotEmpty(String),
    PermissionDenied(String),
    InvalidOperation(&'static str),
    GenerationOverflow,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(source) => source.fmt(formatter),
            Self::InvalidPath(path) => write!(formatter, "invalid virtual path: {path}"),
            Self::NotFound(path) => write!(formatter, "path not found: {path}"),
            Self::AlreadyExists(path) => write!(formatter, "path already exists: {path}"),
            Self::NotDirectory(path) => write!(formatter, "not a directory: {path}"),
            Self::IsDirectory(path) => write!(formatter, "is a directory: {path}"),
            Self::DirectoryNotEmpty(path) => write!(formatter, "directory is not empty: {path}"),
            Self::PermissionDenied(path) => write!(formatter, "permission denied: {path}"),
            Self::InvalidOperation(reason) => write!(formatter, "invalid operation: {reason}"),
            Self::GenerationOverflow => formatter.write_str("root generation overflow"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(source) => Some(source),
            _ => None,
        }
    }
}
impl From<atlasfs_store::Error> for Error {
    fn from(value: atlasfs_store::Error) -> Self {
        Self::Store(value)
    }
}
