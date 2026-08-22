# Milestone 5 — Indexes, analytics, and garbage collection

Milestone 5 adds repository maintenance built on one authoritative object-graph traversal. `HEAD` and every named snapshot descriptor are roots; traversal follows root history, inode history, directory entries, file manifests, chunks, and snapshot targets.

## Disposable indexes

`VirtualFileSystem::rebuild_indexes` atomically rebuilds three versioned files under `index/`:

- `reachability-v1` contains every marked object ID.
- `paths-v1` maps hex-encoded current virtual paths to inode IDs.
- `history-v1` maps each current path to every inode version in its history chain.

These indexes are accelerators only. Canonical objects and references remain authoritative, so indexes can always be deleted and rebuilt.

## Analytics

`VirtualFileSystem::storage_usage` reports current logical bytes, physical and reachable bytes, unreachable bytes, object counts, unique current chunks and bytes, deduplication ratio, metadata storage, and snapshot count.

## Garbage collection

`VirtualFileSystem::garbage_collect(true)` performs a read-only preview. Passing `false` runs mark-and-sweep under the cross-process writer lock. Each unreachable object is first atomically renamed into `tmp/` through the durability abstraction and only then unlinked. Objects reachable through current state, history, or any named snapshot are retained.

The mark pass validates every visited object, so corrupt references stop collection before sweeping begins.
