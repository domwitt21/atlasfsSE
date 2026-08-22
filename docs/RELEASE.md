# Release process

## Compatibility policy

Repository and object format version 1 is stable for the 0.1 release line. Readers reject future versions and unknown required features rather than guessing. Optional features may be ignored only when the canonical format declares them optional. Any incompatible encoding change requires a new format version and an explicit offline migration tool.

## Reproducible inputs

- Rust is pinned by `rust-toolchain.toml`.
- Dependencies are pinned by `Cargo.lock`; release commands use `--locked`.
- The release profile fixes optimization, one codegen unit, fat LTO, panic behavior, and symbol stripping.
- Build scripts generate a SHA-256 checksum beside the executable.

Rust binaries can still differ across operating systems, architectures, linkers, and source paths. Reproducibility means rebuilding from the same commit, lockfile, toolchain, target, linker, and environment. Record those inputs and set a consistent `SOURCE_DATE_EPOCH` in packaging environments.

## Checklist

1. Confirm the worktree contains only intended release changes.
2. Run `scripts/build-release.ps1` on Windows and `scripts/build-release.sh` on Linux.
3. Run the Linux live FUSE test when `/dev/fuse` is available.
4. Initialize a repository with the release binary and exercise CLI plus authenticated REST smoke tests.
5. Create a backup, verify it, and test opening/restoring it.
6. Compare artifact checksums from equivalent clean builders.
7. Review `THREAT_MODEL.md`, dependency advisories, and operational limits.
8. Tag the exact tested commit and publish the binaries, checksums, license files, changelog, and format documentation together.

## Rollback

Do not downgrade a repository after a newer binary has written a new format or required feature. For version 1, stop all adapters and point the previous compatible binary at the last verified pre-upgrade backup.
