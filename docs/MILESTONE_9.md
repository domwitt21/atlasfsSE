# Milestone 9 — Hardening and release

Milestone 9 completes the planned engineering sequence with strict format/config compatibility, bounded hostile-input handling, structured command diagnostics, rotating API audit logs, verified atomic backups, deterministic randomized version testing, pinned release inputs, cross-platform CI, a threat model, and operational/release runbooks.

The version 1 format fixture freezes both canonical bytes and its SHA-256 object identity. Future versions and required features fail closed. Backup output is itself a valid repository and includes a human-inspectable `BACKUP` manifest.

The project remains an educational storage engine rather than a replacement for a production encrypted, replicated filesystem. The threat model and runbook explicitly document that boundary.
