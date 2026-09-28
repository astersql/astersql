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

// DDL（数据定义语言，如建表、加索引）相关 Prometheus 指标定义与初始化。
//
// 覆盖任务排队、处理耗时、schema 同步器（Syncer）、Owner 选举、回填（backfill）
// 进度，以及与 Lightning 共用的按 job 注册的指标集合。DDL 在分布式场景下通过
// Owner 协调多节点，本模块用 Gauge/Histogram/Counter 暴露各阶段观测点。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;
use crate::{metric, promutil};

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

/// 回填动态指标注册表：按表 ID 记录已产生的 type 标签集合。
// backfillMetricRegistry 对应 Go 的带锁注册表：table ID 映射到该表产生过的 type label 集合。
// 集合用于任务结束时删除所有动态指标序列，避免高基数标签长期滞留。
pub struct backfillMetricRegistry {
    /// 表 ID → 该表回填指标用过的 type label 集合。
    pub byTblID: HashMap<i64, HashSet<String>>,
}

impl backfillMetricRegistry {
    /// 登记某表产生的 type 标签，供任务结束时按表清理。
    // register 对应 Go 方法；外层 Mutex 已承担 Go r.mu 的临界区职责。
    pub fn register(&mut self, tableID: i64, typeLabel: String) {
        self.byTblID
            .entry(tableID)
            .or_insert_with(|| HashSet::with_capacity(8))
            .insert(typeLabel);
    }

    /// 移除该表全部标签并返回副本，供调用方删除 Prometheus 时序。
    // clear 在锁内移除整张表的集合，再把标签交给调用方逐项删除 Prometheus series。
    pub fn clear(&mut self, tableID: i64) -> Vec<String> {
        self.byTblID
            .remove(&tableID)
            .map(|labels| labels.into_iter().collect())
            .unwrap_or_default()
    }
}

/// 按 DDL job ID 缓存已注册的 Lightning 通用指标；仅容器初始化，不自动注册。
// registeredJobMetrics 对应 Go 的 Lightning common metrics 缓存；LazyLock 只初始化容器，不注册指标。
pub static registeredJobMetrics: LazyLock<Mutex<HashMap<i64, metric::Common>>> =
    LazyLock::new(|| Mutex::new(HashMap::with_capacity(64)));
/// 全局回填指标注册表，外层 Mutex 对应 Go 侧互斥保护。
pub static backfillMetricsRegistry: LazyLock<Mutex<backfillMetricRegistry>> = LazyLock::new(|| {
    Mutex::new(backfillMetricRegistry {
        byTblID: HashMap::with_capacity(64),
    })
});

// DDL 指标槽位逐一对应 Go 包变量；Option 表示 InitDDLMetrics 执行前尚未初始化。
/// 等待中的 DDL 任务数量 Gauge（按 type 标签）。
pub static mut JobsGauge: Option<prometheus::GaugeVec> = None;
/// 处理单个 DDL 任务耗时直方图。
pub static mut HandleJobHistogram: Option<prometheus::HistogramVec> = None;
/// 批量加索引数据处理耗时直方图。
pub static mut BatchAddIdxHistogram: Option<prometheus::HistogramVec> = None;

/// Syncer 操作标签：初始化。
pub const SyncerInit: &str = "init";
/// Syncer 操作类型：重启。
pub const SyncerRestart: &str = "restart";
/// Syncer 操作类型：清理。
pub const SyncerClear: &str = "clear";
/// Syncer 操作类型：重新 watch。
pub const SyncerRewatch: &str = "rewatch";
/// 全局状态 Syncer 初始化标签。
pub const StateSyncerInit: &str = "init_global_state";

/// 部署 Syncer 耗时直方图。
pub static mut DeploySyncerHistogram: Option<prometheus::HistogramVec> = None;
/// 更新本节点 schema 版本耗时直方图。
pub static mut UpdateSelfVersionHistogram: Option<prometheus::HistogramVec> = None;
/// Owner 更新全局 schema 版本的 type 标签。
pub const OwnerUpdateGlobalVersion: &str = "update_global_version";
/// Owner 检查各节点版本对齐的 type 标签。
pub const OwnerCheckAllVersions: &str = "check_all_versions";
/// 更新全局 DDL 状态的 type 标签。
pub const UpdateGlobalState: &str = "update_global_state";
/// Owner 处理 Syncer 相关操作耗时直方图。
pub static mut OwnerHandleSyncerHistogram: Option<prometheus::HistogramVec> = None;

/// Worker 添加 DDL 任务的 type 标签。
pub const WorkerAddDDLJob: &str = "add_job";
/// DDL Worker 各类操作耗时直方图（按 type/action/result）。
pub static mut DDLWorkerHistogram: Option<prometheus::HistogramVec> = None;

// DDLRunOneStep 及下列 Observer 对应 Go 的 run_job 分层耗时：
// run_job -> transit_one_step -> run_one_step，并包含 schema version、同步等待和 MDL 清理阶段。
/// Worker 执行单步 DDL 的 type 标签。
pub const DDLRunOneStep: &str = "run_one_step";
/// 等待 schema 同步完成的 type 标签。
pub const DDLWaitSchemaSynced: &str = "wait_schema_synced";
/// 递增 schema 版本阶段的预绑定 Observer。
pub static mut DDLIncrSchemaVerOpHist: Option<prometheus::Observer> = None;
/// 锁定 schema 版本阶段的预绑定 Observer。
pub static mut DDLLockSchemaVerOpHist: Option<prometheus::Observer> = None;
/// 运行整个 DDL job 的预绑定 Observer。
pub static mut DDLRunJobOpHist: Option<prometheus::Observer> = None;
/// 任务完成后收尾阶段的预绑定 Observer。
pub static mut DDLHandleJobDoneOpHist: Option<prometheus::Observer> = None;
/// 状态机单步迁移的预绑定 Observer。
pub static mut DDLTransitOneStepOpHist: Option<prometheus::Observer> = None;
/// 持有版本锁时长的预绑定 Observer。
pub static mut DDLLockVerDurationHist: Option<prometheus::Observer> = None;
/// 清理 MDL（Metadata Lock，元数据锁）信息的预绑定 Observer。
pub static mut DDLCleanMDLInfoHist: Option<prometheus::Observer> = None;
/// DDL 可重试错误计数。
pub static mut RetryableErrorCount: Option<prometheus::CounterVec> = None;

/// 创建 DDL 实例事件的 type 标签。
pub const CreateDDLInstance: &str = "create_ddl_instance";
/// 创建 DDL 子系统事件的 type 标签。
pub const CreateDDL: &str = "create_ddl";
/// 成为 DDL Owner 事件的 type 标签。
pub const DDLOwner: &str = "owner";
/// DDL Worker 创建/Owner 等生命周期计数。
pub static mut DDLCounter: Option<prometheus::CounterVec> = None;
/// 加索引回填总量计数（动态 type 标签）。
pub static mut BackfillTotalCounter: Option<prometheus::CounterVec> = None;
/// 回填百分比进度 Gauge（动态 type 标签）。
pub static mut BackfillProgressGauge: Option<prometheus::GaugeVec> = None;
/// 访问三张 DDL job 表的耗时直方图。
pub static mut DDLJobTableDuration: Option<prometheus::HistogramVec> = None;
/// 当前运行中的 DDL 任务数。
pub static mut DDLRunningJobCount: Option<prometheus::GaugeVec> = None;
/// 加索引扫描速率直方图。
pub static mut AddIndexScanRate: Option<prometheus::HistogramVec> = None;

/// 集中初始化全部 DDL 指标 collector，并派生常用阶段 Observer。
// InitDDLMetrics 对应 Go 的集中初始化，保留指标名、标签顺序、帮助文本和指数桶边界。
// 可变静态量只用于机械表达启动期赋值；正式 Rust 接线应采用 OnceLock 并保证只初始化一次。
pub fn InitDDLMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    unsafe {
        JobsGauge = Some(metricscommon::NewGaugeVec(
            prometheus::GaugeOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "waiting_jobs",
                Help: "Gauge of jobs.",
            },
            &[LblType],
        ));
        HandleJobHistogram = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "handle_job_duration_seconds",
                Help: "Bucketed histogram of processing time (s) of handle jobs",
                Buckets: prometheus::ExponentialBuckets(0.01, 2.0, 24), // 10ms ~ 24hours
            },
            &[LblType, LblResult],
        ));
        BatchAddIdxHistogram = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "batch_add_idx_duration_seconds",
                Help: "Bucketed histogram of processing time (s) of batch handle data",
                Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 28), // 1ms ~ 1.5days
            },
            &[LblType],
        ));

        // syncer/owner 操作都记录 result；deploy 与 owner handler 额外按 type 拆分。
        DeploySyncerHistogram = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "deploy_syncer_duration_seconds",
                Help: "Bucketed histogram of processing time (s) of deploy syncer",
                Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 20),
            },
            &[LblType, LblResult],
        ));
        UpdateSelfVersionHistogram = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "update_self_ver_duration_seconds",
                Help: "Bucketed histogram of processing time (s) of update self version",
                Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 20),
            },
            &[LblResult],
        ));
        OwnerHandleSyncerHistogram = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "owner_handle_syncer_duration_seconds",
                Help: "Bucketed histogram of processing time (s) of handle syncer",
                Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 20),
            },
            &[LblType, LblResult],
        ));
        DDLWorkerHistogram = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "worker_operation_duration_seconds",
                Help: "Bucketed histogram of processing time (s) of ddl worker operations",
                Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 28),
            },
            &[LblType, LblAction, LblResult],
        ));
        DDLCounter = Some(metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "worker_operation_total",
                Help: "Counter of creating ddl/worker and isowner.",
            },
            &[LblType],
        ));
        BackfillTotalCounter = Some(metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "add_index_total",
                Help: "Speed of add index",
            },
            &[LblType],
        ));
        BackfillProgressGauge = Some(metricscommon::NewGaugeVec(
            prometheus::GaugeOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "backfill_percentage_progress",
                Help: "Percentage progress of backfill",
            },
            &[LblType],
        ));
        DDLJobTableDuration = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "job_table_duration_seconds",
                Help: "Bucketed histogram of processing time (s) of the 3 DDL job tables",
                Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 20),
            },
            &[LblType],
        ));
        DDLRunningJobCount = Some(metricscommon::NewGaugeVec(
            prometheus::GaugeOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "running_job_count",
                Help: "Running DDL jobs count",
            },
            &[LblType],
        ));
        AddIndexScanRate = Some(metricscommon::NewHistogramVec(
            prometheus::HistogramOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "scan_rate",
                Help: "scan rate",
                Buckets: prometheus::ExponentialBuckets(0.05, 2.0, 20),
            },
            &[LblType],
        ));
        RetryableErrorCount = Some(metricscommon::NewCounterVec(
            prometheus::CounterOpts {
                Namespace: "tidb",
                Subsystem: "ddl",
                Name: "retryable_error_total",
                Help: "Retryable error count during ddl.",
            },
            &[LblType],
        ));

        // 这些 Observer 用固定的 "*" action/result 聚合短时间内多 DDL 的公共阶段，避免再增加 DDL type 标签。
        let worker = DDLWorkerHistogram
            .as_ref()
            .expect("DDLWorkerHistogram initialized");
        DDLIncrSchemaVerOpHist = Some(worker.WithLabelValues(&["incr_schema_ver", "*", "*"]));
        DDLLockSchemaVerOpHist = Some(worker.WithLabelValues(&["lock_schema_ver", "*", "*"]));
        DDLRunJobOpHist = Some(worker.WithLabelValues(&["run_job", "*", "*"]));
        DDLHandleJobDoneOpHist = Some(worker.WithLabelValues(&["handle_job_done", "*", "*"]));
        DDLTransitOneStepOpHist = Some(worker.WithLabelValues(&["transit_one_step", "*", "*"]));
        DDLLockVerDurationHist = Some(worker.WithLabelValues(&["lock_ver_duration", "*", "*"]));
        DDLCleanMDLInfoHist = Some(worker.WithLabelValues(&["clean_mdl_info", "*", "*"]));
    }
}

// 临时索引写入钩子对应 Go 中可被运行时替换的包变量；默认实现保持无副作用。
/// 记录一次临时索引写入的钩子（可运行时替换）。
pub static mut DDLAddOneTempIndexWrite: fn(u64, i64, bool) = |_, _, _| {};
/// 提交临时索引写入的钩子。
pub static mut DDLCommitTempIndexWrite: fn(u64) = |_| {};
/// 回滚临时索引写入的钩子。
pub static mut DDLRollbackTempIndexWrite: fn(u64) = |_| {};
/// 重置某表临时索引写入状态的钩子。
pub static mut DDLResetTempIndexWrite: fn(i64) = |_| {};
/// 清理会话相关临时索引写入的钩子。
pub static mut DDLClearTempIndexWrite: fn(u64) = |_| {};
/// 设置临时索引扫描与合并统计的钩子。
pub static mut DDLSetTempIndexScanAndMerge: fn(i64, u64, u64) = |_, _, _| {};

// Label constants 与 Go 完全对应，分别供 worker、回填进度和回填速率指标使用。
/// Worker 直方图的 action 标签名。
pub const LblAction: &str = "action";
/// 加索引回填的 type 标签前缀。
pub const LblAddIndex: &str = "add_index";
/// 合并临时索引回填的 type 标签前缀。
pub const LblAddIndexMerge: &str = "add_index_merge_tmp";
/// 改列回填的 type 标签前缀。
pub const LblModifyColumn: &str = "modify_column";
/// 重组分区回填的 type 标签前缀。
pub const LblReorgPartition: &str = "reorganize_partition";
/// 加索引速率标签。
pub const LblAddIdxRate: &str = "add_idx_rate";
/// 合并临时索引速率标签。
pub const LblMergeTmpIdxRate: &str = "merge_tmp_idx_rate";
/// 清理索引速率标签。
pub const LblCleanupIdxRate: &str = "cleanup_idx_rate";
/// 更新列速率标签。
pub const LblUpdateColRate: &str = "update_col_rate";
/// 重组分区速率标签。
pub const LblReorgPartitionRate: &str = "reorg_partition_rate";

/// 拼接重组（reorg）动态 type 标签：`label-schema-table[-column/index]`。
// generateReorgLabel 对应 Go strings.Builder 拼接：label-schema-table[-column/index]。
// 多列或多索引名称已由调用方用 "+" 连接，本函数不再拆分或转义。
pub fn generateReorgLabel(
    label: &str,
    schemaName: &str,
    tableName: &str,
    colOrIdxNames: &str,
) -> String {
    let extra = if colOrIdxNames.is_empty() {
        2
    } else {
        colOrIdxNames.len() + 3
    };
    let mut result =
        String::with_capacity(label.len() + schemaName.len() + tableName.len() + extra);
    result.push_str(label);
    result.push('-');
    result.push_str(schemaName);
    result.push('-');
    result.push_str(tableName);
    if !colOrIdxNames.is_empty() {
        result.push('-');
        result.push_str(colOrIdxNames);
    }
    result
}

/// 按表取得回填总量 Counter，并登记标签以便事后清理。
// GetBackfillTotalByTableID 生成动态 type label、登记清理索引，再取得对应 Counter。
pub fn GetBackfillTotalByTableID(
    tableID: i64,
    label: &str,
    schemaName: &str,
    tableName: &str,
    optionalColOrIdxName: &str,
) -> prometheus::Counter {
    let typeLabel = generateReorgLabel(label, schemaName, tableName, optionalColOrIdxName);
    backfillMetricsRegistry
        .lock()
        .unwrap()
        .register(tableID, typeLabel.clone());
    unsafe {
        BackfillTotalCounter
            .as_ref()
            .expect("BackfillTotalCounter initialized")
            .WithLabelValues(&[typeLabel.as_str()])
    }
}

/// 按表取得回填进度 Gauge，与 Counter 共享同一注册表。
// GetBackfillProgressByTableID 与 counter 路径共享同一注册表，确保进度 Gauge 也能按 table ID 清理。
pub fn GetBackfillProgressByTableID(
    tableID: i64,
    label: &str,
    schemaName: &str,
    tableName: &str,
    optionalColOrIdxName: &str,
) -> prometheus::Gauge {
    let typeLabel = generateReorgLabel(label, schemaName, tableName, optionalColOrIdxName);
    backfillMetricsRegistry
        .lock()
        .unwrap()
        .register(tableID, typeLabel.clone());
    unsafe {
        BackfillProgressGauge
            .as_ref()
            .expect("BackfillProgressGauge initialized")
            .WithLabelValues(&[typeLabel.as_str()])
    }
}

/// 清理指定表的全部动态回填指标时序，避免高基数标签泄漏。
// DDLClearBackfillMetrics 先在锁内取走标签集合，再在锁外删除 series，避免外部 Prometheus 调用扩大临界区。
pub fn DDLClearBackfillMetrics(tableID: i64) {
    let labels = backfillMetricsRegistry.lock().unwrap().clear(tableID);
    for typeLabel in labels {
        unsafe {
            BackfillProgressGauge
                .as_ref()
                .expect("BackfillProgressGauge initialized")
                .DeleteLabelValues(&[typeLabel.as_str()]);
            BackfillTotalCounter
                .as_ref()
                .expect("BackfillTotalCounter initialized")
                .DeleteLabelValues(&[typeLabel.as_str()]);
        }
    }
}

/// 是否仍有任何表登记了回填动态指标。
// DDLHasBackfillMetrics 对应 Go 的锁内非空检查。
pub fn DDLHasBackfillMetrics() -> bool {
    !backfillMetricsRegistry.lock().unwrap().byTblID.is_empty()
}

/// 测试用：返回某表已登记标签的副本。
// GetBackfillLabelsForTest 返回集合副本，避免测试修改真实注册表；缺失 table ID 时沿用空集合表达 nil map。
pub fn GetBackfillLabelsForTest(tableID: i64) -> HashSet<String> {
    backfillMetricsRegistry
        .lock()
        .unwrap()
        .byTblID
        .get(&tableID)
        .cloned()
        .unwrap_or_default()
}

/// 为 DDL job 注册（或复用）Lightning 通用指标集合。
// RegisterLightningCommonMetricsForDDL 在同一 job ID 上复用已注册对象，否则构造、注册并缓存。
// RegisterTo 是外部注册动作；这里只保留 Go 调用形状，本身不会触发真实注册。
pub fn RegisterLightningCommonMetricsForDDL(jobID: i64) -> metric::Common {
    let mut registry = registeredJobMetrics.lock().unwrap();
    if let Some(metrics) = registry.get(&jobID) {
        return metrics.clone();
    }
    let labels = prometheus::Labels::from([("job_id".to_owned(), jobID.to_string())]);
    let factory = promutil::NewDefaultFactory();
    let metrics = metric::new_common(factory.as_ref(), TiDB, "ddl", labels);
    metrics.register_to(&prometheus::DefaultRegisterer);
    registry.insert(jobID, metrics.clone());
    metrics
}

/// 注销并移除指定 job 的 Lightning 通用指标。
// UnregisterLightningCommonMetricsForDDL 对应 Go 的 nil 保护、注销和缓存删除流程。
// Option 表达 Go *metric.Common 可为 nil；注销与删除在同一互斥区内完成。
pub fn UnregisterLightningCommonMetricsForDDL(jobID: i64, metrics: Option<&metric::Common>) {
    let Some(metrics) = metrics else { return };
    let mut registry = registeredJobMetrics.lock().unwrap();
    metrics.unregister_from(&prometheus::DefaultRegisterer);
    registry.remove(&jobID);
}

/// 测试用：返回已注册 job 指标缓存快照。
// GetRegisteredJob 仅供测试使用，返回缓存快照而不是暴露受锁保护的原 map。
pub fn GetRegisteredJob() -> HashMap<i64, metric::Common> {
    registeredJobMetrics.lock().unwrap().clone()
}
