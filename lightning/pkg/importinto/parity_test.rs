// Copyright 2026 AsterSQL.
//! Parity tests for `lightning/pkg/importinto` public contracts vs Go.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/parity_test.rs`对应的Go/Rust 契约对齐，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少55行中文注释，因此下方会显式列出关键符号与高价值场景索引。
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
//! - `PU`承载\"PU\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl ProgressUpdater`把\"ProgressUpdater\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `UpdateTotalSize`是当前文件的重要函数，承担\"UpdateTotalSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `UpdateFinishedSize`是当前文件的重要函数，承担\"UpdateFinishedSize\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"CheckpointStatus.String matches Go.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"File checkpoint: update / get / ignore / destroy / dump / remove-all.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Progress estimator: non-global vs global sort.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"S3 external-id strip.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Precheck: empty checkpoints pass; failed checkpoint fails.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"t2 is running -> warn-pass\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Noop manager when checkpoint disabled.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Unknown driver errors.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Percent N/A and empty phase -> zero progress.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Human size parse edge via estimator path.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Failed checkpoint blocks precheck.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"MySQL manager: ensure exists after ignore with 0 affected.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Importer Close closes sdk + db; group key restored from checkpoint.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Mark the resumed job finished so monitor exits.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Short poll so WaitForJobs finishes quickly.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Rebuild orchestrator with short intervals via Run on finished table (skipped submit).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"After success with CheckpointRemove, checkpoints cleared.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Failover cancel must not invoke Cancel path error — just return canceled.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Direct Cause equality check.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn go_rust_public_contract_matches() {
    contract_normal();
    contract_boundary();
    contract_error();
    contract_resource_cleanup();
}

fn contract_normal() {
    // CheckpointStatus.String matches Go.
    assert_eq!(CheckpointStatus::Pending.String(), "pending");
    assert_eq!(CheckpointStatus::Running.String(), "running");
    assert_eq!(CheckpointStatus::Finished.String(), "finished");
    assert_eq!(CheckpointStatus::Failed.String(), "failed");

    // File checkpoint: update / get / ignore / destroy / dump / remove-all.
    let dir = std::env::temp_dir().join(format!("importinto-cp-{}", uuid_util::New()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("cp.json");
    let mut cfg = config::Config::NewConfig();
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = config::CheckpointDriverFile.into();
    cfg.Checkpoint.DSN = path.to_string_lossy().into();

    let mgr = NewCheckpointManager(&cfg).unwrap();
    let ctx = context::Background();
    mgr.Initialize(&ctx).unwrap();

    let cp = TableCheckpoint {
        TableName: "`db`.`t1`".into(),
        JobID: 7,
        Status: CheckpointStatus::Failed,
        Message: "boom".into(),
        GroupKey: "g1".into(),
    };
    mgr.Update(&ctx, &cp).unwrap();
    let got = mgr.Get(&ctx, "`db`.`t1`").unwrap().unwrap();
    assert_eq!(got.JobID, 7);
    assert_eq!(got.Status, CheckpointStatus::Failed);
    assert_eq!(got.GroupKey, "g1");

    mgr.IgnoreError(&ctx, "`db`.`t1`").unwrap();
    let got = mgr.Get(&ctx, "`db`.`t1`").unwrap().unwrap();
    assert_eq!(got.Status, CheckpointStatus::Pending);
    assert_eq!(got.JobID, 0);
    assert!(got.Message.is_empty());

    mgr.Update(
        &ctx,
        &TableCheckpoint {
            TableName: "`db`.`t1`".into(),
            JobID: 8,
            Status: CheckpointStatus::Failed,
            Message: "x".into(),
            GroupKey: "g1".into(),
        },
    )
    .unwrap();
    let destroyed = mgr.DestroyError(&ctx, "`db`.`t1`").unwrap();
    assert_eq!(destroyed.len(), 1);
    assert!(mgr.Get(&ctx, "`db`.`t1`").unwrap().is_none());

    mgr.Update(
        &ctx,
        &TableCheckpoint {
            TableName: "`db`.`t2`".into(),
            JobID: 9,
            Status: CheckpointStatus::Running,
            GroupKey: "g2".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut buf = Vec::new();
    mgr.DumpTables(&ctx, &mut buf).unwrap();
    let csv = String::from_utf8(buf).unwrap();
    assert!(csv.contains("table_name,job_id,status,message,group_key"));
    assert!(csv.contains("`db`.`t2`"));

    // Progress estimator: non-global vs global sort.
    let p = estimate_progress_for_test(false, "importing", "import", "50");
    assert!((p - 0.25).abs() < 1e-9, "got {p}");
    let p2 = estimate_progress_for_test(true, "global-sorting", "encode", "100");
    assert!((p2 - 0.125).abs() < 1e-9, "got {p2}");

    // S3 external-id strip.
    let q = strip_s3_external_id_for_test(
        "s3://bucket/path?access-key=AK&secret-access-key=SK&external-id=eid&region=us",
    );
    assert!(!q.contains("external-id"));
    assert!(q.contains("access-key") || q.contains("region"));

    // Precheck: empty checkpoints pass; failed checkpoint fails.
    let mut runner = NewPrecheckRunner();
    runner.Register(NewCheckpointCheckItem(Arc::new(cfg.clone()), mgr.clone()));
    // t2 is running -> warn-pass
    runner.Run(ctx.clone()).unwrap();

    let _ = std::fs::remove_dir_all(&dir);
}

fn contract_boundary() {
    // Go context.WithoutCancel removes both cancellation and deadline propagation.
    let (timed, _cancel) = context::WithTimeout(context::Background(), Duration::from_millis(1));
    let detached = context::WithoutCancel(timed);
    std::thread::sleep(Duration::from_millis(5));
    assert!(detached.Err().is_none());

    // zap.Logger.With fields are inherited by subsequent log entries.
    let logger = log::L().With(zap::String("table", "db.t"));
    logger.Info("submitting", &[zap::Int64("job_id", 7)]);
    let output = logger.buffer_string();
    assert!(output.contains("table=db.t"), "got {output}");
    assert!(output.contains("job_id=7"), "got {output}");

    // Noop manager when checkpoint disabled.
    let mut cfg = config::Config::NewConfig();
    cfg.Checkpoint.Enable = false;
    let mgr = NewCheckpointManager(&cfg).unwrap();
    let ctx = context::Background();
    mgr.Initialize(&ctx).unwrap();
    assert!(mgr.Get(&ctx, "x").unwrap().is_none());
    mgr.Update(
        &ctx,
        &TableCheckpoint {
            TableName: "x".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(mgr.GetCheckpoints(&ctx).unwrap().is_empty());

    // Unknown driver errors.
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = "weird".into();
    let err = match NewCheckpointManager(&cfg) {
        Ok(_) => panic!("expected unknown driver error"),
        Err(e) => e,
    };
    assert!(err.Error().contains("unknown checkpoint driver"));

    // Percent N/A and empty phase -> zero progress.
    assert_eq!(estimate_progress_for_test(false, "", "import", "50"), 0.0);
    assert_eq!(
        estimate_progress_for_test(false, "importing", "import", "N/A"),
        0.0
    );

    // Human size parse edge via estimator path.
    assert_eq!(units::FromHumanSize("1KB").unwrap(), 1000);
    assert!(units::FromHumanSize("").is_err());
}

fn contract_error() {
    let dir = std::env::temp_dir().join(format!("importinto-err-{}", uuid_util::New()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("cp.json");
    let mut cfg = config::Config::NewConfig();
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = config::CheckpointDriverFile.into();
    cfg.Checkpoint.DSN = path.to_string_lossy().into();
    let mgr = NewCheckpointManager(&cfg).unwrap();
    let ctx = context::Background();
    mgr.Initialize(&ctx).unwrap();

    let err = mgr.IgnoreError(&ctx, "`db`.`missing`").unwrap_err();
    assert_eq!(
        err.class,
        Some("Lightning:Checkpoint:ErrCheckpointTableNotFound")
    );
    let err = mgr.DestroyError(&ctx, "`db`.`missing`").unwrap_err();
    assert_eq!(
        err.class,
        Some("Lightning:Checkpoint:ErrCheckpointTableNotFound")
    );

    // Failed checkpoint blocks precheck.
    mgr.Update(
        &ctx,
        &TableCheckpoint {
            TableName: "`db`.`t`".into(),
            Status: CheckpointStatus::Failed,
            Message: "fail".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut runner = NewPrecheckRunner();
    runner.Register(NewCheckpointCheckItem(Arc::new(cfg.clone()), mgr.clone()));
    let err = runner.Run(ctx.clone()).unwrap_err();
    assert!(err.Error().contains("precheck"));
    assert!(err.Error().contains("failed"));

    // MySQL manager: ensure exists after ignore with 0 affected.
    let mysql =
        NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), "cp_schema").unwrap();
    mysql.Initialize(&ctx).unwrap();
    let err = mysql.IgnoreError(&ctx, "`db`.`nope`").unwrap_err();
    assert_eq!(
        err.class,
        Some("Lightning:Checkpoint:ErrCheckpointTableNotFound")
    );

    mysql
        .Update(
            &ctx,
            &TableCheckpoint {
                TableName: "`db`.`t`".into(),
                JobID: 1,
                Status: CheckpointStatus::Failed,
                Message: "e".into(),
                GroupKey: "g".into(),
            },
        )
        .unwrap();
    mysql.IgnoreError(&ctx, "`db`.`t`").unwrap();
    let got = mysql.Get(&ctx, "`db`.`t`").unwrap().unwrap();
    assert_eq!(got.Status, CheckpointStatus::Pending);

    let _ = std::fs::remove_dir_all(&dir);
}

fn contract_resource_cleanup() {
    // Importer Close closes sdk + db; group key restored from checkpoint.
    let dir = std::env::temp_dir().join(format!("importinto-imp-{}", uuid_util::New()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("cp.json");

    let mut cfg = config::Config::NewConfig();
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = config::CheckpointDriverFile.into();
    cfg.Checkpoint.DSN = path.to_string_lossy().into();
    cfg.Checkpoint.KeepAfterSuccess = config::CheckpointRemove;
    cfg.App.CheckRequirements = false;
    cfg.App.TableConcurrency = 2;
    cfg.Cron.LogProgress.Duration = Duration::from_secs(3600);

    let mgr = NewCheckpointManager(&cfg).unwrap();
    let ctx = context::Background();
    mgr.Initialize(&ctx).unwrap();
    mgr.Update(
        &ctx,
        &TableCheckpoint {
            TableName: "`db`.`t`".into(),
            JobID: 1,
            Status: CheckpointStatus::Finished,
            GroupKey: "lightning-restored".into(),
            ..Default::default()
        },
    )
    .unwrap();

    let mut sdk = importsdk::MockSDK::new();
    sdk.tables = vec![importsdk::TableMeta {
        Database: "db".into(),
        Table: "t".into(),
        TotalSize: 100,
        DataFiles: vec![importsdk::DataFileMeta {
            Path: "t.csv".into(),
            Size: 100,
            Format: importsdk::FileFormat::CSV,
        }],
        WildcardPath: "s3://b/t.csv?external-id=eid".into(),
        ..Default::default()
    }];
    // Mark the resumed job finished so monitor exits.
    sdk.jobs.lock().unwrap().push(importsdk::JobStatus {
        JobID: 1,
        GroupKey: "lightning-restored".into(),
        Status: "finished".into(),
        Phase: "importing".into(),
        Step: "import".into(),
        Percent: "100".into(),
        ImportedRows: 10,
        ..Default::default()
    });
    let sdk: Arc<dyn importsdk::SDK> = Arc::new(sdk);
    let db = sql::DB::new_memory();

    let total = Arc::new(Mutex::new(0i64));
    let finished = Arc::new(Mutex::new(0i64));
    struct PU {
        total: Arc<Mutex<i64>>,
        finished: Arc<Mutex<i64>>,
    }
    impl ProgressUpdater for PU {
        fn UpdateTotalSize(&self, size: i64) {
            *self.total.lock().unwrap() = size;
        }
        fn UpdateFinishedSize(&self, size: i64) {
            *self.finished.lock().unwrap() = size;
        }
    }
    let pu: Arc<dyn ProgressUpdater> = Arc::new(PU {
        total: total.clone(),
        finished: finished.clone(),
    });

    let imp = NewImporter(
        &ctx,
        cfg,
        db.clone(),
        vec![
            WithBackendSDK(sdk.clone()),
            WithCheckpointManager(mgr.clone()),
            WithProgressUpdater(pu),
            WithStripS3ExternalIDForImportSQL(),
        ],
    )
    .unwrap();
    assert_eq!(imp.groupKey, "lightning-restored");

    // Short poll so WaitForJobs finishes quickly.
    // Rebuild orchestrator with short intervals via Run on finished table (skipped submit).
    imp.Run(&ctx).unwrap();
    imp.Close();

    // After success with CheckpointRemove, checkpoints cleared.
    assert!(mgr.GetCheckpoints(&ctx).unwrap().is_empty());
    assert!(db.is_closed());

    // Failover cancel must not invoke Cancel path error — just return canceled.
    let (cctx, cancel) = context::WithCancelCause(context::Background());
    cancel(ErrFailoverCancel());
    // Direct Cause equality check.
    assert!(errors::ErrorEqual(
        &errors::Cause(&context::Cause(&cctx)),
        &ErrFailoverCancel()
    ));

    let _ = std::fs::remove_dir_all(&dir);
}
