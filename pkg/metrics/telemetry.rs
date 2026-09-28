// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Telemetry metrics and snapshot arithmetic corresponding to `telemetry.go`.
//
// 遥测（Telemetry）用量计数与快照差分，对应 Go 的 `telemetry.go`。
// 聚合 CTE、分区表、DDL、Fair Locking 等特性的使用次数，供周期性上报；
// 快照结构支持 `sub`/`Cal` 计算两个采样点之间的增量。

use prometheus::{Counter, CounterVec, Error, Opts};
use std::sync::OnceLock;

/// 构造 telemetry 子系统下的 Counter。
fn counter(name: &str, help: &str) -> Result<Counter, Error> {
    Counter::with_opts(
        Opts::new(name, help)
            .namespace("tidb")
            .subsystem("telemetry"),
    )
}

/// 构造 telemetry 子系统下的 CounterVec。
fn counter_vec(name: &str, help: &str, labels: &[&str]) -> Result<CounterVec, Error> {
    CounterVec::new(
        Opts::new(name, help)
            .namespace("tidb")
            .subsystem("telemetry"),
        labels,
    )
}

/// 读取 Counter 当前值并转为 i64（与 Go 上报整型一致）。
fn read_counter(counter: &Counter) -> i64 {
    counter.get() as i64
}

/// 遥测指标集合：持有各类特性用量 Counter/CounterVec。
pub struct TelemetryMetrics {
    pub cte: CounterVec,
    pub account_lock: CounterVec,
    pub multi_schema_change: Counter,
    pub table_partition: [Counter; 15],
    pub exchange_partition: Counter,
    pub add_index_ingest: Counter,
    pub flashback_cluster: Counter,
    pub index_merge: Counter,
    pub dist_reorg: Counter,
    pub store_batched_query: Counter,
    pub batched_query_task: Counter,
    pub store_batched: Counter,
    pub store_batched_fallback: Counter,
    pub non_transactional_dml: CounterVec,
    pub stmt_node: CounterVec,
    pub lazy_pessimistic_unique_check_set: Counter,
    pub fair_locking_usage: CounterVec,
}

impl TelemetryMetrics {
    /// 按 Go 顺序构造全部遥测 Counter；失败时返回 Prometheus 构造错误。
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            cte: counter_vec(
                "non_recursive_cte_usage",
                "Counter of usage of CTE",
                &["cte_type"],
            )?,
            account_lock: counter_vec(
                "account_lock_usage",
                "Counter of locked/unlocked users",
                &["account_lock"],
            )?,
            multi_schema_change: counter(
                "multi_schema_change_usage",
                "Counter of usage of multi-schema change",
            )?,
            table_partition: [
                counter(
                    "table_partition_usage",
                    "Counter of CREATE TABLE statements using partitioning",
                )?,
                counter("table_partition_list_usage", "Counter of LIST partitioning")?,
                counter(
                    "table_partition_range_usage",
                    "Counter of RANGE partitioning",
                )?,
                counter("table_partition_hash_usage", "Counter of HASH partitioning")?,
                counter(
                    "table_partition_range_columns_usage",
                    "Counter of RANGE COLUMNS partitioning",
                )?,
                counter(
                    "table_partition_range_multi_columns_usage",
                    "Counter of RANGE COLUMNS with more than one column",
                )?,
                counter(
                    "table_partition_range_multi_columns_usage",
                    "Counter of RANGE COLUMNS with more than two columns",
                )?,
                counter(
                    "table_partition_range_multi_columns_usage",
                    "Counter of RANGE COLUMNS with more than three columns",
                )?,
                counter(
                    "table_partition_list_columns_usage",
                    "Counter of LIST COLUMNS partitioning",
                )?,
                counter(
                    "table_partition_max_partition_usage",
                    "Counter of partitions created by CREATE TABLE",
                )?,
                counter(
                    "table_partition_create_interval_partition_usage",
                    "Counter of partitions created by CREATE TABLE INTERVAL",
                )?,
                counter(
                    "table_partition_add_interval_partition_usage",
                    "Counter of partitions added by ALTER TABLE LAST PARTITION",
                )?,
                counter(
                    "table_partition_drop_interval_partition_usage",
                    "Counter of partitions dropped by ALTER TABLE FIRST PARTITION",
                )?,
                counter(
                    "compact_partition_usage",
                    "Counter of compact table partition",
                )?,
                counter(
                    "reorganize_partition_usage",
                    "Counter of alter table reorganize partition",
                )?,
            ],
            exchange_partition: counter(
                "exchange_partition_usage",
                "Counter of exchange partition statements",
            )?,
            add_index_ingest: counter(
                "add_index_ingest_usage",
                "Counter of add index acceleration usage",
            )?,
            flashback_cluster: counter(
                "flashback_cluster_usage",
                "Counter of flashback cluster usage",
            )?,
            index_merge: counter("index_merge_usage", "Counter of index merge usage")?,
            dist_reorg: counter(
                "distributed_reorg_count",
                "Counter of distributed reorg DDL tasks",
            )?,
            store_batched_query: counter(
                "store_batched_query",
                "Counter of queries using store batched coprocessor tasks",
            )?,
            batched_query_task: counter(
                "batched_query_task",
                "Counter of coprocessor tasks in batched queries",
            )?,
            store_batched: counter(
                "store_batched",
                "Counter of successfully batched coprocessor tasks",
            )?,
            store_batched_fallback: counter(
                "store_batched_fallback",
                "Counter of fallback batched coprocessor tasks",
            )?,
            non_transactional_dml: CounterVec::new(
                Opts::new(
                    "non_transactional_dml_count",
                    "Counter of non-transactional DML statements",
                )
                .namespace("tidb")
                .subsystem("server"),
                &["type"],
            )?,
            stmt_node: CounterVec::new(
                Opts::new("statement_node_total", "Counter of statement nodes")
                    .namespace("tidb")
                    .subsystem("server"),
                &["type", "db", "resource_group"],
            )?,
            lazy_pessimistic_unique_check_set: Counter::with_opts(
                Opts::new(
                    "lazy_pessimistic_unique_check_set",
                    "Counter of disabling pessimistic unique checks",
                )
                .namespace("tidb")
                .subsystem("server"),
            )?,
            fair_locking_usage: CounterVec::new(
                Opts::new("fair_locking_usage", "Counter of fair locking usage")
                    .namespace("tidb")
                    .subsystem("server"),
                &["type"],
            )?,
        })
    }

    /// 读取 CTE（公用表表达式）用量快照。
    pub fn cte_counter(&self) -> CTEUsageCounter {
        CTEUsageCounter::new(
            read_counter(&self.cte.with_label_values(&["nonRecurCTE"])),
            read_counter(&self.cte.with_label_values(&["recurCTE"])),
            read_counter(&self.cte.with_label_values(&["notCTE"])),
        )
    }

    /// 读取账户锁定/解锁用量快照。
    pub fn account_lock_counter(&self) -> AccountLockCounter {
        AccountLockCounter::new(
            read_counter(&self.account_lock.with_label_values(&["lockUser"])),
            read_counter(&self.account_lock.with_label_values(&["unlockUser"])),
            read_counter(&self.account_lock.with_label_values(&["createOrAlterUser"])),
        )
    }

    /// 读取分区表相关用量快照（15 个计数器）。
    pub fn table_partition_counter(&self) -> TablePartitionUsageCounter {
        TablePartitionUsageCounter::from_values(
            self.table_partition.each_ref().map(|c| read_counter(c)),
        )
    }

    /// 按 Go 规则重置分区表计数：保留 max-partitions 峰值语义并清零区间分区相关项。
    pub fn reset_table_partition_counter(
        &self,
        previous: &TablePartitionUsageCounter,
    ) -> TablePartitionUsageCounter {
        let mut values = self.table_partition_counter().values();
        values[9] = (values[9] - previous.values()[9]).max(previous.values()[9]);
        values[10..14].fill(0);
        TablePartitionUsageCounter::from_values(values)
    }

    /// 读取非事务 DML（delete/update/insert）用量快照。
    pub fn non_transactional_stmt_counter(&self) -> NonTransactionalStmtCounter {
        NonTransactionalStmtCounter::new(
            read_counter(&self.non_transactional_dml.with_label_values(&["delete"])),
            read_counter(&self.non_transactional_dml.with_label_values(&["update"])),
            read_counter(&self.non_transactional_dml.with_label_values(&["insert"])),
        )
    }

    /// 读取 DDL 相关用量（加索引加速、flashback、分布式重组等）。
    pub fn ddl_usage_counter(&self) -> DDLUsageCounter {
        DDLUsageCounter::new(
            read_counter(&self.add_index_ingest),
            false,
            read_counter(&self.flashback_cluster),
            read_counter(&self.dist_reorg),
        )
    }

    /// 读取 Store 批量 Coprocessor 用量；BatchSize 固定填 0 与 Go 一致。
    pub fn store_batch_copr_counter(&self) -> StoreBatchCoprCounter {
        StoreBatchCoprCounter::new(
            0,
            read_counter(&self.store_batched_query),
            read_counter(&self.batched_query_task),
            read_counter(&self.store_batched),
            read_counter(&self.store_batched_fallback),
        )
    }

    /// 读取事务级 Fair Locking 使用/生效次数。
    pub fn fair_locking_usage_counter(&self) -> FairLockingUsageCounter {
        FairLockingUsageCounter::new(
            read_counter(&self.fair_locking_usage.with_label_values(&["txn-used"])),
            read_counter(
                &self
                    .fair_locking_usage
                    .with_label_values(&["txn-effective"]),
            ),
        )
    }
}

/// 全局遥测指标单例；OnceLock 保证只初始化一次。
pub static TELEMETRY_METRICS: OnceLock<TelemetryMetrics> = OnceLock::new();

/// 惰性初始化并返回全局遥测指标引用。
pub fn init_telemetry_metrics() -> Result<&'static TelemetryMetrics, Error> {
    if let Some(metrics) = TELEMETRY_METRICS.get() {
        return Ok(metrics);
    }
    let metrics = TelemetryMetrics::new()?;
    let _ = TELEMETRY_METRICS.set(metrics);
    Ok(TELEMETRY_METRICS
        .get()
        .expect("telemetry metrics initialized"))
}

/// 取得已初始化的全局指标；初始化失败则 panic。
fn metrics() -> &'static TelemetryMetrics {
    init_telemetry_metrics().expect("valid telemetry metric descriptors")
}

#[allow(non_snake_case)]
/// 对应 Go 的 `InitTelemetryMetrics`。
pub fn InitTelemetryMetrics() -> Result<&'static TelemetryMetrics, Error> {
    init_telemetry_metrics()
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetCTECounter`。
pub fn GetCTECounter() -> CTEUsageCounter {
    metrics().cte_counter()
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetAccountLockCounter`。
pub fn GetAccountLockCounter() -> AccountLockCounter {
    metrics().account_lock_counter()
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetMultiSchemaCounter`。
pub fn GetMultiSchemaCounter() -> MultiSchemaChangeUsageCounter {
    MultiSchemaChangeUsageCounter {
        MultiSchemaChangeUsed: read_counter(&metrics().multi_schema_change),
    }
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetExchangePartitionCounter`。
pub fn GetExchangePartitionCounter() -> ExchangePartitionUsageCounter {
    ExchangePartitionUsageCounter {
        ExchangePartitionCnt: read_counter(&metrics().exchange_partition),
    }
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetTablePartitionCounter`。
pub fn GetTablePartitionCounter() -> TablePartitionUsageCounter {
    metrics().table_partition_counter()
}
#[allow(non_snake_case)]
/// 对应 Go 的 `ResetTablePartitionCounter`。
pub fn ResetTablePartitionCounter(
    previous: &TablePartitionUsageCounter,
) -> TablePartitionUsageCounter {
    metrics().reset_table_partition_counter(previous)
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetNonTransactionalStmtCounter`。
pub fn GetNonTransactionalStmtCounter() -> NonTransactionalStmtCounter {
    unsafe {
        crate::session::NonTransactionalDMLCount
            .as_ref()
            .map(|counter| {
                NonTransactionalStmtCounter::new(
                    read_counter(&counter.with_label_values(&["delete"])),
                    read_counter(&counter.with_label_values(&["update"])),
                    read_counter(&counter.with_label_values(&["insert"])),
                )
            })
            .unwrap_or_else(|| metrics().non_transactional_stmt_counter())
    }
}
#[allow(non_snake_case)]
/// 读取 Savepoint 语句节点计数（固定标签 Savepoint/空库/default 资源组）。
pub fn GetSavepointStmtCounter() -> i64 {
    unsafe {
        crate::executor::StmtNodeCounter
            .as_ref()
            .map(|counter| read_counter(&counter.with_label_values(&["Savepoint", "", "default"])))
            .unwrap_or_else(|| {
                read_counter(
                    &metrics()
                        .stmt_node
                        .with_label_values(&["Savepoint", "", "default"]),
                )
            })
    }
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetLazyPessimisticUniqueCheckSetCounter`。
pub fn GetLazyPessimisticUniqueCheckSetCounter() -> i64 {
    unsafe {
        crate::session::LazyPessimisticUniqueCheckSetCount
            .as_ref()
            .map(read_counter)
            .unwrap_or_else(|| read_counter(&metrics().lazy_pessimistic_unique_check_set))
    }
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetDDLUsageCounter`。
pub fn GetDDLUsageCounter() -> DDLUsageCounter {
    metrics().ddl_usage_counter()
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetIndexMergeCounter`。
pub fn GetIndexMergeCounter() -> IndexMergeUsageCounter {
    IndexMergeUsageCounter {
        IndexMergeUsed: read_counter(&metrics().index_merge),
    }
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetStoreBatchCoprCounter`。
pub fn GetStoreBatchCoprCounter() -> StoreBatchCoprCounter {
    metrics().store_batch_copr_counter()
}
#[allow(non_snake_case)]
/// 对应 Go 的 `GetFairLockingUsageCounter`。
pub fn GetFairLockingUsageCounter() -> FairLockingUsageCounter {
    unsafe {
        crate::session::FairLockingUsageCount
            .as_ref()
            .map(|counter| {
                FairLockingUsageCounter::new(
                    read_counter(
                        &counter.with_label_values(&[crate::session::LblFairLockingTxnUsed]),
                    ),
                    read_counter(
                        &counter.with_label_values(&[crate::session::LblFairLockingTxnEffective]),
                    ),
                )
            })
            .unwrap_or_else(|| metrics().fair_locking_usage_counter())
    }
}

/// 为遥测快照结构生成 `sub`/`Sub` 差分方法的宏。
macro_rules! snapshot {
    ($name:ident, $($field:ident),+ $(,)?) => {
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $name { $(pub $field: i64),+ }
        impl $name {
            pub fn sub(&self, rhs: &Self) -> Self {
                Self { $($field: self.$field - rhs.$field),+ }
            }
            #[allow(non_snake_case)]
            pub fn Sub(&self, rhs: &Self) -> Self { self.sub(rhs) }
        }
    };
}

snapshot!(
    CTEUsageCounter,
    NonRecursiveCTEUsed,
    RecursiveUsed,
    NonCTEUsed
);
impl CTEUsageCounter {
    /// 由非递归/递归/非 CTE 三个计数值构造快照。
    pub fn new(non_recursive: i64, recursive: i64, non_cte: i64) -> Self {
        Self {
            NonRecursiveCTEUsed: non_recursive,
            RecursiveUsed: recursive,
            NonCTEUsed: non_cte,
        }
    }
    pub fn values(&self) -> (i64, i64, i64) {
        (
            self.NonRecursiveCTEUsed,
            self.RecursiveUsed,
            self.NonCTEUsed,
        )
    }
}

snapshot!(AccountLockCounter, LockUser, UnlockUser, CreateOrAlterUser);
impl AccountLockCounter {
    /// 由锁定/解锁/创建或修改用户三个计数值构造快照。
    pub fn new(lock: i64, unlock: i64, create_or_alter: i64) -> Self {
        Self {
            LockUser: lock,
            UnlockUser: unlock,
            CreateOrAlterUser: create_or_alter,
        }
    }
    pub fn values(&self) -> (i64, i64, i64) {
        (self.LockUser, self.UnlockUser, self.CreateOrAlterUser)
    }
}

snapshot!(MultiSchemaChangeUsageCounter, MultiSchemaChangeUsed);
snapshot!(ExchangePartitionUsageCounter, ExchangePartitionCnt);
snapshot!(IndexMergeUsageCounter, IndexMergeUsed);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 分区表各类用法的用量快照（字段顺序与 Go 及 `table_partition` 数组一致）。
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
    /// 从长度为 15 的计数值数组构造快照。
    pub fn from_values(v: [i64; 15]) -> Self {
        Self {
            TablePartitionCnt: v[0],
            TablePartitionListCnt: v[1],
            TablePartitionRangeCnt: v[2],
            TablePartitionHashCnt: v[3],
            TablePartitionRangeColumnsCnt: v[4],
            TablePartitionRangeColumnsGt1Cnt: v[5],
            TablePartitionRangeColumnsGt2Cnt: v[6],
            TablePartitionRangeColumnsGt3Cnt: v[7],
            TablePartitionListColumnsCnt: v[8],
            TablePartitionMaxPartitionsCnt: v[9],
            TablePartitionCreateIntervalPartitionsCnt: v[10],
            TablePartitionAddIntervalPartitionsCnt: v[11],
            TablePartitionDropIntervalPartitionsCnt: v[12],
            TablePartitionComactCnt: v[13],
            TablePartitionReorganizePartitionCnt: v[14],
        }
    }
    pub fn values(&self) -> [i64; 15] {
        [
            self.TablePartitionCnt,
            self.TablePartitionListCnt,
            self.TablePartitionRangeCnt,
            self.TablePartitionHashCnt,
            self.TablePartitionRangeColumnsCnt,
            self.TablePartitionRangeColumnsGt1Cnt,
            self.TablePartitionRangeColumnsGt2Cnt,
            self.TablePartitionRangeColumnsGt3Cnt,
            self.TablePartitionListColumnsCnt,
            self.TablePartitionMaxPartitionsCnt,
            self.TablePartitionCreateIntervalPartitionsCnt,
            self.TablePartitionAddIntervalPartitionsCnt,
            self.TablePartitionDropIntervalPartitionsCnt,
            self.TablePartitionComactCnt,
            self.TablePartitionReorganizePartitionCnt,
        ]
    }
    /// 计算相对上一快照的增量；max-partitions 取差值与历史峰值的较大者。
    pub fn cal(&self, rhs: &Self) -> Self {
        let left = self.values();
        let right = rhs.values();
        let mut result = std::array::from_fn(|i| left[i] - right[i]);
        result[9] = result[9].max(right[9]);
        Self::from_values(result)
    }
    #[allow(non_snake_case)]
    pub fn Cal(&self, rhs: &Self) -> Self {
        self.cal(rhs)
    }
}

snapshot!(
    NonTransactionalStmtCounter,
    DeleteCount,
    UpdateCount,
    InsertCount
);
impl NonTransactionalStmtCounter {
    /// 由 delete/update/insert 三个计数值构造快照。
    pub fn new(delete: i64, update: i64, insert: i64) -> Self {
        Self {
            DeleteCount: delete,
            UpdateCount: update,
            InsertCount: insert,
        }
    }
    pub fn values(&self) -> (i64, i64, i64) {
        (self.DeleteCount, self.UpdateCount, self.InsertCount)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// DDL 特性用量快照（含元数据锁布尔位）。
pub struct DDLUsageCounter {
    pub AddIndexIngestUsed: i64,
    pub MetadataLockUsed: bool,
    pub FlashbackClusterUsed: i64,
    pub DistReorgUsed: i64,
}

impl DDLUsageCounter {
    /// 构造 DDL 用量快照；`metadata_lock` 差分时固定为 false。
    pub fn new(ingest: i64, metadata_lock: bool, flashback: i64, dist_reorg: i64) -> Self {
        Self {
            AddIndexIngestUsed: ingest,
            MetadataLockUsed: metadata_lock,
            FlashbackClusterUsed: flashback,
            DistReorgUsed: dist_reorg,
        }
    }
    pub fn sub(&self, rhs: &Self) -> Self {
        Self::new(
            self.AddIndexIngestUsed - rhs.AddIndexIngestUsed,
            false,
            self.FlashbackClusterUsed - rhs.FlashbackClusterUsed,
            self.DistReorgUsed - rhs.DistReorgUsed,
        )
    }
    #[allow(non_snake_case)]
    pub fn Sub(&self, rhs: &Self) -> Self {
        self.sub(rhs)
    }
    pub fn values(&self) -> (i64, bool, i64, i64) {
        (
            self.AddIndexIngestUsed,
            self.MetadataLockUsed,
            self.FlashbackClusterUsed,
            self.DistReorgUsed,
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// Store 批量 Coprocessor 用量快照。
pub struct StoreBatchCoprCounter {
    pub BatchSize: i32,
    pub BatchedQuery: i64,
    pub BatchedQueryTask: i64,
    pub BatchedCount: i64,
    pub BatchedFallbackCount: i64,
}

impl StoreBatchCoprCounter {
    /// 构造批量 Coprocessor 用量快照。
    pub fn new(batch_size: i32, query: i64, tasks: i64, batched: i64, fallback: i64) -> Self {
        Self {
            BatchSize: batch_size,
            BatchedQuery: query,
            BatchedQueryTask: tasks,
            BatchedCount: batched,
            BatchedFallbackCount: fallback,
        }
    }
    pub fn sub(&self, rhs: &Self) -> Self {
        Self::new(
            0,
            self.BatchedQuery - rhs.BatchedQuery,
            self.BatchedQueryTask - rhs.BatchedQueryTask,
            self.BatchedCount - rhs.BatchedCount,
            self.BatchedFallbackCount - rhs.BatchedFallbackCount,
        )
    }
    #[allow(non_snake_case)]
    pub fn Sub(&self, rhs: &Self) -> Self {
        self.sub(rhs)
    }
    pub fn values(&self) -> (i32, i64, i64, i64, i64) {
        (
            self.BatchSize,
            self.BatchedQuery,
            self.BatchedQueryTask,
            self.BatchedCount,
            self.BatchedFallbackCount,
        )
    }
}

snapshot!(
    FairLockingUsageCounter,
    TxnFairLockingUsed,
    TxnFairLockingEffective
);
impl FairLockingUsageCounter {
    /// 由事务级 used/effective 两个计数值构造快照。
    pub fn new(used: i64, effective: i64) -> Self {
        Self {
            TxnFairLockingUsed: used,
            TxnFairLockingEffective: effective,
        }
    }
    pub fn values(&self) -> (i64, i64) {
        (self.TxnFairLockingUsed, self.TxnFairLockingEffective)
    }
}
