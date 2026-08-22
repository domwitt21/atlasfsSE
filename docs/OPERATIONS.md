# Operations and recovery runbook

## Routine checks

Run these commands under the same OS account that owns the repository:

```text
atlas verify REPOSITORY
atlas usage REPOSITORY
atlas gc REPOSITORY
atlas backup REPOSITORY BACKUP_DESTINATION
```

`gc` is a dry run unless `--execute` is supplied. Keep at least one tested backup outside the repository filesystem before executing collection.

Set `ATLASFS_TRACE=1` to emit one JSON command-completion event to stderr with command, exit code, and elapsed milliseconds. REST audit records are stored in `journal/api-audit.jsonl` and rotate to `.jsonl.1` at 16 MiB.

## Backup and restore

`atlas backup` acquires the writer lock, verifies all objects, creates an isolated staging tree, copies canonical state with bounded streaming I/O, writes a backup manifest, reopens the result, and atomically publishes it. The destination must not exist.

To restore, stop API/FUSE processes, preserve the damaged repository for investigation, and open the backup directly:

```text
atlas verify BACKUP_DESTINATION
atlas usage BACKUP_DESTINATION
```

Once verified, configure clients to use that directory or copy it to a new final location while AtlasFS is offline. Never merge object/reference directories manually.

## Suspected corruption

1. Stop writers, the REST server, and FUSE mounts.
2. Make a filesystem-level copy for forensic retention if storage permits.
3. Run `atlas verify` and record stderr plus `ATLASFS_TRACE=1` output.
4. Run `atlas recover` only on a retained copy or after securing a backup.
5. If an object is missing or corrupt, restore the entire repository from a verified backup. Do not invent replacement object bytes.

## Capacity and limits

- Object payload: 1 GiB
- CLI host-file import: 1 GiB
- Journal segment read: 256 MiB
- Journal record payload: 1 MiB
- REST headers: 32 KiB
- REST body default: 64 MiB, configurable
- REST concurrent connections default: 64
- REST audit log generation: 16 MiB plus one rotated file

Monitor both `physical_bytes` and `unreachable_bytes`. Snapshots and version history intentionally retain objects, so a low deduplication ratio or growing reachable storage is not necessarily a leak.
