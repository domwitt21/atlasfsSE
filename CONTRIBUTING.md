# Contributing to AtlasFS

Thank you for helping improve AtlasFS. Small, focused changes with tests and
clear compatibility reasoning are easiest to review.

## Development workflow

1. Create a branch from `main`.
2. Keep canonical storage encodings deterministic and versioned.
3. Add or update tests for behavior changes, corruption cases, and limits.
4. Run the full quality gate:

   ```text
   cargo fmt --all -- --check
   cargo check --workspace --all-targets --locked
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```

5. Update format or operational documentation when behavior changes.
6. Open a pull request describing the problem, design, tests, and compatibility
   impact.

## Design expectations

- Treat repository bytes, paths, REST input, and FUSE input as untrusted.
- Preserve atomic publication and crash-recovery invariants.
- Put authorization and platform behavior at adapter boundaries.
- Avoid incompatible on-disk changes without a new format version and migration
  strategy.
- Never commit credentials, bearer tokens, private keys, repositories, backups,
  or generated release artifacts.

By submitting a contribution, you agree that it is licensed under the project's
MIT OR Apache-2.0 terms.
