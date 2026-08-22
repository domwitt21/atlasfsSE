# Platform Durability

AtlasFS routes every canonical publication through `atlasfs-platform::publish` after the source file has been fully written and flushed.

## Unix

Publication uses a same-filesystem rename followed by `sync_all` on the destination's parent directory. A non-replacing publication checks for an existing destination before rename. AtlasFS uses immutable, content-addressed destinations, so a race is resolved by verifying the winning object.

## Windows

Publication uses the documented `MoveFileExW` API with `MOVEFILE_WRITE_THROUGH`. Replacing a reference adds `MOVEFILE_REPLACE_EXISTING`; immutable object publication omits that flag. UTF-16 paths are checked for embedded NUL and NUL-terminated before the call.

The Windows call is the only unsafe block in the workspace. It lives in a dedicated crate that denies unsafe operations inside unsafe functions. The source and destination buffers remain alive for the complete call, and only documented flags are supplied.

## Other platforms

Unsupported platforms return an explicit `Unsupported` I/O error. AtlasFS does not silently advertise durable commits when it has no implemented publication primitive.

## Limits

Durability ultimately depends on the filesystem, storage controller, device firmware, and virtual-machine host honoring flush requests. AtlasFS can enforce correct API ordering but cannot compensate for hardware that falsely reports persistence. Crash-injection and VM hard-reset testing remain part of the transaction milestone.
