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

// `CloneConstantsForPlanCache` 含中间 nil 条目的回归测试。
//
// 对应 Go `func_pointer_misc_test.go` / issue #66265：常量切片中间为 None 时
// 不得 panic，且克隆结果须在相同下标保留 None。

// 本文件对应 pkg/planner/util/utilfuncp/func_pointer_misc_test.go。
// 回归点：constants 中间包含 nil 时，CloneConstantsForPlanCache 不应 panic，且 nil 位置必须保留。

#![allow(non_snake_case)]

use std::sync::Arc;

use astersql_expression::{Column, Constant, mysql, types};

use super::CloneConstantsForPlanCache;

// TestCloneConstantsForPlanCacheWithNilEntry 对应 Go 同名测试（issue #66265）。
/// 强制走克隆路径后，验证中间 None 仍保留且不 panic。
#[test]
fn TestCloneConstantsForPlanCacheWithNilEntry() {
    // A Column with VirtualExpr set returns SafeToShareAcrossSession() == false.
    // Using it as DeferredExpr on a Constant makes that Constant unsafe,
    // which forces the cloning path (allSafe == false).
    // 带 VirtualExpr 的列视为会话不安全；挂到 Constant.DeferredExpr 可强制进入克隆分支。
    let mut unsafe_deferred_expr = Column::default();
    unsafe_deferred_expr.RetType = Some(*types::NewFieldType(mysql::TypeLonglong));
    unsafe_deferred_expr.VirtualExpr = Some(Box::new(Constant::with_type(
        types::NewIntDatum(0),
        *types::NewFieldType(mysql::TypeLonglong),
    )));

    let mut unsafe_const = Constant::with_type(
        types::NewIntDatum(1),
        *types::NewFieldType(mysql::TypeLonglong),
    );
    unsafe_const.DeferredExpr = Some(Box::new(unsafe_deferred_expr));

    // constants slice contains a nil entry -- this is the scenario that
    // caused a panic before the fix (issue #66265).
    // 中间插入 None，复现修复前会 panic 的场景。
    let constants = vec![
        Some(Arc::new(unsafe_const.clone())),
        None,
        Some(Arc::new(unsafe_const)),
    ];

    // Should not panic.
    let cloned = CloneConstantsForPlanCache(Some(&constants), None)
        .expect("nil outer slice is not used in this regression");

    assert_eq!(3, cloned.len());
    // The nil entry must be preserved as nil in the cloned slice.
    // 克隆后下标 1 仍须为 None。
    assert!(cloned[0].is_some());
    assert!(cloned[1].is_none());
    assert!(cloned[2].is_some());
}
