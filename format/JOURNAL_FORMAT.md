# AtlasFS Journal and Recovery

Status: accepted for the `v0.1` prototype. Format version: `1`.

## Consistency model

The MVP supports one writer and multiple readers. Readers pin a committed root generation. A writer creates immutable objects privately and publishes exactly one new root. Readers therefore observe the complete old generation or complete new generation.

## Commit state machine

1. Acquire the repository writer lock.
2. Append a `Begin` record containing transaction ID and expected parent root.
3. Construct and stage all new immutable objects.
4. Flush each staged object, atomically publish it, and flush affected directories.
5. Append an `ObjectsDurable` record containing the proposed root ID and flush the journal.
6. Validate that the current root still equals the expected parent.
7. Append and flush a `Commit` record.
8. Write and flush a temporary root reference, atomically replace `refs/HEAD`, and flush `refs/`.
9. Append a `Complete` record. This final record is useful for cleanup but is not required for commit visibility.
10. Release the writer lock.

## Records

Every journal record contains eight-byte magic `ATLASJNL`, `u16` version, `u8` kind, one zero reserved byte, monotonically increasing `u64` sequence, 16-byte transaction ID, `u32` payload length, payload, and a 32-byte SHA-256 checksum over every preceding byte of that record. Initial record kinds are `Begin`, `ObjectsDurable`, `Commit`, `Complete`, `Abort`, and `Checkpoint`.

`Begin` contains the expected parent root and proposed generation. `ObjectsDurable` contains the proposed root. `Commit` repeats the parent and proposed root. `Complete` names the published root. `Abort` contains a stable reason code. `Checkpoint` contains the last fully resolved sequence and the current root.

Journal files are append-only segments named by their first sequence number as 16 lowercase hexadecimal digits. The initial implementation writes `0000000000000001.log`; rotation begins only when bounded retention is implemented. A segment is sealed before a new one begins. Recovery scans records in numeric order and rejects gaps, duplicate sequences, or checksum failure before additional data.

## Recovery rules

- A torn or checksum-invalid tail record is ignored after the final valid record boundary and reported.
- A transaction without a durable `Commit` leaves `HEAD` unchanged; its immutable objects may later be collected.
- If `Commit` is durable but `HEAD` still names the expected parent, recovery validates the proposed root and publishes it.
- If `HEAD` already names the proposed root, recovery treats the transaction as committed and may append `Complete`.
- If `HEAD` names neither expected parent nor proposed root, recovery stops with a conflict rather than guessing.
- Recovery operations are safe to repeat.

## Required fault tests

Inject termination before and after every append, file flush, directory flush, object rename, reference rename, and lock release. Each recovered repository must expose either the previous committed root or the proposed committed root and must never expose partial namespace state.

## State transition table

| Durable state | `HEAD` value | Recovery action |
| --- | --- | --- |
| No `Commit` | Expected parent | Leave `HEAD`; transaction is abandoned |
| `Commit` | Expected parent | Validate and publish proposed root |
| `Commit` | Proposed root | Treat as committed; append `Complete` when writable |
| `Complete` | Proposed root | No namespace action |
| Any transaction state | Other root | Stop with a recovery conflict |

Publishing is never attempted until the proposed root and its complete transitive object closure pass structural validation.
