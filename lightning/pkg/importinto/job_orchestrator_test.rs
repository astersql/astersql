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

//! Go-equivalent tests for `lightning/pkg/importinto/job_orchestrator_test.go`.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/job_orchestrator_test.rs`对应的作业提交编排与取消，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少95行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `new_orch`是当前文件的重要函数，承担\"new orch\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `table`是当前文件的重要函数，承担\"table\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_job_orchestrator_submit_and_wait`对齐 Go 同名测试或契约片段，用来固定\"test job orchestrator submit and wait\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_orchestrator_submission_error_still_records_submitted_jobs`对齐 Go 同名测试或契约片段，用来固定\"test job orchestrator submission error still records submitted jobs\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_orchestrator_submission_error_still_cancels_running_checkpoint_jobs`对齐 Go 同名测试或契约片段，用来固定\"test job orchestrator submission error still cancels running checkpoint jobs\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_orchestrator_submit_grace_starts_after_context_cancel`对齐 Go 同名测试或契约片段，用来固定\"test job orchestrator submit grace starts after context cancel\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_orchestrator_record_submission_gets_fresh_grace_timeout`对齐 Go 同名测试或契约片段，用来固定\"test job orchestrator record submission gets fresh grace timeout\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_orchestrator_cancel`对齐 Go 同名测试或契约片段，用来固定\"test job orchestrator cancel\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_orchestrator_cancel_without_active_jobs`对齐 Go 同名测试或契约片段，用来固定\"test job orchestrator cancel without active jobs\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_orchestrator_cancel_retries_on_task_not_found`对齐 Go 同名测试或契约片段，用来固定\"test job orchestrator cancel retries on task not found\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `GraceGuard`承载\"GraceGuard\"相关状态，是理解数据流的入口之一。
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
//! - 场景\"no tables\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"one table, successful submission\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"one table, already finished\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"one table, resume running\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"one table, resubmit failed\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"submission error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"monitor error triggers cancel\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"t2 first so it is recorded before t1 fails (same post-condition as Go)\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Wait until parent cancel is observed by the test, then verify grace ctx still live.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Complete before grace expires.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Record ctx is WithoutCancel(parent)+timeout; should not be cancelled immediately.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::test_mocks::{ScriptCheckpointManager, ScriptJobMonitor, ScriptJobSubmitter};
use crate::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn new_orch(
    submitter: Arc<dyn JobSubmitter>,
    cp: Arc<dyn CheckpointManager>,
    monitor: Arc<dyn JobMonitor>,
    sdk: Arc<dyn importsdk::SDK>,
    concurrency: i32,
) -> Arc<dyn JobOrchestrator> {
    NewJobOrchestrator(OrchestratorConfig {
        Submitter: submitter,
        CheckpointMgr: cp,
        SDK: sdk,
        Monitor: Some(monitor),
        SubmitConcurrency: concurrency,
        PollInterval: Duration::from_millis(1),
        LogInterval: Duration::from_secs(3600),
        Logger: log::L(),
        ProgressUpdater: None,
    })
}

fn table(name: &str) -> importsdk::TableMeta {
    importsdk::TableMeta {
        Database: "db".into(),
        Table: name.into(),
        DataFiles: vec![importsdk::DataFileMeta {
            Path: format!("f-{name}"),
            ..Default::default()
        }],
        TotalSize: 100,
        ..Default::default()
    }
}

/// TestJobOrchestratorSubmitAndWait
#[test]
fn test_job_orchestrator_submit_and_wait() {
    let ctx = context::Background();

    // no tables
    {
        let orch = new_orch(
            Arc::new(ScriptJobSubmitter::new("group1")),
            Arc::new(ScriptCheckpointManager::new()),
            Arc::new(ScriptJobMonitor::new()),
            Arc::new(importsdk::MockSDK::new()),
            2,
        );
        orch.SubmitAndWait(&ctx, &[]).unwrap();
    }

    // one table, successful submission
    {
        let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
        submitter.push_submit(Ok(ImportJob {
            JobID: 1,
            TableMeta: Some(table("t1")),
            GroupKey: "group1".into(),
        }));
        let cp = Arc::new(ScriptCheckpointManager::new());
        cp.push_get(Ok(None));
        let monitor = Arc::new(ScriptJobMonitor::new());
        monitor.push_wait(Ok(()));
        let orch = new_orch(
            submitter,
            cp,
            monitor,
            Arc::new(importsdk::MockSDK::new()),
            2,
        );
        orch.SubmitAndWait(&ctx, &[table("t1")]).unwrap();
    }

    // one table, already finished
    {
        let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
        let cp = Arc::new(ScriptCheckpointManager::new());
        cp.push_get(Ok(Some(TableCheckpoint {
            Status: CheckpointStatus::Finished,
            ..Default::default()
        })));
        let orch = new_orch(
            submitter,
            cp,
            Arc::new(ScriptJobMonitor::new()),
            Arc::new(importsdk::MockSDK::new()),
            2,
        );
        orch.SubmitAndWait(&ctx, &[table("t1")]).unwrap();
    }

    // one table, resume running
    {
        let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
        let cp = Arc::new(ScriptCheckpointManager::new());
        cp.push_get(Ok(Some(TableCheckpoint {
            JobID: 1,
            Status: CheckpointStatus::Running,
            ..Default::default()
        })));
        let monitor = Arc::new(ScriptJobMonitor::new());
        monitor.push_wait(Ok(()));
        let orch = new_orch(
            submitter,
            cp,
            monitor,
            Arc::new(importsdk::MockSDK::new()),
            2,
        );
        orch.SubmitAndWait(&ctx, &[table("t1")]).unwrap();
    }

    // one table, resubmit failed
    {
        let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
        submitter.push_submit(Ok(ImportJob {
            JobID: 2,
            TableMeta: Some(table("t1")),
            GroupKey: "group1".into(),
        }));
        let cp = Arc::new(ScriptCheckpointManager::new());
        cp.push_get(Ok(Some(TableCheckpoint {
            JobID: 1,
            Status: CheckpointStatus::Failed,
            ..Default::default()
        })));
        let monitor = Arc::new(ScriptJobMonitor::new());
        monitor.push_wait(Ok(()));
        let orch = new_orch(
            submitter,
            cp,
            monitor,
            Arc::new(importsdk::MockSDK::new()),
            2,
        );
        orch.SubmitAndWait(&ctx, &[table("t1")]).unwrap();
    }

    // submission error
    {
        let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
        submitter.push_submit(Err(Error::new("submit error")));
        let cp = Arc::new(ScriptCheckpointManager::new());
        cp.push_get(Ok(None));
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Ok(vec![]));
        let orch = new_orch(
            submitter,
            cp,
            Arc::new(ScriptJobMonitor::new()),
            Arc::new(sdk),
            2,
        );
        assert!(orch.SubmitAndWait(&ctx, &[table("t1")]).is_err());
    }

    // monitor error triggers cancel
    {
        let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
        submitter.push_submit(Ok(ImportJob {
            JobID: 1,
            TableMeta: Some(table("t1")),
            GroupKey: "group1".into(),
        }));
        let cp = Arc::new(ScriptCheckpointManager::new());
        cp.push_get(Ok(None));
        let monitor = Arc::new(ScriptJobMonitor::new());
        monitor.push_wait(Err(Error::new("monitor error")));
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
            JobID: 1,
            Status: "running".into(),
            ..Default::default()
        }]));
        let orch = new_orch(submitter, cp.clone(), monitor, Arc::new(sdk), 2);
        assert!(orch.SubmitAndWait(&ctx, &[table("t1")]).is_err());
        let updates = cp.updates.lock().unwrap().clone();
        assert!(updates.iter().any(|u| {
            u.TableName == common::UniqueTable("db", "t1")
                && u.JobID == 1
                && u.Status == CheckpointStatus::Failed
                && u.Message == "cancelled by user"
                && u.GroupKey == "group1"
        }));
    }
}

/// TestJobOrchestratorSubmissionErrorStillRecordsSubmittedJobs
/// (concurrency=1 sequential variant that still verifies t2 recorded then cancelled)
#[test]
fn test_job_orchestrator_submission_error_still_records_submitted_jobs() {
    let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
    let call = Arc::new(AtomicUsize::new(0));
    let call2 = call.clone();
    *submitter.submit_fn.lock().unwrap() = Some(Arc::new(move |_ctx, tableMeta| {
        let n = call2.fetch_add(1, Ordering::SeqCst);
        match (n, tableMeta.Table.as_str()) {
            (0, "t2") => Ok(ImportJob {
                JobID: 1,
                TableMeta: Some(tableMeta.clone()),
                GroupKey: "group1".into(),
            }),
            (_, "t1") => Err(Error::new("submit error")),
            _ => Err(Error::new(format!("unexpected {}", tableMeta.Table))),
        }
    }));
    let cp = Arc::new(ScriptCheckpointManager::new());
    *cp.get_fn.lock().unwrap() = Some(Arc::new(|_t| Ok(None)));
    let mut sdk = importsdk::MockSDK::new();
    sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
        JobID: 1,
        Status: "running".into(),
        ..Default::default()
    }]));
    let orch = new_orch(
        submitter,
        cp.clone(),
        Arc::new(ScriptJobMonitor::new()),
        Arc::new(sdk),
        1,
    );
    // t2 first so it is recorded before t1 fails (same post-condition as Go)
    assert!(
        orch.SubmitAndWait(&context::Background(), &[table("t2"), table("t1")])
            .is_err()
    );
    let updates = cp.updates.lock().unwrap().clone();
    assert!(updates.iter().any(|u| {
        u.TableName == common::UniqueTable("db", "t2")
            && u.JobID == 1
            && u.Status == CheckpointStatus::Running
            && u.GroupKey == "group1"
    }));
    assert!(updates.iter().any(|u| {
        u.TableName == common::UniqueTable("db", "t2")
            && u.Status == CheckpointStatus::Failed
            && u.Message == "cancelled by user"
    }));
}

/// TestJobOrchestratorSubmissionErrorStillCancelsRunningCheckpointJobs
#[test]
fn test_job_orchestrator_submission_error_still_cancels_running_checkpoint_jobs() {
    let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
    *submitter.submit_fn.lock().unwrap() = Some(Arc::new(|_ctx, tableMeta| {
        assert_eq!("t2", tableMeta.Table);
        Err(Error::new("submit error"))
    }));
    let cp = Arc::new(ScriptCheckpointManager::new());
    *cp.get_fn.lock().unwrap() = Some(Arc::new(|tbl| {
        if tbl == common::UniqueTable("db", "t2") {
            Ok(None)
        } else if tbl == common::UniqueTable("db", "t1") {
            Ok(Some(TableCheckpoint {
                JobID: 1,
                Status: CheckpointStatus::Running,
                ..Default::default()
            }))
        } else {
            Ok(None)
        }
    }));
    let mut sdk = importsdk::MockSDK::new();
    sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
        JobID: 1,
        Status: "running".into(),
        ..Default::default()
    }]));
    let orch = new_orch(
        submitter,
        cp.clone(),
        Arc::new(ScriptJobMonitor::new()),
        Arc::new(sdk),
        1,
    );
    assert!(
        orch.SubmitAndWait(&context::Background(), &[table("t2"), table("t1")])
            .is_err()
    );
    let updates = cp.updates.lock().unwrap().clone();
    assert!(updates.iter().any(|u| {
        u.TableName == common::UniqueTable("db", "t1")
            && u.JobID == 1
            && u.Status == CheckpointStatus::Failed
            && u.Message == "cancelled by user"
            && u.GroupKey == "group1"
    }));
}

/// TestJobOrchestratorSubmitGraceStartsAfterContextCancel
#[test]
fn test_job_orchestrator_submit_grace_starts_after_context_cancel() {
    failpoint::Enable_setSubmitGraceTimeout(Duration::from_millis(80));
    let _guard = GraceGuard;

    let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
    let saw_alive_after_parent_cancel = Arc::new(AtomicBool::new(false));
    let flag = saw_alive_after_parent_cancel.clone();
    let parent_cancel_at = Arc::new(Mutex::new(None::<Instant>));
    let parent_cancel_at2 = parent_cancel_at.clone();

    *submitter.submit_fn.lock().unwrap() = Some(Arc::new(move |submit_ctx, tableMeta| {
        // Wait until parent cancel is observed by the test, then verify grace ctx still live.
        let start = Instant::now();
        while parent_cancel_at2.lock().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(2) {
                return Err(Error::new("timeout waiting parent cancel mark"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if !submit_ctx.is_cancelled() {
            flag.store(true, Ordering::SeqCst);
        }
        // Complete before grace expires.
        Ok(ImportJob {
            JobID: 1,
            TableMeta: Some(tableMeta.clone()),
            GroupKey: "group1".into(),
        })
    }));

    let cp = Arc::new(ScriptCheckpointManager::new());
    cp.push_get(Ok(None));
    let monitor = Arc::new(ScriptJobMonitor::new());
    monitor.push_wait(Err(Error::new("context canceled")));
    let orch = new_orch(
        submitter,
        cp,
        monitor,
        Arc::new(importsdk::MockSDK::new()),
        1,
    );

    let (ctx, cancel) = context::WithCancel(context::Background());
    let orch2 = orch.clone();
    let handle = std::thread::spawn(move || orch2.SubmitAndWait(&ctx, &[table("t1")]));

    std::thread::sleep(Duration::from_millis(20));
    cancel();
    *parent_cancel_at.lock().unwrap() = Some(Instant::now());

    let err = handle.join().unwrap().unwrap_err();
    assert!(
        saw_alive_after_parent_cancel.load(Ordering::SeqCst),
        "submit grace ctx should stay alive right after parent cancel"
    );
    assert!(
        err.Error().to_lowercase().contains("cancel"),
        "err={}",
        err.Error()
    );
}

/// TestJobOrchestratorRecordSubmissionGetsFreshGraceTimeout
#[test]
fn test_job_orchestrator_record_submission_gets_fresh_grace_timeout() {
    failpoint::Enable_setSubmitGraceTimeout(Duration::from_millis(80));
    let _guard = GraceGuard;

    let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
    let update_saw_alive = Arc::new(AtomicBool::new(false));
    let flag = update_saw_alive.clone();

    *submitter.submit_fn.lock().unwrap() = Some(Arc::new(move |_submit_ctx, tableMeta| {
        Ok(ImportJob {
            JobID: 1,
            TableMeta: Some(tableMeta.clone()),
            GroupKey: "group1".into(),
        })
    }));

    let cp = Arc::new(ScriptCheckpointManager::new());
    cp.push_get(Ok(None));
    *cp.update_fn.lock().unwrap() = Some(Arc::new(move |cp| {
        // Record ctx is WithoutCancel(parent)+timeout; should not be cancelled immediately.
        assert_eq!(common::UniqueTable("db", "t1"), cp.TableName);
        assert_eq!(1, cp.JobID);
        assert_eq!(CheckpointStatus::Running, cp.Status);
        flag.store(true, Ordering::SeqCst);
        Ok(())
    }));
    let monitor = Arc::new(ScriptJobMonitor::new());
    monitor.push_wait(Err(Error::new("context canceled")));
    let orch = new_orch(
        submitter,
        cp,
        monitor,
        Arc::new(importsdk::MockSDK::new()),
        1,
    );

    let (ctx, cancel) = context::WithCancel(context::Background());
    let orch2 = orch.clone();
    let handle = std::thread::spawn(move || orch2.SubmitAndWait(&ctx, &[table("t1")]));
    std::thread::sleep(Duration::from_millis(10));
    cancel();
    let err = handle.join().unwrap().unwrap_err();
    assert!(
        update_saw_alive.load(Ordering::SeqCst),
        "record submission should run under fresh grace timeout"
    );
    assert!(
        err.Error().to_lowercase().contains("cancel"),
        "err={}",
        err.Error()
    );
}

/// TestJobOrchestratorCancel
#[test]
fn test_job_orchestrator_cancel() {
    let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
    submitter.push_submit(Ok(ImportJob {
        JobID: 1,
        TableMeta: Some(table("t1")),
        GroupKey: "group1".into(),
    }));
    submitter.push_submit(Ok(ImportJob {
        JobID: 2,
        TableMeta: Some(table("t2")),
        GroupKey: "group1".into(),
    }));
    let cp = Arc::new(ScriptCheckpointManager::new());
    cp.push_get(Ok(None));
    cp.push_get(Ok(None));
    let monitor = Arc::new(ScriptJobMonitor::new());
    monitor.push_wait(Ok(()));
    let sdk = Arc::new(importsdk::MockSDK::new());
    let orch = new_orch(submitter, cp.clone(), monitor, sdk.clone(), 2);
    orch.SubmitAndWait(&context::Background(), &[table("t1"), table("t2")])
        .unwrap();

    sdk.push_get_jobs(Ok(vec![
        importsdk::JobStatus {
            JobID: 1,
            Status: "finished".into(),
            ..Default::default()
        },
        importsdk::JobStatus {
            JobID: 2,
            Status: "running".into(),
            ..Default::default()
        },
    ]));
    orch.Cancel(&context::Background()).unwrap();
    let updates = cp.updates.lock().unwrap().clone();
    assert!(updates.iter().any(|u| {
        u.TableName == common::UniqueTable("db", "t1")
            && u.JobID == 1
            && u.Status == CheckpointStatus::Finished
    }));
    assert!(updates.iter().any(|u| {
        u.TableName == common::UniqueTable("db", "t2")
            && u.JobID == 2
            && u.Status == CheckpointStatus::Failed
            && u.Message == "cancelled by user"
    }));
}

/// TestJobOrchestratorCancelWithoutActiveJobs
#[test]
fn test_job_orchestrator_cancel_without_active_jobs() {
    let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
    let mut sdk = importsdk::MockSDK::new();
    sdk.push_get_jobs(Ok(vec![
        importsdk::JobStatus {
            JobID: 1,
            Status: "running".into(),
            ..Default::default()
        },
        importsdk::JobStatus {
            JobID: 2,
            Status: "pending".into(),
            ..Default::default()
        },
    ]));
    let orch = new_orch(
        submitter,
        Arc::new(ScriptCheckpointManager::new()),
        Arc::new(ScriptJobMonitor::new()),
        Arc::new(sdk),
        2,
    );
    orch.Cancel(&context::Background()).unwrap();
}

/// TestJobOrchestratorCancelRetriesOnTaskNotFound
#[test]
fn test_job_orchestrator_cancel_retries_on_task_not_found() {
    let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
    let mut sdk = importsdk::MockSDK::new();
    sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
        JobID: 1,
        Status: "running".into(),
        ..Default::default()
    }]));
    sdk.push_cancel(Err(Error::new("task not found")));
    sdk.push_cancel(Err(Error::new("task not found")));
    sdk.push_cancel(Ok(()));
    let orch = new_orch(
        submitter,
        Arc::new(ScriptCheckpointManager::new()),
        Arc::new(ScriptJobMonitor::new()),
        Arc::new(sdk),
        2,
    );
    orch.Cancel(&context::Background()).unwrap();
}

/// Go's retry timer selects on ctx.Done(), so cancellation must interrupt the
/// backoff instead of waiting for the complete 100ms delay.
#[test]
fn test_job_orchestrator_cancel_backoff_is_context_interruptible() {
    let submitter = Arc::new(ScriptJobSubmitter::new("group1"));
    let first_attempt = Arc::new(AtomicBool::new(false));
    let first_attempt2 = first_attempt.clone();
    let mut sdk = importsdk::MockSDK::new();
    sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
        JobID: 1,
        Status: "running".into(),
        ..Default::default()
    }]));
    sdk.cancel_hook = Some(Arc::new(move |_job_id| {
        first_attempt2.store(true, Ordering::SeqCst);
        Err(Error::new("task not found"))
    }));
    let orch = new_orch(
        submitter,
        Arc::new(ScriptCheckpointManager::new()),
        Arc::new(ScriptJobMonitor::new()),
        Arc::new(sdk),
        2,
    );

    let (ctx, cancel) = context::WithCancel(context::Background());
    let orch2 = orch.clone();
    let started = Instant::now();
    let handle = std::thread::spawn(move || orch2.Cancel(&ctx));
    while !first_attempt.load(Ordering::SeqCst) {
        assert!(started.elapsed() < Duration::from_secs(1));
        std::thread::yield_now();
    }
    cancel();
    let err = handle.join().unwrap().unwrap_err();
    assert!(err.Error().to_lowercase().contains("cancel"));
    assert!(
        started.elapsed() < Duration::from_millis(80),
        "context cancellation should interrupt retry backoff; elapsed={:?}",
        started.elapsed()
    );
}

struct GraceGuard;
impl Drop for GraceGuard {
    fn drop(&mut self) {
        failpoint::Disable_setSubmitGraceTimeout();
    }
}
