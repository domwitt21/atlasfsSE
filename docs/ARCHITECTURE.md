# AtlasFS Architecture

## Boundaries

`atlasfs-store` owns canonical encoding, object identity, object placement, durable publication, chunk streaming, integrity verification, and later reachability-based collection. It knows object structure but not user authorization or path commands.

`atlasfs-platform` owns the smallest possible operating-system durability boundary. Unix publication renames and flushes the destination directory. Windows publication uses `MoveFileExW` with `MOVEFILE_WRITE_THROUGH` and optional `MOVEFILE_REPLACE_EXISTING`. This is the only crate permitted to contain the narrowly reviewed Windows FFI call; storage and core continue to forbid unsafe code.

`atlasfs-core` owns paths, namespace traversal, inode/version construction, permissions, transactions, root generations, snapshots, history, and filesystem errors. It depends on storage through a narrow repository interface.

`atlasfs-cli` is an adapter. It parses commands, invokes core services, and renders stable human or JSON results. It contains no filesystem rules.

Later, `atlasfs-server` and `atlasfs-fuse` will be adapters over the same core service. `atlasfs-bench` will generate repeatable workloads without introducing production behavior.

## Dependency rule

Dependencies point inward: adapters depend on core; core depends on storage abstractions; storage depends on the minimal platform durability layer and general-purpose libraries. Core and storage never depend on CLI, HTTP, or FUSE types.

## Canonical and derived state

Canonical state consists of the format marker, immutable objects, journal, and references. Indexes, caches, reports, lock files, and temporary files are operational or derived state. Deleting all derived state must not change the visible committed filesystem after rebuilding.

## Concurrency model

Version 1 permits one writer per repository through an exclusive lock. Readers resolve `HEAD` once when opening a read transaction and retain that root ID for consistent traversal. Because reachable objects are immutable, readers need no object-level locks.

Garbage collection takes the writer lock, records all roots pinned by active in-process readers, and never sweeps an object reachable from `HEAD`, snapshots, journal recovery candidates, or pinned roots. Cross-process reader pinning is deferred; therefore the MVP CLI performs offline GC while no other process has the repository open.

## Durability contract

With durable mode enabled, a successful mutation means objects, commit record, and `HEAD` have crossed their required flush boundaries. AtlasFS documents platforms where directory durability cannot be guaranteed. A fast mode may be added later but cannot be the default and must never claim crash durability.

## Trust boundaries

All repository bytes are untrusted input. Lengths, counts, object kinds, hashes, paths, journal sequences, and graph references are validated. API and FUSE inputs pass through the same normalized path and permission logic as CLI inputs.
