// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

// 导出进度与速率统计，对应 Go `status.go`。
//
// 负责周期性日志输出、HTTP/CLI 可见的 DumpStatus 快照，以及按表类型统计导出范围。
// 指标来自 Prometheus counter/gauge；chunk 进度在 progressReady 后才写入 Progress 字段。

/// Snapshot sampling is independent of HTTP polling and progress logging.
pub const statusRefreshTick: Duration = Duration::from_secs(5);

pub const logProgressTick: Duration = Duration::from_secs(2 * 60);

/// The join guard mirrors Go's cancel-and-wait stop function, including repeated stop calls.
pub struct LogProgressGuard {
    cancel: astersql_dumpling_context::CancelFunc,
    worker: Option<thread::JoinHandle<()>>,
}
impl LogProgressGuard {
    pub fn stop(&mut self) {
        self.cancel.call();
        if let Some(worker) = self.worker.take() {
            worker.join().expect("dump progress worker panicked");
        }
    }
}
impl Drop for LogProgressGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Dumper {
    fn progressView(&self) -> Dumper {
        Dumper {
            tctx: self.tctx.clone(),
            conf: self.conf.clone(),
            db: None,
            ext_storage: None,
            metrics: self.metrics.clone(),
            speedRecorder: self.speedRecorder.clone(),
            status: self.status.clone(),
            totalTables: self.totalTables.clone(),
            cancel: None,
            http: None,
            pd_client: None,
        }
    }

    pub fn startLogProgress(&self, tctx: &tcontext::Context) -> LogProgressGuard {
        let (ctx, cancel) = tctx.WithCancel();
        let view = self.progressView();
        let worker = thread::spawn(move || view.runLogProgress(&ctx));
        LogProgressGuard {
            cancel,
            worker: Some(worker),
        }
    }

    pub fn runLogProgress(&self, tctx: &tcontext::Context) {
        self.runLogProgressWithTicks(
            tctx,
            statusRefreshTick,
            logProgressTick,
            failpoint_inject("EnableLogProgress"),
        );
    }

    // Tick durations are the explicit time boundary used by Rust's deterministic scoped tests.
    fn runLogProgressWithTicks(
        &self,
        tctx: &tcontext::Context,
        refresh_tick: Duration,
        log_tick: Duration,
        accelerated: bool,
    ) {
        self.RefreshStatus();
        let tick = if accelerated {
            Duration::from_secs(1)
        } else {
            log_tick
        };
        if accelerated {
            tctx.L().Debug("EnableLogProgress", []);
        }
        let mut last_checkpoint = Instant::now();
        let mut last_bytes = 0.0;
        let mut next_status = last_checkpoint + refresh_tick;
        let mut next_log = last_checkpoint + tick;
        loop {
            if tctx.Done() {
                tctx.L().Debug("stopping log progress", []);
                self.RefreshStatus();
                return;
            }
            let now = Instant::now();
            if now >= next_status {
                self.RefreshStatus();
                while next_status <= now {
                    next_status += refresh_tick;
                }
            }
            if now >= next_log {
                if accelerated {
                    self.RefreshStatus();
                }
                let nanoseconds = now.duration_since(last_checkpoint).as_nanos() as f64;
                let finished_bytes = ReadGauge(Some(&self.metrics.finishedSizeGauge));
                let s = self.GetStatus();
                let avg = (finished_bytes - last_bytes) / (1048576e-9 * nanoseconds);
                tctx.L().Info(
                    "progress",
                    [
                        Field::string(
                            "tables",
                            format!(
                                "{:.0}/{:.0} ({:.1}%)",
                                s.CompletedTables,
                                s.TotalTables,
                                s.CompletedTables / s.TotalTables as f64 * 100.0
                            ),
                        ),
                        Field::string("finished rows", format!("{:.0}", s.FinishedRows)),
                        Field::string("estimate total rows", format!("{:.0}", s.EstimateTotalRows)),
                        Field::string("finished size", HumanSize(s.FinishedBytes)),
                        Field::string("average speed(MiB/s)", avg.to_string()),
                        Field::string("recent speed bps", s.CurrentSpeedBPS.to_string()),
                        Field::string("chunks progress", s.Progress),
                    ],
                );
                last_checkpoint = Instant::now();
                last_bytes = finished_bytes;
                while next_log <= now {
                    next_log += tick;
                }
            }
            thread::sleep(
                next_status
                    .min(next_log)
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        }
    }

    /// Return an independent snapshot without changing the speed sampling window.
    pub fn GetStatus(&self) -> DumpStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn RefreshStatus(&self) {
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
                ret.setProgress(1.0);
                *self.status.lock().unwrap() = ret;
                return;
            }
            // completed/total 换算百分比字符串供日志与 HTTP 展示。
            let progress = self.metrics.completedChunks.load(Ordering::SeqCst) as f64
                / self.metrics.totalChunks.load(Ordering::SeqCst) as f64;
            if progress > 1.0 {
                // 计数器竞态可能导致 completed>total，钳制并告警。
                ret.setProgress(1.0);
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
                ret.setProgress(progress);
            }
        }
        *self.status.lock().unwrap() = ret;
    }
}

/// 对外暴露的导出快照；字段名与 Go `DumpStatus` JSON 标签一致。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DumpStatus {
    pub CompletedTables: f64,
    pub FinishedBytes: f64,
    pub FinishedRows: f64,
    pub EstimateTotalRows: f64,
    pub TotalTables: i64,
    pub CurrentSpeedBPS: f64,
    pub Progress: String,
    pub ProgressPercent: Option<f64>,
}

impl DumpStatus {
    fn setProgress(&mut self, fraction: f64) {
        self.ProgressPercent = Some(fraction * 100.0);
        self.Progress = if fraction >= 1.0 {
            "100 %".to_owned()
        } else {
            format!("{:5.2} %", fraction * 100.0)
        };
    }

    fn toJSON(&self) -> Result<String> {
        let values = [
            self.CompletedTables,
            self.FinishedBytes,
            self.FinishedRows,
            self.EstimateTotalRows,
            self.CurrentSpeedBPS,
        ];
        if values.iter().any(|n| !n.is_finite())
            || self.ProgressPercent.is_some_and(|n| !n.is_finite())
        {
            return Err(errors_new("unsupported non-finite JSON status value"));
        }
        let mut body = format!(
            "{{\"completedTables\":{},\"finishedBytes\":{},\"finishedRows\":{},\"estimateTotalRows\":{},\"totalTables\":{},\"currentSpeedBPS\":{}",
            self.CompletedTables,
            self.FinishedBytes,
            self.FinishedRows,
            self.EstimateTotalRows,
            self.TotalTables,
            self.CurrentSpeedBPS
        );
        if !self.Progress.is_empty() {
            body.push_str(&format!(",\"progress\":\"{}\"", self.Progress));
        }
        if let Some(percent) = self.ProgressPercent {
            body.push_str(&format!(",\"progressPercent\":{percent}"));
        }
        body.push_str("}\n");
        Ok(body)
    }
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
