use std::ffi::OsStr;
use std::fs::{DirEntry, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use same_file::Handle;

use crate::{
    MAX_JSON_FILE_BYTES, MAX_JSON_FILES, MAX_JSON_TOTAL_BYTES, MAX_JSON_WALK_DEPTH,
    MAX_JSON_WALK_ENTRIES,
};

#[derive(Debug, Clone, Copy)]
struct SidecarLimits {
    max_depth: usize,
    max_entries: usize,
    max_files: usize,
    max_file_bytes: usize,
    max_total_bytes: usize,
}

const DEFAULT_LIMITS: SidecarLimits = SidecarLimits {
    max_depth: MAX_JSON_WALK_DEPTH,
    max_entries: MAX_JSON_WALK_ENTRIES,
    max_files: MAX_JSON_FILES,
    max_file_bytes: MAX_JSON_FILE_BYTES,
    max_total_bytes: MAX_JSON_TOTAL_BYTES,
};

#[derive(Debug)]
pub(crate) struct SidecarIoError {
    pub(crate) path: PathBuf,
    pub(crate) source: io::Error,
}

pub(crate) fn collect_files(
    root: &Path,
    extension: &str,
    recursive: bool,
) -> Result<Vec<PathBuf>, SidecarIoError> {
    collect_files_with_limits(root, extension, recursive, DEFAULT_LIMITS)
}

pub(crate) fn collect_files_under(
    trusted_root: &Path,
    root: &Path,
    extension: &str,
    recursive: bool,
) -> Result<Vec<PathBuf>, SidecarIoError> {
    validate_directory_under(trusted_root, root)?;
    collect_files_with_limits(root, extension, recursive, DEFAULT_LIMITS)
}

pub(crate) fn directory_exists_under(
    trusted_root: &Path,
    path: &Path,
) -> Result<bool, SidecarIoError> {
    validate_directory_components(trusted_root, path, true)
}

fn collect_files_with_limits(
    root: &Path,
    extension: &str,
    recursive: bool,
    limits: SidecarLimits,
) -> Result<Vec<PathBuf>, SidecarIoError> {
    validate_directory(root)?;

    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut files = Vec::new();
    let mut entries_seen = 0_usize;
    let mut total_bytes = 0_usize;

    while let Some((dir, depth)) = pending.pop() {
        validate_directory(&dir)?;
        let entries = read_sorted_entries(
            &dir,
            limits.max_entries.saturating_sub(entries_seen),
            entries_seen,
        )?;
        let mut child_dirs = Vec::new();

        for entry in entries {
            entries_seen = entries_seen.checked_add(1).ok_or_else(|| SidecarIoError {
                path: dir.clone(),
                source: invalid_data("sidecar entry count overflow"),
            })?;
            if entries_seen > limits.max_entries {
                return Err(SidecarIoError {
                    path: dir.clone(),
                    source: invalid_data(format!(
                        "sidecar walk visited {entries_seen} entries, exceeding limit {}",
                        limits.max_entries
                    )),
                });
            }

            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| SidecarIoError {
                path: path.clone(),
                source,
            })?;
            if file_type.is_symlink() {
                return Err(SidecarIoError {
                    path,
                    source: invalid_data("sidecar symlinks are not allowed"),
                });
            }
            if file_type.is_dir() {
                if recursive {
                    let child_depth = depth.checked_add(1).ok_or_else(|| SidecarIoError {
                        path: path.clone(),
                        source: invalid_data("sidecar depth overflow"),
                    })?;
                    if child_depth > limits.max_depth {
                        return Err(SidecarIoError {
                            path,
                            source: invalid_data(format!(
                                "sidecar depth {child_depth} exceeds limit {}",
                                limits.max_depth
                            )),
                        });
                    }
                    child_dirs.push((path, child_depth));
                }
                continue;
            }
            if !file_type.is_file() {
                return Err(SidecarIoError {
                    path,
                    source: invalid_data("sidecar contains an unsupported filesystem entry"),
                });
            }
            if path.extension() != Some(OsStr::new(extension)) {
                continue;
            }

            let metadata = entry.metadata().map_err(|source| SidecarIoError {
                path: path.clone(),
                source,
            })?;
            let file_bytes = usize::try_from(metadata.len()).map_err(|_| SidecarIoError {
                path: path.clone(),
                source: invalid_data("sidecar file size does not fit this platform"),
            })?;
            if file_bytes > limits.max_file_bytes {
                return Err(SidecarIoError {
                    path,
                    source: invalid_data(format!(
                        "sidecar file is {file_bytes} bytes, exceeding limit {}",
                        limits.max_file_bytes
                    )),
                });
            }

            let file_count = files.len().checked_add(1).ok_or_else(|| SidecarIoError {
                path: path.clone(),
                source: invalid_data("sidecar file count overflow"),
            })?;
            if file_count > limits.max_files {
                return Err(SidecarIoError {
                    path,
                    source: invalid_data(format!(
                        "sidecar walk found {file_count} matching files, exceeding limit {}",
                        limits.max_files
                    )),
                });
            }
            total_bytes = total_bytes
                .checked_add(file_bytes)
                .ok_or_else(|| SidecarIoError {
                    path: path.clone(),
                    source: invalid_data("sidecar aggregate byte count overflow"),
                })?;
            if total_bytes > limits.max_total_bytes {
                return Err(SidecarIoError {
                    path,
                    source: invalid_data(format!(
                        "sidecar matching files total {total_bytes} bytes, exceeding limit {}",
                        limits.max_total_bytes
                    )),
                });
            }
            files.push(path);
        }

        child_dirs.reverse();
        pending.extend(child_dirs);
    }

    files.sort();
    Ok(files)
}

pub(crate) fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    read_file_with_limit(path, MAX_JSON_FILE_BYTES)
}

pub(crate) fn read_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read_file(path)?)
        .map_err(|source| invalid_data(format!("sidecar file is not UTF-8: {source}")))
}

fn read_file_with_limit(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    validate_regular_path(path)?;
    let file = File::open(path)?;
    validate_opened_path(path, &file)?;
    read_opened_file_with_limit(path, file, max_bytes)
}

pub(crate) fn read_opened_file(path: &Path, file: File) -> io::Result<Vec<u8>> {
    read_opened_file_with_limit(path, file, MAX_JSON_FILE_BYTES)
}

fn read_opened_file_with_limit(
    path: &Path,
    mut file: File,
    max_bytes: usize,
) -> io::Result<Vec<u8>> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid_data("sidecar path is not a regular file"));
    }
    let len = usize::try_from(metadata.len())
        .map_err(|_| invalid_data("sidecar file size does not fit this platform"))?;
    if len > max_bytes {
        return Err(invalid_data(format!(
            "sidecar file at {} is {len} bytes, exceeding limit {max_bytes}",
            path.display()
        )));
    }

    file.seek(SeekFrom::Start(0))?;
    read_exact_len(&mut file, len)
}

fn read_exact_len(reader: &mut impl Read, len: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|error| io::Error::other(format!("reserve sidecar buffer: {error}")))?;
    bytes.resize(len, 0);
    reader.read_exact(&mut bytes)?;

    let mut extra = [0_u8; 1];
    if reader.read(&mut extra)? != 0 {
        return Err(invalid_data(
            "sidecar file changed or grew while being read",
        ));
    }
    Ok(bytes)
}

fn validate_directory_under(trusted_root: &Path, path: &Path) -> Result<(), SidecarIoError> {
    if validate_directory_components(trusted_root, path, false)? {
        Ok(())
    } else {
        Err(SidecarIoError {
            path: path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::NotFound, "sidecar directory is missing"),
        })
    }
}

fn validate_directory_components(
    trusted_root: &Path,
    path: &Path,
    allow_missing: bool,
) -> Result<bool, SidecarIoError> {
    validate_directory(trusted_root)?;
    let relative = path
        .strip_prefix(trusted_root)
        .map_err(|_| SidecarIoError {
            path: path.to_path_buf(),
            source: invalid_data(format!(
                "sidecar path is not below trusted root {}",
                trusted_root.display()
            )),
        })?;
    let mut current = trusted_root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(SidecarIoError {
                path: path.to_path_buf(),
                source: invalid_data("sidecar path contains a non-normal component"),
            });
        };
        current.push(component);
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(source) if allow_missing && source.kind() == io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(source) => {
                return Err(SidecarIoError {
                    path: current,
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(SidecarIoError {
                path: current,
                source: invalid_data("sidecar directory symlinks are not allowed"),
            });
        }
        if !metadata.is_dir() {
            return Err(SidecarIoError {
                path: current,
                source: invalid_data("sidecar path component is not a directory"),
            });
        }
    }
    Ok(true)
}

fn validate_directory(path: &Path) -> Result<(), SidecarIoError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| SidecarIoError {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(SidecarIoError {
            path: path.to_path_buf(),
            source: invalid_data("sidecar directory symlinks are not allowed"),
        });
    }
    if !metadata.is_dir() {
        return Err(SidecarIoError {
            path: path.to_path_buf(),
            source: invalid_data("sidecar walk root is not a directory"),
        });
    }
    Ok(())
}

fn validate_regular_path(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid_data("sidecar file symlinks are not allowed"));
    }
    if !metadata.is_file() {
        return Err(invalid_data("sidecar path is not a regular file"));
    }
    Ok(())
}

fn validate_opened_path(path: &Path, file: &File) -> io::Result<()> {
    validate_regular_path(path)?;
    let opened = Handle::from_file(file.try_clone()?)?;
    let current = Handle::from_path(path)?;
    if opened != current {
        return Err(invalid_data("sidecar path changed while being opened"));
    }
    Ok(())
}

fn read_sorted_entries(
    dir: &Path,
    remaining_entries: usize,
    entries_seen: usize,
) -> Result<Vec<DirEntry>, SidecarIoError> {
    let directory = std::fs::read_dir(dir).map_err(|source| SidecarIoError {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut entries = Vec::new();
    for entry in directory {
        if entries.len() >= remaining_entries {
            return Err(SidecarIoError {
                path: dir.to_path_buf(),
                source: invalid_data(format!(
                    "sidecar walk visited {} entries, exceeding limit {}",
                    entries_seen.saturating_add(entries.len()).saturating_add(1),
                    entries_seen.saturating_add(remaining_entries)
                )),
            });
        }
        if entries.len() == entries.capacity() {
            entries.try_reserve(1).map_err(|error| SidecarIoError {
                path: dir.to_path_buf(),
                source: io::Error::other(format!("reserve sidecar directory entries: {error}")),
            })?;
        }
        entries.push(entry.map_err(|source| SidecarIoError {
            path: dir.to_path_buf(),
            source,
        })?);
    }
    entries.sort_by_key(DirEntry::file_name);
    Ok(entries)
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn limits(
        max_depth: usize,
        max_entries: usize,
        max_files: usize,
        max_file_bytes: usize,
        max_total_bytes: usize,
    ) -> SidecarLimits {
        SidecarLimits {
            max_depth,
            max_entries,
            max_files,
            max_file_bytes,
            max_total_bytes,
        }
    }

    #[test]
    fn walk_is_iterative_and_returns_deterministic_order() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("z/deep")).unwrap();
        fs::create_dir_all(tmp.path().join("a")).unwrap();
        fs::write(tmp.path().join("z/deep/third.json"), b"{}").unwrap();
        fs::write(tmp.path().join("z/second.json"), b"{}").unwrap();
        fs::write(tmp.path().join("a/first.json"), b"{}").unwrap();
        fs::write(tmp.path().join("ignored.txt"), b"ignored").unwrap();

        let paths =
            collect_files_with_limits(tmp.path(), "json", true, limits(8, 16, 8, 16, 64)).unwrap();
        let relative = paths
            .iter()
            .map(|path| path.strip_prefix(tmp.path()).unwrap().to_path_buf())
            .collect::<Vec<_>>();
        assert_eq!(
            relative,
            vec![
                PathBuf::from("a/first.json"),
                PathBuf::from("z/deep/third.json"),
                PathBuf::from("z/second.json"),
            ]
        );
    }

    #[test]
    fn walk_enforces_depth_entry_file_and_byte_budgets() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        fs::write(tmp.path().join("a/b/deep.json"), b"{}").unwrap();
        let error = collect_files_with_limits(tmp.path(), "json", true, limits(1, 16, 8, 16, 64))
            .unwrap_err();
        assert_eq!(error.source.kind(), io::ErrorKind::InvalidData);
        assert!(error.source.to_string().contains("depth 2"));

        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.json"), b"{}").unwrap();
        fs::write(tmp.path().join("b.json"), b"{}").unwrap();
        let entry_error =
            collect_files_with_limits(tmp.path(), "json", false, limits(0, 1, 8, 16, 64))
                .unwrap_err();
        assert!(entry_error.source.to_string().contains("visited 2 entries"));
        let file_error =
            collect_files_with_limits(tmp.path(), "json", false, limits(0, 8, 1, 16, 64))
                .unwrap_err();
        assert!(
            file_error
                .source
                .to_string()
                .contains("found 2 matching files")
        );
        let total_error =
            collect_files_with_limits(tmp.path(), "json", false, limits(0, 8, 8, 16, 3))
                .unwrap_err();
        assert!(total_error.source.to_string().contains("total 4 bytes"));

        fs::write(tmp.path().join("large.json"), b"12345").unwrap();
        let size_error =
            collect_files_with_limits(tmp.path(), "json", false, limits(0, 8, 8, 4, 64))
                .unwrap_err();
        assert!(size_error.source.to_string().contains("5 bytes"));
    }

    #[cfg(unix)]
    #[test]
    fn walk_and_direct_read_reject_symlinks() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.json");
        fs::write(&target, b"{}").unwrap();
        let link = tmp.path().join("link.json");
        symlink(&target, &link).unwrap();

        let walk_error = collect_files(tmp.path(), "json", true).unwrap_err();
        assert_eq!(walk_error.path, link);
        assert!(walk_error.source.to_string().contains("symlinks"));
        let read_error = read_file(&link).unwrap_err();
        assert!(read_error.to_string().contains("symlinks"));

        fs::remove_file(&link).unwrap();
        let loop_path = tmp.path().join("loop");
        symlink(tmp.path(), &loop_path).unwrap();
        let loop_error = collect_files(tmp.path(), "json", true).unwrap_err();
        assert_eq!(loop_error.path, loop_path);
        assert!(loop_error.source.to_string().contains("symlinks"));

        let anchor = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(outside.path().join("nested")).unwrap();
        fs::write(outside.path().join("nested/escaped.json"), b"{}").unwrap();
        let ancestor_link = anchor.path().join("linked");
        symlink(outside.path(), &ancestor_link).unwrap();
        let escaped_root = ancestor_link.join("nested");
        let escape_error = collect_files_under(anchor.path(), &escaped_root, "json", true)
            .expect_err("trusted-root traversal must reject a symlinked ancestor");
        assert_eq!(escape_error.path, ancestor_link);
        assert!(escape_error.source.to_string().contains("symlinks"));
    }

    #[test]
    fn bounded_read_accepts_exact_limit_and_propagates_partial_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exact.json");
        fs::write(&path, b"1234").unwrap();
        assert_eq!(read_file_with_limit(&path, 4).unwrap(), b"1234");
        assert!(read_file_with_limit(&path, 3).is_err());

        struct FailsAfterPrefix {
            emitted: bool,
        }
        impl Read for FailsAfterPrefix {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.emitted {
                    return Err(io::Error::other("injected partial read failure"));
                }
                self.emitted = true;
                let len = buf.len().min(2);
                buf[..len].copy_from_slice(&b"12"[..len]);
                Ok(len)
            }
        }

        let error = read_exact_len(&mut FailsAfterPrefix { emitted: false }, 4).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.to_string().contains("partial read failure"));
    }
}
