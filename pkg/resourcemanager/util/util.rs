// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 资源管理器公共工具类型。
//
// 定义原子调度间隔、协程池（GoroutinePool）接口、池与组件的绑定容器，
// 以及 DDL / DistTask 等资源组件枚举，供调度器与 ShardPoolMap 使用。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// An atomic duration with the same nanosecond precision used by Go's
/// `atomic.Duration`.
/// 与 Go `atomic.Duration` 相同负载边界和纳秒精度的原子 Duration。
pub struct AtomicDuration(AtomicU64);

impl AtomicDuration {
    /// 以给定 Duration 构造（内部按纳秒存储）。
    pub const fn new(value: Duration) -> Self {
        Self(AtomicU64::new(value.as_nanos() as u64))
    }

    /// 原子读取当前 Duration。
    pub fn Load(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::SeqCst))
    }

    /// 原子写入 Duration。
    pub fn Store(&self, value: Duration) {
        self.0.store(value.as_nanos() as u64, Ordering::SeqCst);
    }
}

/// Minimum interval between two scheduling operations.
/// 两次调度操作之间的最小间隔。
pub static MinSchedulerInterval: AtomicDuration = AtomicDuration::new(Duration::from_millis(200));

/// Maximum number of goroutines by which a pool may be overclocked.
/// 池允许超频（临时加大并发）的最大协程增量。
pub static MaxOverclockCount: i32 = 1;

/// Rust counterpart of the Go `GoroutinePool` interface.
/// Go `GoroutinePool` 接口的 Rust 对应：调谐、容量与运行态查询。
pub trait GoroutinePool: Send + Sync {
    /// 释放池并等待进行中任务结束。
    fn ReleaseAndWait(&self);
    /// 将池容量调整为 `size`。
    fn Tune(&self, size: i32);
    /// 最近一次调谐时间戳。
    fn LastTunerTs(&self) -> SystemTime;
    /// 当前容量。
    fn Cap(&self) -> i32;
    /// 正在运行的任务/协程数。
    fn Running(&self) -> i32;
    /// 池名称。
    fn Name(&self) -> &str;
    /// 创建时的原始并发度。
    fn GetOriginConcurrency(&self) -> i32;
}

/// A pool and the resource-manager component that owns it.
/// 池实例及其所属资源管理组件的绑定。
pub struct PoolContainer {
    /// 具体协程池实现。
    pub Pool: Arc<dyn GoroutinePool>,
    /// 拥有该池的组件类别。
    pub Component: Component,
}

/// 资源管理器识别的组件类别（DDL、分布式任务等）。
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Component {
    /// 未知组件。
    UNKNOWN = 0,
    /// DDL（数据定义语言）相关后台任务池。
    DDL = 1,
    /// 分布式任务（DistTask）框架池。
    DistTask = 2,
    /// CHECK TABLE 等校验任务池。
    CheckTable = 3,
    /// IMPORT INTO 导入任务池。
    ImportInto = 4,
}

/// 未知组件常量别名。
pub const UNKNOWN: Component = Component::UNKNOWN;
/// DDL 组件常量别名。
pub const DDL: Component = Component::DDL;
/// DistTask 组件常量别名。
pub const DistTask: Component = Component::DistTask;
/// CheckTable 组件常量别名。
pub const CheckTable: Component = Component::CheckTable;
/// ImportInto 组件常量别名。
pub const ImportInto: Component = Component::ImportInto;
