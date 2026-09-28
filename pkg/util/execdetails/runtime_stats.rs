// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 执行器运行时统计（RuntimeStats）：按算子类型收集、合并与格式化执行信息。
//
// 对应 Go `runtime_stats.go`。`RuntimeStatsColl` 按 plan id 聚合根节点与 Cop 任务统计；
// Cop 指下推到 TiKV/TiFlash 的协处理器（coprocessor）任务。

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::time::Duration as StdDuration;

// TpBasicRuntimeStats is the tp for BasicRuntimeStats.
/// BasicRuntimeStats 类型编号。
pub const TpBasicRuntimeStats: i32 = 0;
// TpRuntimeStatsWithCommit is the tp for RuntimeStatsWithCommit.
/// RuntimeStatsWithCommit 类型编号。
pub const TpRuntimeStatsWithCommit: i32 = 1;
// TpRuntimeStatsWithConcurrencyInfo is the tp for RuntimeStatsWithConcurrencyInfo.
/// RuntimeStatsWithConcurrencyInfo 类型编号。
pub const TpRuntimeStatsWithConcurrencyInfo: i32 = 2;
// TpSnapshotRuntimeStats is the tp for SnapshotRuntimeStats.
/// SnapshotRuntimeStats 类型编号。
pub const TpSnapshotRuntimeStats: i32 = 3;
// TpHashJoinRuntimeStats is the tp for HashJoinRuntimeStats.
/// HashJoinRuntimeStats 类型编号。
pub const TpHashJoinRuntimeStats: i32 = 4;
// TpHashJoinRuntimeStatsV2 is the tp for hashJoinRuntimeStatsV2.
/// hashJoinRuntimeStatsV2 类型编号。
pub const TpHashJoinRuntimeStatsV2: i32 = 5;
// TpIndexLookUpJoinRuntimeStats is the tp for IndexLookUpJoinRuntimeStats.
/// IndexLookUpJoinRuntimeStats 类型编号。
pub const TpIndexLookUpJoinRuntimeStats: i32 = 6;
// TpRuntimeStatsWithSnapshot is the tp for RuntimeStatsWithSnapshot.
/// RuntimeStatsWithSnapshot 类型编号。
pub const TpRuntimeStatsWithSnapshot: i32 = 7;
// TpJoinRuntimeStats is the tp for JoinRuntimeStats.
/// JoinRuntimeStats 类型编号。
pub const TpJoinRuntimeStats: i32 = 8;
// TpSelectResultRuntimeStats is the tp for SelectResultRuntimeStats.
/// SelectResultRuntimeStats 类型编号。
pub const TpSelectResultRuntimeStats: i32 = 9;
// TpInsertRuntimeStat is the tp for InsertRuntimeStat
/// InsertRuntimeStat 类型编号。
pub const TpInsertRuntimeStat: i32 = 10;
// TpIndexLookUpRunTimeStats is the tp for IndexLookUpRunTimeStats
/// IndexLookUpRunTimeStats 类型编号。
pub const TpIndexLookUpRunTimeStats: i32 = 11;
// TpSlowQueryRuntimeStat is the tp for SlowQueryRuntimeStat
/// SlowQueryRuntimeStat 类型编号。
pub const TpSlowQueryRuntimeStat: i32 = 12;
// TpHashAggRuntimeStat is the tp for HashAggRuntimeStat
/// HashAggRuntimeStat 类型编号。
pub const TpHashAggRuntimeStat: i32 = 13;
// TpIndexMergeRunTimeStats is the tp for IndexMergeRunTimeStats
/// IndexMergeRunTimeStats 类型编号。
pub const TpIndexMergeRunTimeStats: i32 = 14;
// TpBasicCopRunTimeStats is the tp for BasicCopRunTimeStats
/// BasicCopRunTimeStats 类型编号。
pub const TpBasicCopRunTimeStats: i32 = 15;
// TpUpdateRuntimeStats is the tp for UpdateRuntimeStats
/// UpdateRuntimeStats 类型编号。
pub const TpUpdateRuntimeStats: i32 = 16;
// TpFKCheckRuntimeStats is the tp for FKCheckRuntimeStats
/// FKCheckRuntimeStats 类型编号。
pub const TpFKCheckRuntimeStats: i32 = 17;
// TpFKCascadeRuntimeStats is the tp for FKCascadeRuntimeStats
/// FKCascadeRuntimeStats 类型编号。
pub const TpFKCascadeRuntimeStats: i32 = 18;
// TpRURuntimeStats is the tp for RURuntimeStats
/// RURuntimeStats 类型编号。
pub const TpRURuntimeStats: i32 = 19;

// RuntimeStats is used to express the executor runtime information.
// Rust 额外加入 as_any 以表达 Go type assertion；后续真正编译时可替换为 enum 或 trait object downcast。
// `+ Sync`（Go 接口没有对应约束）是让 `ProcessInfo`/`Arc<ProcessInfo>` 能真正跨线程共享的
// 必要条件：`sessmgr::Manager` 要求实现者 `Send + Sync`，而 `ProcessInfo.RuntimeStatsColl`
// 一路持有 `Vec<Box<dyn RuntimeStats>>`，若trait 本身不是 `Sync`，`Arc<ProcessInfo>` 就永远
// 无法满足 `Send`（`Arc<T>: Send` 要求 `T: Send + Sync`），任何真实的 `Manager` 实现
// （包括 `pkg/util/servermemorylimit` 的后台线程用法）都会在这里被卡住。所有现有
// 实现者都是纯数据字段（计数、时长、百分位数），补 `Sync` 不改变任何 Go 语义。
/// 执行器运行时统计接口：克隆、合并、字符串化与类型编号。
pub trait RuntimeStats: Any + Send + Sync {
    fn String(&self) -> String;
    fn Merge(&mut self, other: &dyn RuntimeStats);
    fn CloneBox(&self) -> Box<dyn RuntimeStats>;
    fn Tp(&self) -> i32;
    fn as_any(&self) -> &dyn Any;
}

// basicCopRuntimeStats 对应 Go 的内部 cop task 基础统计。
#[derive(Clone, Default)]
/// 单个 Cop 任务的基础运行统计。
pub struct basicCopRuntimeStats {
    pub loopCount: i32,
    pub rows: i64,
    pub threads: i32,
    pub procTimes: Percentile<Duration>,
    // executor extra infos
    pub tiflashStats: Option<TiflashStats>,
}

impl basicCopRuntimeStats {
    // String implements the RuntimeStats interface.
    pub fn String(&self) -> String {
        let mut buf = String::with_capacity(16);
        buf.push_str("time:");
        buf.push_str(&FormatDuration(StdDuration::from_nanos(
            self.procTimes.Sum() as u64,
        )));
        buf.push_str(", loops:");
        buf.push_str(&self.loopCount.to_string());
        if let Some(tiflashStats) = self.tiflashStats.as_ref() {
            buf.push_str(", threads:");
            buf.push_str(&self.threads.to_string());
            if !tiflashStats.waitSummary.CanBeIgnored() {
                buf.push_str(", ");
                buf.push_str(&tiflashStats.waitSummary.String());
            }
            if !tiflashStats.networkSummary.Empty() {
                buf.push_str(", ");
                buf.push_str(&tiflashStats.networkSummary.String());
            }
            buf.push_str(", ");
            buf.push_str(&tiflashStats.scanContext.String());
        }
        buf
    }

    // Clone implements the RuntimeStats interface.
    pub fn Clone(&self) -> basicCopRuntimeStats {
        self.clone()
    }

    // Merge implements the RuntimeStats interface.
    pub fn Merge(&mut self, rs: &basicCopRuntimeStats) {
        self.loopCount += rs.loopCount;
        self.rows += rs.rows;
        self.threads += rs.threads;
        if rs.procTimes.Size() > 0 {
            self.procTimes.MergePercentile(&rs.procTimes);
        }
        if let Some(tmp) = rs.tiflashStats.as_ref() {
            let stats = self.tiflashStats.get_or_insert_with(TiflashStats::default);
            stats.scanContext.Merge(tmp.scanContext.Clone());
            stats
                .columnarScanContext
                .Merge(tmp.columnarScanContext.Clone());
            stats.waitSummary.Merge(tmp.waitSummary.Clone());
            stats.networkSummary.Merge(tmp.networkSummary.Clone());
        }
    }

    // mergeExecSummary likes Merge, but it merges ExecutorExecutionSummary directly.
    pub fn mergeExecSummary(&mut self, summary: &tipb::ExecutorExecutionSummary) {
        self.loopCount += summary.NumIterations.unwrap_or_default() as i32;
        self.rows += summary.NumProducedRows.unwrap_or_default() as i64;
        self.threads += summary.GetConcurrency() as i32;
        self.procTimes.Add(StdDuration::from_nanos(
            summary.TimeProcessedNs.unwrap_or_default(),
        ));
        // Go 按 protobuf 中 TiFlash 子 summary 是否存在来懒创建 tiflashStats。
        if let Some(tiflashScanContext) = summary.GetTiflashScanContext() {
            self.tiflashStats
                .get_or_insert_with(TiflashStats::default)
                .scanContext
                .mergeExecSummary(Some(tiflashScanContext));
        }
        if let Some(columnarScanContext) = summary.GetColumnarScanContext() {
            self.tiflashStats
                .get_or_insert_with(TiflashStats::default)
                .columnarScanContext
                .mergeExecSummary(Some(columnarScanContext));
        }
        if let Some(tiflashWaitSummary) = summary.GetTiflashWaitSummary() {
            self.tiflashStats
                .get_or_insert_with(TiflashStats::default)
                .waitSummary
                .mergeExecSummary(
                    Some(tiflashWaitSummary),
                    summary.TimeProcessedNs.unwrap_or_default(),
                );
        }
        if let Some(tiflashNetworkSummary) = summary.GetTiflashNetworkSummary() {
            self.tiflashStats
                .get_or_insert_with(TiflashStats::default)
                .networkSummary
                .mergeExecSummary(Some(tiflashNetworkSummary));
        }
    }

    // Tp implements the RuntimeStats interface.
    pub fn Tp(&self) -> i32 {
        TpBasicCopRunTimeStats
    }
}

// StmtCopRuntimeStats stores the cop runtime stats of the total statement
#[derive(Clone, Default)]
/// 整条语句累计的 Cop 运行统计。
pub struct StmtCopRuntimeStats {
    // TiflashNetworkStats stats all mpp tasks' network traffic info, nil if no any mpp tasks' network traffic
    pub TiflashNetworkStats: Option<TiFlashNetworkTrafficSummary>,
}

impl StmtCopRuntimeStats {
    // mergeExecSummary merges ExecutorExecutionSummary into stmt cop runtime stats directly.
    pub fn mergeExecSummary(&mut self, summary: &tipb::ExecutorExecutionSummary) {
        if let Some(tiflashNetworkSummary) = summary.GetTiflashNetworkSummary() {
            self.TiflashNetworkStats
                .get_or_insert_with(TiFlashNetworkTrafficSummary::default)
                .mergeExecSummary(Some(tiflashNetworkSummary));
        }
    }
}

// CopRuntimeStats collects cop tasks' execution info.
#[derive(Clone, Default)]
/// 收集并按 store 聚合 Cop 任务执行信息。
pub struct CopRuntimeStats {
    // stats stores the runtime statistics of coprocessor tasks.
    // The key of the map is the tikv-server address. Because a tikv-server can
    // have many region leaders, several coprocessor tasks can be sent to the
    // same tikv-server instance. We have to use a list to maintain all tasks
    // executed on each instance.
    pub stats: basicCopRuntimeStats,
    pub scanDetail: util::ScanDetail,
    pub timeDetail: util::TimeDetail,
    pub storeType: kv::StoreType,
}

// zeroTimeDetail 对应 Go 的包级零值。
/// 包级零值 TimeDetail。
pub static zeroTimeDetail: util::TimeDetail = util::TimeDetail {
    ProcessTime: StdDuration::ZERO,
    WaitTime: StdDuration::ZERO,
};

impl CopRuntimeStats {
    // GetActRows return total rows of CopRuntimeStats.
    pub fn GetActRows(&self) -> i64 {
        self.stats.rows
    }

    // GetTasks return total tasks of CopRuntimeStats
    pub fn GetTasks(&self) -> i32 {
        self.stats.procTimes.Size() as i32
    }

    // String 对应 Go 的 cop task runtime stats 格式化。
    pub fn String(&mut self) -> String {
        let mut procTimes = self.stats.procTimes.clone();
        let totalTasks = procTimes.Size();
        let isTiFlashCop = self.storeType == kv::TiFlash;
        let mut buf = String::with_capacity(16);
        let printTiFlashSpecificInfo = |buf: &mut String| {
            if isTiFlashCop {
                buf.push_str(", threads:");
                buf.push_str(&self.stats.threads.to_string());
                buf.push('}');
                if let Some(tiflashStats) = self.stats.tiflashStats.as_ref() {
                    if !tiflashStats.waitSummary.CanBeIgnored() {
                        buf.push_str(", ");
                        buf.push_str(&tiflashStats.waitSummary.String());
                    }
                    if !tiflashStats.networkSummary.Empty() {
                        buf.push_str(", ");
                        buf.push_str(&tiflashStats.networkSummary.String());
                    }
                    if !tiflashStats.columnarScanContext.Empty() {
                        buf.push_str(", ");
                        buf.push_str(&tiflashStats.columnarScanContext.String());
                    } else if !tiflashStats.scanContext.Empty() {
                        buf.push_str(", ");
                        buf.push_str(&tiflashStats.scanContext.String());
                    }
                }
            } else {
                buf.push('}');
            }
        };
        if totalTasks == 1 {
            buf.push_str(&self.storeType.Name());
            buf.push_str("_task:{time:");
            buf.push_str(&FormatDuration(StdDuration::from_nanos(
                procTimes.GetPercentile(0.0) as u64,
            )));
            buf.push_str(", loops:");
            buf.push_str(&self.stats.loopCount.to_string());
            printTiFlashSpecificInfo(&mut buf);
        } else if totalTasks > 0 {
            buf.push_str(&self.storeType.Name());
            buf.push_str("_task:{proc max:");
            buf.push_str(&FormatDuration(StdDuration::from_nanos(
                procTimes.GetMax().unwrap().GetFloat64() as u64,
            )));
            buf.push_str(", min:");
            buf.push_str(&FormatDuration(StdDuration::from_nanos(
                procTimes.GetMin().unwrap().GetFloat64() as u64,
            )));
            buf.push_str(", avg: ");
            buf.push_str(&FormatDuration(StdDuration::from_nanos(
                (procTimes.Sum() / totalTasks as f64) as u64,
            )));
            buf.push_str(", p80:");
            buf.push_str(&FormatDuration(StdDuration::from_nanos(
                procTimes.GetPercentile(0.8) as u64,
            )));
            buf.push_str(", p95:");
            buf.push_str(&FormatDuration(StdDuration::from_nanos(
                procTimes.GetPercentile(0.95) as u64,
            )));
            buf.push_str(", iters:");
            buf.push_str(&self.stats.loopCount.to_string());
            buf.push_str(", tasks:");
            buf.push_str(&totalTasks.to_string());
            printTiFlashSpecificInfo(&mut buf);
        }
        if !isTiFlashCop {
            let detail = self.scanDetail.String();
            if !detail.is_empty() {
                buf.push_str(", ");
                buf.push_str(&detail);
            }
            if self.timeDetail != zeroTimeDetail {
                let timeDetailStr = self.timeDetail.String();
                if !timeDetailStr.is_empty() {
                    buf.push_str(", ");
                    buf.push_str(&timeDetailStr);
                }
            }
        }
        buf
    }
}

// BasicRuntimeStats is the basic runtime stats.
#[derive(Default)]
/// 基础运行统计：循环次数、行数、耗时等。
pub struct BasicRuntimeStats {
    // the count of executors with the same id
    executorCount: AtomicI32,
    // executor's Next() called times.
    loopCount: AtomicI32,
    // executor consume time, including open, next, and close time.
    consume: AtomicI64,
    // executor open time.
    open: AtomicI64,
    // executor close time.
    close: AtomicI64,
    // executor return row count.
    rows: AtomicI64,
}

impl BasicRuntimeStats {
    // GetActRows return total rows of BasicRuntimeStats.
    pub fn GetActRows(&self) -> i64 {
        self.rows.load(Ordering::Relaxed)
    }

    // Clone implements the RuntimeStats interface.
    // BasicRuntimeStats shouldn't implement Clone interface because all executors with the same executor_id
    // should share the same BasicRuntimeStats, duplicated BasicRuntimeStats are easy to cause mistakes.
    pub fn Clone(&self) -> Box<dyn RuntimeStats> {
        panic!("BasicRuntimeStats should not implement Clone function")
    }

    // Merge implements the RuntimeStats interface.
    pub fn MergeBasic(&self, tmp: &BasicRuntimeStats) {
        self.loopCount
            .fetch_add(tmp.loopCount.load(Ordering::Relaxed), Ordering::Relaxed);
        self.consume
            .fetch_add(tmp.consume.load(Ordering::Relaxed), Ordering::Relaxed);
        self.open
            .fetch_add(tmp.open.load(Ordering::Relaxed), Ordering::Relaxed);
        self.close
            .fetch_add(tmp.close.load(Ordering::Relaxed), Ordering::Relaxed);
        self.rows
            .fetch_add(tmp.rows.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    // Tp implements the RuntimeStats interface.
    pub fn Tp(&self) -> i32 {
        TpBasicRuntimeStats
    }

    // Record records executor's execution.
    pub fn Record(&self, d: StdDuration, rowNum: i32) {
        self.loopCount.fetch_add(1, Ordering::Relaxed);
        self.consume
            .fetch_add(d.as_nanos() as i64, Ordering::Relaxed);
        self.rows.fetch_add(rowNum as i64, Ordering::Relaxed);
    }

    // RecordOpen records executor's open time.
    pub fn RecordOpen(&self, d: StdDuration) {
        self.consume
            .fetch_add(d.as_nanos() as i64, Ordering::Relaxed);
        self.open.fetch_add(d.as_nanos() as i64, Ordering::Relaxed);
    }

    // RecordClose records executor's close time.
    pub fn RecordClose(&self, d: StdDuration) {
        self.consume
            .fetch_add(d.as_nanos() as i64, Ordering::Relaxed);
        self.close.fetch_add(d.as_nanos() as i64, Ordering::Relaxed);
    }

    // SetRowNum sets the row num.
    pub fn SetRowNum(&self, rowNum: i64) {
        self.rows.store(rowNum, Ordering::Relaxed);
    }

    // String implements the RuntimeStats interface.
    pub fn String(&self) -> String {
        let mut str_builder = String::new();
        let mut timePrefix = "";
        if self.executorCount.load(Ordering::Relaxed) > 1 {
            timePrefix = "total_";
        }
        let totalTime = self.consume.load(Ordering::Relaxed);
        let openTime = self.open.load(Ordering::Relaxed);
        let closeTime = self.close.load(Ordering::Relaxed);
        str_builder.push_str(timePrefix);
        str_builder.push_str("time:");
        str_builder.push_str(&FormatDuration(StdDuration::from_nanos(totalTime as u64)));
        str_builder.push_str(", ");
        str_builder.push_str(timePrefix);
        str_builder.push_str("open:");
        str_builder.push_str(&FormatDuration(StdDuration::from_nanos(openTime as u64)));
        str_builder.push_str(", ");
        str_builder.push_str(timePrefix);
        str_builder.push_str("close:");
        str_builder.push_str(&FormatDuration(StdDuration::from_nanos(closeTime as u64)));
        str_builder.push_str(", loops:");
        str_builder.push_str(&self.loopCount.load(Ordering::Relaxed).to_string());
        str_builder
    }

    // GetTime get the int64 total time
    pub fn GetTime(&self) -> i64 {
        self.consume.load(Ordering::Relaxed)
    }
}

// RootRuntimeStats is the executor runtime stats that combine with multiple runtime stats.
#[derive(Default)]
/// 根执行器上多个 RuntimeStats 的组合。
pub struct RootRuntimeStats {
    basic: Option<BasicRuntimeStats>,
    groupRss: Vec<Box<dyn RuntimeStats>>,
}

// NewRootRuntimeStats returns a new RootRuntimeStats
/// 创建空的 RootRuntimeStats。
pub fn NewRootRuntimeStats() -> RootRuntimeStats {
    RootRuntimeStats::default()
}

impl RootRuntimeStats {
    // GetActRows return total rows of RootRuntimeStats.
    pub fn GetActRows(&self) -> i64 {
        self.basic
            .as_ref()
            .map(|basic| basic.rows.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    // MergeStats merges stats in the RootRuntimeStats and return the stats suitable for display directly.
    pub fn MergeStats(&self) -> (Option<&BasicRuntimeStats>, &Vec<Box<dyn RuntimeStats>>) {
        (self.basic.as_ref(), &self.groupRss)
    }

    // String implements the RuntimeStats interface.
    pub fn String(&self) -> String {
        let (basic, groups) = self.MergeStats();
        let mut strs = Vec::with_capacity(groups.len() + 1);
        if let Some(basic) = basic {
            strs.push(basic.String());
        }
        for group in groups {
            let str_value = group.String();
            if !str_value.is_empty() {
                strs.push(str_value);
            }
        }
        strs.join(", ")
    }
}

// RuntimeStatsColl collects executors's execution info.
#[derive(Default)]
/// 按 plan id 收集根节点与 Cop 运行时统计的集合。
pub struct RuntimeStatsColl {
    rootStats: HashMap<i32, RootRuntimeStats>,
    /// Stats registered through a shared Arc are merged into rootStats on owned access.
    sharedRootStats: Mutex<HashMap<i32, Vec<Box<dyn RuntimeStats>>>>,
    copStats: HashMap<i32, CopRuntimeStats>,
    stmtCopStats: StmtCopRuntimeStats,
    mu: Mutex<()>,
}

// NewRuntimeStatsColl creates new executor collector.
// Reuse the object to reduce allocation when *RuntimeStatsColl is not nil.
/// 新建或复用 RuntimeStatsColl。
pub fn NewRuntimeStatsColl(reuse: Option<RuntimeStatsColl>) -> RuntimeStatsColl {
    if let Some(mut reuse) = reuse {
        // Reuse map is cheaper than create a new map object.
        // Go compiler optimize this cleanup code pattern to a clearmap() function.
        {
            let _guard = reuse.mu.lock().expect("runtime stats coll lock poisoned");
            reuse.rootStats.clear();
            reuse
                .sharedRootStats
                .lock()
                .expect("shared runtime stats lock poisoned")
                .clear();
            reuse.copStats.clear();
        }
        return reuse;
    }
    RuntimeStatsColl {
        rootStats: HashMap::new(),
        sharedRootStats: Mutex::new(HashMap::new()),
        copStats: HashMap::new(),
        stmtCopStats: StmtCopRuntimeStats::default(),
        mu: Mutex::new(()),
    }
}

impl RuntimeStatsColl {
    /// Registers executor stats while the collector is shared by StmtCtx and process info.
    pub fn RegisterStatsShared(&self, planID: i32, info: Box<dyn RuntimeStats>) {
        let mut shared = self
            .sharedRootStats
            .lock()
            .expect("shared runtime stats lock poisoned");
        let groups = shared.entry(planID).or_default();
        if let Some(existing) = groups
            .iter_mut()
            .find(|existing| existing.Tp() == info.Tp())
        {
            existing.Merge(info.as_ref());
        } else {
            groups.push(info);
        }
    }

    /// Reads root stats including entries registered through the shared collector.
    pub fn GetRootStatsStringShared(&self, planID: i32) -> String {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        let empty = RootRuntimeStats::default();
        let root = self.rootStats.get(&planID).unwrap_or(&empty);
        let basic = root
            .basic
            .as_ref()
            .map(BasicRuntimeStats::String)
            .unwrap_or_default();
        let groups = self.shared_group_stats_string(planID, root);
        [basic, groups]
            .into_iter()
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn shared_group_stats_string(&self, planID: i32, root: &RootRuntimeStats) -> String {
        let mut combined = RootRuntimeStats::default();
        combined.groupRss = root.groupRss.iter().map(|group| group.CloneBox()).collect();
        let shared = self
            .sharedRootStats
            .lock()
            .expect("shared runtime stats lock poisoned");
        if let Some(groups) = shared.get(&planID) {
            for group in groups {
                if let Some(existing) = combined
                    .groupRss
                    .iter_mut()
                    .find(|existing| existing.Tp() == group.Tp())
                {
                    existing.Merge(group.as_ref());
                } else {
                    combined.groupRss.push(group.CloneBox());
                }
            }
        }
        combined.String()
    }

    // RegisterStats register execStat for a executor.
    pub fn RegisterStats(&mut self, planID: i32, info: Box<dyn RuntimeStats>) {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        let stats = self
            .rootStats
            .entry(planID)
            .or_insert_with(NewRootRuntimeStats);
        let tp = info.Tp();
        let mut found = false;
        for rss in stats.groupRss.iter_mut() {
            if rss.Tp() == tp {
                rss.Merge(info.as_ref());
                found = true;
                break;
            }
        }
        if !found {
            stats.groupRss.push(info);
        }
    }

    // GetBasicRuntimeStats gets basicRuntimeStats for a executor
    // When rootStat/rootStat's basicRuntimeStats is nil, the behavior is decided by initNewExecutorStats argument:
    // 1. If true, it created a new one, and increase basicRuntimeStats' executorCount
    // 2. Else, it returns nil
    pub fn GetBasicRuntimeStats(
        &mut self,
        planID: i32,
        initNewExecutorStats: bool,
    ) -> Option<&BasicRuntimeStats> {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        if !self.rootStats.contains_key(&planID) && initNewExecutorStats {
            self.rootStats.insert(planID, NewRootRuntimeStats());
        }
        let stats = self.rootStats.get_mut(&planID)?;
        if stats.basic.is_none() && initNewExecutorStats {
            stats.basic = Some(BasicRuntimeStats::default());
            stats
                .basic
                .as_ref()
                .unwrap()
                .executorCount
                .fetch_add(1, Ordering::Relaxed);
        } else if let Some(basic) = stats.basic.as_ref() {
            if initNewExecutorStats {
                basic.executorCount.fetch_add(1, Ordering::Relaxed);
            }
        }
        stats.basic.as_ref()
    }

    // GetStmtCopRuntimeStats gets execStat for a executor.
    pub fn GetStmtCopRuntimeStats(&self) -> StmtCopRuntimeStats {
        self.stmtCopStats.clone()
    }

    // GetRootStats gets execStat for a executor.
    pub fn GetRootStats(&mut self, planID: i32) -> &RootRuntimeStats {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        let root = self
            .rootStats
            .entry(planID)
            .or_insert_with(NewRootRuntimeStats);
        if let Some(groups) = self
            .sharedRootStats
            .lock()
            .expect("shared runtime stats lock poisoned")
            .remove(&planID)
        {
            for info in groups {
                if let Some(existing) = root
                    .groupRss
                    .iter_mut()
                    .find(|existing| existing.Tp() == info.Tp())
                {
                    existing.Merge(info.as_ref());
                } else {
                    root.groupRss.push(info);
                }
            }
        }
        root
    }

    // GetPlanActRows returns the actual rows of the plan.
    pub fn GetPlanActRows(&self, planID: i32) -> i64 {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        self.rootStats
            .get(&planID)
            .map(|stats| stats.GetActRows())
            .unwrap_or(0)
    }

    // GetCopStats gets the CopRuntimeStats specified by planID.
    pub fn GetCopStats(&self, planID: i32) -> Option<&CopRuntimeStats> {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        self.copStats.get(&planID)
    }

    // GetCopCountAndRows returns the total cop-tasks count and total rows of all cop-tasks.
    pub fn GetCopCountAndRows(&self, planID: i32) -> (i32, i64) {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        if let Some(copStats) = self.copStats.get(&planID) {
            return (copStats.GetTasks(), copStats.GetActRows());
        }
        (0, 0)
    }
}

// getPlanIDFromExecutionSummary 对应 Go 从 executor id 尾段解析 planID。
/// 从执行摘要解析 plan id；第二返回值表示是否成功。
pub fn getPlanIDFromExecutionSummary(summary: &tipb::ExecutorExecutionSummary) -> (i32, bool) {
    if !summary.GetExecutorId().is_empty() {
        let strs: Vec<&str> = summary.GetExecutorId().split('_').collect();
        if let Some(last) = strs.last() {
            if let Ok(id) = last.parse::<i32>() {
                return (id, true);
            }
        }
    }
    (0, false)
}

impl RuntimeStatsColl {
    // RecordCopStats records a specific cop tasks's execution detail.
    pub fn RecordCopStats(
        &mut self,
        mut planID: i32,
        storeType: kv::StoreType,
        scan: Option<&util::ScanDetail>,
        time: util::TimeDetail,
        summary: Option<&tipb::ExecutorExecutionSummary>,
    ) -> i32 {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        if let Some(copStats) = self.copStats.get_mut(&planID) {
            if let Some(scan) = scan {
                copStats.scanDetail.Merge(scan);
            }
            copStats.timeDetail.Merge(&time);
        } else {
            let mut stats = CopRuntimeStats {
                timeDetail: time.clone(),
                storeType,
                ..Default::default()
            };
            if let Some(scan) = scan {
                stats.scanDetail = scan.clone();
            }
            self.copStats.insert(planID, stats);
        }

        if let Some(summary) = summary {
            // for TiFlash cop response, ExecutorExecutionSummary contains executor id, so if there is a valid executor id in
            // summary, use it overwrite the planID
            let (id, valid) = getPlanIDFromExecutionSummary(summary);
            if valid && id != planID {
                planID = id;
                self.copStats
                    .entry(planID)
                    .or_insert_with(|| CopRuntimeStats {
                        storeType,
                        ..Default::default()
                    });
            }
            if let Some(copStats) = self.copStats.get_mut(&planID) {
                copStats.stats.mergeExecSummary(summary);
            }
            self.stmtCopStats.mergeExecSummary(summary);
        }
        planID
    }

    // RecordOneCopTask records a specific cop tasks's execution summary.
    pub fn RecordOneCopTask(
        &mut self,
        mut planID: i32,
        storeType: kv::StoreType,
        summary: &tipb::ExecutorExecutionSummary,
    ) -> i32 {
        // for TiFlash cop response, ExecutorExecutionSummary contains executor id, so if there is a valid executor id in
        // summary, use it overwrite the planID
        if let (id, true) = getPlanIDFromExecutionSummary(summary) {
            planID = id;
        }
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        let copStats = self
            .copStats
            .entry(planID)
            .or_insert_with(|| CopRuntimeStats {
                storeType,
                ..Default::default()
            });
        copStats.stats.mergeExecSummary(summary);
        self.stmtCopStats.mergeExecSummary(summary);
        planID
    }

    // ExistsRootStats checks if the planID exists in the rootStats collection.
    pub fn ExistsRootStats(&self, planID: i32) -> bool {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        self.rootStats.contains_key(&planID)
    }

    // ExistsCopStats checks if the planID exists in the copStats collection.
    pub fn ExistsCopStats(&self, planID: i32) -> bool {
        let _guard = self.mu.lock().expect("runtime stats coll lock poisoned");
        self.copStats.contains_key(&planID)
    }
}

// ConcurrencyInfo is used to save the concurrency information of the executor operator
#[derive(Clone)]
/// 并发度信息：名称与并发数。
pub struct ConcurrencyInfo {
    concurrencyName: String,
    concurrencyNum: i32,
}

// NewConcurrencyInfo creates new executor's concurrencyInfo.
/// 构造 ConcurrencyInfo。
pub fn NewConcurrencyInfo(name: String, num: i32) -> ConcurrencyInfo {
    ConcurrencyInfo {
        concurrencyName: name,
        concurrencyNum: num,
    }
}

// RuntimeStatsWithConcurrencyInfo is the BasicRuntimeStats with ConcurrencyInfo.
#[derive(Default)]
/// 附带并发信息的运行时统计。
pub struct RuntimeStatsWithConcurrencyInfo {
    // executor concurrency information
    concurrency: Vec<ConcurrencyInfo>,
    // protect concurrency
    mu: Mutex<()>,
}

impl RuntimeStatsWithConcurrencyInfo {
    // Tp implements the RuntimeStats interface.
    pub fn Tp(&self) -> i32 {
        TpRuntimeStatsWithConcurrencyInfo
    }

    // SetConcurrencyInfo sets the concurrency informations.
    // We must clear the concurrencyInfo first when we call the SetConcurrencyInfo.
    // When the num <= 0, it means the exector operator is not executed parallel.
    pub fn SetConcurrencyInfo(&mut self, infos: Vec<ConcurrencyInfo>) {
        let _guard = self.mu.lock().expect("concurrency info lock poisoned");
        self.concurrency.clear();
        self.concurrency.extend(infos);
    }

    // Clone implements the RuntimeStats interface.
    pub fn Clone(&self) -> RuntimeStatsWithConcurrencyInfo {
        RuntimeStatsWithConcurrencyInfo {
            concurrency: self.concurrency.clone(),
            mu: Mutex::new(()),
        }
    }

    // String implements the RuntimeStats interface.
    pub fn String(&self) -> String {
        let mut parts = Vec::new();
        for concurrency in &self.concurrency {
            if concurrency.concurrencyNum > 0 {
                parts.push(format!(
                    "{}:{}",
                    concurrency.concurrencyName, concurrency.concurrencyNum
                ));
            } else {
                parts.push(format!("{}:OFF", concurrency.concurrencyName));
            }
        }
        parts.join(", ")
    }

    // Merge implements the RuntimeStats interface.
    pub fn Merge(&mut self, _other: &dyn RuntimeStats) {}
}

// RuntimeStatsWithCommit is the RuntimeStats with commit detail.
#[derive(Default)]
/// 附带提交/加锁明细的运行时统计。
pub struct RuntimeStatsWithCommit {
    pub Commit: Option<util::CommitDetails>,
    pub LockKeys: Option<util::LockKeysDetails>,
    pub SharedLockKeys: Option<util::LockKeysDetails>,
    pub TxnCnt: i32,
}

impl RuntimeStatsWithCommit {
    // Tp implements the RuntimeStats interface.
    pub fn Tp(&self) -> i32 {
        TpRuntimeStatsWithCommit
    }

    // MergeCommitDetails merges the commit details.
    pub fn MergeCommitDetails(&mut self, detail: Option<util::CommitDetails>) {
        let Some(detail) = detail else {
            return;
        };
        if self.Commit.is_none() {
            self.Commit = Some(detail);
            self.TxnCnt = 1;
            return;
        }
        self.Commit.as_mut().unwrap().Merge(&detail);
        self.TxnCnt += 1;
    }

    // Merge implements the RuntimeStats interface.
    pub fn MergeCommitStats(&mut self, tmp: &RuntimeStatsWithCommit) {
        self.TxnCnt += tmp.TxnCnt;
        if let Some(commit) = tmp.Commit.as_ref() {
            self.Commit
                .get_or_insert_with(util::CommitDetails::default)
                .Merge(commit);
        }
        if let Some(lockKeys) = tmp.LockKeys.as_ref() {
            self.LockKeys
                .get_or_insert_with(util::LockKeysDetails::default)
                .Merge(lockKeys);
        }
        if let Some(sharedLockKeys) = tmp.SharedLockKeys.as_ref() {
            self.SharedLockKeys
                .get_or_insert_with(util::LockKeysDetails::default)
                .Merge(sharedLockKeys);
        }
    }

    // Clone implements the RuntimeStats interface.
    pub fn Clone(&self) -> RuntimeStatsWithCommit {
        RuntimeStatsWithCommit {
            TxnCnt: self.TxnCnt,
            Commit: self.Commit.as_ref().map(|commit| commit.Clone()),
            LockKeys: self.LockKeys.as_ref().map(|lockKeys| lockKeys.Clone()),
            SharedLockKeys: self
                .SharedLockKeys
                .as_ref()
                .map(|lockKeys| lockKeys.Clone()),
        }
    }

    // String implements the RuntimeStats interface.
    pub fn String(&self) -> String {
        let mut buf = String::with_capacity(32);
        if let Some(commit) = self.Commit.as_ref() {
            buf.push_str("commit_txn: {");
            // Only print out when there are more than 1 transaction.
            if self.TxnCnt > 1 {
                buf.push_str("count: ");
                buf.push_str(&self.TxnCnt.to_string());
                buf.push_str(", ");
            }
            if commit.PrewriteTime > StdDuration::default() {
                buf.push_str("prewrite:");
                buf.push_str(&FormatDuration(commit.PrewriteTime));
            }
            if commit.WaitPrewriteBinlogTime > StdDuration::default() {
                buf.push_str(", wait_prewrite_binlog:");
                buf.push_str(&FormatDuration(commit.WaitPrewriteBinlogTime));
            }
            if commit.GetCommitTsTime > StdDuration::default() {
                buf.push_str(", get_commit_ts:");
                buf.push_str(&FormatDuration(commit.GetCommitTsTime));
            }
            if commit.CommitTime > StdDuration::default() {
                buf.push_str(", commit:");
                buf.push_str(&FormatDuration(commit.CommitTime));
            }
            // Go 在读取 Commit.Mu 内字段前后手动 Lock/Unlock；用 guard 生命周期表达。
            let mu = commit.Mu.lock().expect("commit detail lock poisoned");
            let commitBackoffTime = mu.CommitBackoffTime;
            if commitBackoffTime > 0 {
                buf.push_str(", backoff: {time: ");
                buf.push_str(&FormatDuration(StdDuration::from_nanos(
                    commitBackoffTime as u64,
                )));
                if !mu.PrewriteBackoffTypes.is_empty() {
                    buf.push_str(", prewrite type: ");
                    self.formatBackoff(&mut buf, &mu.PrewriteBackoffTypes);
                }
                if !mu.CommitBackoffTypes.is_empty() {
                    buf.push_str(", commit type: ");
                    self.formatBackoff(&mut buf, &mu.CommitBackoffTypes);
                }
                buf.push('}');
            }
            if mu.SlowestPrewrite.ReqTotalTime > StdDuration::default() {
                buf.push_str(", slowest_prewrite_rpc: {total: ");
                buf.push_str(&format!(
                    "{:.3}s",
                    mu.SlowestPrewrite.ReqTotalTime.as_secs_f64()
                ));
                buf.push_str(", region_id: ");
                buf.push_str(&mu.SlowestPrewrite.Region.to_string());
                buf.push_str(", store: ");
                buf.push_str(&mu.SlowestPrewrite.StoreAddr);
                buf.push_str(", ");
                buf.push_str(&mu.SlowestPrewrite.ExecDetails.String());
                buf.push('}');
            }
            if mu.CommitPrimary.ReqTotalTime > StdDuration::default() {
                buf.push_str(", commit_primary_rpc: {total: ");
                buf.push_str(&format!(
                    "{:.3}s",
                    mu.CommitPrimary.ReqTotalTime.as_secs_f64()
                ));
                buf.push_str(", region_id: ");
                buf.push_str(&mu.CommitPrimary.Region.to_string());
                buf.push_str(", store: ");
                buf.push_str(&mu.CommitPrimary.StoreAddr);
                buf.push_str(", ");
                buf.push_str(&mu.CommitPrimary.ExecDetails.String());
                buf.push('}');
            }
            drop(mu);
            let resolveLockTime = commit.ResolveLock.ResolveLockTime.load(Ordering::Relaxed);
            if resolveLockTime > 0 {
                buf.push_str(", resolve_lock: ");
                buf.push_str(&FormatDuration(StdDuration::from_nanos(
                    resolveLockTime as u64,
                )));
            }
            let prewriteRegionNum = commit.PrewriteRegionNum.load(Ordering::Relaxed);
            if prewriteRegionNum > 0 {
                buf.push_str(", region_num:");
                buf.push_str(&prewriteRegionNum.to_string());
            }
            if commit.WriteKeys > 0 {
                buf.push_str(", write_keys:");
                buf.push_str(&commit.WriteKeys.to_string());
            }
            if commit.WriteSize > 0 {
                buf.push_str(", write_byte:");
                buf.push_str(&commit.WriteSize.to_string());
            }
            if commit.TxnRetry > 0 {
                buf.push_str(", txn_retry:");
                buf.push_str(&commit.TxnRetry.to_string());
            }
            buf.push('}');
        }
        self.formatLockKeysDetails(&mut buf, "lock_keys", self.LockKeys.as_ref());
        self.formatLockKeysDetails(&mut buf, "shared_lock_keys", self.SharedLockKeys.as_ref());
        buf
    }

    fn formatBackoff(&self, buf: &mut String, backoffTypes: &[String]) {
        if backoffTypes.is_empty() {
            return;
        }
        let mut tpMap = HashSet::new();
        let mut tpArray = Vec::new();
        for tpStr in backoffTypes {
            if tpMap.insert(tpStr.clone()) {
                tpArray.push(tpStr.clone());
            }
        }
        tpArray.sort();
        buf.push('[');
        for (i, tp) in tpArray.iter().enumerate() {
            if i > 0 {
                buf.push(' ');
            }
            buf.push_str(tp);
        }
        buf.push(']');
    }

    fn formatLockKeysDetails(
        &self,
        buf: &mut String,
        label: &str,
        lockKeys: Option<&util::LockKeysDetails>,
    ) {
        let Some(lockKeys) = lockKeys else {
            return;
        };
        if !buf.is_empty() {
            buf.push_str(", ");
        }
        buf.push_str(label);
        buf.push_str(": {");
        if lockKeys.TotalTime > StdDuration::default() {
            buf.push_str("time:");
            buf.push_str(&FormatDuration(lockKeys.TotalTime));
        }
        if lockKeys.RegionNum > 0 {
            buf.push_str(", region:");
            buf.push_str(&lockKeys.RegionNum.to_string());
        }
        if lockKeys.LockKeys > 0 {
            buf.push_str(", keys:");
            buf.push_str(&lockKeys.LockKeys.to_string());
        }
        let resolveLockTime = lockKeys.ResolveLock.ResolveLockTime.load(Ordering::Relaxed);
        if resolveLockTime > 0 {
            buf.push_str(", resolve_lock:");
            buf.push_str(&FormatDuration(StdDuration::from_nanos(
                resolveLockTime as u64,
            )));
        }
        let mu = lockKeys.Mu.lock().expect("lock keys detail lock poisoned");
        if lockKeys.BackoffTime > 0 {
            buf.push_str(", backoff: {time: ");
            buf.push_str(&FormatDuration(StdDuration::from_nanos(
                lockKeys.BackoffTime as u64,
            )));
            if !mu.BackoffTypes.is_empty() {
                buf.push_str(", type: ");
                self.formatBackoff(buf, &mu.BackoffTypes);
            }
            buf.push('}');
        }
        if mu.SlowestReqTotalTime > StdDuration::default() {
            buf.push_str(", slowest_rpc: {total: ");
            buf.push_str(&format!("{:.3}s", mu.SlowestReqTotalTime.as_secs_f64()));
            buf.push_str(", region_id: ");
            buf.push_str(&mu.SlowestRegion.to_string());
            buf.push_str(", store: ");
            buf.push_str(&mu.SlowestStoreAddr);
            buf.push_str(", ");
            buf.push_str(&mu.SlowestExecDetails.String());
            buf.push('}');
        }
        drop(mu);
        if lockKeys.LockRPCTime > 0 {
            buf.push_str(", lock_rpc:");
            buf.push_str(&format!(
                "{:?}",
                StdDuration::from_nanos(lockKeys.LockRPCTime as u64)
            ));
        }
        if lockKeys.LockRPCCount > 0 {
            buf.push_str(", rpc_count:");
            buf.push_str(&lockKeys.LockRPCCount.to_string());
        }
        if lockKeys.RetryCount > 0 {
            buf.push_str(", retry_count:");
            buf.push_str(&lockKeys.RetryCount.to_string());
        }
        buf.push('}');
    }
}

// RURuntimeStats wraps RU details and statement-level RU v2 metrics for EXPLAIN output.
// RUVersion controls which RU accounting version produces output:
//   - 1 (v1): shows RRU + WRU
//   - 2 (v2): shows total RU from v2 metrics
//   - 0 / unknown: defaults to v1
#[derive(Default)]
/// 资源单位（RU）相关运行时统计。
pub struct RURuntimeStats {
    pub RUDetails: Option<util::RUDetails>,
    pub Metrics: Option<RUV2Metrics>,
    pub Weights: RUV2Weights,
    pub RUVersion: rmclient::RUVersion,
}

impl RURuntimeStats {
    // String implements the RuntimeStats interface.
    pub fn String(&self) -> String {
        match self.RUVersion {
            rmclient::RUVersionV2 => {
                let mut tiKVRU = 0.0;
                let mut tiFlashRU = 0.0;
                if let Some(ruDetails) = self.RUDetails.as_ref() {
                    tiKVRU = ruDetails.TiKVRUV2();
                    tiFlashRU = ruDetails.TiflashRU();
                }
                let totalRU = self
                    .Metrics
                    .as_ref()
                    .map(|metrics| metrics.TotalRU(self.Weights, tiKVRU, tiFlashRU))
                    .unwrap_or(tiKVRU + tiFlashRU);
                if totalRU == 0.0 {
                    return String::new();
                }
                format!("RU:{:.2}", totalRU)
            }
            _ => {
                if let Some(ruDetails) = self.RUDetails.as_ref() {
                    return format!("RU:{:.2}", ruDetails.RRU() + ruDetails.WRU());
                }
                String::new()
            }
        }
    }

    // Clone implements the RuntimeStats interface.
    pub fn Clone(&self) -> RURuntimeStats {
        RURuntimeStats {
            RUDetails: self.RUDetails.as_ref().map(|ruDetails| ruDetails.Clone()),
            Metrics: self.Metrics.as_ref().map(|metrics| metrics.Clone()),
            Weights: self.Weights,
            RUVersion: self.RUVersion,
        }
    }

    // Merge implements the RuntimeStats interface.
    pub fn MergeRURuntimeStats(&mut self, tmp: &RURuntimeStats) {
        if let (Some(e), Some(other)) = (self.RUDetails.as_mut(), tmp.RUDetails.as_ref()) {
            e.Merge(other);
        } else if self.RUDetails.is_none() {
            self.RUDetails = tmp.RUDetails.as_ref().map(|details| details.Clone());
        }
        if let Some(metrics) = self.Metrics.as_ref() {
            metrics.Merge(tmp.Metrics.as_ref());
        } else {
            self.Metrics = tmp.Metrics.as_ref().map(|metrics| metrics.Clone());
        }
        if self.Weights == RUV2Weights::default() {
            self.Weights = tmp.Weights;
        }
        if self.RUVersion == 0 {
            self.RUVersion = tmp.RUVersion;
        }
    }

    // Tp implements the RuntimeStats interface.
    pub fn Tp(&self) -> i32 {
        TpRURuntimeStats
    }
}

impl RuntimeStats for basicCopRuntimeStats {
    fn String(&self) -> String {
        basicCopRuntimeStats::String(self)
    }

    fn Merge(&mut self, other: &dyn RuntimeStats) {
        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            basicCopRuntimeStats::Merge(self, other);
        }
    }

    fn CloneBox(&self) -> Box<dyn RuntimeStats> {
        Box::new(basicCopRuntimeStats::Clone(self))
    }

    fn Tp(&self) -> i32 {
        basicCopRuntimeStats::Tp(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RuntimeStats for BasicRuntimeStats {
    fn String(&self) -> String {
        BasicRuntimeStats::String(self)
    }

    fn Merge(&mut self, other: &dyn RuntimeStats) {
        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            self.MergeBasic(other);
        }
    }

    fn CloneBox(&self) -> Box<dyn RuntimeStats> {
        self.Clone()
    }

    fn Tp(&self) -> i32 {
        BasicRuntimeStats::Tp(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RuntimeStats for RuntimeStatsWithConcurrencyInfo {
    fn String(&self) -> String {
        RuntimeStatsWithConcurrencyInfo::String(self)
    }

    fn Merge(&mut self, other: &dyn RuntimeStats) {
        RuntimeStatsWithConcurrencyInfo::Merge(self, other);
    }

    fn CloneBox(&self) -> Box<dyn RuntimeStats> {
        Box::new(RuntimeStatsWithConcurrencyInfo::Clone(self))
    }

    fn Tp(&self) -> i32 {
        RuntimeStatsWithConcurrencyInfo::Tp(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RuntimeStats for RuntimeStatsWithCommit {
    fn String(&self) -> String {
        RuntimeStatsWithCommit::String(self)
    }

    fn Merge(&mut self, other: &dyn RuntimeStats) {
        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            self.MergeCommitStats(other);
        }
    }

    fn CloneBox(&self) -> Box<dyn RuntimeStats> {
        Box::new(RuntimeStatsWithCommit::Clone(self))
    }

    fn Tp(&self) -> i32 {
        RuntimeStatsWithCommit::Tp(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RuntimeStats for RURuntimeStats {
    fn String(&self) -> String {
        RURuntimeStats::String(self)
    }

    fn Merge(&mut self, other: &dyn RuntimeStats) {
        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            self.MergeRURuntimeStats(other);
        }
    }

    fn CloneBox(&self) -> Box<dyn RuntimeStats> {
        Box::new(RURuntimeStats::Clone(self))
    }

    fn Tp(&self) -> i32 {
        RURuntimeStats::Tp(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
