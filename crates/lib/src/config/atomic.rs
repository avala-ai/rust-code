//! Atomic, mode-preserving write helper for configuration-style files.
//!
//! Configuration and plugin-settings files frequently bear secrets or
//! security-sensitive flags (per-project plugin enable/disable, MCP
//! credentials, OAuth tokens). The naive `std::fs::write` round-trip
//! truncates and rewrites in place, which both leaves a corruption
//! window if the process dies mid-write AND resets the file to the
//! umask default (usually `0644`), turning a locked-down `0600` file
//! into a world-readable one.
//!
//! [`atomic_write_secret`] sidesteps both problems:
//!
//! 1. Captures the destination's mode (or defaults to `0600` if the
//!    file doesn't exist yet — config-style files are secret-bearing
//!    by default).
//! 2. Materialises the new contents in an unguessable sibling tempfile
//!    via [`tempfile::NamedTempFile::new_in`] so two concurrent writers
//!    can't clobber each other's temp file.
//! 3. Applies the captured mode to the temp file BEFORE `persist`-ing.
//! 4. Atomically renames into place (POSIX guarantees this; on Windows
//!    `MoveFileEx` is atomic on local filesystems).
//! 5. Best-effort fsync of the parent directory on Unix so the rename
//!    survives a power loss.

use std::io::Write as _;
use std::path::Path;

/// Default mode for newly-created config-style files: owner read+write
/// only. Files in this category routinely hold secrets, so the only
/// safe default is restrictive.
#[cfg(unix)]
const DEFAULT_NEW_MODE: u32 = 0o600;

/// Errors raised by [`atomic_write_secret`].
#[derive(Debug, thiserror::Error)]
pub enum AtomicWriteError {
    #[error("path has no parent directory: {0}")]
    NoParent(String),
    #[error("create directory {0}: {1}")]
    CreateDir(String, std::io::Error),
    #[error("create temp file in {0}: {1}")]
    CreateTemp(String, std::io::Error),
    #[error("set mode on temp file: {0}")]
    SetMode(std::io::Error),
    #[error("write temp file: {0}")]
    Write(std::io::Error),
    #[error("fsync temp file: {0}")]
    Fsync(std::io::Error),
    #[error("rename temp into {0}: {1}")]
    Persist(String, std::io::Error),
}

/// Write `bytes` to `path` atomically, preserving the destination's
/// existing file mode (or applying the secret-default mode when the
/// file doesn't yet exist).
pub fn atomic_write_secret(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    let parent = path
        .parent()
        .ok_or_else(|| AtomicWriteError::NoParent(path.display().to_string()))?;
    if !parent.as_os_str().is_empty() && !parent.exists() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AtomicWriteError::CreateDir(parent.display().to_string(), e))?;
    }

    let preserved_perms = std::fs::metadata(path).ok().map(|m| m.permissions());

    let parent_for_temp = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };

    let mut tmp = tempfile::NamedTempFile::new_in(parent_for_temp)
        .map_err(|e| AtomicWriteError::CreateTemp(parent_for_temp.display().to_string(), e))?;

    apply_mode(&tmp, preserved_perms.as_ref()).map_err(AtomicWriteError::SetMode)?;

    tmp.as_file_mut()
        .write_all(bytes)
        .map_err(AtomicWriteError::Write)?;
    tmp.as_file_mut()
        .sync_all()
        .map_err(AtomicWriteError::Fsync)?;

    tmp.persist(path)
        .map_err(|e| AtomicWriteError::Persist(path.display().to_string(), e.error))?;

    #[cfg(unix)]
    {
        if !parent.as_os_str().is_empty()
            && let Ok(dir) = std::fs::File::open(parent)
        {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

#[cfg(unix)]
fn apply_mode(
    tmp: &tempfile::NamedTempFile,
    preserved: Option<&std::fs::Permissions>,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = preserved
        .map(|p| p.mode() & 0o777)
        .unwrap_or(DEFAULT_NEW_MODE);
    let perms = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(tmp.path(), perms)
}

#[cfg(not(unix))]
fn apply_mode(
    _tmp: &tempfile::NamedTempFile,
    _preserved: Option<&std::fs::Permissions>,
) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn writes_new_file_with_default_mode() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        atomic_write_secret(&path, b"hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, DEFAULT_NEW_MODE);
        }
    }

    #[test]
    fn preserves_existing_mode_on_overwrite() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("existing.toml");
        std::fs::write(&path, b"old").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        }

        atomic_write_secret(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o640);
        }
    }

    #[test]
    fn creates_missing_parent_directory() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a").join("b").join("c.toml");
        atomic_write_secret(&nested, b"x").unwrap();
        assert!(nested.exists());
    }
}
