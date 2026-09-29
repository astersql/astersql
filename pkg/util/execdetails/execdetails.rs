// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// SQL 执行明细与 cop 任务汇总。
//
// 对应 Go `util/execdetails`：收集语句级耗时、提交/加锁明细、coprocessor（下推到 TiKV/TiFlash 的计算）任务统计，
// 供慢查询日志、EXPLAIN ANALYZE 与 zap 字段输出。两阶段提交（2PC）相关字段用于描述事务 prewrite/commit。

use std::collections::HashMap;

// ExecDetails contains execution detail information.
// Go 匿名嵌入 CopExecDetails；这里保留为具名字段，后续接线时可恢复嵌入语义。
#[derive(Clone, Default)]
/// 语句执行明细：含 cop 明细、提交/加锁明细与汇总耗时、请求次数。
pub struct ExecDetails {
    pub CopExecDetails: CopExecDetails,
    pub CommitDetail: Option<util::CommitDetails>,
    pub LockKeysDetail: Option<util::LockKeysDetails>,
    pub SharedLockKeysDetail: Option<util::LockKeysDetails>,
    pub ReadPoolTaskDetails: Option<util::PoolTaskDetails>,
    pub CopTime: time::Duration,
    pub LockKeysDuration: time::Duration,
    pub RequestCount: i32,
}

// CopExecDetails contains cop execution detail information.
#[derive(Clone, Default)]
/// 单次/汇总的 coprocessor 执行明细：扫描、耗时、退避与 callee 地址。
pub struct CopExecDetails {
    pub ScanDetail: Option<util::ScanDetail>,
    pub TimeDetail: util::TimeDetail,
    pub CalleeAddress: String,
    pub BackoffTime: time::Duration,
    pub BackoffSleep: HashMap<String, time::Duration>,
    pub BackoffTimes: HashMap<String, i32>,
}

// P90BackoffSummary contains execution summary for a backoff type.
#[derive(Clone, Default)]
/// 某一退避类型的 P90 汇总：请求次数、百分位、总退避时间与次数。
pub struct P90BackoffSummary {
    pub ReqTimes: i32,
    pub BackoffPercentile: Percentile<DurationWithAddr>,
    pub TotBackoffTime: time::Duration,
    pub TotBackoffTimes: i32,
}

// P90Summary contains execution summary for cop tasks.
#[derive(Clone, Default)]
/// 多个 cop 任务的 P90 汇总：处理/等待时间百分位与按类型的退避信息。
pub struct P90Summary {
    pub NumCopTasks: i32,
    pub ProcessTimePercentile: Percentile<DurationWithAddr>,
    pub WaitTimePercentile: Percentile<DurationWithAddr>,
    pub BackoffInfo: HashMap<String, P90BackoffSummary>,
}

// MaxDetailsNumsForOneQuery is the max number of details to keep for P90 for one query.
/// 单条查询为 P90 保留的明细条数上限。
pub const MaxDetailsNumsForOneQuery: usize = 1000;

impl P90Summary {
    // Reset resets all fields in DetailsNeedP90Summary.
    /// 清空 P90 汇总各字段，便于复用。
    pub fn Reset(&mut self) {
        self.NumCopTasks = 0;
        self.ProcessTimePercentile = Percentile::default();
        self.WaitTimePercentile = Percentile::default();
        self.BackoffInfo = HashMap::new();
    }

    // Merge merges DetailsNeedP90 into P90Summary.
    /// 将一次 cop 任务的退避与时间明细并入 P90 汇总。
    pub fn Merge(
        &mut self,
        backoffSleep: &HashMap<String, time::Duration>,
        backoffTimes: &HashMap<String, i32>,
        calleeAddress: &str,
        timeDetail: util::TimeDetail,
    ) {
        // Go 只在 BackoffInfo 为 nil 时 Reset。Rust 的 HashMap 默认值已经
        // 可直接写入，不能把“已初始化但为空”误判为 nil，否则会清掉调用方
        // 预先放入 ProcessTimePercentile / WaitTimePercentile 的样本。
        self.NumCopTasks += 1;
        self.ProcessTimePercentile.Add(DurationWithAddr {
            D: timeDetail.ProcessTime,
            Addr: calleeAddress.to_string(),
        });
        self.WaitTimePercentile.Add(DurationWithAddr {
            D: timeDetail.WaitTime,
            Addr: calleeAddress.to_string(),
        });

        for (backoff, timeItem) in backoffTimes {
            let info = self
                .BackoffInfo
                .entry(backoff.clone())
                .or_insert_with(P90BackoffSummary::default);
            let sleepItem = backoffSleep
                .get(backoff)
                .cloned()
                .unwrap_or_else(time::Duration::default);
            info.ReqTimes += 1;
            info.TotBackoffTime += sleepItem;
            info.TotBackoffTimes += *timeItem;

            info.BackoffPercentile.Add(DurationWithAddr {
                D: sleepItem,
                Addr: calleeAddress.to_string(),
            });
        }
    }
}

// stmtExecDetailKeyType 对应 Go 中 context key 的空结构体类型。
/// 语句执行明细在 context 中的 key 类型（对应 Go 空结构体）。
pub struct stmtExecDetailKeyType {}

// StmtExecDetailKey used to carry StmtExecDetail info in context.Context.
/// 在 context 中携带 `StmtExecDetails` 的静态 key。
pub static StmtExecDetailKey: stmtExecDetailKeyType = stmtExecDetailKeyType {};

// StmtExecDetails contains stmt level execution detail info.
#[derive(Clone, Default)]
/// 语句级执行明细：写回 SQL 响应耗时与可选的 RU v2 指标。
pub struct StmtExecDetails {
    pub WriteSQLRespDuration: time::Duration,
    ruv2Metrics: Option<RUV2Metrics>,
    ruv2MetricsStorage: RUV2Metrics,
}

impl StmtExecDetails {
    // ensureRUV2Metrics 对应 Go 的懒初始化：nil receiver 返回 nil，否则复用内嵌 storage。
    /// 懒初始化并返回可变的 RU v2 指标；对应 Go 的 ensure 语义。
    pub fn ensureRUV2Metrics(&mut self) -> Option<&mut RUV2Metrics> {
        if self.ruv2Metrics.is_none() {
            self.ruv2Metrics = Some(self.ruv2MetricsStorage.clone());
        }
        self.ruv2Metrics.as_mut()
    }

    // getRUV2Metrics 对应 Go 的只读取指针，不触发懒初始化。
    /// 只读获取 RU v2 指标，不触发懒初始化。
    pub fn getRUV2Metrics(&self) -> Option<&RUV2Metrics> {
        self.ruv2Metrics.as_ref()
    }

    // setRUV2Metrics 对应 Go 的 setter；nil receiver 防御在 实现中由调用侧承担。
    /// 设置 RU v2 指标指针/值。
    pub fn setRUV2Metrics(&mut self, metrics: Option<RUV2Metrics>) {
        self.ruv2Metrics = metrics;
    }
}

// CopTimeStr represents the sum of cop-task time spend in TiDB distSQL.
/// distSQL 中所有 cop 任务耗时之和的字段名。
pub const CopTimeStr: &str = "Cop_time";
// WaitTimeStr means the time of all coprocessor wait.
/// coprocessor 等待时间字段名。
pub const WaitTimeStr: &str = "Wait_time";
// LockKeysTimeStr means the time interval between pessimistic lock wait start and lock got obtain
/// 悲观锁等待到获锁的时间间隔字段名。
pub const LockKeysTimeStr: &str = "LockKeys_time";
// RequestCountStr means the request count.
/// 请求次数字段名。
pub const RequestCountStr: &str = "Request_count";
// WaitPrewriteBinlogTimeStr means the time of waiting prewrite binlog finished when transaction committing.
/// 事务提交时等待 prewrite binlog 完成的时间字段名。
pub const WaitPrewriteBinlogTimeStr: &str = "Wait_prewrite_binlog_time";
// GetCommitTSTimeStr means the time of getting commit ts.
/// 获取 commit ts（提交时间戳）的耗时字段名。
pub const GetCommitTSTimeStr: &str = "Get_commit_ts_time";
// GetLatestTsTimeStr means the time of getting latest ts in async commit and 1pc.
/// 异步提交/1PC 中获取 latest ts 的耗时字段名。
pub const GetLatestTsTimeStr: &str = "Get_latest_ts_time";
// CommitBackoffTimeStr means the time of commit backoff.
/// 提交阶段退避总耗时字段名。
pub const CommitBackoffTimeStr: &str = "Commit_backoff_time";
// BackoffTypesStr means the backoff type.
/// 退避类型列表字段名。
pub const BackoffTypesStr: &str = "Backoff_types";
// SlowestPrewriteRPCDetailStr means the details of the slowest RPC during the transaction 2pc prewrite process.
/// 两阶段提交 prewrite 阶段最慢 RPC 明细字段名。
pub const SlowestPrewriteRPCDetailStr: &str = "Slowest_prewrite_rpc_detail";
// CommitPrimaryRPCDetailStr means the details of the slowest RPC during the transaction 2pc commit process.
/// 两阶段提交 commit 主 key 最慢 RPC 明细字段名。
pub const CommitPrimaryRPCDetailStr: &str = "Commit_primary_rpc_detail";
// ResolveLockTimeStr means the time of resolving lock.
/// 解决锁（resolve lock）耗时字段名。
pub const ResolveLockTimeStr: &str = "Resolve_lock_time";
// LocalLatchWaitTimeStr means the time of waiting in local latch.
/// 本地闩锁等待耗时字段名。
pub const LocalLatchWaitTimeStr: &str = "Local_latch_wait_time";
// TxnRetryStr means the count of transaction retry.
/// 事务重试次数字段名。
pub const TxnRetryStr: &str = "Txn_retry";
// GetSnapshotTimeStr means the time spent on getting an engine snapshot.
/// 获取引擎快照耗时字段名。
pub const GetSnapshotTimeStr: &str = "Get_snapshot_time";
// RocksdbDeleteSkippedCountStr means the count of rocksdb delete skipped count.
/// RocksDB 跳过删除标记次数字段名。
pub const RocksdbDeleteSkippedCountStr: &str = "Rocksdb_delete_skipped_count";
// RocksdbKeySkippedCountStr means the count of rocksdb key skipped count.
/// RocksDB 跳过 key 次数字段名。
pub const RocksdbKeySkippedCountStr: &str = "Rocksdb_key_skipped_count";
// RocksdbBlockCacheHitCountStr means the count of rocksdb block cache hit.
/// RocksDB block cache 命中次数字段名。
pub const RocksdbBlockCacheHitCountStr: &str = "Rocksdb_block_cache_hit_count";
// RocksdbBlockReadCountStr means the count of rocksdb block read.
/// RocksDB block 读取次数字段名。
pub const RocksdbBlockReadCountStr: &str = "Rocksdb_block_read_count";
// RocksdbBlockReadByteStr means the bytes of rocksdb block read.
/// RocksDB block 读取字节数字段名。
pub const RocksdbBlockReadByteStr: &str = "Rocksdb_block_read_byte";
// RocksdbBlockReadTimeStr means the time spent on rocksdb block read.
/// RocksDB block 读取耗时字段名。
pub const RocksdbBlockReadTimeStr: &str = "Rocksdb_block_read_time";
pub const ReadPoolTaskDetailsStr: &str = "Read_pool_task_details";
pub const IARemoteReadSegmentCountStr: &str = "IA_remote_read_segment_count";
pub const IARemoteReadSegmentSizeStr: &str = "IA_remote_read_segment_size";
pub const IARemoteReadSegmentWaitTimeStr: &str = "IA_remote_read_segment_wait_time";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IARemoteReadSegmentStats {
    pub Count: u64,
    pub Bytes: u64,
    pub WaitTime: time::Duration,
}

pub fn GetIARemoteReadSegmentStats(
    scanDetail: Option<&util::ScanDetail>,
) -> IARemoteReadSegmentStats {
    let Some(scanDetail) = scanDetail else {
        return IARemoteReadSegmentStats::default();
    };
    IARemoteReadSegmentStats {
        Count: scanDetail.IaRemoteReadSegmentCount,
        Bytes: scanDetail.IaRemoteReadSegmentBytes,
        WaitTime: scanDetail.IaRemoteReadSegmentDuration,
    }
}

// The following constants define the set of fields for SlowQueryLogItems
// that are relevant to evaluating and triggering SlowLogRules.

// ProcessTimeStr represents the sum of process time of all the coprocessor tasks.
/// 所有 cop 任务处理时间之和的字段名（慢日志规则相关）。
pub const ProcessTimeStr: &str = "Process_time";
// BackoffTimeStr means the time of all back-off.
/// 全部退避时间字段名。
pub const BackoffTimeStr: &str = "Backoff_time";
// TotalKeysStr means the total scan keys.
/// 扫描 key 总数字段名。
pub const TotalKeysStr: &str = "Total_keys";
// ProcessKeysStr means the total processed keys.
/// 已处理 key 数字段名。
pub const ProcessKeysStr: &str = "Process_keys";
// PreWriteTimeStr means the time of pre-write.
/// 两阶段提交 prewrite 耗时字段名。
pub const PreWriteTimeStr: &str = "Prewrite_time";
// CommitTimeStr means the time of commit.
/// 两阶段提交 commit 耗时字段名。
pub const CommitTimeStr: &str = "Commit_time";
// WriteKeysStr means the count of keys in the transaction.
/// 事务写入 key 数数字段名。
pub const WriteKeysStr: &str = "Write_keys";
// WriteSizeStr means the key/value size in the transaction.
/// 事务写入 key/value 大小字段名。
pub const WriteSizeStr: &str = "Write_size";
// PrewriteRegionStr means the count of region when pre-write.
/// prewrite 涉及的 Region 数字段名。Region 是 TiKV 数据分片。
pub const PrewriteRegionStr: &str = "Prewrite_region";

impl ExecDetails {
    // String implements the fmt.Stringer interface.
    /// 将非零字段按 Go 顺序拼成空格分隔字符串，供慢查询等输出。
    pub fn String(&self) -> String {
        // 仅追加非零字段，字段顺序需与 Go String 保持一致。
        let mut parts: Vec<String> = Vec::with_capacity(8);
        if self.CopTime > time::Duration::default() {
            parts.push(format!("{}: {}", CopTimeStr, seconds(self.CopTime)));
        }
        if self.CopExecDetails.TimeDetail.ProcessTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                ProcessTimeStr,
                seconds(self.CopExecDetails.TimeDetail.ProcessTime)
            ));
        }
        if self.CopExecDetails.TimeDetail.WaitTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                WaitTimeStr,
                seconds(self.CopExecDetails.TimeDetail.WaitTime)
            ));
        }
        if self.CopExecDetails.BackoffTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                BackoffTimeStr,
                seconds(self.CopExecDetails.BackoffTime)
            ));
        }
        if let Some(lockKeyDetails) = &self.LockKeysDetail {
            if lockKeyDetails.TotalTime > time::Duration::default() {
                parts.push(format!(
                    "{}: {}",
                    LockKeysTimeStr,
                    seconds(lockKeyDetails.TotalTime)
                ));
            }
        }
        if self.RequestCount > 0 {
            parts.push(format!("{}: {}", RequestCountStr, self.RequestCount));
        }
        if let Some(pool) = self
            .ReadPoolTaskDetails
            .as_ref()
            .filter(|pool| !pool.Empty())
        {
            parts.push(format!("{}: {}", ReadPoolTaskDetailsStr, pool.String()));
        }

        if let Some(commitDetails) = &self.CommitDetail {
            self.push_commit_details(&mut parts, commitDetails);
        }

        if let Some(scanDetail) = &self.CopExecDetails.ScanDetail {
            // 扫描明细仅输出非零字段，顺序保持 Go String 方法中的 append 顺序。
            if scanDetail.ProcessedKeys > 0 {
                parts.push(format!("{}: {}", ProcessKeysStr, scanDetail.ProcessedKeys));
            }
            if scanDetail.TotalKeys > 0 {
                parts.push(format!("{}: {}", TotalKeysStr, scanDetail.TotalKeys));
            }
            if scanDetail.GetSnapshotDuration > time::Duration::default() {
                parts.push(format!(
                    "{}: {:.3}",
                    GetSnapshotTimeStr,
                    seconds(scanDetail.GetSnapshotDuration)
                ));
            }
            if scanDetail.RocksdbDeleteSkippedCount > 0 {
                parts.push(format!(
                    "{}: {}",
                    RocksdbDeleteSkippedCountStr, scanDetail.RocksdbDeleteSkippedCount
                ));
            }
            if scanDetail.RocksdbKeySkippedCount > 0 {
                parts.push(format!(
                    "{}: {}",
                    RocksdbKeySkippedCountStr, scanDetail.RocksdbKeySkippedCount
                ));
            }
            if scanDetail.RocksdbBlockCacheHitCount > 0 {
                parts.push(format!(
                    "{}: {}",
                    RocksdbBlockCacheHitCountStr, scanDetail.RocksdbBlockCacheHitCount
                ));
            }
            if scanDetail.RocksdbBlockReadCount > 0 {
                parts.push(format!(
                    "{}: {}",
                    RocksdbBlockReadCountStr, scanDetail.RocksdbBlockReadCount
                ));
            }
            if scanDetail.RocksdbBlockReadByte > 0 {
                parts.push(format!(
                    "{}: {}",
                    RocksdbBlockReadByteStr, scanDetail.RocksdbBlockReadByte
                ));
            }
            if scanDetail.RocksdbBlockReadDuration > time::Duration::default() {
                parts.push(format!(
                    "{}: {:.3}",
                    RocksdbBlockReadTimeStr,
                    seconds(scanDetail.RocksdbBlockReadDuration)
                ));
            }
        }
        parts.join(" ")
    }

    // push_commit_details 保留 Go String 中 commitDetails != nil 后的一长段字段拼接。
    /// 拼接提交明细中非零字段（含 2PC、退避、resolve lock 等）。
    fn push_commit_details(&self, parts: &mut Vec<String>, commitDetails: &util::CommitDetails) {
        if commitDetails.PrewriteTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                PreWriteTimeStr,
                seconds(commitDetails.PrewriteTime)
            ));
        }
        if commitDetails.WaitPrewriteBinlogTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                WaitPrewriteBinlogTimeStr,
                seconds(commitDetails.WaitPrewriteBinlogTime)
            ));
        }
        if commitDetails.CommitTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                CommitTimeStr,
                seconds(commitDetails.CommitTime)
            ));
        }
        if commitDetails.GetCommitTsTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                GetCommitTSTimeStr,
                seconds(commitDetails.GetCommitTsTime)
            ));
        }
        if commitDetails.GetLatestTsTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                GetLatestTsTimeStr,
                seconds(commitDetails.GetLatestTsTime)
            ));
        }

        let mu = commitDetails.Mu.Lock();
        let commitBackoffTime = mu.CommitBackoffTime;
        if commitBackoffTime > 0 {
            parts.push(format!(
                "{}: {}",
                CommitBackoffTimeStr,
                seconds(time::Duration::from_nanos(commitBackoffTime as u64))
            ));
        }
        if !mu.PrewriteBackoffTypes.is_empty() {
            parts.push(format!(
                "Prewrite_{}: {}",
                BackoffTypesStr,
                format_go_string_slice(&mu.PrewriteBackoffTypes)
            ));
        }
        if !mu.CommitBackoffTypes.is_empty() {
            parts.push(format!(
                "Commit_{}: {}",
                BackoffTypesStr,
                format_go_string_slice(&mu.CommitBackoffTypes)
            ));
        }
        if mu.SlowestPrewrite.ReqTotalTime > time::Duration::default() {
            parts.push(format!(
                "{}: {{total:{:.3}s, region_id: {}, store: {}, {}}}",
                SlowestPrewriteRPCDetailStr,
                seconds(mu.SlowestPrewrite.ReqTotalTime),
                mu.SlowestPrewrite.Region,
                mu.SlowestPrewrite.StoreAddr,
                mu.SlowestPrewrite.ExecDetails.String()
            ));
        }
        if mu.CommitPrimary.ReqTotalTime > time::Duration::default() {
            parts.push(format!(
                "{}: {{total:{:.3}s, region_id: {}, store: {}, {}}}",
                CommitPrimaryRPCDetailStr,
                seconds(mu.CommitPrimary.ReqTotalTime),
                mu.CommitPrimary.Region,
                mu.CommitPrimary.StoreAddr,
                mu.CommitPrimary.ExecDetails.String()
            ));
        }
        drop(mu);

        // Go 使用 atomic.LoadInt64 读取 ResolveLockTime；兼容层 helper 保留同样的原子读取语义。
        let resolveLockTime = atomic::LoadInt64(&commitDetails.ResolveLock.ResolveLockTime);
        if resolveLockTime > 0 {
            parts.push(format!(
                "{}: {}",
                ResolveLockTimeStr,
                seconds(time::Duration::from_nanos(resolveLockTime as u64))
            ));
        }
        if commitDetails.LocalLatchTime > time::Duration::default() {
            parts.push(format!(
                "{}: {}",
                LocalLatchWaitTimeStr,
                seconds(commitDetails.LocalLatchTime)
            ));
        }
        if commitDetails.WriteKeys > 0 {
            parts.push(format!("{}: {}", WriteKeysStr, commitDetails.WriteKeys));
        }
        if commitDetails.WriteSize > 0 {
            parts.push(format!("{}: {}", WriteSizeStr, commitDetails.WriteSize));
        }
        let prewriteRegionNum = atomic::LoadInt32(&commitDetails.PrewriteRegionNum);
        if prewriteRegionNum > 0 {
            parts.push(format!("{}: {}", PrewriteRegionStr, prewriteRegionNum));
        }
        if commitDetails.TxnRetry > 0 {
            parts.push(format!("{}: {}", TxnRetryStr, commitDetails.TxnRetry));
        }
    }

    // ToZapFields wraps the ExecDetails as zap.Fields.
    /// 将执行明细包装为 zap 日志字段（小写 key，过滤零值）。
    pub fn ToZapFields(&self) -> Vec<zap::Field> {
        let mut fields: Vec<zap::Field> = Vec::with_capacity(16);
        if self.CopTime > time::Duration::default() {
            fields.push(zap::String(
                &CopTimeStr.to_lowercase(),
                format!("{}s", seconds(self.CopTime)),
            ));
        }
        if self.CopExecDetails.TimeDetail.ProcessTime > time::Duration::default() {
            fields.push(zap::String(
                &ProcessTimeStr.to_lowercase(),
                format!("{}s", seconds(self.CopExecDetails.TimeDetail.ProcessTime)),
            ));
        }
        if self.CopExecDetails.TimeDetail.WaitTime > time::Duration::default() {
            fields.push(zap::String(
                &WaitTimeStr.to_lowercase(),
                format!("{}s", seconds(self.CopExecDetails.TimeDetail.WaitTime)),
            ));
        }
        if self.CopExecDetails.BackoffTime > time::Duration::default() {
            fields.push(zap::String(
                &BackoffTimeStr.to_lowercase(),
                format!("{}s", seconds(self.CopExecDetails.BackoffTime)),
            ));
        }
        if self.RequestCount > 0 {
            fields.push(zap::String(
                &RequestCountStr.to_lowercase(),
                self.RequestCount.to_string(),
            ));
        }
        if let Some(pool) = self
            .ReadPoolTaskDetails
            .as_ref()
            .filter(|pool| !pool.Empty())
        {
            fields.push(zap::String(
                &ReadPoolTaskDetailsStr.to_lowercase(),
                pool.String(),
            ));
        }
        if let Some(scanDetail) = &self.CopExecDetails.ScanDetail {
            if scanDetail.TotalKeys > 0 {
                fields.push(zap::String(
                    &TotalKeysStr.to_lowercase(),
                    scanDetail.TotalKeys.to_string(),
                ));
            }
            if scanDetail.ProcessedKeys > 0 {
                fields.push(zap::String(
                    &ProcessKeysStr.to_lowercase(),
                    scanDetail.ProcessedKeys.to_string(),
                ));
            }
        }

        if let Some(commitDetails) = &self.CommitDetail {
            self.push_commit_zap_fields(&mut fields, commitDetails);
        }
        fields
    }

    // push_commit_zap_fields 对应 Go ToZapFields 中 commitDetails 分支。
    /// 将提交明细写入 zap 字段列表。
    fn push_commit_zap_fields(
        &self,
        fields: &mut Vec<zap::Field>,
        commitDetails: &util::CommitDetails,
    ) {
        if commitDetails.PrewriteTime > time::Duration::default() {
            fields.push(zap::String(
                "prewrite_time",
                format!("{}s", seconds(commitDetails.PrewriteTime)),
            ));
        }
        if commitDetails.CommitTime > time::Duration::default() {
            fields.push(zap::String(
                "commit_time",
                format!("{}s", seconds(commitDetails.CommitTime)),
            ));
        }
        if commitDetails.GetCommitTsTime > time::Duration::default() {
            fields.push(zap::String(
                "get_commit_ts_time",
                format!("{}s", seconds(commitDetails.GetCommitTsTime)),
            ));
        }

        let mu = commitDetails.Mu.Lock();
        let commitBackoffTime = mu.CommitBackoffTime;
        if commitBackoffTime > 0 {
            fields.push(zap::String(
                "commit_backoff_time",
                format!(
                    "{}s",
                    seconds(time::Duration::from_nanos(commitBackoffTime as u64))
                ),
            ));
        }
        if !mu.PrewriteBackoffTypes.is_empty() {
            fields.push(zap::String(
                &format!("Prewrite_{}", BackoffTypesStr),
                format_go_string_slice(&mu.PrewriteBackoffTypes),
            ));
        }
        if !mu.CommitBackoffTypes.is_empty() {
            fields.push(zap::String(
                &format!("Commit_{}", BackoffTypesStr),
                format_go_string_slice(&mu.CommitBackoffTypes),
            ));
        }
        if mu.SlowestPrewrite.ReqTotalTime > time::Duration::default() {
            fields.push(zap::String(
                SlowestPrewriteRPCDetailStr,
                format!(
                    "total:{:.3}s, region_id: {}, store: {}, {}}}",
                    seconds(mu.SlowestPrewrite.ReqTotalTime),
                    mu.SlowestPrewrite.Region,
                    mu.SlowestPrewrite.StoreAddr,
                    mu.SlowestPrewrite.ExecDetails.String()
                ),
            ));
        }
        if mu.CommitPrimary.ReqTotalTime > time::Duration::default() {
            fields.push(zap::String(
                CommitPrimaryRPCDetailStr,
                format!(
                    "{{total:{:.3}s, region_id: {}, store: {}, {}}}",
                    seconds(mu.CommitPrimary.ReqTotalTime),
                    mu.CommitPrimary.Region,
                    mu.CommitPrimary.StoreAddr,
                    mu.CommitPrimary.ExecDetails.String()
                ),
            ));
        }
        drop(mu);

        let resolveLockTime = atomic::LoadInt64(&commitDetails.ResolveLock.ResolveLockTime);
        if resolveLockTime > 0 {
            fields.push(zap::String(
                "resolve_lock_time",
                format!(
                    "{}s",
                    seconds(time::Duration::from_nanos(resolveLockTime as u64))
                ),
            ));
        }
        if commitDetails.LocalLatchTime > time::Duration::default() {
            fields.push(zap::String(
                "local_latch_wait_time",
                format!("{}s", seconds(commitDetails.LocalLatchTime)),
            ));
        }
        if commitDetails.WriteKeys > 0 {
            fields.push(zap::Int("write_keys", commitDetails.WriteKeys));
        }
        if commitDetails.WriteSize > 0 {
            fields.push(zap::Int("write_size", commitDetails.WriteSize));
        }
        let prewriteRegionNum = atomic::LoadInt32(&commitDetails.PrewriteRegionNum);
        if prewriteRegionNum > 0 {
            fields.push(zap::Int32("prewrite_region", prewriteRegionNum));
        }
        if commitDetails.TxnRetry > 0 {
            fields.push(zap::Int("txn_retry", commitDetails.TxnRetry));
        }
    }
}

// SyncExecDetails is a synced version of `ExecDetails` and its `P90Summary`
#[derive(Default)]
/// 带互斥保护的 `ExecDetails` 与其 `P90Summary`，供并发合并。
pub struct SyncExecDetails {
    mu: sync::Mutex<SyncExecDetailsInner>,
}

#[derive(Default)]
/// `SyncExecDetails` 的受保护内部状态。
struct SyncExecDetailsInner {
    execDetails: ExecDetails,
    detailsSummary: P90Summary,
}

impl SyncExecDetails {
    // MergeExecDetails merges a single region execution details into self, used to print
    // the information in slow query log.
    /// 合并单 Region/语句的提交明细，供慢查询汇总。
    pub fn MergeExecDetails(&self, commitDetails: Option<util::CommitDetails>) {
        let mut guard = self.mu.Lock();
        if let Some(commitDetails) = commitDetails {
            if guard.execDetails.CommitDetail.is_none() {
                guard.execDetails.CommitDetail = Some(commitDetails);
            } else if let Some(existing) = guard.execDetails.CommitDetail.as_mut() {
                existing.Merge(&commitDetails);
            }
        }
    }

    // MergeCopExecDetails merges a CopExecDetails into self.
    /// 合并一次 cop 执行明细与 cop 耗时，并更新 P90 汇总。
    pub fn MergeCopExecDetails(&self, details: Option<&CopExecDetails>, copTime: time::Duration) {
        let Some(details) = details else {
            return;
        };
        let mut guard = self.mu.Lock();
        guard.execDetails.CopTime += copTime;
        guard.execDetails.CopExecDetails.BackoffTime += details.BackoffTime;
        guard.execDetails.RequestCount += 1;
        Self::mergeScanDetailLocked(&mut guard.execDetails, details.ScanDetail.as_ref());
        Self::mergeTimeDetailLocked(&mut guard.execDetails, details.TimeDetail.clone());
        guard.detailsSummary.Merge(
            &details.BackoffSleep,
            &details.BackoffTimes,
            &details.CalleeAddress,
            details.TimeDetail.clone(),
        );
    }

    // mergeScanDetail merges scan details into self.
    /// 合并扫描明细；TiFlash 可能不填 scanDetail，需跳过 nil。
    fn mergeScanDetailLocked(execDetails: &mut ExecDetails, scanDetail: Option<&util::ScanDetail>) {
        // Currently TiFlash cop task does not fill scanDetail, so need to skip it if scanDetail is nil
        let Some(scanDetail) = scanDetail else {
            return;
        };
        if execDetails.CopExecDetails.ScanDetail.is_none() {
            execDetails.CopExecDetails.ScanDetail = Some(util::ScanDetail::default());
        }
        if let Some(existing) = execDetails.CopExecDetails.ScanDetail.as_mut() {
            existing.Merge(scanDetail);
        }
    }

    /// 合并扫描明细，不改变 cop 任务次数与耗时。
    pub fn MergeScanDetail(&self, scanDetail: Option<&util::ScanDetail>) {
        if scanDetail.is_none() {
            return;
        }
        let mut guard = self.mu.Lock();
        Self::mergeScanDetailLocked(&mut guard.execDetails, scanDetail);
    }

    /// 合并读池任务明细，不改变 cop 任务次数与其他执行统计。
    pub fn MergeReadPoolTaskDetails(&self, details: Option<&util::PoolTaskDetails>) {
        let Some(details) = details.filter(|details| !details.Empty()) else {
            return;
        };
        let mut guard = self.mu.Lock();
        if let Some(existing) = guard.execDetails.ReadPoolTaskDetails.as_mut() {
            existing.Merge(details);
        } else {
            guard.execDetails.ReadPoolTaskDetails = Some(details.Clone());
        }
    }

    // MergeTimeDetail merges time details into self.
    /// 累加处理时间与等待时间。
    fn mergeTimeDetailLocked(execDetails: &mut ExecDetails, timeDetail: util::TimeDetail) {
        execDetails.CopExecDetails.TimeDetail.ProcessTime += timeDetail.ProcessTime;
        execDetails.CopExecDetails.TimeDetail.WaitTime += timeDetail.WaitTime;
    }

    // MergeLockKeysExecDetails merges lock keys execution details into self.
    /// 合并悲观加锁（lock keys）执行明细。
    pub fn MergeLockKeysExecDetails(&self, lockKeys: Option<util::LockKeysDetails>) {
        let mut guard = self.mu.Lock();
        if guard.execDetails.LockKeysDetail.is_none() {
            guard.execDetails.LockKeysDetail = lockKeys;
        } else if let (Some(existing), Some(lockKeys)) =
            (guard.execDetails.LockKeysDetail.as_mut(), lockKeys)
        {
            existing.Merge(&lockKeys);
        }
    }

    // MergeSharedLockKeysExecDetails merges shared lock keys execution details into self.
    /// 合并共享锁加锁执行明细。
    pub fn MergeSharedLockKeysExecDetails(&self, lockKeys: Option<util::LockKeysDetails>) {
        let mut guard = self.mu.Lock();
        if guard.execDetails.SharedLockKeysDetail.is_none() {
            guard.execDetails.SharedLockKeysDetail = lockKeys;
        } else if let (Some(existing), Some(lockKeys)) =
            (guard.execDetails.SharedLockKeysDetail.as_mut(), lockKeys)
        {
            existing.Merge(&lockKeys);
        }
    }

    // Reset resets the content inside
    /// 清空内部执行明细与 P90 汇总。
    pub fn Reset(&self) {
        let mut guard = self.mu.Lock();
        guard.execDetails = ExecDetails::default();
        guard.detailsSummary.Reset();
    }

    // GetExecDetails returns the exec details inside.
    // It's actually not safe, because the `ExecDetails` still contains some reference, which is not protected after returning
    // outside.
    /// 返回执行明细快照（clone）；内部引用在返回后不再受锁保护。
    pub fn GetExecDetails(&self) -> ExecDetails {
        let guard = self.mu.Lock();
        // Go 返回结构体副本但内部仍有引用；用 clone 保留这种快照语义。
        guard.execDetails.clone()
    }

    // CopTasksDetails returns some useful information of cop-tasks during execution.
    /// 汇总 cop 任务的平均/P90/最大处理与等待时间及退避统计。
    pub fn CopTasksDetails(&self) -> Option<CopTasksDetails> {
        let mut guard = self.mu.Lock();
        let n = guard.detailsSummary.NumCopTasks;
        if n == 0 {
            return None;
        }
        // 用任务数做平均分母；n==0 已在上方提前返回。
        let divisor = u32::try_from(n).expect("cop task count must be positive");
        let process_total = guard.execDetails.CopExecDetails.TimeDetail.ProcessTime;
        let process_p90 = guard
            .detailsSummary
            .ProcessTimePercentile
            .GetPercentile(0.9);
        let process_max = guard
            .detailsSummary
            .ProcessTimePercentile
            .GetMax()
            .expect("cop process percentile contains one sample per task");
        let wait_total = guard.execDetails.CopExecDetails.TimeDetail.WaitTime;
        let wait_p90 = guard.detailsSummary.WaitTimePercentile.GetPercentile(0.9);
        let wait_max = guard
            .detailsSummary
            .WaitTimePercentile
            .GetMax()
            .expect("cop wait percentile contains one sample per task");
        let mut d = CopTasksDetails {
            NumCopTasks: n,
            ..CopTasksDetails::default()
        };
        d.ProcessTimeStats = TaskTimeStats {
            TotTime: process_total,
            AvgTime: process_total / divisor,
            P90Time: time::Duration::from_nanos(process_p90 as u64),
            MaxTime: process_max.D,
            MaxAddress: process_max.Addr,
        };

        d.WaitTimeStats = TaskTimeStats {
            TotTime: wait_total,
            AvgTime: wait_total / divisor,
            P90Time: time::Duration::from_nanos(wait_p90 as u64),
            MaxTime: wait_max.D,
            MaxAddress: wait_max.Addr,
        };

        if !guard.detailsSummary.BackoffInfo.is_empty() {
            d.BackoffTimeStatsMap = HashMap::new();
            d.TotBackoffTimes = HashMap::new();
        }
        for (backoff, items) in &mut guard.detailsSummary.BackoffInfo {
            let n = items.ReqTimes;
            let divisor = u32::try_from(n).expect("backoff request count must be positive");
            let max = items
                .BackoffPercentile
                .GetMax()
                .expect("backoff percentile contains one sample per request");
            let p90 = items.BackoffPercentile.GetPercentile(0.9);
            d.BackoffTimeStatsMap.insert(
                backoff.clone(),
                TaskTimeStats {
                    MaxAddress: max.Addr,
                    MaxTime: max.D,
                    P90Time: time::Duration::from_nanos(p90 as u64),
                    AvgTime: items.TotBackoffTime / divisor,
                    TotTime: items.TotBackoffTime,
                },
            );
            d.TotBackoffTimes
                .insert(backoff.clone(), items.TotBackoffTimes);
        }
        Some(d)
    }

    // CopTasksSummary returns some summary information of cop-tasks for statement summary.
    /// 为语句摘要提供更精简的 cop 任务汇总。
    pub fn CopTasksSummary(&self) -> Option<CopTasksSummary> {
        let guard = self.mu.Lock();
        let n = guard.detailsSummary.NumCopTasks;
        if n == 0 {
            return None;
        }
        let process_max = guard
            .detailsSummary
            .ProcessTimePercentile
            .GetMax()
            .expect("cop process percentile contains one sample per task");
        let wait_max = guard
            .detailsSummary
            .WaitTimePercentile
            .GetMax()
            .expect("cop wait percentile contains one sample per task");
        Some(CopTasksSummary {
            NumCopTasks: n,
            MaxProcessAddress: process_max.Addr,
            MaxProcessTime: process_max.D,
            TotProcessTime: guard.execDetails.CopExecDetails.TimeDetail.ProcessTime,
            MaxWaitAddress: wait_max.Addr,
            MaxWaitTime: wait_max.D,
            TotWaitTime: guard.execDetails.CopExecDetails.TimeDetail.WaitTime,
        })
    }
}

// CopTasksDetails collects some useful information of cop-tasks during execution.
#[derive(Clone, Default)]
/// 执行期间 cop 任务的详细统计（含退避 map）。
pub struct CopTasksDetails {
    pub NumCopTasks: i32,
    pub ProcessTimeStats: TaskTimeStats,
    pub WaitTimeStats: TaskTimeStats,
    pub BackoffTimeStatsMap: HashMap<String, TaskTimeStats>,
    pub TotBackoffTimes: HashMap<String, i32>,
}

// TaskTimeStats is used for recording time-related statistical metrics, including dimensions such as average values, percentile values, maximum values, etc.
// It is suitable for scenarios involving latency statistics, wait time analysis, and similar use cases.
#[derive(Clone, Default)]
/// 时间类统计：平均、P90、最大及最大所在地址、总计。
pub struct TaskTimeStats {
    pub AvgTime: time::Duration,
    pub P90Time: time::Duration,
    pub MaxAddress: String,
    pub MaxTime: time::Duration,
    pub TotTime: time::Duration,
}

impl TaskTimeStats {
    // String returns the TaskTimeStats fields as a string.
    /// 按任务数选择简洁或完整格式，输出时间统计字符串。
    pub fn String(
        &self,
        numCopTasks: i32,
        spaceMarkStr: &str,
        avgStr: &str,
        p90Str: &str,
        maxStr: &str,
        addrStr: &str,
    ) -> String {
        if numCopTasks == 1 {
            return format!(
                "{}{}{} {}{}{}",
                avgStr,
                spaceMarkStr,
                seconds(self.AvgTime),
                addrStr,
                spaceMarkStr,
                self.MaxAddress
            );
        }

        format!(
            "{}{}{} {}{}{} {}{}{} {}{}{}",
            avgStr,
            spaceMarkStr,
            seconds(self.AvgTime),
            p90Str,
            spaceMarkStr,
            seconds(self.P90Time),
            maxStr,
            spaceMarkStr,
            seconds(self.MaxTime),
            addrStr,
            spaceMarkStr,
            self.MaxAddress
        )
    }

    // FormatFloatFields returns the AvgTime, P90Time and MaxTime in float format.
    /// 将 Avg/P90/Max 转为浮点秒数字符串三元组。
    pub fn FormatFloatFields(&self) -> (String, String, String) {
        (
            seconds(self.AvgTime).to_string(),
            seconds(self.P90Time).to_string(),
            seconds(self.MaxTime).to_string(),
        )
    }
}

// CopTasksSummary collects some summary information of cop-tasks for statement summary.
#[derive(Clone, Default)]
/// 面向语句摘要的 cop 任务精简汇总。
pub struct CopTasksSummary {
    pub NumCopTasks: i32,
    pub MaxProcessAddress: String,
    pub MaxProcessTime: time::Duration,
    pub TotProcessTime: time::Duration,
    pub MaxWaitAddress: String,
    pub MaxWaitTime: time::Duration,
    pub TotWaitTime: time::Duration,
}

impl CopTasksDetails {
    // ToZapFields wraps the CopTasksDetails as zap.Fileds.
    /// 将 cop 任务明细包装为 zap 字段；无任务时返回空。
    pub fn ToZapFields(&self) -> Vec<zap::Field> {
        if self.NumCopTasks == 0 {
            return Vec::new();
        }
        let mut fields: Vec<zap::Field> = Vec::with_capacity(10);
        fields.push(zap::Int("num_cop_tasks", self.NumCopTasks));
        let (avgStr, p90Str, maxStr) = self.ProcessTimeStats.FormatFloatFields();
        fields.push(zap::String("process_avg_time", format!("{}s", avgStr)));
        fields.push(zap::String("process_p90_time", format!("{}s", p90Str)));
        fields.push(zap::String("process_max_time", format!("{}s", maxStr)));
        fields.push(zap::String(
            "process_max_addr",
            self.ProcessTimeStats.MaxAddress.clone(),
        ));
        let (avgStr, p90Str, maxStr) = self.WaitTimeStats.FormatFloatFields();
        fields.push(zap::String("wait_avg_time", format!("{}s", avgStr)));
        fields.push(zap::String("wait_p90_time", format!("{}s", p90Str)));
        fields.push(zap::String("wait_max_time", format!("{}s", maxStr)));
        fields.push(zap::String(
            "wait_max_addr",
            self.WaitTimeStats.MaxAddress.clone(),
        ));
        fields
    }
}

// seconds 对应 Go 的 time.Duration.Seconds()，用于集中表达耗时转浮点秒数。
/// 将 Duration 转为浮点秒，对应 Go `time.Duration.Seconds()`。
fn seconds(d: time::Duration) -> f64 {
    d.as_secs_f64()
}

/// Match Go's `fmt.Sprintf("%v", []string{...})` representation.
fn format_go_string_slice(values: &[String]) -> String {
    format!("[{}]", values.join(" "))
}
