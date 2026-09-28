// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 这个文件集中定义 export 包运行期会用到的 metrics 容器和若干轻量包装函数。
// 设计目标不是实现完整 Prometheus 语义，而是给 dumper / writer / task 流程提供
// 一套稳定的“可注册、可读写、可在测试中为空”的指标接口。

// metrics 聚合 export 主流程关心的核心统计项。
// 字段命名与 Go prometheus 指标名对齐，便于跨语言对照观测面板。
pub struct metrics {
    // finished_size / finished_rows / finished_tables 共同表示已完成导出的工作量。
    pub finishedSizeGauge: GaugeVec,
    pub finishedRowsGauge: GaugeVec,
    // estimate_total_rows 用于给进度估算提供总量参考。
    pub finishedTablesCounter: CounterVec,
    // 两个 histogram 分别统计真正写盘耗时和 chunk 交接耗时。
    pub estimateTotalRowsCounter: CounterVec,
    pub writeTimeHistogram: HistogramVec,
    // errorCount 和 taskChannelCapacity 用于观测错误与任务堆积情况。
    pub receiveWriteChunkTimeHistogram: HistogramVec,
    pub errorCount: CounterVec,
    // 下面三个原子字段服务于进度条，而不是 Prometheus registry 本身。
    pub taskChannelCapacity: GaugeVec,
    pub totalChunks: AtomicI64,
    pub completedChunks: AtomicI64,
    pub progressReady: AtomicBool,
}

pub fn newMetrics(f: &dyn Factory, _const_labels: &Labels) -> metrics {
    // 当前最小实现忽略 const_labels，但保留参数位以贴近 Go 构造签名。
    metrics {
        finishedSizeGauge: f.NewGaugeVec("finished_size"),
        estimateTotalRowsCounter: f.NewCounterVec("estimate_total_rows"),
        finishedRowsGauge: f.NewGaugeVec("finished_rows"),
        finishedTablesCounter: f.NewCounterVec("finished_tables"),
        writeTimeHistogram: f.NewHistogramVec("write_duration_time"),
        receiveWriteChunkTimeHistogram: f.NewHistogramVec("receive_chunk_duration_time"),
        errorCount: f.NewCounterVec("error_count"),
        taskChannelCapacity: f.NewGaugeVec("channel_capacity"),
        totalChunks: AtomicI64::new(0),
        completedChunks: AtomicI64::new(0),
        progressReady: AtomicBool::new(false),
    }
}

impl metrics {
    pub fn registerTo(&self, registry: &dyn Registry) {
        // 显式逐项注册，便于测试替身 registry 精确记录被挂载的名字。
        registry.MustRegister("finished_size");
        registry.MustRegister("finished_rows");
        registry.MustRegister("estimate_total_rows");
        registry.MustRegister("finished_tables");
        registry.MustRegister("write_duration_time");
        registry.MustRegister("receive_chunk_duration_time");
        registry.MustRegister("error_count");
        registry.MustRegister("channel_capacity");
    }
    pub fn unregisterFrom(&self, registry: &dyn Registry) {
        // 与 registerTo 成对出现，避免重复运行测试时留下脏注册状态。
        registry.Unregister("finished_size");
        registry.Unregister("finished_rows");
        registry.Unregister("estimate_total_rows");
        registry.Unregister("finished_tables");
        registry.Unregister("write_duration_time");
        registry.Unregister("receive_chunk_duration_time");
        registry.Unregister("error_count");
        registry.Unregister("channel_capacity");
    }
}

pub fn ReadCounter(counter_vec: Option<&CounterVec>) -> f64 {
    // 允许指标对象为空，这样业务代码在测试里不必处处判空。
    match counter_vec {
        None => f64::NAN,
        Some(c) => c.With(None).get(),
    }
}
pub fn AddCounter(counter_vec: Option<&CounterVec>, v: f64) {
    // `Option<&...>` 包装层让调用点可以把“无指标”视为 no-op。
    if let Some(c) = counter_vec {
        c.With(None).Add(v);
    }
}
pub fn IncCounter(counter_vec: Option<&CounterVec>) {
    if let Some(c) = counter_vec {
        c.With(None).Inc();
    }
}
pub fn ObserveHistogram(histogram_vec: Option<&HistogramVec>, v: f64) {
    if let Some(h) = histogram_vec {
        h.With(None).Observe(v);
    }
}
pub fn ReadGauge(gauge_vec: Option<&GaugeVec>) -> f64 {
    // ReadGauge 与 ReadCounter 一样，空值返回 NaN 方便测试识别“未接线”状态。
    match gauge_vec {
        None => f64::NAN,
        Some(g) => g.With(None).get(),
    }
}
pub fn AddGauge(gauge_vec: Option<&GaugeVec>, v: f64) {
    if let Some(g) = gauge_vec {
        g.With(None).Add(v);
    }
}
pub fn SubGauge(gauge_vec: Option<&GaugeVec>, v: f64) {
    if let Some(g) = gauge_vec {
        g.With(None).Sub(v);
    }
}
pub fn IncGauge(gauge_vec: Option<&GaugeVec>) {
    if let Some(g) = gauge_vec {
        g.With(None).Inc();
    }
}
pub fn DecGauge(gauge_vec: Option<&GaugeVec>) {
    if let Some(g) = gauge_vec {
        g.With(None).Dec();
    }
}

// 这个别名只是为了压住某些迁移路径下的未使用导入告警。
// silence unused Arc import warning path
pub type _ArcMetrics = Arc<metrics>;
