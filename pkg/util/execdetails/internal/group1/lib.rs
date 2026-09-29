// Copyright 2026 AsterSQL.

// `execdetails` 核心类型与 runtime stats 的内部 crate（group1）。
//
// 提供与 Go 对齐的时间/扫描/提交/加锁明细、TiFlash 占位统计、百分位数，
// 并 include 正式的 `execdetails.rs` / `runtime_stats.rs` 实现。
// Region 是 TiKV 数据分片；两阶段提交（2PC）的 prewrite/commit 耗时记入 CommitDetails。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::cmp::Ordering as CmpOrdering;
/// 时间类型再导出。
pub mod time {
    pub use std::time::Duration;
}

/// 包装 std Mutex，提供 Go 风格 Lock()。
pub mod sync {
    pub struct Mutex<T>(std::sync::Mutex<T>);

    impl<T: Default> Default for Mutex<T> {
        fn default() -> Self {
            Self(std::sync::Mutex::new(T::default()))
        }
    }

    impl<T> Mutex<T> {
        pub fn new(value: T) -> Self {
            Self(std::sync::Mutex::new(value))
        }

        pub fn Lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().expect("mutex poisoned")
        }

        pub fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, T>> {
            self.0.lock()
        }
    }
}

/// 原子读辅助（Relaxed）。
pub mod atomic {
    use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

    pub fn LoadInt64(value: &AtomicI64) -> i64 {
        value.load(Ordering::Relaxed)
    }

    pub fn LoadInt32(value: &AtomicI32) -> i32 {
        value.load(Ordering::Relaxed)
    }
}

/// 结构化日志字段桩（对齐 zap.Field）。
pub mod zap {
    #[derive(Clone, Debug, PartialEq)]
    pub enum Value {
        String(String),
        Int(i32),
    }

    #[derive(Clone, Debug, PartialEq)]
    pub struct Field {
        pub key: String,
        pub value: Value,
    }

    pub fn String(key: &str, value: String) -> Field {
        Field {
            key: key.to_owned(),
            value: Value::String(value),
        }
    }

    pub fn Int(key: &str, value: i32) -> Field {
        Field {
            key: key.to_owned(),
            value: Value::Int(value),
        }
    }

    pub fn Int32(key: &str, value: i32) -> Field {
        Int(key, value)
    }
}

/// 存储引擎类型枚举（TiKV/TiFlash/TiDB）。
pub mod kv {
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    /// KV 存储类型。
    pub enum StoreType {
        #[default]
        TiKV,
        TiFlash,
        TiDB,
        UnSpecified,
    }

    impl StoreType {
        /// 返回小写类型名。
        pub fn Name(self) -> &'static str {
            match self {
                Self::TiKV => "tikv",
                Self::TiFlash => "tiflash",
                Self::TiDB => "tidb",
                Self::UnSpecified => "unspecified",
            }
        }
    }

    pub const TiKV: StoreType = StoreType::TiKV;
    pub const TiFlash: StoreType = StoreType::TiFlash;
}

/// 时间/扫描/RPC/提交/加锁与 RU 明细结构。
pub mod util {
    use crate::sync;
    use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
    use std::time::Duration;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    /// 处理与等待耗时。
    pub struct TimeDetail {
        pub ProcessTime: Duration,
        pub WaitTime: Duration,
    }

    impl TimeDetail {
        pub fn Merge(&mut self, other: &Self) {
            self.ProcessTime += other.ProcessTime;
            self.WaitTime += other.WaitTime;
        }

        pub fn String(&self) -> String {
            let mut parts = Vec::new();
            if !self.ProcessTime.is_zero() {
                parts.push(format!("process_time:{}s", self.ProcessTime.as_secs_f64()));
            }
            if !self.WaitTime.is_zero() {
                parts.push(format!("wait_time:{}s", self.WaitTime.as_secs_f64()));
            }
            parts.join(", ")
        }
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    /// 扫描键数与 RocksDB 块读统计。
    pub struct ScanDetail {
        pub ProcessedKeys: i64,
        pub TotalKeys: i64,
        pub GetSnapshotDuration: Duration,
        pub RocksdbDeleteSkippedCount: u64,
        pub RocksdbKeySkippedCount: u64,
        pub RocksdbBlockCacheHitCount: u64,
        pub RocksdbBlockReadCount: u64,
        pub RocksdbBlockReadByte: u64,
        pub RocksdbBlockReadDuration: Duration,
        pub IaRemoteReadSegmentCount: u64,
        pub IaRemoteReadSegmentBytes: u64,
        pub IaRemoteReadSegmentDuration: Duration,
    }

    impl ScanDetail {
        pub fn Merge(&mut self, other: &Self) {
            self.ProcessedKeys += other.ProcessedKeys;
            self.TotalKeys += other.TotalKeys;
            self.GetSnapshotDuration += other.GetSnapshotDuration;
            self.RocksdbDeleteSkippedCount += other.RocksdbDeleteSkippedCount;
            self.RocksdbKeySkippedCount += other.RocksdbKeySkippedCount;
            self.RocksdbBlockCacheHitCount += other.RocksdbBlockCacheHitCount;
            self.RocksdbBlockReadCount += other.RocksdbBlockReadCount;
            self.RocksdbBlockReadByte += other.RocksdbBlockReadByte;
            self.RocksdbBlockReadDuration += other.RocksdbBlockReadDuration;
            self.IaRemoteReadSegmentCount += other.IaRemoteReadSegmentCount;
            self.IaRemoteReadSegmentBytes += other.IaRemoteReadSegmentBytes;
            self.IaRemoteReadSegmentDuration += other.IaRemoteReadSegmentDuration;
        }

        pub fn String(&self) -> String {
            let mut parts = Vec::new();
            if self.ProcessedKeys > 0 {
                parts.push(format!("processed_keys:{}", self.ProcessedKeys));
            }
            if self.TotalKeys > 0 {
                parts.push(format!("total_keys:{}", self.TotalKeys));
            }
            parts.join(", ")
        }
    }

    /// TiKV 读池任务聚合；与 client-go PoolTaskDetails 的计数、极值和耗时口径一致。
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct PoolTaskDetails {
        pub TaskCount: u64,
        pub PollCount: u64,
        pub MaxPollCount: u64,
        pub MinPollCount: u64,
        pub DispatchCount: u64,
        pub MaxDispatchCount: u64,
        pub MinDispatchCount: u64,
        pub TotalWallTime: Duration,
        pub TaskWallTimeSampleCount: u64,
        pub MaxTaskWallTime: Duration,
        pub MinTaskWallTime: Duration,
        pub TotalQueueWaitTime: Duration,
        pub MaxQueueWaitTime: Duration,
        pub MinQueueWaitTime: Duration,
        pub TotalWakeWaitTime: Duration,
        pub MaxWakeWaitTime: Duration,
        pub MinWakeWaitTime: Duration,
        pub FairQueueSampleCount: u64,
        pub TotalFairQueueWaitedTaskSlices: u64,
        pub MaxFairQueueWaitedTaskSlices: u64,
        pub MinFairQueueWaitedTaskSlices: u64,
        pub PollCPUTime: Duration,
        pub MaxPollCPUTime: Duration,
        pub MinPollCPUTime: Duration,
        pub PollWallTime: Duration,
        pub MaxPollWallTime: Duration,
        pub MinPollWallTime: Duration,
    }

    impl PoolTaskDetails {
        pub fn Empty(&self) -> bool {
            self.TaskCount == 0
        }
        pub fn Clone(&self) -> Self {
            self.clone()
        }

        /// 合并另一聚合，保留仅在存在样本时更新最小值的 Go 语义。
        pub fn Merge(&mut self, other: &Self) {
            if other.Empty() {
                return;
            }
            let had_tasks = self.TaskCount > 0;
            let had_poll = self.PollCount > 0;
            let had_wall = self.TaskWallTimeSampleCount > 0;
            let had_queue = !self.TotalQueueWaitTime.is_zero();
            let had_wake = !self.TotalWakeWaitTime.is_zero();
            let had_fair = self.FairQueueSampleCount > 0;
            macro_rules! minimum {
                ($field:ident, $present:expr) => {
                    if !$present || other.$field < self.$field {
                        self.$field = other.$field;
                    }
                };
            }
            self.TaskCount += other.TaskCount;
            self.PollCount += other.PollCount;
            self.MaxPollCount = self.MaxPollCount.max(other.MaxPollCount);
            minimum!(MinPollCount, had_tasks);
            self.DispatchCount += other.DispatchCount;
            self.MaxDispatchCount = self.MaxDispatchCount.max(other.MaxDispatchCount);
            minimum!(MinDispatchCount, had_tasks);
            self.TotalWallTime += other.TotalWallTime;
            self.TaskWallTimeSampleCount += other.TaskWallTimeSampleCount;
            self.MaxTaskWallTime = self.MaxTaskWallTime.max(other.MaxTaskWallTime);
            if !other.TotalWallTime.is_zero() {
                minimum!(MinTaskWallTime, had_wall);
            }
            self.TotalQueueWaitTime += other.TotalQueueWaitTime;
            self.MaxQueueWaitTime = self.MaxQueueWaitTime.max(other.MaxQueueWaitTime);
            if !other.TotalQueueWaitTime.is_zero() {
                minimum!(MinQueueWaitTime, had_queue);
            }
            self.TotalWakeWaitTime += other.TotalWakeWaitTime;
            self.MaxWakeWaitTime = self.MaxWakeWaitTime.max(other.MaxWakeWaitTime);
            if !other.TotalWakeWaitTime.is_zero() {
                minimum!(MinWakeWaitTime, had_wake);
            }
            self.FairQueueSampleCount += other.FairQueueSampleCount;
            self.TotalFairQueueWaitedTaskSlices += other.TotalFairQueueWaitedTaskSlices;
            self.MaxFairQueueWaitedTaskSlices = self
                .MaxFairQueueWaitedTaskSlices
                .max(other.MaxFairQueueWaitedTaskSlices);
            if other.FairQueueSampleCount > 0 {
                minimum!(MinFairQueueWaitedTaskSlices, had_fair);
            }
            self.PollCPUTime += other.PollCPUTime;
            self.MaxPollCPUTime = self.MaxPollCPUTime.max(other.MaxPollCPUTime);
            self.PollWallTime += other.PollWallTime;
            self.MaxPollWallTime = self.MaxPollWallTime.max(other.MaxPollWallTime);
            if other.PollCount > 0 {
                minimum!(MinPollCPUTime, had_poll);
                minimum!(MinPollWallTime, had_poll);
            }
        }

        pub fn String(&self) -> String {
            if self.Empty() {
                return String::new();
            }
            let mut result = format!("{{tasks:{}", self.TaskCount);
            fn average(total: u64, count: u64) -> String {
                let value = format!("{:.2}", total as f64 / count as f64);
                value.trim_end_matches('0').trim_end_matches('.').to_owned()
            }
            fn count_stats(
                result: &mut String,
                name: &str,
                total: u64,
                divisor: u64,
                max: u64,
                min: u64,
            ) {
                result.push_str(&format!(", {name}:{{total:{total}"));
                if divisor > 0 {
                    result.push_str(&format!(", avg:{}", average(total, divisor)));
                }
                result.push_str(&format!(", max:{max}, min:{min}}}"));
            }
            fn time_stats(
                result: &mut String,
                name: &str,
                total: Duration,
                samples: u64,
                max: Duration,
                min: Duration,
            ) {
                if total.is_zero() {
                    return;
                }
                result.push_str(&format!(
                    ", {name}:{{total:{}",
                    super::FormatDuration(total)
                ));
                if samples > 0 {
                    let average_nanos = total.as_nanos() / u128::from(samples);
                    let average =
                        Duration::from_nanos(u64::try_from(average_nanos).unwrap_or(u64::MAX));
                    result.push_str(&format!(", avg:{}", super::FormatDuration(average)));
                }
                result.push_str(&format!(
                    ", max:{}, min:{}}}",
                    super::FormatDuration(max),
                    super::FormatDuration(min)
                ));
            }
            count_stats(
                &mut result,
                "poll_count",
                self.PollCount,
                self.TaskCount,
                self.MaxPollCount,
                self.MinPollCount,
            );
            count_stats(
                &mut result,
                "dispatch_count",
                self.DispatchCount,
                0,
                self.MaxDispatchCount,
                self.MinDispatchCount,
            );
            time_stats(
                &mut result,
                "task_wall_time",
                self.TotalWallTime,
                self.TaskWallTimeSampleCount,
                self.MaxTaskWallTime,
                self.MinTaskWallTime,
            );
            time_stats(
                &mut result,
                "queue_wait",
                self.TotalQueueWaitTime,
                self.DispatchCount,
                self.MaxQueueWaitTime,
                self.MinQueueWaitTime,
            );
            time_stats(
                &mut result,
                "wake_wait",
                self.TotalWakeWaitTime,
                self.DispatchCount.saturating_sub(self.TaskCount),
                self.MaxWakeWaitTime,
                self.MinWakeWaitTime,
            );
            result.push_str(&format!(
                ", fair_queue:{{enabled:{}, waited_task_slices:{{total:{}",
                self.FairQueueSampleCount > 0,
                self.TotalFairQueueWaitedTaskSlices
            ));
            if self.FairQueueSampleCount > 0 {
                result.push_str(&format!(
                    ", avg:{}",
                    average(
                        self.TotalFairQueueWaitedTaskSlices,
                        self.FairQueueSampleCount
                    )
                ));
            }
            result.push_str(&format!(
                ", max:{}, min:{}}}}}",
                self.MaxFairQueueWaitedTaskSlices, self.MinFairQueueWaitedTaskSlices
            ));
            time_stats(
                &mut result,
                "poll_cpu",
                self.PollCPUTime,
                self.PollCount,
                self.MaxPollCPUTime,
                self.MinPollCPUTime,
            );
            time_stats(
                &mut result,
                "poll_wall",
                self.PollWallTime,
                self.PollCount,
                self.MaxPollWallTime,
                self.MinPollWallTime,
            );
            result.push('}');
            result
        }
    }

    #[derive(Clone, Debug, Default)]
    /// RPC 执行详情文本。
    pub struct RPCExecDetails {
        pub text: String,
    }

    impl RPCExecDetails {
        pub fn String(&self) -> String {
            self.text.clone()
        }
    }

    #[derive(Clone, Debug, Default)]
    /// 单次 RPC：总耗时、Region、store 地址与执行详情。
    pub struct RPCDetails {
        pub ReqTotalTime: Duration,
        pub Region: u64,
        pub StoreAddr: String,
        pub ExecDetails: RPCExecDetails,
    }

    #[derive(Clone, Debug, Default)]
    /// 提交阶段需加锁保护的退避与最慢 RPC 信息。
    pub struct CommitDetailsMu {
        pub CommitBackoffTime: i64,
        pub PrewriteBackoffTypes: Vec<String>,
        pub CommitBackoffTypes: Vec<String>,
        pub SlowestPrewrite: RPCDetails,
        pub CommitPrimary: RPCDetails,
    }

    #[derive(Debug, Default)]
    /// 解决悲观锁/过期锁所耗时间。
    pub struct ResolveLockDetails {
        pub ResolveLockTime: AtomicI64,
    }

    impl Clone for ResolveLockDetails {
        fn clone(&self) -> Self {
            Self {
                ResolveLockTime: AtomicI64::new(self.ResolveLockTime.load(Ordering::Relaxed)),
            }
        }
    }

    #[derive(Default)]
    /// 两阶段提交各阶段耗时与写入规模。
    pub struct CommitDetails {
        pub PrewriteTime: Duration,
        pub WaitPrewriteBinlogTime: Duration,
        pub CommitTime: Duration,
        pub GetCommitTsTime: Duration,
        pub GetLatestTsTime: Duration,
        pub Mu: sync::Mutex<CommitDetailsMu>,
        pub ResolveLock: ResolveLockDetails,
        pub LocalLatchTime: Duration,
        pub WriteKeys: i32,
        pub WriteSize: i32,
        pub PrewriteRegionNum: AtomicI32,
        pub TxnRetry: i32,
    }

    impl Clone for CommitDetails {
        fn clone(&self) -> Self {
            Self {
                PrewriteTime: self.PrewriteTime,
                WaitPrewriteBinlogTime: self.WaitPrewriteBinlogTime,
                CommitTime: self.CommitTime,
                GetCommitTsTime: self.GetCommitTsTime,
                GetLatestTsTime: self.GetLatestTsTime,
                Mu: sync::Mutex::new(self.Mu.Lock().clone()),
                ResolveLock: self.ResolveLock.clone(),
                LocalLatchTime: self.LocalLatchTime,
                WriteKeys: self.WriteKeys,
                WriteSize: self.WriteSize,
                PrewriteRegionNum: AtomicI32::new(self.PrewriteRegionNum.load(Ordering::Relaxed)),
                TxnRetry: self.TxnRetry,
            }
        }
    }

    impl CommitDetails {
        /// 合并另一份提交明细；最慢 prewrite/primary 取较大值。
        pub fn Merge(&mut self, other: &Self) {
            self.PrewriteTime += other.PrewriteTime;
            self.WaitPrewriteBinlogTime += other.WaitPrewriteBinlogTime;
            self.CommitTime += other.CommitTime;
            self.GetCommitTsTime += other.GetCommitTsTime;
            self.GetLatestTsTime += other.GetLatestTsTime;
            self.LocalLatchTime += other.LocalLatchTime;
            self.WriteKeys += other.WriteKeys;
            self.WriteSize += other.WriteSize;
            self.TxnRetry += other.TxnRetry;
            self.ResolveLock.ResolveLockTime.fetch_add(
                other.ResolveLock.ResolveLockTime.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.PrewriteRegionNum.fetch_add(
                other.PrewriteRegionNum.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            let mut target = self.Mu.Lock();
            let source = other.Mu.Lock();
            target.CommitBackoffTime += source.CommitBackoffTime;
            target
                .PrewriteBackoffTypes
                .extend(source.PrewriteBackoffTypes.clone());
            target
                .CommitBackoffTypes
                .extend(source.CommitBackoffTypes.clone());
            // 保留更慢的 prewrite RPC。
            if source.SlowestPrewrite.ReqTotalTime > target.SlowestPrewrite.ReqTotalTime {
                target.SlowestPrewrite = source.SlowestPrewrite.clone();
            }
            if source.CommitPrimary.ReqTotalTime > target.CommitPrimary.ReqTotalTime {
                target.CommitPrimary = source.CommitPrimary.clone();
            }
        }

        pub fn Clone(&self) -> Self {
            Clone::clone(self)
        }
    }

    #[derive(Clone, Debug, Default)]
    /// LockKeys 路径上需加锁的退避与最慢请求。
    pub struct LockKeysDetailsMu {
        pub BackoffTypes: Vec<String>,
        pub SlowestReqTotalTime: Duration,
        pub SlowestRegion: u64,
        pub SlowestStoreAddr: String,
        pub SlowestExecDetails: RPCExecDetails,
    }

    #[derive(Default)]
    /// 悲观事务 LockKeys 阶段统计。
    pub struct LockKeysDetails {
        pub TotalTime: Duration,
        pub RegionNum: i32,
        pub LockKeys: i32,
        pub ResolveLock: ResolveLockDetails,
        pub BackoffTime: i64,
        pub Mu: sync::Mutex<LockKeysDetailsMu>,
        pub LockRPCTime: i64,
        pub LockRPCCount: i64,
        pub RetryCount: i32,
        pub AggressiveLockNewCount: i32,
        pub AggressiveLockDerivedCount: i32,
        pub LockedWithConflictCount: i32,
    }

    impl Clone for LockKeysDetails {
        fn clone(&self) -> Self {
            Self {
                TotalTime: self.TotalTime,
                RegionNum: self.RegionNum,
                LockKeys: self.LockKeys,
                ResolveLock: self.ResolveLock.clone(),
                BackoffTime: self.BackoffTime,
                Mu: sync::Mutex::new(self.Mu.Lock().clone()),
                LockRPCTime: self.LockRPCTime,
                LockRPCCount: self.LockRPCCount,
                RetryCount: self.RetryCount,
                AggressiveLockNewCount: self.AggressiveLockNewCount,
                AggressiveLockDerivedCount: self.AggressiveLockDerivedCount,
                LockedWithConflictCount: self.LockedWithConflictCount,
            }
        }
    }

    impl LockKeysDetails {
        pub fn Merge(&mut self, other: &Self) {
            self.TotalTime += other.TotalTime;
            self.RegionNum += other.RegionNum;
            self.LockKeys += other.LockKeys;
            self.BackoffTime += other.BackoffTime;
            self.LockRPCTime += other.LockRPCTime;
            self.LockRPCCount += other.LockRPCCount;
            self.RetryCount += other.RetryCount;
            self.AggressiveLockNewCount += other.AggressiveLockNewCount;
            self.AggressiveLockDerivedCount += other.AggressiveLockDerivedCount;
            self.LockedWithConflictCount += other.LockedWithConflictCount;
            self.ResolveLock.ResolveLockTime.fetch_add(
                other.ResolveLock.ResolveLockTime.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            self.Mu
                .Lock()
                .BackoffTypes
                .extend(other.Mu.Lock().BackoffTypes.clone());
        }

        pub fn Clone(&self) -> Self {
            Clone::clone(self)
        }
    }

    #[derive(Clone, Debug, Default)]
    /// 读写 RU、TiKV RUv2、TiFlash RU 与等待时长。
    pub struct RUDetails {
        pub read_ru: f64,
        pub write_ru: f64,
        pub tikv_ru_v2: f64,
        pub tiflash_ru: f64,
        pub ru_wait_duration: Duration,
    }

    impl RUDetails {
        pub fn RRU(&self) -> f64 {
            self.read_ru
        }
        pub fn WRU(&self) -> f64 {
            self.write_ru
        }
        pub fn TiKVRUV2(&self) -> f64 {
            self.tikv_ru_v2
        }
        pub fn TiflashRU(&self) -> f64 {
            self.tiflash_ru
        }
        pub fn RUWaitDuration(&self) -> Duration {
            self.ru_wait_duration
        }
        pub fn Merge(&mut self, other: &Self) {
            self.read_ru += other.read_ru;
            self.write_ru += other.write_ru;
            self.tikv_ru_v2 += other.tikv_ru_v2;
            self.tiflash_ru += other.tiflash_ru;
            self.ru_wait_duration += other.ru_wait_duration;
        }
        pub fn Clone(&self) -> Self {
            Clone::clone(self)
        }
    }
}

/// tipb 执行摘要相关占位类型（group1 内简化版）。
pub mod tipb {
    #[derive(Clone, Debug, Default)]
    pub struct TiFlashScanContext {
        pub UserReadBytes: Option<u64>,
    }
    #[derive(Clone, Debug, Default)]
    pub struct ColumnarScanContext {
        pub UserReadBytes: Option<u64>,
        pub MvccInputBytes: Option<u64>,
    }
    #[derive(Clone, Debug, Default)]
    pub struct TiFlashWaitSummary;
    #[derive(Clone, Debug, Default)]
    pub struct TiFlashNetworkSummary {
        pub InnerZoneSendBytes: Option<u64>,
        pub InterZoneSendBytes: Option<u64>,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum TiFlashHashTableSizeKind {
        #[default]
        DistinctKeyCount,
        BuildRowCount,
        Unknown(i32),
    }

    #[derive(Clone, Debug, Default)]
    pub struct TiFlashHashTableStats {
        pub Size_: Option<u64>,
        pub SizeKind: TiFlashHashTableSizeKind,
    }

    #[derive(Clone, Debug, Default)]
    /// 单个执行器的执行摘要字段。
    pub struct ExecutorExecutionSummary {
        pub NumIterations: Option<u64>,
        pub NumProducedRows: Option<u64>,
        pub TimeProcessedNs: Option<u64>,
        pub Concurrency: u64,
        pub ExecutorId: String,
        pub TiflashScanContext: Option<TiFlashScanContext>,
        pub ColumnarScanContext: Option<ColumnarScanContext>,
        pub TiflashWaitSummary: Option<TiFlashWaitSummary>,
        pub TiflashNetworkSummary: Option<TiFlashNetworkSummary>,
        pub TiflashHashTableStats: Option<TiFlashHashTableStats>,
    }

    impl ExecutorExecutionSummary {
        pub fn GetConcurrency(&self) -> u64 {
            self.Concurrency
        }
        pub fn GetExecutorId(&self) -> &str {
            &self.ExecutorId
        }
        pub fn GetTiflashScanContext(&self) -> Option<&TiFlashScanContext> {
            self.TiflashScanContext.as_ref()
        }
        pub fn GetColumnarScanContext(&self) -> Option<&ColumnarScanContext> {
            self.ColumnarScanContext.as_ref()
        }
        pub fn GetTiflashWaitSummary(&self) -> Option<&TiFlashWaitSummary> {
            self.TiflashWaitSummary.as_ref()
        }
        pub fn GetTiflashNetworkSummary(&self) -> Option<&TiFlashNetworkSummary> {
            self.TiflashNetworkSummary.as_ref()
        }
        pub fn GetTiflashHashTableStats(&self) -> Option<&TiFlashHashTableStats> {
            self.TiflashHashTableStats.as_ref()
        }
    }
}

#[derive(Clone, Debug, Default)]
/// TiFlash 表扫描上下文占位（完整实现见 tiflash_stats）。
pub struct TiFlashScanContext {
    present: bool,
}

impl TiFlashScanContext {
    pub fn Clone(&self) -> Self {
        Clone::clone(self)
    }
    pub fn Merge(&mut self, other: Self) {
        self.present |= other.present;
    }
    pub fn Empty(&self) -> bool {
        !self.present
    }
    pub fn String(&self) -> String {
        "tiflash_scan".to_owned()
    }
    pub fn mergeExecSummary(&mut self, summary: Option<&tipb::TiFlashScanContext>) {
        self.present |= summary.is_some();
    }
}

#[derive(Clone, Debug, Default)]
/// TiFlash 列存扫描上下文占位。
pub struct TiFlashColumnarScanContext {
    present: bool,
}

impl TiFlashColumnarScanContext {
    pub fn Clone(&self) -> Self {
        Clone::clone(self)
    }
    pub fn Merge(&mut self, other: Self) {
        self.present |= other.present;
    }
    pub fn Empty(&self) -> bool {
        !self.present
    }
    pub fn String(&self) -> String {
        "columnar_scan".to_owned()
    }
    pub fn mergeExecSummary(&mut self, summary: Option<&tipb::ColumnarScanContext>) {
        self.present |= summary.is_some();
    }
}

#[derive(Clone, Debug, Default)]
/// TiFlash 等待摘要占位。
pub struct TiFlashWaitSummary {
    present: bool,
}

impl TiFlashWaitSummary {
    pub fn Clone(&self) -> Self {
        Clone::clone(self)
    }
    pub fn Merge(&mut self, other: Self) {
        self.present |= other.present;
    }
    pub fn CanBeIgnored(&self) -> bool {
        !self.present
    }
    pub fn String(&self) -> String {
        "wait_summary".to_owned()
    }
    pub fn mergeExecSummary(&mut self, summary: Option<&tipb::TiFlashWaitSummary>, _ns: u64) {
        self.present |= summary.is_some();
    }
}

#[derive(Clone, Debug, Default)]
/// TiFlash 网络流量摘要占位。
pub struct TiFlashNetworkTrafficSummary {
    present: bool,
}

impl TiFlashNetworkTrafficSummary {
    pub fn Clone(&self) -> Self {
        Clone::clone(self)
    }
    pub fn Merge(&mut self, other: Self) {
        self.present |= other.present;
    }
    pub fn Empty(&self) -> bool {
        !self.present
    }
    pub fn String(&self) -> String {
        "network_summary".to_owned()
    }
    pub fn mergeExecSummary(&mut self, summary: Option<&tipb::TiFlashNetworkSummary>) {
        self.present |= summary.is_some();
    }
}

#[derive(Clone, Debug, Default)]
/// 聚合 TiFlash 四类统计。
pub struct TiflashStats {
    pub scanContext: TiFlashScanContext,
    pub columnarScanContext: TiFlashColumnarScanContext,
    pub waitSummary: TiFlashWaitSummary,
    pub networkSummary: TiFlashNetworkTrafficSummary,
}

/// 可转为 f64，供百分位数计算。
pub trait canGetFloat64 {
    fn GetFloat64(&self) -> f64;
}

/// 时长别名。
pub type Duration = std::time::Duration;

impl canGetFloat64 for Duration {
    fn GetFloat64(&self) -> f64 {
        self.as_nanos() as f64
    }
}

#[derive(Clone, Debug, Default)]
/// 带 store 地址的时长样本。
pub struct DurationWithAddr {
    pub D: std::time::Duration,
    pub Addr: String,
}

impl canGetFloat64 for DurationWithAddr {
    fn GetFloat64(&self) -> f64 {
        self.D.as_nanos() as f64
    }
}

#[derive(Clone, Debug)]
/// 样本集合上的百分位/最大最小统计。
pub struct Percentile<T: canGetFloat64 + Clone> {
    values: Vec<T>,
    sum: f64,
}

impl<T: canGetFloat64 + Clone> Default for Percentile<T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            sum: 0.0,
        }
    }
}

impl<T: canGetFloat64 + Clone> Percentile<T> {
    pub fn Add(&mut self, value: T) {
        self.sum += value.GetFloat64();
        self.values.push(value);
    }

    pub fn MergePercentile(&mut self, other: &Self) {
        for value in &other.values {
            self.Add(value.clone());
        }
    }

    pub fn Size(&self) -> usize {
        self.values.len()
    }
    pub fn Sum(&self) -> f64 {
        self.sum
    }

    /// 排序后按比例 f 取分位值。
    pub fn GetPercentile(&mut self, f: f64) -> f64 {
        self.values.sort_by(|a, b| {
            a.GetFloat64()
                .partial_cmp(&b.GetFloat64())
                .unwrap_or(CmpOrdering::Equal)
        });
        let index = ((self.values.len() as f64) * f) as usize;
        self.values[index.min(self.values.len().saturating_sub(1))].GetFloat64()
    }

    pub fn GetMax(&self) -> Option<T> {
        self.values
            .iter()
            .max_by(|a, b| {
                a.GetFloat64()
                    .partial_cmp(&b.GetFloat64())
                    .unwrap_or(CmpOrdering::Equal)
            })
            .cloned()
    }

    pub fn GetMin(&self) -> Option<T> {
        self.values
            .iter()
            .min_by(|a, b| {
                a.GetFloat64()
                    .partial_cmp(&b.GetFloat64())
                    .unwrap_or(CmpOrdering::Equal)
            })
            .cloned()
    }
}

/// 按 Go `FormatDuration` 的可读性规则裁剪精度后格式化耗时。
pub fn FormatDuration(mut d: std::time::Duration) -> String {
    use std::time::Duration as StdDuration;

    if d <= StdDuration::from_micros(1) {
        return formatGoDuration(d);
    }
    let unit = getUnit(d);
    let unit_ns = unit.as_nanos();
    let d_ns = d.as_nanos();
    let integer_ns = (d_ns / unit_ns) * unit_ns;
    let scale = if d < unit * 10 { 100 } else { 10 };
    let rounded_fraction = ((d_ns % unit_ns) * scale + unit_ns / 2) / unit_ns;
    let rounded_ns = integer_ns + rounded_fraction * (unit_ns / scale);
    d = StdDuration::from_nanos(rounded_ns.min(u64::MAX as u128) as u64);
    formatGoDuration(d)
}

fn formatGoDuration(d: std::time::Duration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return "0s".to_owned();
    }
    if nanos < 1_000 {
        return format!("{nanos}ns");
    }
    if nanos < 1_000_000 {
        return formatDecimalDuration(nanos / 1_000, nanos % 1_000, 3, "µs");
    }
    if nanos < 1_000_000_000 {
        return formatDecimalDuration(nanos / 1_000_000, nanos % 1_000_000, 6, "ms");
    }

    let total_seconds = nanos / 1_000_000_000;
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    let second_part = formatDecimalDuration(seconds, nanos % 1_000_000_000, 9, "s");
    if hours > 0 {
        format!("{hours}h{minutes}m{second_part}")
    } else if minutes > 0 {
        format!("{minutes}m{second_part}")
    } else {
        second_part
    }
}

fn formatDecimalDuration(whole: u128, fraction: u128, width: usize, suffix: &str) -> String {
    if fraction == 0 {
        return format!("{whole}{suffix}");
    }
    let mut fraction = format!("{fraction:0width$}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{whole}.{fraction}{suffix}")
}

fn getUnit(d: std::time::Duration) -> std::time::Duration {
    use std::time::Duration as StdDuration;

    if d >= StdDuration::from_secs(1) {
        StdDuration::from_secs(1)
    } else if d >= StdDuration::from_millis(1) {
        StdDuration::from_millis(1)
    } else if d >= StdDuration::from_micros(1) {
        StdDuration::from_micros(1)
    } else {
        StdDuration::from_nanos(1)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// RUv2 缩放权重（group1 精简版仅含 RUScale）。
pub struct RUV2Weights {
    pub RUScale: f64,
}

#[derive(Debug, Default)]
/// 简单 RUv2 累加器（完整版见 ruv2_metrics）。
pub struct RUV2Metrics {
    total: std::sync::Mutex<f64>,
}

impl Clone for RUV2Metrics {
    fn clone(&self) -> Self {
        Self {
            total: std::sync::Mutex::new(*self.total.lock().expect("metrics lock poisoned")),
        }
    }
}

impl RUV2Metrics {
    pub fn Clone(&self) -> Self {
        Clone::clone(self)
    }

    pub fn Add(&self, value: f64) {
        *self.total.lock().expect("metrics lock poisoned") += value;
    }

    pub fn Merge(&self, other: Option<&Self>) {
        if let Some(other) = other {
            self.Add(*other.total.lock().expect("metrics lock poisoned"));
        }
    }

    pub fn TotalRU(&self, weights: RUV2Weights, tikv: f64, tiflash: f64) -> f64 {
        let scale = if weights.RUScale == 0.0 {
            1.0
        } else {
            weights.RUScale
        };
        (*self.total.lock().expect("metrics lock poisoned") + tikv + tiflash) * scale
    }
}

/// 资源管理客户端 RU 版本常量。
pub mod rmclient {
    pub type RUVersion = i32;
    pub const RUVersionV2: RUVersion = 2;
}

mod execdetails_impl {
    use crate::*;
    include!("../../execdetails.rs");
}
pub use execdetails_impl::*;

mod runtime_stats_impl {
    use crate::*;
    include!("../../runtime_stats.rs");
    include!("../../tiflash_execution_units.rs");
}
pub use runtime_stats_impl::*;

#[cfg(test)]
#[path = "../../execdetails_1_aster_unit_test.rs"]
mod execdetails_1_aster_unit_test;
