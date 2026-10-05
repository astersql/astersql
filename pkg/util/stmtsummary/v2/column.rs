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

// 语句摘要表列定义与取值工厂。
//
// 将 `StmtRecord` 各聚合字段映射为 `INFORMATION_SCHEMA` 风格列（DIGEST、延迟、
// Coprocessor、两阶段提交 Prewrite/Commit、RocksDB、请求单元 RU 等）。
// `columnFactoryMap` 按列名注册工厂；`makeColumnFactories` 按表定义顺序组装。
// Digest 为 SQL 归一化指纹；两阶段提交（2PC）是分布式事务的预写与提交阶段。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use crate::{StmtRecord, model, mysql, plancodec, types};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 列工厂产出的中间值，再转为 TiDB Datum。
#[derive(Clone)]
pub enum ColumnValue {
    Null,
    Int(i64),
    Uint(u64),
    Float(f64),
    String(String),
    Time(types::Time),
    Datum(types::Datum),
}

impl ColumnValue {
    /// 转为 types::Datum，供表扫描行组装。
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

/// 为若干数值类型批量实现 `From` → `ColumnValue` 变体。
macro_rules! column_value_from {
    ($variant:ident: $($ty:ty),+ $(,)?) => {$(
        impl From<$ty> for ColumnValue {
            fn from(value: $ty) -> Self { Self::$variant(value as _) }
        }
    )+};
}

column_value_from!(Int: i64, i32, isize);
column_value_from!(Uint: u64, u32, usize);
column_value_from!(Float: f64);

// Go strings can hold non-UTF-8 plan bytes; preserve their string Datum kind.
impl From<Vec<u8>> for ColumnValue {
    fn from(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(text) => Self::String(text),
            Err(error) => {
                let mut datum = types::NewStringDatum(String::new());
                datum.SetBytesAsString(error.into_bytes(), datum.Collation(), 0);
                Self::Datum(datum)
            }
        }
    }
}

impl From<String> for ColumnValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}
impl From<&str> for ColumnValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}
impl From<bool> for ColumnValue {
    fn from(value: bool) -> Self {
        Self::Int(i64::from(value))
    }
}
impl From<types::Time> for ColumnValue {
    fn from(value: types::Time) -> Self {
        Self::Time(value)
    }
}
impl From<Option<String>> for ColumnValue {
    fn from(value: Option<String>) -> Self {
        value.map_or(Self::Null, Self::String)
    }
}

/// 对应 Go `interface{}` 装箱：任意可转 `ColumnValue` 的值。
fn go_any<T: Into<ColumnValue>>(value: T) -> ColumnValue {
    value.into()
}
/// 对应 Go `nil`，映射为 `ColumnValue::Null`。
fn go_nil() -> ColumnValue {
    ColumnValue::Null
}

/// Duration 转纳秒 i64，溢出钳制到 i64::MAX。
fn duration_nanos(value: Duration) -> i64 {
    value.as_nanos().min(i64::MAX as u128) as i64
}

/// Unix 秒 + 会话时区 → TiDB timestamp 类型。
fn timestamp_value(seconds: i64, tz: Tz) -> types::Time {
    let utc = DateTime::<Utc>::from_timestamp(seconds, 0)
        .expect("statement summary timestamp is outside chrono range");
    types::NewTime(
        types::FromGoTime(utc.with_timezone(&tz)),
        mysql::TypeTimestamp,
        0,
    )
}

/// 将 SystemTime / Option 统一转为 Unix 秒。
trait ToSystemTimeSeconds {
    fn to_seconds(&self) -> i64;
}

impl ToSystemTimeSeconds for SystemTime {
    fn to_seconds(&self) -> i64 {
        match self.duration_since(UNIX_EPOCH) {
            Ok(value) => value.as_secs().min(i64::MAX as u64) as i64,
            Err(error) => {
                let duration = error.duration();
                let whole_seconds = duration.as_secs().min(i64::MAX as u64) as i64;
                if duration.subsec_nanos() == 0 {
                    -whole_seconds
                } else {
                    whole_seconds
                        .checked_add(1)
                        .map_or(i64::MIN, |seconds| -seconds)
                }
            }
        }
    }
}

impl ToSystemTimeSeconds for Option<SystemTime> {
    fn to_seconds(&self) -> i64 {
        self.as_ref().map_or(0, ToSystemTimeSeconds::to_seconds)
    }
}

fn system_time_seconds(value: &impl ToSystemTimeSeconds) -> i64 {
    value.to_seconds()
}

// Statements summary table column name.
// 下面常量逐项对应 Go const 块，保持 statement summary 表列名的声明顺序和字符串值。
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
pub const IAExecCountStr: &str = "IA_REMOTE_EXEC_COUNT";
pub const AvgIARemoteReadSegmentCountStr: &str = "AVG_IA_REMOTE_READ_SEGMENT_COUNT";
pub const MaxIARemoteReadSegmentCountStr: &str = "MAX_IA_REMOTE_READ_SEGMENT_COUNT";
pub const AvgIARemoteReadSegmentSizeStr: &str = "AVG_IA_REMOTE_READ_SEGMENT_SIZE";
pub const MaxIARemoteReadSegmentSizeStr: &str = "MAX_IA_REMOTE_READ_SEGMENT_SIZE";
pub const AvgIARemoteReadSegmentWaitTimeStr: &str = "AVG_IA_REMOTE_READ_SEGMENT_WAIT_TIME";
pub const MaxIARemoteReadSegmentWaitTimeStr: &str = "MAX_IA_REMOTE_READ_SEGMENT_WAIT_TIME";
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
pub const AvgRequestUnitRead: &str = "AVG_REQUEST_UNIT_READ";
pub const MaxRequestUnitRead: &str = "MAX_REQUEST_UNIT_READ";
pub const AvgRequestUnitWrite: &str = "AVG_REQUEST_UNIT_WRITE";
pub const MaxRequestUnitWrite: &str = "MAX_REQUEST_UNIT_WRITE";
pub const AvgQueuedRcTimeStr: &str = "AVG_QUEUED_RC_TIME";
pub const MaxQueuedRcTimeStr: &str = "MAX_QUEUED_RC_TIME";
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

// columnInfo 对应 Go interface columnInfo。
// 调用方提供实例地址和会话时区；本文件只读这些上下文，不持有连接或外部资源。
/// 列工厂上下文：实例地址与会话时区。
pub trait ColumnInfoContext {
    fn getInstanceAddr(&self) -> &str;
    fn getTimeLocation(&self) -> Tz;
}

#[derive(Clone)]
/// 默认列上下文实现。
pub struct ColumnContext {
    instance_addr: String,
    time_location: Tz,
}

impl ColumnContext {
    /// 由实例地址与时区构造上下文。
    pub fn new(instance_addr: impl Into<String>, time_location: Tz) -> Self {
        Self {
            instance_addr: instance_addr.into(),
            time_location,
        }
    }
}

impl ColumnInfoContext for ColumnContext {
    fn getInstanceAddr(&self) -> &str {
        &self.instance_addr
    }
    fn getTimeLocation(&self) -> Tz {
        self.time_location
    }
}

/// 列工厂函数类型：`(上下文, 记录) -> 列值`。
pub type ColumnFactory = fn(info: &dyn ColumnInfoContext, record: &StmtRecord) -> ColumnValue;

// columnFactoryMap 对应 Go 的 map[string]columnFactory。
// LazyLock 只是 实现中表达“全局延迟初始化 map”的占位；每个闭包仍按 Go map 条目顺序排列。
static columnFactoryMap: LazyLock<HashMap<&'static str, ColumnFactory>> = LazyLock::new(|| {
    let mut factories: HashMap<&'static str, ColumnFactory> = HashMap::new();

    factories.insert(ClusterTableInstanceColumnNameStr, |info, _record| {
        go_any(info.getInstanceAddr())
    });
    factories.insert(SummaryBeginTimeStr, |info, record| {
        // Go time.Unix(record.Begin, 0) 先用 Unix 秒构造时间，再按请求时区转换后包装成 TiDB timestamp。
        go_any(timestamp_value(record.Begin, info.getTimeLocation()))
    });
    factories.insert(SummaryEndTimeStr, |info, record| {
        // End 与 Begin 的时区处理完全一致，保持 Go 里只在 Location 不同时才调用 In 的分支。
        go_any(timestamp_value(record.End, info.getTimeLocation()))
    });
    factories.insert(StmtTypeStr, |_info, record| go_any(record.StmtType.clone()));
    factories.insert(SchemaNameStr, |_info, record| {
        go_any(convertEmptyToNil(&record.SchemaName))
    });
    factories.insert(DigestStr, |_info, record| {
        go_any(convertEmptyToNil(&record.Digest))
    });
    factories.insert(DigestTextStr, |_info, record| {
        go_any(record.NormalizedSQL.clone())
    });
    factories.insert(BindingDigestStr, |_info, record| {
        go_any(convertEmptyToNil(&record.BindingDigest))
    });
    factories.insert(BindingDigestTextStr, |_info, record| {
        go_any(record.BindingSQL.clone())
    });
    factories.insert(TableNamesStr, |_info, record| {
        go_any(convertEmptyToNil(&record.TableNames))
    });
    factories.insert(IndexNamesStr, |_info, record| {
        // Go strings.Join(record.IndexNames, ",") 会在空 slice 时得到空字符串，随后转为 nil。
        go_any(convertEmptyToNil(&record.IndexNames.join(",")))
    });
    factories.insert(SampleUserStr, |_info, record| {
        let mut sampleUser = String::new();
        // Go map 遍历顺序不稳定；这里同样只取第一个遇到的 key，保留 sample user 的非确定性语义。
        for key in record.AuthUsers.iter() {
            sampleUser = key.clone();
            break;
        }
        go_any(convertEmptyToNil(&sampleUser))
    });
    factories.insert(ExecCountStr, |_info, record| go_any(record.ExecCount));
    factories.insert(SumErrorsStr, |_info, record| go_any(record.SumErrors));
    factories.insert(SumWarningsStr, |_info, record| go_any(record.SumWarnings));
    factories.insert(SumLatencyStr, |_info, record| {
        go_any(duration_nanos(record.SumLatency))
    });
    factories.insert(MaxLatencyStr, |_info, record| {
        go_any(duration_nanos(record.MaxLatency))
    });
    factories.insert(MinLatencyStr, |_info, record| {
        go_any(duration_nanos(record.MinLatency))
    });
    factories.insert(AvgLatencyStr, |_info, record| {
        go_any(avgInt(duration_nanos(record.SumLatency), record.ExecCount))
    });
    factories.insert(AvgParseLatencyStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumParseLatency),
            record.ExecCount,
        ))
    });
    factories.insert(MaxParseLatencyStr, |_info, record| {
        go_any(duration_nanos(record.MaxParseLatency))
    });
    factories.insert(AvgCompileLatencyStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumCompileLatency),
            record.ExecCount,
        ))
    });
    factories.insert(MaxCompileLatencyStr, |_info, record| {
        go_any(duration_nanos(record.MaxCompileLatency))
    });
    factories.insert(SumCopTaskNumStr, |_info, record| {
        go_any(record.SumNumCopTasks)
    });
    factories.insert(MaxCopProcessTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxCopProcessTime))
    });
    factories.insert(MaxCopProcessAddressStr, |_info, record| {
        go_any(convertEmptyToNil(&record.MaxCopProcessAddress))
    });
    factories.insert(MaxCopWaitTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxCopWaitTime))
    });
    factories.insert(MaxCopWaitAddressStr, |_info, record| {
        go_any(convertEmptyToNil(&record.MaxCopWaitAddress))
    });
    factories.insert(AvgProcessTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumProcessTime),
            record.ExecCount,
        ))
    });
    factories.insert(MaxProcessTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxProcessTime))
    });
    factories.insert(AvgWaitTimeStr, |_info, record| {
        go_any(avgInt(duration_nanos(record.SumWaitTime), record.ExecCount))
    });
    factories.insert(MaxWaitTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxWaitTime))
    });
    factories.insert(AvgBackoffTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumBackoffTime),
            record.ExecCount,
        ))
    });
    factories.insert(MaxBackoffTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxBackoffTime))
    });
    factories.insert(AvgTotalKeysStr, |_info, record| {
        go_any(avgInt(record.SumTotalKeys, record.ExecCount))
    });
    factories.insert(MaxTotalKeysStr, |_info, record| go_any(record.MaxTotalKeys));
    factories.insert(AvgProcessedKeysStr, |_info, record| {
        go_any(avgInt(record.SumProcessedKeys, record.ExecCount))
    });
    factories.insert(MaxProcessedKeysStr, |_info, record| {
        go_any(record.MaxProcessedKeys)
    });
    factories.insert(AvgRocksdbDeleteSkippedCountStr, |_info, record| {
        go_any(avgFloat4Uint(
            record.SumRocksdbDeleteSkippedCount,
            record.ExecCount,
        ))
    });
    factories.insert(MaxRocksdbDeleteSkippedCountStr, |_info, record| {
        go_any(record.MaxRocksdbDeleteSkippedCount)
    });
    factories.insert(AvgRocksdbKeySkippedCountStr, |_info, record| {
        go_any(avgFloat4Uint(
            record.SumRocksdbKeySkippedCount,
            record.ExecCount,
        ))
    });
    factories.insert(MaxRocksdbKeySkippedCountStr, |_info, record| {
        go_any(record.MaxRocksdbKeySkippedCount)
    });
    factories.insert(AvgRocksdbBlockCacheHitCountStr, |_info, record| {
        go_any(avgFloat4Uint(
            record.SumRocksdbBlockCacheHitCount,
            record.ExecCount,
        ))
    });
    factories.insert(MaxRocksdbBlockCacheHitCountStr, |_info, record| {
        go_any(record.MaxRocksdbBlockCacheHitCount)
    });
    factories.insert(AvgRocksdbBlockReadCountStr, |_info, record| {
        go_any(avgFloat4Uint(
            record.SumRocksdbBlockReadCount,
            record.ExecCount,
        ))
    });
    factories.insert(MaxRocksdbBlockReadCountStr, |_info, record| {
        go_any(record.MaxRocksdbBlockReadCount)
    });
    factories.insert(AvgRocksdbBlockReadByteStr, |_info, record| {
        go_any(avgFloat4Uint(
            record.SumRocksdbBlockReadByte,
            record.ExecCount,
        ))
    });
    factories.insert(MaxRocksdbBlockReadByteStr, |_info, record| {
        go_any(record.MaxRocksdbBlockReadByte)
    });
    factories.insert(IAExecCountStr, |_info, record| go_any(record.IAExecCount));
    factories.insert(AvgIARemoteReadSegmentCountStr, |_info, record| {
        go_any(avgFloat4Uint(
            record.SumIARemoteReadSegmentCount,
            record.ExecCount,
        ))
    });
    factories.insert(MaxIARemoteReadSegmentCountStr, |_info, record| {
        go_any(record.MaxIARemoteReadSegmentCount)
    });
    factories.insert(AvgIARemoteReadSegmentSizeStr, |_info, record| {
        go_any(avgFloat4Uint(
            record.SumIARemoteReadSegmentSize,
            record.ExecCount,
        ))
    });
    factories.insert(MaxIARemoteReadSegmentSizeStr, |_info, record| {
        go_any(record.MaxIARemoteReadSegmentSize)
    });
    factories.insert(AvgIARemoteReadSegmentWaitTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumIARemoteReadSegmentWaitTime),
            record.ExecCount,
        ))
    });
    factories.insert(MaxIARemoteReadSegmentWaitTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxIARemoteReadSegmentWaitTime))
    });
    factories.insert(AvgPrewriteTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumPrewriteTime),
            record.CommitCount,
        ))
    });
    factories.insert(MaxPrewriteTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxPrewriteTime))
    });
    factories.insert(AvgCommitTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumCommitTime),
            record.CommitCount,
        ))
    });
    factories.insert(MaxCommitTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxCommitTime))
    });
    factories.insert(AvgGetCommitTsTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumGetCommitTsTime),
            record.CommitCount,
        ))
    });
    factories.insert(MaxGetCommitTsTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxGetCommitTsTime))
    });
    factories.insert(AvgCommitBackoffTimeStr, |_info, record| {
        go_any(avgInt(record.SumCommitBackoffTime, record.CommitCount))
    });
    factories.insert(MaxCommitBackoffTimeStr, |_info, record| {
        go_any(record.MaxCommitBackoffTime)
    });
    factories.insert(AvgResolveLockTimeStr, |_info, record| {
        go_any(avgInt(record.SumResolveLockTime, record.CommitCount))
    });
    factories.insert(MaxResolveLockTimeStr, |_info, record| {
        go_any(record.MaxResolveLockTime)
    });
    factories.insert(AvgLocalLatchWaitTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumLocalLatchTime),
            record.CommitCount,
        ))
    });
    factories.insert(MaxLocalLatchWaitTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxLocalLatchTime))
    });
    factories.insert(AvgWriteKeysStr, |_info, record| {
        go_any(avgFloat(record.SumWriteKeys, record.CommitCount))
    });
    factories.insert(MaxWriteKeysStr, |_info, record| go_any(record.MaxWriteKeys));
    factories.insert(AvgWriteSizeStr, |_info, record| {
        go_any(avgFloat(record.SumWriteSize, record.CommitCount))
    });
    factories.insert(MaxWriteSizeStr, |_info, record| go_any(record.MaxWriteSize));
    factories.insert(AvgPrewriteRegionsStr, |_info, record| {
        go_any(avgFloat(record.SumPrewriteRegionNum, record.CommitCount))
    });
    factories.insert(MaxPrewriteRegionsStr, |_info, record| {
        go_any(record.MaxPrewriteRegionNum as isize)
    });
    factories.insert(AvgTxnRetryStr, |_info, record| {
        go_any(avgFloat(record.SumTxnRetry, record.CommitCount))
    });
    factories.insert(MaxTxnRetryStr, |_info, record| go_any(record.MaxTxnRetry));
    factories.insert(SumExecRetryStr, |_info, record| {
        go_any(record.ExecRetryCount as isize)
    });
    factories.insert(SumExecRetryTimeStr, |_info, record| {
        go_any(duration_nanos(record.ExecRetryTime))
    });
    factories.insert(SumBackoffTimesStr, |_info, record| {
        go_any(record.SumBackoffTimes)
    });
    factories.insert(BackoffTypesStr, |_info, record| {
        go_any(formatBackoffTypes(&record.BackoffTypes))
    });
    factories.insert(AvgMemStr, |_info, record| {
        go_any(avgInt(record.SumMem, record.ExecCount))
    });
    factories.insert(MaxMemStr, |_info, record| go_any(record.MaxMem));
    factories.insert(AvgMemArbitrationStr, |_info, record| {
        go_any(avgSumFloat(record.SumMemArbitration, record.ExecCount))
    });
    factories.insert(MaxMemArbitrationStr, |_info, record| {
        go_any(record.MaxMemArbitration)
    });
    factories.insert(AvgDiskStr, |_info, record| {
        go_any(avgInt(record.SumDisk, record.ExecCount))
    });
    factories.insert(MaxDiskStr, |_info, record| go_any(record.MaxDisk));
    factories.insert(AvgKvTimeStr, |_info, record| {
        go_any(avgInt(duration_nanos(record.SumKVTotal), record.ExecCount))
    });
    factories.insert(AvgPdTimeStr, |_info, record| {
        go_any(avgInt(duration_nanos(record.SumPDTotal), record.ExecCount))
    });
    factories.insert(AvgBackoffTotalTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumBackoffTotal),
            record.ExecCount,
        ))
    });
    factories.insert(AvgWriteSQLRespTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumWriteSQLRespTotal),
            record.ExecCount,
        ))
    });
    factories.insert(AvgTidbCPUTimeStr, |_info, record| {
        go_any(avgInt(duration_nanos(record.SumTidbCPU), record.ExecCount))
    });
    factories.insert(AvgTikvCPUTimeStr, |_info, record| {
        go_any(avgInt(duration_nanos(record.SumTikvCPU), record.ExecCount))
    });
    factories.insert(MaxResultRowsStr, |_info, record| {
        go_any(record.MaxResultRows)
    });
    factories.insert(MinResultRowsStr, |_info, record| {
        go_any(record.MinResultRows)
    });
    factories.insert(AvgResultRowsStr, |_info, record| {
        go_any(avgInt(record.SumResultRows, record.ExecCount))
    });
    factories.insert(PreparedStr, |_info, record| go_any(record.Prepared));
    factories.insert(AvgAffectedRowsStr, |_info, record| {
        go_any(avgFloat4Uint(record.SumAffectedRows, record.ExecCount))
    });
    factories.insert(FirstSeenStr, |info, record| {
        go_any(timestamp_value(
            system_time_seconds(&record.FirstSeen),
            info.getTimeLocation(),
        ))
    });
    factories.insert(LastSeenStr, |info, record| {
        go_any(timestamp_value(
            system_time_seconds(&record.LastSeen),
            info.getTimeLocation(),
        ))
    });
    factories.insert(PlanInCacheStr, |_info, record| go_any(record.PlanInCache));
    factories.insert(PlanCacheHitsStr, |_info, record| {
        go_any(record.PlanCacheHits)
    });
    factories.insert(PlanInBindingStr, |_info, record| {
        go_any(record.PlanInBinding)
    });
    factories.insert(QuerySampleTextStr, |_info, record| {
        go_any(record.SampleSQL.clone())
    });
    factories.insert(PrevSampleTextStr, |_info, record| {
        go_any(record.PrevSQL.clone())
    });
    factories.insert(PlanDigestStr, |_info, record| {
        go_any(record.PlanDigest.clone())
    });
    factories.insert(PlanStr, |_info, record| {
        // DecodePlan 可能失败。Go 代码记录结构化日志后返回空字符串；这里保留同样错误处理路径。
        let plan = match plancodec::DecodePlan(&record.SamplePlan) {
            Ok(plan) => plan,
            Err(error) => {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "decode plan in statement summary failed: plan={:?}, query={:?}, error={error}",
                    record.SamplePlan,
                    record.SampleSQL,
                );
                Vec::new()
            }
        };
        go_any(plan)
    });
    factories.insert(BinaryPlan, |_info, record| {
        go_any(record.SampleBinaryPlan.clone())
    });
    factories.insert(Charset, |_info, record| go_any(record.Charset.clone()));
    factories.insert(Collation, |_info, record| go_any(record.Collation.clone()));
    factories.insert(PlanHint, |_info, record| go_any(record.PlanHint.clone()));
    factories.insert(AvgRequestUnitRead, |_info, record| {
        go_any(avgSumFloat(record.SumRRU, record.ExecCount))
    });
    factories.insert(MaxRequestUnitRead, |_info, record| go_any(record.MaxRRU));
    factories.insert(AvgRequestUnitWrite, |_info, record| {
        go_any(avgSumFloat(record.SumWRU, record.ExecCount))
    });
    factories.insert(MaxRequestUnitWrite, |_info, record| go_any(record.MaxWRU));
    factories.insert(AvgQueuedRcTimeStr, |_info, record| {
        go_any(avgInt(
            duration_nanos(record.SumRUWaitDuration),
            record.ExecCount,
        ))
    });
    factories.insert(MaxQueuedRcTimeStr, |_info, record| {
        go_any(duration_nanos(record.MaxRUWaitDuration))
    });
    factories.insert(ResourceGroupName, |_info, record| {
        go_any(record.ResourceGroupName.clone())
    });
    factories.insert(PlanCacheUnqualifiedStr, |_info, record| {
        go_any(record.PlanCacheUnqualifiedCount)
    });
    factories.insert(PlanCacheUnqualifiedLastReasonStr, |_info, record| {
        go_any(record.PlanCacheUnqualifiedLastReason.clone())
    });
    factories.insert(SumUnpackedBytesSentTiKVTotalStr, |_info, record| {
        go_any(record.UnpackedBytesSentTiKVTotal)
    });
    factories.insert(SumUnpackedBytesReceivedTiKVTotalStr, |_info, record| {
        go_any(record.UnpackedBytesReceivedTiKVTotal)
    });
    factories.insert(SumUnpackedBytesSentTiKVCrossZoneStr, |_info, record| {
        go_any(record.UnpackedBytesSentTiKVCrossZone)
    });
    factories.insert(SumUnpackedBytesReceivedTiKVCrossZoneStr, |_info, record| {
        go_any(record.UnpackedBytesReceivedTiKVCrossZone)
    });
    factories.insert(SumUnpackedBytesSentTiFlashTotalStr, |_info, record| {
        go_any(record.UnpackedBytesSentTiFlashTotal)
    });
    factories.insert(SumUnpackedBytesReceivedTiFlashTotalStr, |_info, record| {
        go_any(record.UnpackedBytesReceivedTiFlashTotal)
    });
    factories.insert(SumUnpackedBytesSentTiFlashCrossZoneStr, |_info, record| {
        go_any(record.UnpackedBytesSentTiFlashCrossZone)
    });
    factories.insert(
        SumUnpackedBytesReceiveTiFlashCrossZoneStr,
        |_info, record| go_any(record.UnpackedBytesReceivedTiFlashCrossZone),
    );
    factories.insert(StorageKVStr, |_info, record| go_any(record.StorageKV));
    factories.insert(StorageMPPStr, |_info, record| go_any(record.StorageMPP));

    factories
});

// makeColumnFactories 对应 Go 函数 makeColumnFactories。
// 参数 columns 来自 model.ColumnInfo 列定义；函数按输入列顺序查找对应列工厂并返回。
pub fn makeColumnFactories(columns: &[model::ColumnInfo]) -> Vec<ColumnFactory> {
    let mut columnFactories = Vec::with_capacity(columns.len());
    for col in columns {
        // Go 代码在漏注册列时 panic，提示“应该永远不会发生”；这里保持同样强失败语义。
        let factory = *columnFactoryMap
            .get(col.Name.O.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "should never happen, should register new column {} into columnValueFactoryMap",
                    col.Name.O
                )
            });
        columnFactories.push(factory);
    }
    columnFactories
}

// Format the backoffType map to a string or nil.
// formatBackoffTypes 对应 Go 函数 formatBackoffTypes。
// 它把 backoff 类型计数 map 转为按次数倒序排列的 "type:count,type:count" 字符串；空 map 返回 nil。
pub fn formatBackoffTypes(backoffMap: &HashMap<String, i32>) -> Option<String> {
    // backoffStat 对应 Go 函数内局部 struct，保持字段含义：类型名和出现次数。
    struct backoffStat {
        backoffType: String,
        count: i32,
    }

    let size = backoffMap.len();
    if size == 0 {
        // Go 返回 nil，让 statement summary 表里空 backoff 类型显示为 NULL。
        return None;
    }

    let mut backoffArray = Vec::with_capacity(backoffMap.len());
    for (backoffType, count) in backoffMap {
        backoffArray.push(backoffStat {
            backoffType: backoffType.clone(),
            count: *count,
        });
    }
    // Go slices.SortFunc 使用 cmp.Compare(j.count, i.count)，即 count 降序。
    backoffArray.sort_by(|i, j| j.count.cmp(&i.count));

    let mut buffer = String::new();
    for (index, stat) in backoffArray.iter().enumerate() {
        // Go fmt.Fprintf 理论上可能返回错误；Rust write! 到 String 通常不会失败，但仍保留 FORMAT ERROR 分支。
        buffer.push_str(&format!("{}:{}", stat.backoffType, stat.count));
        if index < backoffArray.len() - 1 {
            buffer.push(',');
        }
    }
    Some(buffer)
}

// avgInt 对应 Go 函数 avgInt。
// count 为 0 时返回 0，避免除零；这是 statement summary 所有整数平均值列的共同保护。
pub fn avgInt(sum: i64, count: i64) -> i64 {
    if count > 0 {
        return sum / count;
    }
    0
}

// avgFloat 对应 Go 函数 avgFloat。
// sum 是整数累计值，只有输出平均值时才转 float64，保持 Go 的转换位置。
pub fn avgFloat(sum: i64, count: i64) -> f64 {
    if count > 0 {
        return sum as f64 / count as f64;
    }
    0.0
}

/// uint64 累计值的浮点平均，避免 int64 转换溢出。
pub fn avgFloat4Uint(sum: u64, count: i64) -> f64 {
    if count > 0 {
        sum as f64 / count as f64
    } else {
        0.0
    }
}

// avgSumFloat 对应 Go 函数 avgSumFloat。
// sum 已经是 float64，用于 RU、内存仲裁等浮点累计值；同样用 count>0 防除零。
pub fn avgSumFloat(sum: f64, count: i64) -> f64 {
    if count > 0 {
        return sum / count as f64;
    }
    0.0
}

// convertEmptyToNil 对应 Go 函数 convertEmptyToNil。
// 空字符串返回 nil，用于 DIGEST、SCHEMA、TABLE_NAMES 等列显示 NULL；非空字符串按原值返回。
pub fn convertEmptyToNil(str_value: &str) -> Option<String> {
    if str_value.is_empty() {
        return None;
    }
    Some(str_value.to_owned())
}
