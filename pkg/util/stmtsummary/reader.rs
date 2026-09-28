// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 语句摘要 reader：从内存 map 读出累计 / 当前 / 历史行，转为 Datum。
//
// 对照 Go `reader.go`：保留权限过滤、digest checker、完整列工厂、
// 时区转换及计划解码错误处理。

// 对照 pkg/util/stmtsummary/reader.go：从内存摘要读取累计、当前和历史行，
// 保留权限过滤、digest checker、完整列工厂、时区转换及计划解码错误处理。

use crate::{
    StmtSummaryByDigestMap, auth, avgFloat, avgInt, avgSumFloat, convertEmptyToNil,
    formatBackoffTypes, model, mysql, plancodec, set, stmtSummaryByDigest,
    stmtSummaryByDigestElement, stmtSummaryByDigestEvicted, stmtSummaryByDigestMap,
    stmtSummaryStats, types,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// stmtSummaryReader uses to read the statement summaries data and convert to []datum row.
// stmtSummaryReader 对应 Go 的 reader 结构体，保存调用者身份、PROCESS 权限、列定义、
// 实例地址、全局 summary map、按列预解析好的取值工厂、digest 过滤器以及时区。
/// 语句摘要读取器：持有权限上下文、列工厂与全局 map 引用。
pub struct stmtSummaryReader {
    pub user: Option<auth::UserIdentity>,
    // If the user has the 'PROCESS' privilege, he can read all the statements.
    // PROCESS 权限会跳过普通用户可见性过滤；没有该权限时按 authUsers 判断是否可读。
    pub hasProcessPriv: bool,
    pub columns: Vec<model::ColumnInfo>,
    pub instanceAddr: String,
    pub ssMap: &'static Mutex<stmtSummaryByDigestMap>,
    pub columnValueFactories: Vec<columnValueFactory>,
    pub checker: Option<stmtSummaryChecker>,
    pub tz: chrono_tz::Tz,
}

// NewStmtSummaryReader return a new statement summaries reader.
// NewStmtSummaryReader 对应 Go 构造函数：绑定全局 StmtSummaryByDigestMap，并把列名提前解析为取值函数。
/// 构造 reader：绑定全局 map，并将列名解析为取值工厂。
pub fn NewStmtSummaryReader(
    user: Option<auth::UserIdentity>,
    hasProcessPriv: bool,
    cols: Vec<model::ColumnInfo>,
    instanceAddr: String,
    tz: chrono_tz::Tz,
) -> stmtSummaryReader {
    let mut reader = stmtSummaryReader {
        user,
        hasProcessPriv,
        columns: cols,
        instanceAddr,
        ssMap: &StmtSummaryByDigestMap,
        columnValueFactories: Vec::new(),
        checker: None,
        tz,
    };

    // initialize column value factories.
    // Go 代码在构造阶段把每个 ColumnInfo.Name.O 查到闭包；这里保持同样的提前失败语义。
    let factories = columnValueFactoryMap();
    reader.columnValueFactories = Vec::with_capacity(reader.columns.len());
    for col in &reader.columns {
        let name = col.Name.O.as_str();
        let Some(factory) = factories.get(name) else {
            // Go 中这里 panic(fmt.Sprintf(...))，表示表定义新增列但没有注册取值工厂。
            panic!(
                "should never happen, should register new column {} into columnValueFactoryMap",
                col.Name.O
            );
        };
        reader.columnValueFactories.push(*factory);
    }
    reader
}

impl stmtSummaryReader {
    /// 读取各 digest 的累计（跨窗口）统计行。
    pub fn GetStmtSummaryCumulativeRows(&self) -> Vec<Vec<types::Datum>> {
        let ssMap = self
            .ssMap
            .lock()
            .expect("statement summary map mutex poisoned");
        let mut rows = Vec::with_capacity(ssMap.summaryMap.len());
        for (_, ssbd) in ssMap.summaryMap.iter() {
            if self
                .checker
                .as_ref()
                .is_some_and(|checker| !checker.isDigestValid(&ssbd.digest))
            {
                continue;
            }
            if let Some(record) = self.getStmtByDigestCumulativeRow(ssbd) {
                rows.push(record);
            }
        }
        rows
    }

    /// 读取当前刷新区间行；无 checker 时附带淘汰 other 行。
    pub fn GetStmtSummaryCurrentRows(&self) -> Vec<Vec<types::Datum>> {
        let ssMap = self
            .ssMap
            .lock()
            .expect("statement summary map mutex poisoned");
        let mut rows = Vec::with_capacity(ssMap.summaryMap.len());
        for (_, ssbd) in ssMap.summaryMap.iter() {
            if self
                .checker
                .as_ref()
                .is_some_and(|checker| !checker.isDigestValid(&ssbd.digest))
            {
                continue;
            }
            if let Some(record) = self.getStmtByDigestRow(ssbd, ssMap.beginTimeForCurInterval) {
                rows.push(record);
            }
        }
        if self.checker.is_none() {
            if let Some(other_datum) = self.getStmtEvictedOtherRow(&ssMap.other) {
                rows.push(other_datum);
            }
        }
        rows
    }

    /// 读取历史窗口行；无 checker 时附带淘汰 other 历史。
    pub fn GetStmtSummaryHistoryRows(&self) -> Vec<Vec<types::Datum>> {
        let ssMap = self
            .ssMap
            .lock()
            .expect("statement summary map mutex poisoned");
        let historySize = ssMap.historySize().max(0) as usize;
        let mut rows = Vec::with_capacity(ssMap.summaryMap.len() * historySize);
        for (_, value) in ssMap.summaryMap.iter() {
            let records = self.getStmtByDigestHistoryRow(value, historySize);
            rows.extend(records);
        }

        if self.checker.is_none() {
            let other_datum = self.getStmtEvictedOtherHistoryRow(&ssMap.other, historySize);
            rows.extend(other_datum);
        }
        rows
    }

    /// 设置 digest 白名单过滤器。
    pub fn SetChecker(&mut self, checker: Option<stmtSummaryChecker>) {
        self.checker = checker;
    }

    /// 判断当前用户是否有权读取该统计（PROCESS 或出现在 authUsers）。
    fn isAuthed(&self, ssStats: &stmtSummaryStats) -> bool {
        self.user
            .as_ref()
            .is_none_or(|user| self.hasProcessPriv || ssStats.authUsers.contains(&user.username))
    }

    /// 将累计统计转为单行 Datum。
    fn getStmtByDigestCumulativeRow(
        &self,
        ssbd: &stmtSummaryByDigest,
    ) -> Option<Vec<types::Datum>> {
        if !self.isAuthed(&ssbd.cumulative) {
            return None;
        }
        Some(
            self.columnValueFactories
                .iter()
                .map(|factory| factory(self, None, Some(ssbd), &ssbd.cumulative).into_datum())
                .collect(),
        )
    }

    /// 取当前区间最新元素行；未初始化或窗口已过期则跳过。
    fn getStmtByDigestRow(
        &self,
        ssbd: &stmtSummaryByDigest,
        beginTimeForCurInterval: i64,
    ) -> Option<Vec<types::Datum>> {
        if !ssbd.initialized {
            return None;
        }
        let ssElement = ssbd.history.back()?;
        if ssElement.beginTime < beginTimeForCurInterval {
            return None;
        }
        self.getStmtByDigestElementRow(ssElement, ssbd)
    }

    /// 按列工厂把单个窗口元素转为 Datum 行。
    fn getStmtByDigestElementRow(
        &self,
        ssElement: &stmtSummaryByDigestElement,
        ssbd: &stmtSummaryByDigest,
    ) -> Option<Vec<types::Datum>> {
        if !self.isAuthed(&ssElement.stmtSummaryStats) {
            return None;
        }
        Some(
            self.columnValueFactories
                .iter()
                .map(|factory| {
                    factory(
                        self,
                        Some(ssElement),
                        Some(ssbd),
                        &ssElement.stmtSummaryStats,
                    )
                    .into_datum()
                })
                .collect(),
        )
    }

    /// 收集 digest 历史窗口行（受 historySize 与 checker 限制）。
    fn getStmtByDigestHistoryRow(
        &self,
        ssbd: &stmtSummaryByDigest,
        historySize: usize,
    ) -> Vec<Vec<types::Datum>> {
        if !ssbd.initialized
            || self
                .checker
                .as_ref()
                .is_some_and(|checker| !checker.isDigestValid(&ssbd.digest))
        {
            return Vec::new();
        }
        ssbd.history
            .iter()
            .take(historySize)
            .filter_map(|element| self.getStmtByDigestElementRow(element, ssbd))
            .collect()
    }

    /// 当前区间淘汰 other 桶的汇总行。
    fn getStmtEvictedOtherRow(
        &self,
        ssbde: &stmtSummaryByDigestEvicted,
    ) -> Option<Vec<types::Datum>> {
        let seElement = ssbde.history.back()?;
        let empty_ssbd = stmtSummaryByDigest::default();
        self.getStmtByDigestElementRow(&seElement.otherSummary, &empty_ssbd)
    }

    /// 淘汰 other 桶的历史窗口行。
    fn getStmtEvictedOtherHistoryRow(
        &self,
        ssbde: &stmtSummaryByDigestEvicted,
        historySize: usize,
    ) -> Vec<Vec<types::Datum>> {
        let ssbd = stmtSummaryByDigest::default();
        ssbde
            .history
            .iter()
            .take(historySize)
            .filter_map(|element| self.getStmtByDigestElementRow(&element.otherSummary, &ssbd))
            .collect()
    }
}

// stmtSummaryChecker 保存允许读取的 digest 集合。
/// digest 白名单检查器。
pub struct stmtSummaryChecker {
    pub digests: set::StringSet,
}

// NewStmtSummaryChecker return a new statement summaries checker.
// NewStmtSummaryChecker 与 Go 构造函数一致，只包装传入的 StringSet。
/// 用给定 StringSet 构造 checker。
pub fn NewStmtSummaryChecker(digests: set::StringSet) -> stmtSummaryChecker {
    stmtSummaryChecker { digests }
}

impl stmtSummaryChecker {
    // isDigestValid 对应 Go 的 set.Exist 调用，用于 reader 侧过滤 digest。
    /// 判断 digest 是否在白名单中。
    pub(crate) fn isDigestValid(&self, digest: &str) -> bool {
        self.digests.Exist(digest)
    }
}

// Statements summary table column name.
// 以下常量保持 Go 文件中的列名声明顺序，供 columnValueFactoryMap 逐项注册使用。
/// 集群表 INSTANCE 列名。
pub const ClusterTableInstanceColumnNameStr: &str = "INSTANCE";
pub const SummaryBeginTimeStr: &str = "SUMMARY_BEGIN_TIME";
pub const SummaryEndTimeStr: &str = "SUMMARY_END_TIME";
pub const StmtTypeStr: &str = "STMT_TYPE";
pub const SchemaNameStr: &str = "SCHEMA_NAME";
pub const DigestStr: &str = "DIGEST";
pub const DigestTextStr: &str = "DIGEST_TEXT";
pub const TableNamesStr: &str = "TABLE_NAMES";
pub const IndexNamesStr: &str = "INDEX_NAMES";
pub const SampleUserStr: &str = "SAMPLE_USER";
pub const ExecCountStr: &str = "EXEC_COUNT";
pub const SumErrorsStr: &str = "SUM_ERRORS";
pub const SumWarningsStr: &str = "SUM_WARNINGS";
pub const SumLatencyStr: &str = "SUM_LATENCY";
pub const MaxLatencyStr: &str = "MAX_LATENCY";
pub const MinLatencyStr: &str = "MIN_LATENCY";
pub const AvgLatencyStr: &str = "AVG_LATENCY";
pub const AvgParseLatencyStr: &str = "AVG_PARSE_LATENCY";
pub const MaxParseLatencyStr: &str = "MAX_PARSE_LATENCY";
pub const AvgCompileLatencyStr: &str = "AVG_COMPILE_LATENCY";
pub const MaxCompileLatencyStr: &str = "MAX_COMPILE_LATENCY";
pub const SumCopTaskNumStr: &str = "SUM_COP_TASK_NUM";
pub const MaxCopProcessTimeStr: &str = "MAX_COP_PROCESS_TIME";
pub const MaxCopProcessAddressStr: &str = "MAX_COP_PROCESS_ADDRESS";
pub const MaxCopWaitTimeStr: &str = "MAX_COP_WAIT_TIME";
pub const MaxCopWaitAddressStr: &str = "MAX_COP_WAIT_ADDRESS";
pub const AvgProcessTimeStr: &str = "AVG_PROCESS_TIME";
pub const MaxProcessTimeStr: &str = "MAX_PROCESS_TIME";
pub const AvgWaitTimeStr: &str = "AVG_WAIT_TIME";
pub const MaxWaitTimeStr: &str = "MAX_WAIT_TIME";
pub const AvgBackoffTimeStr: &str = "AVG_BACKOFF_TIME";
pub const MaxBackoffTimeStr: &str = "MAX_BACKOFF_TIME";
pub const AvgTotalKeysStr: &str = "AVG_TOTAL_KEYS";
pub const MaxTotalKeysStr: &str = "MAX_TOTAL_KEYS";
pub const AvgProcessedKeysStr: &str = "AVG_PROCESSED_KEYS";
pub const MaxProcessedKeysStr: &str = "MAX_PROCESSED_KEYS";
pub const AvgRocksdbDeleteSkippedCountStr: &str = "AVG_ROCKSDB_DELETE_SKIPPED_COUNT";
pub const MaxRocksdbDeleteSkippedCountStr: &str = "MAX_ROCKSDB_DELETE_SKIPPED_COUNT";
pub const AvgRocksdbKeySkippedCountStr: &str = "AVG_ROCKSDB_KEY_SKIPPED_COUNT";
pub const MaxRocksdbKeySkippedCountStr: &str = "MAX_ROCKSDB_KEY_SKIPPED_COUNT";
pub const AvgRocksdbBlockCacheHitCountStr: &str = "AVG_ROCKSDB_BLOCK_CACHE_HIT_COUNT";
pub const MaxRocksdbBlockCacheHitCountStr: &str = "MAX_ROCKSDB_BLOCK_CACHE_HIT_COUNT";
pub const AvgRocksdbBlockReadCountStr: &str = "AVG_ROCKSDB_BLOCK_READ_COUNT";
pub const MaxRocksdbBlockReadCountStr: &str = "MAX_ROCKSDB_BLOCK_READ_COUNT";
pub const AvgRocksdbBlockReadByteStr: &str = "AVG_ROCKSDB_BLOCK_READ_BYTE";
pub const MaxRocksdbBlockReadByteStr: &str = "MAX_ROCKSDB_BLOCK_READ_BYTE";
pub const AvgPrewriteTimeStr: &str = "AVG_PREWRITE_TIME";
pub const MaxPrewriteTimeStr: &str = "MAX_PREWRITE_TIME";
pub const AvgCommitTimeStr: &str = "AVG_COMMIT_TIME";
pub const MaxCommitTimeStr: &str = "MAX_COMMIT_TIME";
pub const AvgGetCommitTsTimeStr: &str = "AVG_GET_COMMIT_TS_TIME";
pub const MaxGetCommitTsTimeStr: &str = "MAX_GET_COMMIT_TS_TIME";
pub const AvgCommitBackoffTimeStr: &str = "AVG_COMMIT_BACKOFF_TIME";
pub const MaxCommitBackoffTimeStr: &str = "MAX_COMMIT_BACKOFF_TIME";
pub const AvgResolveLockTimeStr: &str = "AVG_RESOLVE_LOCK_TIME";
pub const MaxResolveLockTimeStr: &str = "MAX_RESOLVE_LOCK_TIME";
pub const AvgLocalLatchWaitTimeStr: &str = "AVG_LOCAL_LATCH_WAIT_TIME";
pub const MaxLocalLatchWaitTimeStr: &str = "MAX_LOCAL_LATCH_WAIT_TIME";
pub const AvgWriteKeysStr: &str = "AVG_WRITE_KEYS";
pub const MaxWriteKeysStr: &str = "MAX_WRITE_KEYS";
pub const AvgWriteSizeStr: &str = "AVG_WRITE_SIZE";
pub const MaxWriteSizeStr: &str = "MAX_WRITE_SIZE";
pub const AvgPrewriteRegionsStr: &str = "AVG_PREWRITE_REGIONS";
pub const MaxPrewriteRegionsStr: &str = "MAX_PREWRITE_REGIONS";
pub const AvgTxnRetryStr: &str = "AVG_TXN_RETRY";
pub const MaxTxnRetryStr: &str = "MAX_TXN_RETRY";
pub const SumExecRetryStr: &str = "SUM_EXEC_RETRY";
pub const SumExecRetryTimeStr: &str = "SUM_EXEC_RETRY_TIME";
pub const SumBackoffTimesStr: &str = "SUM_BACKOFF_TIMES";
pub const BackoffTypesStr: &str = "BACKOFF_TYPES";
pub const AvgMemStr: &str = "AVG_MEM";
pub const MaxMemStr: &str = "MAX_MEM";
pub const AvgMemArbitrationStr: &str = "AVG_MEM_ARBITRATION";
pub const MaxMemArbitrationStr: &str = "MAX_MEM_ARBITRATION";
pub const AvgDiskStr: &str = "AVG_DISK";
pub const MaxDiskStr: &str = "MAX_DISK";
pub const AvgKvTimeStr: &str = "AVG_KV_TIME";
pub const AvgPdTimeStr: &str = "AVG_PD_TIME";
pub const AvgBackoffTotalTimeStr: &str = "AVG_BACKOFF_TOTAL_TIME";
pub const AvgWriteSQLRespTimeStr: &str = "AVG_WRITE_SQL_RESP_TIME";
pub const AvgTidbCPUTimeStr: &str = "AVG_TIDB_CPU_TIME";
pub const AvgTikvCPUTimeStr: &str = "AVG_TIKV_CPU_TIME";
pub const MaxResultRowsStr: &str = "MAX_RESULT_ROWS";
pub const MinResultRowsStr: &str = "MIN_RESULT_ROWS";
pub const AvgResultRowsStr: &str = "AVG_RESULT_ROWS";
pub const PreparedStr: &str = "PREPARED";
pub const AvgAffectedRowsStr: &str = "AVG_AFFECTED_ROWS";
pub const FirstSeenStr: &str = "FIRST_SEEN";
pub const LastSeenStr: &str = "LAST_SEEN";
pub const PlanInCacheStr: &str = "PLAN_IN_CACHE";
pub const PlanCacheHitsStr: &str = "PLAN_CACHE_HITS";
pub const PlanCacheUnqualifiedStr: &str = "PLAN_CACHE_UNQUALIFIED";
pub const PlanCacheUnqualifiedLastReasonStr: &str = "PLAN_CACHE_UNQUALIFIED_LAST_REASON";
pub const PlanInBindingStr: &str = "PLAN_IN_BINDING";
pub const QuerySampleTextStr: &str = "QUERY_SAMPLE_TEXT";
pub const PrevSampleTextStr: &str = "PREV_SAMPLE_TEXT";
pub const PlanDigestStr: &str = "PLAN_DIGEST";
pub const PlanStr: &str = "PLAN";
pub const BinaryPlan: &str = "BINARY_PLAN";
pub const BindingDigestStr: &str = "BINDING_DIGEST";
pub const BindingDigestTextStr: &str = "BINDING_DIGEST_TEXT";
pub const Charset: &str = "CHARSET";
pub const Collation: &str = "COLLATION";
pub const PlanHint: &str = "PLAN_HINT";
pub const AvgRequestUnitReadStr: &str = "AVG_REQUEST_UNIT_READ";
pub const MaxRequestUnitReadStr: &str = "MAX_REQUEST_UNIT_READ";
pub const AvgRequestUnitWriteStr: &str = "AVG_REQUEST_UNIT_WRITE";
pub const MaxRequestUnitWriteStr: &str = "MAX_REQUEST_UNIT_WRITE";
pub const AvgQueuedRcTimeStr: &str = "AVG_QUEUED_RC_TIME";
pub const MaxQueuedRcTimeStr: &str = "MAX_QUEUED_RC_TIME";
pub const AvgRequestUnitV2Str: &str = "AVG_REQUEST_UNIT_V2";
pub const MaxRequestUnitV2Str: &str = "MAX_REQUEST_UNIT_V2";
pub const ResourceGroupName: &str = "RESOURCE_GROUP";
pub const SumUnpackedBytesSentTiKVTotalStr: &str = "SUM_UNPACKED_BYTES_SENT_TIKV_TOTAL";
pub const SumUnpackedBytesReceivedTiKVTotalStr: &str = "SUM_UNPACKED_BYTES_RECEIVED_TIKV_TOTAL";
pub const SumUnpackedBytesSentTiKVCrossZoneStr: &str = "SUM_UNPACKED_BYTES_SENT_TIKV_CROSS_ZONE";
pub const SumUnpackedBytesReceivedTiKVCrossZoneStr: &str =
    "SUM_UNPACKED_BYTES_RECEIVED_TIKV_CROSS_ZONE";
pub const SumUnpackedBytesSentTiFlashTotalStr: &str = "SUM_UNPACKED_BYTES_SENT_TIFLASH_TOTAL";
pub const SumUnpackedBytesReceivedTiFlashTotalStr: &str =
    "SUM_UNPACKED_BYTES_RECEIVED_TIFLASH_TOTAL";
pub const SumUnpackedBytesSentTiFlashCrossZoneStr: &str =
    "SUM_UNPACKED_BYTES_SENT_TIFLASH_CROSS_ZONE";
pub const SumUnpackedBytesReceiveTiFlashCrossZoneStr: &str =
    "SUM_UNPACKED_BYTES_RECEIVED_TIFLASH_CROSS_ZONE";
pub const StorageKVStr: &str = "STORAGE_KV";
pub const StorageMPPStr: &str = "STORAGE_MPP";

// Column names for the statement stats table, including columns that have been
// renamed from their equivalent columns in the statement summary table.
// statement stats 表的列名与 summary 表部分不同；这里保留 Go 注释和同名常量。
pub const ErrorsStr: &str = "ERRORS";
pub const WarningsStr: &str = "WARNINGS";
pub const MemStr: &str = "MEM";
pub const MemArbitrationStr: &str = "MEM_ARBITRATION";
pub const DiskStr: &str = "DISK";
pub const TotalTimeStr: &str = "TOTAL_TIME";
pub const ParseTimeStr: &str = "PARSE_TIME";
pub const CompileTimeStr: &str = "COMPILE_TIME";
pub const CopTaskNumStr: &str = "COP_TASK_NUM";
pub const CopProcessTimeStr: &str = "COP_PROCESS_TIME";
pub const CopWaitTimeStr: &str = "COP_WAIT_TIME";
pub const PdTimeStr: &str = "PD_TIME";
pub const KvTimeStr: &str = "KV_TIME";
pub const ProcessTimeStr: &str = "PROCESS_TIME";
pub const WaitTimeStr: &str = "WAIT_TIME";
pub const BackoffTimeStr: &str = "BACKOFF_TIME";
pub const TotalKeysStr: &str = "TOTAL_KEYS";
pub const ProcessedKeysStr: &str = "PROCESSED_KEYS";
pub const RocksdbDeleteSkippedCountStr: &str = "ROCKSDB_DELETE_SKIPPED_COUNT";
pub const RocksdbKeySkippedCountStr: &str = "ROCKSDB_KEY_SKIPPED_COUNT";
pub const RocksdbBlockCacheHitCountStr: &str = "ROCKSDB_BLOCK_CACHE_HIT_COUNT";
pub const RocksdbBlockReadCountStr: &str = "ROCKSDB_BLOCK_READ_COUNT";
pub const RocksdbBlockReadByteStr: &str = "ROCKSDB_BLOCK_READ_BYTE";
pub const PrewriteTimeStr: &str = "PREWRITE_TIME";
pub const CommitTimeStr: &str = "COMMIT_TIME";
pub const CommitTsTimeStr: &str = "COMMIT_TS_TIME";
pub const CommitBackoffTimeStr: &str = "COMMIT_BACKOFF_TIME";
pub const ResolveLockTimeStr: &str = "RESOLVE_LOCK_TIME";
pub const LocalLatchWaitTimeStr: &str = "LOCAL_LATCH_WAIT_TIME";
pub const WriteKeysStr: &str = "WRITE_KEYS";
pub const WriteSizeStr: &str = "WRITE_SIZE";
pub const PrewriteRegionsStr: &str = "PREWRITE_REGIONS";
pub const TxnRetryStr: &str = "TXN_RETRY";
pub const ExecRetryStr: &str = "EXEC_RETRY";
pub const ExecRetryTimeStr: &str = "EXEC_RETRY_TIME";
pub const BackoffTimesStr: &str = "BACKOFF_TIMES";
pub const BackoffTotalTimeStr: &str = "BACKOFF_TOTAL_TIME";
pub const WriteSQLRespTimeStr: &str = "WRITE_SQL_RESP_TIME";
pub const ResultRowsStr: &str = "RESULT_ROWS";
pub const AffectedRowsStr: &str = "AFFECTED_ROWS";
pub const RequestUnitReadStr: &str = "REQUEST_UNIT_READ";
pub const RequestUnitWriteStr: &str = "REQUEST_UNIT_WRITE";
pub const QueuedRcTimeStr: &str = "QUEUED_RC_TIME";
pub const UnpackedBytesSentTiKVTotalStr: &str = "UNPACKED_BYTES_SENT_TIKV_TOTAL";
pub const UnpackedBytesReceivedTiKVTotalStr: &str = "UNPACKED_BYTES_RECEIVED_TIKV_TOTAL";
pub const UnpackedBytesSentTiKVCrossZoneStr: &str = "UNPACKED_BYTES_SENT_TIKV_CROSS_ZONE";
pub const UnpackedBytesReceivedTiKVCrossZoneStr: &str = "UNPACKED_BYTES_RECEIVED_TIKV_CROSS_ZONE";
pub const UnpackedBytesSentTiFlashTotalStr: &str = "UNPACKED_BYTES_SENT_TIFLASH_TOTAL";
pub const UnpackedBytesReceivedTiFlashTotalStr: &str = "UNPACKED_BYTES_RECEIVED_TIFLASH_TOTAL";
pub const UnpackedBytesSentTiFlashCrossZoneStr: &str = "UNPACKED_BYTES_SENT_TIFLASH_CROSS_ZONE";
pub const UnpackedBytesReceiveTiFlashCrossZoneStr: &str =
    "UNPACKED_BYTES_RECEIVED_TIFLASH_CROSS_ZONE";

/// 列工厂中间值：可转为 Datum 的类型联合。
#[derive(Clone)]
pub enum StmtSummaryValue {
    Null,
    Int(i64),
    Uint(u64),
    Float(f64),
    String(String),
    Time(types::Time),
    Datum(types::Datum),
}

impl StmtSummaryValue {
    /// 转为信息模式表用的 Datum。
    pub fn into_datum(self) -> types::Datum {
        match self {
            Self::Null => types::Datum::default(),
            Self::Int(value) => types::NewIntDatum(value),
            Self::Uint(value) => types::NewUintDatum(value),
            Self::Float(value) => types::NewFloat64Datum(value),
            Self::String(value) => types::NewStringDatum(value),
            Self::Time(value) => types::NewTimeDatum(value),
            Self::Datum(value) => value,
        }
    }
}

// Go strings can hold non-UTF-8 plan bytes; preserve their string Datum kind.
impl From<Vec<u8>> for StmtSummaryValue {
    fn from(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(text) => Self::String(text),
            Err(error) => {
                let mut datum = types::Datum::default();
                datum.SetBytesAsString(
                    error.into_bytes(),
                    mysql::DefaultCollationName.to_owned(),
                    0,
                );
                Self::Datum(datum)
            }
        }
    }
}

impl From<String> for StmtSummaryValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<&str> for StmtSummaryValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}
impl From<Option<String>> for StmtSummaryValue {
    fn from(value: Option<String>) -> Self {
        value.map_or(Self::Null, Self::String)
    }
}
impl From<i64> for StmtSummaryValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}
impl From<i32> for StmtSummaryValue {
    fn from(value: i32) -> Self {
        Self::Int(value as i64)
    }
}
impl From<u32> for StmtSummaryValue {
    fn from(value: u32) -> Self {
        Self::Uint(value as u64)
    }
}
impl From<u64> for StmtSummaryValue {
    fn from(value: u64) -> Self {
        Self::Uint(value)
    }
}
impl From<f64> for StmtSummaryValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}
impl From<bool> for StmtSummaryValue {
    fn from(value: bool) -> Self {
        Self::Int(i64::from(value))
    }
}
impl From<Duration> for StmtSummaryValue {
    fn from(value: Duration) -> Self {
        Self::Int(duration_nanos(value))
    }
}
impl From<types::Time> for StmtSummaryValue {
    fn from(value: types::Time) -> Self {
        Self::Time(value)
    }
}
impl From<types::Datum> for StmtSummaryValue {
    fn from(value: types::Datum) -> Self {
        Self::Datum(value)
    }
}

fn duration_nanos(value: Duration) -> i64 {
    value.as_nanos().min(i64::MAX as u128) as i64
}

fn timestamp_value(seconds: i64, tz: chrono_tz::Tz) -> types::Time {
    let utc = DateTime::<Utc>::from_timestamp(seconds, 0)
        .expect("statement summary timestamp is outside chrono range");
    types::NewTime(
        types::FromGoTime(utc.with_timezone(&tz)),
        mysql::TypeTimestamp,
        0,
    )
}

fn system_time_seconds(value: SystemTime) -> i64 {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs().min(i64::MAX as u64) as i64,
        Err(error) => -(error.duration().as_secs().min(i64::MAX as u64) as i64),
    }
}

trait ToI64 {
    fn to_i64(self) -> i64;
}
impl ToI64 for Duration {
    fn to_i64(self) -> i64 {
        duration_nanos(self)
    }
}
impl ToI64 for i64 {
    fn to_i64(self) -> i64 {
        self
    }
}
impl ToI64 for i32 {
    fn to_i64(self) -> i64 {
        self as i64
    }
}
impl ToI64 for u64 {
    fn to_i64(self) -> i64 {
        self.min(i64::MAX as u64) as i64
    }
}
impl ToI64 for u32 {
    fn to_i64(self) -> i64 {
        self as i64
    }
}

// columnValueFactory 对应 Go 的函数类型，四个参数分别是 reader、可选的窗口 element、digest 聚合和统计值。
/// 按列从 reader/窗口/digest/统计中取值的工厂函数类型。
pub type columnValueFactory = fn(
    &stmtSummaryReader,
    Option<&stmtSummaryByDigestElement>,
    Option<&stmtSummaryByDigest>,
    &stmtSummaryStats,
) -> StmtSummaryValue;

// Go 的 map 字面量很大；用宏减少重复，但每个 insert 仍按 Go 顺序列出。
macro_rules! insert_factory {
    ($map:ident, $key:ident, |$reader:ident, $ssElement:ident, $ssbd:ident, $ssStats:ident| $body:block) => {
        $map.insert(
            $key,
            |$reader, $ssElement, $ssbd, $ssStats| -> StmtSummaryValue {
                StmtSummaryValue::from($body)
            },
        );
    };
}

macro_rules! stat_field {
    ($map:ident, $key:ident, $field:ident) => {
        insert_factory!($map, $key, |_reader, _ssElement, _ssbd, ssStats| {
            ssStats.$field.clone()
        });
    };
}

macro_rules! ru_field {
    ($map:ident, $key:ident, $field:ident) => {
        insert_factory!($map, $key, |_reader, _ssElement, _ssbd, ssStats| {
            ssStats.StmtRUSummary.$field.clone()
        });
    };
}

macro_rules! network_field {
    ($map:ident, $key:ident, $field:ident) => {
        insert_factory!($map, $key, |_reader, _ssElement, _ssbd, ssStats| {
            ssStats.StmtNetworkTrafficSummary.$field
        });
    };
}

macro_rules! digest_field {
    ($map:ident, $key:ident, $field:ident) => {
        insert_factory!($map, $key, |_reader, _ssElement, ssbd, _ssStats| {
            let ssbd = ssbd.expect("Go factory expected non-nil stmtSummaryByDigest");
            ssbd.$field.clone()
        });
    };
}

macro_rules! avg_int_stat {
    ($map:ident, $key:ident, $sum:ident, $count:ident) => {
        insert_factory!($map, $key, |_reader, _ssElement, _ssbd, ssStats| {
            avgInt(ssStats.$sum.to_i64(), ssStats.$count)
        });
    };
}

macro_rules! avg_float_stat {
    ($map:ident, $key:ident, $sum:ident, $count:ident) => {
        insert_factory!($map, $key, |_reader, _ssElement, _ssbd, ssStats| {
            avgFloat(ssStats.$sum.to_i64(), ssStats.$count)
        });
    };
}

macro_rules! avg_sum_float_stat {
    ($map:ident, $key:ident, $sum:ident, $count:ident) => {
        insert_factory!($map, $key, |_reader, _ssElement, _ssbd, ssStats| {
            avgSumFloat(ssStats.$sum, ssStats.$count)
        });
    };
}

// columnValueFactoryMap 对应 Go 的全局 var map[string]columnValueFactory。
// 这个构造函数只登记列名到取值逻辑，不触发数据库访问；时间转换、计划解码错误处理等 Go 语义保留在闭包里。
/// 构建列名 → 取值工厂映射（与 Go 列顺序一致）。
pub fn columnValueFactoryMap() -> HashMap<&'static str, columnValueFactory> {
    let mut map: HashMap<&'static str, columnValueFactory> = HashMap::new();

    insert_factory!(
        map,
        ClusterTableInstanceColumnNameStr,
        |reader, _ssElement, _ssbd, _ssStats| { reader.instanceAddr.clone() }
    );
    insert_factory!(
        map,
        SummaryBeginTimeStr,
        |reader, ssElement, _ssbd, _ssStats| {
            let ssElement = ssElement.expect("SUMMARY_BEGIN_TIME needs stmtSummaryByDigestElement");
            timestamp_value(ssElement.beginTime, reader.tz)
        }
    );
    insert_factory!(
        map,
        SummaryEndTimeStr,
        |reader, ssElement, _ssbd, _ssStats| {
            let ssElement = ssElement.expect("SUMMARY_END_TIME needs stmtSummaryByDigestElement");
            timestamp_value(ssElement.endTime, reader.tz)
        }
    );
    digest_field!(map, StmtTypeStr, stmtType);
    insert_factory!(map, SchemaNameStr, |_reader, _ssElement, ssbd, _ssStats| {
        let ssbd = ssbd.expect("SCHEMA_NAME needs stmtSummaryByDigest");
        convertEmptyToNil(&ssbd.schemaName)
    });
    insert_factory!(map, DigestStr, |_reader, _ssElement, ssbd, _ssStats| {
        let ssbd = ssbd.expect("DIGEST needs stmtSummaryByDigest");
        convertEmptyToNil(&ssbd.digest)
    });
    digest_field!(map, DigestTextStr, normalizedSQL);
    insert_factory!(
        map,
        BindingDigestStr,
        |_reader, _ssElement, ssbd, _ssStats| {
            let ssbd = ssbd.expect("BINDING_DIGEST needs stmtSummaryByDigest");
            convertEmptyToNil(&ssbd.bindingDigest)
        }
    );
    digest_field!(map, BindingDigestTextStr, bindingSQL);
    insert_factory!(map, TableNamesStr, |_reader, _ssElement, ssbd, _ssStats| {
        let ssbd = ssbd.expect("TABLE_NAMES needs stmtSummaryByDigest");
        convertEmptyToNil(&ssbd.tableNames)
    });
    insert_factory!(map, IndexNamesStr, |_reader, _ssElement, _ssbd, ssStats| {
        convertEmptyToNil(&ssStats.indexNames.join(","))
    });
    insert_factory!(map, SampleUserStr, |_reader, _ssElement, _ssbd, ssStats| {
        // Go 遍历 map 取任意一个 sampleUser；用 keys().next() 表达相同的非确定样例语义。
        let sampleUser = ssStats.authUsers.iter().next().cloned().unwrap_or_default();
        convertEmptyToNil(&sampleUser)
    });
    stat_field!(map, ExecCountStr, execCount);
    stat_field!(map, ErrorsStr, sumErrors);
    stat_field!(map, SumErrorsStr, sumErrors);
    stat_field!(map, WarningsStr, sumWarnings);
    stat_field!(map, SumWarningsStr, sumWarnings);
    insert_factory!(map, TotalTimeStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.sumLatency)
    });
    insert_factory!(map, SumLatencyStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.sumLatency)
    });
    insert_factory!(map, MaxLatencyStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.maxLatency)
    });
    insert_factory!(map, MinLatencyStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.minLatency)
    });
    avg_int_stat!(map, AvgLatencyStr, sumLatency, execCount);
    insert_factory!(map, ParseTimeStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.sumParseLatency)
    });
    avg_int_stat!(map, AvgParseLatencyStr, sumParseLatency, execCount);
    insert_factory!(
        map,
        MaxParseLatencyStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxParseLatency) }
    );
    insert_factory!(
        map,
        CompileTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumCompileLatency) }
    );
    avg_int_stat!(map, AvgCompileLatencyStr, sumCompileLatency, execCount);
    insert_factory!(
        map,
        MaxCompileLatencyStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxCompileLatency) }
    );
    stat_field!(map, CopTaskNumStr, sumNumCopTasks);
    stat_field!(map, SumCopTaskNumStr, sumNumCopTasks);
    insert_factory!(
        map,
        CopProcessTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumCopProcessTime) }
    );
    insert_factory!(
        map,
        MaxCopProcessTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxCopProcessTime) }
    );
    insert_factory!(
        map,
        MaxCopProcessAddressStr,
        |_reader, _ssElement, _ssbd, ssStats| { convertEmptyToNil(&ssStats.maxCopProcessAddress) }
    );
    insert_factory!(
        map,
        CopWaitTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumCopWaitTime) }
    );
    insert_factory!(
        map,
        MaxCopWaitTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxCopWaitTime) }
    );
    insert_factory!(
        map,
        MaxCopWaitAddressStr,
        |_reader, _ssElement, _ssbd, ssStats| { convertEmptyToNil(&ssStats.maxCopWaitAddress) }
    );
    insert_factory!(
        map,
        ProcessTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumProcessTime) }
    );
    avg_int_stat!(map, AvgProcessTimeStr, sumProcessTime, execCount);
    insert_factory!(
        map,
        MaxProcessTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxProcessTime) }
    );
    insert_factory!(map, WaitTimeStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.sumWaitTime)
    });
    avg_int_stat!(map, AvgWaitTimeStr, sumWaitTime, execCount);
    insert_factory!(
        map,
        MaxWaitTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxWaitTime) }
    );
    insert_factory!(
        map,
        BackoffTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumBackoffTime) }
    );
    avg_int_stat!(map, AvgBackoffTimeStr, sumBackoffTime, execCount);
    insert_factory!(
        map,
        MaxBackoffTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxBackoffTime) }
    );
    stat_field!(map, TotalKeysStr, sumTotalKeys);
    avg_int_stat!(map, AvgTotalKeysStr, sumTotalKeys, execCount);
    stat_field!(map, MaxTotalKeysStr, maxTotalKeys);
    stat_field!(map, ProcessedKeysStr, sumProcessedKeys);
    avg_int_stat!(map, AvgProcessedKeysStr, sumProcessedKeys, execCount);
    stat_field!(map, MaxProcessedKeysStr, maxProcessedKeys);
    stat_field!(
        map,
        RocksdbDeleteSkippedCountStr,
        sumRocksdbDeleteSkippedCount
    );
    avg_int_stat!(
        map,
        AvgRocksdbDeleteSkippedCountStr,
        sumRocksdbDeleteSkippedCount,
        execCount
    );
    stat_field!(
        map,
        MaxRocksdbDeleteSkippedCountStr,
        maxRocksdbDeleteSkippedCount
    );
    stat_field!(map, RocksdbKeySkippedCountStr, sumRocksdbKeySkippedCount);
    avg_int_stat!(
        map,
        AvgRocksdbKeySkippedCountStr,
        sumRocksdbKeySkippedCount,
        execCount
    );
    stat_field!(map, MaxRocksdbKeySkippedCountStr, maxRocksdbKeySkippedCount);
    stat_field!(
        map,
        RocksdbBlockCacheHitCountStr,
        sumRocksdbBlockCacheHitCount
    );
    avg_int_stat!(
        map,
        AvgRocksdbBlockCacheHitCountStr,
        sumRocksdbBlockCacheHitCount,
        execCount
    );
    stat_field!(
        map,
        MaxRocksdbBlockCacheHitCountStr,
        maxRocksdbBlockCacheHitCount
    );
    stat_field!(map, RocksdbBlockReadCountStr, sumRocksdbBlockReadCount);
    avg_int_stat!(
        map,
        AvgRocksdbBlockReadCountStr,
        sumRocksdbBlockReadCount,
        execCount
    );
    stat_field!(map, MaxRocksdbBlockReadCountStr, maxRocksdbBlockReadCount);
    stat_field!(map, RocksdbBlockReadByteStr, sumRocksdbBlockReadByte);
    avg_int_stat!(
        map,
        AvgRocksdbBlockReadByteStr,
        sumRocksdbBlockReadByte,
        execCount
    );
    stat_field!(map, MaxRocksdbBlockReadByteStr, maxRocksdbBlockReadByte);
    insert_factory!(
        map,
        PrewriteTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumPrewriteTime) }
    );
    avg_int_stat!(map, AvgPrewriteTimeStr, sumPrewriteTime, commitCount);
    insert_factory!(
        map,
        MaxPrewriteTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxPrewriteTime) }
    );
    insert_factory!(map, CommitTimeStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.sumCommitTime)
    });
    avg_int_stat!(map, AvgCommitTimeStr, sumCommitTime, commitCount);
    insert_factory!(
        map,
        MaxCommitTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxCommitTime) }
    );
    insert_factory!(
        map,
        CommitTsTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumGetCommitTsTime) }
    );
    avg_int_stat!(map, AvgGetCommitTsTimeStr, sumGetCommitTsTime, commitCount);
    insert_factory!(
        map,
        MaxGetCommitTsTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxGetCommitTsTime) }
    );
    stat_field!(map, CommitBackoffTimeStr, sumCommitBackoffTime);
    avg_int_stat!(
        map,
        AvgCommitBackoffTimeStr,
        sumCommitBackoffTime,
        commitCount
    );
    stat_field!(map, MaxCommitBackoffTimeStr, maxCommitBackoffTime);
    stat_field!(map, ResolveLockTimeStr, sumResolveLockTime);
    avg_int_stat!(map, AvgResolveLockTimeStr, sumResolveLockTime, commitCount);
    stat_field!(map, MaxResolveLockTimeStr, maxResolveLockTime);
    insert_factory!(
        map,
        LocalLatchWaitTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumLocalLatchTime) }
    );
    avg_int_stat!(
        map,
        AvgLocalLatchWaitTimeStr,
        sumLocalLatchTime,
        commitCount
    );
    insert_factory!(
        map,
        MaxLocalLatchWaitTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.maxLocalLatchTime) }
    );
    stat_field!(map, WriteKeysStr, sumWriteKeys);
    avg_float_stat!(map, AvgWriteKeysStr, sumWriteKeys, commitCount);
    stat_field!(map, MaxWriteKeysStr, maxWriteKeys);
    stat_field!(map, WriteSizeStr, sumWriteSize);
    avg_float_stat!(map, AvgWriteSizeStr, sumWriteSize, commitCount);
    stat_field!(map, MaxWriteSizeStr, maxWriteSize);
    stat_field!(map, PrewriteRegionsStr, sumPrewriteRegionNum);
    avg_float_stat!(
        map,
        AvgPrewriteRegionsStr,
        sumPrewriteRegionNum,
        commitCount
    );
    insert_factory!(
        map,
        MaxPrewriteRegionsStr,
        |_reader, _ssElement, _ssbd, ssStats| { ssStats.maxPrewriteRegionNum as i32 }
    );
    stat_field!(map, TxnRetryStr, sumTxnRetry);
    avg_float_stat!(map, AvgTxnRetryStr, sumTxnRetry, commitCount);
    stat_field!(map, MaxTxnRetryStr, maxTxnRetry);
    insert_factory!(map, ExecRetryStr, |_reader, _ssElement, _ssbd, ssStats| {
        ssStats.execRetryCount as i32
    });
    insert_factory!(
        map,
        SumExecRetryStr,
        |_reader, _ssElement, _ssbd, ssStats| { ssStats.execRetryCount as i32 }
    );
    insert_factory!(
        map,
        ExecRetryTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.execRetryTime) }
    );
    insert_factory!(
        map,
        SumExecRetryTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.execRetryTime) }
    );
    stat_field!(map, BackoffTimesStr, sumBackoffTimes);
    stat_field!(map, SumBackoffTimesStr, sumBackoffTimes);
    insert_factory!(
        map,
        BackoffTypesStr,
        |_reader, _ssElement, _ssbd, ssStats| { formatBackoffTypes(&ssStats.backoffTypes) }
    );
    stat_field!(map, MemStr, sumMem);
    avg_int_stat!(map, AvgMemStr, sumMem, execCount);
    stat_field!(map, MaxMemStr, maxMem);
    stat_field!(map, MemArbitrationStr, sumMemArbitration);
    avg_sum_float_stat!(map, AvgMemArbitrationStr, sumMemArbitration, execCount);
    stat_field!(map, MaxMemArbitrationStr, maxMemArbitration);
    stat_field!(map, DiskStr, sumDisk);
    avg_int_stat!(map, AvgDiskStr, sumDisk, execCount);
    stat_field!(map, MaxDiskStr, maxDisk);
    insert_factory!(map, KvTimeStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.sumKVTotal)
    });
    avg_int_stat!(map, AvgKvTimeStr, sumKVTotal, commitCount);
    insert_factory!(map, PdTimeStr, |_reader, _ssElement, _ssbd, ssStats| {
        duration_nanos(ssStats.sumPDTotal)
    });
    avg_int_stat!(map, AvgPdTimeStr, sumPDTotal, commitCount);
    insert_factory!(
        map,
        BackoffTotalTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumBackoffTotal) }
    );
    avg_int_stat!(map, AvgBackoffTotalTimeStr, sumBackoffTotal, commitCount);
    insert_factory!(
        map,
        WriteSQLRespTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| { duration_nanos(ssStats.sumWriteSQLRespTotal) }
    );
    avg_int_stat!(
        map,
        AvgWriteSQLRespTimeStr,
        sumWriteSQLRespTotal,
        commitCount
    );
    avg_int_stat!(map, AvgTidbCPUTimeStr, sumTidbCPU, execCount);
    avg_int_stat!(map, AvgTikvCPUTimeStr, sumTikvCPU, execCount);
    stat_field!(map, ResultRowsStr, sumResultRows);
    stat_field!(map, MaxResultRowsStr, maxResultRows);
    stat_field!(map, MinResultRowsStr, minResultRows);
    stat_field!(map, AffectedRowsStr, sumAffectedRows);
    avg_int_stat!(map, AvgResultRowsStr, sumResultRows, execCount);
    stat_field!(map, PreparedStr, prepared);
    insert_factory!(
        map,
        AvgAffectedRowsStr,
        |_reader, _ssElement, _ssbd, ssStats| {
            avgFloat(ssStats.sumAffectedRows as i64, ssStats.execCount)
        }
    );
    insert_factory!(map, FirstSeenStr, |reader, _ssElement, _ssbd, ssStats| {
        timestamp_value(system_time_seconds(ssStats.firstSeen), reader.tz)
    });
    insert_factory!(map, LastSeenStr, |reader, _ssElement, _ssbd, ssStats| {
        timestamp_value(system_time_seconds(ssStats.lastSeen), reader.tz)
    });
    stat_field!(map, PlanInCacheStr, planInCache);
    stat_field!(map, PlanCacheHitsStr, planCacheHits);
    stat_field!(map, PlanInBindingStr, planInBinding);
    stat_field!(map, QuerySampleTextStr, sampleSQL);
    stat_field!(map, PrevSampleTextStr, prevSQL);
    digest_field!(map, PlanDigestStr, planDigest);
    insert_factory!(map, PlanStr, |_reader, _ssElement, _ssbd, ssStats| {
        // DecodePlan 失败时 Go 代码记录结构化日志并返回空字符串；这里保留错误处理分支。
        match plancodec::DecodePlan(&ssStats.samplePlan) {
            Ok(plan) => plan,
            Err(err) => {
                log::error!(
                    "decode plan in statement summary failed: plan={:?}, query={:?}, error={}",
                    ssStats.samplePlan,
                    ssStats.sampleSQL,
                    err,
                );
                Vec::new()
            }
        }
    });
    stat_field!(map, BinaryPlan, sampleBinaryPlan);
    stat_field!(map, Charset, charset);
    stat_field!(map, Collation, collation);
    stat_field!(map, PlanHint, planHint);
    ru_field!(map, RequestUnitReadStr, SumRRU);
    insert_factory!(
        map,
        AvgRequestUnitReadStr,
        |_reader, _ssElement, _ssbd, ssStats| {
            avgSumFloat(ssStats.StmtRUSummary.SumRRU, ssStats.execCount)
        }
    );
    ru_field!(map, MaxRequestUnitReadStr, MaxRRU);
    ru_field!(map, RequestUnitWriteStr, SumWRU);
    insert_factory!(
        map,
        AvgRequestUnitWriteStr,
        |_reader, _ssElement, _ssbd, ssStats| {
            avgSumFloat(ssStats.StmtRUSummary.SumWRU, ssStats.execCount)
        }
    );
    ru_field!(map, MaxRequestUnitWriteStr, MaxWRU);
    insert_factory!(
        map,
        QueuedRcTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| {
            duration_nanos(ssStats.StmtRUSummary.SumRUWaitDuration)
        }
    );
    insert_factory!(
        map,
        AvgQueuedRcTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| {
            avgInt(
                duration_nanos(ssStats.StmtRUSummary.SumRUWaitDuration),
                ssStats.execCount,
            )
        }
    );
    insert_factory!(
        map,
        MaxQueuedRcTimeStr,
        |_reader, _ssElement, _ssbd, ssStats| {
            duration_nanos(ssStats.StmtRUSummary.MaxRUWaitDuration)
        }
    );
    insert_factory!(
        map,
        AvgRequestUnitV2Str,
        |_reader, _ssElement, _ssbd, ssStats| {
            avgSumFloat(ssStats.StmtRUSummary.SumRUV2, ssStats.execCount)
        }
    );
    ru_field!(map, MaxRequestUnitV2Str, MaxRUV2);
    stat_field!(map, ResourceGroupName, resourceGroupName);
    stat_field!(map, PlanCacheUnqualifiedStr, planCacheUnqualifiedCount);
    stat_field!(
        map,
        PlanCacheUnqualifiedLastReasonStr,
        lastPlanCacheUnqualified
    );
    network_field!(
        map,
        SumUnpackedBytesSentTiKVTotalStr,
        UnpackedBytesSentTiKVTotal
    );
    network_field!(
        map,
        SumUnpackedBytesReceivedTiKVTotalStr,
        UnpackedBytesReceivedTiKVTotal
    );
    network_field!(
        map,
        SumUnpackedBytesSentTiKVCrossZoneStr,
        UnpackedBytesSentTiKVCrossZone
    );
    network_field!(
        map,
        SumUnpackedBytesReceivedTiKVCrossZoneStr,
        UnpackedBytesReceivedTiKVCrossZone
    );
    network_field!(
        map,
        SumUnpackedBytesSentTiFlashTotalStr,
        UnpackedBytesSentTiFlashTotal
    );
    network_field!(
        map,
        SumUnpackedBytesReceivedTiFlashTotalStr,
        UnpackedBytesReceivedTiFlashTotal
    );
    network_field!(
        map,
        SumUnpackedBytesSentTiFlashCrossZoneStr,
        UnpackedBytesSentTiFlashCrossZone
    );
    network_field!(
        map,
        SumUnpackedBytesReceiveTiFlashCrossZoneStr,
        UnpackedBytesReceivedTiFlashCrossZone
    );
    network_field!(
        map,
        UnpackedBytesSentTiKVTotalStr,
        UnpackedBytesSentTiKVTotal
    );
    network_field!(
        map,
        UnpackedBytesReceivedTiKVTotalStr,
        UnpackedBytesReceivedTiKVTotal
    );
    network_field!(
        map,
        UnpackedBytesSentTiKVCrossZoneStr,
        UnpackedBytesSentTiKVCrossZone
    );
    network_field!(
        map,
        UnpackedBytesReceivedTiKVCrossZoneStr,
        UnpackedBytesReceivedTiKVCrossZone
    );
    network_field!(
        map,
        UnpackedBytesSentTiFlashTotalStr,
        UnpackedBytesSentTiFlashTotal
    );
    network_field!(
        map,
        UnpackedBytesReceivedTiFlashTotalStr,
        UnpackedBytesReceivedTiFlashTotal
    );
    network_field!(
        map,
        UnpackedBytesSentTiFlashCrossZoneStr,
        UnpackedBytesSentTiFlashCrossZone
    );
    network_field!(
        map,
        UnpackedBytesReceiveTiFlashCrossZoneStr,
        UnpackedBytesReceivedTiFlashCrossZone
    );
    stat_field!(map, StorageKVStr, storageKV);
    stat_field!(map, StorageMPPStr, storageMPP);

    map
}
