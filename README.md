# AtlasFS

AtlasFS is a user-space, versioned virtual filesystem and snapshot engine built around immutable, SHA-256-addressed objects and copy-on-write roots.

Milestones 0 through 9 are complete. AtlasFS now has repository initialization, canonical SHA-256 object storage, a permission-aware copy-on-write filesystem, checksummed recovery, named snapshots, rebuildable indexes, storage analytics, safe garbage collection, a scriptable CLI, an authenticated REST API, optional Linux FUSE mounting, verified backups, hostile-input limits, and a documented release/security posture. See `PLAN.md`, `docs/`, and `format/`.

Durable publication is platform-specific: Unix uses rename plus parent-directory flush, while Windows uses write-through `MoveFileExW` through an isolated platform crate.

Run `cargo run -p atlasfs-cli -- help` for the command overview. Version 0.1.0 now satisfies the planned Milestones 0–9; follow `docs/RELEASE.md` for release builds.
