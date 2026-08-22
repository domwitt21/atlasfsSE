# AtlasFS threat model

## Security objectives

AtlasFS protects repository integrity and atomicity against accidental corruption, interrupted writes, malformed stored objects, hostile API requests, and untrusted virtual filesystem users constrained by the AtlasFS permission model. SHA-256 identities detect modification; they do not provide secrecy or prove who created data.

## Trust boundaries

- The host operating system, administrator, repository owner, executable, and Rust runtime are trusted.
- Virtual file paths, imported files, REST requests, FUSE requests, repository objects, references, configuration, journal bytes, and backup media are untrusted inputs.
- REST bearer tokens are secrets. CLI arguments may be visible to other local users, so `ATLASFS_TOKEN` is preferred to `--token`.
- TLS is outside the embedded HTTP server. Remote deployments must use a trusted TLS reverse proxy and restrict the listener with a firewall.

## Defenses

- Canonical decoders reject unknown versions/kinds, oversized payloads, overflow, trailing data, invalid ordering, and broken cross-object invariants.
- Repository configuration rejects duplicate, reordered, missing, unknown-required, and malformed values.
- Objects are rehashed on every read and before backup; references and recovery roots are type checked.
- Single-writer locking, immutable objects, atomic reference replacement, checksummed journaling, and parent-directory flushes protect commit atomicity.
- Journal segments are bounded before allocation; object payloads, paths, names, HTTP headers/bodies, connections, CLI imports, and audit logs have explicit limits.
- REST administration requires a nonempty bearer token, compares it without early exit, and emits bounded audit logs.
- FUSE maps kernel identities into the core permission model and enables kernel default-permission checks.
- Garbage collection validates the full reachable graph before deleting anything and requires an explicit execute flag.
- Backups are verified, copied into a staging directory, validated by reopening, and atomically published to a previously absent destination.

## Out of scope and residual risks

- AtlasFS does not encrypt data at rest or backups. Use encrypted volumes and encrypted transport.
- A malicious host administrator can replace binaries, read tokens, alter memory, or delete repositories.
- SHA-256 is integrity addressing, not a MAC or digital signature.
- Denial of service remains possible through allowed large files, many valid objects, expensive verification, disk exhaustion, or many sequential authenticated operations.
- The embedded server is HTTP/1.1 without TLS, HTTP/2, chunked transfer encoding, or distributed rate limiting.
- FUSE does not implement hard links, symlinks, special files, ACLs, extended attributes, or advisory locks.
- Hardware or hypervisors that falsely acknowledge durable flushes can violate crash guarantees.
- Backups are point-in-time copies, not replication; operators must copy them off-host and test restores.

Report suspected vulnerabilities privately to the repository owner before public disclosure.
