use atlasfs_core::{Actor, DiffKind, Error, VirtualFileSystem, VirtualPath};
use std::time::Instant;
use std::{env, fs, io::Write, process::ExitCode};

const EXIT_USAGE: u8 = 2;
const EXIT_NOT_FOUND: u8 = 3;
const EXIT_CONFLICT: u8 = 4;
const EXIT_IO: u8 = 6;
const EXIT_CORRUPT: u8 = 7;
const EXIT_PERMISSION: u8 = 8;
const MAX_IMPORT_BYTES: u64 = 1024 * 1024 * 1024;

fn main() -> ExitCode {
    let arguments: Vec<_> = env::args().skip(1).collect();
    let started = Instant::now();
    let command = arguments
        .iter()
        .find(|value| value.as_str() != "--json")
        .map_or("unknown", String::as_str);
    match execute(&arguments) {
        Ok(output) => {
            if let Err(error) = std::io::stdout().write_all(&output) {
                eprintln!("atlas: {error}");
                return ExitCode::from(EXIT_IO);
            }
            trace(command, 0, started.elapsed().as_millis());
            ExitCode::SUCCESS
        }
        Err(failure) => {
            if arguments.first().is_some_and(|value| value == "--json") {
                eprintln!(
                    "{{\"error\":\"{}\",\"exit_code\":{}}}",
                    escape(&failure.message),
                    failure.code
                );
            } else {
                eprintln!("atlas: {}", failure.message);
            }
            trace(command, failure.code, started.elapsed().as_millis());
            ExitCode::from(failure.code)
        }
    }
}

#[derive(Debug)]
struct Failure {
    code: u8,
    message: String,
}

impl Failure {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_USAGE,
            message: message.into(),
        }
    }
    #[allow(clippy::needless_pass_by_value)]
    fn io(action: &str, error: std::io::Error) -> Self {
        Self {
            code: EXIT_IO,
            message: format!("{action}: {error}"),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn execute(arguments: &[String]) -> Result<Vec<u8>, Failure> {
    let (json, arguments) = if arguments.first().is_some_and(|value| value == "--json") {
        (true, &arguments[1..])
    } else {
        (false, arguments)
    };
    let command = required(arguments, 0, "missing command")?;
    if matches!(command, "help" | "--help" | "-h") {
        return Ok(help().as_bytes().to_vec());
    }
    if command == "version" || command == "--version" {
        return Ok(format!(
            "atlas {} format={}\n",
            env!("CARGO_PKG_VERSION"),
            atlasfs_core::OBJECT_FORMAT_VERSION
        )
        .into_bytes());
    }
    let repository = required(arguments, 1, "missing repository path")?;
    let actor = Actor::root();
    let text = match command {
        "init" => {
            exact(arguments, 2)?;
            let vfs = VirtualFileSystem::init(repository).map_err(map_core)?;
            object(
                json,
                &[
                    ("repository", repository.to_owned()),
                    ("head", vfs.head().to_string()),
                ],
            )
        }
        "mkdir" => {
            exact(arguments, 3)?;
            let mut vfs = open(repository)?;
            let id = vfs
                .create_directory(&virtual_path(&arguments[2])?, 0o755, &actor)
                .map_err(map_core)?;
            object(json, &[("object", id.to_string())])
        }
        "put" => {
            exact(arguments, 4)?;
            let size = fs::metadata(&arguments[2])
                .map_err(|error| Failure::io("inspect host file", error))?
                .len();
            if size > MAX_IMPORT_BYTES {
                return Err(Failure::usage(format!(
                    "host file is {size} bytes; maximum import size is {MAX_IMPORT_BYTES}"
                )));
            }
            let data =
                fs::read(&arguments[2]).map_err(|error| Failure::io("read host file", error))?;
            let mut vfs = open(repository)?;
            let path = virtual_path(&arguments[3])?;
            let id = match vfs.create_file(&path, &data, 0o644, &actor) {
                Ok(id) => id,
                Err(Error::AlreadyExists(_)) => {
                    vfs.write_file(&path, &data, &actor).map_err(map_core)?
                }
                Err(error) => return Err(map_core(error)),
            };
            object(
                json,
                &[
                    ("object", id.to_string()),
                    ("bytes", data.len().to_string()),
                ],
            )
        }
        "get" => {
            exact(arguments, 4)?;
            let vfs = open(repository)?;
            let data = vfs
                .read_file(&virtual_path(&arguments[2])?, &actor)
                .map_err(map_core)?;
            fs::write(&arguments[3], &data)
                .map_err(|error| Failure::io("write host file", error))?;
            object(
                json,
                &[
                    ("path", arguments[3].clone()),
                    ("bytes", data.len().to_string()),
                ],
            )
        }
        "cat" => {
            exact(arguments, 3)?;
            let vfs = open(repository)?;
            let data = vfs
                .read_file(&virtual_path(&arguments[2])?, &actor)
                .map_err(map_core)?;
            if json {
                format!(
                    "{{\"data_hex\":\"{}\",\"bytes\":{}}}\n",
                    hex(&data),
                    data.len()
                )
            } else {
                return Ok(data);
            }
        }
        "ls" => {
            exact(arguments, 3)?;
            let entries = open(repository)?
                .list(&virtual_path(&arguments[2])?, &actor)
                .map_err(map_core)?;
            if json {
                json_array("entries", &entries)
            } else {
                lines(&entries)
            }
        }
        "stat" => {
            exact(arguments, 3)?;
            let stat = open(repository)?
                .stat(&virtual_path(&arguments[2])?, &actor)
                .map_err(map_core)?;
            let fields = vec![
                ("object", stat.object_id.to_string()),
                ("kind", format!("{:?}", stat.kind).to_lowercase()),
                ("mode", format!("{:04o}", stat.mode)),
                ("owner", stat.owner.to_string()),
                ("group", stat.group.to_string()),
                ("size", stat.size.to_string()),
            ];
            object(json, &fields)
        }
        "write" => {
            exact(arguments, 4)?;
            let mut vfs = open(repository)?;
            let id = vfs
                .write_file(
                    &virtual_path(&arguments[2])?,
                    arguments[3].as_bytes(),
                    &actor,
                )
                .map_err(map_core)?;
            object(
                json,
                &[
                    ("object", id.to_string()),
                    ("bytes", arguments[3].len().to_string()),
                ],
            )
        }
        "rm" => {
            if arguments.len() < 3 || arguments.len() > 4 {
                return Err(Failure::usage(
                    "usage: atlas rm REPOSITORY VIRTUAL_PATH [--recursive]",
                ));
            }
            let mut vfs = open(repository)?;
            if arguments.get(3).is_some_and(|value| value == "--recursive") {
                vfs.remove_recursive(&virtual_path(&arguments[2])?, &actor)
                    .map_err(map_core)?;
            } else {
                vfs.remove(&virtual_path(&arguments[2])?, &actor)
                    .map_err(map_core)?;
            }
            success(json, "removed")
        }
        "mv" => {
            exact(arguments, 4)?;
            let mut vfs = open(repository)?;
            vfs.rename(
                &virtual_path(&arguments[2])?,
                &virtual_path(&arguments[3])?,
                &actor,
            )
            .map_err(map_core)?;
            success(json, "renamed")
        }
        "chmod" => {
            exact(arguments, 4)?;
            let mode = u32::from_str_radix(arguments[3].trim_start_matches("0o"), 8)
                .map_err(|_| Failure::usage("mode must be octal, for example 0644"))?;
            let mut vfs = open(repository)?;
            let id = vfs
                .chmod(&virtual_path(&arguments[2])?, mode, &actor)
                .map_err(map_core)?;
            object(
                json,
                &[("object", id.to_string()), ("mode", format!("{mode:04o}"))],
            )
        }
        "snapshot" => snapshot(arguments, repository, json)?,
        "history" => {
            exact(arguments, 3)?;
            let values = open(repository)?
                .history(&virtual_path(&arguments[2])?, &actor)
                .map_err(map_core)?;
            let ids: Vec<_> = values
                .into_iter()
                .map(|entry| entry.object_id.to_string())
                .collect();
            if json {
                json_array("versions", &ids)
            } else {
                lines(&ids)
            }
        }
        "verify" => {
            exact(arguments, 2)?;
            let report = open(repository)?.verify().map_err(map_core)?;
            object(
                json,
                &[
                    ("objects_checked", report.objects_checked.to_string()),
                    ("bytes_checked", report.bytes_checked.to_string()),
                ],
            )
        }
        "recover" => {
            exact(arguments, 2)?;
            let mut vfs = open(repository)?;
            vfs.recover().map_err(map_core)?;
            success(json, "recovered")
        }
        "index" => {
            exact(arguments, 2)?;
            let mut vfs = open(repository)?;
            let report = vfs.rebuild_indexes().map_err(map_core)?;
            object(
                json,
                &[
                    ("reachable_objects", report.reachable_objects.to_string()),
                    ("path_entries", report.path_entries.to_string()),
                    ("history_entries", report.history_entries.to_string()),
                ],
            )
        }
        "gc" => {
            if arguments.len() < 2 || arguments.len() > 3 {
                return Err(Failure::usage("usage: atlas gc REPOSITORY [--execute]"));
            }
            let dry_run = arguments.get(2).is_none_or(|value| value != "--execute");
            let mut vfs = open(repository)?;
            let report = vfs.garbage_collect(dry_run).map_err(map_core)?;
            object(
                json,
                &[
                    ("dry_run", report.dry_run.to_string()),
                    (
                        "unreachable_objects",
                        report.unreachable_objects.to_string(),
                    ),
                    ("bytes_reclaimable", report.bytes_reclaimable.to_string()),
                    ("objects_removed", report.objects_removed.to_string()),
                    ("bytes_reclaimed", report.bytes_reclaimed.to_string()),
                ],
            )
        }
        "usage" => {
            exact(arguments, 2)?;
            let value = open(repository)?.storage_usage().map_err(map_core)?;
            object(
                json,
                &[
                    ("logical_bytes", value.logical_bytes.to_string()),
                    ("physical_bytes", value.physical_bytes.to_string()),
                    ("metadata_bytes", value.metadata_bytes.to_string()),
                    ("unreachable_bytes", value.unreachable_bytes.to_string()),
                    ("unique_chunks", value.unique_chunks.to_string()),
                    (
                        "deduplication_ratio",
                        format!("{:.3}", value.deduplication_ratio),
                    ),
                    ("snapshots", value.snapshot_count.to_string()),
                ],
            )
        }
        "backup" => {
            exact(arguments, 3)?;
            let mut vfs = open(repository)?;
            let report = vfs.backup(&arguments[2]).map_err(map_core)?;
            object(
                json,
                &[
                    ("destination", report.destination.display().to_string()),
                    ("head", report.head.to_string()),
                    ("objects", report.objects.to_string()),
                    ("bytes", report.bytes.to_string()),
                ],
            )
        }
        "server" => {
            run_server(arguments, repository)?;
            success(json, "server stopped")
        }
        "mount" => {
            exact(arguments, 3)?;
            atlasfs_fuse::mount(repository, &arguments[2])
                .map_err(|error| Failure::io("mount AtlasFS repository", error))?;
            success(json, "filesystem unmounted")
        }
        _ => {
            return Err(Failure::usage(format!(
                "unknown command: {command}\n{}",
                help()
            )));
        }
    };
    Ok(text.into_bytes())
}

fn run_server(arguments: &[String], repository: &str) -> Result<(), Failure> {
    let mut bind = "127.0.0.1:8787".to_owned();
    let mut token = env::var("ATLASFS_TOKEN").ok();
    let mut maximum = 64 * 1024 * 1024;
    let mut index = 2;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::usage("server option requires a value"))?;
        match arguments[index].as_str() {
            "--bind" => bind.clone_from(value),
            "--token" => token = Some(value.clone()),
            "--max-request-bytes" => {
                maximum = value
                    .parse()
                    .map_err(|_| Failure::usage("max request bytes must be an integer"))?;
            }
            option => return Err(Failure::usage(format!("unknown server option: {option}"))),
        }
        index += 2;
    }
    let bind = bind
        .parse()
        .map_err(|_| Failure::usage("bind address must be HOST:PORT"))?;
    let token = token.ok_or_else(|| {
        Failure::usage("set ATLASFS_TOKEN or pass --token with a non-empty bearer token")
    })?;
    let mut config = atlasfs_api::ServerConfig::new(repository, bind, token);
    config.max_request_bytes = maximum;
    eprintln!("AtlasFS API listening on http://{bind}");
    atlasfs_api::serve(config).map_err(|error| Failure::io("run API server", error))
}

fn snapshot(arguments: &[String], repository: &str, json: bool) -> Result<String, Failure> {
    let action = required(arguments, 2, "missing snapshot action")?;
    match action {
        "create" => {
            exact(arguments, 4)?;
            let info = open(repository)?
                .create_snapshot(&arguments[3])
                .map_err(map_core)?;
            Ok(object(
                json,
                &[
                    ("name", info.name),
                    ("object", info.object.to_string()),
                    ("root", info.root.to_string()),
                ],
            ))
        }
        "list" => {
            exact(arguments, 3)?;
            let names: Vec<_> = open(repository)?
                .snapshots()
                .map_err(map_core)?
                .into_iter()
                .map(|item| item.name)
                .collect();
            Ok(if json {
                json_array("snapshots", &names)
            } else {
                lines(&names)
            })
        }
        "delete" => {
            exact(arguments, 4)?;
            let mut vfs = open(repository)?;
            vfs.delete_snapshot(&arguments[3]).map_err(map_core)?;
            Ok(success(json, "snapshot deleted"))
        }
        "restore" => {
            exact(arguments, 4)?;
            let mut vfs = open(repository)?;
            vfs.restore_snapshot(&arguments[3]).map_err(map_core)?;
            Ok(success(json, "snapshot restored"))
        }
        "diff" => {
            exact(arguments, 5)?;
            let values = open(repository)?
                .diff_snapshots(&arguments[3], &arguments[4])
                .map_err(map_core)?;
            let rows: Vec<_> = values
                .into_iter()
                .map(|entry| format!("{}\t{}", diff_name(entry.kind), entry.path))
                .collect();
            Ok(if json {
                json_array("changes", &rows)
            } else {
                lines(&rows)
            })
        }
        "cat" => {
            exact(arguments, 5)?;
            let data = open(repository)?
                .read_snapshot_file(&arguments[3], &virtual_path(&arguments[4])?, &Actor::root())
                .map_err(map_core)?;
            if json {
                Ok(format!(
                    "{{\"data_hex\":\"{}\",\"bytes\":{}}}\n",
                    hex(&data),
                    data.len()
                ))
            } else {
                String::from_utf8(data)
                    .map_err(|_| Failure::usage("snapshot cat contains non-UTF-8 data; use --json"))
            }
        }
        _ => Err(Failure::usage(
            "snapshot action must be create, list, delete, restore, diff, or cat",
        )),
    }
}

fn open(path: &str) -> Result<VirtualFileSystem, Failure> {
    VirtualFileSystem::open(path).map_err(map_core)
}
fn virtual_path(value: &str) -> Result<VirtualPath, Failure> {
    VirtualPath::parse(value).map_err(map_core)
}
fn required<'a>(values: &'a [String], index: usize, message: &str) -> Result<&'a str, Failure> {
    values
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| Failure::usage(message))
}
fn exact(values: &[String], length: usize) -> Result<(), Failure> {
    if values.len() == length {
        Ok(())
    } else {
        Err(Failure::usage(
            "incorrect number of arguments; run `atlas help`",
        ))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn map_core(error: Error) -> Failure {
    let code = match &error {
        Error::NotFound(_) => EXIT_NOT_FOUND,
        Error::AlreadyExists(_) => EXIT_CONFLICT,
        Error::PermissionDenied(_) => EXIT_PERMISSION,
        Error::InvalidPath(_) | Error::InvalidOperation(_) => EXIT_USAGE,
        Error::Store(source) => {
            let text = source.to_string().to_lowercase();
            if text.contains("corrupt") || text.contains("checksum") {
                EXIT_CORRUPT
            } else if text.contains("not found") {
                EXIT_NOT_FOUND
            } else if text.contains("busy") || text.contains("conflict") || text.contains("exists")
            {
                EXIT_CONFLICT
            } else {
                EXIT_IO
            }
        }
        _ => EXIT_IO,
    };
    Failure {
        code,
        message: error.to_string(),
    }
}

#[allow(clippy::format_collect)]
fn object(json: bool, fields: &[(&str, String)]) -> String {
    if json {
        let body = fields
            .iter()
            .map(|(key, value)| format!("\"{}\":{}", escape(key), json_value(value)))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{{body}}}\n")
    } else {
        fields
            .iter()
            .map(|(key, value)| format!("{key}: {value}\n"))
            .collect()
    }
}
fn success(json: bool, message: &str) -> String {
    if json {
        format!("{{\"status\":\"{}\"}}\n", escape(message))
    } else {
        format!("{message}\n")
    }
}
fn lines(values: &[String]) -> String {
    if values.is_empty() {
        String::new()
    } else {
        format!("{}\n", values.join("\n"))
    }
}
fn json_array(key: &str, values: &[String]) -> String {
    format!(
        "{{\"{}\":[{}]}}\n",
        escape(key),
        values
            .iter()
            .map(|value| format!("\"{}\"", escape(value)))
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn json_value(value: &str) -> String {
    if value == "true" || value == "false" || value.parse::<f64>().is_ok() {
        value.to_owned()
    } else {
        format!("\"{}\"", escape(value))
    }
}
fn escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect(),
            '\n' => "\\n".chars().collect(),
            value if value.is_control() => format!("\\u{:04x}", value as u32).chars().collect(),
            value => vec![value],
        })
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 15)]),
            ]
        })
        .collect()
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
fn trace(command: &str, exit_code: u8, elapsed_ms: u128) {
    if env::var_os("ATLASFS_TRACE").is_some() {
        eprintln!(
            "{{\"event\":\"command_complete\",\"command\":\"{}\",\"exit_code\":{exit_code},\"elapsed_ms\":{elapsed_ms}}}",
            escape(command)
        );
    }
}
fn help() -> &'static str {
    "AtlasFS management CLI\n\nUsage: atlas [--json] COMMAND REPOSITORY [ARGS]\n\nCommands: init mkdir put get cat ls stat write rm mv chmod snapshot history verify recover index gc usage backup server mount\nBackup: atlas backup REPOSITORY DESTINATION\nServer: atlas server REPOSITORY [--bind HOST:PORT] [--token TOKEN] [--max-request-bytes BYTES]\nLinux FUSE: atlas mount REPOSITORY MOUNTPOINT\nExit codes: 0 success, 2 usage, 3 not found, 4 conflict, 6 I/O, 7 corruption, 8 permission\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    fn temp() -> String {
        std::env::temp_dir()
            .join(format!(
                "atlas-cli-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ))
            .to_string_lossy()
            .into_owned()
    }
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }
    #[test]
    fn command_workflow_and_json_output() {
        let repo = temp();
        execute(&args(&["init", &repo])).unwrap();
        execute(&args(&["mkdir", &repo, "/docs"])).unwrap();
        execute(&args(&["write", &repo, "/missing", "x"])).unwrap_err();
        let host = format!("{repo}-input");
        fs::write(&host, b"hello").unwrap();
        execute(&args(&["put", &repo, &host, "/docs/a"])).unwrap();
        assert_eq!(
            execute(&args(&["cat", &repo, "/docs/a"])).unwrap(),
            b"hello"
        );
        let usage = execute(&args(&["--json", "usage", &repo])).unwrap();
        assert!(
            String::from_utf8(usage)
                .unwrap()
                .contains("\"logical_bytes\":5")
        );
        execute(&args(&["snapshot", &repo, "create", "one"])).unwrap();
        assert!(
            String::from_utf8(execute(&args(&["snapshot", &repo, "list"])).unwrap())
                .unwrap()
                .contains("one")
        );
        fs::remove_dir_all(&repo).unwrap();
        fs::remove_file(host).unwrap();
    }
}
