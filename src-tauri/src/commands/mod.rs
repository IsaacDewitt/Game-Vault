pub mod games;
pub mod stats;
pub mod settings;

use std::sync::{Arc, Mutex};

/// 获取 Mutex 锁，中毒时自动恢复而非 panic
pub(crate) fn lock_or_recover<T>(mutex: &Arc<Mutex<T>>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
