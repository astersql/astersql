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
// 遥测功能使用统计：计数器、快照差分与 infoschema 采集。
//
// 维护 current/initial 两份 MetricsSnapshot；上报时用差分得到周期内增量，
// 并结合会话全局变量与 infoschema 汇总 featureUsage。

use crate::{SessionContext, TelemetryError, getTTLUsageInfo, ttlUsageCounter};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 事务提交路径计数：两阶段提交（2PC）、异步提交、一阶段提交（1PC）。
pub struct TxnCommitCounter {
    pub TwoPC: i64,
    pub AsyncCommit: i64,
    pub OnePC: i64,
}

impl TxnCommitCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            TwoPC: self.TwoPC - old.TwoPC,
            AsyncCommit: self.AsyncCommit - old.AsyncCommit,
            OnePC: self.OnePC - old.OnePC,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// CTE（公共表表达式）使用计数：非递归 / 递归 / 非 CTE。
pub struct CTEUsageCounter {
    pub NonRecursiveCTEUsed: i64,
    pub RecursiveUsed: i64,
    pub NonCTEUsed: i64,
}

impl CTEUsageCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            NonRecursiveCTEUsed: self.NonRecursiveCTEUsed - old.NonRecursiveCTEUsed,
            RecursiveUsed: self.RecursiveUsed - old.RecursiveUsed,
            NonCTEUsed: self.NonCTEUsed - old.NonCTEUsed,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 账号锁定相关 DDL 计数：锁用户、解锁、创建或修改用户。
pub struct AccountLockCounter {
    pub LockUser: i64,
    pub UnlockUser: i64,
    pub CreateOrAlterUser: i64,
}

impl AccountLockCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            LockUser: self.LockUser - old.LockUser,
            UnlockUser: self.UnlockUser - old.UnlockUser,
            CreateOrAlterUser: self.CreateOrAlterUser - old.CreateOrAlterUser,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 非事务 DML（分片执行）语句计数。
pub struct NonTransactionalStmtCounter {
    pub DeleteCount: i64,
    pub UpdateCount: i64,
    pub InsertCount: i64,
}

impl NonTransactionalStmtCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            DeleteCount: self.DeleteCount - old.DeleteCount,
            UpdateCount: self.UpdateCount - old.UpdateCount,
            InsertCount: self.InsertCount - old.InsertCount,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 多 schema 变更（一次 DDL 改多列/索引）使用计数。
pub struct MultiSchemaChangeUsageCounter {
    pub MultiSchemaChangeUsed: i64,
}

impl MultiSchemaChangeUsageCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            MultiSchemaChangeUsed: self.MultiSchemaChangeUsed - old.MultiSchemaChangeUsed,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// EXCHANGE PARTITION 使用计数。
pub struct ExchangePartitionUsageCounter {
    pub ExchangePartitionCnt: i64,
}

impl ExchangePartitionUsageCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            ExchangePartitionCnt: self.ExchangePartitionCnt - old.ExchangePartitionCnt,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 表分区各类型与运维操作的使用计数。
pub struct TablePartitionUsageCounter {
    pub TablePartitionCnt: i64,
    pub TablePartitionListCnt: i64,
    pub TablePartitionRangeCnt: i64,
    pub TablePartitionHashCnt: i64,
    pub TablePartitionRangeColumnsCnt: i64,
    pub TablePartitionRangeColumnsGt1Cnt: i64,
    pub TablePartitionRangeColumnsGt2Cnt: i64,
    pub TablePartitionRangeColumnsGt3Cnt: i64,
    pub TablePartitionListColumnsCnt: i64,
    pub TablePartitionMaxPartitionsCnt: i64,
    pub TablePartitionCreateIntervalPartitionsCnt: i64,
    pub TablePartitionAddIntervalPartitionsCnt: i64,
    pub TablePartitionDropIntervalPartitionsCnt: i64,
    pub TablePartitionComactCnt: i64,
    pub TablePartitionReorganizePartitionCnt: i64,
}

impl TablePartitionUsageCounter {
    /// 表分区计数差分；MaxPartitions 取差分与旧值的较大者。
    pub fn Cal(self, rhs: Self) -> Self {
        Self {
            TablePartitionCnt: self.TablePartitionCnt - rhs.TablePartitionCnt,
            TablePartitionListCnt: self.TablePartitionListCnt - rhs.TablePartitionListCnt,
            TablePartitionRangeCnt: self.TablePartitionRangeCnt - rhs.TablePartitionRangeCnt,
            TablePartitionHashCnt: self.TablePartitionHashCnt - rhs.TablePartitionHashCnt,
            TablePartitionRangeColumnsCnt: self.TablePartitionRangeColumnsCnt
                - rhs.TablePartitionRangeColumnsCnt,
            TablePartitionRangeColumnsGt1Cnt: self.TablePartitionRangeColumnsGt1Cnt
                - rhs.TablePartitionRangeColumnsGt1Cnt,
            TablePartitionRangeColumnsGt2Cnt: self.TablePartitionRangeColumnsGt2Cnt
                - rhs.TablePartitionRangeColumnsGt2Cnt,
            TablePartitionRangeColumnsGt3Cnt: self.TablePartitionRangeColumnsGt3Cnt
                - rhs.TablePartitionRangeColumnsGt3Cnt,
            TablePartitionListColumnsCnt: self.TablePartitionListColumnsCnt
                - rhs.TablePartitionListColumnsCnt,
            TablePartitionMaxPartitionsCnt: (self.TablePartitionMaxPartitionsCnt
                - rhs.TablePartitionMaxPartitionsCnt)
                .max(rhs.TablePartitionMaxPartitionsCnt),
            TablePartitionCreateIntervalPartitionsCnt: self
                .TablePartitionCreateIntervalPartitionsCnt
                - rhs.TablePartitionCreateIntervalPartitionsCnt,
            TablePartitionAddIntervalPartitionsCnt: self.TablePartitionAddIntervalPartitionsCnt
                - rhs.TablePartitionAddIntervalPartitionsCnt,
            TablePartitionDropIntervalPartitionsCnt: self.TablePartitionDropIntervalPartitionsCnt
                - rhs.TablePartitionDropIntervalPartitionsCnt,
            TablePartitionComactCnt: self.TablePartitionComactCnt - rhs.TablePartitionComactCnt,
            TablePartitionReorganizePartitionCnt: self.TablePartitionReorganizePartitionCnt
                - rhs.TablePartitionReorganizePartitionCnt,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// IndexMerge（多索引合并扫描）使用计数。
pub struct IndexMergeUsageCounter {
    pub IndexMergeUsed: i64,
}

impl IndexMergeUsageCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            IndexMergeUsed: self.IndexMergeUsed - old.IndexMergeUsed,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 悲观事务公平加锁（fair locking）使用与生效计数。
pub struct FairLockingUsageCounter {
    pub TxnFairLockingUsed: i64,
    pub TxnFairLockingEffective: i64,
}

impl FairLockingUsageCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            TxnFairLockingUsed: self.TxnFairLockingUsed - old.TxnFairLockingUsed,
            TxnFairLockingEffective: self.TxnFairLockingEffective - old.TxnFairLockingEffective,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// DDL 相关功能使用：加索引 ingest、元数据锁、闪回集群、分布式重组。
pub struct DDLUsageCounter {
    pub AddIndexIngestUsed: i64,
    pub MetadataLockUsed: bool,
    pub FlashbackClusterUsed: i64,
    pub DistReorgUsed: i64,
}

impl DDLUsageCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            AddIndexIngestUsed: self.AddIndexIngestUsed - old.AddIndexIngestUsed,
            MetadataLockUsed: false,
            FlashbackClusterUsed: self.FlashbackClusterUsed - old.FlashbackClusterUsed,
            DistReorgUsed: self.DistReorgUsed - old.DistReorgUsed,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Store 批量 Coprocessor 请求相关计数与批大小。
pub struct StoreBatchCoprCounter {
    pub BatchSize: i32,
    pub BatchedQuery: i64,
    pub BatchedQueryTask: i64,
    pub BatchedCount: i64,
    pub BatchedFallbackCount: i64,
}

impl StoreBatchCoprCounter {
    /// 相对旧快照做字段差分（current - initial）。
    pub fn Sub(self, old: Self) -> Self {
        Self {
            BatchSize: 0,
            BatchedQuery: self.BatchedQuery - old.BatchedQuery,
            BatchedQueryTask: self.BatchedQueryTask - old.BatchedQueryTask,
            BatchedCount: self.BatchedCount - old.BatchedCount,
            BatchedFallbackCount: self.BatchedFallbackCount - old.BatchedFallbackCount,
        }
    }
}

#[derive(Clone, Debug, Default)]
/// 遥测指标快照：各功能计数器的当前或基准值集合。
pub struct MetricsSnapshot {
    pub Txn: TxnCommitCounter,
    pub CTE: CTEUsageCounter,
    pub AccountLock: AccountLockCounter,
    pub NonTransactional: NonTransactionalStmtCounter,
    pub MultiSchema: MultiSchemaChangeUsageCounter,
    pub ExchangePartition: ExchangePartitionUsageCounter,
    pub TablePartition: TablePartitionUsageCounter,
    pub Savepoint: i64,
    pub LazyUnique: i64,
    pub DDL: DDLUsageCounter,
    pub IndexMerge: IndexMergeUsageCounter,
    pub StoreBatch: StoreBatchCoprCounter,
    pub FairLocking: FairLockingUsageCounter,
}

/// 当前累计指标快照（上报周期内持续增长）。
fn current() -> &'static Mutex<MetricsSnapshot> {
    static M: OnceLock<Mutex<MetricsSnapshot>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(MetricsSnapshot::default()))
}

/// 上一周期结束时的基准快照，用于计算增量。
fn initial() -> &'static Mutex<MetricsSnapshot> {
    static M: OnceLock<Mutex<MetricsSnapshot>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(MetricsSnapshot::default()))
}

/// 测试/注入用：直接覆盖当前指标快照。
pub fn SetMetricsSnapshot(value: MetricsSnapshot) {
    *current().lock().expect("metrics lock poisoned") = value
}

/// 测试用：清空 current 与 initial 快照。
pub fn ResetMetricsForTest() {
    *current().lock().expect("metrics lock poisoned") = MetricsSnapshot::default();
    *initial().lock().expect("metrics lock poisoned") = MetricsSnapshot::default();
}

#[derive(Clone, Debug, Default)]
/// Placement Policy（副本放置策略）使用概况。
pub struct placementPolicyUsage {
    pub NumPlacementPolicies: u64,
    pub NumDBWithPolicies: u64,
    pub NumTableWithPolicies: u64,
    pub NumPartitionWithExplicitPolicies: u64,
}

#[derive(Clone, Debug, Default)]
/// 资源管控（Resource Control）开关与资源组数量。
pub struct resourceControlUsage {
    pub Enabled: bool,
    pub NumResourceGroups: u64,
}

#[derive(Clone, Debug, Default)]
/// 单表是否使用聚簇索引及其主键类型。
pub struct TableClusteredInfo {
    pub IsClustered: bool,
    pub ClusterPKType: String,
}

#[derive(Clone, Debug, Default)]
/// 聚簇索引表数量相对总表数的统计。
pub struct NewClusterIndexUsage {
    pub NumClusteredTables: u64,
    pub NumTotalTables: u64,
}

#[derive(Clone, Debug, Default)]
/// 事务相关功能开关与提交路径增量计数。
pub struct TxnUsage {
    pub AsyncCommitUsed: bool,
    pub OnePCUsed: bool,
    pub TxnCommitCounter: TxnCommitCounter,
    pub MutationCheckerUsed: bool,
    pub AssertionLevel: String,
    pub RcCheckTS: bool,
    pub RCWriteCheckTS: bool,
    pub FairLocking: bool,
    pub SavepointCounter: i64,
    pub LazyUniqueCheckSetCounter: i64,
    pub FairLockingUsageCounter: FairLockingUsageCounter,
}

#[derive(Clone, Debug, Default)]
/// 功能使用总览，序列化后写入遥测报告的 featureUsage 字段。
pub struct featureUsage {
    pub Txn: TxnUsage,
    pub NewClusterIndex: NewClusterIndexUsage,
    pub TemporaryTable: bool,
    pub CTE: CTEUsageCounter,
    pub AccountLock: AccountLockCounter,
    pub CachedTable: bool,
    pub AutoCapture: bool,
    pub PlacementPolicyUsage: placementPolicyUsage,
    pub NonTransactionalUsage: NonTransactionalStmtCounter,
    pub GlobalKill: bool,
    pub MultiSchemaChange: MultiSchemaChangeUsageCounter,
    pub ExchangePartition: ExchangePartitionUsageCounter,
    pub TablePartition: TablePartitionUsageCounter,
    pub LogBackup: bool,
    pub EnablePaging: bool,
    pub EnableCostModelVer2: bool,
    pub DDLUsageCounter: DDLUsageCounter,
    pub EnableGlobalMemoryControl: bool,
    pub AutoIDNoCache: bool,
    pub IndexMergeUsageCounter: IndexMergeUsageCounter,
    pub ResourceControlUsage: resourceControlUsage,
    pub TTLUsage: ttlUsageCounter,
    pub StoreBatchCoprUsage: StoreBatchCoprCounter,
}

impl featureUsage {
    /// 将字符串编码为 JSON string，等价于 Go `encoding/json` 对字符串的处理。
    fn json_quote(value: &str) -> String {
        let mut out = String::with_capacity(value.len() + 2);
        out.push('"');
        for ch in value.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '<' => out.push_str("\\u003c"),
                '>' => out.push_str("\\u003e"),
                '&' => out.push_str("\\u0026"),
                '\u{2028}' => out.push_str("\\u2028"),
                '\u{2029}' => out.push_str("\\u2029"),
                ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", ch as u32)),
                ch => out.push(ch),
            }
        }
        out.push('"');
        out
    }

    fn txn_json(&self) -> String {
        format!(
            "{{\"asyncCommitUsed\":{},\"onePCUsed\":{},\"txnCommitCounter\":{{\"twoPC\":{},\"asyncCommit\":{},\"onePC\":{}}},\"mutationCheckerUsed\":{},\"assertionLevel\":{},\"rcCheckTS\":{},\"rcWriteCheckTS\":{},\"fairLocking\":{},\"SavepointCounter\":{},\"lazyUniqueCheckSetCounter\":{},\"FairLockingUsageCounter\":{{\"txn_fair_locking_used\":{},\"txn_fair_locking_effective\":{}}}}}",
            self.Txn.AsyncCommitUsed,
            self.Txn.OnePCUsed,
            self.Txn.TxnCommitCounter.TwoPC,
            self.Txn.TxnCommitCounter.AsyncCommit,
            self.Txn.TxnCommitCounter.OnePC,
            self.Txn.MutationCheckerUsed,
            Self::json_quote(&self.Txn.AssertionLevel),
            self.Txn.RcCheckTS,
            self.Txn.RCWriteCheckTS,
            self.Txn.FairLocking,
            self.Txn.SavepointCounter,
            self.Txn.LazyUniqueCheckSetCounter,
            self.Txn.FairLockingUsageCounter.TxnFairLockingUsed,
            self.Txn.FairLockingUsageCounter.TxnFairLockingEffective,
        )
    }

    fn ttl_json(&self) -> String {
        fn hist(items: &[crate::ttlHistItem]) -> String {
            items
                .iter()
                .map(|item| {
                    let less_than = item
                        .LessThan
                        .map(|value| format!("\"less_than\":{},", value))
                        .unwrap_or_default();
                    let less_than_max = if item.LessThanMax {
                        "\"less_than_max\":true,".to_owned()
                    } else {
                        String::new()
                    };
                    format!("{{{}{}\"count\":{}}}", less_than, less_than_max, item.Count)
                })
                .collect::<Vec<_>>()
                .join(",")
        }

        format!(
            "{{\"ttl_job_enabled\":{},\"ttl_table_count\":{},\"ttl_job_enabled_tables\":{},\"ttl_hist_date\":{},\"table_hist_with_delete_rows\":[{}],\"table_hist_with_delay_time\":[{}]}}",
            self.TTLUsage.TTLJobEnabled,
            self.TTLUsage.TTLTables,
            self.TTLUsage.TTLJobEnabledTables,
            Self::json_quote(&self.TTLUsage.TTLHistDate),
            hist(&self.TTLUsage.TableHistWithDeleteRows),
            hist(&self.TTLUsage.TableHistWithDelayTime),
        )
    }

    /// 序列化为与 Go `encoding/json` tags 一致的完整 JSON。
    pub fn Marshal(&self) -> String {
        format!(
            "{{\"txn\":{},\"newClusterIndex\":{{\"numClusteredTables\":{},\"numTotalTables\":{}}},\"temporaryTable\":{},\"cte\":{{\"nonRecursiveCTEUsed\":{},\"recursiveUsed\":{},\"nonCTEUsed\":{}}},\"accountLock\":{{\"lockUser\":{},\"unlockUser\":{},\"createOrAlterUser\":{}}},\"cachedTable\":{},\"autoCapture\":{},\"placementPolicy\":{{\"numPlacementPolicies\":{},\"numDBWithPolicies\":{},\"numTableWithPolicies\":{},\"numPartitionWithExplicitPolicies\":{}}},\"nonTransactional\":{{\"delete\":{},\"update\":{},\"insert\":{}}},\"globalKill\":{},\"multiSchemaChange\":{{\"multi_schema_change_used\":{}}},\"exchangePartition\":{{\"exchange_partition_cnt\":{}}},\"tablePartition\":{{\"table_partition_cnt\":{},\"table_partition_list_cnt\":{},\"table_partition_range_cnt\":{},\"table_partition_hash_cnt\":{},\"table_partition_range_columns_cnt\":{},\"table_partition_range_columns_gt_1_cnt\":{},\"table_partition_range_columns_gt_2_cnt\":{},\"table_partition_range_columns_gt_3_cnt\":{},\"table_partition_list_columns_cnt\":{},\"table_partition_max_partitions_cnt\":{},\"table_partition_create_interval_partitions_cnt\":{},\"table_partition_add_interval_partitions_cnt\":{},\"table_partition_drop_interval_partitions_cnt\":{},\"table_TablePartitionComactCnt\":{},\"table_reorganize_partition_cnt\":{}}},\"logBackup\":{},\"enablePaging\":{},\"enableCostModelVer2\":{},\"DDLUsageCounter\":{{\"add_index_ingest_used\":{},\"metadata_lock_used\":{},\"flashback_cluster_used\":{},\"dist_reorg_used\":{}}},\"enableGlobalMemoryControl\":{},\"autoIDNoCache\":{},\"indexMergeUsageCounter\":{{\"index_merge_used\":{}}},\"resourceControl\":{{\"resourceControlEnabled\":{},\"numResourceGroups\":{}}},\"ttlUsage\":{},\"storeBatchCopr\":{{\"batch_size\":{},\"query\":{},\"tasks\":{},\"batched\":{},\"batched_fallback\":{}}}}}",
            self.txn_json(),
            self.NewClusterIndex.NumClusteredTables,
            self.NewClusterIndex.NumTotalTables,
            self.TemporaryTable,
            self.CTE.NonRecursiveCTEUsed,
            self.CTE.RecursiveUsed,
            self.CTE.NonCTEUsed,
            self.AccountLock.LockUser,
            self.AccountLock.UnlockUser,
            self.AccountLock.CreateOrAlterUser,
            self.CachedTable,
            self.AutoCapture,
            self.PlacementPolicyUsage.NumPlacementPolicies,
            self.PlacementPolicyUsage.NumDBWithPolicies,
            self.PlacementPolicyUsage.NumTableWithPolicies,
            self.PlacementPolicyUsage.NumPartitionWithExplicitPolicies,
            self.NonTransactionalUsage.DeleteCount,
            self.NonTransactionalUsage.UpdateCount,
            self.NonTransactionalUsage.InsertCount,
            self.GlobalKill,
            self.MultiSchemaChange.MultiSchemaChangeUsed,
            self.ExchangePartition.ExchangePartitionCnt,
            self.TablePartition.TablePartitionCnt,
            self.TablePartition.TablePartitionListCnt,
            self.TablePartition.TablePartitionRangeCnt,
            self.TablePartition.TablePartitionHashCnt,
            self.TablePartition.TablePartitionRangeColumnsCnt,
            self.TablePartition.TablePartitionRangeColumnsGt1Cnt,
            self.TablePartition.TablePartitionRangeColumnsGt2Cnt,
            self.TablePartition.TablePartitionRangeColumnsGt3Cnt,
            self.TablePartition.TablePartitionListColumnsCnt,
            self.TablePartition.TablePartitionMaxPartitionsCnt,
            self.TablePartition
                .TablePartitionCreateIntervalPartitionsCnt,
            self.TablePartition.TablePartitionAddIntervalPartitionsCnt,
            self.TablePartition.TablePartitionDropIntervalPartitionsCnt,
            self.TablePartition.TablePartitionComactCnt,
            self.TablePartition.TablePartitionReorganizePartitionCnt,
            self.LogBackup,
            self.EnablePaging,
            self.EnableCostModelVer2,
            self.DDLUsageCounter.AddIndexIngestUsed,
            self.DDLUsageCounter.MetadataLockUsed,
            self.DDLUsageCounter.FlashbackClusterUsed,
            self.DDLUsageCounter.DistReorgUsed,
            self.EnableGlobalMemoryControl,
            self.AutoIDNoCache,
            self.IndexMergeUsageCounter.IndexMergeUsed,
            self.ResourceControlUsage.Enabled,
            self.ResourceControlUsage.NumResourceGroups,
            self.ttl_json(),
            self.StoreBatchCoprUsage.BatchSize,
            self.StoreBatchCoprUsage.BatchedQuery,
            self.StoreBatchCoprUsage.BatchedQueryTask,
            self.StoreBatchCoprUsage.BatchedCount,
            self.StoreBatchCoprUsage.BatchedFallbackCount,
        )
    }
}

/// 判断会话全局变量是否为开启态（on/1/true）。
fn on(ctx: &SessionContext, key: &str) -> bool {
    ctx.GlobalVars
        .get(key)
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "on" | "1" | "true"))
}

/// 从会话上下文统计聚簇索引表占比；可注入查询失败。
pub fn getClusterIndexUsageInfo(
    ctx: &SessionContext,
) -> Result<NewClusterIndexUsage, TelemetryError> {
    if ctx.FailClusterQuery {
        return Err(TelemetryError("cluster index query failed".into()));
    }
    Ok(NewClusterIndexUsage {
        NumClusteredTables: ctx
            .ClusteredTableTypes
            .iter()
            .filter(|v| v.as_str() == "CLUSTERED")
            .count() as u64,
        NumTotalTables: ctx.ClusteredTableTypes.len() as u64,
    })
}

/// 采集事务功能开关与提交计数相对基准的增量。
pub fn getTxnUsageInfo(ctx: &SessionContext) -> TxnUsage {
    let c = current().lock().expect("metrics lock poisoned").clone();
    let i = initial().lock().expect("metrics lock poisoned").clone();
    TxnUsage {
        AsyncCommitUsed: on(ctx, "tidb_enable_async_commit"),
        OnePCUsed: on(ctx, "tidb_enable_1pc"),
        TxnCommitCounter: c.Txn.Sub(i.Txn),
        MutationCheckerUsed: on(ctx, "tidb_enable_mutation_checker"),
        AssertionLevel: ctx
            .GlobalVars
            .get("tidb_txn_assertion_level")
            .cloned()
            .unwrap_or_default(),
        RcCheckTS: on(ctx, "tidb_rc_read_check_ts"),
        RCWriteCheckTS: on(ctx, "tidb_rc_write_check_ts"),
        FairLocking: on(ctx, "tidb_pessimistic_transaction_fair_locking"),
        SavepointCounter: c.Savepoint - i.Savepoint,
        LazyUniqueCheckSetCounter: c.LazyUnique - i.LazyUnique,
        FairLockingUsageCounter: c.FairLocking.Sub(i.FairLocking),
    }
}

/// 遍历 infoschema：临时表、缓存表、放置策略与资源组等。
pub fn collectFeatureUsageFromInfoschema(ctx: &SessionContext, usage: &mut featureUsage) {
    for schema in &ctx.Schemas {
        if schema.PlacementPolicy {
            usage.PlacementPolicyUsage.NumDBWithPolicies += 1
        }
        for table in &schema.Tables {
            usage.TemporaryTable |= table.Temporary;
            usage.CachedTable |= table.Cached;
            usage.AutoIDNoCache |= table.AutoIDCache == 1;
            usage.PlacementPolicyUsage.NumTableWithPolicies += table.PlacementPolicy as u64;
            usage.PlacementPolicyUsage.NumPartitionWithExplicitPolicies += table.PartitionPolicies
        }
    }
    usage.PlacementPolicyUsage.NumPlacementPolicies = ctx.PlacementPolicies;
    usage.ResourceControlUsage.NumResourceGroups = ctx.ResourceGroups;
    usage.ResourceControlUsage.Enabled = on(ctx, "tidb_enable_resource_control")
}

/// 汇总全部功能使用信息，供遥测上报。
pub fn getFeatureUsage(ctx: &SessionContext) -> Result<featureUsage, TelemetryError> {
    let mut u = featureUsage {
        Txn: getTxnUsageInfo(ctx),
        NewClusterIndex: getClusterIndexUsageInfo(ctx)?,
        CTE: getCTEUsageInfo(),
        AccountLock: getAccountLockUsageInfo(),
        AutoCapture: getAutoCaptureUsageInfo(ctx),
        NonTransactionalUsage: getNonTransactionalUsage(),
        GlobalKill: getGlobalKillUsageInfo(ctx),
        MultiSchemaChange: getMultiSchemaChangeUsageInfo(),
        ExchangePartition: getExchangePartitionUsageInfo(),
        TablePartition: getTablePartitionUsageInfo(),
        LogBackup: getLogBackupUsageInfo(ctx),
        EnablePaging: getPagingUsageInfo(ctx),
        EnableCostModelVer2: getCostModelVer2UsageInfo(ctx),
        DDLUsageCounter: getDDLUsageInfo(ctx),
        EnableGlobalMemoryControl: getGlobalMemoryControl(ctx),
        IndexMergeUsageCounter: getIndexMergeUsageInfo(),
        TTLUsage: getTTLUsageInfo(ctx),
        StoreBatchCoprUsage: getStoreBatchUsage(ctx),
        ..Default::default()
    };
    collectFeatureUsageFromInfoschema(ctx, &mut u);
    Ok(u)
}

/// 返回 CTE 使用计数相对基准的增量。
pub fn getCTEUsageInfo() -> CTEUsageCounter {
    let c = current().lock().unwrap().CTE;
    let i = initial().lock().unwrap().CTE;
    c.Sub(i)
}

/// 返回账号锁定相关计数增量。
pub fn getAccountLockUsageInfo() -> AccountLockCounter {
    let c = current().lock().unwrap().AccountLock;
    let i = initial().lock().unwrap().AccountLock;
    c.Sub(i)
}

/// 返回多 schema 变更使用增量。
pub fn getMultiSchemaChangeUsageInfo() -> MultiSchemaChangeUsageCounter {
    let c = current().lock().unwrap().MultiSchema;
    let i = initial().lock().unwrap().MultiSchema;
    c.Sub(i)
}

/// 返回交换分区使用增量。
pub fn getExchangePartitionUsageInfo() -> ExchangePartitionUsageCounter {
    let c = current().lock().unwrap().ExchangePartition;
    let i = initial().lock().unwrap().ExchangePartition;
    c.Sub(i)
}

/// 返回表分区使用增量（含最大分区数特殊合并）。
pub fn getTablePartitionUsageInfo() -> TablePartitionUsageCounter {
    let c = current().lock().unwrap().TablePartition;
    let i = initial().lock().unwrap().TablePartition;
    c.Cal(i)
}

/// 返回非事务语句使用增量。
pub fn getNonTransactionalUsage() -> NonTransactionalStmtCounter {
    let c = current().lock().unwrap().NonTransactional;
    let i = initial().lock().unwrap().NonTransactional;
    c.Sub(i)
}

/// 返回 IndexMerge 使用增量。
pub fn getIndexMergeUsageInfo() -> IndexMergeUsageCounter {
    let c = current().lock().unwrap().IndexMerge;
    let i = initial().lock().unwrap().IndexMerge;
    c.Sub(i)
}

/// 将 current 快照中某字段写回 initial，作为新的差分基准。
fn snapshot_field(update: impl FnOnce(&mut MetricsSnapshot, &MetricsSnapshot)) {
    let c = current().lock().unwrap().clone();
    update(&mut initial().lock().unwrap(), &c)
}

/// 上报后重置事务提交计数基准。
pub fn postReportTxnUsage() {
    snapshot_field(|i, c| i.Txn = c.Txn)
}

/// 上报后重置 CTE 计数基准。
pub fn postReportCTEUsage() {
    snapshot_field(|i, c| i.CTE = c.CTE)
}

/// 上报后重置账号锁定计数基准。
pub fn postReportAccountLockUsage() {
    snapshot_field(|i, c| i.AccountLock = c.AccountLock)
}

/// 上报后重置 SAVEPOINT 计数基准。
pub fn PostSavepointCount() {
    snapshot_field(|i, c| i.Savepoint = c.Savepoint)
}

/// 上报后重置惰性唯一性检查计数基准。
pub fn postReportLazyPessimisticUniqueCheckSetCount() {
    snapshot_field(|i, c| i.LazyUnique = c.LazyUnique)
}

/// 上报后重置公平加锁计数基准。
pub fn postReportFairLockingUsageCounter() {
    snapshot_field(|i, c| i.FairLocking = c.FairLocking)
}

/// 上报后重置多 schema 变更计数基准。
pub fn postReportMultiSchemaChangeUsage() {
    snapshot_field(|i, c| i.MultiSchema = c.MultiSchema)
}

/// 上报后重置交换分区计数基准。
pub fn postReportExchangePartitionUsage() {
    snapshot_field(|i, c| i.ExchangePartition = c.ExchangePartition)
}

/// 上报后重置表分区计数基准，并按 Go 语义保留 MaxPartitions。
pub fn postReportTablePartitionUsage() {
    // Align with Go ResetTablePartitionCounter: refresh initial from current,
    // keeping MaxPartitions as the Cal-style max.
    let c = current().lock().unwrap().TablePartition;
    let mut i = initial().lock().unwrap();
    let max = (c.TablePartitionMaxPartitionsCnt - i.TablePartition.TablePartitionMaxPartitionsCnt)
        .max(i.TablePartition.TablePartitionMaxPartitionsCnt);
    i.TablePartition = c;
    i.TablePartition.TablePartitionMaxPartitionsCnt = max;
}

/// 上报后重置 DDL 使用计数基准。
pub fn postReportDDLUsage() {
    snapshot_field(|i, c| i.DDL = c.DDL)
}

/// 上报后重置非事务语句计数基准。
pub fn postReportNonTransactionalCounter() {
    snapshot_field(|i, c| i.NonTransactional = c.NonTransactional)
}

/// 上报后重置 IndexMerge 计数基准。
pub fn postReportIndexMergeUsage() {
    snapshot_field(|i, c| i.IndexMerge = c.IndexMerge)
}

/// 上报后重置 Store Batch Copr 计数基准。
pub fn postStoreBatchUsage() {
    snapshot_field(|i, c| i.StoreBatch = c.StoreBatch)
}

/// 是否开启执行计划基线自动捕获。
pub fn getAutoCaptureUsageInfo(c: &SessionContext) -> bool {
    on(c, "tidb_capture_plan_baselines")
}

/// 是否开启全局 Kill。
pub fn getGlobalKillUsageInfo(c: &SessionContext) -> bool {
    on(c, "enable_global_kill")
}

/// 是否启用日志备份。
pub fn getLogBackupUsageInfo(c: &SessionContext) -> bool {
    c.LogBackup
}

/// 是否使用代价模型 v2。
pub fn getCostModelVer2UsageInfo(c: &SessionContext) -> bool {
    c.CostModelVersion == 2
}

/// 是否开启 Coprocessor 分页（paging）。
pub fn getPagingUsageInfo(c: &SessionContext) -> bool {
    c.EnablePaging
}

/// 采集 DDL 使用增量，并单独读取元数据锁开关。
pub fn getDDLUsageInfo(c: &SessionContext) -> DDLUsageCounter {
    let mut d = current()
        .lock()
        .unwrap()
        .DDL
        .Sub(initial().lock().unwrap().DDL);
    d.MetadataLockUsed = on(c, "tidb_enable_metadata_lock");
    d
}

/// 是否配置了大于 0 的服务器内存限制。
pub fn getGlobalMemoryControl(c: &SessionContext) -> bool {
    c.GlobalVars
        .get("tidb_server_memory_limit")
        .or_else(|| c.GlobalVars.get("server_memory_limit"))
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|value| value > 0)
}

/// 采集 Store Batch Copr 增量，并填入当前批大小配置。
pub fn getStoreBatchUsage(c: &SessionContext) -> StoreBatchCoprCounter {
    let mut diff = current()
        .lock()
        .unwrap()
        .StoreBatch
        .Sub(initial().lock().unwrap().StoreBatch);
    if let Some(val) = c.GlobalVars.get("tidb_store_batch_size") {
        if let Ok(batch_size) = val.parse::<i32>() {
            diff.BatchSize = batch_size;
        }
    }
    diff
}
