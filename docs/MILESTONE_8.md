# Milestone 8 — Optional Linux FUSE integration

Milestone 8 adds the Linux-only `atlasfs-fuse` adapter and the cross-platform `atlas mount` command. Windows builds retain a small unsupported-platform stub, so FUSE remains optional and does not change the core storage format.

## Build and mount

Linux requires a kernel FUSE device and `fusermount3`. The crate uses `fuser`'s pure-Rust Linux mount path and does not require the libfuse development headers.

```sh
cargo build -p atlasfs-cli
mkdir /tmp/atlas-mount
target/debug/atlas mount /path/to/repository /tmp/atlas-mount
```

The foreground mount exits after `fusermount3 -u /tmp/atlas-mount`. Run `scripts/test-fuse.sh target/debug/atlas` to execute the disposable live-mount integration test.

## Operation mapping

- Lookup, metadata, directory reads, and file reads resolve through `VirtualFileSystem`.
- Create, `mkdir`, rename, unlink, and `rmdir` use the same atomic core transactions as the CLI and REST API.
- Kernel UID/GID values become AtlasFS permission actors. `DefaultPermissions` also enables kernel-side mode checks.
- Every open file gets a stable in-memory view. Writes are buffered per handle and create one immutable AtlasFS version on `flush`, `fsync`, or final release.
- Sparse writes and extension fill gaps with zero bytes. Truncation is implemented by the platform-neutral handle engine.
- Open handles follow file and ancestor-directory renames.
- Unlinked open handles remain readable until release; later dirty flushes are discarded instead of recreating the removed pathname.
- Hard links, symlinks, devices, extended attributes, advisory locking, and nonzero rename flags return unsupported errors because the core model does not define them yet.

The adapter deliberately contains no storage logic. Crash consistency, content addressing, permissions, snapshots, deduplication, and writer serialization remain owned by the existing core and store crates.

## Validation

The platform-neutral handle tests run on every supported host. Linux CI should run native compilation, strict Clippy, unit tests, and `scripts/test-fuse.sh`. The live test exercises kernel-mediated `mkdir`, create, write, `fsync`, read, rename, unlink, and `rmdir` operations.
