use std::sync::{LazyLock, Mutex, MutexGuard};

static GUEST_BUILD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(crate) fn guest_build_lock() -> MutexGuard<'static, ()> {
    GUEST_BUILD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
