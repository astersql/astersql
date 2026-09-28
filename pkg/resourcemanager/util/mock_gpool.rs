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

// 测试用 Mock 协程池（GoroutinePool）。
//
// 仅实现名称、容量调谐与原始并发度等调度器常用接口；
// 运行时统计类方法（InFlight、RTT 等）保持 `implement me` panic，
// 与 Go 侧 mock 行为对齐。

#![allow(non_snake_case)]

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, SystemTime};

use crate::util::GoroutinePool;

/// 可调谐并发度的内存 Mock 池，用于单元测试与调度器联调。
pub struct MockGPool {
    /// 池名称，供资源管理器按名注册/查找。
    name: String,
    /// 当前并发容量（可被 Tune 修改）。
    concurrency: AtomicI32,
    /// 创建时的原始并发度，Tune 后不变。
    origin_concurrency: i32,
}

/// 创建指定名称与初始并发度的 Mock 池。
pub fn NewMockGPool(name: String, concurrency: i32) -> MockGPool {
    MockGPool {
        name,
        concurrency: AtomicI32::new(concurrency),
        origin_concurrency: concurrency,
    }
}

impl MockGPool {
    /// 释放并等待池内任务结束；Mock 未实现。
    pub fn ReleaseAndWait(&self) {
        panic!("implement me");
    }

    /// 将当前并发容量调整为 `concurrency`（超频/缩容由调度器触发）。
    pub fn Tune(&self, concurrency: i32) {
        self.concurrency.store(concurrency, Ordering::SeqCst);
    }

    /// 返回「最近一次调谐时间」的 Mock 值：当前时间减 10 秒，
    /// 使调度器认为距上次调谐已足够久。
    pub fn LastTunerTs(&self) -> SystemTime {
        SystemTime::now() - Duration::from_secs(10)
    }

    /// 最大飞行中任务数；Mock 未实现。
    pub fn MaxInFlight(&self) -> i64 {
        panic!("implement me");
    }

    /// 当前飞行中任务数；Mock 未实现。
    pub fn InFlight(&self) -> i64 {
        panic!("implement me");
    }

    /// 最小响应时间（RTT）；Mock 未实现。
    pub fn MinRT(&self) -> u64 {
        panic!("implement me");
    }

    /// 最大 PASS 指标；Mock 未实现。
    pub fn MaxPASS(&self) -> u64 {
        panic!("implement me");
    }

    /// 当前池容量（并发上限）。
    pub fn Cap(&self) -> i32 {
        self.concurrency.load(Ordering::SeqCst)
    }

    /// 长期 RTT 估计；Mock 未实现。
    pub fn LongRTT(&self) -> f64 {
        panic!("implement me");
    }

    /// 用闭包更新长期 RTT；Mock 未实现。
    pub fn UpdateLongRTT<F>(&self, _update: F)
    where
        F: FnOnce(f64) -> f64,
    {
        panic!("implement me");
    }

    /// 短期 RTT；Mock 未实现。
    pub fn ShortRTT(&self) -> u64 {
        panic!("implement me");
    }

    /// 排队任务数；Mock 未实现。
    pub fn GetQueueSize(&self) -> i64 {
        panic!("implement me");
    }

    /// 正在运行的协程数；Mock 未实现。
    pub fn Running(&self) -> i32 {
        panic!("implement me");
    }

    /// 返回池名称。
    pub fn Name(&self) -> &str {
        &self.name
    }

    /// 返回创建时记录的原始并发度。
    pub fn GetOriginConcurrency(&self) -> i32 {
        self.origin_concurrency
    }
}

/// 将 MockGPool 适配为 `GoroutinePool` trait，供资源管理器统一调度。
impl GoroutinePool for MockGPool {
    fn ReleaseAndWait(&self) {
        MockGPool::ReleaseAndWait(self);
    }

    fn Tune(&self, size: i32) {
        MockGPool::Tune(self, size);
    }

    fn LastTunerTs(&self) -> SystemTime {
        MockGPool::LastTunerTs(self)
    }

    fn Cap(&self) -> i32 {
        MockGPool::Cap(self)
    }

    fn Running(&self) -> i32 {
        MockGPool::Running(self)
    }

    fn Name(&self) -> &str {
        MockGPool::Name(self)
    }

    fn GetOriginConcurrency(&self) -> i32 {
        MockGPool::GetOriginConcurrency(self)
    }
}
