//! Small shared helpers.

use std::sync::{Mutex, MutexGuard};

/// Lock a mutex, recovering the guard even if a previous holder panicked and
/// poisoned it.
///
/// A poisoned lock means *some thread crashed while holding it*, not that the
/// guarded data is unusable. `lock().unwrap()` would then panic on every later
/// caller, turning one isolated crash into a cascade that takes the whole app
/// down. Recovering the guard keeps the app responsive; the panic hook in
/// `logging` has already recorded the original failure for the bug report.
pub trait LockExt<T> {
    fn lock_safe(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_safe(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
