use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LockResult, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

static AUTHORITATIVE_POISON_COUNT: AtomicU64 = AtomicU64::new(0);
static BENIGN_RESET_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuthoritativeLockPoison {
    lock: &'static str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeLockPoisonMetricsSnapshot {
    pub authoritative_poison: u64,
    pub benign_reset: u64,
}

#[must_use]
pub fn runtime_lock_poison_metrics_snapshot() -> RuntimeLockPoisonMetricsSnapshot {
    RuntimeLockPoisonMetricsSnapshot {
        authoritative_poison: AUTHORITATIVE_POISON_COUNT.load(Ordering::Relaxed),
        benign_reset: BENIGN_RESET_COUNT.load(Ordering::Relaxed),
    }
}

#[must_use]
pub(crate) fn authoritative_lock_poison_from_panic(
    payload: &(dyn Any + Send),
) -> Option<&'static str> {
    payload
        .downcast_ref::<AuthoritativeLockPoison>()
        .map(|poison| poison.lock)
}

#[cold]
#[inline(never)]
fn fail_authoritative(lock: &'static str) -> ! {
    AUTHORITATIVE_POISON_COUNT.fetch_add(1, Ordering::Relaxed);
    std::panic::panic_any(AuthoritativeLockPoison { lock });
}

pub(crate) fn resolve_authoritative_lock<T>(result: LockResult<T>, name: &'static str) -> T {
    match result {
        Ok(value) => value,
        Err(_) => fail_authoritative(name),
    }
}

pub(crate) fn lock_authoritative_mutex<'a, T>(
    lock: &'a Mutex<T>,
    name: &'static str,
) -> MutexGuard<'a, T> {
    resolve_authoritative_lock(lock.lock(), name)
}

pub(crate) fn lock_benign_mutex<'a, T: Default>(
    lock: &'a Mutex<T>,
    _name: &'static str,
) -> MutexGuard<'a, T> {
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            BENIGN_RESET_COUNT.fetch_add(1, Ordering::Relaxed);
            let mut guard = poisoned.into_inner();
            *guard = T::default();
            lock.clear_poison();
            guard
        }
    }
}

pub(crate) fn read_benign_rwlock<'a, T: Default>(
    lock: &'a RwLock<T>,
    _name: &'static str,
) -> RwLockReadGuard<'a, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            drop(poisoned.into_inner());
            BENIGN_RESET_COUNT.fetch_add(1, Ordering::Relaxed);
            let mut guard = match lock.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = T::default();
            lock.clear_poison();
            drop(guard);
            lock.read()
                .expect("benign RwLock was reset and poison was cleared")
        }
    }
}

pub(crate) fn write_benign_rwlock<'a, T: Default>(
    lock: &'a RwLock<T>,
    _name: &'static str,
) -> RwLockWriteGuard<'a, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            BENIGN_RESET_COUNT.fetch_add(1, Ordering::Relaxed);
            let mut guard = poisoned.into_inner();
            *guard = T::default();
            lock.clear_poison();
            guard
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn authoritative_poison_is_typed_and_measured() {
        let lock = Arc::new(Mutex::new(11_u8));
        let poisoned = Arc::clone(&lock);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("inject authority poison");
        })
        .join();
        let before = runtime_lock_poison_metrics_snapshot().authoritative_poison;

        let panic = std::panic::catch_unwind(|| {
            drop(lock_authoritative_mutex(&lock, "test.runtime_authority"));
        })
        .expect_err("authoritative poison must fail-stop");

        assert_eq!(
            authoritative_lock_poison_from_panic(panic.as_ref()),
            Some("test.runtime_authority")
        );
        assert!(runtime_lock_poison_metrics_snapshot().authoritative_poison > before);
        assert!(lock.is_poisoned());
    }

    #[test]
    fn benign_mutex_poison_resets_and_clears_poison() {
        let lock = Arc::new(Mutex::new(vec![1_u8, 2]));
        let poisoned = Arc::clone(&lock);
        let _ = std::thread::spawn(move || {
            let mut guard = poisoned.lock().unwrap();
            guard.push(3);
            panic!("inject telemetry poison");
        })
        .join();
        let before = runtime_lock_poison_metrics_snapshot().benign_reset;

        assert!(lock_benign_mutex(&lock, "test.telemetry").is_empty());
        assert!(!lock.is_poisoned());
        assert!(runtime_lock_poison_metrics_snapshot().benign_reset > before);
    }

    #[test]
    fn benign_rwlock_poison_resets_cache_without_deadlock() {
        let lock = Arc::new(RwLock::new(HashMap::from([(1_u8, 2_u8)])));
        let poisoned = Arc::clone(&lock);
        let _ = std::thread::spawn(move || {
            let mut guard = poisoned.write().unwrap();
            guard.insert(3, 4);
            panic!("inject cache poison");
        })
        .join();

        assert!(read_benign_rwlock(&lock, "test.cache").is_empty());
        write_benign_rwlock(&lock, "test.cache").insert(5, 6);
        assert_eq!(read_benign_rwlock(&lock, "test.cache").get(&5), Some(&6));
        assert!(!lock.is_poisoned());
    }
}
