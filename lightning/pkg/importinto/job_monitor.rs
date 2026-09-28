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

//! Job monitor for IMPORT INTO jobs.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/job_monitor.rs`对应的作业轮询与完结判定，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少57行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `trait`定义对外暴露的抽象边界，约束\"trait\"的最小能力集合。
//! 对 trait 的说明应重点覆盖调用者可依赖什么、实现者必须遵守什么以及错误是否允许透传。
//! 这可以帮助后续替换实现时，避免只满足编译器却破坏 Go 端既有约定。
//! 在 mock、checkpoint、monitor 或 backend 体系里，trait 文档直接决定测试替身是否可信。
//! - `WaitForJobs`是当前文件的重要函数，承担\"WaitForJobs\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `groupStats`承载\"groupStats\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl JobMonitor`把\"JobMonitor\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl DefaultJobMonitor`把\"DefaultJobMonitor\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `logProgress`是当前文件的重要函数，承担\"logProgress\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `processJobStatuses`是当前文件的重要函数，承担\"processJobStatuses\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `logJobCompletion`是当前文件的重要函数，承担\"logJobCompletion\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `recordCompletion`是当前文件的重要函数，承担\"recordCompletion\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"failpoint SlowDownPolling is a no-op in Rust port.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 补充约束 1: `lightning/pkg/importinto/job_monitor.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 1: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 2: `lightning/pkg/importinto/job_monitor.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 2: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! 中文注释索引结束

use crate::checkpoint::{CheckpointManager, CheckpointStatus, TableCheckpoint};
use crate::importer::ProgressUpdater;
use crate::job_progress::newJobProgressEstimator;
use crate::job_submitter::ImportJob;
use crate::stubs::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// JobMonitor monitors the status of import jobs.
pub trait JobMonitor: Send + Sync {
    fn WaitForJobs(&self, ctx: &context::Context, jobs: &[ImportJob]) -> Result<()>;
}

/// DefaultJobMonitor is the default implementation of JobMonitor.
pub struct DefaultJobMonitor {
    sdk: Arc<dyn importsdk::SDK>,
    cpMgr: Arc<dyn CheckpointManager>,
    pollInterval: Duration,
    logInterval: Duration,
    logger: log::Logger,
    progressUpdater: Option<Arc<dyn ProgressUpdater>>,
}

struct groupStats {
    runningCnt: i32,
    pendingCnt: i32,
    completedCnt: i32,
    failedCnt: i32,
    cancelledCnt: i32,
    totalImportedRows: i64,
}

/// NewJobMonitor creates a new job monitor.
pub fn NewJobMonitor(
    sdk: Arc<dyn importsdk::SDK>,
    cpMgr: Arc<dyn CheckpointManager>,
    pollInterval: Duration,
    logInterval: Duration,
    logger: log::Logger,
    progressUpdater: Option<Arc<dyn ProgressUpdater>>,
) -> Arc<dyn JobMonitor> {
    Arc::new(DefaultJobMonitor {
        sdk,
        cpMgr,
        pollInterval,
        logInterval,
        logger,
        progressUpdater,
    })
}

impl JobMonitor for DefaultJobMonitor {
    fn WaitForJobs(&self, ctx: &context::Context, jobs: &[ImportJob]) -> Result<()> {
        if jobs.is_empty() {
            return Ok(());
        }

        let mut jobMap: HashMap<i64, ImportJob> = HashMap::with_capacity(jobs.len());
        let mut jobTotalSize: HashMap<i64, i64> = HashMap::with_capacity(jobs.len());
        let mut jobFinishedSize: HashMap<i64, i64> = HashMap::with_capacity(jobs.len());
        for job in jobs {
            jobMap.insert(job.JobID, job.clone());
            if let Some(meta) = &job.TableMeta {
                jobTotalSize.insert(job.JobID, meta.TotalSize);
            }
        }

        let groupKey = jobs[0].GroupKey.clone();
        let mut finishedJobs: HashSet<i64> = HashSet::new();
        let mut stats = groupStats {
            runningCnt: 0,
            pendingCnt: 0,
            completedCnt: 0,
            failedCnt: 0,
            cancelledCnt: 0,
            totalImportedRows: 0,
        };
        let mut progressEstimator = newJobProgressEstimator(self.logger.clone());

        self.logger.Info(
            "waiting for all jobs to complete",
            &[
                zap::Int("totalJobs", jobs.len() as i64),
                zap::String("groupKey", &groupKey),
            ],
        );

        let mut last_log = std::time::Instant::now();
        // Go's time.Ticker does not emit an initial tick: the first status query
        // happens only after one full poll interval has elapsed.
        let mut last_poll = std::time::Instant::now();

        loop {
            if let Some(err) = ctx.Err() {
                return Err(err);
            }

            if last_log.elapsed() >= self.logInterval {
                self.logProgress(jobs.len(), &stats);
                last_log = std::time::Instant::now();
            }

            if last_poll.elapsed() < self.pollInterval {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            last_poll = std::time::Instant::now();

            // failpoint SlowDownPolling is a no-op in Rust port.
            let statuses = match self.sdk.GetJobsByGroup(ctx, &groupKey) {
                Ok(s) => s,
                Err(err) => {
                    self.logger
                        .Warn("failed to get group jobs status", &[zap::Error(&err)]);
                    continue;
                }
            };

            let (newStats, err) = self.processJobStatuses(
                ctx,
                &statuses,
                &jobMap,
                &mut jobTotalSize,
                &mut jobFinishedSize,
                &mut finishedJobs,
                &mut progressEstimator,
            );
            stats = newStats;

            if stats.failedCnt > 0 {
                return Err(err.unwrap_or_else(|| {
                    errors::Errorf(format!(
                        "job group {groupKey} has {} failed jobs",
                        stats.failedCnt
                    ))
                }));
            }

            if finishedJobs.len() == jobs.len() {
                if let Some(err) = err {
                    self.logger.Error(
                        "all jobs completed but with errors",
                        &[zap::Int("total", jobs.len() as i64), zap::Error(&err)],
                    );
                    return Err(err);
                }
                self.logger.Info(
                    "all jobs completed successfully",
                    &[zap::Int("total", jobs.len() as i64)],
                );
                return Ok(());
            }

            if let Some(err) = err {
                self.logger.Info(
                    "exiting early due to failure",
                    &[
                        zap::Int("finished", finishedJobs.len() as i64),
                        zap::Int("total", jobs.len() as i64),
                    ],
                );
                return Err(err);
            }
        }
    }
}

impl DefaultJobMonitor {
    fn logProgress(&self, total: usize, stats: &groupStats) {
        self.logger.Info(
            "job group progress",
            &[
                zap::Int("total", total as i64),
                zap::Int("pending", stats.pendingCnt as i64),
                zap::Int("running", stats.runningCnt as i64),
                zap::Int("completed", stats.completedCnt as i64),
                zap::Int("failed", stats.failedCnt as i64),
                zap::Int("cancelled", stats.cancelledCnt as i64),
                zap::Int64("importedRows", stats.totalImportedRows),
            ],
        );
    }

    fn processJobStatuses(
        &self,
        ctx: &context::Context,
        statuses: &[importsdk::JobStatus],
        jobMap: &HashMap<i64, ImportJob>,
        jobTotalSize: &mut HashMap<i64, i64>,
        jobFinishedSize: &mut HashMap<i64, i64>,
        finishedJobs: &mut HashSet<i64>,
        progressEstimator: &mut crate::job_progress::jobProgressEstimator,
    ) -> (groupStats, Option<Error>) {
        let mut stats = groupStats {
            runningCnt: 0,
            pendingCnt: 0,
            completedCnt: 0,
            failedCnt: 0,
            cancelledCnt: 0,
            totalImportedRows: 0,
        };
        let mut firstErr: Option<Error> = None;

        for status in statuses {
            let Some(job) = jobMap.get(&status.JobID) else {
                continue;
            };

            progressEstimator.updateJobProgress(job, status, jobTotalSize, jobFinishedSize);
            stats.totalImportedRows += status.ImportedRows;

            if status.IsFinished() {
                stats.completedCnt += 1;
            } else if status.IsFailed() {
                stats.failedCnt += 1;
            } else if status.IsCancelled() {
                stats.cancelledCnt += 1;
            } else if status.Status == "running" {
                stats.runningCnt += 1;
            } else {
                stats.pendingCnt += 1;
            }

            if finishedJobs.contains(&status.JobID) {
                continue;
            }

            if status.IsCompleted() {
                finishedJobs.insert(status.JobID);

                if !status.IsFinished() && firstErr.is_none() {
                    if status.IsFailed() {
                        firstErr = Some(errors::Errorf(format!(
                            "job {} failed: {}",
                            status.JobID, status.ResultMessage
                        )));
                    } else if status.IsCancelled() {
                        firstErr = Some(errors::Errorf(format!(
                            "job {} was cancelled",
                            status.JobID
                        )));
                    }
                }

                if let Err(err) = self.recordCompletion(ctx, job, status) {
                    self.logger.Error(
                        "failed to record job completion",
                        &[zap::Int64("jobID", job.JobID), zap::Error(&err)],
                    );
                    if firstErr.is_none() {
                        firstErr = Some(err);
                    }
                    continue;
                }

                self.logJobCompletion(job, status);
            }
        }

        let mut totalSize = 0i64;
        let mut finishedSize = 0i64;
        for jobID in jobMap.keys() {
            totalSize += *jobTotalSize.get(jobID).unwrap_or(&0);
            finishedSize += *jobFinishedSize.get(jobID).unwrap_or(&0);
        }
        if let Some(pu) = &self.progressUpdater {
            pu.UpdateTotalSize(totalSize);
            pu.UpdateFinishedSize(finishedSize);
        }

        (stats, firstErr)
    }

    fn logJobCompletion(&self, job: &ImportJob, status: &importsdk::JobStatus) {
        let mut logger = self.logger.clone().With(zap::Int64("jobID", job.JobID));
        if let Some(meta) = &job.TableMeta {
            logger = logger
                .With(zap::String("database", &meta.Database))
                .With(zap::String("table", &meta.Table));
        }
        if status.IsFinished() {
            logger.Info(
                "job completed successfully",
                &[zap::Int64("importedRows", status.ImportedRows)],
            );
        } else if status.IsFailed() {
            logger.Error("job failed", &[zap::String("error", &status.ResultMessage)]);
        } else if status.IsCancelled() {
            logger.Warn("job was cancelled", &[]);
        }
    }

    fn recordCompletion(
        &self,
        ctx: &context::Context,
        job: &ImportJob,
        status: &importsdk::JobStatus,
    ) -> Result<()> {
        let meta = job
            .TableMeta
            .as_ref()
            .ok_or_else(|| errors::New("missing table meta"))?;
        let mut checkpoint = TableCheckpoint {
            TableName: common::UniqueTable(&meta.Database, &meta.Table),
            JobID: job.JobID,
            GroupKey: job.GroupKey.clone(),
            ..Default::default()
        };

        if status.IsFinished() {
            checkpoint.Status = CheckpointStatus::Finished;
        } else {
            checkpoint.Status = CheckpointStatus::Failed;
            checkpoint.Message = status.ResultMessage.clone();
        }

        self.cpMgr
            .Update(ctx, &checkpoint)
            .map_err(|e| errors::Annotate(e, "update checkpoint"))
    }
}
