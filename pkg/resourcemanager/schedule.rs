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

// 资源管理器的周期调度实现：遍历已注册池，决策并执行调容命令。
//
// `schedule` 跳过 DistTask 组件；`schedulePool` 依次询问各 Scheduler 得到
// Overclock/Downclock/Hold；`Exec` 在满足最小调容间隔后真正修改池并发度，
// 且 Overclock 受 `MaxOverclockCount` 上限约束。

#![allow(non_snake_case)]

use std::sync::Arc;

use crate::rm::ResourceManager;
use crate::scheduler::Command;
use crate::util::{self, PoolContainer};

impl ResourceManager {
    /// 遍历全部已注册池并执行一次调度调容。
    #[doc(hidden)]
    pub fn schedule(&self) {
        let pool_map = Arc::clone(
            &self
                .inner
                .poolMap
                .read()
                .expect("resource manager pool map lock poisoned"),
        );
        pool_map.Iter(|pool| {
            // DistTask（分布式任务）组件不参与本机资源管理器调容。
            if pool.Component == util::DistTask {
                return;
            }
            let command = self.schedulePool(pool);
            self.Exec(pool, command);
        });
    }

    /// 对单个池询问调度器，返回应执行的 Command。
    #[doc(hidden)]
    pub fn schedulePool(&self, pool: &PoolContainer) -> Command {
        // 无运行中的 worker 时保持不变。
        if pool.Pool.Running() == 0 {
            return Command::Hold;
        }
        for scheduler in &self.inner.scheduler {
            let command = scheduler.Tune(pool.Component, pool.Pool.as_ref());
            if command == Command::Hold {
                continue;
            }
            // 容量已为 1 或实际运行数超过容量时，跳过 Downclock，避免过度缩容。
            if command == Command::Downclock
                && (pool.Pool.Cap() == 1 || pool.Pool.Running() > pool.Pool.Cap())
            {
                continue;
            }
            return command;
        }
        Command::Hold
    }

    // Exec applies a scheduler command when the pool's tuning interval permits.
    /// 在满足最小调容间隔时应用调度命令，实际增减池并发度。
    pub fn Exec(&self, pool: &PoolContainer, command: Command) {
        if command == Command::Hold {
            return;
        }
        let since_last_tune = pool.Pool.LastTunerTs().elapsed().unwrap_or_default();
        // 距上次调容时间过短则跳过，避免频繁抖动。
        if since_last_tune <= util::MinSchedulerInterval.Load() {
            return;
        }

        let current = pool.Pool.Cap();
        match command {
            Command::Downclock => {
                // Go 的 int32 运行时算术在边界回绕。
                let concurrency = current.wrapping_sub(1);
                log::debug!(
                    "downclock goroutine pool: category=resource manager origin_concurrency={current} concurrency={concurrency} name={}",
                    pool.Pool.Name()
                );
                pool.Pool.Tune(concurrency);
            }
            Command::Overclock => {
                let concurrency = current.wrapping_add(1);
                // 超过原始并发 + MaxOverclockCount 则拒绝继续升容。
                if concurrency
                    > pool
                        .Pool
                        .GetOriginConcurrency()
                        .wrapping_add(util::MaxOverclockCount)
                {
                    return;
                }
                log::debug!(
                    "overclock goroutine pool: category=resource manager origin_concurrency={current} concurrency={concurrency} name={}",
                    pool.Pool.Name()
                );
                pool.Pool.Tune(concurrency);
            }
            Command::Hold => {}
        }
    }
}
