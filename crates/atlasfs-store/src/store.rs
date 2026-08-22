use crate::{Error, Object, ObjectId, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VerificationReport {
    pub objects_checked: u64,
    pub bytes_checked: u64,
}

#[derive(Clone, Debug)]
pub struct ObjectStore {
    repository: PathBuf,
}

impl ObjectStore {
    /// Lists every canonically placed object ID.
    /// # Errors
    /// Returns an error for malformed placement or directory I/O failure.
    pub fn object_ids(&self) -> Result<Vec<ObjectId>> {
        let mut ids = Vec::new();
        let root = self.repository.join("objects");
        for shard in fs::read_dir(&root)
            .map_err(|source| Error::io("list object directory", &root, source))?
        {
            let shard = shard.map_err(|source| Error::io("read object shard", &root, source))?;
            if !shard.path().is_dir() {
                return Err(Error::InvalidObject("non-directory object shard"));
            }
            let prefix = shard.file_name().to_string_lossy().into_owned();
            if prefix.len() != 2 {
                return Err(Error::InvalidObject("invalid object shard name"));
            }
            for entry in fs::read_dir(shard.path())
                .map_err(|source| Error::io("list object shard", shard.path(), source))?
            {
                let entry =
                    entry.map_err(|source| Error::io("read object entry", shard.path(), source))?;
                if !entry.path().is_file() {
                    return Err(Error::InvalidObject("non-file entry in object shard"));
                }
                let value = format!("{prefix}{}", entry.file_name().to_string_lossy());
                ids.push(value.parse()?);
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    /// Returns the encoded size of an object.
    /// # Errors
    /// Returns an error when metadata cannot be read.
    pub fn encoded_size(&self, id: ObjectId) -> Result<u64> {
        fs::metadata(self.object_path(id))
            .map(|value| value.len())
            .map_err(|source| Error::io("inspect object size", self.object_path(id), source))
    }

    pub(crate) fn retire(&self, id: ObjectId) -> Result<u64> {
        let source = self.object_path(id);
        let size = self.encoded_size(id)?;
        let retired = self.repository.join("tmp").join(format!("gc-{id}.tmp"));
        atlasfs_platform::publish(&source, &retired, false)
            .map_err(|error| Error::io("retire unreachable object", &source, error))?;
        fs::remove_file(&retired)
            .map_err(|error| Error::io("delete retired object", &retired, error))?;
        Ok(size)
    }

    /// Opens the object store within an existing repository.
    ///
    /// # Errors
    /// Returns an error when required repository directories are absent.
    pub fn open(repository: impl AsRef<Path>) -> Result<Self> {
        let repository = repository.as_ref().to_path_buf();
        if !repository.join("objects").is_dir() || !repository.join("tmp").is_dir() {
            return Err(Error::RepositoryNotFound(repository));
        }
        Ok(Self { repository })
    }
    #[must_use]
    pub fn repository_path(&self) -> &Path {
        &self.repository
    }
    /// Publishes an immutable object or reuses its valid existing copy.
    ///
    /// # Errors
    /// Returns an error when publication fails or an existing copy is corrupt.
    pub fn put(&self, object: &Object) -> Result<ObjectId> {
        let encoded = object.encode();
        let id = ObjectId::digest(&encoded);
        let destination = self.object_path(id);
        if destination.exists() {
            Self::verify_expected(&destination, id)?;
            return Ok(id);
        }
        let parent = destination
            .parent()
            .ok_or(Error::InvalidObject("object path has no parent"))?;
        fs::create_dir_all(parent)
            .map_err(|source| Error::io("create object shard", parent, source))?;
        let temp = self.temp_path();
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)
                .map_err(|source| Error::io("create temporary object", &temp, source))?;
            file.write_all(&encoded)
                .map_err(|source| Error::io("write temporary object", &temp, source))?;
            file.sync_all()
                .map_err(|source| Error::io("flush temporary object", &temp, source))?;
            drop(file);
            match atlasfs_platform::publish(&temp, &destination, false) {
                Ok(()) => Ok(id),
                Err(_) if destination.exists() => {
                    Self::verify_expected(&destination, id)?;
                    fs::remove_file(&temp).map_err(|source| {
                        Error::io("remove duplicate temporary object", &temp, source)
                    })?;
                    Ok(id)
                }
                Err(source) => Err(Error::io("publish object", &destination, source)),
            }
        })();
        if result.is_err() && temp.exists() {
            let _ = fs::remove_file(temp);
        }
        result
    }
    /// Reads an object and verifies both its identity and canonical envelope.
    ///
    /// # Errors
    /// Returns an error when the object is missing, corrupt, or unreadable.
    pub fn get(&self, id: ObjectId) -> Result<Object> {
        let bytes = read_file(&self.object_path(id), id)?;
        if ObjectId::digest(&bytes) != id {
            return Err(Error::ObjectCorrupt(id.to_hex()));
        }
        Object::decode(&bytes).map_err(|_| Error::ObjectCorrupt(id.to_hex()))
    }
    /// Verifies one object and returns its encoded byte length.
    ///
    /// # Errors
    /// Returns an error when the object is missing, corrupt, or unreadable.
    pub fn verify(&self, id: ObjectId) -> Result<u64> {
        Self::verify_expected(&self.object_path(id), id)
    }
    /// Verifies every canonical object currently present in the store.
    ///
    /// # Errors
    /// Returns an error for malformed placement, corrupt objects, or I/O failures.
    pub fn verify_all(&self) -> Result<VerificationReport> {
        let mut report = VerificationReport::default();
        let root = self.repository.join("objects");
        for shard in fs::read_dir(&root)
            .map_err(|source| Error::io("list object directory", &root, source))?
        {
            let shard =
                shard.map_err(|source| Error::io("read object shard entry", &root, source))?;
            if !shard.path().is_dir() {
                return Err(Error::InvalidObject(
                    "non-directory entry in objects directory",
                ));
            }
            let prefix = shard.file_name().to_string_lossy().into_owned();
            if prefix.len() != 2 {
                return Err(Error::InvalidObject("invalid object shard name"));
            }
            for entry in fs::read_dir(shard.path())
                .map_err(|source| Error::io("list object shard", shard.path(), source))?
            {
                let entry =
                    entry.map_err(|source| Error::io("read object entry", shard.path(), source))?;
                if !entry.path().is_file() {
                    return Err(Error::InvalidObject("non-file entry in object shard"));
                }
                let id = format!("{prefix}{}", entry.file_name().to_string_lossy())
                    .parse::<ObjectId>()?;
                report.bytes_checked = report.bytes_checked.checked_add(self.verify(id)?).ok_or(
                    Error::ResourceLimit {
                        requested: u64::MAX,
                        maximum: u64::MAX - 1,
                    },
                )?;
                report.objects_checked += 1;
            }
        }
        Ok(report)
    }
    #[must_use]
    pub fn contains(&self, id: ObjectId) -> bool {
        self.object_path(id).is_file()
    }
    #[must_use]
    pub fn object_path(&self, id: ObjectId) -> PathBuf {
        let hex = id.to_hex();
        self.repository
            .join("objects")
            .join(&hex[..2])
            .join(&hex[2..])
    }
    fn verify_expected(path: &Path, id: ObjectId) -> Result<u64> {
        let bytes = read_file(path, id)?;
        if ObjectId::digest(&bytes) != id || Object::decode(&bytes).is_err() {
            return Err(Error::ObjectCorrupt(id.to_hex()));
        }
        Ok(bytes.len() as u64)
    }
    fn temp_path(&self) -> PathBuf {
        self.repository.join("tmp").join(format!(
            "object-{}-{:016x}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}

fn read_file(path: &Path, id: ObjectId) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            Error::ObjectMissing(id.to_hex())
        } else {
            Error::io("open object", path, source)
        }
    })?;
    let metadata = file
        .metadata()
        .map_err(|source| Error::io("inspect object", path, source))?;
    let maximum = crate::MAX_PAYLOAD_SIZE + 19;
    if metadata.len() > maximum {
        return Err(Error::ResourceLimit {
            requested: metadata.len(),
            maximum,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|source| Error::io("read object", path, source))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectKind, Repository};
    use std::fs;
    #[test]
    fn duplicate_objects_share_one_path() {
        let directory = crate::repository::test_directory("duplicate");
        let repository = Repository::init(directory.path()).unwrap();
        let object = Object::new(ObjectKind::Chunk, b"same bytes".to_vec()).unwrap();
        let first = repository.store().put(&object).unwrap();
        let second = repository.store().put(&object).unwrap();
        assert_eq!(first, second);
        assert_eq!(repository.store().get(first).unwrap(), object);
    }
    #[test]
    fn verification_detects_tampering() {
        let directory = crate::repository::test_directory("tamper");
        let repository = Repository::init(directory.path()).unwrap();
        let id = repository
            .store()
            .put(&Object::new(ObjectKind::Chunk, b"original".to_vec()).unwrap())
            .unwrap();
        fs::write(repository.store().object_path(id), b"changed").unwrap();
        assert!(matches!(
            repository.store().verify(id),
            Err(Error::ObjectCorrupt(_))
        ));
    }
}
