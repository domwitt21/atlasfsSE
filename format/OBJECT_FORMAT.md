# AtlasFS Object Format

Status: accepted for the `v0.1` prototype. Format version: `1`.

## Principles

Objects are immutable. Their IDs are the lowercase hexadecimal SHA-256 digest of their complete canonical encoded bytes. Canonical encoding must be deterministic across machines and process runs. Unknown format versions are rejected rather than guessed.

## Integer and string encoding

AtlasFS uses a small purpose-built canonical binary codec. Unsigned integers are fixed-width big-endian values. Signed integers use fixed-width two's-complement big-endian values. Strings are valid UTF-8 prefixed by an unsigned 32-bit byte length. Byte arrays are prefixed by an unsigned 64-bit length. Lists are prefixed by an unsigned 32-bit element count. Optional values begin with `0` for absent or `1` for present. No alternate encodings are accepted.

Maps are encoded as lists in their specified canonical key order. Boolean values are encoded as exactly `0` or `1`. Reserved bytes must be zero. This deliberately avoids serializer configuration becoming part of the storage contract.

## Object envelope

Each encoded object contains, in order:

1. Eight-byte ASCII magic: `ATLASOBJ`.
2. Unsigned 16-bit format version in big-endian order.
3. Unsigned 8-bit object-kind discriminator.
4. Unsigned 64-bit payload length in big-endian order.
5. Canonically encoded payload bytes.

Initial kind assignments:

- `1`: chunk
- `2`: file manifest
- `3`: directory
- `4`: inode/version metadata
- `5`: committed root
- `6`: snapshot descriptor

The envelope and payload are both covered by the object ID. Decoders reject trailing bytes, invalid lengths, duplicate map keys, noncanonical ordering, and unsupported versions.

## Object placement

An object with ID `abcdef...` is stored at `objects/ab/cdef...`. Publication writes a uniquely named file beneath `tmp/`, flushes it, verifies its ID, and atomically renames it into the object path. An already-existing valid object is treated as successful deduplication.

## Chunk objects

The MVP uses fixed-size 65,536-byte chunks except for the final chunk of a file. The chunk payload is the raw file data. Empty files use a manifest with no chunks rather than an empty chunk object.

## File manifests

A file manifest payload contains the configured chunk size as `u32`, logical length as `u64`, and an ordered list of entries. Each entry contains a 32-byte binary chunk ID and `u32` chunk length. The sum of chunk lengths must equal the logical length. All non-final chunks use the configured repository chunk size.

## Directory objects

A directory payload begins with a `u32` entry count. Entries are sorted lexicographically by their UTF-8 bytes. Each entry contains a `u32` name length, name bytes, and a 32-byte inode/version object ID. Names must be unique and may not be empty, `.`, `..`, exceed 255 bytes, contain NUL, or contain a path separator.

## Inode/version objects

An inode/version payload contains, in order: `u8` node kind (`1` file, `2` directory), 16-byte stable logical inode ID, optional prior-version object ID, POSIX-inspired `u32` mode, `u32` owner, `u32` group, four timestamps (created, modified, metadata-changed, accessed), and a 32-byte content object ID. Timestamps are signed `i64` seconds from the Unix epoch plus `u32` nanoseconds in the inclusive range `0..=999_999_999`. Version 1 mode values may contain only the low nine permission bits.

## Root objects

A committed-root payload contains a 32-byte root inode/version ID, monotonically increasing `u64` generation, optional prior-root object ID, 16-byte transaction ID, and commit timestamp. Generation zero is the initialized empty filesystem.

Optional object IDs use a one-byte tag: `0` means absent and `1` means a 32-byte ID follows. Other tags are invalid.

## Snapshot objects

A snapshot payload contains a 32-byte committed-root object ID followed by its creation timestamp. Snapshot names are stored only in mutable references, so identical descriptors deduplicate naturally and renaming a reference does not rewrite immutable history.

## Validation limits

- Decoders reject objects larger than the repository's configured maximum before allocation.
- Counts and lengths are checked for overflow and against the remaining payload.
- All referenced IDs use exactly 32 binary bytes in payloads and 64 lowercase hex characters in paths and text interfaces.
- Version 1 has no optional payload fields. Future extensions require a new object version or an explicitly versioned enclosing structure.
- Repository feature flags are divided into `required` and `optional`; an opener rejects unknown required flags and ignores unknown optional flags without changing canonical objects.
