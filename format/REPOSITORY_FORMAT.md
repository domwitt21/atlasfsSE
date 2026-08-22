# AtlasFS Repository Layout

Status: accepted for the `v0.1` prototype. Repository format version: `1`.

```text
.atlasfs/
├── FORMAT
├── config
├── objects/ab/cdef...
├── refs/HEAD
├── refs/snapshots/<encoded-name>
├── journal/<first-sequence-hex>.log
├── index/
├── locks/writer.lock
└── tmp/
```

`FORMAT` is an ASCII line `atlasfs 1\n`. `config` uses a canonical key/value text format with keys sorted in byte order, LF endings, and no duplicate keys. Version 1 requires `chunk_size=65536`, records required and optional feature sets, and assigns a random repository UUID.

Object placement and encoding are defined in `OBJECT_FORMAT.md`. References contain exactly a 64-character lowercase hexadecimal root object ID followed by LF. Snapshot names are UTF-8 from 1 through 128 bytes and are percent-encoded for storage; reserved names and ambiguous encodings are rejected.

Temporary names contain a random transaction ID and are never interpreted as canonical state. On open, stale temporary files may be removed only after journal recovery identifies them as unreachable.

Repository initialization builds the complete directory structure in a sibling temporary directory, creates the empty directory/inode/root object closure, writes generation-zero `HEAD`, flushes it, and atomically renames the repository directory into place. Initialization fails if the destination already contains an AtlasFS repository.

The `index/` directory is disposable. The writer lock is advisory where the platform supports reliable advisory locks; failure to obtain it returns `RepositoryBusy`. Breaking a lock is never automatic. AtlasFS uses OS lock lifetime, not lock-file age, to determine ownership.
