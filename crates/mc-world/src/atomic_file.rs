//! Cross-platform same-directory atomic file replacement and durability helpers.

use std::fs::File;
use std::io;
use std::path::Path;

/// Atomically install `temporary` as `target`, replacing an existing target.
///
/// The temporary file must already be fully written and synced by the caller.
/// On replacement failure the temporary path is removed best-effort.
pub fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    let result = replace_file_platform(temporary, target);
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Replace the target and then persist the containing directory entry where the
/// platform supports explicit directory synchronization.
pub fn replace_file_durable(temporary: &Path, target: &Path) -> io::Result<()> {
    replace_file(temporary, target)?;
    sync_parent_dir(target)
}

/// Persist the parent directory entry after create/rename.
pub fn sync_parent_dir(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    #[cfg(unix)]
    {
        File::open(parent)?.sync_all()
    }
    #[cfg(windows)]
    {
        // MoveFileExW with MOVEFILE_WRITE_THROUGH flushes the replacement.
        let _ = parent;
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = parent;
        Ok(())
    }
}

#[cfg(unix)]
fn replace_file_platform(temporary: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(temporary, target)
}

#[cfg(windows)]
fn replace_file_platform(temporary: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let temporary: Vec<u16> = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers that remain
    // alive for the duration of the call.
    let replaced = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn replace_file_platform(temporary: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(temporary, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_missing_and_existing_targets() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("state.dat");
        let first = root.path().join("state.dat.tmp.1");
        std::fs::write(&first, b"first").unwrap();
        replace_file_durable(&first, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first");
        assert!(!first.exists());

        let second = root.path().join("state.dat.tmp.2");
        std::fs::write(&second, b"second").unwrap();
        replace_file_durable(&second, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        assert!(!second.exists());
    }

    #[test]
    fn replacement_failure_cleans_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let temporary = root.path().join("state.tmp");
        let target = root.path().join("missing-parent/state.dat");
        std::fs::write(&temporary, b"payload").unwrap();

        assert!(replace_file(&temporary, &target).is_err());
        assert!(!temporary.exists());
        assert!(!target.exists());
    }
}
