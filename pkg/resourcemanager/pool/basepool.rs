// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 协程池公共基类：名称、任务 ID、上次调谐时间戳。
//
// 具体池（如 spool）在此基础上扩展容量、阻塞策略与任务执行；
// 资源管理器通过调谐时间戳判断升降档冷却间隔。

use std::{
    sync::{
        RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

/// 池已关闭时返回的哨兵错误文案（与 Go 一致）。
pub const ERR_POOL_CLOSED: &str = "this pool has been closed";
/// 并发已达上限且设置为阻塞模式时的过载错误文案。
pub const ERR_POOL_OVERLOAD: &str =
    "the number of concurrency has reached the upper limit and Block is set";
/// 构造参数非法（如容量为 0）时的错误文案。
pub const ERR_POOL_PARAMS_INVALID: &str = "the pool params are invalid";

/// 可命名、可调谐的池元数据基类。
pub struct BasePool {
    /// 最近一次成功调谐（Tune）的时间戳，供调度冷却判断。
    last_tune_ts: RwLock<SystemTime>,
    /// 池在资源管理器中的注册名。
    name: String,
    /// 任务 ID 生成器；`gen_task_id` 从 1 起递增并按 Go `uint64` 语义回绕。
    generator: AtomicU64,
}

impl BasePool {
    /// 构造空名称池，并将调谐时间戳初始化为当前时刻。
    pub fn new() -> Self {
        Self {
            last_tune_ts: RwLock::new(SystemTime::now()),
            name: String::new(),
            generator: AtomicU64::new(0),
        }
    }

    /// 设置池名称（注册到资源管理器前调用）。
    pub fn set_name(&mut self, name: String) {
        self.name = name;
    }

    /// 返回当前池名称。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 原子递增并返回任务 ID；溢出时与 Go `atomic.Uint64.Add` 一致地回绕。
    pub fn gen_task_id(&self) -> u64 {
        self.generator
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1)
    }

    /// 读取最近一次调谐时间戳（读锁中毒时仍返回内部值）。
    pub fn last_tuner_ts(&self) -> SystemTime {
        *self
            .last_tune_ts
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 写入调谐时间戳（写锁中毒时仍更新内部值）。
    pub fn set_last_tune_ts(&self, value: SystemTime) {
        *self
            .last_tune_ts
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = value;
    }
}

#[cfg(test)]
#[path = "basepool_test.rs"]
mod tests;
