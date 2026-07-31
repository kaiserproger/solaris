use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use super::WorldError;

const WORLD_LEASE_FILE_NAME: &str = ".solaris-world.lock";
const WORLD_LEASE_METADATA_MAX_BYTES: usize = 4 * 1024;
static WORLD_LEASES: OnceLock<Mutex<HashMap<PathBuf, Weak<WorldRootLease>>>> = OnceLock::new();

pub(super) struct WorldRootLease {
    _file: File,
}

fn world_leases() -> &'static Mutex<HashMap<PathBuf, Weak<WorldRootLease>>> {
    WORLD_LEASES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn acquire_world_root_lease(root: &Path) -> Result<Arc<WorldRootLease>, WorldError> {
    let canonical_root =
        std::fs::canonicalize(root).map_err(|source| WorldError::WorldLeaseIo {
            operation: "canonicalize root",
            path: root.to_path_buf(),
            source,
        })?;
    let mut leases = world_leases()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(lease) = leases.get(&canonical_root).and_then(Weak::upgrade) {
        return Ok(lease);
    }

    let lease_path = canonical_root.join(WORLD_LEASE_FILE_NAME);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lease_path)
        .map_err(|source| WorldError::WorldLeaseIo {
            operation: "open lease file",
            path: lease_path.clone(),
            source,
        })?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let metadata = read_world_lease_metadata(&mut file)
                .unwrap_or_else(|error| format!("unavailable ({error})"));
            return Err(WorldError::WorldLocked {
                root: canonical_root,
                metadata,
            });
        }
        Err(TryLockError::Error(source)) => {
            return Err(WorldError::WorldLeaseIo {
                operation: "acquire exclusive lease",
                path: lease_path,
                source,
            });
        }
    }

    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let pid = std::process::id();
    let metadata =
        format!("pid={pid}\nstarted_unix_ms={started_unix_ms}\ninstance={pid}-{started_unix_ms}\n");
    file.set_len(0)
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .and_then(|()| file.write_all(metadata.as_bytes()))
        .and_then(|()| file.sync_data())
        .map_err(|source| WorldError::WorldLeaseIo {
            operation: "write holder metadata",
            path: lease_path,
            source,
        })?;

    let lease = Arc::new(WorldRootLease { _file: file });
    leases.insert(canonical_root, Arc::downgrade(&lease));
    Ok(lease)
}

fn read_world_lease_metadata(file: &mut File) -> Result<String, std::io::Error> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = [0_u8; WORLD_LEASE_METADATA_MAX_BYTES];
    let read = file.read(&mut bytes)?;
    if read == 0 {
        return Ok("unavailable".to_string());
    }
    Ok(String::from_utf8_lossy(&bytes[..read]).trim().to_string())
}

#[cfg(test)]
#[path = "world_lease_tests.rs"]
mod tests;
