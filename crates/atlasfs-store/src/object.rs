use std::{fmt, str::FromStr};

use sha2::{Digest, Sha256};

use crate::{Error, OBJECT_FORMAT_VERSION, Result};

const MAGIC: &[u8; 8] = b"ATLASOBJ";
const HEADER_SIZE: usize = 19;
pub const MAX_PAYLOAD_SIZE: u64 = 1 << 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ObjectKind {
    Chunk = 1,
    FileManifest = 2,
    Directory = 3,
    Inode = 4,
    Root = 5,
    Snapshot = 6,
}

impl TryFrom<u8> for ObjectKind {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Chunk),
            2 => Ok(Self::FileManifest),
            3 => Ok(Self::Directory),
            4 => Ok(Self::Inode),
            5 => Ok(Self::Root),
            6 => Ok(Self::Snapshot),
            _ => Err(Error::InvalidObject("unknown object kind")),
        }
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectId([u8; 32]);

impl ObjectId {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    #[must_use]
    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ObjectId")
            .field(&self.to_hex())
            .finish()
    }
}
impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for ObjectId {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        if value.len() != 64 {
            return Err(Error::InvalidObjectId(value.to_owned()));
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high =
                decode_hex(pair[0]).ok_or_else(|| Error::InvalidObjectId(value.to_owned()))?;
            let low =
                decode_hex(pair[1]).ok_or_else(|| Error::InvalidObjectId(value.to_owned()))?;
            bytes[index] = (high << 4) | low;
        }
        let id = Self(bytes);
        if id.to_hex() != value {
            return Err(Error::InvalidObjectId(value.to_owned()));
        }
        Ok(id)
    }
}

const fn decode_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Object {
    kind: ObjectKind,
    payload: Vec<u8>,
}

impl Object {
    /// Creates an object after enforcing the configured payload limit.
    ///
    /// # Errors
    /// Returns [`Error::ResourceLimit`] when the payload is too large.
    pub fn new(kind: ObjectKind, payload: Vec<u8>) -> Result<Self> {
        let requested = u64::try_from(payload.len()).unwrap_or(u64::MAX);
        if requested > MAX_PAYLOAD_SIZE {
            return Err(Error::ResourceLimit {
                requested,
                maximum: MAX_PAYLOAD_SIZE,
            });
        }
        Ok(Self { kind, payload })
    }
    #[must_use]
    pub const fn kind(&self) -> ObjectKind {
        self.kind
    }
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(HEADER_SIZE + self.payload.len());
        encoded.extend_from_slice(MAGIC);
        encoded.extend_from_slice(&OBJECT_FORMAT_VERSION.to_be_bytes());
        encoded.push(self.kind as u8);
        encoded.extend_from_slice(&(self.payload.len() as u64).to_be_bytes());
        encoded.extend_from_slice(&self.payload);
        encoded
    }
    /// Decodes and validates a canonical object envelope.
    ///
    /// # Errors
    /// Returns an error for malformed, noncanonical, oversized, or unsupported input.
    pub fn decode(encoded: &[u8]) -> Result<Self> {
        if encoded.len() < HEADER_SIZE {
            return Err(Error::InvalidObject("truncated envelope"));
        }
        if &encoded[..8] != MAGIC {
            return Err(Error::InvalidObject("incorrect magic"));
        }
        let version = u16::from_be_bytes([encoded[8], encoded[9]]);
        if version != OBJECT_FORMAT_VERSION {
            return Err(Error::UnsupportedFormat(version));
        }
        let kind = ObjectKind::try_from(encoded[10])?;
        let length = u64::from_be_bytes(
            encoded[11..19]
                .try_into()
                .map_err(|_| Error::InvalidObject("invalid payload length"))?,
        );
        if length > MAX_PAYLOAD_SIZE {
            return Err(Error::ResourceLimit {
                requested: length,
                maximum: MAX_PAYLOAD_SIZE,
            });
        }
        if usize::try_from(length).ok() != Some(encoded.len() - HEADER_SIZE) {
            return Err(Error::InvalidObject("payload length mismatch"));
        }
        Self::new(kind, encoded[HEADER_SIZE..].to_vec())
    }
    #[must_use]
    pub fn id(&self) -> ObjectId {
        ObjectId::digest(&self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn object_round_trip_is_canonical() {
        let object = Object::new(ObjectKind::Chunk, b"atlas".to_vec()).unwrap();
        let encoded = object.encode();
        assert_eq!(Object::decode(&encoded).unwrap(), object);
        assert_eq!(ObjectId::digest(&encoded), object.id());
    }
    #[test]
    fn decoder_rejects_bad_lengths() {
        let object = Object::new(ObjectKind::Chunk, vec![1, 2, 3]).unwrap();
        let mut truncated = object.encode();
        truncated.pop();
        assert!(Object::decode(&truncated).is_err());
        let mut trailing = object.encode();
        trailing.push(0);
        assert!(Object::decode(&trailing).is_err());
    }
    #[test]
    fn object_id_requires_lowercase_hex() {
        let id = ObjectId::digest(b"atlas");
        assert_eq!(id.to_hex().parse::<ObjectId>().unwrap(), id);
        assert!(id.to_hex().to_uppercase().parse::<ObjectId>().is_err());
    }
    #[test]
    fn version_one_fixture_is_byte_stable() {
        let object = Object::new(ObjectKind::Chunk, b"atlas".to_vec()).unwrap();
        assert_eq!(
            object.encode(),
            b"ATLASOBJ\0\x01\x01\0\0\0\0\0\0\0\x05atlas"
        );
        assert_eq!(
            object.id().to_hex(),
            "4bd46615b4d3313a0f3f1a32759b5ff8f566cc60b248a9771969f969299f2b1d"
        );
    }

    #[test]
    fn future_object_versions_are_rejected() {
        let mut encoded = Object::new(ObjectKind::Chunk, Vec::new()).unwrap().encode();
        encoded[8..10].copy_from_slice(&2_u16.to_be_bytes());
        assert!(matches!(
            Object::decode(&encoded),
            Err(Error::UnsupportedFormat(2))
        ));
    }
}
