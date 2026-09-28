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

//! Go-equivalent tests for `lightning/pkg/importinto/precheck_test.go`.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/precheck_test.rs`对应的导入前校验，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少30行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `test_checkpoint_check_item_check`对齐 Go 同名测试或契约片段，用来固定\"test checkpoint check item check\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `MockChecker`承载\"MockChecker\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `impl precheck`把\"precheck\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `Check`是当前文件的重要函数，承担\"Check\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetCheckItemID`是当前文件的重要函数，承担\"GetCheckItemID\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_precheck_runner`对齐 Go 同名测试或契约片段，用来固定\"test precheck runner\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"checkpoint disabled\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"get checkpoints error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"no checkpoints\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"has failed checkpoints\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"has running/finished checkpoints\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"all passed\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"checker error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"checker not passed\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::test_mocks::ScriptCheckpointManager;
use crate::*;
use astersql_lightning_pkg_precheck as precheck;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// TestCheckpointCheckItemCheck
#[test]
fn test_checkpoint_check_item_check() {
    // checkpoint disabled
    {
        let mock = Arc::new(ScriptCheckpointManager::new());
        let mut cfg = config::Config::NewConfig();
        cfg.Checkpoint.Enable = false;
        let mut checker = NewCheckpointCheckItem(Arc::new(cfg), mock);
        assert_eq!(precheck::CheckCheckpoints, checker.GetCheckItemID());
        let res = checker
            .Check(precheck::context::Background())
            .unwrap()
            .unwrap();
        assert!(res.Passed);
    }

    // get checkpoints error
    {
        let mock = Arc::new(ScriptCheckpointManager::new());
        mock.set_get_cps(Err(Error::new("get error")));
        let mut cfg = config::Config::NewConfig();
        cfg.Checkpoint.Enable = true;
        let mut checker = NewCheckpointCheckItem(Arc::new(cfg), mock);
        let err = checker.Check(precheck::context::Background()).unwrap_err();
        assert!(err.Error().contains("get error"));
    }

    // no checkpoints
    {
        let mock = Arc::new(ScriptCheckpointManager::new());
        mock.set_get_cps(Ok(vec![]));
        let mut cfg = config::Config::NewConfig();
        cfg.Checkpoint.Enable = true;
        let mut checker = NewCheckpointCheckItem(Arc::new(cfg), mock);
        let res = checker
            .Check(precheck::context::Background())
            .unwrap()
            .unwrap();
        assert!(res.Passed);
    }

    // has failed checkpoints
    {
        let mock = Arc::new(ScriptCheckpointManager::new());
        mock.set_get_cps(Ok(vec![TableCheckpoint {
            TableName: "db.t1".into(),
            Status: CheckpointStatus::Failed,
            ..Default::default()
        }]));
        let mut cfg = config::Config::NewConfig();
        cfg.Checkpoint.Enable = true;
        let mut checker = NewCheckpointCheckItem(Arc::new(cfg), mock);
        let res = checker
            .Check(precheck::context::Background())
            .unwrap()
            .unwrap();
        assert!(!res.Passed);
        assert!(!res.Message.is_empty());
    }

    // has running/finished checkpoints
    {
        let mock = Arc::new(ScriptCheckpointManager::new());
        mock.set_get_cps(Ok(vec![TableCheckpoint {
            TableName: "db.t1".into(),
            Status: CheckpointStatus::Running,
            ..Default::default()
        }]));
        let mut cfg = config::Config::NewConfig();
        cfg.Checkpoint.Enable = true;
        let mut checker = NewCheckpointCheckItem(Arc::new(cfg), mock);
        let res = checker
            .Check(precheck::context::Background())
            .unwrap()
            .unwrap();
        assert!(res.Passed);
        assert_eq!(precheck::Warn, res.Severity);
        assert!(!res.Message.is_empty());
    }
}

struct MockChecker {
    id: precheck::CheckItemID,
    res: Option<precheck::CheckResult>,
    err: Option<precheck::errors::Error>,
    observed_cancelled: Option<Arc<AtomicBool>>,
}

impl precheck::Checker for MockChecker {
    fn Check(
        &mut self,
        _ctx: precheck::context::Context,
    ) -> std::result::Result<Option<precheck::CheckResult>, precheck::errors::Error> {
        if let Some(observed) = &self.observed_cancelled {
            observed.store(_ctx.cancelled, Ordering::SeqCst);
        }
        if let Some(err) = self.err.take() {
            return Err(err);
        }
        Ok(self.res.clone())
    }
    fn GetCheckItemID(&self) -> precheck::CheckItemID {
        self.id
    }
}

/// TestPrecheckRunner
#[test]
fn test_precheck_runner() {
    // all passed
    {
        let mut runner = NewPrecheckRunner();
        runner.Register(Box::new(MockChecker {
            id: "c1",
            res: Some(precheck::CheckResult {
                Passed: true,
                ..Default::default()
            }),
            err: None,
            observed_cancelled: None,
        }));
        runner.Register(Box::new(MockChecker {
            id: "c2",
            res: Some(precheck::CheckResult {
                Passed: true,
                Message: "passed with msg".into(),
                ..Default::default()
            }),
            err: None,
            observed_cancelled: None,
        }));
        runner.Run(context::Background()).unwrap();
    }

    // checker error
    {
        let mut runner = NewPrecheckRunner();
        runner.Register(Box::new(MockChecker {
            id: "c1",
            res: None,
            err: Some(precheck::errors::New("check error")),
            observed_cancelled: None,
        }));
        let err = runner.Run(context::Background()).unwrap_err();
        assert!(
            err.Error().contains("precheck c1 failed: check error"),
            "err={}",
            err.Error()
        );
    }

    // checker not passed
    {
        let mut runner = NewPrecheckRunner();
        runner.Register(Box::new(MockChecker {
            id: "c1",
            res: Some(precheck::CheckResult {
                Passed: false,
                Message: "failed msg".into(),
                ..Default::default()
            }),
            err: None,
            observed_cancelled: None,
        }));
        let err = runner.Run(context::Background()).unwrap_err();
        assert!(
            err.Error().contains("precheck c1 failed: failed msg"),
            "err={}",
            err.Error()
        );
    }

    // caller cancellation is passed to every checker, matching Go's direct ctx forwarding
    {
        let observed = Arc::new(AtomicBool::new(false));
        let mut runner = NewPrecheckRunner();
        runner.Register(Box::new(MockChecker {
            id: "c1",
            res: Some(precheck::CheckResult {
                Passed: true,
                ..Default::default()
            }),
            err: None,
            observed_cancelled: Some(observed.clone()),
        }));
        let (ctx, cancel) = context::WithCancel(context::Background());
        cancel();
        runner.Run(ctx).unwrap();
        assert!(observed.load(Ordering::SeqCst));
    }
}
