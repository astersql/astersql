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

// 慢查询日志（Slow Query Log）格式化、规则解析与字段匹配。
//
// 慢日志记录超过阈值的 SQL 及其耗时、计划、Coprocessor/KV 细节等；
// 本模块提供字段常量、`SlowQueryLogItems`、规则（SlowLogRules）解析，
// 以及按字段 accessor 做阈值匹配的基础设施。
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use crc::{CRC_64_ECMA_182, Crc};
use execdetails::execdetails::{self as ed, CopTasksDetails};
use execdetails::util::{self as execdetails_util, context as exec_context, util as tikv_util};

use super::session::{RewritePhaseInfo, SessionVars};

pub use exec_context::Context as SlowLogExecContext;

/// 慢日志每行前缀（`# `）。
pub const SlowLogRowPrefixStr: &str = "# ";
/// 字段名与值之间的分隔（`: `）。
pub const SlowLogSpaceMarkStr: &str = ": ";
/// SQL 行后缀分号。
pub const SlowLogSQLSuffixStr: &str = ";";
/// 事务开始时间戳字段名。
pub const SlowLogTxnStartTSStr: &str = "Txn_start_ts";
/// Keyspace 名称字段（多租户隔离命名空间）。
pub const SlowLogKeyspaceName: &str = "Keyspace_name";
/// Keyspace 数字 ID 字段。
pub const SlowLogKeyspaceID: &str = "Keyspace_ID";
/// 连接 ID 字段。
pub const SlowLogConnIDStr: &str = "Conn_ID";
/// 会话别名字段。
pub const SlowLogSessAliasStr: &str = "Session_alias";
/// 查询总耗时字段。
pub const SlowLogQueryTimeStr: &str = "Query_time";
/// 解析耗时字段。
pub const SlowLogParseTimeStr: &str = "Parse_time";
/// 编译耗时字段。
pub const SlowLogCompileTimeStr: &str = "Compile_time";
/// SQL 改写耗时字段。
pub const SlowLogRewriteTimeStr: &str = "Rewrite_time";
/// 优化耗时字段。
pub const SlowLogOptimizeTimeStr: &str = "Optimize_time";
/// 等待时间戳耗时字段。
pub const SlowLogWaitTSTimeStr: &str = "Wait_TS";
/// 当前库名字段。
pub const SlowLogDBStr: &str = "DB";
/// 使用的索引名称字段。
pub const SlowLogIndexNamesStr: &str = "Index_names";
/// 是否内部 SQL 字段。
pub const SlowLogIsInternalStr: &str = "Is_internal";
/// SQL digest（归一化指纹）字段。
pub const SlowLogDigestStr: &str = "Digest";
/// Coprocessor 任务数字段。
pub const SlowLogNumCopTasksStr: &str = "Num_cop_tasks";
/// 峰值内存字段。
pub const SlowLogMemMax: &str = "Mem_max";
/// 内存仲裁相关指标字段。
pub const SlowLogMemArbitration: &str = "Mem_arbitration";
/// 峰值磁盘使用字段。
pub const SlowLogDiskMax: &str = "Disk_max";
/// KV 总等待耗时字段。
pub const SlowLogKVTotal: &str = "KV_total";
/// PD 总等待耗时字段。
pub const SlowLogPDTotal: &str = "PD_total";
/// 发往 TiKV 的解包字节总量字段。
pub const SlowLogUnpackedBytesSentTiKVTotal: &str = "Unpacked_bytes_sent_tikv_total";
/// 自 TiKV 接收的解包字节总量字段。
pub const SlowLogUnpackedBytesReceivedTiKVTotal: &str = "Unpacked_bytes_received_tikv_total";
/// 跨可用区发往 TiKV 的解包字节字段。
pub const SlowLogUnpackedBytesSentTiKVCrossZone: &str = "Unpacked_bytes_sent_tikv_cross_zone";
/// 跨可用区自 TiKV 接收的解包字节字段。
pub const SlowLogUnpackedBytesReceivedTiKVCrossZone: &str =
    "Unpacked_bytes_received_tikv_cross_zone";
/// 发往 TiFlash 的解包字节总量字段。
pub const SlowLogUnpackedBytesSentTiFlashTotal: &str = "Unpacked_bytes_sent_tiflash_total";
/// 自 TiFlash 接收的解包字节总量字段。
pub const SlowLogUnpackedBytesReceivedTiFlashTotal: &str = "Unpacked_bytes_received_tiflash_total";
/// 跨可用区发往 TiFlash 的解包字节字段。
pub const SlowLogUnpackedBytesSentTiFlashCrossZone: &str = "Unpacked_bytes_sent_tiflash_cross_zone";
/// 跨可用区自 TiFlash 接收的解包字节字段。
pub const SlowLogUnpackedBytesReceivedTiFlashCrossZone: &str =
    "Unpacked_bytes_received_tiflash_cross_zone";
/// 写 SQL 响应总耗时字段。
pub const SlowLogWriteSQLRespTotal: &str = "Write_sql_response_total";
/// 执行是否成功字段。
pub const SlowLogSucc: &str = "Succ";
/// 执行重试次数字段。
pub const SlowLogExecRetryCount: &str = "Exec_retry_count";
/// 资源组名称字段。
pub const SlowLogResourceGroup: &str = "Resource_group";
/// Coprocessor MVCC 读放大字段（TotalKeys/ProcessedKeys）。
pub const SlowLogCopMVCCReadAmplification: &str = "cop_mvcc_read_amplification";
/// 是否 prepared 语句字段。
pub const SlowLogPrepared: &str = "Prepared";
/// 计划是否来自缓存字段。
pub const SlowLogPlanFromCache: &str = "Plan_from_cache";
/// 计划是否来自绑定字段。
pub const SlowLogPlanFromBinding: &str = "Plan_from_binding";
/// 是否还有更多结果字段。
pub const SlowLogHasMoreResults: &str = "Has_more_results";
/// 前一条语句字段。
pub const SlowLogPrevStmt: &str = "Prev_stmt";
/// 执行计划文本字段。
pub const SlowLogPlan: &str = "Plan";
/// 计划 digest 字段。
pub const SlowLogPlanDigest: &str = "Plan_digest";
/// 二进制计划字段。
pub const SlowLogBinaryPlan: &str = "Binary_plan";
/// 结果行数字段。
pub const SlowLogResultRows: &str = "Result_rows";
/// 是否显式事务字段。
pub const SlowLogIsExplicitTxn: &str = "IsExplicitTxn";
/// 是否写缓存表字段。
pub const SlowLogIsWriteCacheTable: &str = "IsWriteCacheTable";
/// 同步统计是否失败字段。
pub const SlowLogIsSyncStatsFailed: &str = "IsSyncStatsFailed";
/// 是否从 KV 存储读字段。
pub const SlowLogStorageFromKV: &str = "Storage_from_kv";
/// 是否从 MPP 存储读字段。
pub const SlowLogStorageFromMPP: &str = "Storage_from_mpp";
/// 规则未指定连接 ID 时的占位值。
pub const UnsetConnID: i64 = -1;

/// 慢日志中序列化的 SQL 警告条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JSONSQLWarnForSlowLog {
    pub Level: String,
    pub Message: String,
    pub IsExtra: bool,
}

/// 一条慢查询日志所需的全部可格式化字段集合。
#[derive(Default)]
pub struct SlowQueryLogItems {
    pub TxnTS: u64,
    pub KeyspaceName: String,
    pub KeyspaceID: u32,
    pub SQL: String,
    pub Digest: String,
    pub TimeTotal: Duration,
    pub IndexNames: String,
    pub Succ: bool,
    pub IsExplicitTxn: bool,
    pub IsWriteCacheTable: bool,
    pub IsSyncStatsFailed: bool,
    pub Prepared: bool,
    pub PlanFromCache: bool,
    pub PlanFromBinding: bool,
    pub HasMoreResults: bool,
    pub PrevStmt: String,
    pub Plan: String,
    pub PlanDigest: String,
    pub BinaryPlan: String,
    pub CopTasks: Option<Box<CopTasksDetails>>,
    pub RewriteInfo: RewritePhaseInfo,
    pub WriteSQLRespTotal: Duration,
    pub KVExecDetail: Option<Box<tikv_util::ExecDetails>>,
    pub ExecDetail: Option<Box<ed::ExecDetails>>,
    pub ExecRetryCount: u64,
    pub ExecRetryTime: Duration,
    pub ResultRows: i64,
    pub Warnings: Vec<JSONSQLWarnForSlowLog>,
    pub ResourceGroupName: String,
    pub MemMax: i64,
    pub DiskMax: i64,
    pub StorageKV: bool,
    pub StorageMPP: bool,
    pub MemArbitration: f64,
    pub SessionConnectAttrs: BTreeMap<String, String>,
}

/// 将 Duration 格式化为秒数字符串（整数秒无小数点）。
fn duration_seconds(value: Duration) -> String {
    let seconds = value.as_secs_f64();
    if seconds.fract() == 0.0 {
        format!("{seconds:.0}")
    } else {
        seconds.to_string()
    }
}

/// 按慢日志行格式写入一个 `key: value` 字段。
pub fn writeSlowLogItem(buffer: &mut String, key: &str, value: impl std::fmt::Display) {
    let _ = writeln!(
        buffer,
        "{SlowLogRowPrefixStr}{key}{SlowLogSpaceMarkStr}{value}"
    );
}

impl SessionVars {
    /// 按与 Go 相同的字段顺序格式化本文件拥有的慢日志字段。
    /// Formats the fields owned by this file in the same order as Go. Details
    /// supplied by executor-specific structures are formatted by their modules.
    pub fn SlowLogFormat(&self, items: &SlowQueryLogItems) -> String {
        // 按 Go 侧字段顺序写出；空字段多数跳过，SQL 追加在末尾。
        let mut buffer = String::new();
        writeSlowLogItem(&mut buffer, SlowLogTxnStartTSStr, items.TxnTS);
        if !items.KeyspaceName.is_empty() {
            writeSlowLogItem(&mut buffer, SlowLogKeyspaceName, &items.KeyspaceName);
            writeSlowLogItem(&mut buffer, SlowLogKeyspaceID, items.KeyspaceID);
        }
        if self.ConnectionID != 0 {
            writeSlowLogItem(&mut buffer, SlowLogConnIDStr, self.ConnectionID);
        }
        if items.ExecRetryCount > 0 {
            let _ = writeln!(
                buffer,
                "# Exec_retry_time: {} Exec_retry_count: {}",
                duration_seconds(items.ExecRetryTime),
                items.ExecRetryCount
            );
        }
        writeSlowLogItem(
            &mut buffer,
            SlowLogQueryTimeStr,
            duration_seconds(items.TimeTotal),
        );
        let ia_stats = ed::GetIARemoteReadSegmentStats(
            items
                .ExecDetail
                .as_deref()
                .and_then(|details| details.CopExecDetails.ScanDetail.as_ref()),
        );
        if ia_stats.Count > 0 {
            writeSlowLogItem(&mut buffer, ed::IARemoteReadSegmentCountStr, ia_stats.Count);
        }
        if ia_stats.Bytes > 0 {
            writeSlowLogItem(&mut buffer, ed::IARemoteReadSegmentSizeStr, ia_stats.Bytes);
        }
        if ia_stats.WaitTime > Duration::ZERO {
            writeSlowLogItem(
                &mut buffer,
                ed::IARemoteReadSegmentWaitTimeStr,
                duration_seconds(ia_stats.WaitTime),
            );
        }
        if let Some(pool) = items
            .ExecDetail
            .as_deref()
            .and_then(|details| details.ReadPoolTaskDetails.as_ref())
            .filter(|pool| !pool.Empty())
        {
            writeSlowLogItem(&mut buffer, ed::ReadPoolTaskDetailsStr, pool.String());
        }
        if !self.CurrentDB().is_empty() {
            writeSlowLogItem(
                &mut buffer,
                SlowLogDBStr,
                self.CurrentDB().to_ascii_lowercase(),
            );
        }
        if !items.IndexNames.is_empty() {
            writeSlowLogItem(&mut buffer, SlowLogIndexNamesStr, &items.IndexNames);
        }
        writeSlowLogItem(&mut buffer, SlowLogIsInternalStr, self.InRestrictedSQL);
        if !items.Digest.is_empty() {
            writeSlowLogItem(&mut buffer, SlowLogDigestStr, &items.Digest);
        }
        if items.MemMax > 0 {
            writeSlowLogItem(&mut buffer, SlowLogMemMax, items.MemMax);
        }
        if items.MemArbitration > 0.0 {
            writeSlowLogItem(&mut buffer, SlowLogMemArbitration, items.MemArbitration);
        }
        if items.DiskMax > 0 {
            writeSlowLogItem(&mut buffer, SlowLogDiskMax, items.DiskMax);
        }
        writeSlowLogItem(&mut buffer, SlowLogPrepared, items.Prepared);
        writeSlowLogItem(&mut buffer, SlowLogPlanFromCache, items.PlanFromCache);
        writeSlowLogItem(&mut buffer, SlowLogPlanFromBinding, items.PlanFromBinding);
        writeSlowLogItem(&mut buffer, SlowLogHasMoreResults, items.HasMoreResults);
        writeSlowLogItem(
            &mut buffer,
            SlowLogWriteSQLRespTotal,
            duration_seconds(items.WriteSQLRespTotal),
        );
        writeSlowLogItem(&mut buffer, SlowLogResultRows, items.ResultRows);
        writeSlowLogItem(&mut buffer, SlowLogSucc, items.Succ);
        writeSlowLogItem(&mut buffer, SlowLogIsExplicitTxn, items.IsExplicitTxn);
        writeSlowLogItem(
            &mut buffer,
            SlowLogIsSyncStatsFailed,
            items.IsSyncStatsFailed,
        );
        if items.IsWriteCacheTable {
            writeSlowLogItem(&mut buffer, SlowLogIsWriteCacheTable, true);
        }
        if !items.Plan.is_empty() {
            writeSlowLogItem(&mut buffer, SlowLogPlan, &items.Plan);
        }
        if !items.PlanDigest.is_empty() {
            writeSlowLogItem(&mut buffer, SlowLogPlanDigest, &items.PlanDigest);
        }
        if !items.BinaryPlan.is_empty() {
            writeSlowLogItem(&mut buffer, SlowLogBinaryPlan, &items.BinaryPlan);
        }
        if !items.ResourceGroupName.is_empty() {
            writeSlowLogItem(&mut buffer, SlowLogResourceGroup, &items.ResourceGroupName);
        }
        writeSlowLogItem(&mut buffer, SlowLogStorageFromKV, items.StorageKV);
        writeSlowLogItem(&mut buffer, SlowLogStorageFromMPP, items.StorageMPP);
        if !items.PrevStmt.is_empty() {
            writeSlowLogItem(&mut buffer, SlowLogPrevStmt, &items.PrevStmt);
        }
        buffer.push_str(&items.SQL);
        if !items.SQL.ends_with(';') {
            buffer.push(';');
        }
        buffer
    }
}

/// 慢日志规则条件的阈值：字符串/整型/浮点/布尔等。
#[derive(Clone, Debug, PartialEq)]
pub enum Threshold {
    String(String),
    Int(i64),
    UInt(u64),
    Float(f64),
    Bool(bool),
}

impl std::fmt::Display for Threshold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(v) => f.write_str(v),
            Self::Int(v) => v.fmt(f),
            Self::UInt(v) => v.fmt(f),
            Self::Float(v) => v.fmt(f),
            Self::Bool(v) => v.fmt(f),
        }
    }
}

/// 单条规则条件：字段名 + 阈值。
#[derive(Clone, Debug, PartialEq)]
pub struct SlowLogCondition {
    pub field: String,
    pub threshold: Threshold,
}

/// 一条慢日志规则，由多个 AND 条件组成。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlowLogRule {
    pub conditions: Vec<SlowLogCondition>,
}

impl SlowLogRule {
    /// 查找指定字段的阈值。
    pub fn threshold(&self, field: &str) -> Option<&Threshold> {
        self.conditions
            .iter()
            .find(|condition| condition.field == field)
            .map(|condition| &condition.threshold)
    }
}

/// 一组规则及其涉及字段集合、原始规则串。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlowLogRules {
    pub fields: BTreeSet<String>,
    pub rules: Vec<SlowLogRule>,
    pub raw_rules: String,
}

/// 全局慢日志规则：按 ConnID 映射，并带原始串与 CRC 哈希。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlobalSlowLogRules {
    pub raw_rules: String,
    pub raw_rules_hash: u64,
    pub rules_map: BTreeMap<i64, SlowLogRules>,
}

/// 会话侧慢日志规则视图，含有效字段与全局哈希缓存。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionSlowLogRules {
    pub SlowLogRules: Option<SlowLogRules>,
    pub EffectiveFields: BTreeSet<String>,
    pub GlobalRawRulesHash: u64,
    pub NeedUpdateEffectiveFields: bool,
}

/// 构造会话慢日志规则包装，默认需要刷新有效字段。
pub fn NewSessionSlowLogRules(rules: Option<SlowLogRules>) -> SessionSlowLogRules {
    SessionSlowLogRules {
        SlowLogRules: rules,
        EffectiveFields: BTreeSet::new(),
        GlobalRawRulesHash: 0,
        NeedUpdateEffectiveFields: true,
    }
}

/// 慢日志字段访问器：解析阈值、可选 Setter、以及 Match 判定。
/// SlowLogFieldAccessor defines how to get or set a specific field in SlowQueryLogItems.
#[derive(Clone)]
pub struct SlowLogFieldAccessor {
    pub Parse: fn(&str) -> Result<Threshold, String>,
    pub Setter: Option<
        Arc<
            dyn Fn(&SlowLogExecContext, Option<&SessionVars>, &mut SlowQueryLogItems) + Send + Sync,
        >,
    >,
    pub Match:
        Arc<dyn Fn(Option<&SessionVars>, &SlowQueryLogItems, &Threshold) -> bool + Send + Sync>,
}

/// 基于 `ExecDetails` 构造字段 accessor（惰性填充 ExecDetail）。
fn make_exec_detail_accessor(
    parse: fn(&str) -> Result<Threshold, String>,
    match_fn: fn(&ed::ExecDetails, &Threshold) -> bool,
) -> SlowLogFieldAccessor {
    SlowLogFieldAccessor {
        Parse: parse,
        Setter: Some(Arc::new(|_ctx, se_vars, items| {
            if items.ExecDetail.is_none() {
                if let Some(se_vars) = se_vars {
                    items.ExecDetail = Some(Box::new(se_vars.StmtCtx.GetExecDetails()));
                }
            }
        })),
        Match: Arc::new(
            move |_se_vars, items, threshold| match items.ExecDetail.as_deref() {
                None => matchZero(threshold),
                Some(detail) => match_fn(detail, threshold),
            },
        ),
    }
}

/// 基于 TiKV `ExecDetails` 构造字段 accessor。
fn make_kv_exec_detail_accessor(
    parse: fn(&str) -> Result<Threshold, String>,
    match_fn: fn(&tikv_util::ExecDetails, &Threshold) -> bool,
) -> SlowLogFieldAccessor {
    SlowLogFieldAccessor {
        Parse: parse,
        Setter: Some(Arc::new(|ctx, _se_vars, items| {
            if items.KVExecDetail.is_none() {
                if let Some(raw) =
                    ctx.value::<Arc<tikv_util::ExecDetails>, _>(&tikv_util::ExecDetailsKey)
                {
                    items.KVExecDetail = Some(Box::new(execdetails_util::LoadTiKVExecDetails(
                        Some(raw.as_ref()),
                    )));
                }
            }
        })),
        Match: Arc::new(
            move |_se_vars, items, threshold| match items.KVExecDetail.as_deref() {
                None => matchZero(threshold),
                Some(detail) => match_fn(detail, threshold),
            },
        ),
    }
}

/// 构建全部可匹配字段名（小写）到 accessor 的映射表。
fn build_slow_log_rule_field_accessors() -> BTreeMap<String, SlowLogFieldAccessor> {
    use std::sync::atomic::Ordering;

    // 字段名统一转小写后插入，保证规则匹配时大小写不敏感。
    let mut map = BTreeMap::new();
    let insert = |map: &mut BTreeMap<String, SlowLogFieldAccessor>,
                  key: &str,
                  accessor: SlowLogFieldAccessor| {
        map.insert(key.to_ascii_lowercase(), accessor);
    };

    insert(
        &mut map,
        SlowLogConnIDStr,
        SlowLogFieldAccessor {
            Parse: parseUint64,
            Setter: None,
            Match: Arc::new(|se_vars, _items, threshold| {
                se_vars.is_some_and(|v| matchGE_u64(threshold, v.ConnectionID))
            }),
        },
    );
    insert(
        &mut map,
        SlowLogSessAliasStr,
        SlowLogFieldAccessor {
            Parse: ParseString,
            Setter: None,
            Match: Arc::new(|se_vars, _items, threshold| {
                se_vars.is_some_and(|v| MatchEqual(threshold, &v.SessionAlias))
            }),
        },
    );
    insert(
        &mut map,
        SlowLogDBStr,
        SlowLogFieldAccessor {
            Parse: ParseString,
            Setter: None,
            Match: Arc::new(|se_vars, _items, threshold| {
                let Some(se_vars) = se_vars else {
                    return false;
                };
                match threshold {
                    Threshold::String(expected) => {
                        expected.to_ascii_lowercase() == se_vars.CurrentDB().to_ascii_lowercase()
                    }
                    _ => false,
                }
            }),
        },
    );
    insert(
        &mut map,
        SlowLogExecRetryCount,
        SlowLogFieldAccessor {
            Parse: parseUint64,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(se_vars) = se_vars {
                    items.ExecRetryCount = se_vars.StmtCtx.ExecRetryCountValue();
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| {
                matchGE_u64(threshold, items.ExecRetryCount)
            }),
        },
    );
    insert(
        &mut map,
        SlowLogQueryTimeStr,
        SlowLogFieldAccessor {
            Parse: parseFloat64,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(se_vars) = se_vars {
                    items.TimeTotal = se_vars.GetTotalCostDuration();
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| {
                matchGE(threshold, items.TimeTotal.as_secs_f64())
            }),
        },
    );
    insert(
        &mut map,
        SlowLogParseTimeStr,
        SlowLogFieldAccessor {
            Parse: parseFloat64,
            Setter: None,
            Match: Arc::new(|se_vars, _items, threshold| {
                se_vars.is_some_and(|v| matchGE(threshold, v.DurationParseValue().as_secs_f64()))
            }),
        },
    );
    insert(
        &mut map,
        SlowLogCompileTimeStr,
        SlowLogFieldAccessor {
            Parse: parseFloat64,
            Setter: None,
            Match: Arc::new(|se_vars, _items, threshold| {
                se_vars.is_some_and(|v| matchGE(threshold, v.DurationCompile.as_secs_f64()))
            }),
        },
    );
    insert(
        &mut map,
        SlowLogRewriteTimeStr,
        SlowLogFieldAccessor {
            Parse: parseFloat64,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(se_vars) = se_vars {
                    items.RewriteInfo = se_vars.SnapshotRewritePhaseInfo();
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| {
                matchGE(threshold, items.RewriteInfo.DurationRewrite.as_secs_f64())
            }),
        },
    );
    insert(
        &mut map,
        SlowLogOptimizeTimeStr,
        SlowLogFieldAccessor {
            Parse: parseFloat64,
            Setter: None,
            Match: Arc::new(|se_vars, _items, threshold| {
                se_vars.is_some_and(|v| matchGE(threshold, v.DurationOptimizer.Total.as_secs_f64()))
            }),
        },
    );
    insert(
        &mut map,
        SlowLogWaitTSTimeStr,
        SlowLogFieldAccessor {
            Parse: parseFloat64,
            Setter: None,
            Match: Arc::new(|se_vars, _items, threshold| {
                se_vars.is_some_and(|v| {
                    matchGE(
                        threshold,
                        v.DurationWaitTS
                            .lock()
                            .expect("wait TS lock poisoned")
                            .as_secs_f64(),
                    )
                })
            }),
        },
    );
    insert(
        &mut map,
        SlowLogIsInternalStr,
        SlowLogFieldAccessor {
            Parse: parseBool,
            Setter: None,
            Match: Arc::new(|se_vars, _items, threshold| {
                se_vars.is_some_and(|v| MatchEqualBool(threshold, v.InRestrictedSQL))
            }),
        },
    );
    insert(
        &mut map,
        SlowLogDigestStr,
        SlowLogFieldAccessor {
            Parse: ParseString,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(se_vars) = se_vars {
                    let (_, digest) = se_vars.StmtCtx.SQLDigest();
                    items.Digest = digest.String().to_owned();
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| MatchEqual(threshold, &items.Digest)),
        },
    );
    insert(
        &mut map,
        SlowLogNumCopTasksStr,
        SlowLogFieldAccessor {
            Parse: parseInt64,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(se_vars) = se_vars {
                    if let Some(details) = se_vars.StmtCtx.SyncExecDetails.CopTasksDetails() {
                        items.CopTasks = Some(Box::new(details));
                    }
                }
            })),
            Match: Arc::new(
                |_se_vars, items, threshold| match items.CopTasks.as_deref() {
                    None => matchZero(threshold),
                    Some(tasks) => matchGE_i64(threshold, tasks.NumCopTasks as i64),
                },
            ),
        },
    );
    insert(
        &mut map,
        SlowLogMemMax,
        SlowLogFieldAccessor {
            Parse: parseInt64,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(tracker) = se_vars.and_then(|vars| vars.StmtCtx.MemTracker.as_ref()) {
                    items.MemMax = tracker.MaxConsumed();
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| matchGE_i64(threshold, items.MemMax)),
        },
    );
    insert(
        &mut map,
        SlowLogDiskMax,
        SlowLogFieldAccessor {
            Parse: parseInt64,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(tracker) = se_vars.and_then(|vars| vars.StmtCtx.DiskTracker.as_ref()) {
                    items.DiskMax = tracker.MaxConsumed();
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| matchGE_i64(threshold, items.DiskMax)),
        },
    );
    insert(
        &mut map,
        SlowLogWriteSQLRespTotal,
        SlowLogFieldAccessor {
            Parse: parseFloat64,
            Setter: Some(Arc::new(|ctx, _se_vars, items| {
                if let Some(stmt) = ctx.value::<Arc<execdetails_util::StmtExecDetails>, _>(
                    &execdetails_util::StmtExecDetailKey,
                ) {
                    items.WriteSQLRespTotal = stmt.WriteSQLRespDuration;
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| {
                matchGE(threshold, items.WriteSQLRespTotal.as_secs_f64())
            }),
        },
    );
    insert(
        &mut map,
        SlowLogSucc,
        SlowLogFieldAccessor {
            Parse: parseBool,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(se_vars) = se_vars {
                    items.Succ = se_vars.StmtCtx.ExecSuccess;
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| MatchEqualBool(threshold, items.Succ)),
        },
    );
    insert(
        &mut map,
        SlowLogResourceGroup,
        SlowLogFieldAccessor {
            Parse: ParseString,
            Setter: Some(Arc::new(|_ctx, se_vars, items| {
                if let Some(se_vars) = se_vars {
                    items.ResourceGroupName = se_vars.StmtCtx.ResourceGroupName.clone();
                }
            })),
            Match: Arc::new(|_se_vars, items, threshold| match threshold {
                Threshold::String(expected) => {
                    expected.to_ascii_lowercase() == items.ResourceGroupName.to_ascii_lowercase()
                }
                _ => false,
            }),
        },
    );
    insert(
        &mut map,
        SlowLogKVTotal,
        make_kv_exec_detail_accessor(parseFloat64, |d, threshold| {
            matchGE(
                threshold,
                Duration::from_nanos(d.WaitKVRespDuration.load(Ordering::Relaxed) as u64)
                    .as_secs_f64(),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogPDTotal,
        make_kv_exec_detail_accessor(parseFloat64, |d, threshold| {
            matchGE(
                threshold,
                Duration::from_nanos(d.WaitPDRespDuration.load(Ordering::Relaxed) as u64)
                    .as_secs_f64(),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogUnpackedBytesSentTiKVTotal,
        make_kv_exec_detail_accessor(parseInt64, |d, threshold| {
            matchGE_i64(
                threshold,
                d.TrafficDetails
                    .UnpackedBytesSentKVTotal
                    .load(Ordering::Relaxed),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogUnpackedBytesReceivedTiKVTotal,
        make_kv_exec_detail_accessor(parseInt64, |d, threshold| {
            matchGE_i64(
                threshold,
                d.TrafficDetails
                    .UnpackedBytesReceivedKVTotal
                    .load(Ordering::Relaxed),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogUnpackedBytesSentTiKVCrossZone,
        make_kv_exec_detail_accessor(parseInt64, |d, threshold| {
            matchGE_i64(
                threshold,
                d.TrafficDetails
                    .UnpackedBytesSentKVCrossZone
                    .load(Ordering::Relaxed),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogUnpackedBytesReceivedTiKVCrossZone,
        make_kv_exec_detail_accessor(parseInt64, |d, threshold| {
            matchGE_i64(
                threshold,
                d.TrafficDetails
                    .UnpackedBytesReceivedKVCrossZone
                    .load(Ordering::Relaxed),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogUnpackedBytesSentTiFlashTotal,
        make_kv_exec_detail_accessor(parseInt64, |d, threshold| {
            matchGE_i64(
                threshold,
                d.TrafficDetails
                    .UnpackedBytesSentMPPTotal
                    .load(Ordering::Relaxed),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogUnpackedBytesReceivedTiFlashTotal,
        make_kv_exec_detail_accessor(parseInt64, |d, threshold| {
            matchGE_i64(
                threshold,
                d.TrafficDetails
                    .UnpackedBytesReceivedMPPTotal
                    .load(Ordering::Relaxed),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogUnpackedBytesSentTiFlashCrossZone,
        make_kv_exec_detail_accessor(parseInt64, |d, threshold| {
            matchGE_i64(
                threshold,
                d.TrafficDetails
                    .UnpackedBytesSentMPPCrossZone
                    .load(Ordering::Relaxed),
            )
        }),
    );
    insert(
        &mut map,
        SlowLogUnpackedBytesReceivedTiFlashCrossZone,
        make_kv_exec_detail_accessor(parseInt64, |d, threshold| {
            matchGE_i64(
                threshold,
                d.TrafficDetails
                    .UnpackedBytesReceivedMPPCrossZone
                    .load(Ordering::Relaxed),
            )
        }),
    );
    insert(
        &mut map,
        ed::ProcessTimeStr,
        make_exec_detail_accessor(parseFloat64, |d, threshold| {
            matchGE(
                threshold,
                d.CopExecDetails.TimeDetail.ProcessTime.as_secs_f64(),
            )
        }),
    );
    insert(
        &mut map,
        ed::BackoffTimeStr,
        make_exec_detail_accessor(parseFloat64, |d, threshold| {
            matchGE(threshold, d.CopExecDetails.BackoffTime.as_secs_f64())
        }),
    );
    insert(
        &mut map,
        ed::TotalKeysStr,
        make_exec_detail_accessor(parseUint64, |d, threshold| {
            let Some(scan) = d.CopExecDetails.ScanDetail.as_ref() else {
                return matchZero(threshold);
            };
            let (total_keys, ok) = uint64FromNonNegative(scan.TotalKeys);
            ok && matchGE_u64(threshold, total_keys)
        }),
    );
    insert(
        &mut map,
        ed::ProcessKeysStr,
        make_exec_detail_accessor(parseUint64, |d, threshold| {
            let Some(scan) = d.CopExecDetails.ScanDetail.as_ref() else {
                return matchZero(threshold);
            };
            let (processed_keys, ok) = uint64FromNonNegative(scan.ProcessedKeys);
            ok && matchGE_u64(threshold, processed_keys)
        }),
    );
    insert(
        &mut map,
        SlowLogCopMVCCReadAmplification,
        make_exec_detail_accessor(parseFloat64, |d, threshold| {
            let Some(scan) = d.CopExecDetails.ScanDetail.as_ref() else {
                return matchZero(threshold);
            };
            if scan.ProcessedKeys <= 0 {
                return matchZero(threshold);
            }
            matchGE(threshold, scan.TotalKeys as f64 / scan.ProcessedKeys as f64)
        }),
    );
    insert(
        &mut map,
        ed::PreWriteTimeStr,
        make_exec_detail_accessor(parseFloat64, |d, threshold| {
            let Some(commit) = d.CommitDetail.as_ref() else {
                return matchZero(threshold);
            };
            matchGE(threshold, commit.PrewriteTime.as_secs_f64())
        }),
    );
    insert(
        &mut map,
        ed::CommitTimeStr,
        make_exec_detail_accessor(parseFloat64, |d, threshold| {
            let Some(commit) = d.CommitDetail.as_ref() else {
                return matchZero(threshold);
            };
            matchGE(threshold, commit.CommitTime.as_secs_f64())
        }),
    );
    insert(
        &mut map,
        ed::WriteKeysStr,
        make_exec_detail_accessor(parseUint64, |d, threshold| {
            let Some(commit) = d.CommitDetail.as_ref() else {
                return matchZero(threshold);
            };
            let (write_keys, ok) = uint64FromNonNegative(i64::from(commit.WriteKeys));
            ok && matchGE_u64(threshold, write_keys)
        }),
    );
    insert(
        &mut map,
        ed::WriteSizeStr,
        make_exec_detail_accessor(parseUint64, |d, threshold| {
            let Some(commit) = d.CommitDetail.as_ref() else {
                return matchZero(threshold);
            };
            let (write_size, ok) = uint64FromNonNegative(i64::from(commit.WriteSize));
            ok && matchGE_u64(threshold, write_size)
        }),
    );
    insert(
        &mut map,
        ed::PrewriteRegionStr,
        make_exec_detail_accessor(parseUint64, |d, threshold| {
            let Some(commit) = d.CommitDetail.as_ref() else {
                return matchZero(threshold);
            };
            let prewrite_region_num = commit.PrewriteRegionNum.load(Ordering::Relaxed);
            let (prewrite_region, ok) = uint64FromNonNegative(i64::from(prewrite_region_num));
            ok && matchGE_u64(threshold, prewrite_region)
        }),
    );
    if let Some(factory) = PLAN_DIGEST_ACCESSOR.get() {
        insert(&mut map, SlowLogPlanDigest, factory());
    }
    map
}

// The executor supplies its accessor during package initialization, as in Go.
static PLAN_DIGEST_ACCESSOR: std::sync::OnceLock<fn() -> SlowLogFieldAccessor> =
    std::sync::OnceLock::new();

/// Called by executor package initialization before any slow-log rule is parsed.
pub fn RegisterPlanDigestAccessor(factory: fn() -> SlowLogFieldAccessor) {
    let _ = PLAN_DIGEST_ACCESSOR.set(factory);
}

use std::sync::LazyLock;

/// 评估慢日志规则时使用的字段 accessor 全局表。
/// SlowLogRuleFieldAccessors defines the set of field accessors for SlowQueryLogItems
/// that are relevant to evaluating and triggering SlowLogRules.
pub static SlowLogRuleFieldAccessors: LazyLock<BTreeMap<String, SlowLogFieldAccessor>> =
    LazyLock::new(build_slow_log_rule_field_accessors);

/// 解析无符号整数阈值。
pub fn parseUint64(value: &str) -> Result<Threshold, String> {
    value
        .parse::<u64>()
        .map(Threshold::UInt)
        .map_err(|error| error.to_string())
}

/// 解析非负有符号整数阈值。
pub fn parseInt64(value: &str) -> Result<Threshold, String> {
    let value = value.parse::<i64>().map_err(|error| error.to_string())?;
    if value < 0 {
        Err(format!("threshold value must be non-negative, got {value}"))
    } else {
        Ok(Threshold::Int(value))
    }
}

/// 解析非负有限浮点阈值。
pub fn parseFloat64(value: &str) -> Result<Threshold, String> {
    let value = value.parse::<f64>().map_err(|error| error.to_string())?;
    if !value.is_finite() {
        Err(format!("threshold value must be finite, got {value}"))
    } else if value < 0.0 {
        Err(format!("threshold value must be non-negative, got {value}"))
    } else {
        Ok(Threshold::Float(value))
    }
}

/// 解析布尔阈值。
pub fn parseBool(value: &str) -> Result<Threshold, String> {
    parse_bool(value).map(Threshold::Bool)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 字段值的逻辑类型分类。
enum FieldKind {
    String,
    Int,
    UInt,
    Float,
    Bool,
}

/// 按字段名返回其逻辑类型；未知字段返回 `None`。
fn field_kind(field: &str) -> Option<FieldKind> {
    Some(match field {
        "conn_id" | "exec_retry_count" | "total_keys" | "processed_keys" | "write_keys"
        | "write_size" | "prewrite_region" => FieldKind::UInt,
        "num_cop_tasks"
        | "mem_max"
        | "disk_max"
        | "unpacked_bytes_sent_tikv_total"
        | "unpacked_bytes_received_tikv_total"
        | "unpacked_bytes_sent_tikv_cross_zone"
        | "unpacked_bytes_received_tikv_cross_zone"
        | "unpacked_bytes_sent_tiflash_total"
        | "unpacked_bytes_received_tiflash_total"
        | "unpacked_bytes_sent_tiflash_cross_zone"
        | "unpacked_bytes_received_tiflash_cross_zone" => FieldKind::Int,
        "query_time"
        | "parse_time"
        | "compile_time"
        | "rewrite_time"
        | "optimize_time"
        | "wait_ts"
        | "write_sql_response_total"
        | "kv_total"
        | "pd_total"
        | "process_time"
        | "backoff_time"
        | "cop_mvcc_read_amplification"
        | "prewrite_time"
        | "commit_time" => FieldKind::Float,
        "is_internal" | "succ" => FieldKind::Bool,
        "session_alias" | "db" | "digest" | "resource_group" => FieldKind::String,
        _ => return None,
    })
}

/// 将字符串包装为字符串阈值。
pub fn ParseString(value: &str) -> Result<Threshold, String> {
    Ok(Threshold::String(value.to_owned()))
}

/// 解析常见真/假字面量（1/0、t/f、true/false 等）。
fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!("invalid syntax parsing bool: {value}")),
    }
}

/// 按字段名查找 accessor 并解析阈值字符串。
pub fn ParseSlowLogFieldValue(field_name: &str, value: &str) -> Result<Threshold, String> {
    let field = field_name.to_ascii_lowercase();
    let accessor = SlowLogRuleFieldAccessors
        .get(&field)
        .ok_or_else(|| format!("unknown slow log field name:{field_name}"))?;
    (accessor.Parse)(value)
}

/// 解析单条规则条目（逗号分隔 `field:value`），可选允许 ConnID。
fn parse_slow_log_rule_entry(
    raw_rule: &str,
    allow_conn_id: bool,
) -> Result<(i64, Option<SlowLogRule>), String> {
    let raw_rule = raw_rule.trim();
    if raw_rule.is_empty() {
        return Ok((UnsetConnID, None));
    }
    // Go uses `\s*(\w+)\s*:\s*([^,]+)\s*` with FindAllStringSubmatch. Search
    // each comma-delimited fragment for the trailing ASCII word before the
    // first colon instead of requiring the field to start the fragment.
    let mut values = BTreeMap::<String, Threshold>::new();
    let mut found = false;
    let mut conn_id = UnsetConnID;
    for raw_field in raw_rule.split(',') {
        let raw_field = raw_field.trim();
        if raw_field.is_empty() {
            continue;
        }
        let Some((field_prefix, value)) = raw_field.split_once(':') else {
            continue;
        };
        let field_prefix = field_prefix.trim_end();
        let field_start = field_prefix
            .char_indices()
            .rev()
            .take_while(|(_, ch)| *ch == '_' || ch.is_ascii_alphanumeric())
            .last()
            .map(|(index, _)| index);
        let Some(field_start) = field_start else {
            continue;
        };
        let field = &field_prefix[field_start..];
        found = true;
        let field = field.to_ascii_lowercase();
        let value = value.trim();
        let value = value.trim_matches(['\'', '"']);
        let threshold = ParseSlowLogFieldValue(&field, value)
            .map_err(|error| format!("invalid slow log format, value:{value}, err:{error}"))?;
        if field == "conn_id" {
            if !allow_conn_id {
                return Err(format!("do not allow ConnID value:{value}"));
            }
            conn_id = match threshold {
                Threshold::UInt(value) => value as i64,
                _ => unreachable!(),
            };
        }
        values.insert(field, threshold);
    }
    if !found {
        return Err(format!("invalid slow log rule format:{raw_rule}"));
    }
    Ok((
        conn_id,
        Some(SlowLogRule {
            conditions: values
                .into_iter()
                .map(|(field, threshold)| SlowLogCondition { field, threshold })
                .collect(),
        }),
    ))
}

/// 解析分号分隔的多条规则，最多 10 条，按 ConnID 归组。
fn parse_slow_log_rule_set(
    raw_rules: &str,
    allow_conn_id: bool,
) -> Result<BTreeMap<i64, SlowLogRules>, String> {
    let raw_rules = raw_rules.trim();
    if raw_rules.is_empty() {
        return Ok(BTreeMap::new());
    }
    // 规则条数上限 10，与 Go 侧校验一致。
    let raw_entries: Vec<_> = raw_rules.split(';').collect();
    if raw_entries.len() > 10 {
        return Err(format!(
            "invalid slow log rules count:{}, limit is 10",
            raw_entries.len()
        ));
    }
    let mut result = BTreeMap::<i64, SlowLogRules>::new();
    for raw_entry in raw_entries {
        let (conn_id, rule) = parse_slow_log_rule_entry(raw_entry, allow_conn_id)?;
        let Some(rule) = rule else {
            continue;
        };
        let rules = result.entry(conn_id).or_default();
        rules.fields.extend(
            rule.conditions
                .iter()
                .map(|condition| condition.field.clone()),
        );
        rules.rules.push(rule);
    }
    Ok(result)
}

/// 将规则集编码回 `field:value,...;...` 原始串。
pub fn encodeRules(rules: &SlowLogRules) -> String {
    rules
        .rules
        .iter()
        .map(|rule| {
            rule.conditions
                .iter()
                .map(|condition| format!("{}:{}", condition.field, condition.threshold))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// 解析会话级慢日志规则（不允许 ConnID）。
pub fn ParseSessionSlowLogRules(raw_rules: &str) -> Result<Option<SlowLogRules>, String> {
    let mut rule_map = parse_slow_log_rule_set(raw_rules, false)?;
    let Some(mut rules) = rule_map.remove(&UnsetConnID) else {
        return Ok(None);
    };
    rules.raw_rules = encodeRules(&rules);
    Ok(Some(rules))
}

/// 解析全局慢日志规则（允许按 ConnID 分组）并计算 CRC64 哈希。
pub fn ParseGlobalSlowLogRules(raw_rules: &str) -> Result<GlobalSlowLogRules, String> {
    let rules_map = parse_slow_log_rule_set(raw_rules, true)?;
    let raw_rules = rules_map
        .values()
        .map(encodeRules)
        .filter(|raw| !raw.is_empty())
        .collect::<Vec<_>>()
        .join(";");
    let raw_rules_hash = Crc::<u64>::new(&CRC_64_ECMA_182).checksum(raw_rules.as_bytes());
    Ok(GlobalSlowLogRules {
        raw_rules,
        raw_rules_hash,
        rules_map,
    })
}

/// 字符串阈值精确相等匹配。
pub fn MatchEqual(threshold: &Threshold, value: impl AsRef<str>) -> bool {
    matches!(threshold, Threshold::String(s) if s == value.as_ref())
}

/// 布尔阈值精确相等匹配。
pub fn MatchEqualBool(threshold: &Threshold, value: bool) -> bool {
    matches!(threshold, Threshold::Bool(b) if *b == value)
}

/// 浮点阈值：实际值 ≥ 阈值。
pub fn matchGE(threshold: &Threshold, value: f64) -> bool {
    match threshold {
        Threshold::Float(t) => value >= *t,
        _ => false,
    }
}

/// 无符号整数阈值：实际值 ≥ 阈值。
pub fn matchGE_u64(threshold: &Threshold, value: u64) -> bool {
    match threshold {
        Threshold::UInt(t) => value >= *t,
        _ => false,
    }
}

/// 有符号整数阈值：实际值 ≥ 阈值。
pub fn matchGE_i64(threshold: &Threshold, value: i64) -> bool {
    match threshold {
        Threshold::Int(t) => value >= *t,
        _ => false,
    }
}

/// 将非负 i64 转为 u64；负值返回 `(0, false)`。
pub fn uint64FromNonNegative(value: i64) -> (u64, bool) {
    if value < 0 {
        (0, false)
    } else {
        (value as u64, true)
    }
}
/// 判定阈值是否为零（缺省 ExecDetail 时的匹配语义）。
pub fn matchZero(threshold: &Threshold) -> bool {
    match threshold {
        Threshold::Int(v) => *v == 0,
        Threshold::UInt(v) => *v == 0,
        Threshold::Float(v) => *v == 0.0,
        _ => false,
    }
}
