// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// SQL 级 CPU 用量（TiDB/TiKV）的记录与并发合并。
//
// 对应 Go `pkg/util/ppcpuusage`。`CPUUsages` 保存两端 CPU 时间；
// `SQLCPUUsages` 用互斥锁保护 sqlID 与用量，供 profiler/executor 并发更新。

#![allow(non_snake_case)]

use std::sync::Mutex;
use std::time::Duration;

// CPUUsages records tidb/tikv cpu usages
// CPUUsages 对应 Go 的同名结构体，按原字段顺序记录 TiDB 与 TiKV 的 CPU 时间。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// TiDB 与 TiKV 两侧的 CPU 时间累计。
pub struct CPUUsages {
    // 对应 Go 导出字段 TidbCPUTime time.Duration。
    /// TiDB 侧累计 CPU 时间。
    pub TidbCPUTime: Duration,
    // 对应 Go 导出字段 TikvCPUTime time.Duration，字段名保持 Go 形状以便逐行对照。
    /// TiKV 侧累计 CPU 时间。
    pub TikvCPUTime: Duration,
}

// SQLCPUUsages is used to record sqlID and its cpu usages
// SQLCPUUsages 对应 Go 的同名结构体，用来把当前 sqlID 与 CPU 用量绑定在一起。
/// 绑定当前 sqlID 的 CPU 用量容器（内部加锁）。
pub struct SQLCPUUsages {
    // Go 结构体里嵌入 sync.Mutex；用 Mutex 包住受保护字段，
    // 表达原代码所有方法先 Lock、最后 Unlock 的并发语义。
    inner: Mutex<SQLCPUUsagesInner>,
}

// SQLCPUUsagesInner 是 Rust 迁移辅助结构，不是 Go 里的独立声明。
// 它把原 SQLCPUUsages 的 sqlID 与 cpuUsages 放到同一个 Mutex 保护范围内。
#[derive(Clone, Copy, Debug, Default)]
struct SQLCPUUsagesInner {
    // 对应 Go 私有字段 sqlID uint64，用于过滤来自旧 SQL 的 TiDB CPU 时间更新。
    sqlID: u64,
    // 对应 Go 私有字段 cpuUsages CPUUsages，记录当前 SQL 的 CPU 用量。
    cpuUsages: CPUUsages,
}

// Go 的 SQLCPUUsages 零值可直接使用；Rust 用 Default 提供相同的零值初始化。
impl Default for SQLCPUUsages {
    fn default() -> Self {
        Self {
            inner: Mutex::new(SQLCPUUsagesInner::default()),
        }
    }
}

impl CPUUsages {
    // Reset resets all cpu times to 0
    // Reset 对应 Go 的 (*CPUUsages).Reset，把 TiDB/TiKV 两个 CPU 时间清零。
    /// 将 TiDB/TiKV 两个 CPU 时间清零。
    pub fn Reset(&mut self) {
        // Go 中给 time.Duration 赋值 0；Rust 里用 Duration::ZERO 表达同样的零时长。
        self.TikvCPUTime = Duration::ZERO;
        self.TidbCPUTime = Duration::ZERO;
    }
}

impl SQLCPUUsages {
    // SetCPUUsages sets cpu usages value
    // SetCPUUsages 对应 Go 方法，在持锁状态下整体替换 cpuUsages。
    /// 在持锁状态下整体替换当前 CPU 用量。
    pub fn SetCPUUsages(&self, usage: CPUUsages) {
        // MutexGuard 的生命周期对应 Go 里的 defer c.Unlock()，离开作用域时自动释放锁。
        let mut inner = self
            .inner
            .lock()
            .expect("SQLCPUUsages mutex poisoned while setting CPU usages");
        inner.cpuUsages = usage;
    }

    // MergeTidbCPUTime merges tidbCPU time into self when sqlID matches
    // Checks sqlID here, because tidb cpu time can only be collected by profiler now, and updated in concurrent goroutines
    // MergeTidbCPUTime 对应 Go 方法：只有传入 sqlID 与当前 sqlID 相等时才合并 TiDB CPU 时间。
    /// 仅当 sqlID 匹配时合并 TiDB CPU 时间（profiler 并发更新场景）。
    pub fn MergeTidbCPUTime(&self, sqlID: u64, d: Duration) {
        // 原 Go 注释说明 TiDB CPU 时间来自 profiler，可能由并发 goroutine 更新，所以这里必须持锁。
        let mut inner = self
            .inner
            .lock()
            .expect("SQLCPUUsages mutex poisoned while merging TiDB CPU time");
        if inner.sqlID == sqlID {
            // 保留 Go 的 += 形状。
            inner.cpuUsages.TidbCPUTime += d;
        }
    }

    // MergeTikvCPUTime merges tikvCPU time into self.
    // Doesn't need to check sqlID here, because tikv cpu time is updated in executors now.
    // MergeTikvCPUTime 对应 Go 方法：TiKV CPU 时间由 executor 更新，因此原代码不检查 sqlID。
    /// 合并 TiKV CPU 时间（由 executor 更新，不校验 sqlID）。
    pub fn MergeTikvCPUTime(&self, d: Duration) {
        // 与 Go 的 Lock/defer Unlock 对应；这里的临界区只覆盖一次 TiKV CPU 时间累加。
        let mut inner = self
            .inner
            .lock()
            .expect("SQLCPUUsages mutex poisoned while merging TiKV CPU time");
        inner.cpuUsages.TikvCPUTime += d;
    }

    // GetCPUUsages returns tidbCPU, tikvCPU time
    // GetCPUUsages 对应 Go 方法，在锁保护下复制并返回当前 CPUUsages。
    /// 返回当前 CPU 用量的值拷贝。
    pub fn GetCPUUsages(&self) -> CPUUsages {
        // CPUUsages 是 Copy 类型，因此这里对应 Go 直接 return c.cpuUsages 的值拷贝。
        let inner = self
            .inner
            .lock()
            .expect("SQLCPUUsages mutex poisoned while getting CPU usages");
        inner.cpuUsages
    }

    // AllocNewSQLID alloc new ID, will restart from 0 when exceeds uint64 max limit
    // AllocNewSQLID 对应 Go 方法：递增 sqlID，并在 uint64 达到上限后回绕到 0。
    /// 分配新的 sqlID（uint64 回绕语义与 Go 一致）。
    pub fn AllocNewSQLID(&self) -> u64 {
        // Go 的 uint64 加法自然回绕；Rust 用 wrapping_add 明确保留该语义。
        let mut inner = self
            .inner
            .lock()
            .expect("SQLCPUUsages mutex poisoned while allocating SQL ID");
        inner.sqlID = inner.sqlID.wrapping_add(1);
        inner.sqlID
    }

    // ResetCPUTimes resets tidb/tikv cpu times to 0
    // ResetCPUTimes 对应 Go 方法，在持锁状态下复用 CPUUsages.Reset 清零两个 CPU 时间。
    /// 在持锁状态下清零两端 CPU 时间。
    pub fn ResetCPUTimes(&self) {
        // Go defer Unlock 在 Rust 中由 guard 作用域结束自动完成，避免手写资源收尾。
        let mut inner = self
            .inner
            .lock()
            .expect("SQLCPUUsages mutex poisoned while resetting CPU times");
        inner.cpuUsages.Reset();
    }
}
