// Copyright 2025 PingCAP, Inc.
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

// 执行明细（exec details）上下文工具与百分位统计。
//
// 负责在 `context` 中初始化/补齐/继承 Stmt、TiKV、RU（Resource Unit，资源计量）
// 明细；提供原子字段快照、百分位聚合与人类可读的耗时格式化。对应 Go
// `pkg/util/execdetails` 工具函数。

use std::cmp::Ordering as CmpOrdering;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration as StdDuration;

/// 初始化并写入 stmt、TiKV exec details 与 RU details 到 context。
// ContextWithInitializedExecDetails 对应 Go 的同名函数：把 stmt、TiKV exec details 和 RU details 都初始化进 context。
pub fn ContextWithInitializedExecDetails(mut ctx: context::Context) -> context::Context {
    let stmtDetails = Arc::new(StmtExecDetails::default());
    stmtDetails.ensureRUV2Metrics();
    ctx = context::WithValue(
        ctx,
        &util::ExecDetailsKey,
        Arc::new(util::ExecDetails::default()),
    );
    ctx = context::WithValue(ctx, &util::RUDetailsCtxKey, util::NewRUDetails());
    ctx = context::WithValue(ctx, &StmtExecDetailKey, stmtDetails);
    ctx
}

/// 仅补齐 context 中缺失的执行明细对象，保留已有指针语义。
// ContextWithMissingExecDetailsInitialized 对应 Go 的缺失项补齐逻辑。
// 已存在的对象会被保留，只在 context 缺字段时补默认值，并尝试继承已有 RUv2 metrics。
pub fn ContextWithMissingExecDetailsInitialized(mut ctx: context::Context) -> context::Context {
    let mut stmtDetails = ctx.value::<Arc<StmtExecDetails>, _>(&StmtExecDetailKey);
    if ctx
        .value::<Arc<util::ExecDetails>, _>(&util::ExecDetailsKey)
        .is_none()
    {
        ctx = context::WithValue(
            ctx,
            &util::ExecDetailsKey,
            Arc::new(util::ExecDetails::default()),
        );
    }
    if ctx
        .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
        .is_none()
    {
        ctx = context::WithValue(ctx, &util::RUDetailsCtxKey, util::NewRUDetails());
    }
    // 补 StmtExecDetails，并尽量继承独立 key 上的 RUv2 metrics。
    if stmtDetails.is_none() {
        let details = Arc::new(StmtExecDetails::default());
        if let Some(inherited) = ctx.value::<Arc<RUV2Metrics>, _>(&RUV2MetricsCtxKey) {
            details.setRUV2Metrics(inherited);
        }
        ctx = context::WithValue(ctx, &StmtExecDetailKey, details.clone());
        stmtDetails = Some(details);
    }
    // 已有 statement details 但缺 metrics 时同样继承或新建。
    if let Some(details) = stmtDetails.as_ref() {
        if details.getRUV2Metrics().is_none() {
            if let Some(inherited) = ctx.value::<Arc<RUV2Metrics>, _>(&RUV2MetricsCtxKey) {
                details.setRUV2Metrics(inherited);
            } else {
                details.ensureRUV2Metrics();
            }
        }
    }
    ctx
}

/// 从 source 继承缺失的 RUDetails / RUv2 metrics。
// ContextWithInheritedRUV2Details 对应 Go 的 RUDetails/RUv2 metrics 继承逻辑。
// source 为 nil 时直接返回；目标 context 仅在缺失时继承，避免覆盖调用方已有统计对象。
pub fn ContextWithInheritedRUV2Details(mut ctx: context::Context, source: Option<context::Context>) -> context::Context {
    let Some(source) = source else {
        return ctx;
    };
    if ctx
        .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
        .is_none()
    {
        if let Some(ruDetails) =
            source.value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
        {
            ctx = context::WithValue(ctx, &util::RUDetailsCtxKey, ruDetails);
        }
    }
    if RUV2MetricsFromContext(&ctx).is_none() {
        if let Some(metrics) = RUV2MetricsFromContext(&source) {
            ctx = contextWithRUV2Metrics(ctx, Some(metrics));
        }
    }
    ctx
}

/// 将 RUv2 metrics 绑定到 context（优先写入 StmtExecDetails）。
// ContextWithRUV2Metrics 对应 Go 的导出包装函数，返回绑定 metrics 后的 context。
pub fn ContextWithRUV2Metrics(
    ctx: context::Context,
    metrics: Option<Arc<RUV2Metrics>>,
) -> context::Context {
    contextWithRUV2Metrics(ctx, metrics)
}

/// 内部实现：有 StmtExecDetails 则写入其中，否则挂独立 key。
// contextWithRUV2Metrics 对应 Go 的内部辅助函数。
// 如果 context 已带 StmtExecDetails，优先写入 statement details；否则使用独立 key 作为兜底路径。
fn contextWithRUV2Metrics(
    ctx: context::Context,
    metrics: Option<Arc<RUV2Metrics>>,
) -> context::Context {
    let Some(metrics) = metrics else {
        return ctx;
    };
    if let Some(stmtDetails) = ctx.value::<Arc<StmtExecDetails>, _>(&StmtExecDetailKey) {
        stmtDetails.setRUV2Metrics(metrics);
        return ctx;
    }
    context::WithValue(ctx, &RUV2MetricsCtxKey, metrics)
}

/// 将 context 中的 RUDetails 同步进 statement 的 RUv2 metrics。
// SyncRUV2MetricsFromContext 对应 Go 的从 context 中 drain RUDetails 并同步到 statement RUv2 metrics。
pub fn SyncRUV2MetricsFromContext(ctx: &context::Context) -> Option<Arc<RUV2Metrics>> {
    let metrics = RUV2MetricsFromContext(ctx)?;
    let ruDetails = ctx.value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey);
    SyncRUV2MetricsFromRUDetails(Some(metrics.as_ref()), ruDetails.as_deref());
    Some(metrics)
}

/// 从 context 取出写响应耗时、TiKV 明细快照与 RUDetails。
// GetExecDetailsFromContext 对应 Go 的三返回值函数。
// Rust 用元组表达命名返回值，并在缺少 RUDetails 时创建一个新的默认对象。
pub fn GetExecDetailsFromContext(
    ctx: &context::Context,
) -> (StdDuration, util::ExecDetails, Arc<util::RUDetails>) {
    let mut writeSQLRespDuration = StdDuration::default();
    if let Some(stmtDetail) = ctx.value::<Arc<StmtExecDetails>, _>(&StmtExecDetailKey) {
        writeSQLRespDuration = stmtDetail.WriteSQLRespDuration;
    }
    let mut tikvExecDetail = util::ExecDetails::default();
    if let Some(raw) = ctx.value::<Arc<util::ExecDetails>, _>(&util::ExecDetailsKey) {
        tikvExecDetail = LoadTiKVExecDetails(Some(raw.as_ref()));
    }
    let ruDetails = ctx
        .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
        .unwrap_or_else(util::NewRUDetails);
    (writeSQLRespDuration, tikvExecDetail, ruDetails)
}

/// 对 util.ExecDetails 的原子字段做一次宽松内存序快照。
// LoadTiKVExecDetails 对应 Go 的 atomic 字段快照函数。
// Go 的 util.ExecDetails 多个字段由 atomic 更新；这里保留逐字段 load 的快照语义。
pub fn LoadTiKVExecDetails(detail: Option<&util::ExecDetails>) -> util::ExecDetails {
    let Some(detail) = detail else {
        return util::ExecDetails::default();
    };
    util::ExecDetails {
        BackoffCount: AtomicI64::new(detail.BackoffCount.load(Ordering::Relaxed)),
        BackoffDuration: AtomicI64::new(detail.BackoffDuration.load(Ordering::Relaxed)),
        WaitKVRespDuration: AtomicI64::new(detail.WaitKVRespDuration.load(Ordering::Relaxed)),
        WaitPDRespDuration: AtomicI64::new(detail.WaitPDRespDuration.load(Ordering::Relaxed)),
        TrafficDetails: util::TrafficDetails {
            UnpackedBytesSentKVTotal: AtomicI64::new(
                detail.TrafficDetails.UnpackedBytesSentKVTotal.load(Ordering::Relaxed),
            ),
            UnpackedBytesReceivedKVTotal: AtomicI64::new(
                detail
                    .TrafficDetails
                    .UnpackedBytesReceivedKVTotal
                    .load(Ordering::Relaxed),
            ),
            UnpackedBytesSentKVCrossZone: AtomicI64::new(
                detail
                    .TrafficDetails
                    .UnpackedBytesSentKVCrossZone
                    .load(Ordering::Relaxed),
            ),
            UnpackedBytesReceivedKVCrossZone: AtomicI64::new(
                detail
                    .TrafficDetails
                    .UnpackedBytesReceivedKVCrossZone
                    .load(Ordering::Relaxed),
            ),
            UnpackedBytesSentMPPTotal: AtomicI64::new(
                detail
                    .TrafficDetails
                    .UnpackedBytesSentMPPTotal
                    .load(Ordering::Relaxed),
            ),
            UnpackedBytesReceivedMPPTotal: AtomicI64::new(
                detail
                    .TrafficDetails
                    .UnpackedBytesReceivedMPPTotal
                    .load(Ordering::Relaxed),
            ),
            UnpackedBytesSentMPPCrossZone: AtomicI64::new(
                detail
                    .TrafficDetails
                    .UnpackedBytesSentMPPCrossZone
                    .load(Ordering::Relaxed),
            ),
            UnpackedBytesReceivedMPPCrossZone: AtomicI64::new(
                detail
                    .TrafficDetails
                    .UnpackedBytesReceivedMPPCrossZone
                    .load(Ordering::Relaxed),
            ),
        },
    }
}

/// 可将自身转为 f64，供百分位统计使用。
// canGetFloat64 对应 Go 的泛型约束接口。
pub trait canGetFloat64 {
    fn GetFloat64(&self) -> f64;
}

/// i64 包装，实现 canGetFloat64。
// Int64 is a wrapper of int64 to implement the canGetFloat64 interface.
pub type Int64 = i64;

// GetFloat64 implements the canGetFloat64 interface.
impl canGetFloat64 for Int64 {
    fn GetFloat64(&self) -> f64 {
        *self as f64
    }
}

/// Duration 包装，按纳秒转为 f64。
// Duration is a wrapper of time.Duration to implement the canGetFloat64 interface.
pub type Duration = StdDuration;

// GetFloat64 implements the canGetFloat64 interface.
impl canGetFloat64 for Duration {
    fn GetFloat64(&self) -> f64 {
        self.as_nanos() as f64
    }
}

/// 带地址的耗时，用于按节点聚合延迟。
// DurationWithAddr is a wrapper of time.Duration and string to implement the canGetFloat64 interface.
#[derive(Clone, Default)]
pub struct DurationWithAddr {
    pub D: StdDuration,
    pub Addr: String,
}

// GetFloat64 implements the canGetFloat64 interface.
impl canGetFloat64 for DurationWithAddr {
    fn GetFloat64(&self) -> f64 {
        self.D.as_nanos() as f64
    }
}

/// 百分位统计：小样本精确排序，大样本切到 t-digest 近似。
// Percentile 对应 Go 的泛型百分位统计结构。
// values 在样本量较小时保留原始值；超过 MaxDetailsNumsForOneQuery 后切换到 tdigest。
#[derive(Clone)]
pub struct Percentile<valueType: canGetFloat64 + Clone> {
    values: Vec<valueType>,
    size: usize,
    isSorted: bool,
    minVal: Option<valueType>,
    maxVal: Option<valueType>,
    sumVal: f64,
    dt: Option<tdigest::TDigest>,
}

impl<valueType: canGetFloat64 + Clone> Default for Percentile<valueType> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            size: 0,
            isSorted: false,
            minVal: None,
            maxVal: None,
            sumVal: 0.0,
            dt: None,
        }
    }
}

impl<valueType: canGetFloat64 + Clone> Percentile<valueType> {
    /// 追加一个样本；达阈值后迁入 t-digest。
    // Add adds a value to calculate the percentile.
    pub fn Add(&mut self, value: valueType) {
        let floatValue = value.GetFloat64();
        self.isSorted = false;
        self.sumVal += floatValue;
        self.size += 1;
        if self.dt.is_none() && self.values.is_empty() {
            self.minVal = Some(value.clone());
            self.maxVal = Some(value.clone());
        } else {
            if self
                .minVal
                .as_ref()
                .map(|min| value.GetFloat64() < min.GetFloat64())
                .unwrap_or(true)
            {
                self.minVal = Some(value.clone());
            }
            if self
                .maxVal
                .as_ref()
                .map(|max| value.GetFloat64() > max.GetFloat64())
                .unwrap_or(true)
            {
                self.maxVal = Some(value.clone());
            }
        }
        // 未建 digest 时攒原始值；达 MaxDetailsNumsForOneQuery 后压缩为 t-digest。
        if self.dt.is_none() {
            self.values.push(value);
            if self.values.len() >= MaxDetailsNumsForOneQuery as usize {
                let values = self
                    .values
                    .drain(..)
                    .map(|item| item.GetFloat64())
                    .collect();
                self.dt = Some(tdigest::TDigest::new_with_size(100).merge_unsorted(values));
            }
            return;
        }
        // 已有 digest：单点 merge。
        if let Some(digest) = self.dt.take() {
            self.dt = Some(digest.merge_unsorted(vec![floatValue]));
        }
    }

    /// 返回分位 `f`（0..=1）对应的估计值。
    // GetPercentile returns the percentile `f` of the values.
    pub fn GetPercentile(&mut self, f: f64) -> f64 {
        // 小样本：排序后按下标取精确分位。
        if self.dt.is_none() {
            if !self.isSorted {
                self.isSorted = true;
                // Go 使用 slices.SortFunc + cmp.Compare；按 GetFloat64 排序保留同一语义。
                self.values.sort_by(|i, j| {
                    i.GetFloat64()
                        .partial_cmp(&j.GetFloat64())
                        .unwrap_or(CmpOrdering::Equal)
                });
            }
            let idx = (self.values.len() as f64 * f) as usize;
            return self.values[idx].GetFloat64();
        }
        self.dt
            .as_ref()
            .and_then(|digest| digest.estimate_quantile(f))
            .unwrap_or(0.0)
    }

    /// 返回观测到的最大值。
    // GetMax returns the max value.
    pub fn GetMax(&self) -> Option<valueType> {
        self.maxVal.clone()
    }

    /// 返回观测到的最小值。
    // GetMin returns the min value.
    pub fn GetMin(&self) -> Option<valueType> {
        self.minVal.clone()
    }

    /// 合并另一个 Percentile 的样本/摘要。
    // MergePercentile merges two Percentile.
    pub fn MergePercentile(&mut self, p2: &Percentile<valueType>) {
        self.isSorted = false;
        if p2.dt.is_none() {
            for v in &p2.values {
                self.Add(v.clone());
            }
            return;
        }
        self.sumVal += p2.sumVal;
        self.size += p2.size;
        if self.dt.is_none() {
            let values = self
                .values
                .drain(..)
                .map(|item| item.GetFloat64())
                .collect();
            self.dt = Some(tdigest::TDigest::new_with_size(100).merge_unsorted(values));
        }
        if let (Some(dst), Some(src)) = (self.dt.take(), p2.dt.as_ref()) {
            self.dt = Some(tdigest::TDigest::merge_digests(vec![dst, src.clone()]));
        }
    }

    /// 样本个数。
    // Size returns the size of the values.
    pub fn Size(&self) -> usize {
        self.size
    }

    /// 样本之和（按 GetFloat64）。
    // Sum returns the sum of the values.
    pub fn Sum(&self) -> f64 {
        self.sumVal
    }
}

/// 按人类可读规则裁剪精度后格式化耗时（对齐 Go FormatDuration）。
// FormatDuration uses to format duration, this function will prune precision before format duration.
// Pruning precision is for human readability. The prune rule is:
//  1. if the duration was less than 1us, return the original string.
//  2. readable value >=10, keep 1 decimal, otherwise, keep 2 decimal. such as:
// 9.412345ms -> 9.41ms
//     10.412345ms -> 10.4ms
// 5.999s -> 6s
// 100.45µs -> 100.5µs
pub fn FormatDuration(mut d: StdDuration) -> String {
    if d <= StdDuration::from_micros(1) {
        return formatGoDuration(d);
    }
    let unit = getUnit(d);
    if unit == StdDuration::from_nanos(1) {
        return formatGoDuration(d);
    }
    // 按单位截整，再按 >=10 保留 1 位小数、否则 2 位做四舍五入。
    let unit_ns = unit.as_nanos();
    let d_ns = d.as_nanos();
    let integer_ns = (d_ns / unit_ns) * unit_ns;
    let scale = if d < unit * 10 { 100 } else { 10 };
    let roundedFraction = ((d_ns % unit_ns) * scale + unit_ns / 2) / unit_ns;
    let rounded_ns = integer_ns + roundedFraction * (unit_ns / scale);
    d = StdDuration::from_nanos(rounded_ns.min(u64::MAX as u128) as u64);
    formatGoDuration(d)
}

/// 模拟 Go `time.Duration` 字符串风格（ns/µs/ms/s 及 h/m 组合）。
fn formatGoDuration(d: StdDuration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return "0s".to_string();
    }
    if nanos < 1_000 {
        return format!("{nanos}ns");
    }
    if nanos < 1_000_000 {
        return formatDecimalDuration(nanos / 1_000, nanos % 1_000, 3, "µs");
    }
    if nanos < 1_000_000_000 {
        return formatDecimalDuration(
            nanos / 1_000_000,
            nanos % 1_000_000,
            6,
            "ms",
        );
    }

    let totalSeconds = nanos / 1_000_000_000;
    let hours = totalSeconds / 3_600;
    let minutes = (totalSeconds % 3_600) / 60;
    let seconds = totalSeconds % 60;
    let secondPart = formatDecimalDuration(
        seconds,
        nanos % 1_000_000_000,
        9,
        "s",
    );
    if hours > 0 {
        format!("{hours}h{minutes}m{secondPart}")
    } else if minutes > 0 {
        format!("{minutes}m{secondPart}")
    } else {
        secondPart
    }
}

/// 整部 + 去尾零小数 + 单位后缀。
fn formatDecimalDuration(whole: u128, fraction: u128, width: usize, suffix: &str) -> String {
    if fraction == 0 {
        return format!("{whole}{suffix}");
    }
    let mut fraction = format!("{fraction:0width$}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{whole}.{fraction}{suffix}")
}

/// 按量级选择格式化基准单位。
// getUnit 对应 Go 的内部辅助函数，按秒、毫秒、微秒、纳秒选择格式化单位。
fn getUnit(d: StdDuration) -> StdDuration {
    if d >= StdDuration::from_secs(1) {
        StdDuration::from_secs(1)
    } else if d >= StdDuration::from_millis(1) {
        StdDuration::from_millis(1)
    } else if d >= StdDuration::from_micros(1) {
        StdDuration::from_micros(1)
    } else {
        StdDuration::from_nanos(1)
    }
}
