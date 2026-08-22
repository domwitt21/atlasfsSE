# Milestone 1 Acceptance Record

Milestone 1 delivers the first durable content-addressed storage slice.

- [x] Repository initialization uses a sibling staging directory and refuses existing destinations.
- [x] Initialization creates the documented repository directory structure and canonical configuration.
- [x] Initialization publishes an empty directory, root inode, generation-zero root, and `HEAD` reference.
- [x] Object envelopes use magic, version, type, big-endian length, and payload.
- [x] Object IDs are SHA-256 digests of complete canonical encoded bytes.
- [x] Object IDs have strict lowercase hexadecimal parsing and hash-sharded placement.
- [x] Object publication uses unique temporary files, file flushes, and atomic rename.
- [x] Re-publishing identical objects deduplicates them safely.
- [x] Fixed 65,536-byte streaming chunking handles empty, partial, and multi-chunk files.
- [x] File manifests have canonical encoding, persistence, decoding, and invariant validation.
- [x] Chunked files can be streamed back while checking object kinds and lengths.
- [x] Individual and full-store integrity verification detect hash or envelope corruption.
- [x] Object reads reject missing, oversized, malformed, or hash-mismatched data.
- [x] Format, compile, strict Clippy, unit-test, and doc-test gates pass.

## Verification

Verified on 2026-08-19 with Rust 1.97.1:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace` — 10 storage tests passed

## Platform durability

Object files are flushed before publication. Unix publication then renames the object and flushes the containing directory. Windows publication calls `MoveFileExW` with `MOVEFILE_WRITE_THROUGH`; replacement additionally uses `MOVEFILE_REPLACE_EXISTING`. The platform implementation is isolated in `atlasfs-platform`, while `atlasfs-store` and `atlasfs-core` continue to forbid unsafe code. New publication tests cover creation, replacement, and refusal to overwrite without explicit permission.

## Next target

Milestone 2 will add typed directory/inode codecs, normalized paths, namespace traversal, file and directory operations, metadata, permissions, and immutable version ancestry.
