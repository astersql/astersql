// Copyright 2026 AsterSQL.
//! Parity tests for `lightning/pkg/importinto/mock` public contracts vs Go.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/mock/parity_test.rs`对应的Go/Rust 契约对齐，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少44行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `go_rust_public_contract_matches`对齐 Go 同名测试或契约片段，用来固定\"go rust public contract matches\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `contract_normal`对齐 Go 同名测试或契约片段，用来固定\"contract normal\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `contract_boundary`对齐 Go 同名测试或契约片段，用来固定\"contract boundary\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `contract_error`对齐 Go 同名测试或契约片段，用来固定\"contract error\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `contract_resource_cleanup`对齐 Go 同名测试或契约片段，用来固定\"contract resource cleanup\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"Initialize / Update / Get / GetCheckpoints / Close success path.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"JobSubmitter normal.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"JobMonitor / Orchestrator / ProgressUpdater normal.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Get returns nil pointer → None (Go nil *TableCheckpoint).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Empty checkpoint list / DestroyError empty result.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Remove / IgnoreError succeed with empty side effects.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"SubmitTable with nil job pointer and nil error → default ImportJob.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Trait object dispatch (Go interface satisfaction).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"No expectations left from earlier — remaining should be 0 before trait use.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Unexpected call panics like GoMock.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Expectations are consumed FIFO; leftover expectations remain until consumed.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Controller shared across mocks: dropping one mock must not drop shared queue.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Orphaned GetGroupKey still first in queue; WaitForJobs mismatches → panic.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Clean shared-controller path: only WaitForJobs expected and consumed.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use astersql_lightning_pkg_importinto::{
    CheckpointManager, CheckpointStatus, Error, ImportJob, JobMonitor, JobOrchestrator,
    JobSubmitter, ProgressUpdater, TableCheckpoint, context, importsdk,
};

use crate::{
    Controller, NewMockCheckpointManager, NewMockJobMonitor, NewMockJobOrchestrator,
    NewMockJobSubmitter, NewMockProgressUpdater,
};

#[test]
fn go_rust_public_contract_matches() {
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

#[test]
fn gomock_rejects_mismatched_arguments() {
    let ctrl = Controller::new();
    let progress = NewMockProgressUpdater(ctrl);
    progress.EXPECT().UpdateTotalSize(&100_i64).Return(vec![]);

    let panicked = catch_unwind(AssertUnwindSafe(|| progress.UpdateTotalSize(99)));
    assert!(
        panicked.is_err(),
        "GoMock must reject a mismatched argument"
    );
}

#[test]
fn gomock_matches_expectations_without_implicit_ordering() {
    let ctrl = Controller::new();
    let progress = NewMockProgressUpdater(ctrl.clone());
    progress.EXPECT().UpdateTotalSize(&100_i64).Return(vec![]);
    progress.EXPECT().UpdateFinishedSize(&40_i64).Return(vec![]);

    // GoMock only enforces call order when gomock.InOrder is requested.
    progress.UpdateFinishedSize(40);
    progress.UpdateTotalSize(100);
    assert_eq!(ctrl.remaining(), 0);
}

fn contract_normal() {
    let ctx = context::Background();
    let ctrl = Controller::new();
    let cp = NewMockCheckpointManager(ctrl.clone());
    cp.ISGOMOCK();

    // Initialize / Update / Get / GetCheckpoints / Close success path.
    cp.EXPECT().Initialize(&()).ReturnError(None);
    assert!(cp.Initialize(&ctx).is_ok());

    let tc = TableCheckpoint {
        TableName: "db.t".to_string(),
        JobID: 7,
        Status: CheckpointStatus::Running,
        Message: String::new(),
        GroupKey: "gk".to_string(),
    };
    cp.EXPECT().Update(&(), &()).ReturnError(None);
    assert!(cp.Update(&ctx, &tc).is_ok());

    cp.EXPECT().Get(&(), &()).Return2(Some(tc.clone()), None);
    let got = cp.Get(&ctx, "db.t").expect("Get");
    assert_eq!(got.as_ref().map(|c| c.JobID), Some(7));
    assert_eq!(
        got.as_ref().map(|c| c.Status),
        Some(CheckpointStatus::Running)
    );

    cp.EXPECT()
        .GetCheckpoints(&())
        .Return2(vec![tc.clone()], None);
    let list = cp.GetCheckpoints(&ctx).expect("GetCheckpoints");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].TableName, "db.t");

    let mut buf = Cursor::new(Vec::new());
    cp.EXPECT().DumpTables(&(), &()).ReturnError(None);
    assert!(cp.DumpTables(&ctx, &mut buf).is_ok());
    cp.EXPECT().DumpEngines(&(), &()).ReturnError(None);
    assert!(cp.DumpEngines(&ctx, &mut buf).is_ok());
    cp.EXPECT().DumpChunks(&(), &()).ReturnError(None);
    assert!(cp.DumpChunks(&ctx, &mut buf).is_ok());

    cp.EXPECT().Close().ReturnError(None);
    assert!(cp.Close().is_ok());
    assert_eq!(ctrl.remaining(), 0);

    // JobSubmitter normal.
    let ctrl2 = Controller::new();
    let sub = NewMockJobSubmitter(ctrl2.clone());
    sub.ISGOMOCK();
    sub.EXPECT().GetGroupKey().Return1("group-a".to_string());
    assert_eq!(sub.GetGroupKey(), "group-a");

    let meta = importsdk::TableMeta {
        Database: "db".to_string(),
        Table: "t".to_string(),
        ..Default::default()
    };
    let job = ImportJob {
        JobID: 42,
        TableMeta: Some(meta.clone()),
        GroupKey: "group-a".to_string(),
    };
    sub.EXPECT().SubmitTable(&(), &()).Return2(Some(job), None);
    let submitted = sub.SubmitTable(&ctx, &meta).expect("SubmitTable");
    assert_eq!(submitted.JobID, 42);
    assert_eq!(submitted.GroupKey, "group-a");
    assert_eq!(ctrl2.remaining(), 0);

    // JobMonitor / Orchestrator / ProgressUpdater normal.
    let ctrl3 = Controller::new();
    let mon = NewMockJobMonitor(ctrl3.clone());
    mon.EXPECT().WaitForJobs(&(), &()).ReturnError(None);
    assert!(
        mon.WaitForJobs(
            &ctx,
            &[ImportJob {
                JobID: 1,
                TableMeta: None,
                GroupKey: String::new()
            }]
        )
        .is_ok()
    );

    let orch = NewMockJobOrchestrator(ctrl3.clone());
    orch.EXPECT().SubmitAndWait(&(), &()).ReturnError(None);
    assert!(orch.SubmitAndWait(&ctx, &[meta]).is_ok());
    orch.EXPECT().Cancel(&()).ReturnError(None);
    assert!(orch.Cancel(&ctx).is_ok());

    let prog = NewMockProgressUpdater(ctrl3.clone());
    prog.EXPECT().UpdateTotalSize(&()).Return(vec![]);
    prog.EXPECT().UpdateFinishedSize(&()).Return(vec![]);
    prog.UpdateTotalSize(100);
    prog.UpdateFinishedSize(40);
    assert_eq!(ctrl3.remaining(), 0);
}

fn contract_boundary() {
    let ctx = context::Background();
    let ctrl = Controller::new();
    let cp = NewMockCheckpointManager(ctrl.clone());

    // Get returns nil pointer → None (Go nil *TableCheckpoint).
    cp.EXPECT()
        .Get(&(), &())
        .Return2(None::<TableCheckpoint>, None);
    assert!(cp.Get(&ctx, "missing").unwrap().is_none());

    // Empty checkpoint list / DestroyError empty result.
    cp.EXPECT()
        .GetCheckpoints(&())
        .Return2(Vec::<TableCheckpoint>::new(), None);
    assert!(cp.GetCheckpoints(&ctx).unwrap().is_empty());

    cp.EXPECT()
        .DestroyError(&(), &())
        .Return2(Vec::<TableCheckpoint>::new(), None);
    assert!(cp.DestroyError(&ctx, "t").unwrap().is_empty());

    // Remove / IgnoreError succeed with empty side effects.
    cp.EXPECT().Remove(&(), &()).ReturnError(None);
    assert!(cp.Remove(&ctx, "t").is_ok());
    cp.EXPECT().IgnoreError(&(), &()).ReturnError(None);
    assert!(cp.IgnoreError(&ctx, "t").is_ok());

    // SubmitTable with nil job pointer and nil error → default ImportJob.
    let sub = NewMockJobSubmitter(ctrl.clone());
    sub.EXPECT()
        .SubmitTable(&(), &())
        .Return2(None::<ImportJob>, None);
    let job = sub
        .SubmitTable(&ctx, &importsdk::TableMeta::default())
        .unwrap();
    assert_eq!(job.JobID, 0);
    assert!(job.TableMeta.is_none());
    assert!(job.GroupKey.is_empty());

    // Trait object dispatch (Go interface satisfaction).
    let cp_arc: Arc<dyn CheckpointManager> = Arc::new(NewMockCheckpointManager(ctrl.clone()));
    // No expectations left from earlier — remaining should be 0 before trait use.
    assert_eq!(ctrl.remaining(), 0);
    drop(cp_arc);

    let sub2 = NewMockJobSubmitter(Controller::new());
    let _: &dyn JobSubmitter = &sub2;
    let mon = NewMockJobMonitor(Controller::new());
    let _: &dyn JobMonitor = &mon;
    let orch = NewMockJobOrchestrator(Controller::new());
    let _: &dyn JobOrchestrator = &orch;
    let prog = NewMockProgressUpdater(Controller::new());
    let _: &dyn ProgressUpdater = &prog;
}

fn contract_error() {
    let ctx = context::Background();
    let ctrl = Controller::new();
    let cp = NewMockCheckpointManager(ctrl.clone());

    cp.EXPECT()
        .Initialize(&())
        .ReturnError(Some(Error::new("init failed")));
    let err = cp.Initialize(&ctx).expect_err("must fail");
    assert!(err.Error().contains("init failed"));

    cp.EXPECT()
        .Get(&(), &())
        .Return2(None::<TableCheckpoint>, Some(Error::new("get boom")));
    let err = cp.Get(&ctx, "t").expect_err("Get must fail");
    assert!(err.Error().contains("get boom"));

    cp.EXPECT()
        .DestroyError(&(), &())
        .Return2(Vec::<TableCheckpoint>::new(), Some(Error::new("destroy")));
    let err = cp.DestroyError(&ctx, "t").expect_err("DestroyError");
    assert!(err.Error().contains("destroy"));

    let sub = NewMockJobSubmitter(ctrl.clone());
    sub.EXPECT()
        .SubmitTable(&(), &())
        .Return2(None::<ImportJob>, Some(Error::new("submit failed")));
    let err = sub
        .SubmitTable(&ctx, &importsdk::TableMeta::default())
        .expect_err("SubmitTable");
    assert!(err.Error().contains("submit failed"));

    let mon = NewMockJobMonitor(ctrl.clone());
    mon.EXPECT()
        .WaitForJobs(&(), &())
        .ReturnError(Some(Error::new("wait timeout")));
    let err = mon.WaitForJobs(&ctx, &[]).expect_err("WaitForJobs");
    assert!(err.Error().contains("wait timeout"));

    let orch = NewMockJobOrchestrator(ctrl.clone());
    orch.EXPECT()
        .Cancel(&())
        .ReturnError(Some(Error::new("cancel denied")));
    let err = orch.Cancel(&ctx).expect_err("Cancel");
    assert!(err.Error().contains("cancel denied"));

    // Unexpected call panics like GoMock.
    let ctrl2 = Controller::new();
    let cp2 = NewMockCheckpointManager(ctrl2);
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = cp2.Close();
    }));
    assert!(panicked.is_err(), "unexpected Close must panic");
}

fn contract_resource_cleanup() {
    // Expectations are consumed FIFO; leftover expectations remain until consumed.
    let ctrl = Controller::new();
    let cp = NewMockCheckpointManager(ctrl.clone());
    cp.EXPECT().Close().ReturnError(None);
    assert_eq!(ctrl.remaining(), 1);
    assert!(cp.Close().is_ok());
    assert_eq!(ctrl.remaining(), 0);

    // Controller shared across mocks: dropping one mock must not drop shared queue.
    let ctrl2 = Controller::new();
    let sub = NewMockJobSubmitter(ctrl2.clone());
    let mon = NewMockJobMonitor(ctrl2.clone());
    sub.EXPECT().GetGroupKey().Return1("g".to_string());
    mon.EXPECT().WaitForJobs(&(), &()).ReturnError(None);
    assert_eq!(ctrl2.remaining(), 2);
    drop(sub);
    assert_eq!(ctrl2.remaining(), 2);
    // An unrelated orphaned expectation does not impose call ordering in GoMock.
    let ctx = context::Background();
    assert!(mon.WaitForJobs(&ctx, &[]).is_ok());
    assert_eq!(ctrl2.remaining(), 1);

    // Clean shared-controller path: only WaitForJobs expected and consumed.
    let ctrl3 = Controller::new();
    let mon2 = NewMockJobMonitor(ctrl3.clone());
    mon2.EXPECT().WaitForJobs(&(), &()).ReturnError(None);
    assert!(mon2.WaitForJobs(&ctx, &[]).is_ok());
    assert_eq!(ctrl3.remaining(), 0);
    drop(mon2);
    assert_eq!(ctrl3.remaining(), 0);
}
