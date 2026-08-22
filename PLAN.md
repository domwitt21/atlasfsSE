# AtlasFS — Versioned Virtual File System & Snapshot Engine

## Goal

Build a user-space virtual filesystem in Rust where files, directories, metadata, and snapshots are immutable versioned objects backed by content-addressed storage.

## Architectural direction

- Rust workspace with a reusable core, storage engine, CLI, REST server, optional FUSE adapter, and benchmarks.
- Immutable SHA-256-addressed chunks and objects.
- Files represented by manifests containing ordered chunk references.
- Directories represented by canonical, sorted entry maps.
- Copy-on-write Merkle DAG rooted at a committed root object.
- Named snapshots as immutable references to committed roots.
- Canonical repository state recoverable without its rebuildable indexes.
- Single-writer/multiple-reader transactions for the first release.

## Repository components

- `atlasfs-core`: namespace, metadata, permissions, transactions, history, and snapshots.
- `atlasfs-store`: object encoding, hashing, chunking, publication, verification, and GC.
- `atlasfs-cli`: local management commands and JSON output.
- `atlasfs-server`: REST administration API.
- `atlasfs-fuse`: optional Linux FUSE adapter.
- `atlasfs-bench`: benchmarks and repeatable workloads.

## Storage invariants

1. Published immutable objects never change.
2. Every object ID equals the SHA-256 digest of its canonical encoded bytes.
3. A committed root references only durable and structurally valid objects.
4. Directory entries are uniquely named and canonically ordered.
5. Readers observe either the old root or the new root, never a partial commit.
6. Snapshot references point only to committed roots.
7. Indexes are disposable and rebuildable from canonical objects and references.
8. Journal recovery is deterministic and idempotent.
9. Restore creates a new committed root transition and does not erase history.
10. Unreachable data is deleted only by explicit, reachability-checked garbage collection.

## Milestones

### M0 — Specification and scaffold

- Establish the Rust workspace and crate boundaries.
- Specify object types, canonical encoding, repository layout, error semantics, and format versioning.
- Specify the commit/recovery state machine and durability boundaries.
- Record path, permission, timestamp, locking, and compatibility rules.

### M1 — Content-addressed storage

- Initialize and open repositories.
- Implement canonical object envelopes and SHA-256 identities.
- Store objects in hash-sharded paths using atomic temporary-file publication.
- Add fixed 64 KiB chunking, streaming reads, deduplication, and integrity verification.
- Detect changed, truncated, missing, or incorrectly named objects.

### M2 — Virtual filesystem semantics

- Implement normalized path resolution.
- Add file create/read/write/truncate/delete operations.
- Add directory create/list/rename/delete operations.
- Add metadata, timestamps, ownership, and POSIX-inspired mode checks.
- Create immutable file, directory, and ancestor versions on mutation.

### M3 — Transactions and recovery

- Add exclusive writer locking and stable reader roots.
- Implement checksummed, sequence-numbered journal records.
- Stage objects, flush them, commit the journal, and atomically publish the root reference.
- Recover from partial records, abandoned transactions, and interrupted reference updates.
- Add fault injection at every write, flush, rename, and checkpoint boundary.

### M4 — Snapshots and history

- Create, inspect, list, delete, compare, and restore named snapshots.
- Traverse file and directory version history.
- Report added, removed, content-modified, and metadata-only differences.
- Preserve deduplication across all snapshots.

### M5 — Indexes, analytics, and garbage collection

Status: complete.

- Build disposable path, history, and reachability indexes.
- Rebuild and validate indexes from canonical storage.
- Implement dry-run and mark-and-sweep garbage collection.
- Report logical size, physical size, unique chunks, deduplication ratio, metadata overhead, and unreachable bytes.

### M6 — CLI

Status: complete.

- Provide `init`, `mkdir`, `put`, `get`, `cat`, `ls`, `stat`, `write`, `rm`, `mv`, and `chmod`.
- Provide snapshot, history, verify, recover, GC, usage, and server commands.
- Support stable exit codes plus human-readable and `--json` output.

### M7 — REST API

Status: complete.

- Add versioned endpoints for files, directories, snapshots, history, verification, and usage.
- Support streaming transfers, bearer authentication, request limits, audit logs, and OpenAPI documentation.
- Route all behavior through the same core services used by the CLI.

### M8 — Optional FUSE integration

Status: complete.

- Map Linux FUSE operations onto AtlasFS transactions.
- Define open-handle, `fsync`, rename, unlink, permission, and error semantics.
- Keep FUSE as an adapter rather than a source of storage behavior.

### M9 — Hardening and release

Status: complete.

- Add format compatibility and migration tests.
- Add hostile-input limits, tracing, backup/export tools, and a threat model.
- Produce reproducible builds and operational documentation.

## Test strategy

- Unit tests for encoding, hashing, paths, chunks, permissions, DAG traversal, journal parsing, and diffs.
- Property tests for serialization, content identity, randomized operations, and recovery invariants.
- Crash tests that terminate at every durability boundary.
- Corruption tests for bit flips, truncation, missing objects, bad hashes, damaged manifests, broken references, and bad indexes.
- Concurrency tests for readers during writes, competing writers, snapshots, GC, and abandoned locks.
- Benchmarks for sequential/random I/O, small overwrites, snapshot operations, integrity scans, recovery, and GC.

## MVP boundary

The MVP includes fixed-size chunking, content addressing, deduplication, files and directories, metadata, permissions, atomic single-writer transactions, snapshots, history, recovery, verification, analytics, and the CLI.

Defer FUSE, content-defined chunking, encryption, compression, replication, distributed locking, rich ACLs, remote backends, and simultaneous writers until the MVP is trustworthy.

## Release sequence

- `v0.1`: object store, files, directories, and basic CLI.
- `v0.2`: transactions, journal, recovery, and fault injection.
- `v0.3`: snapshots, history, diff, and restore.
- `v0.4`: indexes, analytics, verification, and GC.
- `v0.5`: authenticated REST API.
- `v0.6`: performance and format hardening.
- `v0.7`: experimental Linux FUSE adapter.
- `v1.0`: stable format and documented migration/recovery guarantees.

## Immediate next steps

1. Finish the format and recovery specifications.
2. Install the stable Rust toolchain with `rustfmt` and `clippy`.
3. Implement repository initialization and canonical object envelopes.
4. Add hash identity and corruption tests before namespace operations.
