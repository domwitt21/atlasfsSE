use crate::{DEFAULT_CHUNK_SIZE, Error, Object, ObjectId, ObjectKind, ObjectStore, Result};
use std::io::{Read, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkRef {
    pub id: ObjectId,
    pub length: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkedFile {
    pub logical_length: u64,
    pub chunks: Vec<ChunkRef>,
}

impl ChunkedFile {
    /// Encodes the canonical version 1 file-manifest payload.
    ///
    /// # Errors
    /// Returns an error if lengths overflow or violate chunking invariants.
    pub fn encode_manifest(&self) -> Result<Vec<u8>> {
        validate_manifest(self)?;
        let count = u32::try_from(self.chunks.len()).map_err(|_| Error::ResourceLimit {
            requested: self.chunks.len() as u64,
            maximum: u64::from(u32::MAX),
        })?;
        let mut payload = Vec::with_capacity(16 + self.chunks.len() * 36);
        let chunk_size = u32::try_from(DEFAULT_CHUNK_SIZE)
            .map_err(|_| Error::InvalidObject("configured chunk size exceeds format"))?;
        payload.extend_from_slice(&chunk_size.to_be_bytes());
        payload.extend_from_slice(&self.logical_length.to_be_bytes());
        payload.extend_from_slice(&count.to_be_bytes());
        for chunk in &self.chunks {
            payload.extend_from_slice(chunk.id.as_bytes());
            payload.extend_from_slice(&chunk.length.to_be_bytes());
        }
        Ok(payload)
    }

    /// Decodes and validates a canonical version 1 file-manifest payload.
    ///
    /// # Errors
    /// Returns an error for truncation, trailing bytes, or invalid chunk lengths.
    pub fn decode_manifest(payload: &[u8]) -> Result<Self> {
        if payload.len() < 16 {
            return Err(Error::InvalidObject("truncated file manifest"));
        }
        let chunk_size = u32::from_be_bytes(
            payload[0..4]
                .try_into()
                .map_err(|_| Error::InvalidObject("invalid chunk size"))?,
        );
        if chunk_size as usize != DEFAULT_CHUNK_SIZE {
            return Err(Error::InvalidObject("unsupported manifest chunk size"));
        }
        let logical_length = u64::from_be_bytes(
            payload[4..12]
                .try_into()
                .map_err(|_| Error::InvalidObject("invalid logical length"))?,
        );
        let count = usize::try_from(u32::from_be_bytes(
            payload[12..16]
                .try_into()
                .map_err(|_| Error::InvalidObject("invalid chunk count"))?,
        ))
        .map_err(|_| Error::InvalidObject("chunk count unsupported"))?;
        let expected = 16_usize
            .checked_add(
                count
                    .checked_mul(36)
                    .ok_or(Error::InvalidObject("manifest length overflow"))?,
            )
            .ok_or(Error::InvalidObject("manifest length overflow"))?;
        if payload.len() != expected {
            return Err(Error::InvalidObject("file manifest length mismatch"));
        }
        let mut chunks = Vec::with_capacity(count);
        for entry in payload[16..].chunks_exact(36) {
            let mut id_bytes = [0_u8; 32];
            id_bytes.copy_from_slice(&entry[..32]);
            let id = ObjectId::from_bytes(id_bytes);
            let length = u32::from_be_bytes(
                entry[32..36]
                    .try_into()
                    .map_err(|_| Error::InvalidObject("invalid chunk length"))?,
            );
            chunks.push(ChunkRef { id, length });
        }
        let manifest = Self {
            logical_length,
            chunks,
        };
        validate_manifest(&manifest)?;
        Ok(manifest)
    }

    /// Stores this file manifest as an immutable object.
    ///
    /// # Errors
    /// Returns an error when validation or durable object publication fails.
    pub fn store(&self, store: &ObjectStore) -> Result<ObjectId> {
        store.put(&Object::new(
            ObjectKind::FileManifest,
            self.encode_manifest()?,
        )?)
    }

    /// Loads and validates a file manifest object.
    ///
    /// # Errors
    /// Returns an error when the object is missing, corrupt, the wrong kind, or malformed.
    pub fn load(id: ObjectId, store: &ObjectStore) -> Result<Self> {
        let object = store.get(id)?;
        if object.kind() != ObjectKind::FileManifest {
            return Err(Error::ObjectKindMismatch {
                expected: ObjectKind::FileManifest as u8,
                actual: object.kind() as u8,
            });
        }
        Self::decode_manifest(object.payload())
    }
}

fn validate_manifest(file: &ChunkedFile) -> Result<()> {
    let mut total = 0_u64;
    for (index, chunk) in file.chunks.iter().enumerate() {
        if chunk.length == 0
            || chunk.length as usize > DEFAULT_CHUNK_SIZE
            || (index + 1 != file.chunks.len() && chunk.length as usize != DEFAULT_CHUNK_SIZE)
        {
            return Err(Error::InvalidObject("chunk length violates manifest"));
        }
        total = total
            .checked_add(u64::from(chunk.length))
            .ok_or(Error::InvalidObject("manifest length overflow"))?;
    }
    if total != file.logical_length || (file.logical_length == 0) != file.chunks.is_empty() {
        return Err(Error::InvalidObject("logical file length mismatch"));
    }
    Ok(())
}

/// Streams input into fixed-size, deduplicated chunk objects.
///
/// # Errors
/// Returns an error when input cannot be read or a chunk cannot be stored durably.
pub fn chunk_reader(reader: &mut impl Read, store: &ObjectStore) -> Result<ChunkedFile> {
    let mut chunks = Vec::new();
    let mut logical_length = 0_u64;
    let mut buffer = vec![0_u8; DEFAULT_CHUNK_SIZE];
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            match reader.read(&mut buffer[filled..]) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(source) => return Err(Error::io("read chunk input", "<reader>", source)),
            }
        }
        if filled == 0 {
            break;
        }
        let id = store.put(&Object::new(ObjectKind::Chunk, buffer[..filled].to_vec())?)?;
        let length = u32::try_from(filled).map_err(|_| Error::InvalidObject("chunk too large"))?;
        logical_length =
            logical_length
                .checked_add(u64::from(length))
                .ok_or(Error::ResourceLimit {
                    requested: u64::MAX,
                    maximum: u64::MAX - 1,
                })?;
        chunks.push(ChunkRef { id, length });
        if filled < buffer.len() {
            break;
        }
    }
    Ok(ChunkedFile {
        logical_length,
        chunks,
    })
}

/// Reassembles a chunk sequence while validating its lengths and object kinds.
///
/// # Errors
/// Returns an error for missing/corrupt chunks, invalid metadata, or output failures.
pub fn read_chunked_file(
    file: &ChunkedFile,
    store: &ObjectStore,
    output: &mut impl Write,
) -> Result<()> {
    let mut observed = 0_u64;
    for (index, chunk) in file.chunks.iter().enumerate() {
        let object = store.get(chunk.id)?;
        if object.kind() != ObjectKind::Chunk {
            return Err(Error::ObjectKindMismatch {
                expected: ObjectKind::Chunk as u8,
                actual: object.kind() as u8,
            });
        }
        if object.payload().len() != chunk.length as usize
            || (index + 1 != file.chunks.len() && chunk.length as usize != DEFAULT_CHUNK_SIZE)
        {
            return Err(Error::InvalidObject("chunk length violates manifest"));
        }
        output
            .write_all(object.payload())
            .map_err(|source| Error::io("write chunk output", "<writer>", source))?;
        observed += u64::from(chunk.length);
    }
    if observed != file.logical_length {
        return Err(Error::InvalidObject("logical file length mismatch"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repository;
    use std::io::Cursor;
    #[test]
    fn chunks_round_trip_across_boundaries() {
        let directory = crate::repository::test_directory("chunks");
        let repository = Repository::init(directory.path()).unwrap();
        let input: Vec<u8> = (0..(DEFAULT_CHUNK_SIZE * 2 + 17))
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect();
        let file = chunk_reader(&mut Cursor::new(&input), repository.store()).unwrap();
        assert_eq!(file.chunks.len(), 3);
        assert_eq!(file.logical_length, input.len() as u64);
        let mut output = Vec::new();
        read_chunked_file(&file, repository.store(), &mut output).unwrap();
        assert_eq!(output, input);
    }
    #[test]
    fn empty_input_has_no_chunks() {
        let directory = crate::repository::test_directory("empty-chunks");
        let repository = Repository::init(directory.path()).unwrap();
        let file = chunk_reader(&mut Cursor::new(Vec::<u8>::new()), repository.store()).unwrap();
        assert_eq!(file.logical_length, 0);
        assert!(file.chunks.is_empty());
    }

    #[test]
    fn manifest_object_round_trips() {
        let directory = crate::repository::test_directory("manifest");
        let repository = Repository::init(directory.path()).unwrap();
        let file = chunk_reader(
            &mut Cursor::new(vec![7_u8; DEFAULT_CHUNK_SIZE + 5]),
            repository.store(),
        )
        .unwrap();
        let id = file.store(repository.store()).unwrap();
        assert_eq!(ChunkedFile::load(id, repository.store()).unwrap(), file);
    }
}
