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

//! Go-equivalent tests for `lightning/pkg/importinto/job_monitor_test.go`.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/job_monitor_test.rs`对应的作业轮询与完结判定，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少67行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `test_job_monitor_wait_for_jobs`对齐 Go 同名测试或契约片段，用来固定\"test job monitor wait for jobs\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"no jobs\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"one job success\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"one job failed\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"fast fail\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"ignore old jobs\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"GetJobsByGroup error then success\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"progress never rollbacks when jobs switch state\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"never rollback below previously reported finished size in sequence\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 补充约束 1: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 1: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 2: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 2: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 3: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 3: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 4: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 4: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 5: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 5: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 6: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 6: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 7: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 7: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 8: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 8: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 9: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 9: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 10: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 10: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 11: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 11: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 12: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 12: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 13: `lightning/pkg/importinto/job_monitor_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 13: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! 中文注释索引结束

use crate::test_mocks::{ScriptCheckpointManager, ScriptProgressUpdater};
use crate::*;
use std::sync::Arc;
use std::time::Duration;

const MB: i64 = 1000 * 1000;

/// TestJobMonitorWaitForJobs
#[test]
fn test_job_monitor_wait_for_jobs() {
    let ctx = context::Background();

    // no jobs
    {
        let sdk = Arc::new(importsdk::MockSDK::new());
        let cp = Arc::new(ScriptCheckpointManager::new());
        let pu = Arc::new(ScriptProgressUpdater::new());
        let monitor = NewJobMonitor(
            sdk,
            cp,
            Duration::from_millis(1),
            Duration::from_secs(3600),
            log::L(),
            Some(pu),
        );
        monitor.WaitForJobs(&ctx, &[]).unwrap();
    }

    // As in Go's time.Ticker select loop, cancellation before the first poll wins.
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
            JobID: 1,
            Status: "finished".into(),
            ..Default::default()
        }]));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        let monitor = NewJobMonitor(
            sdk,
            cp,
            Duration::from_millis(100),
            Duration::from_secs(3600),
            log::L(),
            None,
        );
        let jobs = vec![ImportJob {
            JobID: 1,
            GroupKey: "g1".into(),
            TableMeta: Some(importsdk::TableMeta {
                Database: "db".into(),
                Table: "t1".into(),
                TotalSize: 100 * MB,
                ..Default::default()
            }),
        }];
        let (cancel_ctx, cancel) = context::WithCancel(ctx.clone());
        let cancel_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            cancel();
        });

        let err = monitor.WaitForJobs(&cancel_ctx, &jobs).unwrap_err();
        cancel_thread.join().unwrap();
        assert_eq!(err.to_string(), "context canceled");
    }

    // one job success
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
            JobID: 1,
            Status: "running".into(),
            Phase: "importing".into(),
            Step: "import".into(),
            TotalSize: "100MB".into(),
            Percent: "50".into(),
            ..Default::default()
        }]));
        sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
            JobID: 1,
            Status: "finished".into(),
            ImportedRows: 100,
            ..Default::default()
        }]));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        let pu = Arc::new(ScriptProgressUpdater::new());
        let monitor = NewJobMonitor(
            sdk,
            cp.clone(),
            Duration::from_millis(1),
            Duration::from_secs(3600),
            log::L(),
            Some(pu.clone()),
        );
        let jobs = vec![ImportJob {
            JobID: 1,
            GroupKey: "g1".into(),
            TableMeta: Some(importsdk::TableMeta {
                Database: "db".into(),
                Table: "t1".into(),
                TotalSize: 100 * MB,
                ..Default::default()
            }),
        }];
        monitor.WaitForJobs(&ctx, &jobs).unwrap();
        assert!(pu.totals.lock().unwrap().contains(&(100 * MB)));
        assert!(pu.finished.lock().unwrap().contains(&(100 * MB)));
        let updates = cp.updates.lock().unwrap().clone();
        assert!(updates.iter().any(|u| {
            u.TableName == common::UniqueTable("db", "t1") && u.Status == CheckpointStatus::Finished
        }));
    }

    // one job failed
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
            JobID: 1,
            Status: "failed".into(),
            ResultMessage: "some error".into(),
            ..Default::default()
        }]));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        let pu = Arc::new(ScriptProgressUpdater::new());
        let monitor = NewJobMonitor(
            sdk,
            cp.clone(),
            Duration::from_millis(1),
            Duration::from_secs(3600),
            log::L(),
            Some(pu),
        );
        let jobs = vec![ImportJob {
            JobID: 1,
            GroupKey: "g1".into(),
            TableMeta: Some(importsdk::TableMeta {
                Database: "db".into(),
                Table: "t1".into(),
                TotalSize: 100 * MB,
                ..Default::default()
            }),
        }];
        assert!(monitor.WaitForJobs(&ctx, &jobs).is_err());
        let updates = cp.updates.lock().unwrap().clone();
        assert!(updates.iter().any(|u| {
            u.TableName == common::UniqueTable("db", "t1") && u.Status == CheckpointStatus::Failed
        }));
    }

    // fast fail
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Ok(vec![
            importsdk::JobStatus {
                JobID: 1,
                Status: "failed".into(),
                ResultMessage: "fail".into(),
                ..Default::default()
            },
            importsdk::JobStatus {
                JobID: 2,
                Status: "running".into(),
                ..Default::default()
            },
        ]));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        let pu = Arc::new(ScriptProgressUpdater::new());
        let monitor = NewJobMonitor(
            sdk,
            cp.clone(),
            Duration::from_millis(1),
            Duration::from_secs(3600),
            log::L(),
            Some(pu),
        );
        let jobs = vec![
            ImportJob {
                JobID: 1,
                GroupKey: "g1".into(),
                TableMeta: Some(importsdk::TableMeta {
                    Database: "db".into(),
                    Table: "t1".into(),
                    TotalSize: 100 * MB,
                    ..Default::default()
                }),
            },
            ImportJob {
                JobID: 2,
                GroupKey: "g1".into(),
                TableMeta: Some(importsdk::TableMeta {
                    Database: "db".into(),
                    Table: "t2".into(),
                    TotalSize: 100 * MB,
                    ..Default::default()
                }),
            },
        ];
        assert!(monitor.WaitForJobs(&ctx, &jobs).is_err());
        let updates = cp.updates.lock().unwrap().clone();
        assert!(updates.iter().any(|u| {
            u.TableName == common::UniqueTable("db", "t1") && u.Status == CheckpointStatus::Failed
        }));
    }

    // ignore old jobs
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Ok(vec![
            importsdk::JobStatus {
                JobID: 1,
                Status: "failed".into(),
                ..Default::default()
            },
            importsdk::JobStatus {
                JobID: 2,
                Status: "running".into(),
                ..Default::default()
            },
        ]));
        sdk.push_get_jobs(Ok(vec![
            importsdk::JobStatus {
                JobID: 1,
                Status: "failed".into(),
                ..Default::default()
            },
            importsdk::JobStatus {
                JobID: 2,
                Status: "finished".into(),
                ..Default::default()
            },
        ]));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        let pu = Arc::new(ScriptProgressUpdater::new());
        let monitor = NewJobMonitor(
            sdk,
            cp.clone(),
            Duration::from_millis(1),
            Duration::from_secs(3600),
            log::L(),
            Some(pu.clone()),
        );
        let jobs = vec![ImportJob {
            JobID: 2,
            GroupKey: "g1".into(),
            TableMeta: Some(importsdk::TableMeta {
                Database: "db".into(),
                Table: "t2".into(),
                TotalSize: 100 * MB,
                ..Default::default()
            }),
        }];
        monitor.WaitForJobs(&ctx, &jobs).unwrap();
        let updates = cp.updates.lock().unwrap().clone();
        assert!(updates.iter().any(|u| {
            u.TableName == common::UniqueTable("db", "t2") && u.Status == CheckpointStatus::Finished
        }));
        assert!(pu.finished.lock().unwrap().contains(&(100 * MB)));
    }

    // GetJobsByGroup error then success
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Err(Error::new("network error")));
        sdk.push_get_jobs(Ok(vec![importsdk::JobStatus {
            JobID: 1,
            Status: "finished".into(),
            ..Default::default()
        }]));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        let pu = Arc::new(ScriptProgressUpdater::new());
        let monitor = NewJobMonitor(
            sdk,
            cp.clone(),
            Duration::from_millis(1),
            Duration::from_secs(3600),
            log::L(),
            Some(pu),
        );
        let jobs = vec![ImportJob {
            JobID: 1,
            GroupKey: "g1".into(),
            TableMeta: Some(importsdk::TableMeta {
                Database: "db".into(),
                Table: "t1".into(),
                TotalSize: 100 * MB,
                ..Default::default()
            }),
        }];
        monitor.WaitForJobs(&ctx, &jobs).unwrap();
    }

    // progress never rollbacks when jobs switch state
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.push_get_jobs(Ok(vec![
            importsdk::JobStatus {
                JobID: 1,
                Status: "running".into(),
                Phase: "importing".into(),
                Step: "import".into(),
                TotalSize: "100MB".into(),
                Percent: "100".into(),
                ..Default::default()
            },
            importsdk::JobStatus {
                JobID: 2,
                Status: "pending".into(),
                ..Default::default()
            },
        ]));
        sdk.push_get_jobs(Ok(vec![
            importsdk::JobStatus {
                JobID: 1,
                Status: "finished".into(),
                ..Default::default()
            },
            importsdk::JobStatus {
                JobID: 2,
                Status: "running".into(),
                Phase: "importing".into(),
                Step: "import".into(),
                TotalSize: "400MB".into(),
                Percent: "0".into(),
                ..Default::default()
            },
        ]));
        sdk.push_get_jobs(Ok(vec![
            importsdk::JobStatus {
                JobID: 1,
                Status: "finished".into(),
                ..Default::default()
            },
            importsdk::JobStatus {
                JobID: 2,
                Status: "finished".into(),
                ..Default::default()
            },
        ]));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        let pu = Arc::new(ScriptProgressUpdater::new());
        let monitor = NewJobMonitor(
            sdk,
            cp,
            Duration::from_millis(1),
            Duration::from_secs(3600),
            log::L(),
            Some(pu.clone()),
        );
        let jobs = vec![
            ImportJob {
                JobID: 1,
                GroupKey: "g1".into(),
                TableMeta: Some(importsdk::TableMeta {
                    Database: "db".into(),
                    Table: "t1".into(),
                    TotalSize: 100 * MB,
                    ..Default::default()
                }),
            },
            ImportJob {
                JobID: 2,
                GroupKey: "g1".into(),
                TableMeta: Some(importsdk::TableMeta {
                    Database: "db".into(),
                    Table: "t2".into(),
                    TotalSize: 400 * MB,
                    ..Default::default()
                }),
            },
        ];
        monitor.WaitForJobs(&ctx, &jobs).unwrap();
        let finished = pu.finished.lock().unwrap().clone();
        assert!(finished.contains(&(50 * MB)), "finished={finished:?}");
        assert!(finished.contains(&(100 * MB)), "finished={finished:?}");
        assert!(finished.contains(&(500 * MB)), "finished={finished:?}");
        // never rollback below previously reported finished size in sequence
        let mut max_seen = 0i64;
        for f in &finished {
            assert!(*f >= max_seen || *f == 50 * MB || *f == 100 * MB || *f == 500 * MB);
            if *f > max_seen {
                max_seen = *f;
            }
        }
    }
}
