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

// rangerctx 迁移补充单元测试。
//
// 验证 `RangerContext::Detach`：替换表达式上下文、保留 Range 回退/计划缓存句柄指针，
// 并对 OptimizerFixControl 做值拷贝（与 Go 深拷贝语义一致）。

use std::collections::HashMap;
use std::sync::Arc;

use crate::{RangerContext, contextutil, errctx, exprctx, types};

/// 仅用于 Detach 测试的占位 BuildContext；本路径不调用其方法。
struct MockBuildContext;

impl exprctx::BuildContext for MockBuildContext {
    fn GetEvalCtx(&self) -> &dyn exprctx::EvalContext {
        panic!("not used by RangerContext::Detach")
    }

    fn GetCharsetInfo(&self) -> (String, String) {
        panic!("not used by RangerContext::Detach")
    }

    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        panic!("not used by RangerContext::Detach")
    }

    fn GetBlockEncryptionMode(&self) -> String {
        panic!("not used by RangerContext::Detach")
    }

    fn GetSysdateIsNow(&self) -> bool {
        panic!("not used by RangerContext::Detach")
    }

    fn GetNoopFuncsMode(&self) -> i32 {
        panic!("not used by RangerContext::Detach")
    }

    fn Rng(&self) -> &exprctx::mathutil::MysqlRng {
        panic!("not used by RangerContext::Detach")
    }

    fn IsUseCache(&self) -> bool {
        panic!("not used by RangerContext::Detach")
    }

    fn SetSkipPlanCache(&self, _reason: &str) {
        panic!("not used by RangerContext::Detach")
    }

    fn AllocPlanColumnID(&self) -> i64 {
        panic!("not used by RangerContext::Detach")
    }

    fn IsInNullRejectCheck(&self) -> bool {
        panic!("not used by RangerContext::Detach")
    }

    fn IsConstantPropagateCheck(&self) -> bool {
        panic!("not used by RangerContext::Detach")
    }

    fn ConnectionID(&self) -> u64 {
        panic!("not used by RangerContext::Detach")
    }

    fn IsReadonlyUserVar(&self, _name: &str) -> bool {
        panic!("not used by RangerContext::Detach")
    }
}

/// 断言 Detach 替换 ExprCtx、共享 handler 指针，且 FixControl map 独立拷贝。
#[test]
fn detach_replaces_expr_context_and_preserves_go_copy_semantics() {
    // 构造计划缓存跟踪与 Range 回退处理器，供 Detach 后指针相等性检查。
    let warn_handler = Arc::new(contextutil::NewStaticWarnHandler(5));
    let warn_appender: Arc<dyn contextutil::WarnAppender + Send + Sync> = warn_handler.clone();
    let plan_cache_tracker = contextutil::NewPlanCacheTracker(warn_appender.clone());
    let range_fallback_handler =
        contextutil::NewRangeFallbackHandler(&plan_cache_tracker, warn_appender.as_ref());
    let original_expr: Arc<dyn exprctx::BuildContext> = Arc::new(MockBuildContext);
    let static_expr: Arc<dyn exprctx::BuildContext> = Arc::new(MockBuildContext);

    let mut original = RangerContext {
        TypeCtx: types::DefaultStmtNoWarningContext.clone(),
        ErrCtx: errctx::StrictNoWarningContext.clone(),
        ExprCtx: original_expr.clone(),
        RangeFallbackHandler: Some(&range_fallback_handler),
        PlanCacheTracker: Some(&plan_cache_tracker),
        OptimizerFixControl: HashMap::from([(1, "a".to_owned())]),
        UseCache: true,
        RegardNULLAsPoint: true,
        OptPrefixIndexSingleScan: true,
    };

    let detached = original.Detach(static_expr.clone());

    assert!(!Arc::ptr_eq(&original.ExprCtx, &detached.ExprCtx));
    assert!(Arc::ptr_eq(&static_expr, &detached.ExprCtx));
    assert!(std::ptr::eq(
        original.RangeFallbackHandler.unwrap(),
        detached.RangeFallbackHandler.unwrap(),
    ));
    assert!(std::ptr::eq(
        original.PlanCacheTracker.unwrap(),
        detached.PlanCacheTracker.unwrap(),
    ));
    assert_eq!(original.TypeCtx.Flags(), detached.TypeCtx.Flags());
    assert_eq!(original.TypeCtx.Location(), detached.TypeCtx.Location());
    assert_eq!(original.ErrCtx.LevelMap(), detached.ErrCtx.LevelMap());
    assert_eq!(
        detached.ErrCtx.LevelMap(),
        [errctx::Level::LevelError; errctx::errGroupCount],
    );
    assert!(detached.UseCache);
    assert!(detached.RegardNULLAsPoint);
    assert!(detached.OptPrefixIndexSingleScan);
    assert_eq!(
        detached.OptimizerFixControl.get(&1).map(String::as_str),
        Some("a")
    );

    original.OptimizerFixControl.insert(1, "changed".to_owned());
    original.OptimizerFixControl.insert(2, "new".to_owned());
    assert_eq!(
        detached.OptimizerFixControl.get(&1).map(String::as_str),
        Some("a")
    );
    assert!(!detached.OptimizerFixControl.contains_key(&2));
}
