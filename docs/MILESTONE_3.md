# Milestone 3 Acceptance Record

Milestone 3 adds serialized writes, a write-ahead journal, deterministic recovery, and commit fault testing.

- [x] Every VFS mutation acquires an OS-backed exclusive writer lock before reading its mutation root.
- [x] Cross-process contention returns the stable `RepositoryBusy` error.
- [x] Lock ownership is tied to the operating-system file handle, so process exit releases it.
- [x] Journal records use magic, version, kind, reserved byte, sequence, transaction ID, payload length, payload, and SHA-256 checksum.
- [x] Sequence gaps, unknown versions/kinds, nonzero reserved bytes, malformed payloads, and mid-stream checksum failures are rejected.
- [x] A torn final record is identified, ignored, and truncated under the writer lock.
- [x] Transactions append and flush `Begin`, `ObjectsDurable`, `Commit`, and `Complete` records in order.
- [x] `HEAD` is never published before the durable `Commit` record.
- [x] Startup recovery leaves uncommitted transactions invisible.
- [x] Startup recovery publishes a valid proposed root after a durable commit.
- [x] Startup recovery recognizes an already-published root and completes its journal state.
- [x] Recovery stops rather than guessing when `HEAD` matches neither parent nor proposed root.
- [x] Recovery is repeatable and resolved repositories open without taking the writer lock.
- [x] Checkpoint records capture the resolved sequence prefix and current root.
- [x] Deterministic hooks simulate interruption after each commit stage.
- [x] Recovery tests prove each simulated interruption exposes the old or new root, never a partial tree.
- [x] Format, compile, strict Clippy, unit-test, child-process, and doc-test gates pass.

## Commit order

1. Acquire the exclusive writer lock and refresh `HEAD`.
2. Build and durably publish immutable objects.
3. Append and flush `Begin`.
4. Append and flush `ObjectsDurable`.
5. Append and flush `Commit`.
6. Atomically publish `HEAD` using the platform durability layer.
7. Append and flush `Complete`.
8. Release the writer lock.

## Verification

Verified on 2026-08-19 with Rust 1.97.1 and project MSRV 1.85:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace` — 24 workspace tests passed, including a child-process lock probe and five commit interruption stages

## Deliberate boundary

The journal currently uses its first append-only segment. Segment rotation, checkpoint-based pruning, and bounded retention will be added alongside garbage collection and repository maintenance. Deterministic fault hooks validate state-machine behavior; VM hard-reset and physical power-loss tests remain part of later corruption/recovery hardening because storage hardware may misreport completed flushes.

## Next target

Milestone 4 will add named snapshots, read-only historical roots, snapshot comparison, deletion, and restore-as-a-new-transaction.
