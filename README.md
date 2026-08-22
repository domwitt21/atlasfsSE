# AtlasFS

AtlasFS is a user-space, versioned virtual filesystem and snapshot engine built around immutable, SHA-256-addressed objects and copy-on-write roots.

> AtlasFS is experimental pre-1.0 storage software. Keep backups of important
> data and review the threat model before exposing any adapter remotely.

Milestones 0 through 9 are complete. AtlasFS now has repository initialization, canonical SHA-256 object storage, a permission-aware copy-on-write filesystem, checksummed recovery, named snapshots, rebuildable indexes, storage analytics, safe garbage collection, a scriptable CLI, an authenticated REST API, optional Linux FUSE mounting, verified backups, hostile-input limits, and a documented release/security posture. See `PLAN.md`, `docs/`, and `format/`.

Durable publication is platform-specific: Unix uses rename plus parent-directory flush, while Windows uses write-through `MoveFileExW` through an isolated platform crate.

## Quick start

Prerequisites: Git and the Rust toolchain pinned by `rust-toolchain.toml`.

```text
cargo build --workspace --locked
cargo test --workspace --locked
cargo run -p atlasfs-cli -- help
cargo run -p atlasfs-cli -- init ./demo.atlas
```

The REST server binds to `127.0.0.1:8787` by default and refuses to start
without a bearer token. Prefer `ATLASFS_TOKEN` over `--token`, because command
arguments may be visible to other local users. Use a TLS-terminating reverse
proxy and firewall for any remote deployment.

Version 0.1.0 satisfies the planned Milestones 0–9. See `docs/RELEASE.md` for
release builds, `SECURITY.md` for vulnerability reporting, and
`CONTRIBUTING.md` for the contribution workflow.

## License

AtlasFS is available under your choice of the MIT License or Apache License 2.0.
