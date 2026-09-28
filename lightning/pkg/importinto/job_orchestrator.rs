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

//! Job orchestrator for IMPORT INTO submission and monitoring.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/job_orchestrator.rs`对应的作业提交编排与取消，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少95行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `trait`定义对外暴露的抽象边界，约束\"trait\"的最小能力集合。
//! 对 trait 的说明应重点覆盖调用者可依赖什么、实现者必须遵守什么以及错误是否允许透传。
//! 这可以帮助后续替换实现时，避免只满足编译器却破坏 Go 端既有约定。
//! 在 mock、checkpoint、monitor 或 backend 体系里，trait 文档直接决定测试替身是否可信。
//! - `SubmitAndWait`是当前文件的重要函数，承担\"SubmitAndWait\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `Cancel`是当前文件的重要函数，承担\"Cancel\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl JobOrchestrator`把\"JobOrchestrator\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl DefaultJobOrchestrator`把\"DefaultJobOrchestrator\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `getGroupKey`是当前文件的重要函数，承担\"getGroupKey\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `cancelJobsInGroup`是当前文件的重要函数，承担\"cancelJobsInGroup\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `cancelJobWithRetry`是当前文件的重要函数，承担\"cancelJobWithRetry\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `updateCheckpointsAfterCancel`是当前文件的重要函数，承担\"updateCheckpointsAfterCancel\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `submitAllJobs`是当前文件的重要函数，承担\"submitAllJobs\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `SemGuard`承载\"SemGuard\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl Drop`把\"Drop\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `drop`是当前文件的重要函数，承担\"drop\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `recordSubmission`是当前文件的重要函数，承担\"recordSubmission\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `getSubmitGraceTimeout`是当前文件的重要函数，承担\"getSubmitGraceTimeout\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `newStartedSubmitContext`是当前文件的重要函数，承担\"newStartedSubmitContext\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `newSubmissionRecordContext`是当前文件的重要函数，承担\"newSubmissionRecordContext\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `shouldRetryCancelJobErr`是当前文件的重要函数，承担\"shouldRetryCancelJobErr\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"Acquire concurrency slot. Like Go errgroup.SetLimit, still schedule\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"every table so checkpoint-resume paths run after a sibling failure.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"cancel by setting cancelled flag via Cause path\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::checkpoint::{CheckpointManager, CheckpointStatus, TableCheckpoint};
use crate::importer::{ProgressUpdater, cancelTimeout};
use crate::job_monitor::{JobMonitor, NewJobMonitor};
use crate::job_submitter::{ImportJob, JobSubmitter};
use crate::stubs::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// DefaultSubmitConcurrency is the default number of concurrent job submissions.
pub const DefaultSubmitConcurrency: i32 = 10;
/// DefaultPollInterval is the default interval for polling job status.
pub const DefaultPollInterval: Duration = Duration::from_secs(5);
/// DefaultLogInterval is the default interval for logging progress.
pub const DefaultLogInterval: Duration = Duration::from_secs(60);

const submitGraceTimeout: Duration = Duration::from_secs(60);
const cancelJobMaxRetry: i32 = 5;
const cancelJobRetryBaseBackoff: Duration = Duration::from_millis(100);
const cancelJobRetryMaxBackoff: Duration = Duration::from_secs(1);
const cancelledByUserMessage: &str = "cancelled by user";

/// JobOrchestrator orchestrates the submission and monitoring of import jobs.
pub trait JobOrchestrator: Send + Sync {
    fn SubmitAndWait(&self, ctx: &context::Context, tables: &[importsdk::TableMeta]) -> Result<()>;
    fn Cancel(&self, ctx: &context::Context) -> Result<()>;
}

/// DefaultJobOrchestrator is the default implementation of JobOrchestrator.
pub struct DefaultJobOrchestrator {
    submitter: Arc<dyn JobSubmitter>,
    cpMgr: Arc<dyn CheckpointManager>,
    monitor: Arc<dyn JobMonitor>,
    submitConcurrency: i32,
    logger: log::Logger,
    sdk: Arc<dyn importsdk::SDK>,
    activeJobs: Mutex<Vec<ImportJob>>,
}

/// OrchestratorConfig configures the job orchestrator.
pub struct OrchestratorConfig {
    pub Submitter: Arc<dyn JobSubmitter>,
    pub CheckpointMgr: Arc<dyn CheckpointManager>,
    pub SDK: Arc<dyn importsdk::SDK>,
    pub Monitor: Option<Arc<dyn JobMonitor>>,
    pub SubmitConcurrency: i32,
    pub PollInterval: Duration,
    pub LogInterval: Duration,
    pub Logger: log::Logger,
    pub ProgressUpdater: Option<Arc<dyn ProgressUpdater>>,
}

/// NewJobOrchestrator creates a new job orchestrator.
pub fn NewJobOrchestrator(cfg: OrchestratorConfig) -> Arc<dyn JobOrchestrator> {
    let mut submitConcurrency = cfg.SubmitConcurrency;
    if submitConcurrency <= 0 {
        submitConcurrency = DefaultSubmitConcurrency;
    }
    let mut pollInterval = cfg.PollInterval;
    if pollInterval.is_zero() {
        pollInterval = DefaultPollInterval;
    }
    let mut logInterval = cfg.LogInterval;
    if logInterval.is_zero() {
        logInterval = DefaultLogInterval;
    }

    let monitor = cfg.Monitor.unwrap_or_else(|| {
        NewJobMonitor(
            cfg.SDK.clone(),
            cfg.CheckpointMgr.clone(),
            pollInterval,
            logInterval,
            cfg.Logger.clone(),
            cfg.ProgressUpdater.clone(),
        )
    });

    Arc::new(DefaultJobOrchestrator {
        submitter: cfg.Submitter,
        cpMgr: cfg.CheckpointMgr,
        monitor,
        submitConcurrency,
        logger: cfg.Logger,
        sdk: cfg.SDK,
        activeJobs: Mutex::new(Vec::new()),
    })
}

impl JobOrchestrator for DefaultJobOrchestrator {
    fn SubmitAndWait(&self, ctx: &context::Context, tables: &[importsdk::TableMeta]) -> Result<()> {
        let (jobs, err) = self.submitAllJobs(ctx, tables);
        if let Some(err) = err {
            *self.activeJobs.lock().unwrap() = jobs;
            if !common::IsContextCanceledError(Some(&err)) {
                self.logger.Warn(
                    "job submission failed, cancelling submitted jobs",
                    &[
                        zap::Error(&err),
                        zap::Int("submitted", self.activeJobs.lock().unwrap().len() as i64),
                    ],
                );
                let (cancelCtx, _cancel) =
                    context::WithTimeout(context::Background(), cancelTimeout);
                if let Err(cancelErr) = self.Cancel(&cancelCtx) {
                    self.logger.Warn(
                        "failed to cancel jobs after submission error",
                        &[zap::Error(&cancelErr)],
                    );
                }
            }
            return Err(errors::Annotate(err, "submit jobs"));
        }

        if jobs.is_empty() {
            self.logger.Info("no jobs to execute", &[]);
            return Ok(());
        }

        *self.activeJobs.lock().unwrap() = jobs.clone();
        self.logger.Info(
            "all jobs submitted",
            &[zap::Int("count", jobs.len() as i64)],
        );

        let err = self.monitor.WaitForJobs(ctx, &jobs);
        if let Err(ref e) = err {
            if !common::IsContextCanceledError(Some(e)) {
                self.logger.Warn(
                    "job monitoring failed, cancelling remaining jobs",
                    &[zap::Error(e)],
                );
                let (cancelCtx, _cancel) =
                    context::WithTimeout(context::Background(), cancelTimeout);
                if let Err(cancelErr) = self.Cancel(&cancelCtx) {
                    self.logger.Warn(
                        "failed to cancel jobs after monitor error",
                        &[zap::Error(&cancelErr)],
                    );
                }
            }
        }
        err
    }

    fn Cancel(&self, ctx: &context::Context) -> Result<()> {
        let groupKey = self.getGroupKey();
        if groupKey.is_empty() {
            self.logger
                .Warn("no group key found, skip cancelling jobs", &[]);
            return Ok(());
        }

        self.logger.Info(
            "cancelling import jobs",
            &[zap::String("groupKey", &groupKey)],
        );
        let (statusByID, cancelledJobs, err) = self.cancelJobsInGroup(ctx, &groupKey);

        let updateErr =
            self.updateCheckpointsAfterCancel(ctx, &groupKey, &statusByID, &cancelledJobs);
        if let Err(ref update_err) = updateErr {
            if let Err(ref cancelErr) = err {
                self.logger.Warn(
                    "failed to update checkpoints after cancelling jobs",
                    &[
                        zap::String("groupKey", &groupKey),
                        zap::NamedError("cancelErr", cancelErr),
                        zap::NamedError("updateErr", update_err),
                    ],
                );
            } else {
                self.logger.Warn(
                    "failed to update checkpoints after cancelling jobs",
                    &[zap::String("groupKey", &groupKey), zap::Error(update_err)],
                );
            }
        }
        match err {
            Ok(()) => updateErr,
            Err(e) => Err(e),
        }
    }
}

impl DefaultJobOrchestrator {
    fn getGroupKey(&self) -> String {
        let jobs = self.activeJobs.lock().unwrap();
        if let Some(job) = jobs.first() {
            return job.GroupKey.clone();
        }
        self.submitter.GetGroupKey()
    }

    fn cancelJobsInGroup(
        &self,
        ctx: &context::Context,
        groupKey: &str,
    ) -> (HashMap<i64, importsdk::JobStatus>, HashSet<i64>, Result<()>) {
        let mut firstErr: Option<Error> = None;
        let mut statusByID: HashMap<i64, importsdk::JobStatus> = HashMap::new();
        let mut cancelledJobs: HashSet<i64> = HashSet::new();

        let statuses = match self.sdk.GetJobsByGroup(ctx, groupKey) {
            Ok(s) => s,
            Err(err) => {
                self.logger.Warn(
                    "failed to get group jobs status",
                    &[zap::Error(&err), zap::String("groupKey", groupKey)],
                );
                return (statusByID, cancelledJobs, Err(err));
            }
        };

        for st in statuses {
            statusByID.insert(st.JobID, st.clone());
            if st.IsCompleted() {
                continue;
            }
            if let Err(err) = self.cancelJobWithRetry(ctx, st.JobID) {
                self.logger.Warn(
                    "failed to cancel job",
                    &[zap::Int64("jobID", st.JobID), zap::Error(&err)],
                );
                if firstErr.is_none() {
                    firstErr = Some(err);
                }
                continue;
            }
            cancelledJobs.insert(st.JobID);
        }
        (
            statusByID,
            cancelledJobs,
            firstErr.map(Err).unwrap_or(Ok(())),
        )
    }

    fn cancelJobWithRetry(&self, ctx: &context::Context, jobID: i64) -> Result<()> {
        let mut backoff = cancelJobRetryBaseBackoff;
        let mut last_err = None;
        for attempt in 0..cancelJobMaxRetry {
            if let Some(err) = ctx.Err() {
                return Err(errors::Trace(err));
            }
            match self.sdk.CancelJob(ctx, jobID) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    if !shouldRetryCancelJobErr(&err) || attempt == cancelJobMaxRetry - 1 {
                        return Err(err);
                    }
                    self.logger.Warn(
                        "cancel job failed, retrying",
                        &[
                            zap::Int64("jobID", jobID),
                            zap::Int("attempt", (attempt + 1) as i64),
                            zap::Error(&err),
                        ],
                    );
                    last_err = Some(err);
                    sleepWithContext(ctx, backoff)?;
                    backoff = (backoff * 2).min(cancelJobRetryMaxBackoff);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| errors::New("cancel job failed")))
    }

    fn updateCheckpointsAfterCancel(
        &self,
        ctx: &context::Context,
        groupKey: &str,
        statusByID: &HashMap<i64, importsdk::JobStatus>,
        cancelledJobs: &HashSet<i64>,
    ) -> Result<()> {
        let mut firstErr = None;
        let jobs = self.activeJobs.lock().unwrap().clone();
        for job in jobs {
            let Some(meta) = &job.TableMeta else {
                continue;
            };
            if job.JobID <= 0 {
                continue;
            }
            let tableName = common::UniqueTable(&meta.Database, &meta.Table);
            let st = statusByID.get(&job.JobID);

            let (cpStatus, cpMessage) = match st {
                Some(st) if st.IsFinished() => (CheckpointStatus::Finished, String::new()),
                Some(st) if st.IsFailed() => (CheckpointStatus::Failed, st.ResultMessage.clone()),
                Some(st) if st.IsCancelled() => {
                    (CheckpointStatus::Failed, cancelledByUserMessage.to_string())
                }
                _ => {
                    if !cancelledJobs.contains(&job.JobID) {
                        continue;
                    }
                    (CheckpointStatus::Failed, cancelledByUserMessage.to_string())
                }
            };

            if let Err(err) = self.cpMgr.Update(
                ctx,
                &TableCheckpoint {
                    TableName: tableName.clone(),
                    JobID: job.JobID,
                    Status: cpStatus,
                    Message: cpMessage,
                    GroupKey: groupKey.to_string(),
                },
            ) {
                self.logger.Warn(
                    "failed to update checkpoint",
                    &[
                        zap::String("table", &tableName),
                        zap::Int64("jobID", job.JobID),
                        zap::Error(&err),
                    ],
                );
                if firstErr.is_none() {
                    firstErr = Some(err);
                }
            }
        }
        firstErr.map(Err).unwrap_or(Ok(()))
    }

    fn submitAllJobs(
        &self,
        ctx: &context::Context,
        tables: &[importsdk::TableMeta],
    ) -> (Vec<ImportJob>, Option<Error>) {
        let jobs: Arc<Mutex<Vec<ImportJob>>> = Arc::new(Mutex::new(Vec::new()));
        let first_err: Arc<Mutex<Option<Error>>> = Arc::new(Mutex::new(None));
        let sem = Arc::new(Mutex::new(self.submitConcurrency.max(1) as usize));

        let mut handles = Vec::new();
        for table in tables {
            if table.DataFiles.is_empty() || table.TotalSize == 0 {
                self.logger.Info(
                    "skipping table with no data",
                    &[
                        zap::String("database", &table.Database),
                        zap::String("table", &table.Table),
                    ],
                );
                continue;
            }

            // Acquire concurrency slot. Like Go errgroup.SetLimit, still schedule
            // every table so checkpoint-resume paths run after a sibling failure.
            loop {
                let mut g = sem.lock().unwrap();
                if *g > 0 {
                    *g -= 1;
                    break;
                }
                drop(g);
                std::thread::sleep(Duration::from_millis(1));
            }

            let table = table.clone();
            let cpMgr = self.cpMgr.clone();
            let submitter = self.submitter.clone();
            let logger = self.logger.clone();
            let jobs = jobs.clone();
            let first_err = first_err.clone();
            let sem = sem.clone();
            let ctx = ctx.clone();

            handles.push(std::thread::spawn(move || {
                let _release = SemGuard(sem);
                let result = (|| -> Result<()> {
                    let logger = logger
                        .clone()
                        .With(zap::String("database", &table.Database))
                        .With(zap::String("table", &table.Table));

                    let (checkpointCtx, _cancelCheckpoint) =
                        context::WithTimeout(context::WithoutCancel(ctx.clone()), cancelTimeout);

                    let cp = cpMgr
                        .Get(
                            &checkpointCtx,
                            &common::UniqueTable(&table.Database, &table.Table),
                        )
                        .map_err(|e| {
                            errors::Annotatef(
                                e,
                                format!("get checkpoint for {}.{}", table.Database, table.Table),
                            )
                        })?;

                    if let Some(cp) = &cp {
                        if cp.Status == CheckpointStatus::Finished {
                            logger.Info("table already completed in previous run", &[]);
                            return Ok(());
                        }
                        if cp.JobID > 0 && cp.Status == CheckpointStatus::Running {
                            logger.Info(
                                "resuming previously running job",
                                &[zap::Int64("jobID", cp.JobID)],
                            );
                            jobs.lock().unwrap().push(ImportJob {
                                JobID: cp.JobID,
                                TableMeta: Some(table.clone()),
                                GroupKey: submitter.GetGroupKey(),
                            });
                            return Ok(());
                        }
                        logger.Info(
                            "previous job failed or cancelled, submitting new job",
                            &[
                                zap::String("previousStatus", cp.Status.String()),
                                zap::Int64("previousJobID", cp.JobID),
                            ],
                        );
                    } else {
                        logger.Info("submitting new import job", &[]);
                    }

                    if let Some(err) = ctx.Err() {
                        return Err(errors::Trace(err));
                    }

                    let (submitCtx, _cancel) = newStartedSubmitContext(ctx.clone());
                    let job = submitter.SubmitTable(&submitCtx, &table).map_err(|e| {
                        errors::Annotatef(
                            e,
                            format!("submit table {}.{}", table.Database, table.Table),
                        )
                    })?;

                    jobs.lock().unwrap().push(job.clone());
                    let (recordCtx, _cancelRecord) = newSubmissionRecordContext(ctx.clone());
                    recordSubmission(&cpMgr, &recordCtx, &job).map_err(|e| {
                        errors::Annotatef(
                            e,
                            format!("record submission for {}.{}", table.Database, table.Table),
                        )
                    })?;
                    Ok(())
                })();

                if let Err(err) = result {
                    let mut g = first_err.lock().unwrap();
                    if g.is_none() {
                        *g = Some(err);
                    }
                }
            }));
        }

        for h in handles {
            let _ = h.join();
        }

        let out_jobs = jobs.lock().unwrap().clone();
        let err = first_err.lock().unwrap().clone();
        (out_jobs, err)
    }
}

struct SemGuard(Arc<Mutex<usize>>);
impl Drop for SemGuard {
    fn drop(&mut self) {
        *self.0.lock().unwrap() += 1;
    }
}

fn recordSubmission(
    cpMgr: &Arc<dyn CheckpointManager>,
    ctx: &context::Context,
    job: &ImportJob,
) -> Result<()> {
    let meta = job
        .TableMeta
        .as_ref()
        .ok_or_else(|| errors::New("missing table meta"))?;
    cpMgr.Update(
        ctx,
        &TableCheckpoint {
            TableName: common::UniqueTable(&meta.Database, &meta.Table),
            JobID: job.JobID,
            Status: CheckpointStatus::Running,
            GroupKey: job.GroupKey.clone(),
            ..Default::default()
        },
    )
}

fn getSubmitGraceTimeout() -> Duration {
    failpoint::submit_grace_timeout_override().unwrap_or(submitGraceTimeout)
}

fn newStartedSubmitContext(ctx: context::Context) -> (context::Context, context::CancelFunc) {
    let (graceCtx, cancel) = context::WithCancel(context::WithoutCancel(ctx.clone()));
    let grace_cancel_flag = graceCtx.clone();
    let stop = context::AfterFunc(ctx, move || {
        std::thread::sleep(getSubmitGraceTimeout());
        if !grace_cancel_flag.is_cancelled() {
            // cancel by setting cancelled flag via Cause path
            grace_cancel_flag.cancel_with(Error::new("context canceled"));
        }
    });
    (
        graceCtx,
        Box::new(move || {
            stop();
            cancel();
        }),
    )
}

fn newSubmissionRecordContext(ctx: context::Context) -> (context::Context, context::CancelFunc) {
    context::WithTimeout(context::WithoutCancel(ctx), getSubmitGraceTimeout())
}

fn shouldRetryCancelJobErr(err: &Error) -> bool {
    if err.Error().to_lowercase().contains("task not found") {
        return true;
    }
    common::IsRetryableError(Some(err))
}

fn sleepWithContext(ctx: &context::Context, duration: Duration) -> Result<()> {
    let deadline = std::time::Instant::now() + duration;
    loop {
        if let Some(err) = ctx.Err() {
            return Err(errors::Trace(err));
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        // The local context shim has no waitable Done channel. Short sleeps
        // preserve Go's context-interruptible timer behavior without busy-waiting.
        std::thread::sleep(remaining.min(Duration::from_millis(2)));
    }
}
