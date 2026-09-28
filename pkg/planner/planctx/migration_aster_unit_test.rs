// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// planctx 迁移相关 AsterSQL 单元测试：Detach 浅拷贝与 EmptyPlanContextExtended。
//
// 用可区分的 TestBuildContext 验证 Detach 仅替换 ExprCtx，其余字段浅共享；
// 并确认 EmptyPlanContextExtended 的空操作语义与 Go 一致。

use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    BuildContextRef, BuildPBContext, EmptyPlanContextExtended, WarnAppenderRef, contextutil,
    exprctx,
};

/// 可区分身份的表达式构建上下文桩，用于验证 Detach 是否替换 ExprCtx。
#[derive(Debug)]
struct TestBuildContext {
    /// 连接/列 ID 占位，用于区分两个 BuildContext 实例。
    id: u64,
}

impl exprctx::BuildContext for TestBuildContext {
    fn GetEvalCtx(&self) -> &dyn exprctx::EvalContext {
        panic!("not used by the planctx detach test")
    }

    fn GetCharsetInfo(&self) -> (String, String) {
        ("utf8mb4".to_owned(), "utf8mb4_bin".to_owned())
    }

    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        "utf8mb4_bin".to_owned()
    }

    fn GetBlockEncryptionMode(&self) -> String {
        String::new()
    }

    fn GetSysdateIsNow(&self) -> bool {
        false
    }

    fn GetNoopFuncsMode(&self) -> i32 {
        0
    }

    fn Rng(&self) -> &exprctx::mathutil::MysqlRng {
        panic!("not used by the planctx detach test")
    }

    fn IsUseCache(&self) -> bool {
        true
    }

    fn SetSkipPlanCache(&self, _: &str) {}

    fn AllocPlanColumnID(&self) -> i64 {
        self.id as i64
    }

    fn IsInNullRejectCheck(&self) -> bool {
        false
    }

    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }

    fn ConnectionID(&self) -> u64 {
        self.id
    }

    fn IsReadonlyUserVar(&self, _: &str) -> bool {
        false
    }
}

/// 验证 Detach 保留 Go 浅拷贝语义：仅 ExprCtx 被替换，其余字段共享。
#[test]
fn test_build_pb_context_detach_preserves_go_shallow_copy_semantics() {
    let original_expr: BuildContextRef = Arc::new(TestBuildContext { id: 7 });
    let static_expr: BuildContextRef = Arc::new(TestBuildContext { id: 11 });
    let warn_handler: WarnAppenderRef = Arc::new(contextutil::NewStaticWarnHandler(5));

    let obj = BuildPBContext {
        ExprCtx: Arc::clone(&original_expr),
        Client: None,
        TiFlashFastScan: true,
        TiFlashFineGrainedShuffleBatchSize: 1,
        GroupConcatMaxLen: 2,
        InExplainStmt: true,
        WarnHandler: Some(Arc::clone(&warn_handler)),
        ExtraWarnghandler: Some(Arc::clone(&warn_handler)),
    };

    let detached = obj.Detach(Arc::clone(&static_expr));

    // 原对象仍指向 original_expr；Detach 结果指向 static_expr。
    assert!(Arc::ptr_eq(&obj.ExprCtx, &original_expr));
    assert!(Arc::ptr_eq(&detached.ExprCtx, &static_expr));
    assert!(Arc::ptr_eq(&detached.GetExprCtx(), &static_expr));
    assert!(obj.GetClient().is_none());
    assert!(detached.GetClient().is_none());
    assert!(detached.TiFlashFastScan);
    assert_eq!(detached.TiFlashFineGrainedShuffleBatchSize, 1);
    assert_eq!(detached.GroupConcatMaxLen, 2);
    assert!(detached.InExplainStmt);
    assert!(Arc::ptr_eq(
        detached.WarnHandler.as_ref().expect("warn handler"),
        &warn_handler,
    ));
    assert!(Arc::ptr_eq(
        detached
            .ExtraWarnghandler
            .as_ref()
            .expect("extra warn handler"),
        &warn_handler,
    ));
}

/// 验证 EmptyPlanContextExtended 与 Go 空操作行为一致（忽略 Set，Get 恒为 None）。
#[test]
fn test_empty_plan_context_extended_matches_go_noop_behavior() {
    let mut empty = EmptyPlanContextExtended;
    let mut readonly_user_vars = HashMap::new();
    readonly_user_vars.insert("user_var".to_owned(), ());

    assert!(empty.AdviseTxnWarmup().is_ok());
    // Set 被忽略，Get 始终返回 None。
    empty.SetReadonlyUserVarMap(readonly_user_vars);
    assert!(empty.GetReadonlyUserVarMap().is_none());
    empty.Reset();
    assert!(empty.GetReadonlyUserVarMap().is_none());
}
