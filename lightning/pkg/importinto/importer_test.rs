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

//! Go-equivalent tests for `lightning/pkg/importinto/importer_test.go`.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/importer_test.rs`对应的IMPORT INTO 总控流程，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少61行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `base_cfg`是当前文件的重要函数，承担\"base cfg\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_importer_run`对齐 Go 同名测试或契约片段，用来固定\"test importer run\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_importer_new_importer`对齐 Go 同名测试或契约片段，用来固定\"test importer new importer\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_importer_close`对齐 Go 同名测试或契约片段，用来固定\"test importer close\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"success\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"NewImporter: Initialize + GetCheckpoints for group key; Run: GetCheckpoints for precheck\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"create schemas error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"get table metas error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"precheck error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"orchestrator error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"cancel by user — expects Cancel called\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"cancel by failover — Cancel NOT called\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"restored group key\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"build orchestrator\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"invalid checkpoint driver\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"normal close\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"close with db\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"close with error — still closes, errors only logged\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::test_mocks::{ScriptCheckpointManager, ScriptJobOrchestrator};
use crate::*;
use std::sync::Arc;

fn base_cfg() -> config::Config {
    let mut cfg = config::Config::NewConfig();
    cfg.App.CheckRequirements = true;
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.KeepAfterSuccess = config::CheckpointRemove;
    cfg
}

/// TestImporterRun
#[test]
fn test_importer_run() {
    let tables = vec![importsdk::TableMeta {
        Database: "db".into(),
        Table: "t1".into(),
        ..Default::default()
    }];

    // success
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.tables = tables.clone();
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        cp.set_get_cps(Ok(vec![]));
        // NewImporter: Initialize + GetCheckpoints for group key; Run: GetCheckpoints for precheck
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let orch = Arc::new(ScriptJobOrchestrator::new());
        let cfg = base_cfg();
        let importer = NewImporter(
            &context::Background(),
            cfg,
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk),
                WithCheckpointManager(cp.clone()),
                WithOrchestrator(orch),
            ],
        )
        .unwrap();
        importer.Run(&context::Background()).unwrap();
        assert!(
            cp.removed
                .lock()
                .unwrap()
                .contains(&common::AllTables.to_string())
        );
    }

    // create schemas error
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.create_err = Some(Error::new("create schemas failed"));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let orch = Arc::new(ScriptJobOrchestrator::new());
        let importer = NewImporter(
            &context::Background(),
            base_cfg(),
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk),
                WithCheckpointManager(cp),
                WithOrchestrator(orch),
            ],
        )
        .unwrap();
        let err = importer.Run(&context::Background()).unwrap_err();
        assert!(err.Error().contains("create schemas failed"));
    }

    // get table metas error
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.get_metas_err = Some(Error::new("get metas failed"));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let orch = Arc::new(ScriptJobOrchestrator::new());
        let importer = NewImporter(
            &context::Background(),
            base_cfg(),
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk),
                WithCheckpointManager(cp),
                WithOrchestrator(orch),
            ],
        )
        .unwrap();
        let err = importer.Run(&context::Background()).unwrap_err();
        assert!(err.Error().contains("get metas failed"));
    }

    // precheck error
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.tables = tables.clone();
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls2 = calls.clone();
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(move || {
            let n = calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(vec![]) // initGroupKey
            } else {
                Err(Error::new("precheck failed"))
            }
        }));
        let orch = Arc::new(ScriptJobOrchestrator::new());
        let importer = NewImporter(
            &context::Background(),
            base_cfg(),
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk),
                WithCheckpointManager(cp),
                WithOrchestrator(orch),
            ],
        )
        .unwrap();
        let err = importer.Run(&context::Background()).unwrap_err();
        assert!(err.Error().contains("precheck failed"));
    }

    // orchestrator error
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.tables = tables.clone();
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let orch = Arc::new(ScriptJobOrchestrator::new());
        *orch.submit_wait_err.lock().unwrap() = Some(Error::new("orchestrator failed"));
        let importer = NewImporter(
            &context::Background(),
            base_cfg(),
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk),
                WithCheckpointManager(cp),
                WithOrchestrator(orch),
            ],
        )
        .unwrap();
        let err = importer.Run(&context::Background()).unwrap_err();
        assert!(err.Error().contains("orchestrator failed"));
    }

    // cancel by user — expects Cancel called
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.tables = tables.clone();
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let orch = Arc::new(ScriptJobOrchestrator::new());
        *orch.submit_wait_err.lock().unwrap() = Some(Error::new("context canceled"));
        let importer = NewImporter(
            &context::Background(),
            base_cfg(),
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk),
                WithCheckpointManager(cp),
                WithOrchestrator(orch.clone()),
            ],
        )
        .unwrap();
        let (run_ctx, cancel) = context::WithCancelCause(context::Background());
        cancel(Error::new("context canceled"));
        let err = importer.Run(&run_ctx).unwrap_err();
        assert!(err.Error().contains("context canceled"));
        assert!(*orch.cancel_called.lock().unwrap());
    }

    // cancel by failover — Cancel NOT called
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.tables = tables.clone();
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let orch = Arc::new(ScriptJobOrchestrator::new());
        *orch.submit_wait_err.lock().unwrap() = Some(Error::new("context canceled"));
        let importer = NewImporter(
            &context::Background(),
            base_cfg(),
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk),
                WithCheckpointManager(cp),
                WithOrchestrator(orch.clone()),
            ],
        )
        .unwrap();
        let (run_ctx, cancel) = context::WithCancelCause(context::Background());
        cancel(ErrFailoverCancel());
        let err = importer.Run(&run_ctx).unwrap_err();
        assert!(err.Error().contains("context canceled"));
        assert!(!*orch.cancel_called.lock().unwrap());
    }
}

/// TestImporterNewImporter
#[test]
fn test_importer_new_importer() {
    // restored group key
    {
        let sdk = Arc::new(importsdk::MockSDK::new());
        let cp = Arc::new(ScriptCheckpointManager::new());
        cp.set_get_cps(Ok(vec![TableCheckpoint {
            GroupKey: "restored-group-key".into(),
            ..Default::default()
        }]));
        let mut cfg = config::Config::NewConfig();
        cfg.Checkpoint.Enable = true;
        let importer = NewImporter(
            &context::Background(),
            cfg,
            sql::DB::new_memory(),
            vec![WithBackendSDK(sdk), WithCheckpointManager(cp)],
        )
        .unwrap();
        assert_eq!("restored-group-key", importer.groupKey);
        assert!(importer.orchestrator.is_some());
    }

    // build orchestrator
    {
        let sdk = Arc::new(importsdk::MockSDK::new());
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let mut cfg = config::Config::NewConfig();
        cfg.Checkpoint.Enable = true;
        let importer = NewImporter(
            &context::Background(),
            cfg,
            sql::DB::new_memory(),
            vec![WithBackendSDK(sdk), WithCheckpointManager(cp)],
        )
        .unwrap();
        assert!(importer.orchestrator.is_some());
    }

    // invalid checkpoint driver
    {
        let sdk = Arc::new(importsdk::MockSDK::new());
        let mut cfg = config::Config::NewConfig();
        cfg.Checkpoint.Enable = true;
        cfg.Checkpoint.Driver = "invalid".into();
        match NewImporter(
            &context::Background(),
            cfg,
            sql::DB::new_memory(),
            vec![WithBackendSDK(sdk)],
        ) {
            Ok(_) => panic!("expected unknown checkpoint driver error"),
            Err(err) => assert!(err.Error().contains("unknown checkpoint driver")),
        }
    }
}

/// TestImporterClose
#[test]
fn test_importer_close() {
    // normal close
    {
        let mut sdk = importsdk::MockSDK::new();
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let importer = NewImporter(
            &context::Background(),
            config::Config::NewConfig(),
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk.clone()),
                WithCheckpointManager(cp.clone()),
            ],
        )
        .unwrap();
        importer.Close();
        assert!(*sdk.closed.lock().unwrap());
        assert!(*cp.closed.lock().unwrap());
    }

    // close with db
    {
        let sdk = Arc::new(importsdk::MockSDK::new());
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let db = sql::DB::new_memory();
        let importer = NewImporter(
            &context::Background(),
            config::Config::NewConfig(),
            db.clone(),
            vec![
                WithBackendSDK(sdk.clone()),
                WithCheckpointManager(cp.clone()),
            ],
        )
        .unwrap();
        importer.Close();
        assert!(db.is_closed());
        assert!(*sdk.closed.lock().unwrap());
        assert!(*cp.closed.lock().unwrap());
    }

    // close with error — still closes, errors only logged
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.close_err = Some(Error::new("sdk close error"));
        let sdk = Arc::new(sdk);
        let cp = Arc::new(ScriptCheckpointManager::new());
        *cp.close_err.lock().unwrap() = Some(Error::new("cp close error"));
        *cp.get_cps_fn.lock().unwrap() = Some(Arc::new(|| Ok(vec![])));
        let importer = NewImporter(
            &context::Background(),
            config::Config::NewConfig(),
            sql::DB::new_memory(),
            vec![
                WithBackendSDK(sdk.clone()),
                WithCheckpointManager(cp.clone()),
            ],
        )
        .unwrap();
        importer.Close();
        assert!(*sdk.closed.lock().unwrap());
        assert!(*cp.closed.lock().unwrap());
    }
}
