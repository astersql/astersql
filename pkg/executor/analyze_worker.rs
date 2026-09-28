// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// ANALYZE 结果落盘 worker。
//
// 从结果通道取出各分区/任务的统计结果，写入存储并更新作业状态。
// 收到 kill（取消）信号后切换为 drain 模式：继续取完通道中的结果但不落盘，
// 普通落盘失败则不影响后续分区的重试机会。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::any::Any;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 将 ANALYZE 结果持久化到存储时产生的错误。
pub struct AnalyzeSaveError(pub String);

impl fmt::Display for AnalyzeSaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AnalyzeSaveError {}

/// ANALYZE 结果落盘操作的统一 Result 别名。
pub type AnalyzeSaveResult<T = ()> = Result<T, AnalyzeSaveError>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单次 ANALYZE 保存任务的上下文（含请求标识）。
pub struct analyzeContext {
    pub requestID: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 后台 ANALYZE 作业描述：库表与可选分区名。
pub struct analyzeJob {
    pub id: u64,
    pub databaseName: String,
    pub tableName: String,
    pub partitionName: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 待落盘的 ANALYZE 结果载荷（含表 ID、行数估计与序列化 payload）。
pub struct analyzeResults {
    pub job: Option<analyzeJob>,
    pub tableID: i64,
    pub count: i64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 统计元数据历史记录的来源标记。
pub enum statsMetaHistorySource {
    Analyze,
}

/// 落盘 worker 依赖的运行时边界：杀进程信号、写存储、收尾与日志。
pub trait analyzeSaveStatsRuntime: Send + Sync {
    fn handle_kill_signal(&self) -> AnalyzeSaveResult;
    fn save_analyze_result_to_storage(
        &self,
        context: &analyzeContext,
        result: &mut analyzeResults,
        analyze_snapshot: bool,
        source: statsMetaHistorySource,
    ) -> AnalyzeSaveResult;
    fn finish_analyze_job(&self, job: Option<&analyzeJob>, error: Option<&AnalyzeSaveError>);
    fn destroy_and_put_to_pool(&self, result: analyzeResults);
    fn log_save_warning(&self, context: &analyzeContext, error: &AnalyzeSaveError);
    fn log_worker_panic(&self, panic_message: &str);
    fn analyze_panic_error(&self, panic_message: &str) -> AnalyzeSaveError;
    fn log_error_channel_closed(&self, error: &AnalyzeSaveError);
}

/// 消费 ANALYZE 结果通道并持久化的后台 worker。
pub struct analyzeSaveStatsWorker {
    pub resultsCh: mpsc::Receiver<analyzeResults>,
    /// Unbounded by design: each worker reports at most one error, and reporting
    /// must never block result draining if the caller chose a small capacity.
    pub errCh: mpsc::Sender<AnalyzeSaveError>,
    pub runtime: Arc<dyn analyzeSaveStatsRuntime>,
}

/// 构造结果落盘 worker。
pub fn newAnalyzeSaveStatsWorker(
    resultsCh: mpsc::Receiver<analyzeResults>,
    errCh: mpsc::Sender<AnalyzeSaveError>,
    runtime: Arc<dyn analyzeSaveStatsRuntime>,
) -> analyzeSaveStatsWorker {
    analyzeSaveStatsWorker {
        resultsCh,
        errCh,
        runtime,
    }
}

/// 从 catch_unwind 载荷中提取可读 panic 消息。
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".into()
    }
}

impl analyzeSaveStatsWorker {
    // 每个 worker 最多向错误通道上报一次，避免阻塞结果排空。
    /// 向错误通道至多上报一次；通道已关闭则记日志。
    fn report_once(&self, error: &AnalyzeSaveError, reported: &mut bool) {
        if *reported {
            return;
        }
        *reported = true;
        if self.errCh.send(error.clone()).is_err() {
            self.runtime.log_error_channel_closed(error);
        }
    }

    // run consumes analyze results and persists them. A kill switches the
    // worker into drain mode; an ordinary save failure does not.
    /// 循环接收结果并落盘；收到 kill 后进入 drain（只排空不落盘）模式。
    /// analyzeSnapshot：是否按快照（MVCC 可见版本）分析。
    pub fn run(&self, context: &analyzeContext, analyzeSnapshot: bool) {
        let mut error_reported = false;
        let execution = catch_unwind(AssertUnwindSafe(|| {
            let mut drain_error = None::<AnalyzeSaveError>;
            while let Ok(mut results) = self.resultsCh.recv() {
                if let Some(error) = &drain_error {
                    self.runtime
                        .finish_analyze_job(results.job.as_ref(), Some(error));
                    self.runtime.destroy_and_put_to_pool(results);
                    continue;
                }

                if let Err(error) = self.runtime.handle_kill_signal() {
                    drain_error = Some(error.clone());
                    self.runtime
                        .finish_analyze_job(results.job.as_ref(), Some(&error));
                    self.runtime.destroy_and_put_to_pool(results);
                    self.report_once(&error, &mut error_reported);
                    continue;
                }

                match self.runtime.save_analyze_result_to_storage(
                    context,
                    &mut results,
                    analyzeSnapshot,
                    statsMetaHistorySource::Analyze,
                ) {
                    Ok(()) => self.runtime.finish_analyze_job(results.job.as_ref(), None),
                    Err(error) => {
                        self.runtime.log_save_warning(context, &error);
                        self.runtime
                            .finish_analyze_job(results.job.as_ref(), Some(&error));
                        self.report_once(&error, &mut error_reported);
                        // Save errors deliberately do not enter drain mode. Later
                        // partitions still get a persistence attempt.
                    }
                }
                self.runtime.destroy_and_put_to_pool(results);
            }
        }));

        if let Err(payload) = execution {
            let message = panic_message(payload.as_ref());
            self.runtime.log_worker_panic(&message);
            let error = self.runtime.analyze_panic_error(&message);
            self.report_once(&error, &mut error_reported);
        }
    }
}
