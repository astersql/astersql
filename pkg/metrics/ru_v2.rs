// Copyright 2026 PingCAP, Inc.
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

// RU v2（Request Unit 第二版）计费相关 Prometheus 指标。
//
// RU 将 CPU、读写、执行器开销等折算为统一请求单元，用于资源组配额与计费。
// 本文件构造计数器、热点标签缓存与按 executor/TiKV coprocessor 标签取句柄的辅助函数，
// 不执行实际计费或访问 TiKV。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 仅构造 Prometheus 指标描述与句柄，不会注册采集器、连接数据库、访问 TiKV 或执行请求计费。

pub const LblRUV2Unit: &str = "unit";
pub const LblRUV2UnitCPUWork: &str = "cpu_work";
pub const LblRUV2UnitScanBytes: &str = "scan_bytes";
pub const LblRUV2UnitNetBytes: &str = "net_bytes";
pub const LblRUV2UnitCrossAZNetBytes: &str = "cross_az_net_bytes";
pub const LblRUV2UnitFrontendCompileBytes: &str = "frontend_compile_bytes";
pub const LblRUV2UnitHashStateRows: &str = "hash_state_rows";
pub const LblRUV2UnitJoinOutputRows: &str = "join_output_rows";
pub const LblRUV2UnitWriteStatement: &str = "write_statement";
pub const LblRUV2UnitOperatorNum: &str = "operator_num";
pub const LblRUV2UnitWriteKeys: &str = "write_keys";
pub const LblRUV2UnitWriteBytes: &str = "write_bytes";

pub static mut RUV2Total: Option<prometheus::Counter> = None;
pub static mut RUV2TTLTotal: Option<prometheus::Counter> = None;
pub static mut RUV2BySQLType: Option<prometheus::CounterVec> = None;
pub static mut RUV2BySQLTypeDDL: Option<prometheus::Counter> = None;
pub static mut RUV2ByEngine: Option<prometheus::CounterVec> = None;
pub static mut RUV2ByEngineTiKV: Option<prometheus::Counter> = None;
pub static mut RUV2Unit: Option<prometheus::CounterVec> = None;
pub static mut RUV2Statements: Option<prometheus::CounterVec> = None;
static mut ruv2TiDB: Option<prometheus::Counter> = None;
static mut ruv2TiFlash: Option<prometheus::Counter> = None;
static mut ruv2Select: Option<prometheus::Counter> = None;
static mut ruv2Insert: Option<prometheus::Counter> = None;
static mut ruv2Replace: Option<prometheus::Counter> = None;
static mut ruv2Update: Option<prometheus::Counter> = None;
static mut ruv2Delete: Option<prometheus::Counter> = None;
static mut ruv2Commit: Option<prometheus::Counter> = None;
static mut ruv2Analyze: Option<prometheus::Counter> = None;
static mut ruv2Other: Option<prometheus::Counter> = None;

// RU v2 对外指标。Option 对应 Go 包变量在 InitRUV2Metrics 调用前的 nil/零值阶段。
/// 结果 Chunk 单元格累计数（Chunk 是列式批处理的基本数据块）。
pub static mut RUV2ResultChunkCells: Option<prometheus::Counter> = None;
/// 执行器 L1 层输入/输出计数（按类型标签）。
pub static mut RUV2ExecutorL1: Option<prometheus::CounterVec> = None;
/// 执行器 L2 层输入/输出计数（按类型标签）。
pub static mut RUV2ExecutorL2: Option<prometheus::CounterVec> = None;
/// 执行器 L3 层输入/输出计数（按类型标签）。
pub static mut RUV2ExecutorL3: Option<prometheus::CounterVec> = None;
/// L5 插入行数累计。
pub static mut RUV2ExecutorL5InsertRows: Option<prometheus::Counter> = None;
/// 计划构建（plan builder）执行次数。
pub static mut RUV2PlanCnt: Option<prometheus::Counter> = None;
/// 推导统计信息路径次数。
pub static mut RUV2PlanDeriveStatsPaths: Option<prometheus::Counter> = None;
/// 资源管理器读请求次数。
pub static mut RUV2ResourceManagerReadCnt: Option<prometheus::Counter> = None;
/// 资源管理器写请求次数。
pub static mut RUV2ResourceManagerWriteCnt: Option<prometheus::Counter> = None;
/// 提交写键数量。
pub static mut RUV2WriteKeys: Option<prometheus::Counter> = None;
/// 提交写大小影子计数（与 Go 侧 shadow counter 对应）。
pub static mut RUV2WriteSize: Option<prometheus::Counter> = None;
/// 会话解析器执行次数。
pub static mut RUV2SessionParserTotal: Option<prometheus::Counter> = None;
/// 事务次数。
pub static mut RUV2TxnCnt: Option<prometheus::Counter> = None;
/// TiKV KV 引擎缓存未命中次数。
pub static mut RUV2TiKVKVEngineCacheMiss: Option<prometheus::Counter> = None;
/// TiKV Coprocessor 执行器迭代次数（Coprocessor 在存储节点侧下推计算）。
pub static mut RUV2TiKVCoprocessorExecutorIterations: Option<prometheus::Counter> = None;
/// TiKV Coprocessor 响应字节数。
pub static mut RUV2TiKVCoprocessorResponseBytes: Option<prometheus::Counter> = None;
/// TiKV raftstore 写触发 write-batch 字节数。
pub static mut RUV2TiKVRaftstoreStoreWriteTriggerWB: Option<prometheus::Counter> = None;
/// TiKV storage batch-get 处理的键数。
pub static mut RUV2TiKVStorageProcessedKeysBatchGet: Option<prometheus::Counter> = None;
/// TiKV storage get 处理的键数。
pub static mut RUV2TiKVStorageProcessedKeysGet: Option<prometheus::Counter> = None;
/// TiKV Coprocessor 执行器工作量，按类型标签区分。
pub static mut RUV2TiKVCoprocessorWorkTotal: Option<prometheus::CounterVec> = None;

// Go 为热点标签预先取 Counter，避免每次执行路径重复查找 CounterVec 子指标。
// pub(crate) so package-internal tests can assert cache identity like Go's metrics package tests.
pub(crate) static mut ruv2ExecutorL1BatchPointGetExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL1PointGetExecutor: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL1LimitExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2ExpandExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2HashAggExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2HashJoinExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2HashJoinV1Exec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2HashJoinV2Exec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2IndexLookUpJoin: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2IndexLookUpMergeJoin: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2IndexNestedLoopHashJoin: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2IndexLookUpExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2IndexReaderExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2MemTableReaderExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2MergeJoinExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2ProjectionExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2SelectionExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2TableDualExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2TableReaderExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2TopNExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2UnionScanExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2SelectLockExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL2WindowExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL3SortExec: Option<prometheus::Counter> = None;
pub(crate) static mut ruv2ExecutorL3StreamAggExec: Option<prometheus::Counter> = None;
static mut ruv2TiKVCoprocessorWorkTotalBatchIndexScan: Option<prometheus::Counter> = None;
static mut ruv2TiKVCoprocessorWorkTotalBatchTableScan: Option<prometheus::Counter> = None;
static mut ruv2TiKVCoprocessorWorkTotalBatchSelection: Option<prometheus::Counter> = None;
static mut ruv2TiKVCoprocessorWorkTotalBatchTopN: Option<prometheus::Counter> = None;
static mut ruv2TiKVCoprocessorWorkTotalBatchLimit: Option<prometheus::Counter> = None;
static mut ruv2TiKVCoprocessorWorkTotalBatchSimpleAggr: Option<prometheus::Counter> = None;
static mut ruv2TiKVCoprocessorWorkTotalBatchFastHashAggr: Option<prometheus::Counter> = None;

// counter 对应 Go 中重复的 metricscommon.NewCounter(CounterOpts{...}) 构造形状。
/// 构造命名空间为 tidb、子系统为 ruv2 的 Counter。
fn counter(name: &'static str, help: &'static str) -> prometheus::Counter {
    metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: "ruv2",
        Name: name,
        Help: help,
        ..Default::default()
    })
}

// counter_vec 构造只带 LblType 的 CounterVec，保持全部 RU v2 向量指标的标签维度。
/// 构造仅含 `type` 标签的 CounterVec。
fn counter_vec(name: &'static str, help: &'static str) -> prometheus::CounterVec {
    metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "ruv2",
            Name: name,
            Help: help,
            ..Default::default()
        },
        &[LblType],
    )
}

// InitRUV2Metrics 按 Go 源码顺序初始化 RU v2 指标，最后建立热点标签缓存。
/// 初始化全部 RU v2 指标并预热热点 executor/coprocessor 标签缓存。
pub fn InitRUV2Metrics() {
    unsafe {
        RUV2TTLTotal = Some(counter(
            "ttl_ru_total",
            "Counter of RU v2 consumption from TTL user-table scans and deletes, including their commits; included in ru_total.",
        ));
        RUV2Total = Some(counter(
            "ru_total",
            "Counter of resource unit consumption for RU v2.",
        ));
        let sql = metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ruv2",
                Name: "ru_by_sql_type_total",
                Help: "Counter of resource unit consumption by SQL type for RU v2.",
                ..Default::default()
            },
            &[LblSQLType],
        );
        RUV2BySQLTypeDDL = Some(sql.WithLabelValues(&[LblSQLTypeDDL]));
        ruv2Select = Some(sql.WithLabelValues(&["select"]));
        ruv2Insert = Some(sql.WithLabelValues(&["insert"]));
        ruv2Replace = Some(sql.WithLabelValues(&["replace"]));
        ruv2Update = Some(sql.WithLabelValues(&["update"]));
        ruv2Delete = Some(sql.WithLabelValues(&["delete"]));
        ruv2Commit = Some(sql.WithLabelValues(&["commit"]));
        ruv2Analyze = Some(sql.WithLabelValues(&["analyze"]));
        ruv2Other = Some(sql.WithLabelValues(&["other"]));
        RUV2BySQLType = Some(sql);
        let engine = metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ruv2",
                Name: "ru_by_engine_total",
                Help: "Counter of resource unit consumption by engine for RU v2.",
                ..Default::default()
            },
            &[LblEngine],
        );
        ruv2TiDB = Some(engine.WithLabelValues(&["tidb"]));
        ruv2TiFlash = Some(engine.WithLabelValues(&[LblEngineTiFlash]));
        RUV2ByEngineTiKV = Some(engine.WithLabelValues(&[LblEngineTiKV]));
        RUV2ByEngine = Some(engine);
        RUV2Unit = Some(metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ruv2",
                Name: "unit_total",
                Help: "Counter of raw statement units for RU v2.",
                ..Default::default()
            },
            &[LblEngine, "opclass", LblRUV2Unit],
        ));
        RUV2Statements = Some(metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ruv2",
                Name: "statements_total",
                Help: "Counter of RU v2 calculation outcomes in full report mode; success with incomplete evidence remains best effort.",
                ..Default::default()
            },
            &["status", "reason"],
        ));
        RUV2ResultChunkCells = Some(counter(
            "result_chunk_cells",
            "Counter of result chunk cells for RU v2.",
        ));
        RUV2ExecutorL1 = Some(counter_vec(
            "executor_l1",
            "Counter of executor L1 input/output for RU v2.",
        ));
        RUV2ExecutorL2 = Some(counter_vec(
            "executor_l2",
            "Counter of executor L2 input/output for RU v2.",
        ));
        RUV2ExecutorL3 = Some(counter_vec(
            "executor_l3",
            "Counter of executor L3 input/output for RU v2.",
        ));
        RUV2ExecutorL5InsertRows = Some(counter(
            "executor_l5_insert_rows",
            "Counter of insert rows for RU v2.",
        ));
        RUV2PlanCnt = Some(counter(
            "plan_cnt",
            "Counter of plan builder executions for RU v2.",
        ));
        RUV2PlanDeriveStatsPaths = Some(counter(
            "plan_derive_stats_paths",
            "Counter of derive stats paths for RU v2.",
        ));
        RUV2ResourceManagerReadCnt = Some(counter(
            "resource_manager_read_cnt",
            "Counter of resource manager read requests for RU v2.",
        ));
        RUV2ResourceManagerWriteCnt = Some(counter(
            "resource_manager_write_cnt",
            "Counter of resource manager write requests for RU v2.",
        ));
        RUV2WriteKeys = Some(counter(
            "write_keys",
            "Counter of commit write keys for RU v2.",
        ));
        RUV2WriteSize = Some(counter(
            "write_size",
            "Shadow counter of commit write size for RU v2.",
        ));
        RUV2SessionParserTotal = Some(counter(
            "session_parser_total",
            "Counter of session parser executions for RU v2.",
        ));
        RUV2TxnCnt = Some(counter("txn_cnt", "Counter of transactions for RU v2."));
        RUV2TiKVKVEngineCacheMiss = Some(counter(
            "tikv_kv_engine_cache_miss",
            "Counter of TiKV KV engine cache miss for RU v2.",
        ));
        RUV2TiKVCoprocessorExecutorIterations = Some(counter(
            "tikv_coprocessor_executor_iterations",
            "Counter of TiKV coprocessor executor iterations for RU v2.",
        ));
        RUV2TiKVCoprocessorResponseBytes = Some(counter(
            "tikv_coprocessor_response_bytes",
            "Counter of TiKV coprocessor response bytes for RU v2.",
        ));
        RUV2TiKVRaftstoreStoreWriteTriggerWB = Some(counter(
            "tikv_raftstore_store_write_trigger_wb_bytes",
            "Counter of TiKV raftstore write trigger WB bytes for RU v2.",
        ));
        RUV2TiKVStorageProcessedKeysBatchGet = Some(counter(
            "tikv_storage_processed_keys_batch_get",
            "Counter of TiKV storage processed keys (batch get) for RU v2.",
        ));
        RUV2TiKVStorageProcessedKeysGet = Some(counter(
            "tikv_storage_processed_keys_get",
            "Counter of TiKV storage processed keys (get) for RU v2.",
        ));
        RUV2TiKVCoprocessorWorkTotal = Some(counter_vec(
            "tikv_coprocessor_executor_work_total",
            "Counter of TiKV coprocessor executor work for RU v2.",
        ));
        initRUV2CachedLabelCounters();
    }
}

/// Record RU totals by SQL type and execution engine using prebound counters.
pub fn AddRUV2Results(tikv_ru: f64, tidb_ru: f64, tiflash_ru: f64, total_ru: f64, sql_type: &str) {
    unsafe {
        let sql_counter = match sql_type {
            "select" => &ruv2Select,
            "insert" => &ruv2Insert,
            "replace" => &ruv2Replace,
            "update" => &ruv2Update,
            "delete" => &ruv2Delete,
            "commit" => &ruv2Commit,
            "analyze" => &ruv2Analyze,
            _ => &ruv2Other,
        };
        RUV2Total
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(total_ru);
        sql_counter
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(total_ru);
        RUV2ByEngineTiKV
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(tikv_ru);
        ruv2TiDB
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(tidb_ru);
        ruv2TiFlash
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(tiflash_ru);
    }
}

/// Record a completed DDL job as TiKV RU, matching the three Go counters.
pub fn AddDDLJobRU(ru: f64) {
    if ru <= 0.0 {
        return;
    }
    unsafe {
        RUV2Total
            .as_ref()
            .expect("RU v2 metrics initialized")
            .Add(ru);
        RUV2BySQLTypeDDL
            .as_ref()
            .expect("RU v2 SQL type metrics initialized")
            .Add(ru);
        RUV2ByEngineTiKV
            .as_ref()
            .expect("RU v2 engine metrics initialized")
            .Add(ru);
    }
}

// initRUV2CachedLabelCounters 对应 Go 的 WithLabelValues 预热过程；调用前要求三个 CounterVec 已初始化。
/// 为常见 executor 与 TiKV coprocessor 标签预先取出 Counter，避免热路径反复查表。
unsafe fn initRUV2CachedLabelCounters() {
    let l1 = RUV2ExecutorL1.as_ref().expect("RUV2ExecutorL1 initialized");
    ruv2ExecutorL1BatchPointGetExec = Some(l1.WithLabelValues(&["BatchPointGetExec"]));
    ruv2ExecutorL1PointGetExecutor = Some(l1.WithLabelValues(&["PointGetExecutor"]));
    ruv2ExecutorL1LimitExec = Some(l1.WithLabelValues(&["LimitExec"]));

    let l2 = RUV2ExecutorL2.as_ref().expect("RUV2ExecutorL2 initialized");
    ruv2ExecutorL2ExpandExec = Some(l2.WithLabelValues(&["ExpandExec"]));
    ruv2ExecutorL2HashAggExec = Some(l2.WithLabelValues(&["HashAggExec"]));
    ruv2ExecutorL2HashJoinExec = Some(l2.WithLabelValues(&["HashJoinExec"]));
    ruv2ExecutorL2HashJoinV1Exec = Some(l2.WithLabelValues(&["HashJoinV1Exec"]));
    ruv2ExecutorL2HashJoinV2Exec = Some(l2.WithLabelValues(&["HashJoinV2Exec"]));
    ruv2ExecutorL2IndexLookUpJoin = Some(l2.WithLabelValues(&["IndexLookUpJoin"]));
    ruv2ExecutorL2IndexLookUpMergeJoin = Some(l2.WithLabelValues(&["IndexLookUpMergeJoin"]));
    ruv2ExecutorL2IndexNestedLoopHashJoin = Some(l2.WithLabelValues(&["IndexNestedLoopHashJoin"]));
    ruv2ExecutorL2IndexLookUpExec = Some(l2.WithLabelValues(&["IndexLookUpExecutor"]));
    ruv2ExecutorL2IndexReaderExec = Some(l2.WithLabelValues(&["IndexReaderExecutor"]));
    ruv2ExecutorL2MemTableReaderExec = Some(l2.WithLabelValues(&["MemTableReaderExec"]));
    ruv2ExecutorL2MergeJoinExec = Some(l2.WithLabelValues(&["MergeJoinExec"]));
    ruv2ExecutorL2ProjectionExec = Some(l2.WithLabelValues(&["ProjectionExec"]));
    ruv2ExecutorL2SelectionExec = Some(l2.WithLabelValues(&["SelectionExec"]));
    ruv2ExecutorL2TableDualExec = Some(l2.WithLabelValues(&["TableDualExec"]));
    ruv2ExecutorL2TableReaderExec = Some(l2.WithLabelValues(&["TableReaderExecutor"]));
    ruv2ExecutorL2TopNExec = Some(l2.WithLabelValues(&["TopNExec"]));
    ruv2ExecutorL2UnionScanExec = Some(l2.WithLabelValues(&["UnionScanExec"]));
    ruv2ExecutorL2SelectLockExec = Some(l2.WithLabelValues(&["SelectLockExec"]));
    ruv2ExecutorL2WindowExec = Some(l2.WithLabelValues(&["WindowExec"]));

    let l3 = RUV2ExecutorL3.as_ref().expect("RUV2ExecutorL3 initialized");
    ruv2ExecutorL3SortExec = Some(l3.WithLabelValues(&["SortExec"]));
    ruv2ExecutorL3StreamAggExec = Some(l3.WithLabelValues(&["StreamAggExec"]));

    let work = RUV2TiKVCoprocessorWorkTotal
        .as_ref()
        .expect("work counter initialized");
    ruv2TiKVCoprocessorWorkTotalBatchIndexScan = Some(work.WithLabelValues(&["BatchIndexScan"]));
    ruv2TiKVCoprocessorWorkTotalBatchTableScan = Some(work.WithLabelValues(&["BatchTableScan"]));
    ruv2TiKVCoprocessorWorkTotalBatchSelection = Some(work.WithLabelValues(&["BatchSelection"]));
    ruv2TiKVCoprocessorWorkTotalBatchTopN = Some(work.WithLabelValues(&["BatchTopN"]));
    ruv2TiKVCoprocessorWorkTotalBatchLimit = Some(work.WithLabelValues(&["BatchLimit"]));
    ruv2TiKVCoprocessorWorkTotalBatchSimpleAggr = Some(work.WithLabelValues(&["BatchSimpleAggr"]));
    ruv2TiKVCoprocessorWorkTotalBatchFastHashAggr =
        Some(work.WithLabelValues(&["BatchFastHashAggr"]));
}

// cached_counter 复制缓存句柄；Prometheus Counter 本身是可克隆的共享句柄，不复制计数值。
/// 克隆已缓存的 Counter 句柄；未初始化时 panic。
unsafe fn cached_counter(counter: &Option<prometheus::Counter>) -> prometheus::Counter {
    counter.as_ref().expect("RU v2 metrics initialized").clone()
}

// RUV2ExecutorCounter 对已知 executor 标签返回缓存 Counter，未知标签沿用 Go 的动态 WithLabelValues 回退。
/// 按执行器层级与类型标签返回 RU v2 Counter；不支持的 level 返回 None。
pub fn RUV2ExecutorCounter(level: i32, label: &str) -> Option<prometheus::Counter> {
    unsafe {
        let counter = match (level, label) {
            (1, "BatchPointGetExec") => cached_counter(&ruv2ExecutorL1BatchPointGetExec),
            (1, "PointGetExecutor") => cached_counter(&ruv2ExecutorL1PointGetExecutor),
            (1, "LimitExec") => cached_counter(&ruv2ExecutorL1LimitExec),
            (1, other) => RUV2ExecutorL1.as_ref()?.WithLabelValues(&[other]),
            (2, "ExpandExec") => cached_counter(&ruv2ExecutorL2ExpandExec),
            (2, "HashAggExec") => cached_counter(&ruv2ExecutorL2HashAggExec),
            (2, "HashJoinExec") => cached_counter(&ruv2ExecutorL2HashJoinExec),
            (2, "HashJoinV1Exec") => cached_counter(&ruv2ExecutorL2HashJoinV1Exec),
            (2, "HashJoinV2Exec") => cached_counter(&ruv2ExecutorL2HashJoinV2Exec),
            (2, "IndexLookUpJoin") => cached_counter(&ruv2ExecutorL2IndexLookUpJoin),
            (2, "IndexLookUpMergeJoin") => cached_counter(&ruv2ExecutorL2IndexLookUpMergeJoin),
            (2, "IndexNestedLoopHashJoin") => {
                cached_counter(&ruv2ExecutorL2IndexNestedLoopHashJoin)
            }
            (2, "IndexLookUpExecutor") => cached_counter(&ruv2ExecutorL2IndexLookUpExec),
            (2, "IndexReaderExecutor") => cached_counter(&ruv2ExecutorL2IndexReaderExec),
            (2, "MemTableReaderExec") => cached_counter(&ruv2ExecutorL2MemTableReaderExec),
            (2, "MergeJoinExec") => cached_counter(&ruv2ExecutorL2MergeJoinExec),
            (2, "ProjectionExec") => cached_counter(&ruv2ExecutorL2ProjectionExec),
            (2, "SelectionExec") => cached_counter(&ruv2ExecutorL2SelectionExec),
            (2, "TableDualExec") => cached_counter(&ruv2ExecutorL2TableDualExec),
            (2, "TableReaderExecutor") => cached_counter(&ruv2ExecutorL2TableReaderExec),
            (2, "TopNExec") => cached_counter(&ruv2ExecutorL2TopNExec),
            (2, "UnionScanExec") => cached_counter(&ruv2ExecutorL2UnionScanExec),
            (2, "SelectLockExec") => cached_counter(&ruv2ExecutorL2SelectLockExec),
            (2, "WindowExec") => cached_counter(&ruv2ExecutorL2WindowExec),
            (2, other) => RUV2ExecutorL2.as_ref()?.WithLabelValues(&[other]),
            (3, "SortExec") => cached_counter(&ruv2ExecutorL3SortExec),
            (3, "StreamAggExec") => cached_counter(&ruv2ExecutorL3StreamAggExec),
            (3, other) => RUV2ExecutorL3.as_ref()?.WithLabelValues(&[other]),
            // Go 对不支持的 level 返回 nil；Rust 用 None 明确表达该分支。
            _ => return None,
        };
        Some(counter)
    }
}

// RUV2TiKVCoprocessorWorkTotalCounter 保留 TiKV coprocessor 已知标签缓存与未知标签回退。
/// 按 TiKV Coprocessor 工作类型标签返回计数器句柄。
pub fn RUV2TiKVCoprocessorWorkTotalCounter(label: &str) -> Option<prometheus::Counter> {
    unsafe {
        let counter = match label {
            "BatchIndexScan" => cached_counter(&ruv2TiKVCoprocessorWorkTotalBatchIndexScan),
            "BatchTableScan" => cached_counter(&ruv2TiKVCoprocessorWorkTotalBatchTableScan),
            "BatchSelection" => cached_counter(&ruv2TiKVCoprocessorWorkTotalBatchSelection),
            "BatchTopN" => cached_counter(&ruv2TiKVCoprocessorWorkTotalBatchTopN),
            "BatchLimit" => cached_counter(&ruv2TiKVCoprocessorWorkTotalBatchLimit),
            "BatchSimpleAggr" => cached_counter(&ruv2TiKVCoprocessorWorkTotalBatchSimpleAggr),
            "BatchFastHashAggr" => cached_counter(&ruv2TiKVCoprocessorWorkTotalBatchFastHashAggr),
            other => RUV2TiKVCoprocessorWorkTotal
                .as_ref()?
                .WithLabelValues(&[other]),
        };
        Some(counter)
    }
}
