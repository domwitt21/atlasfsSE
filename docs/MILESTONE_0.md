# Milestone 0 Acceptance Record

Milestone 0 establishes the contract that implementation will follow. It is complete when every item below is resolved and reviewed.

- [x] Workspace and initial crate boundaries are established.
- [x] Canonical state and rebuildable state are distinguished.
- [x] Repository directory layout and initialization behavior are specified.
- [x] Object kinds, identities, envelopes, and canonical codec are specified.
- [x] Chunk size and file-manifest invariants are selected.
- [x] Directory ordering and name constraints are selected.
- [x] Node identity, version ancestry, metadata, and timestamps are specified.
- [x] Path normalization and cross-platform comparison behavior are specified.
- [x] Permission behavior and deferred permission features are recorded.
- [x] Single-writer/multiple-reader locking behavior is specified.
- [x] Commit durability boundaries and recovery transitions are specified.
- [x] Stable error categories and adapter translation rules are specified.
- [x] Format compatibility and migration principles are recorded.
- [x] MVP inclusions and explicit deferrals are recorded in `PLAN.md`.
- [x] Compile, format, lint, and test the scaffold with the stable Rust toolchain.

## Verification

Verified on 2026-08-18 with Rust 1.97.1. The following commands passed for the complete workspace:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

Milestone 0 is complete. The next implementation target is repository initialization and canonical object envelopes in Milestone 1.
