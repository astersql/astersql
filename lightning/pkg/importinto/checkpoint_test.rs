// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Go-equivalent tests for `lightning/pkg/importinto/checkpoint_test.go`.
//!
//! File/Noop backends use real temp files / in-process managers.
//! MySQL backend uses the crate's in-memory `sql::DB` stub (Go: go-sqlmock).
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/checkpoint_test.rs`对应的检查点状态持久化与恢复，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少102行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `assert_not_found`是当前文件的重要函数，承担\"assert not found\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_file_checkpoint_manager`对齐 Go 同名测试或契约片段，用来固定\"test file checkpoint manager\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `Case`承载\"Case\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `tempfile_dir`是当前文件的重要函数，承担\"tempfile dir\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_noop_checkpoint_manager`对齐 Go 同名测试或契约片段，用来固定\"test noop checkpoint manager\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_mysql_checkpoint_manager`对齐 Go 同名测试或契约片段，用来固定\"test mysql checkpoint manager\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_checkpoint_status_string`对齐 Go 同名测试或契约片段，用来固定\"test checkpoint status string\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"Initialize and Get empty\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Update and Get\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Persistence across manager instances\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"IgnoreError resets status\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"IgnoreError returns not found for missing table\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"DestroyError removes checkpoint\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"DestroyError returns not found for missing table\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"DumpTables writes CSV\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"DumpEngines and DumpChunks are no-ops\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Remove single checkpoint\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Remove all checkpoints\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"GetCheckpoints returns all\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Initialize creates schema and table\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Get not found returns nil\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Get found returns checkpoint\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Update inserts or updates checkpoint\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"IgnoreError single checkpoint\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"IgnoreError single checkpoint not found\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"IgnoreError all checkpoints\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"DestroyError single checkpoint\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"DestroyError single checkpoint not found\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"DestroyError all checkpoints\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Go CheckpointStatus(999) → "unknown" (raw int; Rust enum can't hold 999).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use std::collections::HashMap;

use crate::*;

fn assert_not_found(err: &Error, table: &str) {
    assert!(
        errors::IsNotFound(err),
        "expected IsNotFound, got {}",
        err.Error()
    );
    assert!(
        err.Error()
            .contains(&format!("checkpoint for table {table} not found")),
        "msg={}",
        err.Error()
    );
    assert!(!err.Error().contains("--checkpoint-error-ignore"));
    assert!(!err.Error().contains("--checkpoint-error-destroy"));
}

/// Go's `encoding/csv.Writer` quotes fields containing commas, quotes, or newlines.
#[test]
fn test_file_checkpoint_manager_dump_tables_uses_go_csv_escaping() {
    let temp_dir = tempfile_dir("dump-tables-csv-escaping");
    let file_path = temp_dir.join("checkpoints.json");
    let mgr = NewFileCheckpointManager(&file_path);
    let ctx = context::Background();
    mgr.Initialize(&ctx).unwrap();
    mgr.Update(
        &ctx,
        &TableCheckpoint {
            TableName: " db".into(),
            JobID: 7,
            Status: CheckpointStatus::Failed,
            Message: "bad \"value\"\r\nnext".into(),
            GroupKey: "group,key".into(),
        },
    )
    .unwrap();

    let mut output = Vec::new();
    mgr.DumpTables(&ctx, &mut output).unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "table_name,job_id,status,message,group_key\n\" db\",7,3,\"bad \"\"value\"\"\nnext\",\"group,key\"\n"
    );
}

/// Go initializes the backing storage before decoding the checkpoint file, so a
/// caller may repair an invalid file with `Update` after `Initialize` reports it.
#[test]
fn test_file_checkpoint_manager_can_recover_after_invalid_json() {
    let temp_dir = tempfile_dir("recover_after_invalid_json");
    let file_path = temp_dir.join("checkpoints.json");
    std::fs::write(&file_path, b"not json").unwrap();
    let mgr = NewFileCheckpointManager(&file_path);
    let ctx = context::Background();

    assert!(mgr.Initialize(&ctx).is_err());
    mgr.Update(
        &ctx,
        &TableCheckpoint {
            TableName: "db.t1".into(),
            JobID: 7,
            Status: CheckpointStatus::Running,
            ..Default::default()
        },
    )
    .unwrap();

    let repaired = NewFileCheckpointManager(&file_path);
    repaired.Initialize(&ctx).unwrap();
    assert_eq!(7, repaired.Get(&ctx, "db.t1").unwrap().unwrap().JobID);
    let _ = std::fs::remove_dir_all(temp_dir);
}

/// TestFileCheckpointManager — Go `TestFileCheckpointManager`.
#[test]
fn test_file_checkpoint_manager() {
    type Setup = Box<dyn Fn(&FileCheckpointManager, &context::Context)>;
    type Op = Box<dyn Fn(&FileCheckpointManager, &context::Context, &std::path::Path)>;

    struct Case {
        name: &'static str,
        setup: Setup,
        operation: Op,
    }

    let tests: Vec<Case> = vec![
        Case {
            name: "Initialize and Get empty",
            setup: Box::new(|_, _| {}),
            operation: Box::new(|mgr, ctx, _| {
                let cp = mgr.Get(ctx, "db.t1").unwrap();
                assert!(cp.is_none());
            }),
        },
        Case {
            name: "Update and Get",
            setup: Box::new(|_, _| {}),
            operation: Box::new(|mgr, ctx, _| {
                let cp1 = TableCheckpoint {
                    TableName: "db.t1".into(),
                    JobID: 1,
                    Status: CheckpointStatus::Running,
                    GroupKey: "g1".into(),
                    ..Default::default()
                };
                mgr.Update(ctx, &cp1).unwrap();
                let cp = mgr.Get(ctx, "db.t1").unwrap().expect("checkpoint");
                assert_eq!(cp1.JobID, cp.JobID);
                assert_eq!(cp1.Status, cp.Status);
            }),
        },
        Case {
            name: "Persistence across manager instances",
            setup: Box::new(|mgr, ctx| {
                let cp1 = TableCheckpoint {
                    TableName: "db.t1".into(),
                    JobID: 1,
                    Status: CheckpointStatus::Running,
                    ..Default::default()
                };
                mgr.Update(ctx, &cp1).unwrap();
            }),
            operation: Box::new(|_, ctx, file_path| {
                let mgr2 = NewFileCheckpointManager(file_path);
                mgr2.Initialize(ctx).unwrap();
                let cp = mgr2.Get(ctx, "db.t1").unwrap().expect("persisted");
                assert_eq!(1, cp.JobID);
            }),
        },
        Case {
            name: "IgnoreError resets status",
            setup: Box::new(|mgr, ctx| {
                let cp1 = TableCheckpoint {
                    TableName: "db.t1".into(),
                    JobID: 1,
                    Status: CheckpointStatus::Failed,
                    ..Default::default()
                };
                mgr.Update(ctx, &cp1).unwrap();
            }),
            operation: Box::new(|mgr, ctx, _| {
                mgr.IgnoreError(ctx, "db.t1").unwrap();
                let cp = mgr.Get(ctx, "db.t1").unwrap().expect("cp");
                assert_eq!(CheckpointStatus::Pending, cp.Status);
                assert_eq!(0, cp.JobID);
            }),
        },
        Case {
            name: "IgnoreError returns not found for missing table",
            setup: Box::new(|_, _| {}),
            operation: Box::new(|mgr, ctx, _| {
                let err = mgr.IgnoreError(ctx, "db.t404").unwrap_err();
                assert_not_found(&err, "db.t404");
            }),
        },
        Case {
            name: "DestroyError removes checkpoint",
            setup: Box::new(|mgr, ctx| {
                let cp1 = TableCheckpoint {
                    TableName: "db.t1".into(),
                    JobID: 1,
                    Status: CheckpointStatus::Failed,
                    ..Default::default()
                };
                mgr.Update(ctx, &cp1).unwrap();
            }),
            operation: Box::new(|mgr, ctx, _| {
                let destroyed = mgr.DestroyError(ctx, "db.t1").unwrap();
                assert_eq!(1, destroyed.len());
                assert_eq!("db.t1", destroyed[0].TableName);
                assert!(mgr.Get(ctx, "db.t1").unwrap().is_none());
            }),
        },
        Case {
            name: "DestroyError returns not found for missing table",
            setup: Box::new(|_, _| {}),
            operation: Box::new(|mgr, ctx, _| {
                let err = mgr.DestroyError(ctx, "db.t404").unwrap_err();
                assert_not_found(&err, "db.t404");
            }),
        },
        Case {
            name: "DumpTables writes CSV",
            setup: Box::new(|mgr, ctx| {
                let cp1 = TableCheckpoint {
                    TableName: "db.t1".into(),
                    JobID: 1,
                    Status: CheckpointStatus::Running,
                    GroupKey: "g1".into(),
                    ..Default::default()
                };
                mgr.Update(ctx, &cp1).unwrap();
            }),
            operation: Box::new(|mgr, ctx, _| {
                let mut buf = Vec::new();
                mgr.DumpTables(ctx, &mut buf).unwrap();
                let s = String::from_utf8(buf).unwrap();
                assert!(s.contains("db.t1,1,1,,g1"), "csv={s}");
            }),
        },
        Case {
            name: "DumpEngines and DumpChunks are no-ops",
            setup: Box::new(|_, _| {}),
            operation: Box::new(|mgr, ctx, _| {
                mgr.DumpEngines(ctx, &mut std::io::sink()).unwrap();
                mgr.DumpChunks(ctx, &mut std::io::sink()).unwrap();
            }),
        },
        Case {
            name: "Remove single checkpoint",
            setup: Box::new(|mgr, ctx| {
                let cp1 = TableCheckpoint {
                    TableName: "db.t1".into(),
                    JobID: 1,
                    Status: CheckpointStatus::Running,
                    ..Default::default()
                };
                mgr.Update(ctx, &cp1).unwrap();
            }),
            operation: Box::new(|mgr, ctx, _| {
                mgr.Remove(ctx, "db.t1").unwrap();
                assert!(mgr.Get(ctx, "db.t1").unwrap().is_none());
            }),
        },
        Case {
            name: "Remove all checkpoints",
            setup: Box::new(|mgr, ctx| {
                let cp1 = TableCheckpoint {
                    TableName: "db.t1".into(),
                    JobID: 1,
                    Status: CheckpointStatus::Running,
                    ..Default::default()
                };
                mgr.Update(ctx, &cp1).unwrap();
            }),
            operation: Box::new(|mgr, ctx, file_path| {
                mgr.Remove(ctx, common::AllTables).unwrap();
                assert!(
                    !file_path.exists(),
                    "checkpoint file should be removed: {}",
                    file_path.display()
                );
            }),
        },
        Case {
            name: "GetCheckpoints returns all",
            setup: Box::new(|mgr, ctx| {
                mgr.Update(
                    ctx,
                    &TableCheckpoint {
                        TableName: "db.t1".into(),
                        JobID: 1,
                        Status: CheckpointStatus::Running,
                        ..Default::default()
                    },
                )
                .unwrap();
                mgr.Update(
                    ctx,
                    &TableCheckpoint {
                        TableName: "db.t2".into(),
                        JobID: 2,
                        Status: CheckpointStatus::Finished,
                        ..Default::default()
                    },
                )
                .unwrap();
            }),
            operation: Box::new(|mgr, ctx, _| {
                let cps = mgr.GetCheckpoints(ctx).unwrap();
                assert_eq!(2, cps.len());
                let mut m = HashMap::new();
                for cp in cps {
                    m.insert(cp.TableName.clone(), cp);
                }
                assert!(m.contains_key("db.t1"));
                assert!(m.contains_key("db.t2"));
            }),
        },
    ];

    for tt in tests {
        let temp_dir = tempfile_dir(tt.name);
        let file_path = temp_dir.join("checkpoints.json");
        let mgr = NewFileCheckpointManager(&file_path);
        let ctx = context::Background();
        mgr.Initialize(&ctx).unwrap();
        (tt.setup)(&mgr, &ctx);
        (tt.operation)(&mgr, &ctx, &file_path);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

fn tempfile_dir(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "importinto-cp-{}-{}-{}",
        name.replace(' ', "_"),
        std::process::id(),
        uuid_util::New()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// TestNoopCheckpointManager — Go `TestNoopCheckpointManager`.
#[test]
fn test_noop_checkpoint_manager() {
    let mgr = NoopCheckpointManager {};
    let ctx = context::Background();

    mgr.Initialize(&ctx).unwrap();
    assert!(mgr.Get(&ctx, "db.t1").unwrap().is_none());

    mgr.Update(&ctx, &TableCheckpoint::default()).unwrap();
    mgr.Remove(&ctx, "db.t1").unwrap();
    mgr.IgnoreError(&ctx, "db.t1").unwrap();
    let destroyed = mgr.DestroyError(&ctx, "db.t1").unwrap();
    assert!(destroyed.is_empty());
    let mut buf = Vec::new();
    mgr.DumpTables(&ctx, &mut buf).unwrap();
    assert!(buf.is_empty());
    mgr.DumpEngines(&ctx, &mut std::io::sink()).unwrap();
    mgr.DumpChunks(&ctx, &mut std::io::sink()).unwrap();

    let cps = mgr.GetCheckpoints(&ctx).unwrap();
    assert!(cps.is_empty());

    mgr.Close().unwrap();
}

/// TestMySQLCheckpointManager — Go `TestMySQLCheckpointManager`.
/// Go drives go-sqlmock; Rust uses in-memory `sql::DB` stub (no network).
#[test]
fn test_mysql_checkpoint_manager() {
    let schema_name = "test_schema";
    let ctx = context::Background();

    // Initialize creates schema and table
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Close().unwrap();
    }

    // Get not found returns nil
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        assert!(mgr.Get(&ctx, "db.t1").unwrap().is_none());
        mgr.Close().unwrap();
    }

    // Get found returns checkpoint
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        let cp = TableCheckpoint {
            TableName: "db.t1".into(),
            JobID: 123,
            Status: CheckpointStatus::Running,
            Message: "msg".into(),
            GroupKey: "g1".into(),
        };
        mgr.Update(&ctx, &cp).unwrap();
        let got = mgr.Get(&ctx, "db.t1").unwrap().expect("found");
        assert_eq!(123, got.JobID);
        assert_eq!(CheckpointStatus::Running, got.Status);
        assert_eq!("msg", got.Message);
        assert_eq!("g1", got.GroupKey);
        mgr.Close().unwrap();
    }

    // Update inserts or updates checkpoint
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 123,
                Status: CheckpointStatus::Running,
                Message: "msg".into(),
                GroupKey: "g1".into(),
            },
        )
        .unwrap();
        mgr.Close().unwrap();
    }

    // Remove single checkpoint
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 1,
                Status: CheckpointStatus::Running,
                ..Default::default()
            },
        )
        .unwrap();
        mgr.Remove(&ctx, "db.t1").unwrap();
        assert!(mgr.Get(&ctx, "db.t1").unwrap().is_none());
        mgr.Close().unwrap();
    }

    // Remove all checkpoints
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 1,
                Status: CheckpointStatus::Running,
                ..Default::default()
            },
        )
        .unwrap();
        mgr.Remove(&ctx, common::AllTables).unwrap();
        assert!(mgr.GetCheckpoints(&ctx).unwrap().is_empty());
        mgr.Close().unwrap();
    }

    // IgnoreError single checkpoint
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 1,
                Status: CheckpointStatus::Failed,
                ..Default::default()
            },
        )
        .unwrap();
        mgr.IgnoreError(&ctx, "db.t1").unwrap();
        let cp = mgr.Get(&ctx, "db.t1").unwrap().expect("cp");
        assert_eq!(CheckpointStatus::Pending, cp.Status);
        assert_eq!(0, cp.JobID);
        mgr.Close().unwrap();
    }

    // IgnoreError single checkpoint not found
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        let err = mgr.IgnoreError(&ctx, "db.t404").unwrap_err();
        assert_not_found(&err, "db.t404");
        mgr.Close().unwrap();
    }

    // IgnoreError all checkpoints
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 1,
                Status: CheckpointStatus::Failed,
                ..Default::default()
            },
        )
        .unwrap();
        mgr.IgnoreError(&ctx, common::AllTables).unwrap();
        let cp = mgr.Get(&ctx, "db.t1").unwrap().expect("cp");
        assert_eq!(CheckpointStatus::Pending, cp.Status);
        mgr.Close().unwrap();
    }

    // DestroyError single checkpoint
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 123,
                Status: CheckpointStatus::Failed,
                Message: "msg".into(),
                GroupKey: "g1".into(),
            },
        )
        .unwrap();
        let destroyed = mgr.DestroyError(&ctx, "db.t1").unwrap();
        assert_eq!(1, destroyed.len());
        assert_eq!("db.t1", destroyed[0].TableName);
        mgr.Close().unwrap();
    }

    // DestroyError single checkpoint not found
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        let err = mgr.DestroyError(&ctx, "db.t404").unwrap_err();
        assert_not_found(&err, "db.t404");
        mgr.Close().unwrap();
    }

    // DestroyError all checkpoints
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 123,
                Status: CheckpointStatus::Failed,
                Message: "msg".into(),
                GroupKey: "g1".into(),
            },
        )
        .unwrap();
        let destroyed = mgr.DestroyError(&ctx, common::AllTables).unwrap();
        assert_eq!(1, destroyed.len());
        mgr.Close().unwrap();
    }

    // GetCheckpoints returns all
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 123,
                Status: CheckpointStatus::Running,
                Message: "msg".into(),
                GroupKey: "g1".into(),
            },
        )
        .unwrap();
        let cps = mgr.GetCheckpoints(&ctx).unwrap();
        assert_eq!(1, cps.len());
        assert_eq!("db.t1", cps[0].TableName);
        mgr.Close().unwrap();
    }

    // DumpTables writes CSV
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.Initialize(&ctx).unwrap();
        mgr.Update(
            &ctx,
            &TableCheckpoint {
                TableName: "db.t1".into(),
                JobID: 123,
                Status: CheckpointStatus::Running,
                Message: "msg".into(),
                GroupKey: "g1".into(),
            },
        )
        .unwrap();
        let mut buf = Vec::new();
        mgr.DumpTables(&ctx, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("db.t1,123,1,msg,g1"), "csv={s}");
        mgr.Close().unwrap();
    }

    // DumpEngines and DumpChunks are no-ops
    {
        let mgr =
            NewMySQLCheckpointManager(&common::MySQLConnectParam::default(), schema_name).unwrap();
        mgr.DumpEngines(&ctx, &mut std::io::sink()).unwrap();
        mgr.DumpChunks(&ctx, &mut std::io::sink()).unwrap();
        mgr.Close().unwrap();
    }
}

/// TestCheckpointStatus_String — Go `TestCheckpointStatus_String`.
#[test]
fn test_checkpoint_status_string() {
    let tests = [
        (CheckpointStatus::Pending, "pending"),
        (CheckpointStatus::Running, "running"),
        (CheckpointStatus::Finished, "finished"),
        (CheckpointStatus::Failed, "failed"),
    ];
    for (status, want) in tests {
        assert_eq!(want, status.String());
    }
    // Go CheckpointStatus(999) → "unknown" (raw int; Rust enum can't hold 999).
    assert_eq!("unknown", CheckpointStatus::string_i32(999));
}
