// DXF Framework Mock 包入口。
//
// 提供 GoMock 风格的可控替身：`Handler` 包装闭包期望，并导出 plan / scheduler /
// storage_manager / task_executor 等 mock 模块，供调度与执行侧单元测试注入依赖。
// Copyright 2026 AsterSQL.

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 可配置期望的回调容器：保存闭包并统计调用次数。
pub struct Handler<F: ?Sized> {
    /// 用户通过 `set` 注入的期望闭包。
    callback: Mutex<Option<Box<F>>>,
    /// 已成功派发的调用次数（SeqCst）。
    calls: AtomicUsize,
}

/// 默认无期望、调用计数为 0。
impl<F: ?Sized> Default for Handler<F> {
    fn default() -> Self {
        Self {
            callback: Mutex::new(None),
            calls: AtomicUsize::new(0),
        }
    }
}

/// 设置期望、查询计数与派发调用。
impl<F: ?Sized> Handler<F> {
    /// 安装期望闭包并重置调用计数。
    pub fn set(&self, callback: Box<F>) {
        *self.callback.lock().expect("mock handler lock poisoned") = Some(callback);
        self.calls.store(0, Ordering::SeqCst);
    }

    /// 返回已派发次数。
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// 取出期望并调用；若未设置期望则 panic（方法名写入消息）。
    pub(crate) fn invoke<R>(&self, method: &str, invoke: impl FnOnce(&mut F) -> R) -> R {
        let mut callback = self
            .callback
            .lock()
            .expect("mock handler lock poisoned")
            .take()
            .unwrap_or_else(|| panic!("mock method {method} called without an expectation"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = invoke(callback.as_mut());

        // Do not hold the callback mutex while user code runs.  A callback may
        // inspect or replace its own expectation; preserve such a replacement
        // instead of restoring the callback that just finished.
        let mut installed = self.callback.lock().expect("mock handler lock poisoned");
        if installed.is_none() {
            *installed = Some(callback);
        }
        result
    }
}

// 逻辑计划 / PipelineSpec mock。
mod plan_mock;
// 调度器、清理例程、TaskManager mock。
mod scheduler_mock;
// 存储 Manager mock。
mod storage_manager_mock;
// TaskTable / TaskExecutor / Extension mock。
mod task_executor_mock;

pub use plan_mock::*;
pub use scheduler_mock::*;
pub use storage_manager_mock::*;
pub use task_executor_mock::*;

#[cfg(test)]
mod migration_aster_unit_test;
