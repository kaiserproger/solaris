use std::io::{BufRead as _, Read as _, Write as _};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use super::WORLD_LEASE_FILE_NAME;
use crate::storage::test_support::single_air_registry;
use crate::storage::{WorldError, WorldStorage};

const HELPER_ROOT_ENV: &str = "SOLARIS_WORLD_LEASE_HELPER_ROOT";
const READY_MARKER: &str = "SOLARIS_WORLD_LEASE_READY";

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn terminate(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[test]
fn world_lease_subprocess_helper() {
    let Some(root) = std::env::var_os(HELPER_ROOT_ENV) else {
        return;
    };
    let _world = WorldStorage::open(root, single_air_registry()).expect("helper acquires lease");
    println!("{READY_MARKER} pid={}", std::process::id());
    std::io::stdout().flush().unwrap();

    let mut release = [0_u8; 1];
    std::io::stdin().read_exact(&mut release).unwrap();
}

#[test]
fn writable_world_lease_is_process_exclusive_and_recovers_after_crash() {
    let first_root = tempfile::tempdir().unwrap();
    let second_root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(first_root.path().join("region")).unwrap();
    std::fs::create_dir_all(second_root.path().join("region")).unwrap();

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "storage::world_lease::tests::world_lease_subprocess_helper",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HELPER_ROOT_ENV, first_root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let child_pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let mut child = ChildGuard { child };
    let mut output = std::io::BufReader::new(stdout);
    let mut line = String::new();
    let mut ready = false;
    for _ in 0..32 {
        line.clear();
        if output.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if line.contains(READY_MARKER) {
            ready = true;
            break;
        }
    }
    assert!(ready, "lease helper exited before publishing readiness");

    let registry = single_air_registry();
    let error = match WorldStorage::open(first_root.path(), Arc::clone(&registry)) {
        Ok(_) => panic!("second process unexpectedly acquired writable world lease"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        WorldError::WorldLocked { metadata, .. }
            if metadata.contains(&format!("pid={child_pid}"))
                && metadata.contains("started_unix_ms=")
                && metadata.contains("instance=")
    ));

    let read_only = WorldStorage::open_read_only(first_root.path(), Arc::clone(&registry)).unwrap();
    assert!(matches!(
        read_only.plan_dirty_flush(),
        Err(WorldError::ReadOnlyWorld(path)) if path == first_root.path()
    ));
    drop(read_only);

    let distinct = WorldStorage::open(second_root.path(), Arc::clone(&registry)).unwrap();
    drop(distinct);

    let lease_path = first_root.path().join(WORLD_LEASE_FILE_NAME);
    let stale_metadata = std::fs::read_to_string(&lease_path).unwrap();
    assert!(stale_metadata.contains(&format!("pid={child_pid}")));

    child.terminate();

    let reopened = WorldStorage::open(first_root.path(), registry).unwrap();
    let refreshed_metadata = std::fs::read_to_string(&lease_path).unwrap();
    assert!(refreshed_metadata.contains(&format!("pid={}", std::process::id())));
    assert_ne!(refreshed_metadata, stale_metadata);
    drop(reopened);
}
