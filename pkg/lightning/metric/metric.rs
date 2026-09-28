// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Lightning 导入过程的 Prometheus 指标定义与上下文。
//
// 定义表/引擎/chunk/SST 等状态标签常量，以及 `Common`/`Metrics` 两类指标集合；
// 提供注册/注销、计数读取与 `MetricContext` 上下文注入，对应 Go `metric` 包。

use crate::promutil;
use prometheus::core::Collector;
use prometheus::proto;
use std::collections::HashMap;
use std::sync::Arc;

/// 表处于待处理状态。
pub const TABLE_STATE_PENDING: &str = "pending";
/// 表数据已导入到后端。
pub const TABLE_STATE_IMPORTED: &str = "imported";
/// 表全部流程（含校验等）已完成。
pub const TABLE_STATE_COMPLETED: &str = "completed";

/// 总恢复量（字节/行等）标签值。
pub const STATE_TOTAL_RESTORE: &str = "total_restore";
/// 已恢复量标签值。
pub const STATE_RESTORED: &str = "restored";
/// 已写入量标签值。
pub const STATE_RESTORE_WRITTEN: &str = "written";
/// 已导入量标签值。
pub const STATE_IMPORTED: &str = "imported";
/// 已合并量标签值。
pub const STATE_MERGED: &str = "merged";

/// 进度总阶段标签。
pub const PROGRESS_PHASE_TOTAL: &str = "total";
/// 恢复（restore）阶段进度标签。
pub const PROGRESS_PHASE_RESTORE: &str = "restore";
/// 导入（import）阶段进度标签。
pub const PROGRESS_PHASE_IMPORT: &str = "import";

/// 表处理成功结果标签。
pub const TABLE_RESULT_SUCCESS: &str = "success";
/// 表处理失败结果标签。
pub const TABLE_RESULT_FAILURE: &str = "failure";

/// chunk 预估状态。
pub const CHUNK_STATE_ESTIMATED: &str = "estimated";
/// chunk 待处理状态。
pub const CHUNK_STATE_PENDING: &str = "pending";
/// chunk 运行中状态。
pub const CHUNK_STATE_RUNNING: &str = "running";
/// chunk 已完成状态。
pub const CHUNK_STATE_FINISHED: &str = "finished";
/// chunk 失败状态。
pub const CHUNK_STATE_FAILED: &str = "failed";

/// SST（Sorted String Table，有序键值文件）切分阶段。
pub const SST_PROCESS_SPLIT: &str = "split";
/// SST 写入阶段。
pub const SST_PROCESS_WRITE: &str = "write";
/// SST 摄入（ingest）到 TiKV 阶段。
pub const SST_PROCESS_INGEST: &str = "ingest";
/// 投递块类型：索引。
pub const BLOCK_DELIVER_KIND_INDEX: &str = "index";
/// 投递块类型：数据。
pub const BLOCK_DELIVER_KIND_DATA: &str = "data";

/// Prometheus 命名空间，所有 Lightning 指标共用。
const LIGHTNING_NAMESPACE: &str = "lightning";
/// 常量标签键值映射，对应 Go `prometheus.Labels`。
pub type Labels = HashMap<String, String>;

/// 构造带 namespace/subsystem/const_labels 的 Counter/Gauge Opts。
fn opts(
    namespace: &str,
    subsystem: &str,
    name: &str,
    help: &str,
    const_labels: &Labels,
) -> prometheus::Opts {
    prometheus::Opts::new(name, help)
        .namespace(namespace.to_owned())
        .subsystem(subsystem.to_owned())
        .const_labels(const_labels.clone())
}

/// 构造带分桶的 Histogram Opts。
fn histogram_opts(
    namespace: &str,
    subsystem: &str,
    name: &str,
    help: &str,
    const_labels: &Labels,
    buckets: Vec<f64>,
) -> prometheus::HistogramOpts {
    prometheus::HistogramOpts::new(name, help)
        .namespace(namespace.to_owned())
        .subsystem(subsystem.to_owned())
        .const_labels(const_labels.clone())
        .buckets(buckets)
}

/// 导入路径共用的 chunk/字节/行计数与行级耗时直方图集合。
#[derive(Clone)]
pub struct Common {
    pub chunk_counter: prometheus::CounterVec,
    pub bytes_counter: prometheus::CounterVec,
    pub rows_counter: prometheus::CounterVec,
    pub row_read_seconds_histogram: prometheus::Histogram,
    pub row_encode_seconds_histogram: prometheus::Histogram,
    pub block_deliver_seconds_histogram: prometheus::Histogram,
    pub block_deliver_bytes_histogram: prometheus::HistogramVec,
    pub block_deliver_kv_pairs_histogram: prometheus::HistogramVec,
}

/// 通过 Factory 创建 `Common` 中全部指标，标签与桶与 Go 保持一致。
pub fn new_common(
    factory: &dyn promutil::Factory,
    namespace: &str,
    subsystem: &str,
    const_labels: Labels,
) -> Common {
    Common {
        chunk_counter: factory.NewCounterVec(
            opts(
                namespace,
                subsystem,
                "chunks",
                "count number of chunks processed",
                &const_labels,
            ),
            vec!["state".to_owned()],
        ),
        bytes_counter: factory.NewCounterVec(
            opts(
                namespace,
                subsystem,
                "bytes",
                "count of total bytes",
                &const_labels,
            ),
            vec!["state".to_owned()],
        ),
        rows_counter: factory.NewCounterVec(
            opts(
                namespace,
                subsystem,
                "rows",
                "count of total rows",
                &const_labels,
            ),
            vec!["state".to_owned(), "table".to_owned()],
        ),
        row_read_seconds_histogram: factory.NewHistogram(histogram_opts(
            namespace,
            subsystem,
            "row_read_seconds",
            "time needed to parse a row(include time to read and decompress file)",
            &const_labels,
            prometheus::exponential_buckets(0.001, 3.1622776601683795, 7).unwrap(),
        )),
        row_encode_seconds_histogram: factory.NewHistogram(histogram_opts(
            namespace,
            subsystem,
            "row_encode_seconds",
            "time needed to encode a row",
            &const_labels,
            prometheus::exponential_buckets(0.001, 3.1622776601683795, 10).unwrap(),
        )),
        block_deliver_seconds_histogram: factory.NewHistogram(histogram_opts(
            namespace,
            subsystem,
            "block_deliver_seconds",
            "time needed to deliver a block",
            &const_labels,
            prometheus::exponential_buckets(0.001, 3.1622776601683795, 10).unwrap(),
        )),
        block_deliver_bytes_histogram: factory.NewHistogramVec(
            histogram_opts(
                namespace,
                subsystem,
                "block_deliver_bytes",
                "number of bytes being sent out to importer",
                &const_labels,
                prometheus::exponential_buckets(512.0, 2.0, 10).unwrap(),
            ),
            vec!["kind".to_owned()],
        ),
        block_deliver_kv_pairs_histogram: factory.NewHistogramVec(
            histogram_opts(
                namespace,
                subsystem,
                "block_deliver_kv_pairs",
                "number of KV pairs being sent out to importer",
                &const_labels,
                prometheus::exponential_buckets(1.0, 2.0, 10).unwrap(),
            ),
            vec!["kind".to_owned()],
        ),
    }
}

impl Common {
    /// 将 Common 内全部收集器注册到给定 Registry。
    pub fn register_to(&self, registry: &dyn promutil::Registry) {
        registry.MustRegister(vec![
            Box::new(self.chunk_counter.clone()),
            Box::new(self.bytes_counter.clone()),
            Box::new(self.rows_counter.clone()),
            Box::new(self.row_read_seconds_histogram.clone()),
            Box::new(self.row_encode_seconds_histogram.clone()),
            Box::new(self.block_deliver_seconds_histogram.clone()),
            Box::new(self.block_deliver_bytes_histogram.clone()),
            Box::new(self.block_deliver_kv_pairs_histogram.clone()),
        ]);
    }

    /// 从 Registry 逐个注销 Common 内收集器。
    pub fn unregister_from(&self, registry: &dyn promutil::Registry) {
        registry.Unregister(Box::new(self.chunk_counter.clone()));
        registry.Unregister(Box::new(self.bytes_counter.clone()));
        registry.Unregister(Box::new(self.rows_counter.clone()));
        registry.Unregister(Box::new(self.row_read_seconds_histogram.clone()));
        registry.Unregister(Box::new(self.row_encode_seconds_histogram.clone()));
        registry.Unregister(Box::new(self.block_deliver_seconds_histogram.clone()));
        registry.Unregister(Box::new(self.block_deliver_bytes_histogram.clone()));
        registry.Unregister(Box::new(self.block_deliver_kv_pairs_histogram.clone()));
    }
}

/// Lightning 全局指标：引擎/worker/表/SST/进度等，并内嵌 `Common`。
#[derive(Clone)]
pub struct Metrics {
    pub importer_engine_counter: prometheus::CounterVec,
    pub idle_workers_gauge: prometheus::GaugeVec,
    pub kv_encoder_counter: prometheus::CounterVec,
    pub table_counter: prometheus::CounterVec,
    pub processed_engine_counter: prometheus::CounterVec,
    pub import_seconds_histogram: prometheus::Histogram,
    pub chunk_parser_read_block_seconds_histogram: prometheus::Histogram,
    pub apply_worker_seconds_histogram: prometheus::HistogramVec,
    pub row_kv_deliver_seconds_histogram: prometheus::Histogram,
    pub row_read_bytes_histogram: prometheus::Histogram,
    pub checksum_seconds_histogram: prometheus::Histogram,
    pub sst_seconds_histogram: prometheus::HistogramVec,
    pub local_storage_usage_bytes_gauge: prometheus::GaugeVec,
    pub progress_gauge: prometheus::GaugeVec,
    pub common: Arc<Common>,
}

/// 使用默认 `lightning` 命名空间创建完整 `Metrics`。
pub fn new_metrics(factory: &dyn promutil::Factory) -> Metrics {
    let labels = Labels::new();
    Metrics {
        importer_engine_counter: factory.NewCounterVec(
            opts(
                LIGHTNING_NAMESPACE,
                "",
                "importer_engine",
                "counting open and closed importer engines",
                &labels,
            ),
            vec!["type".to_owned()],
        ),
        idle_workers_gauge: factory.NewGaugeVec(
            opts(
                LIGHTNING_NAMESPACE,
                "",
                "idle_workers",
                "counting idle workers",
                &labels,
            ),
            vec!["name".to_owned()],
        ),
        kv_encoder_counter: factory.NewCounterVec(
            opts(
                LIGHTNING_NAMESPACE,
                "",
                "kv_encoder",
                "counting kv open and closed kv encoder",
                &labels,
            ),
            vec!["type".to_owned()],
        ),
        table_counter: factory.NewCounterVec(
            opts(
                LIGHTNING_NAMESPACE,
                "",
                "tables",
                "count number of tables processed",
                &labels,
            ),
            vec!["state".to_owned(), "result".to_owned()],
        ),
        processed_engine_counter: factory.NewCounterVec(
            opts(
                LIGHTNING_NAMESPACE,
                "",
                "engines",
                "count number of engines processed",
                &labels,
            ),
            vec!["state".to_owned(), "result".to_owned()],
        ),
        import_seconds_histogram: factory.NewHistogram(histogram_opts(
            LIGHTNING_NAMESPACE,
            "",
            "import_seconds",
            "time needed to import a table",
            &labels,
            prometheus::exponential_buckets(0.125, 2.0, 6).unwrap(),
        )),
        chunk_parser_read_block_seconds_histogram: factory.NewHistogram(histogram_opts(
            LIGHTNING_NAMESPACE,
            "",
            "chunk_parser_read_block_seconds",
            "time needed for chunk parser read a block",
            &labels,
            prometheus::exponential_buckets(0.001, 3.1622776601683795, 10).unwrap(),
        )),
        apply_worker_seconds_histogram: factory.NewHistogramVec(
            histogram_opts(
                LIGHTNING_NAMESPACE,
                "",
                "apply_worker_seconds",
                "time needed to apply a worker",
                &labels,
                prometheus::exponential_buckets(0.001, 3.1622776601683795, 10).unwrap(),
            ),
            vec!["name".to_owned()],
        ),
        row_kv_deliver_seconds_histogram: factory.NewHistogram(histogram_opts(
            LIGHTNING_NAMESPACE,
            "",
            "row_kv_deliver_seconds",
            "time needed to send kvs to deliver loop",
            &labels,
            prometheus::exponential_buckets(0.001, 3.1622776601683795, 10).unwrap(),
        )),
        row_read_bytes_histogram: factory.NewHistogram(histogram_opts(
            LIGHTNING_NAMESPACE,
            "",
            "row_read_bytes",
            "number of bytes being read out from data source",
            &labels,
            prometheus::exponential_buckets(1024.0, 2.0, 8).unwrap(),
        )),
        checksum_seconds_histogram: factory.NewHistogram(histogram_opts(
            LIGHTNING_NAMESPACE,
            "",
            "checksum_seconds",
            "time needed to complete the checksum stage",
            &labels,
            prometheus::exponential_buckets(1.0, 2.2679331552660544, 10).unwrap(),
        )),
        sst_seconds_histogram: factory.NewHistogramVec(
            histogram_opts(
                LIGHTNING_NAMESPACE,
                "",
                "sst_seconds",
                "time needed to complete the sst operations",
                &labels,
                prometheus::exponential_buckets(1.0, 2.2679331552660544, 10).unwrap(),
            ),
            vec!["kind".to_owned()],
        ),
        local_storage_usage_bytes_gauge: factory.NewGaugeVec(
            opts(
                LIGHTNING_NAMESPACE,
                "",
                "local_storage_usage_bytes",
                "disk/memory size currently occupied by intermediate files in local backend",
                &labels,
            ),
            vec!["medium".to_owned()],
        ),
        progress_gauge: factory.NewGaugeVec(
            opts(
                LIGHTNING_NAMESPACE,
                "",
                "progress",
                "progress of lightning phase",
                &labels,
            ),
            vec!["phase".to_owned()],
        ),
        common: Arc::new(new_common(factory, LIGHTNING_NAMESPACE, "", labels)),
    }
}

impl Metrics {
    /// 先注册 Common，再注册 Metrics 自身的全部收集器。
    pub fn register_to(&self, registry: &dyn promutil::Registry) {
        self.common.register_to(registry);
        registry.MustRegister(vec![
            Box::new(self.importer_engine_counter.clone()),
            Box::new(self.idle_workers_gauge.clone()),
            Box::new(self.kv_encoder_counter.clone()),
            Box::new(self.table_counter.clone()),
            Box::new(self.processed_engine_counter.clone()),
            Box::new(self.import_seconds_histogram.clone()),
            Box::new(self.chunk_parser_read_block_seconds_histogram.clone()),
            Box::new(self.apply_worker_seconds_histogram.clone()),
            Box::new(self.row_kv_deliver_seconds_histogram.clone()),
            Box::new(self.row_read_bytes_histogram.clone()),
            Box::new(self.checksum_seconds_histogram.clone()),
            Box::new(self.sst_seconds_histogram.clone()),
            Box::new(self.local_storage_usage_bytes_gauge.clone()),
            Box::new(self.progress_gauge.clone()),
        ]);
    }

    /// 先注销 Common，再逐个注销 Metrics 自身收集器。
    pub fn unregister_from(&self, registry: &dyn promutil::Registry) {
        self.common.unregister_from(registry);
        registry.Unregister(Box::new(self.importer_engine_counter.clone()));
        registry.Unregister(Box::new(self.idle_workers_gauge.clone()));
        registry.Unregister(Box::new(self.kv_encoder_counter.clone()));
        registry.Unregister(Box::new(self.table_counter.clone()));
        registry.Unregister(Box::new(self.processed_engine_counter.clone()));
        registry.Unregister(Box::new(self.import_seconds_histogram.clone()));
        registry.Unregister(Box::new(
            self.chunk_parser_read_block_seconds_histogram.clone(),
        ));
        registry.Unregister(Box::new(self.apply_worker_seconds_histogram.clone()));
        registry.Unregister(Box::new(self.row_kv_deliver_seconds_histogram.clone()));
        registry.Unregister(Box::new(self.row_read_bytes_histogram.clone()));
        registry.Unregister(Box::new(self.checksum_seconds_histogram.clone()));
        registry.Unregister(Box::new(self.sst_seconds_histogram.clone()));
        registry.Unregister(Box::new(self.local_storage_usage_bytes_gauge.clone()));
        registry.Unregister(Box::new(self.progress_gauge.clone()));
    }

    /// 按状态与成功/失败结果递增表计数器。
    pub fn record_table_count(&self, status: &str, error: Option<&dyn std::error::Error>) {
        let result = if error.is_some() {
            TABLE_RESULT_FAILURE
        } else {
            TABLE_RESULT_SUCCESS
        };
        self.table_counter
            .with_label_values(&[status, result])
            .inc();
    }

    /// 按状态与成功/失败结果递增引擎计数器。
    pub fn record_engine_count(&self, status: &str, error: Option<&dyn std::error::Error>) {
        let result = if error.is_some() {
            TABLE_RESULT_FAILURE
        } else {
            TABLE_RESULT_SUCCESS
        };
        self.processed_engine_counter
            .with_label_values(&[status, result])
            .inc();
    }
}

/// 读取 Counter 当前累计值。
pub fn read_counter(counter: &prometheus::Counter) -> f64 {
    counter.get()
}

/// 收集 Histogram 的首个 Metric 快照；无数据时返回 None。
pub fn read_histogram(histogram: &prometheus::Histogram) -> Option<proto::Metric> {
    histogram
        .collect()
        .into_iter()
        .next()
        .and_then(|family| family.get_metric().first().cloned())
}

/// 判断 label 对中是否存在任一键值同时命中 `labels`（Go「任意匹配」语义）。
fn metric_has_label(label_pairs: &[proto::LabelPair], labels: &Labels) -> bool {
    label_pairs.iter().any(|label| {
        labels
            .get(label.name())
            .is_some_and(|value| value == label.value())
    })
}

/// 对 CounterVec 中标签任意匹配的样本求和，对齐 Go ReadCounter 聚合语义。
pub fn read_all_counters(metrics_vec: &prometheus::CounterVec, labels: &Labels) -> f64 {
    let mut sum = 0.0;
    for family in metrics_vec.collect() {
        for metric in family.get_metric() {
            if metric_has_label(metric.get_label(), labels) {
                sum += metric.counter.as_ref().map_or(0.0, proto::Counter::value);
            }
        }
    }
    sum
}

/// 读取 Histogram 的 sample_sum；无快照时返回 NaN。
pub fn read_histogram_sum(histogram: &prometheus::Histogram) -> f64 {
    let Some(metric) = read_histogram(histogram) else {
        return f64::NAN;
    };
    metric
        .histogram
        .as_ref()
        .map_or(f64::NAN, proto::Histogram::sample_sum)
}

/// 可挂载完整 Metrics 与/或 Common 的轻量上下文，对应 Go context 派生值。
#[derive(Clone, Default)]
pub struct MetricContext {
    metrics: Option<Arc<Metrics>>,
    common: Option<Arc<Common>>,
}

impl MetricContext {
    /// 返回空上下文（无指标），作为 background 起点。
    pub fn background() -> Self {
        Self::default()
    }
}

/// 将完整 Metrics（含 Common）注入上下文。
pub fn with_metric(mut ctx: MetricContext, metrics: Arc<Metrics>) -> MetricContext {
    ctx.common = Some(metrics.common.clone());
    ctx.metrics = Some(metrics);
    ctx
}

/// 仅注入 Common，不设置完整 Metrics。
pub fn with_common_metric(mut ctx: MetricContext, common: Arc<Common>) -> MetricContext {
    ctx.common = Some(common);
    ctx
}

/// 从上下文取出完整 Metrics（若有）。
pub fn from_context(ctx: &MetricContext) -> Option<&Arc<Metrics>> {
    ctx.metrics.as_ref()
}

/// 从上下文取出 Common（若有）。
pub fn get_common_metric(ctx: &MetricContext) -> Option<&Arc<Common>> {
    ctx.common.as_ref()
}
