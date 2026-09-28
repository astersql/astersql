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

// 约束/谓词表达式清理工具。
//
// 在优化器改写阶段删除可证明恒真的条件（谓词），减少后续选择率估算与下推的噪声。
// 谓词（predicate）即 WHERE/ON/HAVING 中的布尔条件；计划缓存（plan cache）场景下
// 可变参数不得因单次求值结果被误删。

use crate::{ast, expression, mysql, stmtctx};

// 在内存中筛除可证明恒真的谓词。
// expression、ast、mysql 与 stmtctx 通过 crate 根的兼容模块保持 Go 包边界。

/// 删除可安全求值为 true 的常量条件；对应 Go 的 `DeleteTrueExprs`。
///
/// 计划缓存参数、求值错误与 NULL 均保留原条件，避免过优化。
// DeleteTrueExprs 对应 Go 的 DeleteTrueExprs：删除可安全求值为 true 的常量条件。
pub fn DeleteTrueExprs(
    build_ctx: &expression::BuildContext,
    stmt_ctx: &stmtctx::StatementContext,
    conds: Vec<expression::Expression>,
) -> Vec<expression::Expression> {
    if conds.is_empty() {
        return conds;
    }

    conds
        .into_iter()
        .filter(|cond| {
            let Some(constant) = cond.as_constant() else {
                return true;
            };
            // 计划缓存参数可能在后续执行阶段取不同值，不能因为本次求值为 true 就删掉。
            if expression::MaybeOverOptimized4PlanCache(build_ctx, constant) {
                return true;
            }
            // Go 只有在转换无错误且结果恰为 1 时删除；错误和 NULL 都保留原条件。
            !matches!(constant.Value.ToBool(stmt_ctx.TypeCtx()), Ok(1))
        })
        .collect()
}

/// 删除 `NOT(ISNULL(NOT NULL 列))` 这类可由 schema 证明恒真的条件。
///
/// 用于谓词下推（predicate pushdown）前清理；其余表达式顺序与值不变。
// DeleteTrueExprsBySchema 删除 `not(isnull(not null column))` 这种由 schema 可证明为真的条件。
// 该函数用于谓词下推前清理，其他表达式的顺序和值均保持不变。
pub fn DeleteTrueExprsBySchema(
    ctx: &expression::EvalContext,
    schema: &expression::Schema,
    conds: Vec<expression::Expression>,
) -> Vec<expression::Expression> {
    conds
        .into_iter()
        .filter(|item| {
            let Some(expr) = item.as_scalar_function() else {
                return true;
            };
            if expr.FuncName.L != ast::UnaryNot || expr.GetArgs().len() != 1 {
                return true;
            }
            !isNullWithNotNullColumn(ctx, schema, &expr.GetArgs()[0])
        })
        .collect()
}

/// 判断表达式是否为「对 schema 中 NOT NULL 列的 IS NULL」。
///
/// 对应 Go 的嵌套类型断言：仅单参数 `IS NULL`，且列可从 schema 找回并带 NOT NULL 标志。
// isNullWithNotNullColumn 对应 Go 的嵌套类型断言：只接受单参数 IS NULL，且参数列能从 schema 找回。
fn isNullWithNotNullColumn(
    ctx: &expression::EvalContext,
    schema: &expression::Schema,
    expr: &expression::Expression,
) -> bool {
    let Some(is_null) = expr.as_scalar_function() else {
        return false;
    };
    if is_null.FuncName.L != ast::IsNull || is_null.GetArgs().len() != 1 {
        return false;
    }
    let Some(column) = is_null.GetArgs()[0].as_column() else {
        return false;
    };
    let Some(retrieved) = schema.RetrieveColumn(column) else {
        return false;
    };
    // 最终依据 MySQL NOT NULL 标志判断，而不是仅凭列对象存在就删除谓词。
    mysql::HasNotNullFlag(retrieved.GetType(ctx).GetFlag())
}
