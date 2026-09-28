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

// 逻辑算子侧的谓词常量判定工具。
//
// 用于在谓词下推等优化中识别恒假/恒 NULL 条件，从而将子计划折叠为 TableDual
//（空结果逻辑表）。刻意排除参数化与延迟常量，以免 Plan Cache 复用时取值变化导致误判。
use crate::Expression;

/// 若存在恒假或恒 NULL 的常量谓词，则整表结果为空，可改写为 TableDual。
/// Returns true when every row is rejected by a constant predicate.  Parameter
/// and deferred constants are deliberately excluded because their next value
/// can differ when a cached plan is reused.
pub fn Conds2TableDual(conds: &[Expression]) -> bool {
    if conds.is_empty() {
        return false;
    }
    if conds.iter().any(|condition| {
        expression::IsConstNull(condition.as_ref()) && !is_mutable_constant(condition.as_ref())
    }) {
        return true;
    }
    // Go deliberately checks non-NULL false only for a single condition.  With
    // several predicates, later optimizer stages must retain their joint shape.
    conds.len() == 1 && IsConstFalse(conds[0].as_ref())
}

fn is_mutable_constant(cond: &dyn expression::Expression) -> bool {
    cond.as_any()
        .downcast_ref::<expression::Constant>()
        .is_some_and(|constant| constant.DeferredExpr.is_some() || constant.ParamMarker.is_some())
}

/// 判断表达式是否为“常量假”（含 NULL）。排除 DeferredExpr/ParamMarker，避免缓存计划误折叠。
pub fn IsConstFalse(cond: &dyn expression::Expression) -> bool {
    let Some(constant) = cond.as_any().downcast_ref::<expression::Constant>() else {
        return false;
    };
    // 参数占位与延迟表达式的值在不同执行间可能变化，不能当作编译期常量假。
    if is_mutable_constant(cond) {
        return false;
    }
    let value = &constant.Value;
    if value.IsNull() {
        return true;
    }
    // Go uses Datum.ToBool with the statement type context.  This context-free
    // adapter uses the standard no-warning type context and, like Go, treats a
    // conversion error as not proven false.
    let type_ctx = (*types::DefaultStmtNoWarningContext).clone();
    value.ToBool(type_ctx).is_ok_and(|is_true| is_true == 0)
}
