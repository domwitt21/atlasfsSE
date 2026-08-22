# Milestone 7 — REST API

Milestone 7 adds the `atlasfs-api` crate and activates the `atlas server` command. The adapter is a bounded HTTP/1.1 server that routes repository behavior through `atlasfs-core`; it does not duplicate filesystem semantics.

## Starting the server

```powershell
$env:ATLASFS_TOKEN = "replace-with-a-long-random-secret"
atlas server C:\path\to\repository --bind 127.0.0.1:8787
```

The token can instead be supplied with `--token`. Request bodies default to a 64 MiB limit and can be configured with `--max-request-bytes`. The default concurrency limit is 64 connections. The server refuses an empty token.

AtlasFS serves plain HTTP and binds to loopback by default. For remote administration, place it behind a TLS-terminating reverse proxy and firewall the AtlasFS listener.

## Authentication and public endpoints

Every administration endpoint requires:

```text
Authorization: Bearer TOKEN
```

Token comparison is length-aware and constant-time. Two discovery endpoints are public:

- `GET /v1/health`
- `GET /openapi.json`

## Version 1 endpoints

Virtual paths and snapshot names are URL encoded.

- `GET|PUT|DELETE /v1/files?path=/virtual/path`
- `GET|POST /v1/directories?path=/virtual/path`
- `GET /v1/stat?path=/virtual/path`
- `GET /v1/history?path=/virtual/path`
- `GET /v1/usage`
- `POST /v1/verify`
- `GET|POST /v1/snapshots[?name=NAME]`
- `DELETE /v1/snapshots/NAME`
- `POST /v1/snapshots/NAME/restore`
- `GET /v1/snapshot-diff?left=NAME&right=NAME`

File request and response bodies use `application/octet-stream`. Transport reads and writes operate incrementally in bounded chunks. The core API currently materializes a file version for its atomic copy-on-write commit, while content is stored as independently deduplicated chunks.

## Safety and observability

- Headers are limited to 32 KiB.
- Bodies are rejected before reading when `Content-Length` exceeds the configured limit.
- Unsupported transfer encodings are rejected.
- Connections have a 30-second read timeout.
- Responses use structured JSON errors and `nosniff` protection.
- Every authenticated administration request and every failed authentication is appended to `journal/api-audit.jsonl`.
- Repository writer locking and journal recovery remain authoritative under concurrent API and CLI access.

The bundled OpenAPI 3.1 document describes the stable versioned surface and bearer security scheme.
