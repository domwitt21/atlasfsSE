# Milestone 4 Acceptance Record

Milestone 4 adds named point-in-time snapshots and restoration.

- [x] Snapshot descriptors are immutable content-addressed objects.
- [x] Snapshot references use canonical reversible percent encoding.
- [x] Names are UTF-8, bounded to 128 bytes, and reject separators, NUL, `.`, and `..`.
- [x] Snapshot creation is constant-time apart from descriptor/reference publication.
- [x] Create, inspect, list, and delete operations use the writer lock where mutation occurs.
- [x] Snapshot lists are deterministic by UTF-8 name bytes.
- [x] Historical file reads resolve against the snapshot root without changing `HEAD`.
- [x] Tree diff reports added, removed, content-modified, metadata-modified, and type-changed paths.
- [x] Diff traversal skips identical object subtrees and returns canonical path order.
- [x] Restore commits the selected tree as a new journaled generation.
- [x] Restore preserves the current root as history and never rewinds generation.
- [x] Snapshot deletion removes only its reference; object reclamation remains explicit GC work.
- [x] Snapshot content remains deduplicated with live and other historical roots.
- [x] Format, compile, strict Clippy, unit-test, child-process, and doc-test gates pass.

## Verification

Verified on 2026-08-19 with Rust 1.97.1:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace` — 25 workspace tests passed

## Next target

Milestone 5 will add reachability indexes, storage analytics, index rebuilding, dry-run garbage collection, and safe mark-and-sweep reclamation.
