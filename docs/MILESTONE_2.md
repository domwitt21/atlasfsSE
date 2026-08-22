# Milestone 2 Acceptance Record

Milestone 2 delivers a usable, versioned in-process virtual filesystem.

- [x] Directory, inode, timestamp, unique-ID, and committed-root codecs are typed and validated.
- [x] Repository initialization uses the same typed codecs as ordinary operations.
- [x] Virtual paths are absolute, UTF-8, case-sensitive, bounded, and strictly canonical.
- [x] Traversal resolves immutable directory and inode objects from the current root.
- [x] Directory listings use deterministic UTF-8 byte ordering.
- [x] File create, read, complete write, truncate-by-empty-write, stat, and removal are implemented.
- [x] Directory create, list, empty removal, recursive removal, and rename are implemented.
- [x] Rename publishes one new root and never changes the renamed node's logical identity.
- [x] Mutations rebuild only changed inode objects and ancestor directory paths.
- [x] Logical node IDs remain stable while inode object IDs change between versions.
- [x] Inode `previous` links expose file and metadata history newest-first.
- [x] Owner, primary/supplementary group, and other permission selection is implemented.
- [x] Directory traversal, listing, mutation, and file read/write permissions are enforced.
- [x] Root identity bypass and owner-or-root `chmod` are implemented.
- [x] One transaction timestamp is applied consistently to each mutation.
- [x] Every successful mutation creates a monotonically increasing root generation.
- [x] `HEAD` replacement uses the platform durability abstraction.
- [x] Reopening a repository preserves namespace and file content.
- [x] Format, compile, strict Clippy, unit-test, and doc-test gates pass.

## Verification

Verified on 2026-08-19 with Rust 1.97.1:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace` — 18 unit tests passed

## Deliberate boundary

Milestone 2 assumes the documented single-writer model. Object and root publication are atomic and durable, but there is not yet a journal, cross-process writer lock, or recovery replay. Those guarantees belong to Milestone 3. Symbolic links, hard links, special nodes, ACLs, sticky/set-ID bits, partial-range writes, and simultaneous writers remain deferred.

## Next target

Milestone 3 will add the exclusive writer lock, checksummed journal records, transaction state machine, recovery replay, checkpoints, and deterministic crash-point testing.
