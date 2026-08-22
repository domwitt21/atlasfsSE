# Milestone 6 — Management CLI

Milestone 6 replaces the placeholder executable with `atlas`, a dependency-free command-line adapter over `atlasfs-core`.

## Invocation

```text
atlas [--json] COMMAND REPOSITORY [ARGS]
```

All virtual paths are strict absolute AtlasFS paths. Commands currently execute as the virtual root actor; multi-user authentication belongs at an adapter boundary in a later milestone.

## Commands

```text
atlas init REPOSITORY
atlas mkdir REPOSITORY VIRTUAL_PATH
atlas put REPOSITORY HOST_FILE VIRTUAL_PATH
atlas get REPOSITORY VIRTUAL_PATH HOST_FILE
atlas cat REPOSITORY VIRTUAL_PATH
atlas ls REPOSITORY VIRTUAL_PATH
atlas stat REPOSITORY VIRTUAL_PATH
atlas write REPOSITORY VIRTUAL_PATH TEXT
atlas rm REPOSITORY VIRTUAL_PATH [--recursive]
atlas mv REPOSITORY SOURCE DESTINATION
atlas chmod REPOSITORY VIRTUAL_PATH OCTAL_MODE
atlas history REPOSITORY VIRTUAL_PATH
atlas verify REPOSITORY
atlas recover REPOSITORY
atlas index REPOSITORY
atlas gc REPOSITORY [--execute]
atlas usage REPOSITORY
atlas snapshot REPOSITORY create NAME
atlas snapshot REPOSITORY list
atlas snapshot REPOSITORY delete NAME
atlas snapshot REPOSITORY restore NAME
atlas snapshot REPOSITORY diff LEFT RIGHT
atlas snapshot REPOSITORY cat NAME VIRTUAL_PATH
atlas server REPOSITORY
```

`put` creates or replaces a virtual file. `write` replaces an existing file with the supplied UTF-8 command argument. `cat` writes raw bytes in human mode and hexadecimal data in JSON mode. Garbage collection is a dry run unless `--execute` is explicitly supplied.

Milestone 7 activates the `server` entry point with bearer authentication and configurable bind and request limits.

## Output and exit codes

Human-readable output is the default. Placing `--json` before the command produces one JSON value for scripting, including hexadecimal content for binary-safe `cat` operations.

- `0`: success
- `2`: invalid command, arguments, path, or operation
- `3`: repository, object, snapshot, or virtual path not found
- `4`: already exists, busy writer, or transaction conflict
- `6`: repository or host I/O failure
- `7`: corruption or checksum failure
- `8`: virtual permission denied
