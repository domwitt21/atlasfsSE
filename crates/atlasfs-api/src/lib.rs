#![doc = "Bounded HTTP/1.1 administration adapter for `AtlasFS`."]

use atlasfs_core::{Actor, DiffKind, Error, VirtualFileSystem, VirtualPath};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_HEADERS: usize = 32 * 1024;
static AUDIT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub repository: PathBuf,
    pub bind: SocketAddr,
    pub bearer_token: String,
    pub max_request_bytes: usize,
    pub max_connections: usize,
    pub audit_log: PathBuf,
    pub max_audit_bytes: u64,
}

impl ServerConfig {
    #[must_use]
    pub fn new(repository: impl AsRef<Path>, bind: SocketAddr, bearer_token: String) -> Self {
        let repository = repository.as_ref().to_path_buf();
        Self {
            audit_log: repository.join("journal/api-audit.jsonl"),
            repository,
            bind,
            bearer_token,
            max_request_bytes: 64 * 1024 * 1024,
            max_connections: 64,
            max_audit_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Runs the REST server until the process is stopped.
/// # Errors
/// Returns an error if configuration is unsafe or the listener cannot run.
pub fn serve(config: ServerConfig) -> std::io::Result<()> {
    if config.bearer_token.is_empty()
        || config.max_request_bytes == 0
        || config.max_connections == 0
        || config.max_audit_bytes == 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "token and server limits must be nonzero",
        ));
    }
    VirtualFileSystem::open(&config.repository)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let listener = TcpListener::bind(config.bind)?;
    let config = Arc::new(config);
    let active = Arc::new(AtomicUsize::new(0));
    for connection in listener.incoming() {
        let mut stream = connection?;
        if active.load(Ordering::Acquire) >= config.max_connections {
            write_response(
                &mut stream,
                Response::json(503, "{\"error\":\"server_busy\"}"),
            )?;
            continue;
        }
        active.fetch_add(1, Ordering::AcqRel);
        let config = Arc::clone(&config);
        let active = Arc::clone(&active);
        thread::spawn(move || {
            let _guard = ActiveGuard(active);
            let peer = stream.peer_addr().ok();
            let response = read_request(&mut stream, config.max_request_bytes)
                .map_or_else(|error| error, |request| route(&config, peer, &request));
            let _ = write_response(&mut stream, response);
        });
    }
    Ok(())
}

struct ActiveGuard(Arc<AtomicUsize>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct Request {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug)]
struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    extra_headers: Vec<(&'static str, String)>,
}

impl Response {
    fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: body.as_bytes().to_vec(),
            extra_headers: Vec::new(),
        }
    }
    fn bytes(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: "application/octet-stream",
            body,
            extra_headers: Vec::new(),
        }
    }
}

fn read_request(stream: &mut TcpStream, maximum: usize) -> Result<Request, Response> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .map_err(|_| problem(408, "request_timeout", "could not set request timeout"))?;
    let mut received = Vec::new();
    let mut buffer = [0_u8; 8192];
    let header_end = loop {
        let count = stream
            .read(&mut buffer)
            .map_err(|_| problem(400, "invalid_request", "failed to read request"))?;
        if count == 0 {
            return Err(problem(
                400,
                "invalid_request",
                "request ended before headers",
            ));
        }
        received.extend_from_slice(&buffer[..count]);
        if let Some(index) = find_bytes(&received, b"\r\n\r\n") {
            break index + 4;
        }
        if received.len() > MAX_HEADERS {
            return Err(problem(
                431,
                "headers_too_large",
                "request headers exceed limit",
            ));
        }
    };
    if header_end > MAX_HEADERS {
        return Err(problem(
            431,
            "headers_too_large",
            "request headers exceed limit",
        ));
    }
    let header = std::str::from_utf8(&received[..header_end - 4])
        .map_err(|_| problem(400, "invalid_request", "headers are not UTF-8"))?;
    let mut lines = header.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| problem(400, "invalid_request", "missing request line"))?;
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    if method.is_empty() || target.is_empty() || version != "HTTP/1.1" || parts.next().is_some() {
        return Err(problem(
            400,
            "invalid_request",
            "invalid HTTP/1.1 request line",
        ));
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| problem(400, "invalid_request", "malformed header"))?;
        let name = name.trim().to_ascii_lowercase();
        if headers.insert(name, value.trim().to_owned()).is_some() {
            return Err(problem(400, "invalid_request", "duplicate header"));
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err(problem(
            501,
            "unsupported_encoding",
            "transfer-encoding is unsupported",
        ));
    }
    let length = headers
        .get("content-length")
        .map_or(Ok(0), |value| value.parse::<usize>())
        .map_err(|_| problem(400, "invalid_request", "invalid content-length"))?;
    if length > maximum {
        return Err(problem(
            413,
            "request_too_large",
            "request body exceeds configured limit",
        ));
    }
    let mut body = received[header_end..].to_vec();
    if body.len() > length {
        body.truncate(length);
    }
    while body.len() < length {
        let wanted = (length - body.len()).min(buffer.len());
        let count = stream
            .read(&mut buffer[..wanted])
            .map_err(|_| problem(400, "invalid_request", "failed to read request body"))?;
        if count == 0 {
            return Err(problem(400, "invalid_request", "truncated request body"));
        }
        body.extend_from_slice(&buffer[..count]);
    }
    Ok(Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body,
    })
}

#[allow(clippy::too_many_lines)]
fn route(config: &ServerConfig, peer: Option<SocketAddr>, request: &Request) -> Response {
    let (path, query) = split_target(&request.target);
    if path == "/openapi.json" && request.method == "GET" {
        return Response::json(200, OPENAPI);
    }
    if path == "/v1/health" && request.method == "GET" {
        return Response::json(200, "{\"status\":\"ok\",\"version\":1}");
    }
    if !authorized(request, &config.bearer_token) {
        let mut response = problem(401, "unauthorized", "valid bearer token required");
        response
            .extra_headers
            .push(("WWW-Authenticate", "Bearer".to_owned()));
        audit(config, peer, request, response.status);
        return response;
    }
    let actor = Actor::root();
    let result = (|| -> Result<Response, Error> {
        match (request.method.as_str(), path) {
            ("GET", "/v1/files") => {
                let path = query_path(query)?;
                Ok(Response::bytes(
                    VirtualFileSystem::open(&config.repository)?.read_file(&path, &actor)?,
                ))
            }
            ("PUT", "/v1/files") => {
                let path = query_path(query)?;
                let mut vfs = VirtualFileSystem::open(&config.repository)?;
                let id = match vfs.create_file(&path, &request.body, 0o644, &actor) {
                    Ok(id) => id,
                    Err(Error::AlreadyExists(_)) => vfs.write_file(&path, &request.body, &actor)?,
                    Err(error) => return Err(error),
                };
                Ok(Response::json(
                    200,
                    &format!("{{\"object\":\"{id}\",\"bytes\":{}}}", request.body.len()),
                ))
            }
            ("DELETE", "/v1/files") => {
                let mut vfs = VirtualFileSystem::open(&config.repository)?;
                vfs.remove(&query_path(query)?, &actor)?;
                Ok(Response::json(200, "{\"status\":\"removed\"}"))
            }
            ("GET", "/v1/directories") => {
                let entries = VirtualFileSystem::open(&config.repository)?
                    .list(&query_path(query)?, &actor)?;
                Ok(Response::json(
                    200,
                    &format!("{{\"entries\":[{}]}}", json_strings(&entries)),
                ))
            }
            ("POST", "/v1/directories") => {
                let mut vfs = VirtualFileSystem::open(&config.repository)?;
                let id = vfs.create_directory(&query_path(query)?, 0o755, &actor)?;
                Ok(Response::json(201, &format!("{{\"object\":\"{id}\"}}")))
            }
            ("GET", "/v1/stat") => {
                let stat = VirtualFileSystem::open(&config.repository)?
                    .stat(&query_path(query)?, &actor)?;
                Ok(Response::json(
                    200,
                    &format!(
                        "{{\"object\":\"{}\",\"kind\":\"{}\",\"mode\":\"{:04o}\",\"owner\":{},\"group\":{},\"size\":{}}}",
                        stat.object_id,
                        format!("{:?}", stat.kind).to_lowercase(),
                        stat.mode,
                        stat.owner,
                        stat.group,
                        stat.size
                    ),
                ))
            }
            ("GET", "/v1/history") => {
                let versions: Vec<_> = VirtualFileSystem::open(&config.repository)?
                    .history(&query_path(query)?, &actor)?
                    .into_iter()
                    .map(|entry| entry.object_id.to_string())
                    .collect();
                Ok(Response::json(
                    200,
                    &format!("{{\"versions\":[{}]}}", json_strings(&versions)),
                ))
            }
            ("GET", "/v1/usage") => {
                let value = VirtualFileSystem::open(&config.repository)?.storage_usage()?;
                Ok(Response::json(
                    200,
                    &format!(
                        "{{\"logical_bytes\":{},\"physical_bytes\":{},\"reachable_bytes\":{},\"unreachable_bytes\":{},\"metadata_bytes\":{},\"unique_chunks\":{},\"deduplication_ratio\":{:.3},\"snapshots\":{}}}",
                        value.logical_bytes,
                        value.physical_bytes,
                        value.reachable_bytes,
                        value.unreachable_bytes,
                        value.metadata_bytes,
                        value.unique_chunks,
                        value.deduplication_ratio,
                        value.snapshot_count
                    ),
                ))
            }
            ("POST", "/v1/verify") => {
                let report = VirtualFileSystem::open(&config.repository)?.verify()?;
                Ok(Response::json(
                    200,
                    &format!(
                        "{{\"objects_checked\":{},\"bytes_checked\":{}}}",
                        report.objects_checked, report.bytes_checked
                    ),
                ))
            }
            ("GET", "/v1/snapshots") => {
                let names: Vec<_> = VirtualFileSystem::open(&config.repository)?
                    .snapshots()?
                    .into_iter()
                    .map(|item| item.name)
                    .collect();
                Ok(Response::json(
                    200,
                    &format!("{{\"snapshots\":[{}]}}", json_strings(&names)),
                ))
            }
            ("POST", "/v1/snapshots") => {
                let name = query_value(query, "name")
                    .ok_or(Error::InvalidOperation("missing name query parameter"))?;
                let info = VirtualFileSystem::open(&config.repository)?.create_snapshot(&name)?;
                Ok(Response::json(
                    201,
                    &format!(
                        "{{\"name\":\"{}\",\"object\":\"{}\",\"root\":\"{}\"}}",
                        escape(&info.name),
                        info.object,
                        info.root
                    ),
                ))
            }
            ("DELETE", value) if value.starts_with("/v1/snapshots/") => {
                let name = percent_decode(&value[14..])
                    .map_err(|()| Error::InvalidOperation("invalid snapshot name encoding"))?;
                VirtualFileSystem::open(&config.repository)?.delete_snapshot(&name)?;
                Ok(Response::json(200, "{\"status\":\"snapshot deleted\"}"))
            }
            ("POST", value)
                if value.starts_with("/v1/snapshots/") && value.ends_with("/restore") =>
            {
                let encoded = &value[14..value.len() - 8];
                let name = percent_decode(encoded)
                    .map_err(|()| Error::InvalidOperation("invalid snapshot name encoding"))?;
                VirtualFileSystem::open(&config.repository)?.restore_snapshot(&name)?;
                Ok(Response::json(200, "{\"status\":\"snapshot restored\"}"))
            }
            ("GET", "/v1/snapshot-diff") => {
                let left = query_value(query, "left")
                    .ok_or(Error::InvalidOperation("missing left query parameter"))?;
                let right = query_value(query, "right")
                    .ok_or(Error::InvalidOperation("missing right query parameter"))?;
                let changes: Vec<_> = VirtualFileSystem::open(&config.repository)?
                    .diff_snapshots(&left, &right)?
                    .into_iter()
                    .map(|entry| format!("{}:{}", diff_name(entry.kind), entry.path))
                    .collect();
                Ok(Response::json(
                    200,
                    &format!("{{\"changes\":[{}]}}", json_strings(&changes)),
                ))
            }
            _ => Ok(problem(404, "endpoint_not_found", "unknown API endpoint")),
        }
    })();
    let response = result.unwrap_or_else(core_problem);
    audit(config, peer, request, response.status);
    response
}

fn query_path(query: &str) -> Result<VirtualPath, Error> {
    let value = query_value(query, "path")
        .ok_or(Error::InvalidOperation("missing path query parameter"))?;
    VirtualPath::parse(&value)
}

fn split_target(target: &str) -> (&str, &str) {
    target.split_once('?').unwrap_or((target, ""))
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| percent_decode(value).ok()).flatten()
    })
}

fn percent_decode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let high = hex_digit(bytes[index + 1])?;
                let low = hex_digit(bytes[index + 2])?;
                output.push(high * 16 + low);
                index += 3;
            }
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' => return Err(()),
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).map_err(|_| ())
}

fn hex_digit(value: u8) -> Result<u8, ()> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(()),
    }
}

fn authorized(request: &Request, token: &str) -> bool {
    request.headers.get("authorization").is_some_and(|value| {
        value
            .strip_prefix("Bearer ")
            .is_some_and(|provided| constant_time_eq(provided.as_bytes(), token.as_bytes()))
    })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

#[allow(clippy::needless_pass_by_value)]
fn core_problem(error: Error) -> Response {
    let (status, code) = match error {
        Error::NotFound(_) => (404, "not_found"),
        Error::AlreadyExists(_) => (409, "already_exists"),
        Error::PermissionDenied(_) => (403, "permission_denied"),
        Error::InvalidPath(_) | Error::InvalidOperation(_) => (400, "invalid_request"),
        Error::Store(ref source) if source.to_string().to_lowercase().contains("busy") => {
            (409, "repository_busy")
        }
        Error::Store(ref source) if source.to_string().to_lowercase().contains("corrupt") => {
            (500, "corruption_detected")
        }
        _ => (500, "repository_error"),
    };
    problem(status, code, &error.to_string())
}

fn problem(status: u16, code: &str, message: &str) -> Response {
    Response::json(
        status,
        &format!(
            "{{\"error\":\"{}\",\"message\":\"{}\"}}",
            escape(code),
            escape(message)
        ),
    )
}

fn write_response(stream: &mut TcpStream, response: Response) -> std::io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    )?;
    for (name, value) in response.extra_headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    for chunk in response.body.chunks(64 * 1024) {
        stream.write_all(chunk)?;
    }
    stream.flush()
}

fn audit(config: &ServerConfig, peer: Option<SocketAddr>, request: &Request, status: u16) {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs());
    let record = format!(
        "{{\"timestamp\":{timestamp},\"peer\":\"{}\",\"method\":\"{}\",\"target\":\"{}\",\"status\":{status},\"bytes\":{}}}\n",
        peer.map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
        escape(&request.method),
        escape(&request.target),
        request.body.len()
    );
    let Ok(_guard) = AUDIT_LOCK.lock() else {
        return;
    };
    if fs::metadata(&config.audit_log).is_ok_and(|value| value.len() >= config.max_audit_bytes) {
        let rotated = config.audit_log.with_extension("jsonl.1");
        if rotated.exists() {
            let _ = fs::remove_file(&rotated);
        }
        let _ = fs::rename(&config.audit_log, rotated);
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.audit_log)
    {
        let _ = file.write_all(record.as_bytes());
        let _ = file.flush();
    }
}

fn json_strings(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("\"{}\"", escape(value)))
        .collect::<Vec<_>>()
        .join(",")
}
fn escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect(),
            '\n' => "\\n".chars().collect(),
            value if value.is_control() => format!("\\u{:04x}", value as u32).chars().collect(),
            value => vec![value],
        })
        .collect()
}
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
fn diff_name(kind: DiffKind) -> &'static str {
    match kind {
        DiffKind::Added => "added",
        DiffKind::Removed => "removed",
        DiffKind::ContentModified => "content-modified",
        DiffKind::MetadataModified => "metadata-modified",
        DiffKind::TypeChanged => "type-changed",
    }
}

pub const OPENAPI: &str = include_str!("openapi.json");

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn request(method: &str, target: &str, body: &[u8], authorized: bool) -> Request {
        let mut headers = BTreeMap::new();
        if authorized {
            headers.insert("authorization".to_owned(), "Bearer secret".to_owned());
        }
        Request {
            method: method.to_owned(),
            target: target.to_owned(),
            headers,
            body: body.to_vec(),
        }
    }
    #[test]
    fn decoding_and_authentication_are_strict() {
        assert_eq!(percent_decode("%2Fdocs%2Fa+b").unwrap(), "/docs/a b");
        assert!(percent_decode("%zz").is_err());
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secrex"));
        assert!(!constant_time_eq(b"secret", b"secret-long"));
    }
    #[test]
    fn openapi_is_versioned() {
        assert!(OPENAPI.contains("/v1/files"));
        assert!(OPENAPI.contains("bearerAuth"));
    }

    #[test]
    fn routes_authenticate_and_use_core_services() {
        let repository = std::env::temp_dir().join(format!(
            "atlas-api-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        VirtualFileSystem::init(&repository).unwrap();
        let config = ServerConfig::new(
            &repository,
            "127.0.0.1:0".parse().unwrap(),
            "secret".to_owned(),
        );

        assert_eq!(
            route(&config, None, &request("GET", "/v1/usage", &[], false)).status,
            401
        );
        assert_eq!(
            route(
                &config,
                None,
                &request("PUT", "/v1/files?path=%2Fhello", b"hello", true)
            )
            .status,
            200
        );
        let file = route(
            &config,
            None,
            &request("GET", "/v1/files?path=%2Fhello", &[], true),
        );
        assert_eq!(file.status, 200);
        assert_eq!(file.body, b"hello");
        assert_eq!(
            route(
                &config,
                None,
                &request("POST", "/v1/snapshots?name=first", &[], true)
            )
            .status,
            201
        );
        assert!(config.audit_log.is_file());
        fs::remove_dir_all(repository).unwrap();
    }
}
