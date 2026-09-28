// Copyright 2026 AsterSQL.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// 串行化会修改 driver 进程级配置的测试，避免并行测试互相覆盖。
pub(crate) fn global_state_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
