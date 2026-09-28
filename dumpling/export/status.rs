// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

// 导出进度与速率统计，对应 Go `status.go`。
//
// 负责周期性日志输出、HTTP/CLI 可见的 DumpStatus 快照，以及按表类型统计导出范围。
// 指标来自 Prometheus counter/gauge；chunk 进度在 progressReady 后才写入 Progress 字段。

/// 进度日志 tick 间隔；Go 同名常量，默认 2 分钟。
pub const logProgressTick: Duration = Duration::from_secs(2 * 60);

impl Dumper {
    /// 周期性输出导出进度日志，直到 context 被取消；Go `runLogProgress`。
    ///
    /// failpoint `EnableLogProgress` 可将 tick 缩短为 1 秒便于测试。
    /// 平均 MiB/s 由两次采样间 FinishedBytes 差除以 elapsed 计算。
    pub fn runLogProgress(&self, tctx: &tcontext::Context) {
        let mut tick = logProgressTick;
        if failpoint_inject("EnableLogProgress") {
            // 测试 failpoint：加速进度日志以便集成测试观察。
            tick = Duration::from_secs(1);
            tctx.L().Debug("EnableLogProgress", []);
        }
        let mut last_checkpoint = Instant::now();
        let mut last_bytes = 0.0_f64;
        let mut next_tick = last_checkpoint + tick;
        loop {
            if tctx.Done() {
                tctx.L().Debug("stopping log progress", []);
                return;
            }

            let now = Instant::now();
            if now < next_tick {
                // Context exposes Go-style polling rather than a waitable channel. Poll at a
                // short interval so cancellation remains prompt while preserving ticker timing.
                thread::sleep((next_tick - now).min(Duration::from_millis(10)));
                continue;
            }

            let nanoseconds = now.duration_since(last_checkpoint).as_nanos() as f64;
            let s = self.GetStatus();
            // 汇总 tables/rows/size/chunk 等字段写入 progress 日志。
            // 与 Go 相同：用 1048576e-9 将字节/纳秒换算为 MiB/s。
            let avg = (s.FinishedBytes - last_bytes) / (1048576e-9 * nanoseconds);
            let total_tables = self.totalTables.load(Ordering::SeqCst) as f64;
            tctx.L().Info(
                "progress",
                [
                    Field::string(
                        "tables",
                        format!(
                            "{:.0}/{:.0} ({:.1}%)",
                            s.CompletedTables,
                            total_tables,
                            s.CompletedTables / total_tables * 100.0
                        ),
                    ),
                    Field::string("finished rows", format!("{:.0}", s.FinishedRows)),
                    Field::string("estimate total rows", format!("{:.0}", s.EstimateTotalRows)),
                    Field::string("finished size", HumanSize(s.FinishedBytes)),
                    Field::string("average speed(MiB/s)", avg.to_string()),
                    Field::string("recent speed bps", s.CurrentSpeedBPS.to_string()),
                    Field::string("chunks progress", s.Progress.clone()),
                ],
            );
            last_checkpoint = Instant::now();
            last_bytes = s.FinishedBytes;
            next_tick = last_checkpoint + tick;
        }
    }

    /// 聚合当前导出指标为 `DumpStatus`；Go `GetStatus`。
    ///
    /// chunk 百分比仅在 `progressReady` 为真时填充；completed > total 时钳制为 100% 并打 Warn。
    pub fn GetStatus(&self) -> DumpStatus {
        let mut ret = DumpStatus::default();
        ret.TotalTables = self.totalTables.load(Ordering::SeqCst);
        ret.CompletedTables = ReadCounter(Some(&self.metrics.finishedTablesCounter));
        ret.FinishedBytes = ReadGauge(Some(&self.metrics.finishedSizeGauge));
        ret.FinishedRows = ReadGauge(Some(&self.metrics.finishedRowsGauge));
        ret.EstimateTotalRows = ReadCounter(Some(&self.metrics.estimateTotalRowsCounter));
        // 字节/秒滑动窗口速率，与 FinishedBytes 累计值联动。
        ret.CurrentSpeedBPS = self
            .speedRecorder
            .lock()
            .unwrap()
            .GetSpeed(ret.FinishedBytes);
        if self.metrics.progressReady.load(Ordering::SeqCst) {
            // chunk 进度条仅在 dump 初始化完分片计数后启用。
            if self.metrics.totalChunks.load(Ordering::SeqCst) == 0 {
                // 无 chunk 任务时视为已全部完成。
                ret.Progress = "100 %".to_string();
                return ret;
            }
            // completed/total 换算百分比字符串供日志与 HTTP 展示。
            let progress = self.metrics.completedChunks.load(Ordering::SeqCst) as f64
                / self.metrics.totalChunks.load(Ordering::SeqCst) as f64;
            if progress > 1.0 {
                // 计数器竞态可能导致 completed>total，钳制并告警。
                ret.Progress = "100 %".to_string();
                self.L().Warn(
                    "completedChunks is greater than totalChunks",
                    [
                        Field::string(
                            "completedChunks",
                            self.metrics
                                .completedChunks
                                .load(Ordering::SeqCst)
                                .to_string(),
                        ),
                        Field::string(
                            "totalChunks",
                            self.metrics.totalChunks.load(Ordering::SeqCst).to_string(),
                        ),
                    ],
                );
            } else {
                ret.Progress = format!("{:5.2} %", progress * 100.0);
            }
        }
        ret
    }
}

/// 对外暴露的导出快照；字段名与 Go `DumpStatus` JSON 标签一致。
#[derive(Clone, Debug, Default)]
pub struct DumpStatus {
    pub CompletedTables: f64,
    pub FinishedBytes: f64,
    pub FinishedRows: f64,
    pub EstimateTotalRows: f64,
    pub TotalTables: i64,
    pub CurrentSpeedBPS: f64,
    pub Progress: String,
}

/// 统计 `DatabaseTables` 中基表（非视图/序列）数量；Go `calculateTableCount`。
pub fn calculateTableCount(m: &DatabaseTables) -> i32 {
    let mut cnt = 0;
    for tables in m.values() {
        for table in tables {
            // 视图/序列不计入 totalTables 进度分母。
            if table.Type == TableType::TableTypeBase {
                cnt += 1;
            }
        }
    }
    cnt
}

/// 滑动窗口速率记录器；Go `SpeedRecorder`。
///
/// 仅在 finished 单调递增且 elapsed>0 时更新 speed_bps；零速时保底为 1 避免除零展示问题。
pub struct SpeedRecorder {
    last_finished: f64,
    last_update_time: Instant,
    speed_bps: f64,
}

/// 构造初始速率为 0 的记录器。
pub fn NewSpeedRecorder() -> SpeedRecorder {
    SpeedRecorder {
        last_finished: 0.0,
        last_update_time: Instant::now(),
        speed_bps: 0.0,
    }
}

impl SpeedRecorder {
    /// 根据累计 finished 字节更新并返回当前 bytes/s；Go `GetSpeed`。
    pub fn GetSpeed(&mut self, finished: f64) -> f64 {
        // finished 未增长时返回上次速率，避免除零抖动。
        if finished <= self.last_finished {
            return self.speed_bps;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update_time).as_secs_f64();
        // 同一时刻多次采样则沿用旧速率。
        if elapsed == 0.0 {
            return self.speed_bps;
        }
        // delta_bytes / delta_seconds → bytes per second。
        let mut current_speed = (finished - self.last_finished) / elapsed;
        if current_speed == 0.0 {
            current_speed = 1.0;
        }
        // 更新基线供下次 delta 计算。
        self.last_finished = finished;
        self.last_update_time = now;
        self.speed_bps = current_speed;
        current_speed
    }
}
