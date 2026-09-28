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

// `RangerContext::Detach` 测试：字段拷贝、指针共享与 map 独立。
//
// 验证 Detach 后标志字段一致，ExprCtx/handler 指针身份保持，且 OptimizerFixControl
// 深拷贝后互不影响。

use std::collections::HashMap;
use std::sync::Arc;

use crate::{RangerContext, contextutil, errctx, exprstatic, types};

/// 构造带 handler 的上下文，Detach 后核对共享与 map 隔离。
#[test]
fn test_context_detach() {
    let warn_handler = Arc::new(contextutil::NewStaticWarnHandler(5));
    let warn_appender: Arc<dyn contextutil::WarnAppender + Send + Sync> = warn_handler.clone();
    let plan_cache_tracker = contextutil::NewPlanCacheTracker(warn_appender.clone());
    let range_fallback_handler =
        contextutil::NewRangeFallbackHandler(&plan_cache_tracker, warn_appender.as_ref());

    let mut obj = RangerContext {
        TypeCtx: types::DefaultStmtNoWarningContext.clone(),
        ErrCtx: errctx::StrictNoWarningContext.clone(),
        ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
        RangeFallbackHandler: Some(&range_fallback_handler),
        PlanCacheTracker: Some(&plan_cache_tracker),
        OptimizerFixControl: HashMap::from([(1, "a".to_owned())]),
        UseCache: true,
        RegardNULLAsPoint: true,
        OptPrefixIndexSingleScan: true,
    };

    // Match AssertRecursivelyNotEqual against RangerContext's zero-value fields.
    // 先确认非零字段已按预期填充（对齐 Go 递归非零断言意图）。
    assert_eq!(
        obj.OptimizerFixControl,
        HashMap::from([(1, "a".to_owned())])
    );
    assert!(obj.UseCache);
    assert!(obj.RegardNULLAsPoint);
    assert!(obj.OptPrefixIndexSingleScan);

    let static_obj = obj.Detach(Arc::clone(&obj.ExprCtx));

    // Detach 后标量/标志与类型上下文应一致。
    assert_eq!(obj.OptimizerFixControl, static_obj.OptimizerFixControl);
    assert_eq!(obj.UseCache, static_obj.UseCache);
    assert_eq!(obj.RegardNULLAsPoint, static_obj.RegardNULLAsPoint);
    assert_eq!(
        obj.OptPrefixIndexSingleScan,
        static_obj.OptPrefixIndexSingleScan
    );
    assert_eq!(obj.TypeCtx.Flags(), static_obj.TypeCtx.Flags());
    assert_eq!(obj.TypeCtx.Location(), static_obj.TypeCtx.Location());
    assert_eq!(obj.ErrCtx.LevelMap(), static_obj.ErrCtx.LevelMap());
    // ExprCtx 与两个 handler 保持同一指针身份。
    assert!(Arc::ptr_eq(&obj.ExprCtx, &static_obj.ExprCtx));
    assert!(std::ptr::eq(
        obj.RangeFallbackHandler.unwrap(),
        static_obj.RangeFallbackHandler.unwrap()
    ));
    assert!(std::ptr::eq(
        obj.PlanCacheTracker.unwrap(),
        static_obj.PlanCacheTracker.unwrap()
    ));

    // maps.Clone must detach the mutable map storage from the original.
    // 修改原 map 不应影响 Detach 副本。
    obj.OptimizerFixControl.insert(1, "changed".to_owned());
    obj.OptimizerFixControl.insert(2, "new".to_owned());
    assert_eq!(
        static_obj.OptimizerFixControl.get(&1).map(String::as_str),
        Some("a")
    );
    assert!(!static_obj.OptimizerFixControl.contains_key(&2));
}
