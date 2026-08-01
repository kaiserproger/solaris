use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::lock_policy::lock_authoritative_mutex;

#[derive(Clone)]
pub(crate) struct PreAuthAdmission {
    permits: Arc<Semaphore>,
    per_ip_limit: usize,
    by_ip: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl PreAuthAdmission {
    #[must_use]
    pub(crate) fn new(global_limit: usize, per_ip_limit: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(global_limit.max(1))),
            per_ip_limit: per_ip_limit.max(1),
            by_ip: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }

    pub(crate) fn try_acquire(&self, ip: IpAddr) -> Option<PreAuthPermit> {
        let permit = Arc::clone(&self.permits).try_acquire_owned().ok()?;
        let mut by_ip = lock_authoritative_mutex(&self.by_ip, "network.pre_auth_by_ip");
        let current = by_ip.get(&ip).copied().unwrap_or(0);
        if current >= self.per_ip_limit {
            return None;
        }
        by_ip.insert(ip, current + 1);
        drop(by_ip);
        Some(PreAuthPermit {
            _permit: permit,
            ip,
            by_ip: Arc::clone(&self.by_ip),
        })
    }
}

pub(crate) struct PreAuthPermit {
    _permit: OwnedSemaphorePermit,
    ip: IpAddr,
    by_ip: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl Drop for PreAuthPermit {
    fn drop(&mut self) {
        let mut by_ip = lock_authoritative_mutex(&self.by_ip, "network.pre_auth_by_ip");
        match by_ip.get_mut(&self.ip) {
            Some(count) if *count > 1 => *count -= 1,
            Some(_) => {
                by_ip.remove(&self.ip);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_global_and_per_ip_limits_and_recovers_on_drop() {
        let admission = PreAuthAdmission::new(3, 2);
        let first_ip: IpAddr = "127.0.0.1".parse().unwrap();
        let second_ip: IpAddr = "127.0.0.2".parse().unwrap();

        let first = admission.try_acquire(first_ip).unwrap();
        let second = admission.try_acquire(first_ip).unwrap();
        assert!(admission.try_acquire(first_ip).is_none());
        let third = admission.try_acquire(second_ip).unwrap();
        assert_eq!(admission.available_permits(), 0);
        assert!(admission.try_acquire(second_ip).is_none());

        drop(first);
        let replacement = admission.try_acquire(first_ip).unwrap();
        drop((second, third, replacement));
        assert_eq!(admission.available_permits(), 3);
    }
}
