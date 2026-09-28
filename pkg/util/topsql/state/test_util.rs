// Copyright 2026 AsterSQL.

// TopSQL 状态测试共享工具：串行化进程级全局状态并统一复位。

use std::sync::{Mutex, MutexGuard};

use crate::{DisableTopRU, DisableTopSQL, ResetTopRUItemInterval, TopRUEnabled};

/// 串行化所有修改全局 TopSQL / TopRU 状态的本 crate 测试。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 获取全局状态互斥锁；毒锁时接管内容以保证后续测试可继续。
pub(crate) fn lock_global_state() -> MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 关闭 TopSQL / TopRU 并复位 RU 条目间隔。
pub(crate) fn reset_global_state() {
    DisableTopSQL();
    while TopRUEnabled() {
        DisableTopRU();
    }
    ResetTopRUItemInterval();
}
