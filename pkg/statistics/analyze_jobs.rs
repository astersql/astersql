// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// ANALYZE 作业元数据与进度上报。
//
// 记录一次分析任务的起止时间、库表分区名、采样率原因，以及增量行数进度；
// 进度在累计超过阈值且距上次落库超过间隔时返回可落库的增量，避免过于频繁写元数据。

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime};

/// 作业状态：等待执行。
pub const AnalyzePending: &str = "pending";
/// 作业状态：执行中。
pub const AnalyzeRunning: &str = "running";
/// 作业状态：已完成。
pub const AnalyzeFinished: &str = "finished";
/// 作业状态：失败。
pub const AnalyzeFailed: &str = "failed";

/// 作业类型枚举底层整型。
pub type JobType = i32;
/// 表级分析作业。
pub const TableAnalysisJob: JobType = 1;
/// 全局统计合并作业（分区表汇总）。
pub const GlobalStatsMergeJob: JobType = 2;

/// 触发进度落库的最小累计行数增量。
const maxDelta: i64 = 10_000_000;
/// 两次进度落库之间的最小时间间隔。
const dumpTimeInterval: Duration = Duration::from_secs(5);

/// 一次 ANALYZE 作业的展示与跟踪信息。
pub struct AnalyzeJob {
    /// 作业开始时间。
    pub StartTime: SystemTime,
    /// 作业结束时间。
    pub EndTime: SystemTime,
    /// 持久化作业 ID（未落库前为空）。
    pub ID: Option<u64>,
    /// 数据库名。
    pub DBName: String,
    /// 表名。
    pub TableName: String,
    /// 分区名；非分区表可为空。
    pub PartitionName: String,
    /// 作业描述（如分析列/索引范围）。
    pub JobInfo: String,
    /// 采样率选择原因说明。
    pub SampleRateReason: String,
    /// 扫描进度与落库节流状态。
    pub Progress: AnalyzeProgress,
}

/// 默认空作业；时间为 Unix 纪元，便于未赋值时区分。
impl Default for AnalyzeJob {
    fn default() -> Self {
        Self {
            StartTime: SystemTime::UNIX_EPOCH,
            EndTime: SystemTime::UNIX_EPOCH,
            ID: None,
            DBName: String::new(),
            TableName: String::new(),
            PartitionName: String::new(),
            JobInfo: String::new(),
            SampleRateReason: String::new(),
            Progress: AnalyzeProgress::default(),
        }
    }
}

/// 分析进度：累计增量行数，并按阈值/时间间隔决定是否对外返回可落库增量。
pub struct AnalyzeProgress {
    /// 上次将进度写出的时间。
    lastDumpTime: Mutex<SystemTime>,
    /// 自上次落库以来累计的行数增量。
    deltaCount: AtomicI64,
}

/// 默认进度：尚未落库、增量为 0。
impl Default for AnalyzeProgress {
    fn default() -> Self {
        Self {
            lastDumpTime: Mutex::new(SystemTime::UNIX_EPOCH),
            deltaCount: AtomicI64::new(0),
        }
    }
}

impl AnalyzeProgress {
    /// 累加已处理行数；若超过 `maxDelta` 且距上次落库超过间隔，则清零并返回本次可落库增量，否则返回 0。
    pub fn Update(&self, row_count: i64) -> i64 {
        // 原子累加后得到新总量，再与阈值和时间窗比较。
        let new_count = self.deltaCount.fetch_add(row_count, Ordering::SeqCst) + row_count;
        let now = SystemTime::now();
        let mut last_dump_time = self.lastDumpTime.lock().unwrap();
        if new_count > maxDelta
            && now.duration_since(*last_dump_time).unwrap_or_default() > dumpTimeInterval
        {
            // 达到落库条件：重置增量并刷新时间戳，调用方应把返回值写入元数据。
            self.deltaCount.store(0, Ordering::SeqCst);
            *last_dump_time = now;
            new_count
        } else {
            0
        }
    }

    /// 读取当前未落库的增量行数。
    pub fn GetDeltaCount(&self) -> i64 {
        self.deltaCount.load(Ordering::SeqCst)
    }

    /// 手动设置上次落库时间（测试或恢复场景）。
    pub fn SetLastDumpTime(&self, time: SystemTime) {
        *self.lastDumpTime.lock().unwrap() = time;
    }

    /// 读取上次落库时间。
    pub fn GetLastDumpTime(&self) -> SystemTime {
        *self.lastDumpTime.lock().unwrap()
    }
}
