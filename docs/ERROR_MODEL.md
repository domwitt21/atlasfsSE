# AtlasFS Error Model

Core errors have stable machine-readable codes, structured context, and a human-readable message. Adapters translate them without inspecting message text.

Initial codes:

- `NotFound`, `AlreadyExists`, `NotDirectory`, `IsDirectory`, `DirectoryNotEmpty`
- `InvalidPath`, `InvalidName`, `PathTooLong`, `InvalidArgument`
- `PermissionDenied`, `ReadOnly`
- `RepositoryNotFound`, `RepositoryBusy`, `UnsupportedFormat`, `UnsupportedFeature`
- `ObjectMissing`, `ObjectCorrupt`, `ReferenceCorrupt`, `JournalCorrupt`
- `TransactionConflict`, `RecoveryConflict`
- `IntegrityFailure`, `ResourceLimit`, `Io`

Errors may attach a virtual path, object ID, transaction ID, journal sequence, or underlying I/O operation. They must not include file contents, credentials, bearer tokens, or unbounded repository-controlled strings.

CLI maps usage errors to exit `2`, absent paths to `3`, permission failures to `4`, repository integrity/recovery failures to `5`, conflicts/busy state to `6`, and other operational failures to `1`. Success is `0`. REST will map the same codes to documented HTTP statuses and include a request ID.
