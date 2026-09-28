// Copyright 2022 PingCAP, Inc.
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

// TTL Worker 观测指标与相位追踪实现。
//
// 在 `metrics` 向量指标之上预绑定常用标签句柄；提供 PhaseTracer
//（记录 worker 在 idle/begin_txn/query 等相位停留时长），以及水位调度延迟分桶更新。

#![allow(non_snake_case, non_upper_case_globals)]

use crate::metrics;
use prometheus::{Counter, Gauge, Histogram};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Worker 空闲相位名。
pub const PhaseIdle: &str = "idle";
/// 开启事务相位名（BEGIN）。
pub const PhaseBeginTxn: &str = "begin_txn";
/// 提交事务相位名（COMMIT）。
pub const PhaseCommitTxn: &str = "commit_txn";
/// 执行查询相位名。
pub const PhaseQuery: &str = "query";
/// 检查 TTL 元数据相位名。
pub const PhaseCheckTTL: &str = "check_ttl";
/// 等待重试相位名。
pub const PhaseWaitRetry: &str = "wait_retry";
/// 任务派发相位名。
pub const PhaseDispatch: &str = "dispatch";
/// 等待令牌（限流）相位名。
pub const PhaseWaitToken: &str = "wait_token";
/// 其它未归类相位名。
pub const PhaseOther: &str = "other";

/// SELECT 成功耗时直方图句柄。
pub static SelectSuccessDuration: LazyLock<Histogram> =
    LazyLock::new(|| metrics::TTLQueryDuration.with_label_values(&["select", metrics::LblOK]));
/// SELECT 失败耗时直方图句柄。
pub static SelectErrorDuration: LazyLock<Histogram> =
    LazyLock::new(|| metrics::TTLQueryDuration.with_label_values(&["select", metrics::LblError]));
/// DELETE 成功耗时直方图句柄。
pub static DeleteSuccessDuration: LazyLock<Histogram> =
    LazyLock::new(|| metrics::TTLQueryDuration.with_label_values(&["delete", metrics::LblOK]));
/// DELETE 失败耗时直方图句柄。
pub static DeleteErrorDuration: LazyLock<Histogram> =
    LazyLock::new(|| metrics::TTLQueryDuration.with_label_values(&["delete", metrics::LblError]));

/// 扫描到的过期行计数句柄。
pub static ScannedExpiredRows: LazyLock<Counter> = LazyLock::new(|| {
    metrics::TTLProcessedExpiredRowsCounter.with_label_values(&["select", metrics::LblOK])
});
/// 删除成功的过期行计数句柄。
pub static DeleteSuccessExpiredRows: LazyLock<Counter> = LazyLock::new(|| {
    metrics::TTLProcessedExpiredRowsCounter.with_label_values(&["delete", metrics::LblOK])
});
/// 删除失败的过期行计数句柄。
pub static DeleteErrorExpiredRows: LazyLock<Counter> = LazyLock::new(|| {
    metrics::TTLProcessedExpiredRowsCounter.with_label_values(&["delete", metrics::LblError])
});

/// 正在运行的 Job 数。
pub static RunningJobsCnt: LazyLock<Gauge> =
    LazyLock::new(|| metrics::TTLJobStatus.with_label_values(&["running"]));
/// 正在取消的 Job 数。
pub static CancellingJobsCnt: LazyLock<Gauge> =
    LazyLock::new(|| metrics::TTLJobStatus.with_label_values(&["cancelling"]));
/// 处于扫描中的 Task 数。
pub static ScanningTaskCnt: LazyLock<Gauge> =
    LazyLock::new(|| metrics::TTLTaskStatus.with_label_values(&["scanning"]));
/// 处于删除中的 Task 数。
pub static DeletingTaskCnt: LazyLock<Gauge> =
    LazyLock::new(|| metrics::TTLTaskStatus.with_label_values(&["deleting"]));

/// 水位调度延迟分桶：展示名与上限阈值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaterMarkScheduleDelayName {
    /// 分桶展示名（写入 metrics 标签）。
    pub Name: &'static str,
    /// 该分桶覆盖的最大延迟。
    pub Delay: Duration,
}

/// 与 Go 侧一致的水位调度延迟分桶表（含重复 02 hour / one week 阈值）。
pub static WaterMarkScheduleDelayNames: &[WaterMarkScheduleDelayName] = &[
    WaterMarkScheduleDelayName {
        Name: "01 hour",
        Delay: Duration::from_secs(60 * 60),
    },
    WaterMarkScheduleDelayName {
        Name: "02 hour",
        Delay: Duration::from_secs(60 * 60),
    },
    WaterMarkScheduleDelayName {
        Name: "06 hour",
        Delay: Duration::from_secs(6 * 60 * 60),
    },
    WaterMarkScheduleDelayName {
        Name: "12 hour",
        Delay: Duration::from_secs(12 * 60 * 60),
    },
    WaterMarkScheduleDelayName {
        Name: "24 hour",
        Delay: Duration::from_secs(24 * 60 * 60),
    },
    WaterMarkScheduleDelayName {
        Name: "72 hour",
        Delay: Duration::from_secs(72 * 60 * 60),
    },
    WaterMarkScheduleDelayName {
        Name: "one week",
        Delay: Duration::from_secs(72 * 60 * 60),
    },
    WaterMarkScheduleDelayName {
        Name: "others",
        Delay: Duration::from_nanos(i64::MAX as u64),
    },
];

/// 初始化入口：强制求值所有 LazyLock 指标句柄。
pub fn init() {
    InitMetricsVars();
}

/// 强制初始化查询耗时、行计数、Job/Task 状态与 worker 相位 Counter。
pub fn InitMetricsVars() {
    LazyLock::force(&SelectSuccessDuration);
    LazyLock::force(&SelectErrorDuration);
    LazyLock::force(&DeleteSuccessDuration);
    LazyLock::force(&DeleteErrorDuration);
    LazyLock::force(&ScannedExpiredRows);
    LazyLock::force(&DeleteSuccessExpiredRows);
    LazyLock::force(&DeleteErrorExpiredRows);
    LazyLock::force(&RunningJobsCnt);
    LazyLock::force(&CancellingJobsCnt);
    LazyLock::force(&ScanningTaskCnt);
    LazyLock::force(&DeletingTaskCnt);
    LazyLock::force(&scanWorkerPhases);
    LazyLock::force(&deleteWorkerPhases);
}

/// 为指定 worker 类型构造 phase → Counter 映射。
pub fn initWorkerPhases(workerType: &str) -> HashMap<String, Counter> {
    [
        PhaseIdle,
        PhaseBeginTxn,
        PhaseCommitTxn,
        PhaseQuery,
        PhaseWaitRetry,
        PhaseDispatch,
        PhaseCheckTTL,
        PhaseWaitToken,
        PhaseOther,
    ]
    .into_iter()
    .map(|phase| {
        (
            phase.to_owned(),
            metrics::TTLPhaseTime.with_label_values(&[workerType, phase]),
        )
    })
    .collect()
}

/// 扫描 Worker 各相位耗时 Counter 表。
pub static scanWorkerPhases: LazyLock<HashMap<String, Counter>> =
    LazyLock::new(|| initWorkerPhases("scan_worker"));
/// 删除 Worker 各相位耗时 Counter 表。
pub static deleteWorkerPhases: LazyLock<HashMap<String, Counter>> =
    LazyLock::new(|| initWorkerPhases("delete_worker"));

/// 相位追踪器：在相位切换时把上一相位停留时长交给回调记录。
pub struct PhaseTracer {
    /// 取当前时间的可注入时钟（测试可伪造）。
    getTime: Box<dyn Fn() -> Instant + Send + Sync>,
    /// 记录相位耗时的回调（生产环境累加到 Prometheus Counter）。
    recordDuration: Box<dyn Fn(&str, Duration) + Send + Sync>,
    /// 当前相位名；空串表示未进入任何相位。
    phase: String,
    /// 进入当前相位的时刻。
    phaseTime: Instant,
}

/// 创建绑定到扫描 Worker 相位 Counter 的追踪器。
pub fn NewScanWorkerPhaseTracer() -> PhaseTracer {
    newPhaseTracer(Instant::now, |phase, duration| {
        if let Some(counter) = scanWorkerPhases.get(phase) {
            counter.inc_by(duration.as_secs_f64());
        }
    })
}

/// 创建绑定到删除 Worker 相位 Counter 的追踪器。
pub fn NewDeleteWorkerPhaseTracer() -> PhaseTracer {
    newPhaseTracer(Instant::now, |phase, duration| {
        if let Some(counter) = deleteWorkerPhases.get(phase) {
            counter.inc_by(duration.as_secs_f64());
        }
    })
}

/// 构造可注入时钟与记录回调的 PhaseTracer（便于单测）。
pub fn newPhaseTracer<G, R>(getTime: G, recordDuration: R) -> PhaseTracer
where
    G: Fn() -> Instant + Send + Sync + 'static,
    R: Fn(&str, Duration) + Send + Sync + 'static,
{
    let phaseTime = getTime();
    PhaseTracer {
        getTime: Box::new(getTime),
        recordDuration: Box::new(recordDuration),
        phase: String::new(),
        phaseTime,
    }
}

impl PhaseTracer {
    /// 返回当前相位名。
    pub fn Phase(&self) -> &str {
        &self.phase
    }

    /// 进入新相位：若已有相位则先上报其停留时长。
    pub fn EnterPhase(&mut self, phase: &str) {
        let now = (self.getTime)();
        // 非空相位才记时，避免初始空相位产生噪声。
        if !self.phase.is_empty() {
            (self.recordDuration)(&self.phase, now.duration_since(self.phaseTime));
        }
        self.phase.clear();
        self.phase.push_str(phase);
        self.phaseTime = now;
    }

    /// 结束当前相位（切到空相位并上报上一相位耗时）。
    pub fn EndPhase(&mut self) {
        self.EnterPhase("");
    }
}

/// 可挂载 PhaseTracer 的轻量上下文（对应 Go context.WithValue）。
#[derive(Clone, Default)]
pub struct PhaseContext {
    tracer: Option<Arc<Mutex<PhaseTracer>>>,
}

/// 将 PhaseTracer 写入上下文并返回新上下文。
pub fn CtxWithPhaseTracer(mut ctx: PhaseContext, tracer: Arc<Mutex<PhaseTracer>>) -> PhaseContext {
    ctx.tracer = Some(tracer);
    ctx
}

/// 从上下文取出 PhaseTracer；缺失时返回 None。
pub fn PhaseTracerFromCtx(ctx: &PhaseContext) -> Option<Arc<Mutex<PhaseTracer>>> {
    ctx.tracer.clone()
}

/// 单表水位延迟观测记录。
#[derive(Clone, Debug)]
pub struct DelayMetricsRecord {
    /// 表 ID。
    pub TableID: i64,
    /// 上次 Job 完成时间。
    pub LastJobTime: SystemTime,
    /// 绝对延迟（相对墙钟）。
    pub AbsoluteDelay: Duration,
    /// 相对调度间隔的延迟，用于分桶。
    pub ScheduleRelativeDelay: Duration,
}

impl DelayMetricsRecord {
    /// 构造一条水位延迟记录。
    pub fn new(
        TableID: i64,
        LastJobTime: SystemTime,
        AbsoluteDelay: Duration,
        ScheduleRelativeDelay: Duration,
    ) -> Self {
        Self {
            TableID,
            LastJobTime,
            AbsoluteDelay,
            ScheduleRelativeDelay,
        }
    }
}

/// 将延迟映射到最早满足 `t <= Delay` 的分桶名。
pub fn getWaterMarkScheduleDelayName(t: Duration) -> &'static str {
    WaterMarkScheduleDelayNames
        .iter()
        .find(|bucket| t <= bucket.Delay)
        .unwrap_or_else(|| WaterMarkScheduleDelayNames.last().unwrap())
        .Name
}

/// 按调度相对延迟分桶汇总并写入 `TTLWatermarkDelay` Gauge。
pub fn UpdateDelayMetrics(records: &HashMap<i64, DelayMetricsRecord>) {
    // 先清零各分桶计数，再按记录累加，保证空输入会把 Gauge 置 0。
    let mut scheduleMetrics: HashMap<&'static str, f64> = WaterMarkScheduleDelayNames
        .iter()
        .map(|bucket| (bucket.Name, 0.0))
        .collect();
    for record in records.values() {
        *scheduleMetrics
            .entry(getWaterMarkScheduleDelayName(record.ScheduleRelativeDelay))
            .or_default() += 1.0;
    }
    for (delay, value) in scheduleMetrics {
        metrics::TTLWatermarkDelay
            .with_label_values(&["schedule", delay])
            .set(value);
    }
}

/// 重置水位延迟 Gauge 上的全部样本。
pub fn ClearDelayMetrics() {
    metrics::TTLWatermarkDelay.reset();
}
