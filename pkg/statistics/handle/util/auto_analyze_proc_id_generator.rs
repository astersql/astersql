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

// 自动分析（auto analyze）进程 ID 的分配、释放与全局跟踪。
//
// 为后台 ANALYZE 任务申请伪进程 ID，便于 `SHOW PROCESSLIST` 等路径展示；
// 通过全局列表与回调同步跟踪正在运行的自动分析任务。

use crate::util::StatsError;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, RwLock};

/// 自动分析进程 ID 生成器：申请与释放 ID。
pub trait AutoAnalyzeProcIdGenerator: Send + Sync {
    fn auto_analyze_proc_id(&self) -> u64;
    fn release_auto_analyze_proc_id(&self, id: u64);
}

/// 基于外部 getter/release 闭包的生成器实现。
pub struct Generator {
    getter: Arc<dyn Fn() -> u64 + Send + Sync>,
    release: Arc<dyn Fn(u64) + Send + Sync>,
}

impl Generator {
    /// 用自定义的分配与释放回调构造生成器。
    pub fn new(
        getter: impl Fn() -> u64 + Send + Sync + 'static,
        release: impl Fn(u64) + Send + Sync + 'static,
    ) -> Self {
        Self {
            getter: Arc::new(getter),
            release: Arc::new(release),
        }
    }
}

impl AutoAnalyzeProcIdGenerator for Generator {
    fn auto_analyze_proc_id(&self) -> u64 {
        (self.getter)()
    }

    fn release_auto_analyze_proc_id(&self, id: u64) {
        (self.release)(id);
    }
}

/// 构造 trait 对象形式的进程 ID 生成器。
pub fn new_generator(
    getter: impl Fn() -> u64 + Send + Sync + 'static,
    release: impl Fn(u64) + Send + Sync + 'static,
) -> Arc<dyn AutoAnalyzeProcIdGenerator> {
    Arc::new(Generator::new(getter, release))
}

/// 跟踪自动分析任务时附带的库表与语句上下文。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackProc {
    pub database: String,
    pub table: String,
    pub statement: String,
}

/// 进程内全局的自动分析进程 ID 集合，供查询与去重。
#[derive(Default)]
pub struct GlobalAutoAnalyzeProcessList {
    processes: RwLock<HashSet<u64>>,
}

impl GlobalAutoAnalyzeProcessList {
    /// 将 ID 加入全局跟踪集合。
    pub fn track(&self, id: u64) {
        self.processes.write().unwrap().insert(id);
    }

    /// 从全局跟踪集合移除 ID。
    pub fn untrack(&self, id: u64) {
        self.processes.write().unwrap().remove(&id);
    }

    /// 返回当前全部已跟踪的进程 ID。
    pub fn all(&self) -> Vec<u64> {
        self.processes.read().unwrap().iter().copied().collect()
    }

    /// 判断 ID 是否仍在全局跟踪集合中。
    pub fn contains(&self, id: u64) -> bool {
        self.processes.read().unwrap().contains(&id)
    }
}

/// 进程级单例：所有自动分析任务共享的进程 ID 列表。
pub static GLOBAL_AUTO_ANALYZE_PROCESS_LIST: LazyLock<GlobalAutoAnalyzeProcessList> =
    LazyLock::new(GlobalAutoAnalyzeProcessList::default);

type TrackCallback = dyn Fn(u64, TrackProc) -> Result<(), StatsError> + Send + Sync;
type UntrackCallback = dyn Fn(u64) + Send + Sync;

/// 自动分析任务跟踪器：先更新全局列表，再调用外部回调（如注册到 processlist）。
pub struct AutoAnalyzeTracker {
    track_callback: Arc<TrackCallback>,
    untrack_callback: Arc<UntrackCallback>,
}

impl AutoAnalyzeTracker {
    /// 用 track/untrack 回调构造跟踪器。
    pub fn new(
        track: impl Fn(u64, TrackProc) -> Result<(), StatsError> + Send + Sync + 'static,
        untrack: impl Fn(u64) + Send + Sync + 'static,
    ) -> Self {
        Self {
            track_callback: Arc::new(track),
            untrack_callback: Arc::new(untrack),
        }
    }

    /// 开始跟踪指定进程 ID。
    ///
    /// Go intentionally records the ID before invoking the callback. If the
    /// callback fails, the caller still has to untrack it explicitly.
    /// 与 Go 一致：先写入全局列表再调回调；回调失败时调用方仍需显式 untrack。
    pub fn track(&self, id: u64, context: TrackProc) -> Result<(), StatsError> {
        // Go intentionally records the ID before invoking the callback. If the
        // callback fails, the caller still has to untrack it explicitly.
        GLOBAL_AUTO_ANALYZE_PROCESS_LIST.track(id);
        (self.track_callback)(id, context)
    }

    /// 停止跟踪：先从全局列表移除，再调用外部 untrack 回调。
    pub fn untrack(&self, id: u64) {
        GLOBAL_AUTO_ANALYZE_PROCESS_LIST.untrack(id);
        (self.untrack_callback)(id);
    }
}

/// 便捷构造 `AutoAnalyzeTracker`。
pub fn new_auto_analyze_tracker(
    track: impl Fn(u64, TrackProc) -> Result<(), StatsError> + Send + Sync + 'static,
    untrack: impl Fn(u64) + Send + Sync + 'static,
) -> AutoAnalyzeTracker {
    AutoAnalyzeTracker::new(track, untrack)
}
