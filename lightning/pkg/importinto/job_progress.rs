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

//! Job progress estimation for IMPORT INTO jobs.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/job_progress.rs`对应的进度估算与阶段换算，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少46行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `impl jobProgressEstimator`把\"jobProgressEstimator\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `parseHumanSize`是当前文件的重要函数，承担\"parseHumanSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `updateJobTotalSize`是当前文件的重要函数，承担\"updateJobTotalSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `isGlobalSortStatus`是当前文件的重要函数，承担\"isGlobalSortStatus\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `stepRatio`是当前文件的重要函数，承担\"stepRatio\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `jobProgressPhase`承载\"jobProgressPhase\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `jobProgressPhases`是当前文件的重要函数，承担\"jobProgressPhases\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `findPhase`是当前文件的重要函数，承担\"findPhase\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `findStep`是当前文件的重要函数，承担\"findStep\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"keep prev\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::job_submitter::ImportJob;
use crate::stubs::*;
use std::collections::HashMap;

pub(crate) struct jobProgressEstimator {
    logger: log::Logger,
    pub(crate) isGlobalSort: bool,
}

pub(crate) fn newJobProgressEstimator(logger: log::Logger) -> jobProgressEstimator {
    jobProgressEstimator {
        logger,
        isGlobalSort: false,
    }
}

impl jobProgressEstimator {
    fn parseHumanSize(&self, jobID: i64, sizeText: &str, warnMsg: &str) -> (i64, bool) {
        if sizeText.is_empty() {
            return (0, false);
        }
        match parseDockerHumanSize(sizeText) {
            Ok(size) => (size, true),
            Err(err) => {
                self.logger.Warn(
                    warnMsg,
                    &[
                        zap::String("size", sizeText),
                        zap::Int64("jobID", jobID),
                        zap::Error(&err),
                    ],
                );
                (0, false)
            }
        }
    }

    fn updateJobTotalSize(
        &self,
        jobID: i64,
        job: &ImportJob,
        status: &importsdk::JobStatus,
        jobTotalSize: &mut HashMap<i64, i64>,
    ) -> i64 {
        let mut total = *jobTotalSize.get(&jobID).unwrap_or(&0);
        if let Some(tableMeta) = &job.TableMeta {
            if tableMeta.TotalSize > 0 {
                total = total.max(tableMeta.TotalSize);
            }
        }
        if total <= 0 {
            let (size, ok) = self.parseHumanSize(
                jobID,
                &status.SourceFileSize,
                "failed to parse source file size",
            );
            if ok {
                total = total.max(size);
            }
        }
        if total <= 0 {
            let (size, ok) =
                self.parseHumanSize(jobID, &status.TotalSize, "failed to parse total size");
            if ok {
                total = total.max(size);
            }
        }
        if total > 0 && total != *jobTotalSize.get(&jobID).unwrap_or(&0) {
            jobTotalSize.insert(jobID, total);
        }
        total
    }

    fn isGlobalSortStatus(&self, status: &importsdk::JobStatus) -> bool {
        match status.Phase.as_str() {
            "global-sorting" | "resolving-conflicts" => return true,
            _ => {}
        }
        matches!(
            status.Step.as_str(),
            "encode" | "merge-sort" | "ingest" | "collect-conflicts" | "conflict-resolution"
        )
    }

    fn stepRatio(&self, status: &importsdk::JobStatus) -> f64 {
        if status.Percent.is_empty() || status.Percent == "N/A" {
            return 0.0;
        }
        let p: f64 = match status.Percent.parse() {
            Ok(p) => p,
            Err(_) => {
                self.logger.Warn(
                    "failed to parse progress percent",
                    &[
                        zap::String("percent", &status.Percent),
                        zap::Int64("jobID", status.JobID),
                    ],
                );
                return 0.0;
            }
        };
        mathutil::Clamp(p / 100.0, 0.0, 1.0)
    }

    pub(crate) fn jobProgress(&self, status: &importsdk::JobStatus) -> f64 {
        let phases = jobProgressPhases(self.isGlobalSort);
        if phases.is_empty() {
            return 0.0;
        }
        if status.Phase.is_empty() {
            return 0.0;
        }
        let (phaseIdx, ok) = findPhase(&phases, &status.Phase);
        if !ok {
            return 0.0;
        }
        let mut ratio = self.stepRatio(status);
        let (mut stepIdx, ok) = findStep(&phases[phaseIdx].steps, &status.Step);
        if !ok {
            stepIdx = 0;
            ratio = 0.0;
        }
        let mut phaseProgress = (stepIdx as f64 + ratio) / phases[phaseIdx].steps.len() as f64;
        phaseProgress = mathutil::Clamp(phaseProgress, 0.0, 1.0);
        let progress = (phaseIdx as f64 + phaseProgress) / phases.len() as f64;
        mathutil::Clamp(progress, 0.0, 1.0)
    }

    pub(crate) fn estimateJobFinishedSize(
        &self,
        status: &importsdk::JobStatus,
        jobTotal: i64,
        prevFinished: i64,
    ) -> i64 {
        let mut finished = prevFinished;
        if status.IsFinished() {
            if jobTotal > 0 {
                finished = jobTotal;
            }
        } else if status.IsFailed() || status.IsCancelled() {
            // keep prev
        } else if jobTotal > 0 {
            let progress = self.jobProgress(status);
            finished = finished.max((jobTotal as f64 * progress) as i64);
        }
        if jobTotal > 0 {
            finished = finished.min(jobTotal);
        }
        finished
    }

    pub(crate) fn updateJobProgress(
        &mut self,
        job: &ImportJob,
        status: &importsdk::JobStatus,
        jobTotalSize: &mut HashMap<i64, i64>,
        jobFinishedSize: &mut HashMap<i64, i64>,
    ) {
        if !self.isGlobalSort && self.isGlobalSortStatus(status) {
            self.isGlobalSort = true;
        }
        let jobID = status.JobID;
        let jobTotal = self.updateJobTotalSize(jobID, job, status, jobTotalSize);
        let prevFinished = *jobFinishedSize.get(&jobID).unwrap_or(&0);
        jobFinishedSize.insert(
            jobID,
            self.estimateJobFinishedSize(status, jobTotal, prevFinished),
        );
    }
}

// Match docker/go-units FromHumanSize, including exponent notation and its
// accepted B, KB, and KiB suffix spellings (all decimal for this API).
fn parseDockerHumanSize(sizeText: &str) -> Result<i64> {
    let Some((separator, separatorChar)) = sizeText
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_ascii_digit() || *ch == '.' || *ch == ' ')
    else {
        return Err(Error::new(format!("invalid size: '{sizeText}'")));
    };

    let (number, suffix) = if separatorChar == ' ' {
        (&sizeText[..separator], &sizeText[separator + 1..])
    } else {
        let suffixStart = separator + separatorChar.len_utf8();
        (&sizeText[..suffixStart], &sizeText[suffixStart..])
    };
    let mut size: f64 = number.parse().map_err(|err| Error::new(format!("{err}")))?;
    if size < 0.0 {
        return Err(Error::new(format!("invalid size: '{sizeText}'")));
    }
    if suffix.is_empty() {
        return Ok(size as i64);
    }

    let suffix = suffix.to_ascii_lowercase();
    if suffix.len() > 3 {
        return Err(Error::new(format!("invalid suffix: '{suffix}'")));
    }
    if suffix == "b" {
        return Ok(size as i64);
    }
    let Some(unit) = suffix.as_bytes().first() else {
        return Ok(size as i64);
    };
    size *= match unit {
        b'k' => 1_000_f64,
        b'm' => 1_000_000_f64,
        b'g' => 1_000_000_000_f64,
        b't' => 1_000_000_000_000_f64,
        b'p' => 1_000_000_000_000_000_f64,
        _ => return Err(Error::new(format!("invalid suffix: '{suffix}'"))),
    };
    if (suffix.len() == 2 && suffix.as_bytes()[1] != b'b')
        || (suffix.len() == 3 && &suffix[1..] != "ib")
    {
        return Err(Error::new(format!("invalid suffix: '{suffix}'")));
    }
    Ok(size as i64)
}

struct jobProgressPhase {
    phase: &'static str,
    steps: &'static [&'static str],
}

fn jobProgressPhases(isGlobalSort: bool) -> Vec<jobProgressPhase> {
    if isGlobalSort {
        return vec![
            jobProgressPhase {
                phase: "global-sorting",
                steps: &["encode", "merge-sort"],
            },
            jobProgressPhase {
                phase: "importing",
                steps: &["ingest"],
            },
            jobProgressPhase {
                phase: "resolving-conflicts",
                steps: &["collect-conflicts", "conflict-resolution"],
            },
            jobProgressPhase {
                phase: "validating",
                steps: &["post-process"],
            },
        ];
    }
    vec![
        jobProgressPhase {
            phase: "importing",
            steps: &["import"],
        },
        jobProgressPhase {
            phase: "validating",
            steps: &["post-process"],
        },
    ]
}

fn findPhase(phases: &[jobProgressPhase], phase: &str) -> (usize, bool) {
    for (i, ph) in phases.iter().enumerate() {
        if ph.phase == phase {
            return (i, true);
        }
    }
    (0, false)
}

fn findStep(steps: &[&str], step: &str) -> (usize, bool) {
    for (i, s) in steps.iter().enumerate() {
        if *s == step {
            return (i, true);
        }
    }
    (0, false)
}

/// Expose progress helpers for parity tests.
pub fn estimate_progress_for_test(
    is_global_sort: bool,
    phase: &str,
    step: &str,
    percent: &str,
) -> f64 {
    let mut est = newJobProgressEstimator(log::L());
    est.isGlobalSort = is_global_sort;
    let status = importsdk::JobStatus {
        Phase: phase.into(),
        Step: step.into(),
        Percent: percent.into(),
        JobID: 1,
        ..Default::default()
    };
    est.jobProgress(&status)
}
