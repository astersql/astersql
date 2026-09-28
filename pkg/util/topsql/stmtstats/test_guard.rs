// Copyright 2026 AsterSQL.

// stmtstats 测试互斥守卫：串行化依赖全局状态的单测，避免并发互相干扰。

use std::sync::{Mutex, MutexGuard};

/// 获取进程级静态互斥锁；中毒时仍取出内层守卫以继续测试。
pub(super) fn stmtstats_guard() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    // 若前序测试 panic 导致锁中毒，仍恢复守卫以免后续用例永久阻塞。
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
