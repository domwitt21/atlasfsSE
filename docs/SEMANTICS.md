# AtlasFS Filesystem Semantics

## Paths

Internal paths are absolute UTF-8 paths rooted at `/`. Repeated separators are rejected rather than silently normalized. `.` and `..` components are rejected. A component cannot be empty, exceed 255 UTF-8 bytes, contain NUL, `/`, or `\\`, or equal `.` or `..`. A complete encoded path cannot exceed 4096 bytes.

Names are compared by exact UTF-8 bytes. Version 1 performs no Unicode normalization and is case-sensitive on every host, ensuring repositories behave consistently across platforms. CLI input using host paths is converted explicitly and never inherits host case-folding rules.

## Node identity and versions

Every logical node has a random 128-bit inode ID. Each mutation creates a new immutable inode/version object that retains the logical inode ID and names its prior version. Renaming a node changes the containing directories but does not itself change the node version. Metadata or content changes create a node version and new versions of ancestor directories through the root.

Hard links, symbolic links, devices, sockets, and sparse extents are outside the version 1 MVP. A directory cannot be moved into itself or one of its descendants.

## Metadata and time

Nodes store creation, modification, metadata-change, and access timestamps. The MVP does not update access time on reads, avoiding write amplification; it reports the stored access time. Mutations obtain one transaction timestamp and use it consistently for all objects created by that operation. Clock regression is allowed because generation, not time, defines version order.

## Permissions

Mode bits use the POSIX owner/group/other read, write, and execute model. Owner and group are unsigned 32-bit numeric IDs. Directory execute permits traversal, directory read permits listing, and directory write plus execute permits entry mutation. Root identity `0` bypasses access checks but structural validation still applies. Setuid, setgid behavior, sticky directories, ACLs, and identity-provider integration are deferred.

Permission checks use the actor identity supplied to the core operation, never ambient process identity inside storage code. New nodes receive an explicitly supplied mode after applying the caller-side creation mask.

## Operations

- Create fails if the destination exists or its parent is absent/not a directory.
- Read and list operate against the root pinned at transaction start.
- Write replaces the requested byte range and creates a complete new file manifest.
- Rename within a repository is atomic and may replace a file with a file or an empty directory with a directory, subject to permissions.
- Removing a directory requires it to be empty unless the caller explicitly requests recursive removal.
- Restore commits a new root whose tree equals the selected snapshot; it never rewinds generation or deletes later objects.

## Ordering

Directory listings and tree diffs are returned in ascending UTF-8 byte order. History is ordered by descending root generation. Timestamps are display metadata and never used to break ordering ties.
