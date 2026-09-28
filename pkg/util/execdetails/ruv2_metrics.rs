// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 语句级 RU v2 指标：采集、合并、快照与格式化。
//
// 对应 Go `ruv2_metrics.go`。热路径计数器用独立原子字段，冷门标签落入 extra map；
// bypass 为真时跳过累加。RU 是资源组计费单位。

// 本文件对照 pkg/util/execdetails/ruv2_metrics.go 实现 statement 级 RU v2
// 指标的采集、合并、快照和格式化逻辑。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Mutex;

// ruv2MetricsKeyType 对应 Go 的空结构体 context key。
#[derive(Clone, Copy)]
/// Context 中存放 RUv2 指标的键类型。
pub struct ruv2MetricsKeyType;

// RUV2Weights contains the TiDB-side RU v2 weights needed to calculate scaled
// statement RU values.
#[derive(Clone, Copy, Default, PartialEq)]
/// TiDB 侧计算语句 RU 所需的 RUv2 权重。
pub struct RUV2Weights {
    pub RUScale: f64,
    pub ResultChunkCells: f64,
    pub ExecutorL1: f64,
    pub ExecutorL2: f64,
    pub ExecutorL3: f64,
    pub ExecutorL5InsertRows: f64,
    pub PlanCnt: f64,
    pub PlanDeriveStatsPaths: f64,
    pub ResourceManagerReadCnt: f64,
    pub ResourceManagerWriteCnt: f64,
    pub WriteKeys: f64,
    pub SessionParserTotal: f64,
    pub TxnCnt: f64,
}

// RUV2MetricsCtxKey is used to carry statement-level RUv2 metrics in context.Context.
/// 将语句级 RUv2 指标挂入 Context 的键。
pub static RUV2MetricsCtxKey: ruv2MetricsKeyType = ruv2MetricsKeyType;

// RUV2MetricsFromContext returns the RUv2 metrics stored in ctx.
/// 从 Context 取出 RUv2 指标。
pub fn RUV2MetricsFromContext(ctx: &context::Context) -> Option<RUV2Metrics> {
    if let Some(stmtDetails) = ctx.value::<StmtExecDetails>(StmtExecDetailKey) {
        if let Some(metrics) = stmtDetails.getRUV2Metrics() {
            return Some(metrics);
        }
    }
    // Keep the standalone context key as the fallback path for callers that
    // intentionally inherit RUv2 metrics into a context without StmtExecDetails.
    ctx.value::<RUV2Metrics>(RUV2MetricsCtxKey)
}

// UpdateRUV2MetricsFromRUV2 adds raw RUv2 counters into the statement-level metrics snapshot.
/// 把原始 RUv2 计数累加进语句级指标快照。
pub fn UpdateRUV2MetricsFromRUV2(m: Option<&RUV2Metrics>, ru: Option<&kvrpcpb::Ruv2>) {
    let (Some(m), Some(ru)) = (m, ru) else {
        return;
    };
    if m.Bypass() {
        return;
    }
    m.applyRawCounters(ru);
}

impl RUV2Metrics {
    // applyRawCounters writes ru into m. Caller must check Bypass.
    fn applyRawCounters(&self, ru: &kvrpcpb::Ruv2) {
        if ru.get_read_rpc_count() != 0 {
            metrics::RUV2ResourceManagerReadCnt.Add(ru.get_read_rpc_count() as f64);
            self.resourceManagerReadCnt.fetch_add(ru.get_read_rpc_count() as i64, Ordering::Relaxed);
        }
        if ru.get_kv_engine_cache_miss() != 0 {
            metrics::RUV2TiKVKVEngineCacheMiss.Add(ru.get_kv_engine_cache_miss() as f64);
            self.tikvKvEngineCacheMiss.fetch_add(ru.get_kv_engine_cache_miss() as i64, Ordering::Relaxed);
        }
        if ru.get_storage_processed_keys_batch_get() != 0 {
            metrics::RUV2TiKVStorageProcessedKeysBatchGet.Add(ru.get_storage_processed_keys_batch_get() as f64);
            self.tikvStorageProcessedKeysBatchGet.fetch_add(ru.get_storage_processed_keys_batch_get() as i64, Ordering::Relaxed);
        }
        if ru.get_storage_processed_keys_get() != 0 {
            metrics::RUV2TiKVStorageProcessedKeysGet.Add(ru.get_storage_processed_keys_get() as f64);
            self.tikvStorageProcessedKeysGet.fetch_add(ru.get_storage_processed_keys_get() as i64, Ordering::Relaxed);
        }

        // Go 通过 ensureExtra 延迟创建 cold counters；同样只在需要时进入 extra。
        if ru.get_write_rpc_count() != 0 {
            metrics::RUV2ResourceManagerWriteCnt.Add(ru.get_write_rpc_count() as f64);
            self.with_extra(|extra| extra.resourceManagerWriteCnt.fetch_add(ru.get_write_rpc_count() as i64, Ordering::Relaxed));
        }
        if ru.get_coprocessor_executor_iterations() != 0 {
            metrics::RUV2TiKVCoprocessorExecutorIterations.Add(ru.get_coprocessor_executor_iterations() as f64);
            self.with_extra(|extra| extra.tikvCoprocessorExecutorIterations.fetch_add(ru.get_coprocessor_executor_iterations() as i64, Ordering::Relaxed));
        }
        if ru.get_coprocessor_response_bytes() != 0 {
            metrics::RUV2TiKVCoprocessorResponseBytes.Add(ru.get_coprocessor_response_bytes() as f64);
            self.with_extra(|extra| extra.tikvCoprocessorResponseBytes.fetch_add(ru.get_coprocessor_response_bytes() as i64, Ordering::Relaxed));
        }
        if ru.get_raftstore_store_write_trigger_wb_bytes() != 0 {
            metrics::RUV2TiKVRaftstoreStoreWriteTriggerWB.Add(ru.get_raftstore_store_write_trigger_wb_bytes() as f64);
            self.with_extra(|extra| extra.tikvRaftstoreStoreWriteTriggerWB.fetch_add(ru.get_raftstore_store_write_trigger_wb_bytes() as i64, Ordering::Relaxed));
        }
        { let inputs = ru.get_executor_inputs();
            // addWork 对应 Go 闭包：过滤 0 值、上报 Prometheus，再累加 label counter。
            let addWork = |label: &str, v: u64| {
                if v == 0 {
                    return;
                }
                metrics::RUV2TiKVCoprocessorWorkTotalCounter(label).Add(v as f64);
                self.with_extra(|extra| addRUV2ExtraLabelCounter(&extra.tikvCoprocessorWorkTotal, label, v as i64));
            };
            addWork("BatchIndexScan", inputs.get_tikv_coprocessor_executor_work_total_batch_index_scan());
            addWork("BatchTableScan", inputs.get_tikv_coprocessor_executor_work_total_batch_table_scan());
            addWork("BatchSelection", inputs.get_tikv_coprocessor_executor_work_total_batch_selection());
            addWork("BatchTopN", inputs.get_tikv_coprocessor_executor_work_total_batch_top_n());
            addWork("BatchLimit", inputs.get_tikv_coprocessor_executor_work_total_batch_limit());
            addWork("BatchSimpleAggr", inputs.get_tikv_coprocessor_executor_work_total_batch_simple_aggr());
            addWork("BatchFastHashAggr", inputs.get_tikv_coprocessor_executor_work_total_batch_fast_hash_aggr());
        }
    }
}

// SyncRUV2MetricsFromRUDetails drains the raw RUv2 counters accumulated in
// RUDetails since the last drain and adds them into the statement-level metrics.
// It is safe to call multiple times; each call transfers only the delta.
/// 排空 RUDetails 中自上次以来的原始 RUv2 增量并写入指标。
pub fn SyncRUV2MetricsFromRUDetails(metrics: Option<&RUV2Metrics>, ruDetails: Option<&tikvutil::RUDetails>) {
    let (Some(metrics), Some(ruDetails)) = (metrics, ruDetails) else {
        return;
    };
    if metrics.Bypass() {
        return;
    }
    UpdateRUV2MetricsFromRUV2(Some(metrics), Some(&ruDetails.DrainRUV2()));
}

// UpdateRUV2MetricsFromCommitDetails adds commit write counters into RUv2 metrics.
/// 把提交阶段的写键/写字节计入 RUv2 指标。
pub fn UpdateRUV2MetricsFromCommitDetails(metrics: Option<&RUV2Metrics>, commitDetails: Option<&tikvutil::CommitDetails>) {
    let (Some(metrics), Some(commitDetails)) = (metrics, commitDetails) else {
        return;
    };
    if metrics.Bypass() {
        return;
    }
    if commitDetails.WriteKeys != 0 {
        metrics.AddWriteKeys(commitDetails.WriteKeys as i64);
    }
    if commitDetails.WriteSize != 0 {
        metrics.AddWriteSize(commitDetails.WriteSize as i64);
    }
}

// RUV2Metrics stores statement-level RUv2 metrics.
// Go 对 hot counters 直接使用 int64 + atomic，对 cold counters 使用 atomic.Pointer 延迟分配；这里用 Mutex<Option<...>> 表达。
/// 语句级 RUv2 指标容器（热路径原子字段 + 冷门 extra）。
pub struct RUV2Metrics {
    bypass: AtomicBool,
    resultChunkCells: AtomicI64,
    executorL1: ruv2ExecutorL1Counter,
    planCnt: AtomicI64,
    sessionParserTotal: AtomicI64,
    txnCnt: AtomicI64,
    resourceManagerReadCnt: AtomicI64,
    tikvKvEngineCacheMiss: AtomicI64,
    tikvStorageProcessedKeysBatchGet: AtomicI64,
    tikvStorageProcessedKeysGet: AtomicI64,
    extra: Mutex<Option<ruv2MetricsExtra>>,
}

// ruv2MetricsExtra 对应 Go 中通过 atomic.Pointer 延迟挂载的冷门 RUv2 计数字段。
#[derive(Default)]
/// 经延迟分配挂载的冷门 RUv2 计数字段。
struct ruv2MetricsExtra {
    executorL2: ruv2ExtraLabelCounter,
    executorL3: ruv2ExtraLabelCounter,
    executorL5InsertRows: AtomicI64,
    planDeriveStatsPaths: AtomicI64,
    resourceManagerWriteCnt: AtomicI64,
    writeKeys: AtomicI64,
    writeSize: AtomicI64,
    tikvCoprocessorExecutorIterations: AtomicI64,
    tikvCoprocessorResponseBytes: AtomicI64,
    tikvRaftstoreStoreWriteTriggerWB: AtomicI64,
    tikvCoprocessorWorkTotal: ruv2ExtraLabelCounter,
}

impl Default for RUV2Metrics {
    fn default() -> Self {
        Self {
            bypass: AtomicBool::new(false),
            resultChunkCells: AtomicI64::new(0),
            executorL1: ruv2ExecutorL1Counter::default(),
            planCnt: AtomicI64::new(0),
            sessionParserTotal: AtomicI64::new(0),
            txnCnt: AtomicI64::new(0),
            resourceManagerReadCnt: AtomicI64::new(0),
            tikvKvEngineCacheMiss: AtomicI64::new(0),
            tikvStorageProcessedKeysBatchGet: AtomicI64::new(0),
            tikvStorageProcessedKeysGet: AtomicI64::new(0),
            extra: Mutex::new(None),
        }
    }
}

impl RUV2Metrics {
    // loadExtra 对应 Go 的 atomic.Pointer.Load。
    fn loadExtra<R>(&self, f: impl FnOnce(&ruv2MetricsExtra) -> R) -> Option<R> {
        let guard = self.extra.lock().expect("ruv2 extra lock poisoned");
        guard.as_ref().map(f)
    }

    // ensureExtra 对应 Go 的 CompareAndSwap(nil, extra) 延迟初始化。
    fn with_extra<R>(&self, f: impl FnOnce(&ruv2MetricsExtra) -> R) -> R {
        let mut guard = self.extra.lock().expect("ruv2 extra lock poisoned");
        if guard.is_none() {
            *guard = Some(ruv2MetricsExtra::default());
        }
        f(guard.as_ref().expect("extra initialized"))
    }
}

// NewRUV2Metrics creates a new RUv2 metrics container.
/// 创建空的 RUv2 指标容器。
pub fn NewRUV2Metrics() -> RUV2Metrics {
    RUV2Metrics::default()
}

impl RUV2Metrics {
    // SetBypass marks whether statement-level RU accounting should be skipped.
    pub fn SetBypass(&self, enabled: bool) {
        self.bypass.store(enabled, Ordering::Relaxed);
    }

    // Bypass returns whether statement-level RU accounting should be skipped.
    pub fn Bypass(&self) -> bool {
        self.bypass.load(Ordering::Relaxed)
    }

    // AddResultChunkCells records result cells written by the current statement.
    pub fn AddResultChunkCells(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2ResultChunkCells.Add(delta as f64);
        self.resultChunkCells.fetch_add(delta, Ordering::Relaxed);
    }

    // AddExecutorMetric records a statement-level executor metric for the given RUv2 level.
    pub fn AddExecutorMetric(&self, level: i32, label: &str, delta: i64) {
        if self.Bypass() || delta == 0 || label.is_empty() {
            return;
        }
        if let Some(counter) = metrics::RUV2ExecutorCounter(level, label) {
            counter.Add(delta as f64);
        }
        match level {
            1 => self.executorL1.add(label, delta),
            2 => self.with_extra(|extra| addRUV2ExtraLabelCounter(&extra.executorL2, label, delta)),
            3 => self.with_extra(|extra| addRUV2ExtraLabelCounter(&extra.executorL3, label, delta)),
            _ => {}
        }
    }
}

// execL1Kind selects one of the hot L1 executor counter fields; execL1None means none.
#[derive(Clone, Copy, PartialEq, Eq)]
/// L1 执行器热标签种类；None 表示非热路径。
pub enum execL1Kind {
    execL1None,
    execL1BatchPointGet,
    execL1PointGet,
    execL1Limit,
}

impl Default for execL1Kind {
    fn default() -> Self {
        execL1Kind::execL1None
    }
}

// ExecutorMetricRecorder is a pre-resolved counter for one hot L1 executor metric.
// The zero value records nothing; callers must check Available before Record.
#[derive(Default)]
/// 预解析的 L1 热标签计数器；零值不记录。
pub struct ExecutorMetricRecorder {
    counter: Option<prometheus::Counter>,
    kind: execL1Kind,
}

impl ExecutorMetricRecorder {
    // Available reports whether this recorder was resolved.
    pub fn Available(&self) -> bool {
        self.kind != execL1Kind::execL1None
    }

    // Record applies delta. Caller must ensure m is non-nil and not bypassed.
    pub fn Record(&self, m: &RUV2Metrics, delta: i64) {
        if let Some(counter) = self.counter.as_ref() {
            counter.Add(delta as f64);
        }
        if let Some(field) = m.executorL1.fieldByKind(self.kind) {
            field.fetch_add(delta, Ordering::Relaxed);
        }
    }
}

// ResolveExecutorMetric returns a pre-resolved recorder for hot L1 executor
// labels, or the zero recorder for everything else.
/// 解析热 L1 标签为 Recorder，其余返回零值 Recorder。
pub fn ResolveExecutorMetric(level: i32, label: &str) -> ExecutorMetricRecorder {
    if level != 1 {
        return ExecutorMetricRecorder::default();
    }
    let kind = execL1KindForLabel(label);
    if kind == execL1Kind::execL1None {
        return ExecutorMetricRecorder::default();
    }
    let Some(counter) = metrics::RUV2ExecutorCounter(level, label) else {
        return ExecutorMetricRecorder::default();
    };
    ExecutorMetricRecorder {
        counter: Some(counter),
        kind,
    }
}

// execL1KindForLabel 对应 Go 的 hot label 到枚举映射。
/// 将热标签名映射为 execL1Kind。
fn execL1KindForLabel(label: &str) -> execL1Kind {
    match label {
        ruv2LabelBatchPointGetExec => execL1Kind::execL1BatchPointGet,
        ruv2LabelPointGetExecutor => execL1Kind::execL1PointGet,
        ruv2LabelLimitExec => execL1Kind::execL1Limit,
        _ => execL1Kind::execL1None,
    }
}

impl RUV2Metrics {
    // AddExecutorL5InsertRows records insert rows multiplied by inserted column count for RUv2 accounting.
    pub fn AddExecutorL5InsertRows(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2ExecutorL5InsertRows.Add(delta as f64);
        self.with_extra(|extra| extra.executorL5InsertRows.fetch_add(delta, Ordering::Relaxed));
    }

    // AddPlanCnt records plan builder invocations for the current statement.
    pub fn AddPlanCnt(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2PlanCnt.Add(delta as f64);
        self.planCnt.fetch_add(delta, Ordering::Relaxed);
    }

    // AddPlanDeriveStatsPaths records derived stats paths for the current statement.
    pub fn AddPlanDeriveStatsPaths(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2PlanDeriveStatsPaths.Add(delta as f64);
        self.with_extra(|extra| extra.planDeriveStatsPaths.fetch_add(delta, Ordering::Relaxed));
    }

    // AddSessionParserTotal records parser executions for the current statement.
    pub fn AddSessionParserTotal(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2SessionParserTotal.Add(delta as f64);
        self.sessionParserTotal.fetch_add(delta, Ordering::Relaxed);
    }

    // AddTxnCnt records transaction completions attributed to the current statement.
    pub fn AddTxnCnt(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2TxnCnt.Add(delta as f64);
        self.txnCnt.fetch_add(delta, Ordering::Relaxed);
    }

    // AddResourceManagerReadCnt records TiKV read RPCs charged to resource management.
    pub fn AddResourceManagerReadCnt(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2ResourceManagerReadCnt.Add(delta as f64);
        self.resourceManagerReadCnt.fetch_add(delta, Ordering::Relaxed);
    }

    // AddResourceManagerWriteCnt records TiKV write RPCs charged to resource management.
    pub fn AddResourceManagerWriteCnt(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2ResourceManagerWriteCnt.Add(delta as f64);
        self.with_extra(|extra| extra.resourceManagerWriteCnt.fetch_add(delta, Ordering::Relaxed));
    }

    // AddWriteKeys records commit write keys for RUv2 accounting.
    pub fn AddWriteKeys(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2WriteKeys.Add(delta as f64);
        self.with_extra(|extra| extra.writeKeys.fetch_add(delta, Ordering::Relaxed));
    }

    // AddWriteSize records commit write size for RUv2 shadow accounting.
    pub fn AddWriteSize(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2WriteSize.Add(delta as f64);
        self.with_extra(|extra| extra.writeSize.fetch_add(delta, Ordering::Relaxed));
    }

    // AddTiKVKVEngineCacheMiss records TiKV kv_engine_cache_miss counters from ExecDetailsV2.
    pub fn AddTiKVKVEngineCacheMiss(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2TiKVKVEngineCacheMiss.Add(delta as f64);
        self.tikvKvEngineCacheMiss.fetch_add(delta, Ordering::Relaxed);
    }

    // AddTiKVCoprocessorExecutorIterations records TiKV coprocessor iteration counters.
    pub fn AddTiKVCoprocessorExecutorIterations(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2TiKVCoprocessorExecutorIterations.Add(delta as f64);
        self.with_extra(|extra| extra.tikvCoprocessorExecutorIterations.fetch_add(delta, Ordering::Relaxed));
    }

    // AddTiKVCoprocessorResponseBytes records TiKV coprocessor response bytes.
    pub fn AddTiKVCoprocessorResponseBytes(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2TiKVCoprocessorResponseBytes.Add(delta as f64);
        self.with_extra(|extra| extra.tikvCoprocessorResponseBytes.fetch_add(delta, Ordering::Relaxed));
    }

    // AddTiKVRaftstoreStoreWriteTriggerWB records TiKV raftstore write trigger bytes.
    pub fn AddTiKVRaftstoreStoreWriteTriggerWB(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2TiKVRaftstoreStoreWriteTriggerWB.Add(delta as f64);
        self.with_extra(|extra| extra.tikvRaftstoreStoreWriteTriggerWB.fetch_add(delta, Ordering::Relaxed));
    }

    // AddTiKVStorageProcessedKeysBatchGet records TiKV batch-get processed keys.
    pub fn AddTiKVStorageProcessedKeysBatchGet(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2TiKVStorageProcessedKeysBatchGet.Add(delta as f64);
        self.tikvStorageProcessedKeysBatchGet.fetch_add(delta, Ordering::Relaxed);
    }

    // AddTiKVStorageProcessedKeysGet records TiKV get processed keys.
    pub fn AddTiKVStorageProcessedKeysGet(&self, delta: i64) {
        if self.Bypass() {
            return;
        }
        metrics::RUV2TiKVStorageProcessedKeysGet.Add(delta as f64);
        self.tikvStorageProcessedKeysGet.fetch_add(delta, Ordering::Relaxed);
    }

    // AddTiKVCoprocessorWorkTotal records TiKV executor input counters by executor type.
    pub fn AddTiKVCoprocessorWorkTotal(&self, label: &str, delta: i64) {
        if self.Bypass() || delta == 0 || label.is_empty() {
            return;
        }
        metrics::RUV2TiKVCoprocessorWorkTotalCounter(label).Add(delta as f64);
        self.with_extra(|extra| addRUV2ExtraLabelCounter(&extra.tikvCoprocessorWorkTotal, label, delta));
    }

    // Clone returns a copy of the current metrics for reporting.
    pub fn Clone(&self) -> RUV2Metrics {
        let cloned = RUV2Metrics::default();
        cloned.bypass.store(self.Bypass(), Ordering::Relaxed);
        cloned.resultChunkCells.store(self.resultChunkCells.load(Ordering::Relaxed), Ordering::Relaxed);
        cloneRUV2ExecutorL1Counter(&cloned.executorL1, &self.executorL1);
        cloned.planCnt.store(self.planCnt.load(Ordering::Relaxed), Ordering::Relaxed);
        cloned.sessionParserTotal.store(self.sessionParserTotal.load(Ordering::Relaxed), Ordering::Relaxed);
        cloned.txnCnt.store(self.txnCnt.load(Ordering::Relaxed), Ordering::Relaxed);
        cloned.resourceManagerReadCnt.store(self.resourceManagerReadCnt.load(Ordering::Relaxed), Ordering::Relaxed);
        cloned.tikvKvEngineCacheMiss.store(self.tikvKvEngineCacheMiss.load(Ordering::Relaxed), Ordering::Relaxed);
        cloned.tikvStorageProcessedKeysBatchGet.store(self.tikvStorageProcessedKeysBatchGet.load(Ordering::Relaxed), Ordering::Relaxed);
        cloned.tikvStorageProcessedKeysGet.store(self.tikvStorageProcessedKeysGet.load(Ordering::Relaxed), Ordering::Relaxed);
        self.loadExtra(|extra| cloned.with_extra(|dst| cloneRUV2MetricsExtra(dst, extra)));
        cloned
    }
}

const ruv2LabelBatchPointGetExec: &str = "BatchPointGetExec";
const ruv2LabelPointGetExecutor: &str = "PointGetExecutor";
const ruv2LabelLimitExec: &str = "LimitExec";

// ruv2ExecutorL1Counter 对应 Go 的 L1 热标签计数器，三个高频标签独立字段，其余落入 extra。
#[derive(Default)]
/// L1 热标签计数：三个高频独立字段，其余进 extra。
struct ruv2ExecutorL1Counter {
    batchPointGetExec: AtomicI64,
    pointGetExecutor: AtomicI64,
    limitExec: AtomicI64,
    extra: ruv2ExtraLabelCounter,
}

// ruv2ExtraLabelCounter 对应 Go 的 atomic.Pointer[sync.Map]。
// Rust 用 Mutex<HashMap> 表达 LoadOrStore + atomic.AddInt64 的并发语义。
#[derive(Default)]
/// 额外标签计数（Mutex+HashMap 表达 Go sync.Map）。
struct ruv2ExtraLabelCounter {
    values: Mutex<HashMap<String, i64>>,
}

impl ruv2ExecutorL1Counter {
    fn add(&self, label: &str, delta: i64) {
        if let Some(field) = self.fieldByKind(execL1KindForLabel(label)) {
            field.fetch_add(delta, Ordering::Relaxed);
            return;
        }
        addRUV2ExtraLabelCounter(&self.extra, label, delta);
    }

    fn fieldByKind(&self, kind: execL1Kind) -> Option<&AtomicI64> {
        match kind {
            execL1Kind::execL1BatchPointGet => Some(&self.batchPointGetExec),
            execL1Kind::execL1PointGet => Some(&self.pointGetExecutor),
            execL1Kind::execL1Limit => Some(&self.limitExec),
            execL1Kind::execL1None => None,
        }
    }

    fn snapshot(&self) -> HashMap<String, i64> {
        let mut out = HashMap::new();
        addRUV2LabelValue(&mut out, ruv2LabelBatchPointGetExec, self.batchPointGetExec.load(Ordering::Relaxed));
        addRUV2LabelValue(&mut out, ruv2LabelPointGetExecutor, self.pointGetExecutor.load(Ordering::Relaxed));
        addRUV2LabelValue(&mut out, ruv2LabelLimitExec, self.limitExec.load(Ordering::Relaxed));
        snapshotRUV2ExtraLabelCounter(&self.extra, &mut out);
        out
    }

    fn sum(&self) -> i64 {
        self.batchPointGetExec.load(Ordering::Relaxed)
            + self.pointGetExecutor.load(Ordering::Relaxed)
            + self.limitExec.load(Ordering::Relaxed)
            + sumRUV2ExtraLabelCounter(&self.extra)
    }

    fn isZero(&self) -> bool {
        self.sum() == 0
    }
}

fn addRUV2LabelValue(out: &mut HashMap<String, i64>, label: &str, value: i64) {
    if value != 0 {
        out.insert(label.to_string(), value);
    }
}

fn addRUV2FixedCounter(dst: &AtomicI64, delta: i64) {
    if delta != 0 {
        dst.fetch_add(delta, Ordering::Relaxed);
    }
}

fn addRUV2ExtraLabelCounter(counter: &ruv2ExtraLabelCounter, label: &str, delta: i64) {
    let mut counterMap = counter.values.lock().expect("ruv2 label counter lock poisoned");
    let value = counterMap.entry(label.to_string()).or_insert(0);
    *value += delta;
}

fn snapshotRUV2ExtraLabelCounter(counter: &ruv2ExtraLabelCounter, out: &mut HashMap<String, i64>) {
    let counterMap = counter.values.lock().expect("ruv2 label counter lock poisoned");
    for (label, value) in counterMap.iter() {
        if *value != 0 {
            out.insert(label.clone(), *value);
        }
    }
}

fn sumRUV2ExtraLabelCounter(counter: &ruv2ExtraLabelCounter) -> i64 {
    let counterMap = counter.values.lock().expect("ruv2 label counter lock poisoned");
    counterMap.values().sum()
}

fn cloneRUV2ExtraLabelCounter(dst: &ruv2ExtraLabelCounter, src: &ruv2ExtraLabelCounter) {
    let snapshot = {
        let counterMap = src.values.lock().expect("ruv2 label counter lock poisoned");
        counterMap.clone()
    };
    for (label, value) in snapshot {
        if value != 0 {
            addRUV2ExtraLabelCounter(dst, &label, value);
        }
    }
}

fn cloneRUV2ExecutorL1Counter(dst: &ruv2ExecutorL1Counter, src: &ruv2ExecutorL1Counter) {
    addRUV2FixedCounter(&dst.batchPointGetExec, src.batchPointGetExec.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.pointGetExecutor, src.pointGetExecutor.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.limitExec, src.limitExec.load(Ordering::Relaxed));
    cloneRUV2ExtraLabelCounter(&dst.extra, &src.extra);
}

fn cloneRUV2MetricsExtra(dst: &ruv2MetricsExtra, src: &ruv2MetricsExtra) {
    cloneRUV2ExtraLabelCounter(&dst.executorL2, &src.executorL2);
    cloneRUV2ExtraLabelCounter(&dst.executorL3, &src.executorL3);
    addRUV2FixedCounter(&dst.executorL5InsertRows, src.executorL5InsertRows.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.planDeriveStatsPaths, src.planDeriveStatsPaths.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.resourceManagerWriteCnt, src.resourceManagerWriteCnt.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.writeKeys, src.writeKeys.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.writeSize, src.writeSize.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.tikvCoprocessorExecutorIterations, src.tikvCoprocessorExecutorIterations.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.tikvCoprocessorResponseBytes, src.tikvCoprocessorResponseBytes.load(Ordering::Relaxed));
    addRUV2FixedCounter(&dst.tikvRaftstoreStoreWriteTriggerWB, src.tikvRaftstoreStoreWriteTriggerWB.load(Ordering::Relaxed));
    cloneRUV2ExtraLabelCounter(&dst.tikvCoprocessorWorkTotal, &src.tikvCoprocessorWorkTotal);
}

impl RUV2Metrics {
    // Merge merges another metrics container into the receiver.
    pub fn Merge(&self, other: Option<&RUV2Metrics>) {
        let Some(other) = other else {
            return;
        };
        if self.Bypass() || other.Bypass() {
            return;
        }
        self.resultChunkCells.fetch_add(other.ResultChunkCells(), Ordering::Relaxed);
        cloneRUV2ExecutorL1Counter(&self.executorL1, &other.executorL1);
        self.planCnt.fetch_add(other.PlanCnt(), Ordering::Relaxed);
        self.sessionParserTotal.fetch_add(other.SessionParserTotal(), Ordering::Relaxed);
        self.txnCnt.fetch_add(other.TxnCnt(), Ordering::Relaxed);
        self.resourceManagerReadCnt.fetch_add(other.ResourceManagerReadCnt(), Ordering::Relaxed);
        self.tikvKvEngineCacheMiss.fetch_add(other.TiKVKVEngineCacheMiss(), Ordering::Relaxed);
        self.tikvStorageProcessedKeysBatchGet.fetch_add(other.TiKVStorageProcessedKeysBatchGet(), Ordering::Relaxed);
        self.tikvStorageProcessedKeysGet.fetch_add(other.TiKVStorageProcessedKeysGet(), Ordering::Relaxed);
        other.loadExtra(|extra| self.with_extra(|dst| cloneRUV2MetricsExtra(dst, extra)));
    }

    // ResultChunkCells returns result cells written by the current statement.
    pub fn ResultChunkCells(&self) -> i64 {
        self.resultChunkCells.load(Ordering::Relaxed)
    }

    // ExecutorL5InsertRows returns insert rows multiplied by inserted column count for RUv2 accounting.
    pub fn ExecutorL5InsertRows(&self) -> i64 {
        self.loadExtra(|extra| extra.executorL5InsertRows.load(Ordering::Relaxed)).unwrap_or(0)
    }

    // PlanCnt returns plan builder invocations for the current statement.
    pub fn PlanCnt(&self) -> i64 {
        self.planCnt.load(Ordering::Relaxed)
    }

    // PlanDeriveStatsPaths returns derived stats paths for the current statement.
    pub fn PlanDeriveStatsPaths(&self) -> i64 {
        self.loadExtra(|extra| extra.planDeriveStatsPaths.load(Ordering::Relaxed)).unwrap_or(0)
    }

    // SessionParserTotal returns parser executions for the current statement.
    pub fn SessionParserTotal(&self) -> i64 {
        self.sessionParserTotal.load(Ordering::Relaxed)
    }

    // TxnCnt returns transaction completions attributed to the current statement.
    pub fn TxnCnt(&self) -> i64 {
        self.txnCnt.load(Ordering::Relaxed)
    }

    // ResourceManagerReadCnt returns TiKV read RPCs charged to resource management.
    pub fn ResourceManagerReadCnt(&self) -> i64 {
        self.resourceManagerReadCnt.load(Ordering::Relaxed)
    }

    // ResourceManagerWriteCnt returns TiKV write RPCs charged to resource management.
    pub fn ResourceManagerWriteCnt(&self) -> i64 {
        self.loadExtra(|extra| extra.resourceManagerWriteCnt.load(Ordering::Relaxed)).unwrap_or(0)
    }

    // WriteKeys returns commit write keys for RUv2 accounting.
    pub fn WriteKeys(&self) -> i64 {
        self.loadExtra(|extra| extra.writeKeys.load(Ordering::Relaxed)).unwrap_or(0)
    }

    // WriteSize returns commit write size for RUv2 shadow accounting.
    pub fn WriteSize(&self) -> i64 {
        self.loadExtra(|extra| extra.writeSize.load(Ordering::Relaxed)).unwrap_or(0)
    }

    // TiKVKVEngineCacheMiss returns TiKV kv_engine_cache_miss counters from ExecDetailsV2.
    pub fn TiKVKVEngineCacheMiss(&self) -> i64 {
        self.tikvKvEngineCacheMiss.load(Ordering::Relaxed)
    }

    // TiKVCoprocessorExecutorIterations returns TiKV coprocessor iteration counters.
    pub fn TiKVCoprocessorExecutorIterations(&self) -> i64 {
        self.loadExtra(|extra| extra.tikvCoprocessorExecutorIterations.load(Ordering::Relaxed)).unwrap_or(0)
    }

    // TiKVCoprocessorResponseBytes returns TiKV coprocessor response bytes.
    pub fn TiKVCoprocessorResponseBytes(&self) -> i64 {
        self.loadExtra(|extra| extra.tikvCoprocessorResponseBytes.load(Ordering::Relaxed)).unwrap_or(0)
    }

    // TiKVRaftstoreStoreWriteTriggerWB returns TiKV raftstore write trigger bytes.
    pub fn TiKVRaftstoreStoreWriteTriggerWB(&self) -> i64 {
        self.loadExtra(|extra| extra.tikvRaftstoreStoreWriteTriggerWB.load(Ordering::Relaxed)).unwrap_or(0)
    }

    // TiKVStorageProcessedKeysBatchGet returns TiKV batch-get processed keys.
    pub fn TiKVStorageProcessedKeysBatchGet(&self) -> i64 {
        self.tikvStorageProcessedKeysBatchGet.load(Ordering::Relaxed)
    }

    // TiKVStorageProcessedKeysGet returns TiKV get processed keys.
    pub fn TiKVStorageProcessedKeysGet(&self) -> i64 {
        self.tikvStorageProcessedKeysGet.load(Ordering::Relaxed)
    }

    // IsZero checks whether all metrics are zero.
    pub fn IsZero(&self) -> bool {
        if self.Bypass() {
            return true;
        }
        if self.ResultChunkCells() != 0
            || !self.executorL1.isZero()
            || self.PlanCnt() != 0
            || self.SessionParserTotal() != 0
            || self.TxnCnt() != 0
            || self.ResourceManagerReadCnt() != 0
            || self.TiKVKVEngineCacheMiss() != 0
            || self.TiKVStorageProcessedKeysBatchGet() != 0
            || self.TiKVStorageProcessedKeysGet() != 0
        {
            return false;
        }
        self.loadExtra(|extra| {
            sumRUV2ExtraLabelCounter(&extra.executorL2) == 0
                && sumRUV2ExtraLabelCounter(&extra.executorL3) == 0
                && extra.executorL5InsertRows.load(Ordering::Relaxed) == 0
                && extra.planDeriveStatsPaths.load(Ordering::Relaxed) == 0
                && extra.resourceManagerWriteCnt.load(Ordering::Relaxed) == 0
                && extra.writeKeys.load(Ordering::Relaxed) == 0
                && extra.writeSize.load(Ordering::Relaxed) == 0
                && extra.tikvCoprocessorExecutorIterations.load(Ordering::Relaxed) == 0
                && extra.tikvCoprocessorResponseBytes.load(Ordering::Relaxed) == 0
                && extra.tikvRaftstoreStoreWriteTriggerWB.load(Ordering::Relaxed) == 0
                && sumRUV2ExtraLabelCounter(&extra.tikvCoprocessorWorkTotal) == 0
        })
        .unwrap_or(true)
    }

    // CalculateRUValues calculates the current TiDB RU from the metrics using the
    // provided weights. The weights specify how each component is weighted in the
    // RU calculation. Returns the calculated TiDB RU as a float64.
    pub fn CalculateRUValues(&self, weights: RUV2Weights) -> f64 {
        if self.Bypass() {
            return 0.0;
        }
        self.calculateRUValuesWithWeights(weights)
    }

    // TotalRU returns the statement RU v2 total as TiDB + TiKV + TiFlash.
    pub fn TotalRU(&self, weights: RUV2Weights, tiKVRU: f64, tiFlashRU: f64) -> f64 {
        if self.Bypass() {
            return 0.0;
        }
        self.CalculateRUValues(weights) + tiKVRU + tiFlashRU
    }

    fn calculateRUValuesWithWeights(&self, weights: RUV2Weights) -> f64 {
        let mut executorL2 = 0;
        let mut executorL3 = 0;
        let mut executorL5InsertRows = 0;
        let mut planDeriveStatsPaths = 0;
        let mut resourceManagerWriteCnt = 0;
        let mut writeKeys = 0;
        self.loadExtra(|extra| {
            executorL2 = sumRUV2ExtraLabelCounter(&extra.executorL2);
            executorL3 = sumRUV2ExtraLabelCounter(&extra.executorL3);
            executorL5InsertRows = extra.executorL5InsertRows.load(Ordering::Relaxed);
            planDeriveStatsPaths = extra.planDeriveStatsPaths.load(Ordering::Relaxed);
            resourceManagerWriteCnt = extra.resourceManagerWriteCnt.load(Ordering::Relaxed);
            writeKeys = extra.writeKeys.load(Ordering::Relaxed);
        });
        let tidbRUFloat =
            self.ResultChunkCells() as f64 * weights.ResultChunkCells
                + self.executorL1.sum() as f64 * weights.ExecutorL1
                + executorL2 as f64 * weights.ExecutorL2
                + executorL3 as f64 * weights.ExecutorL3
                + executorL5InsertRows as f64 * weights.ExecutorL5InsertRows
                + self.PlanCnt() as f64 * weights.PlanCnt
                + planDeriveStatsPaths as f64 * weights.PlanDeriveStatsPaths
                + self.ResourceManagerReadCnt() as f64 * weights.ResourceManagerReadCnt
                + resourceManagerWriteCnt as f64 * weights.ResourceManagerWriteCnt
                + writeKeys as f64 * weights.WriteKeys
                + self.SessionParserTotal() as f64 * weights.SessionParserTotal
                + self.TxnCnt() as f64 * weights.TxnCnt;
        tidbRUFloat * weights.RUScale
    }
}

// FormatRUV2Summary formats the RUv2 total and detailed metrics in one pass.
/// 一次格式化 RUv2 总量与明细字符串。
pub fn FormatRUV2Summary(metrics: Option<&RUV2Metrics>, weights: RUV2Weights, tiKVRU: f64, tiFlashRU: f64) -> (String, String) {
    if metrics.map(|m| m.Bypass()).unwrap_or(false) {
        return (String::new(), String::new());
    }
    let mut resultChunkCells = 0;
    let mut executorL1 = HashMap::new();
    let mut executorL2 = HashMap::new();
    let mut executorL3 = HashMap::new();
    let mut executorL5InsertRows = 0;
    let mut planCnt = 0;
    let mut planDeriveStatsPaths = 0;
    let mut sessionParserTotal = 0;
    let mut txnCnt = 0;
    let mut resourceManagerReadCnt = 0;
    let mut resourceManagerWriteCnt = 0;
    let mut writeKeys = 0;
    let mut writeSize = 0;
    let mut tiKVKVEngineCacheMiss = 0;
    let mut tiKVCoprocessorExecutorIterations = 0;
    let mut tiKVCoprocessorResponseBytes = 0;
    let mut tiKVRaftstoreStoreWriteTriggerWB = 0;
    let mut tiKVStorageProcessedKeysBatchGet = 0;
    let mut tiKVStorageProcessedKeysGet = 0;
    let mut tiKVCoprocessorExecutorWorkTotal = HashMap::new();
    let mut tidbRU = 0.0;

    if let Some(metrics) = metrics {
        resultChunkCells = metrics.ResultChunkCells();
        executorL1 = metrics.executorL1.snapshot();
        metrics.loadExtra(|extra| {
            snapshotRUV2ExtraLabelCounter(&extra.executorL2, &mut executorL2);
            snapshotRUV2ExtraLabelCounter(&extra.executorL3, &mut executorL3);
            executorL5InsertRows = extra.executorL5InsertRows.load(Ordering::Relaxed);
            planDeriveStatsPaths = extra.planDeriveStatsPaths.load(Ordering::Relaxed);
            resourceManagerWriteCnt = extra.resourceManagerWriteCnt.load(Ordering::Relaxed);
            writeKeys = extra.writeKeys.load(Ordering::Relaxed);
            writeSize = extra.writeSize.load(Ordering::Relaxed);
            tiKVCoprocessorExecutorIterations = extra.tikvCoprocessorExecutorIterations.load(Ordering::Relaxed);
            tiKVCoprocessorResponseBytes = extra.tikvCoprocessorResponseBytes.load(Ordering::Relaxed);
            tiKVRaftstoreStoreWriteTriggerWB = extra.tikvRaftstoreStoreWriteTriggerWB.load(Ordering::Relaxed);
            snapshotRUV2ExtraLabelCounter(&extra.tikvCoprocessorWorkTotal, &mut tiKVCoprocessorExecutorWorkTotal);
        });
        planCnt = metrics.PlanCnt();
        sessionParserTotal = metrics.SessionParserTotal();
        txnCnt = metrics.TxnCnt();
        resourceManagerReadCnt = metrics.ResourceManagerReadCnt();
        tiKVKVEngineCacheMiss = metrics.TiKVKVEngineCacheMiss();
        tiKVStorageProcessedKeysBatchGet = metrics.TiKVStorageProcessedKeysBatchGet();
        tiKVStorageProcessedKeysGet = metrics.TiKVStorageProcessedKeysGet();
        tidbRU = metrics.calculateRUValuesWithWeights(weights);
    }
    if resultChunkCells == 0
        && executorL1.is_empty()
        && executorL2.is_empty()
        && executorL3.is_empty()
        && executorL5InsertRows == 0
        && planCnt == 0
        && planDeriveStatsPaths == 0
        && sessionParserTotal == 0
        && txnCnt == 0
        && resourceManagerReadCnt == 0
        && resourceManagerWriteCnt == 0
        && writeKeys == 0
        && writeSize == 0
        && tiKVKVEngineCacheMiss == 0
        && tiKVCoprocessorExecutorIterations == 0
        && tiKVCoprocessorResponseBytes == 0
        && tiKVRaftstoreStoreWriteTriggerWB == 0
        && tiKVStorageProcessedKeysBatchGet == 0
        && tiKVStorageProcessedKeysGet == 0
        && tiKVCoprocessorExecutorWorkTotal.is_empty()
        && tiKVRU == 0.0
        && tiFlashRU == 0.0
    {
        return (String::new(), String::new());
    }

    fn append_int(parts: &mut Vec<String>, key: &str, value: i64) {
        if value != 0 {
            parts.push(format!("{}:{}", key, value));
        }
    }
    fn append_float64_always(parts: &mut Vec<String>, key: &str, value: f64) {
        parts.push(format!("{}:{:.2}", key, value));
    }
    fn append_map(parts: &mut Vec<String>, key: &str, value: &HashMap<String, i64>) {
        if value.is_empty() {
            return;
        }
        let formatted = formatRUV2LabelMap(value);
        if !formatted.is_empty() {
            parts.push(format!("{}:{}", key, formatted));
        }
    }

    let mut parts: Vec<String> = Vec::with_capacity(23);
    let totalRU = tidbRU + tiKVRU + tiFlashRU;
    let total = format!("{:.2}", totalRU);
    append_float64_always(&mut parts, "total_ru", totalRU);
    append_float64_always(&mut parts, "tidb_ru", tidbRU);
    append_float64_always(&mut parts, "tikv_ru", tiKVRU);
    append_float64_always(&mut parts, "tiflash_ru", tiFlashRU);

    append_int(&mut parts, "result_chunk_cells", resultChunkCells);
    append_map(&mut parts, "executor_l1", &executorL1);
    append_map(&mut parts, "executor_l2", &executorL2);
    append_map(&mut parts, "executor_l3", &executorL3);
    append_int(&mut parts, "executor_l5_insert_rows", executorL5InsertRows);
    append_int(&mut parts, "plan_cnt", planCnt);
    append_int(&mut parts, "plan_derive_stats_paths", planDeriveStatsPaths);
    append_int(&mut parts, "session_parser_total", sessionParserTotal);
    append_int(&mut parts, "txn_cnt", txnCnt);
    append_int(&mut parts, "resource_manager_read_cnt", resourceManagerReadCnt);
    append_int(&mut parts, "resource_manager_write_cnt", resourceManagerWriteCnt);
    append_int(&mut parts, "write_keys", writeKeys);
    append_int(&mut parts, "write_size", writeSize);
    append_int(&mut parts, "tikv_kv_engine_cache_miss", tiKVKVEngineCacheMiss);
    append_int(&mut parts, "tikv_coprocessor_executor_iterations", tiKVCoprocessorExecutorIterations);
    append_int(&mut parts, "tikv_coprocessor_response_bytes", tiKVCoprocessorResponseBytes);
    append_int(&mut parts, "tikv_raftstore_store_write_trigger_wb_bytes", tiKVRaftstoreStoreWriteTriggerWB);
    append_int(&mut parts, "tikv_storage_processed_keys_batch_get", tiKVStorageProcessedKeysBatchGet);
    append_int(&mut parts, "tikv_storage_processed_keys_get", tiKVStorageProcessedKeysGet);
    append_map(&mut parts, "tikv_coprocessor_executor_work_total", &tiKVCoprocessorExecutorWorkTotal);

    (total, parts.join(", "))
}

// FormatRUV2Total formats the RUv2 total into a slow log string.
/// 仅格式化 RUv2 总量。
pub fn FormatRUV2Total(metrics: Option<&RUV2Metrics>, weights: RUV2Weights, tiKVRU: f64, tiFlashRU: f64) -> String {
    let (total, _) = FormatRUV2Summary(metrics, weights, tiKVRU, tiFlashRU);
    total
}

// FormatRUV2Metrics formats RUv2 metrics into a compact detail string.
/// 格式化 RUv2 明细详情串。
pub fn FormatRUV2Metrics(metrics: Option<&RUV2Metrics>, weights: RUV2Weights, tiKVRU: f64, tiFlashRU: f64) -> String {
    let (_, detail) = FormatRUV2Summary(metrics, weights, tiKVRU, tiFlashRU);
    detail
}

// formatRUV2LabelMap 对应 Go 的 label map 稳定排序输出。
fn formatRUV2LabelMap(values: &HashMap<String, i64>) -> String {
    let mut keys: Vec<String> = values
        .iter()
        .filter_map(|(key, value)| if *value != 0 { Some(key.clone()) } else { None })
        .collect();
    if keys.is_empty() {
        return String::new();
    }
    keys.sort();
    let mut builder = String::new();
    builder.push('{');
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            builder.push(',');
        }
        builder.push_str(key);
        builder.push(':');
        builder.push_str(&values[key].to_string());
    }
    builder.push('}');
    builder
}
