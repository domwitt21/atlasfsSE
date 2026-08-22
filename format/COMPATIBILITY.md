# AtlasFS Format Compatibility

- Repository and object formats are explicitly versioned from the first prototype.
- Readers reject newer mandatory versions and unknown required feature flags.
- Index formats are not compatibility boundaries because indexes are rebuildable.
- Immutable objects are never rewritten in place during migration.
- A migration creates new objects and atomically publishes a new root only after validation.
- Downgrade behavior must be documented for every format change before release.

