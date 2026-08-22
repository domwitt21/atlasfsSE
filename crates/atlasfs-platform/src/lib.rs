#![doc = "Platform-specific durability operations for `AtlasFS`."]

use std::{io, path::Path};

/// Atomically publishes `source` as `destination` and requests durable metadata.
///
/// When `replace` is true, an existing destination is replaced atomically where
/// the operating system supports it. Both paths must reside on the same volume.
///
/// # Errors
///
/// Returns the operating-system error from rename, replacement, or directory
/// synchronization.
pub fn publish(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    publish_impl(source, destination, replace)
}

#[cfg(unix)]
fn publish_impl(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    use std::{fs, io::ErrorKind};

    if !replace && destination.exists() {
        return Err(io::Error::new(
            ErrorKind::AlreadyExists,
            "publish destination already exists",
        ));
    }
    fs::rename(source, destination)?;
    sync_parent(destination)
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    use std::fs::File;

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn publish_impl(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows path contains NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    }

    let source = wide_path(source)?;
    let destination = wide_path(destination)?;
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }

    // SAFETY: Both path buffers are NUL-terminated and live for the duration of
    // the call. The flags are documented MoveFileExW values.
    let succeeded = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn publish_impl(_source: &Path, _destination: &Path, _replace: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "durable publication is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "atlasfs-platform-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn publishes_new_file_and_replaces_existing_file() {
        let directory = test_directory();
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source");
        let destination = directory.join("destination");

        fs::write(&source, b"first").unwrap();
        publish(&source, &destination, false).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"first");
        assert!(!source.exists());

        fs::write(&source, b"second").unwrap();
        publish(&source, &destination, true).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"second");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn refuses_existing_destination_without_replace() {
        let directory = test_directory();
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source");
        let destination = directory.join("destination");
        fs::write(&source, b"source").unwrap();
        fs::write(&destination, b"destination").unwrap();

        assert_eq!(
            publish(&source, &destination, false).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&destination).unwrap(), b"destination");
        assert_eq!(fs::read(&source).unwrap(), b"source");

        fs::remove_dir_all(directory).unwrap();
    }
}
