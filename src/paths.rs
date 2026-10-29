//! Private per-user paths for the files Hyusk creates at run time.
//!
//! Generated TTS audio, screenshots, and Codex output used to be written into
//! the shared temporary directory, where another local account could read them
//! and where a predictable name could be guessed. They now live under a
//! per-user runtime directory (`$XDG_RUNTIME_DIR/hyusk`) with mode `0700`, and
//! every file is created with mode `0600` and a unique name. The shared
//! temporary directory is used only when the runtime directory is unavailable,
//! and files created there are still owner-only.
//!
//! The generated files sit in a subdirectory rather than beside the indicator
//! files because `hyusk-state`, `hyusk-status.json`, `hyusk-control.json`, and
//! `hyusk-pairing.json` are read directly from `$XDG_RUNTIME_DIR` by the GNOME
//! and Noctalia panels, which is a protocol that must not move.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Build attempts allowed when a generated name collides with an existing file.
const CREATE_ATTEMPTS: u32 = 16;

static FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The per-user directory for generated files: `$XDG_RUNTIME_DIR/hyusk`,
/// created with mode `0700` and tightened on every call.
///
/// Returns `None` when the variable is unset or the directory cannot be
/// created, so callers fall back to the shared temporary directory.
pub fn runtime_dir() -> Option<PathBuf> {
    runtime_directory(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

/// Testable half of [`runtime_dir`]: the environment is read once, here, so a
/// test can drive it with a scratch directory instead of mutating the process
/// environment.
fn runtime_directory(base: Option<PathBuf>) -> Option<PathBuf> {
    let directory = base?.join("hyusk");
    ensure_private_dir(&directory).ok()?;
    Some(directory)
}

/// Create `directory` with mode `0700`, tightening an existing wider mode.
pub fn ensure_private_dir(directory: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    restrict_mode(directory, 0o700);
    Ok(())
}

/// Directory generated files go into: the private runtime directory when it is
/// available, otherwise the shared temporary directory.
pub fn private_dir() -> PathBuf {
    runtime_dir().unwrap_or_else(std::env::temp_dir)
}

/// Create a unique empty file with mode `0600` in [`private_dir`].
///
/// Falls back to the shared temporary directory when the runtime directory is
/// unavailable, keeping the file owner-only. The caller owns the file and is
/// responsible for removing it.
pub fn private_file(prefix: &str, extension: &str) -> io::Result<PathBuf> {
    if let Some(directory) = runtime_dir() {
        if let Ok(path) = create_private_file_in(&directory, prefix, extension) {
            return Ok(path);
        }
    }

    create_private_file_in(&std::env::temp_dir(), prefix, extension)
}

/// Best-effort [`private_file`] for callers that cannot report an error.
///
/// When the file cannot be created the caller still receives a unique path
/// under [`private_dir`], and the external writer that creates it should be
/// followed by [`restrict_file`].
pub fn private_file_or_path(prefix: &str, extension: &str) -> PathBuf {
    private_file(prefix, extension)
        .unwrap_or_else(|_| unique_path(&private_dir(), prefix, extension))
}

/// Tighten an existing file to mode `0600`.
///
/// Best effort on purpose: an external program may have created or replaced
/// the file with the default mode, and the caller cannot always control that.
pub fn restrict_file(path: &Path) {
    restrict_mode(path, 0o600);
}

fn create_private_file_in(directory: &Path, prefix: &str, extension: &str) -> io::Result<PathBuf> {
    if !directory.is_dir() {
        fs::create_dir_all(directory)?;
    }

    for _ in 0..CREATE_ATTEMPTS {
        let path = unique_path(directory, prefix, extension);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        match options.open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique private file",
    ))
}

fn unique_path(directory: &Path, prefix: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);

    directory.join(format!(
        "{prefix}{}-{nanos}-{sequence}.{extension}",
        std::process::id()
    ))
}

#[cfg(unix)]
fn restrict_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict_mode(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hyusk-paths-{label}-{}-{}",
            std::process::id(),
            FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create scratch directory");
        path
    }

    #[test]
    fn runtime_directory_is_a_private_child_of_the_runtime_base() {
        let base = scratch("runtime-base");
        let directory = runtime_directory(Some(base.clone())).expect("runtime directory");

        assert_eq!(directory, base.join("hyusk"));
        assert!(directory.is_dir());

        #[cfg(unix)]
        {
            assert_eq!(mode(&directory), 0o700);

            // A wider mode from an earlier version is tightened on the next call.
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
                .expect("widen directory");
            let again = runtime_directory(Some(base.clone())).expect("runtime directory");
            assert_eq!(mode(&again), 0o700);
        }

        // Without a base directory there is no runtime directory, so the
        // caller falls back to the shared temporary directory.
        assert!(runtime_directory(None).is_none());

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only_and_unique() {
        let directory = scratch("files");
        let first = create_private_file_in(&directory, "hyusk-test-", "wav").expect("first file");
        let second = create_private_file_in(&directory, "hyusk-test-", "wav").expect("second file");

        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(directory.as_path()));

        let name = first
            .file_name()
            .and_then(|name| name.to_str())
            .expect("utf-8 file name");
        assert!(name.starts_with("hyusk-test-"), "{name}");
        assert!(name.ends_with(".wav"), "{name}");

        assert_eq!(mode(&first), 0o600);
        assert_eq!(mode(&second), 0o600);

        let _ = fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn restrict_file_tightens_a_world_readable_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = scratch("restrict");
        let path = directory.join("hyusk-restrict-test.txt");
        fs::write(&path, "text").expect("write file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("widen file");
        assert_eq!(mode(&path), 0o644);

        restrict_file(&path);

        assert_eq!(mode(&path), 0o600);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn generated_paths_keep_their_prefix_and_extension() {
        let directory = scratch("names");
        let path = unique_path(&directory, "hyusk-shot-", "png");
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("utf-8 file name");

        assert!(name.starts_with("hyusk-shot-"), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        assert_eq!(path.parent(), Some(directory.as_path()));

        let _ = fs::remove_dir_all(&directory);
    }
}
