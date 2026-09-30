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

// 语句摘要单条聚合记录（对应 Go `record.go`）。
//
// `StmtRecord` 按 SQL digest / 计划摘要等维度累计一次语句执行的延迟、Coprocessor、
// 两阶段提交（2PC）、资源单位（RU）等指标；`Add` 累加单次执行，`Merge` 合并两条记录。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use task_execdetails::execdetails::{
    CopExecDetails, CopTasksSummary, ExecDetails,
    util::{CommitDetails, RUDetails, ScanDetail, TimeDetail},
};
use task_execdetails::util::LoadTiKVExecDetails;
use task_stmtctx::{NewStmtCtx, TableEntry};
use task_stmtsummary::{StmtExecInfo, StmtExecLazyInfo};

/// Select the RU values shown by statement summary for the active RU version.
/// A statement without a finalized v2 total (for example a cursor fetch)
/// continues to expose its original RU details.
pub fn SelectRUDetailsForStatementSummary(
    raw: Option<RUDetails>,
    version: u8,
    total_ru_v2: Option<f64>,
    is_write: bool,
) -> Option<RUDetails> {
    if version != 2 {
        return raw;
    }
    let Some(total) = total_ru_v2 else {
        return raw;
    };
    let wait = raw
        .as_ref()
        .map_or(Duration::ZERO, RUDetails::RUWaitDuration);
    Some(RUDetails {
        read_ru: if is_write { 0.0 } else { total },
        write_ru: if is_write { total } else { 0.0 },
        ru_wait_duration: wait,
        ..Default::default()
    })
}

/// 文本执行计划超限时的占位串。
const PLAN_DISCARDED_ENCODED: &str = "[discard]";
/// 编码计划（文本/二进制）保留上限，默认 1MiB。
static MAX_ENCODED_PLAN_SIZE_IN_BYTES: AtomicUsize = AtomicUsize::new(1024 * 1024);
/// 采样 SQL 最大长度（字符字节上限），可由系统变量改写。
static GLOBAL_MAX_SQL_LENGTH: AtomicU32 = AtomicU32::new(32768);

mod durationNanosSerde {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        let nanos = i64::try_from(value.as_nanos()).map_err(serde::ser::Error::custom)?;
        serializer.serialize_i64(nanos)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let nanos = i64::deserialize(deserializer)?;
        if nanos < 0 {
            return Err(serde::de::Error::custom("negative IA wait duration"));
        }
        Ok(Duration::from_nanos(nanos as u64))
    }
}

/// Upper bound for the textual and binary plans retained by statement summary.
/// 返回当前允许保留的编码计划最大字节数。
pub fn MaxEncodedPlanSizeInBytes() -> usize {
    MAX_ENCODED_PLAN_SIZE_IN_BYTES.load(Ordering::Relaxed)
}

/// 测试用：覆盖编码计划大小上限。
pub fn SetMaxEncodedPlanSizeInBytesForTest(value: usize) {
    MAX_ENCODED_PLAN_SIZE_IN_BYTES.store(value, Ordering::Relaxed);
}

/// 更新全局采样 SQL 最大长度（供摘要配置联动）。
pub(crate) fn setGlobalMaxSQLLength(value: u32) {
    GLOBAL_MAX_SQL_LENGTH.store(value, Ordering::Relaxed);
}

/// 测试用：覆盖全局采样 SQL 最大长度。
pub fn SetGlobalMaxSQLLengthForTest(value: u32) {
    setGlobalMaxSQLLength(value);
}

/// The protobuf payload is ExplainData{discarded_due_to_too_long:true}:
/// field four, boolean true. TiDB encodes it with raw Snappy and base64.
/// 生成“因过长而丢弃”的二进制计划占位（Snappy + base64）。
fn binaryPlanDiscardedEncoded() -> String {
    use base64::Engine as _;
    let compressed = snap::raw::Encoder::new()
        .compress_vec(&[0x20, 0x01])
        .expect("two-byte protobuf always fits in snappy");
    base64::engine::general_purpose::STANDARD.encode(compressed)
}

/// 一条语句摘要记录：身份字段 + 各类 sum/max/min 执行统计。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct StmtRecord {
    #[serde(rename = "begin")]
    pub Begin: i64,
    #[serde(rename = "end")]
    pub End: i64,
    pub SchemaName: String,
    #[serde(rename = "digest")]
    pub Digest: String,
    pub PlanDigest: String,
    pub StmtType: String,
    pub NormalizedSQL: String,
    pub TableNames: String,
    pub IsInternal: bool,
    pub BindingSQL: String,
    pub BindingDigest: String,
    pub SampleSQL: String,
    pub Charset: String,
    pub Collation: String,
    pub PrevSQL: String,
    pub SamplePlan: String,
    pub SampleBinaryPlan: String,
    pub PlanHint: String,
    pub IndexNames: Vec<String>,
    #[serde(rename = "exec_count")]
    pub ExecCount: i64,
    pub SumErrors: i32,
    pub SumWarnings: i32,
    pub SumLatency: Duration,
    pub MaxLatency: Duration,
    pub MinLatency: Duration,
    pub SumParseLatency: Duration,
    pub MaxParseLatency: Duration,
    pub SumCompileLatency: Duration,
    pub MaxCompileLatency: Duration,
    pub SumNumCopTasks: i64,
    pub MaxCopProcessTime: Duration,
    pub MaxCopProcessAddress: String,
    pub MaxCopWaitTime: Duration,
    pub MaxCopWaitAddress: String,
    pub SumProcessTime: Duration,
    pub MaxProcessTime: Duration,
    pub SumWaitTime: Duration,
    pub MaxWaitTime: Duration,
    pub SumBackoffTime: Duration,
    pub MaxBackoffTime: Duration,
    pub SumTotalKeys: i64,
    pub MaxTotalKeys: i64,
    pub SumProcessedKeys: i64,
    pub MaxProcessedKeys: i64,
    pub SumRocksdbDeleteSkippedCount: u64,
    pub MaxRocksdbDeleteSkippedCount: u64,
    pub SumRocksdbKeySkippedCount: u64,
    pub MaxRocksdbKeySkippedCount: u64,
    pub SumRocksdbBlockCacheHitCount: u64,
    pub MaxRocksdbBlockCacheHitCount: u64,
    pub SumRocksdbBlockReadCount: u64,
    pub MaxRocksdbBlockReadCount: u64,
    pub SumRocksdbBlockReadByte: u64,
    pub MaxRocksdbBlockReadByte: u64,
    #[serde(rename = "ia_exec_count")]
    pub IAExecCount: i64,
    #[serde(rename = "sum_ia_remote_read_segment_count")]
    pub SumIARemoteReadSegmentCount: u64,
    #[serde(rename = "max_ia_remote_read_segment_count")]
    pub MaxIARemoteReadSegmentCount: u64,
    #[serde(rename = "sum_ia_remote_read_segment_size")]
    pub SumIARemoteReadSegmentSize: u64,
    #[serde(rename = "max_ia_remote_read_segment_size")]
    pub MaxIARemoteReadSegmentSize: u64,
    #[serde(
        rename = "sum_ia_remote_read_segment_wait_time",
        with = "durationNanosSerde"
    )]
    pub SumIARemoteReadSegmentWaitTime: Duration,
    #[serde(
        rename = "max_ia_remote_read_segment_wait_time",
        with = "durationNanosSerde"
    )]
    pub MaxIARemoteReadSegmentWaitTime: Duration,
    pub CommitCount: i64,
    pub SumGetCommitTsTime: Duration,
    pub MaxGetCommitTsTime: Duration,
    pub SumPrewriteTime: Duration,
    pub MaxPrewriteTime: Duration,
    pub SumCommitTime: Duration,
    pub MaxCommitTime: Duration,
    pub SumLocalLatchTime: Duration,
    pub MaxLocalLatchTime: Duration,
    pub SumCommitBackoffTime: i64,
    pub MaxCommitBackoffTime: i64,
    pub SumResolveLockTime: i64,
    pub MaxResolveLockTime: i64,
    pub SumWriteKeys: i64,
    pub MaxWriteKeys: i32,
    pub SumWriteSize: i64,
    pub MaxWriteSize: i32,
    pub SumPrewriteRegionNum: i64,
    pub MaxPrewriteRegionNum: i32,
    pub SumTxnRetry: i64,
    pub MaxTxnRetry: i32,
    pub SumBackoffTimes: i64,
    pub BackoffTypes: HashMap<String, i32>,
    pub AuthUsers: HashSet<String>,
    pub SumMem: i64,
    pub MaxMem: i64,
    pub SumDisk: i64,
    pub MaxDisk: i64,
    pub SumAffectedRows: u64,
    pub SumKVTotal: Duration,
    pub SumPDTotal: Duration,
    pub SumBackoffTotal: Duration,
    pub SumWriteSQLRespTotal: Duration,
    pub SumTidbCPU: Duration,
    pub SumTikvCPU: Duration,
    pub SumResultRows: i64,
    pub MaxResultRows: i64,
    pub MinResultRows: i64,
    pub Prepared: bool,
    pub FirstSeen: SystemTime,
    pub LastSeen: SystemTime,
    pub PlanInCache: bool,
    pub PlanCacheHits: i64,
    pub PlanInBinding: bool,
    pub ExecRetryCount: u32,
    pub ExecRetryTime: Duration,
    pub KeyspaceName: String,
    pub KeyspaceID: u32,
    pub ResourceGroupName: String,
    pub SumRRU: f64,
    pub SumWRU: f64,
    pub SumRUWaitDuration: Duration,
    pub MaxRRU: f64,
    pub MaxWRU: f64,
    pub MaxRUWaitDuration: Duration,
    pub PlanCacheUnqualifiedCount: i64,
    pub PlanCacheUnqualifiedLastReason: String,
    pub SumMemArbitration: f64,
    pub MaxMemArbitration: f64,
    pub UnpackedBytesSentTiKVTotal: i64,
    pub UnpackedBytesReceivedTiKVTotal: i64,
    pub UnpackedBytesSentTiKVCrossZone: i64,
    pub UnpackedBytesReceivedTiKVCrossZone: i64,
    pub UnpackedBytesSentTiFlashTotal: i64,
    pub UnpackedBytesReceivedTiFlashTotal: i64,
    pub UnpackedBytesSentTiFlashCrossZone: i64,
    pub UnpackedBytesReceivedTiFlashCrossZone: i64,
    pub StorageKV: bool,
    pub StorageMPP: bool,
}

impl Default for StmtRecord {
    /// 零值记录；`MinResultRows` 等在首次 `Add` 前由构造函数另行设置。
    fn default() -> Self {
        Self {
            Begin: 0,
            End: 0,
            SchemaName: String::new(),
            Digest: String::new(),
            PlanDigest: String::new(),
            StmtType: String::new(),
            NormalizedSQL: String::new(),
            TableNames: String::new(),
            IsInternal: false,
            BindingSQL: String::new(),
            BindingDigest: String::new(),
            SampleSQL: String::new(),
            Charset: String::new(),
            Collation: String::new(),
            PrevSQL: String::new(),
            SamplePlan: String::new(),
            SampleBinaryPlan: String::new(),
            PlanHint: String::new(),
            IndexNames: Vec::new(),
            ExecCount: 0,
            SumErrors: 0,
            SumWarnings: 0,
            SumLatency: Duration::ZERO,
            MaxLatency: Duration::ZERO,
            MinLatency: Duration::ZERO,
            SumParseLatency: Duration::ZERO,
            MaxParseLatency: Duration::ZERO,
            SumCompileLatency: Duration::ZERO,
            MaxCompileLatency: Duration::ZERO,
            SumNumCopTasks: 0,
            MaxCopProcessTime: Duration::ZERO,
            MaxCopProcessAddress: String::new(),
            MaxCopWaitTime: Duration::ZERO,
            MaxCopWaitAddress: String::new(),
            SumProcessTime: Duration::ZERO,
            MaxProcessTime: Duration::ZERO,
            SumWaitTime: Duration::ZERO,
            MaxWaitTime: Duration::ZERO,
            SumBackoffTime: Duration::ZERO,
            MaxBackoffTime: Duration::ZERO,
            SumTotalKeys: 0,
            MaxTotalKeys: 0,
            SumProcessedKeys: 0,
            MaxProcessedKeys: 0,
            SumRocksdbDeleteSkippedCount: 0,
            MaxRocksdbDeleteSkippedCount: 0,
            SumRocksdbKeySkippedCount: 0,
            MaxRocksdbKeySkippedCount: 0,
            SumRocksdbBlockCacheHitCount: 0,
            MaxRocksdbBlockCacheHitCount: 0,
            SumRocksdbBlockReadCount: 0,
            MaxRocksdbBlockReadCount: 0,
            SumRocksdbBlockReadByte: 0,
            MaxRocksdbBlockReadByte: 0,
            IAExecCount: 0,
            SumIARemoteReadSegmentCount: 0,
            MaxIARemoteReadSegmentCount: 0,
            SumIARemoteReadSegmentSize: 0,
            MaxIARemoteReadSegmentSize: 0,
            SumIARemoteReadSegmentWaitTime: Duration::ZERO,
            MaxIARemoteReadSegmentWaitTime: Duration::ZERO,
            CommitCount: 0,
            SumGetCommitTsTime: Duration::ZERO,
            MaxGetCommitTsTime: Duration::ZERO,
            SumPrewriteTime: Duration::ZERO,
            MaxPrewriteTime: Duration::ZERO,
            SumCommitTime: Duration::ZERO,
            MaxCommitTime: Duration::ZERO,
            SumLocalLatchTime: Duration::ZERO,
            MaxLocalLatchTime: Duration::ZERO,
            SumCommitBackoffTime: 0,
            MaxCommitBackoffTime: 0,
            SumResolveLockTime: 0,
            MaxResolveLockTime: 0,
            SumWriteKeys: 0,
            MaxWriteKeys: 0,
            SumWriteSize: 0,
            MaxWriteSize: 0,
            SumPrewriteRegionNum: 0,
            MaxPrewriteRegionNum: 0,
            SumTxnRetry: 0,
            MaxTxnRetry: 0,
            SumBackoffTimes: 0,
            BackoffTypes: HashMap::new(),
            AuthUsers: HashSet::new(),
            SumMem: 0,
            MaxMem: 0,
            SumDisk: 0,
            MaxDisk: 0,
            SumAffectedRows: 0,
            SumKVTotal: Duration::ZERO,
            SumPDTotal: Duration::ZERO,
            SumBackoffTotal: Duration::ZERO,
            SumWriteSQLRespTotal: Duration::ZERO,
            SumTidbCPU: Duration::ZERO,
            SumTikvCPU: Duration::ZERO,
            SumResultRows: 0,
            MaxResultRows: 0,
            MinResultRows: 0,
            Prepared: false,
            FirstSeen: UNIX_EPOCH,
            LastSeen: UNIX_EPOCH,
            PlanInCache: false,
            PlanCacheHits: 0,
            PlanInBinding: false,
            ExecRetryCount: 0,
            ExecRetryTime: Duration::ZERO,
            KeyspaceName: String::new(),
            KeyspaceID: 0,
            ResourceGroupName: String::new(),
            SumRRU: 0.0,
            SumWRU: 0.0,
            SumRUWaitDuration: Duration::ZERO,
            MaxRRU: 0.0,
            MaxWRU: 0.0,
            MaxRUWaitDuration: Duration::ZERO,
            PlanCacheUnqualifiedCount: 0,
            PlanCacheUnqualifiedLastReason: String::new(),
            SumMemArbitration: 0.0,
            MaxMemArbitration: 0.0,
            UnpackedBytesSentTiKVTotal: 0,
            UnpackedBytesReceivedTiKVTotal: 0,
            UnpackedBytesSentTiKVCrossZone: 0,
            UnpackedBytesReceivedTiKVCrossZone: 0,
            UnpackedBytesSentTiFlashTotal: 0,
            UnpackedBytesReceivedTiFlashTotal: 0,
            UnpackedBytesSentTiFlashCrossZone: 0,
            UnpackedBytesReceivedTiFlashCrossZone: 0,
            StorageKV: false,
            StorageMPP: false,
        }
    }
}

/// 由单次执行信息构造新记录：填充身份/采样字段，累计指标仍为默认零值。
pub fn NewStmtRecord(info: &StmtExecInfo) -> Box<StmtRecord> {
    let mut table_names = String::new();
    let logical_plan_tables = info.StmtCtx.LogicalPlanTables();
    // 表名统一小写，格式 db.table，逗号分隔。
    for table in &logical_plan_tables {
        if table.Table.is_empty() {
            continue;
        }
        if !table_names.is_empty() {
            table_names.push(',');
        }
        table_names.push_str(&table.DB.to_lowercase());
        table_names.push('.');
        table_names.push_str(&table.Table.to_lowercase());
    }

    let mut plan_digest = info.PlanDigest.clone();
    if plan_digest.is_empty() {
        plan_digest = info.LazyInfo.GetPlanDigest();
    }
    let (mut sample_plan, plan_hint, _) = info.LazyInfo.GetEncodedPlan();
    // 超限文本/二进制计划分别替换为 discard 占位，避免撑爆摘要内存。
    if sample_plan.len() > MaxEncodedPlanSizeInBytes() {
        sample_plan = PLAN_DISCARDED_ENCODED.to_owned();
    }
    let mut binary_plan = info.LazyInfo.GetBinaryPlan();
    if binary_plan.len() > MaxEncodedPlanSizeInBytes() {
        binary_plan = binaryPlanDiscardedEncoded();
    }
    let (binding_sql, binding_digest) = info.LazyInfo.GetBindingSQLAndDigest();

    Box::new(StmtRecord {
        SchemaName: info.SchemaName.clone(),
        Digest: info.Digest.clone(),
        PlanDigest: plan_digest,
        StmtType: info.StmtCtx.StmtType.clone(),
        NormalizedSQL: formatSQL(info.NormalizedSQL.clone()),
        TableNames: table_names,
        IsInternal: info.IsInternal,
        BindingSQL: binding_sql,
        BindingDigest: binding_digest,
        SampleSQL: formatSQL(info.LazyInfo.GetOriginalSQL()),
        Charset: info.Charset.clone(),
        Collation: info.Collation.clone(),
        PrevSQL: info.PrevSQL.clone(),
        SamplePlan: sample_plan,
        SampleBinaryPlan: binary_plan,
        PlanHint: plan_hint,
        IndexNames: info
            .StmtCtx
            .IndexNames
            .lock()
            .expect("statement index names lock poisoned")
            .clone(),
        MinLatency: info.TotalLatency,
        MinResultRows: i64::MAX,
        Prepared: info.Prepared,
        FirstSeen: info.StartTime,
        LastSeen: info.StartTime,
        KeyspaceName: info.KeyspaceName.clone(),
        KeyspaceID: info.KeyspaceID,
        ResourceGroupName: info.ResourceGroupName.clone(),
        ..Default::default()
    })
}

/// 将 value 累加到 sum，并更新 max。
macro_rules! add_sum_max {
    ($self:ident, $value:expr, $sum:ident, $max:ident) => {{
        let value = $value;
        $self.$sum += value;
        if value > $self.$max {
            $self.$max = value;
        }
    }};
}

/// 合并另一条记录的 sum，并取两侧 max 的较大者。
macro_rules! merge_sum_max {
    ($self:ident, $other:ident, $sum:ident, $max:ident) => {{
        $self.$sum += $other.$sum;
        if $other.$max > $self.$max {
            $self.$max = $other.$max;
        }
    }};
}

impl StmtRecord {
    /// 将一次语句执行累加进本记录（执行次数、错误、延迟、Cop/提交/RU 等）。
    pub fn Add(&mut self, info: &StmtExecInfo) {
        self.IsInternal = self.IsInternal && info.IsInternal;
        if !info.User.is_empty() {
            self.AuthUsers.insert(info.User.clone());
        }
        self.ExecCount += 1;
        if !info.Succeed {
            self.SumErrors += 1;
        }
        self.SumWarnings += i32::from(info.StmtCtx.WarningCount());
        add_sum_max!(self, info.TotalLatency, SumLatency, MaxLatency);
        if info.TotalLatency < self.MinLatency {
            self.MinLatency = info.TotalLatency;
        }
        add_sum_max!(self, info.ParseLatency, SumParseLatency, MaxParseLatency);
        add_sum_max!(
            self,
            info.CompileLatency,
            SumCompileLatency,
            MaxCompileLatency
        );

        // Coprocessor 任务摘要：累计任务数，并记录最慢 process/wait 的地址。
        if let Some(cop) = &info.CopTasks {
            self.SumNumCopTasks += i64::from(cop.NumCopTasks);
            if cop.MaxProcessTime > self.MaxCopProcessTime {
                self.MaxCopProcessTime = cop.MaxProcessTime;
                self.MaxCopProcessAddress = cop.MaxProcessAddress.clone();
            }
            if cop.MaxWaitTime > self.MaxCopWaitTime {
                self.MaxCopWaitTime = cop.MaxWaitTime;
                self.MaxCopWaitAddress = cop.MaxWaitAddress.clone();
            }
        }

        let cop = &info.ExecDetail.CopExecDetails;
        add_sum_max!(
            self,
            cop.TimeDetail.ProcessTime,
            SumProcessTime,
            MaxProcessTime
        );
        add_sum_max!(self, cop.TimeDetail.WaitTime, SumWaitTime, MaxWaitTime);
        add_sum_max!(self, cop.BackoffTime, SumBackoffTime, MaxBackoffTime);
        if let Some(scan) = &cop.ScanDetail {
            add_sum_max!(self, scan.TotalKeys, SumTotalKeys, MaxTotalKeys);
            add_sum_max!(self, scan.ProcessedKeys, SumProcessedKeys, MaxProcessedKeys);
            add_sum_max!(
                self,
                scan.RocksdbDeleteSkippedCount,
                SumRocksdbDeleteSkippedCount,
                MaxRocksdbDeleteSkippedCount
            );
            add_sum_max!(
                self,
                scan.RocksdbKeySkippedCount,
                SumRocksdbKeySkippedCount,
                MaxRocksdbKeySkippedCount
            );
            add_sum_max!(
                self,
                scan.RocksdbBlockCacheHitCount,
                SumRocksdbBlockCacheHitCount,
                MaxRocksdbBlockCacheHitCount
            );
            add_sum_max!(
                self,
                scan.RocksdbBlockReadCount,
                SumRocksdbBlockReadCount,
                MaxRocksdbBlockReadCount
            );
            add_sum_max!(
                self,
                scan.RocksdbBlockReadByte,
                SumRocksdbBlockReadByte,
                MaxRocksdbBlockReadByte
            );
            let ia = task_execdetails::execdetails::GetIARemoteReadSegmentStats(Some(scan));
            if ia.Count > 0 {
                self.IAExecCount += 1;
            }
            add_sum_max!(
                self,
                ia.Count,
                SumIARemoteReadSegmentCount,
                MaxIARemoteReadSegmentCount
            );
            add_sum_max!(
                self,
                ia.Bytes,
                SumIARemoteReadSegmentSize,
                MaxIARemoteReadSegmentSize
            );
            add_sum_max!(
                self,
                ia.WaitTime,
                SumIARemoteReadSegmentWaitTime,
                MaxIARemoteReadSegmentWaitTime
            );
        }

        // 两阶段提交细节：prewrite/commit/resolve-lock 等耗时与写键规模。
        if let Some(commit) = &info.ExecDetail.CommitDetail {
            self.CommitCount += 1;
            add_sum_max!(self, commit.PrewriteTime, SumPrewriteTime, MaxPrewriteTime);
            add_sum_max!(self, commit.CommitTime, SumCommitTime, MaxCommitTime);
            add_sum_max!(
                self,
                commit.GetCommitTsTime,
                SumGetCommitTsTime,
                MaxGetCommitTsTime
            );
            let resolve = commit.ResolveLock.ResolveLockTime.load(Ordering::Relaxed);
            add_sum_max!(self, resolve, SumResolveLockTime, MaxResolveLockTime);
            add_sum_max!(
                self,
                commit.LocalLatchTime,
                SumLocalLatchTime,
                MaxLocalLatchTime
            );
            self.SumWriteKeys += i64::from(commit.WriteKeys);
            self.MaxWriteKeys = self.MaxWriteKeys.max(commit.WriteKeys);
            self.SumWriteSize += i64::from(commit.WriteSize);
            self.MaxWriteSize = self.MaxWriteSize.max(commit.WriteSize);
            let regions = commit.PrewriteRegionNum.load(Ordering::Relaxed);
            self.SumPrewriteRegionNum += i64::from(regions);
            self.MaxPrewriteRegionNum = self.MaxPrewriteRegionNum.max(regions);
            self.SumTxnRetry += i64::from(commit.TxnRetry);
            self.MaxTxnRetry = self.MaxTxnRetry.max(commit.TxnRetry);
            let mu = commit.Mu.Lock();
            self.SumCommitBackoffTime += mu.CommitBackoffTime;
            self.MaxCommitBackoffTime = self.MaxCommitBackoffTime.max(mu.CommitBackoffTime);
            self.SumBackoffTimes +=
                (mu.PrewriteBackoffTypes.len() + mu.CommitBackoffTypes.len()) as i64;
            for name in mu
                .PrewriteBackoffTypes
                .iter()
                .chain(mu.CommitBackoffTypes.iter())
            {
                *self.BackoffTypes.entry(name.clone()).or_default() += 1;
            }
        }

        self.PlanInCache = info.PlanInCache;
        if info.PlanInCache {
            self.PlanCacheHits += 1;
        }
        if !info.PlanCacheUnqualified.is_empty() {
            self.PlanCacheUnqualifiedCount += 1;
            self.PlanCacheUnqualifiedLastReason = info.PlanCacheUnqualified.clone();
        }
        self.PlanInBinding = info.PlanInBinding;
        self.SumAffectedRows += info.StmtCtx.AffectedRows();
        add_sum_max!(self, info.MemMax, SumMem, MaxMem);
        add_sum_max!(
            self,
            info.MemArbitration,
            SumMemArbitration,
            MaxMemArbitration
        );
        add_sum_max!(self, info.DiskMax, SumDisk, MaxDisk);
        if info.StartTime < self.FirstSeen {
            self.FirstSeen = info.StartTime;
        }
        if info.StartTime > self.LastSeen {
            self.LastSeen = info.StartTime;
        }
        if info.ExecRetryCount > 0 {
            self.ExecRetryCount += info.ExecRetryCount;
            self.ExecRetryTime += info.ExecRetryTime;
        }
        if info.ResultRows > 0 {
            self.SumResultRows += info.ResultRows;
            self.MaxResultRows = self.MaxResultRows.max(info.ResultRows);
            self.MinResultRows = self.MinResultRows.min(info.ResultRows);
        } else {
            self.MinResultRows = 0;
        }

        // TiKV/PD 等待、跨区流量与 CPU；RU（资源单位）来自资源组限流。
        let tikv = LoadTiKVExecDetails(Some(&info.TiKVExecDetails));
        self.SumKVTotal += durationFromNanos(tikv.WaitKVRespDuration.load(Ordering::Relaxed));
        self.SumPDTotal += durationFromNanos(tikv.WaitPDRespDuration.load(Ordering::Relaxed));
        self.SumBackoffTotal += durationFromNanos(tikv.BackoffDuration.load(Ordering::Relaxed));
        self.SumWriteSQLRespTotal += info.WriteSQLRespDuration;
        self.SumTidbCPU += info.CPUUsages.TidbCPUTime;
        self.SumTikvCPU += info.CPUUsages.TikvCPUTime;
        let traffic = &tikv.TrafficDetails;
        self.UnpackedBytesSentTiKVTotal += traffic.UnpackedBytesSentKVTotal.load(Ordering::Relaxed);
        self.UnpackedBytesReceivedTiKVTotal +=
            traffic.UnpackedBytesReceivedKVTotal.load(Ordering::Relaxed);
        self.UnpackedBytesSentTiKVCrossZone +=
            traffic.UnpackedBytesSentKVCrossZone.load(Ordering::Relaxed);
        self.UnpackedBytesReceivedTiKVCrossZone += traffic
            .UnpackedBytesReceivedKVCrossZone
            .load(Ordering::Relaxed);
        self.UnpackedBytesSentTiFlashTotal +=
            traffic.UnpackedBytesSentMPPTotal.load(Ordering::Relaxed);
        self.UnpackedBytesReceivedTiFlashTotal += traffic
            .UnpackedBytesReceivedMPPTotal
            .load(Ordering::Relaxed);
        self.UnpackedBytesSentTiFlashCrossZone += traffic
            .UnpackedBytesSentMPPCrossZone
            .load(Ordering::Relaxed);
        self.UnpackedBytesReceivedTiFlashCrossZone += traffic
            .UnpackedBytesReceivedMPPCrossZone
            .load(Ordering::Relaxed);
        if let Some(ru) = info.RUDetail.as_ref() {
            let rru = ru.RRU();
            let wru = ru.WRU();
            let wait = ru.RUWaitDuration();
            self.SumRRU += rru;
            self.MaxRRU = self.MaxRRU.max(rru);
            self.SumWRU += wru;
            self.MaxWRU = self.MaxWRU.max(wru);
            self.SumRUWaitDuration += wait;
            self.MaxRUWaitDuration = self.MaxRUWaitDuration.max(wait);
        }
        self.StorageKV = info.StmtCtx.IsTiKV.load(Ordering::Relaxed);
        self.StorageMPP = info.StmtCtx.IsTiFlash.load(Ordering::Relaxed);
    }

    /// 将另一条同键记录的累计指标合并进来（用于窗口内合并或驱逐聚合）。
    pub fn Merge(&mut self, other: &StmtRecord) {
        self.AuthUsers.extend(other.AuthUsers.iter().cloned());
        self.ExecCount += other.ExecCount;
        self.SumWarnings += other.SumWarnings;
        merge_sum_max!(self, other, SumLatency, MaxLatency);
        self.MinLatency = self.MinLatency.min(other.MinLatency);
        merge_sum_max!(self, other, SumParseLatency, MaxParseLatency);
        merge_sum_max!(self, other, SumCompileLatency, MaxCompileLatency);
        self.SumNumCopTasks += other.SumNumCopTasks;
        if other.MaxCopProcessTime > self.MaxCopProcessTime {
            self.MaxCopProcessTime = other.MaxCopProcessTime;
            self.MaxCopProcessAddress
                .clone_from(&other.MaxCopProcessAddress);
        }
        if other.MaxCopWaitTime > self.MaxCopWaitTime {
            self.MaxCopWaitTime = other.MaxCopWaitTime;
            self.MaxCopWaitAddress.clone_from(&other.MaxCopWaitAddress);
        }
        merge_sum_max!(self, other, SumProcessTime, MaxProcessTime);
        merge_sum_max!(self, other, SumWaitTime, MaxWaitTime);
        merge_sum_max!(self, other, SumBackoffTime, MaxBackoffTime);
        merge_sum_max!(self, other, SumTotalKeys, MaxTotalKeys);
        merge_sum_max!(self, other, SumProcessedKeys, MaxProcessedKeys);
        merge_sum_max!(
            self,
            other,
            SumRocksdbDeleteSkippedCount,
            MaxRocksdbDeleteSkippedCount
        );
        merge_sum_max!(
            self,
            other,
            SumRocksdbKeySkippedCount,
            MaxRocksdbKeySkippedCount
        );
        merge_sum_max!(
            self,
            other,
            SumRocksdbBlockCacheHitCount,
            MaxRocksdbBlockCacheHitCount
        );
        merge_sum_max!(
            self,
            other,
            SumRocksdbBlockReadCount,
            MaxRocksdbBlockReadCount
        );
        merge_sum_max!(
            self,
            other,
            SumRocksdbBlockReadByte,
            MaxRocksdbBlockReadByte
        );
        self.IAExecCount += other.IAExecCount;
        merge_sum_max!(
            self,
            other,
            SumIARemoteReadSegmentCount,
            MaxIARemoteReadSegmentCount
        );
        merge_sum_max!(
            self,
            other,
            SumIARemoteReadSegmentSize,
            MaxIARemoteReadSegmentSize
        );
        merge_sum_max!(
            self,
            other,
            SumIARemoteReadSegmentWaitTime,
            MaxIARemoteReadSegmentWaitTime
        );
        self.CommitCount += other.CommitCount;
        merge_sum_max!(self, other, SumPrewriteTime, MaxPrewriteTime);
        merge_sum_max!(self, other, SumCommitTime, MaxCommitTime);
        merge_sum_max!(self, other, SumGetCommitTsTime, MaxGetCommitTsTime);
        merge_sum_max!(self, other, SumCommitBackoffTime, MaxCommitBackoffTime);
        merge_sum_max!(self, other, SumResolveLockTime, MaxResolveLockTime);
        merge_sum_max!(self, other, SumLocalLatchTime, MaxLocalLatchTime);
        merge_sum_max!(self, other, SumWriteKeys, MaxWriteKeys);
        merge_sum_max!(self, other, SumWriteSize, MaxWriteSize);
        merge_sum_max!(self, other, SumPrewriteRegionNum, MaxPrewriteRegionNum);
        merge_sum_max!(self, other, SumTxnRetry, MaxTxnRetry);
        self.SumBackoffTimes += other.SumBackoffTimes;
        for (name, count) in &other.BackoffTypes {
            *self.BackoffTypes.entry(name.clone()).or_default() += count;
        }
        self.PlanCacheHits += other.PlanCacheHits;
        self.PlanCacheUnqualifiedCount += other.PlanCacheUnqualifiedCount;
        if !other.PlanCacheUnqualifiedLastReason.is_empty() {
            self.PlanCacheUnqualifiedLastReason
                .clone_from(&other.PlanCacheUnqualifiedLastReason);
        }
        self.SumAffectedRows += other.SumAffectedRows;
        merge_sum_max!(self, other, SumMem, MaxMem);
        merge_sum_max!(self, other, SumDisk, MaxDisk);
        self.FirstSeen = self.FirstSeen.min(other.FirstSeen);
        self.LastSeen = self.LastSeen.max(other.LastSeen);
        self.ExecRetryCount += other.ExecRetryCount;
        self.ExecRetryTime += other.ExecRetryTime;
        self.SumKVTotal += other.SumKVTotal;
        self.SumPDTotal += other.SumPDTotal;
        self.SumBackoffTotal += other.SumBackoffTotal;
        self.SumWriteSQLRespTotal += other.SumWriteSQLRespTotal;
        self.SumTidbCPU += other.SumTidbCPU;
        self.SumTikvCPU += other.SumTikvCPU;
        self.SumErrors += other.SumErrors;
        self.SumRRU += other.SumRRU;
        self.SumWRU += other.SumWRU;
        self.SumRUWaitDuration += other.SumRUWaitDuration;
        self.MaxRRU = self.MaxRRU.max(other.MaxRRU);
        self.MaxWRU = self.MaxWRU.max(other.MaxWRU);
        self.MaxRUWaitDuration = self.MaxRUWaitDuration.max(other.MaxRUWaitDuration);
    }
}

/// 将纳秒计数转为 `Duration`；非正值视为零。
fn durationFromNanos(value: i64) -> Duration {
    if value <= 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos(value as u64)
    }
}

/// 按全局最大长度截断 SQL；超长时在末尾附加 `(len:N)`。
pub fn formatSQL(sql: String) -> String {
    let max = GLOBAL_MAX_SQL_LENGTH.load(Ordering::Relaxed) as usize;
    let length = sql.len();
    if length <= max {
        return sql;
    }
    // 回退到合法 UTF-8 边界，避免截断半个字符。
    let mut boundary = max;
    while boundary > 0 && !sql.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}(len:{length})", &sql[..boundary])
}

/// 测试用空懒加载信息（无 SQL/计划）。
#[derive(Default)]
struct mockLazyInfo;
impl StmtExecLazyInfo for mockLazyInfo {
    fn GetOriginalSQL(&self) -> String {
        String::new()
    }
    fn GetEncodedPlan(&self) -> (String, String, Option<String>) {
        (String::new(), String::new(), None)
    }
    fn GetBinaryPlan(&self) -> String {
        String::new()
    }
    fn GetPlanDigest(&self) -> String {
        String::new()
    }
    fn GetBindingSQLAndDigest(&self) -> (String, String) {
        (String::new(), String::new())
    }
}

/// 生成固定字段的测试用 `StmtExecInfo`（可指定 digest）。
pub fn GenerateStmtExecInfo4Test(digest: impl Into<String>) -> Box<StmtExecInfo> {
    let mut ctx = *NewStmtCtx();
    ctx.StmtType = "Select".to_owned();
    ctx.SetLogicalPlanTables(vec![
        TableEntry {
            DB: "db1".into(),
            Table: "tb1".into(),
        },
        TableEntry {
            DB: "db2".into(),
            Table: "tb2".into(),
        },
    ]);
    *ctx.IndexNames
        .lock()
        .expect("statement index names lock poisoned") = vec!["a".into()];
    ctx.AddAffectedRows(10_000);
    let commit = CommitDetails {
        GetCommitTsTime: Duration::from_nanos(100),
        PrewriteTime: Duration::from_nanos(10_000),
        CommitTime: Duration::from_nanos(1_000),
        LocalLatchTime: Duration::from_nanos(10),
        WriteKeys: 20_000,
        WriteSize: 200_000,
        TxnRetry: 2,
        ..Default::default()
    };
    commit
        .ResolveLock
        .ResolveLockTime
        .store(2_000, Ordering::Relaxed);
    commit.PrewriteRegionNum.store(20, Ordering::Relaxed);
    {
        let mut mu = commit.Mu.Lock();
        mu.CommitBackoffTime = 200;
        mu.PrewriteBackoffTypes = vec!["txnlock".into()];
    }

    let mut info = Box::new(StmtExecInfo {
        SchemaName: "schema_name".into(),
        NormalizedSQL: "normalized_sql".into(),
        Digest: digest.into(),
        PlanDigest: "plan_digest".into(),
        User: "user".into(),
        TotalLatency: Duration::from_nanos(10_000),
        ParseLatency: Duration::from_nanos(100),
        CompileLatency: Duration::from_nanos(1_000),
        CopTasks: Some(CopTasksSummary {
            NumCopTasks: 10,
            MaxProcessAddress: "127".into(),
            MaxProcessTime: Duration::from_nanos(15_000),
            MaxWaitAddress: "128".into(),
            MaxWaitTime: Duration::from_nanos(1_500),
            ..Default::default()
        }),
        ExecDetail: ExecDetails {
            CommitDetail: Some(commit),
            CopExecDetails: CopExecDetails {
                BackoffTime: Duration::from_nanos(80),
                ScanDetail: Some(ScanDetail {
                    TotalKeys: 1_000,
                    ProcessedKeys: 500,
                    RocksdbDeleteSkippedCount: 100,
                    RocksdbKeySkippedCount: 10,
                    RocksdbBlockCacheHitCount: 10,
                    RocksdbBlockReadCount: 10,
                    RocksdbBlockReadByte: 1_000,
                    ..Default::default()
                }),
                TimeDetail: TimeDetail {
                    ProcessTime: Duration::from_nanos(500),
                    WaitTime: Duration::from_nanos(50),
                },
                CalleeAddress: "129".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        StmtCtx: ctx,
        MemMax: 10_000,
        DiskMax: 10_000,
        StartTime: UNIX_EPOCH + Duration::from_secs(1_546_335_010),
        Succeed: true,
        KeyspaceName: "keyspace_a".into(),
        KeyspaceID: 1,
        ResourceGroupName: "rg1".into(),
        RUDetail: Some(RUDetails {
            read_ru: 1.2,
            write_ru: 3.4,
            ru_wait_duration: Duration::from_millis(2),
            ..Default::default()
        }),
        LazyInfo: Box::new(mockLazyInfo),
        MemArbitration: 22_222.0,
        ..Default::default()
    });
    info.CPUUsages.TidbCPUTime = Duration::from_nanos(20);
    info.CPUUsages.TikvCPUTime = Duration::from_nanos(10_000);
    info
}
