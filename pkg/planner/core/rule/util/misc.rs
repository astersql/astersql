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

// 逻辑优化规则共用杂项工具。
//
// 提供列/表达式的写时复制（Copy-on-Write）替换、外连接相关列归属判断、
// 「最大一行」等值条件检查、唯一索引能否充当键，以及谓词下推/简化钩子的
// 注册与调用；另含后序遍历构建键信息的门户。

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use base::LogicalPlan;
use expression::{Column, CorrelatedColumn, ExprBox, Expression, KeyInfo, ScalarFunction, Schema};

/// 列替换映射：键为列表达式的原始 HashCode 字节，值为替换后的子计划列。
pub type ColumnReplaceMap = HashMap<Vec<u8>, Column>;

/// 用列的二进制 HashCode 生成替换表查找键。
fn column_hash_key(column: &Column) -> Vec<u8> {
    Expression::HashCode(column)
}

/// 按替换表解析单列；命中时保留原点 RetType/InOperand，身份取自目标列。
fn resolve_column_and_replace(origin: &Column, replace: &ColumnReplaceMap) -> (Column, bool) {
    let Some(destination) = replace.get(&column_hash_key(origin)) else {
        return (origin.clone(), false);
    };

    // The destination identifies the child's column. The result type and IN
    // operand marker belong to the expression being rewritten.
    // 目标列给出子节点列身份；返回类型与 IN 操作数标记仍属被改写的原表达式。
    let mut replaced = destination.clone();
    replaced.RetType = origin.RetType.clone();
    replaced.InOperand = origin.InOperand;
    (replaced, true)
}

/// Replaces column fields using child logical-plan columns.
///
/// Scalar-function trees are rewritten copy-on-write: an unchanged root is
/// returned intact, while the first changed descendant causes its ancestors to
/// be cloned before their argument slots are updated.
///
/// 用子逻辑计划中的列替换表达式里的列字段；标量函数树写时复制改写。
pub fn ResolveExprAndReplace(origin: ExprBox, replace: &ColumnReplaceMap) -> ExprBox {
    resolve_expr_and_replace(origin, replace).0
}

/// 递归改写表达式；返回 `(新表达式, 是否发生变化)`。
fn resolve_expr_and_replace(origin: ExprBox, replace: &ColumnReplaceMap) -> (ExprBox, bool) {
    // 普通列：直接查替换表。
    if let Some(column) = origin.as_any().downcast_ref::<Column>() {
        let (column, changed) = resolve_column_and_replace(column, replace);
        return if changed {
            (Box::new(column), true)
        } else {
            (origin, false)
        };
    }

    // 相关列（Correlated Column，引用外层查询的列）：只替换其内嵌列。
    if let Some(correlated) = origin.as_any().downcast_ref::<CorrelatedColumn>() {
        let (column, changed) = resolve_column_and_replace(&correlated.column, replace);
        if !changed {
            return (origin, false);
        }
        let mut cloned = correlated.Clone();
        cloned.column = column;
        return (Box::new(cloned), true);
    }

    // 标量函数：先递归参数，任一参数变化再克隆函数节点。
    if let Some(function) = origin.as_any().downcast_ref::<ScalarFunction>() {
        let mut replacements = Vec::with_capacity(function.GetArgs().len());
        let mut any_changed = false;
        for argument in function.GetArgs() {
            let (rewritten, changed) = resolve_expr_and_replace(argument.CloneExpr(), replace);
            replacements.push((rewritten, changed));
            any_changed |= changed;
        }
        if !any_changed {
            return (origin, false);
        }

        let mut cloned = function.clone_scalar();
        cloned.SetCharsetAndCollation(function.CharsetAndCollation());
        cloned.SetCoercibility(function.Coercibility());
        cloned.SetRepertoire(function.Repertoire());
        for (slot, (rewritten, changed)) in
            cloned.GetArgsMut().iter_mut().zip(replacements.into_iter())
        {
            if changed {
                *slot = rewritten;
            }
        }
        return (Box::new(cloned), true);
    }

    (origin, false)
}

/// Replaces a column using its binary expression hash.
/// 按表达式二进制哈希在替换表中查找并替换单列。
pub fn ResolveColumnAndReplace(origin: &Column, replace: &ColumnReplaceMap) -> Column {
    resolve_column_and_replace(origin, replace).0
}

/// Replaces columns in an expression with the corresponding projection
/// expression, preserving scalar-function copy-on-write behavior.
/// 用投影表达式按 schema 下标替换列，同样写时复制标量函数树。
pub fn ReplaceColumnOfExpr(expr: ExprBox, exprs: &[ExprBox], schema: &Schema) -> ExprBox {
    replace_column_of_expr(expr, exprs, schema).0
}

/// 递归：列命中 schema 则换成对应投影表达式；函数节点写时复制。
fn replace_column_of_expr(expr: ExprBox, exprs: &[ExprBox], schema: &Schema) -> (ExprBox, bool) {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        if let Some(index) = schema.ColumnIndex(column)
            && index < exprs.len()
        {
            return (exprs[index].CloneExpr(), true);
        }
        return (expr, false);
    }

    if let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() {
        let mut replacements = Vec::with_capacity(function.GetArgs().len());
        let mut any_changed = false;
        for argument in function.GetArgs() {
            let (rewritten, changed) = replace_column_of_expr(argument.CloneExpr(), exprs, schema);
            replacements.push((rewritten, changed));
            any_changed |= changed;
        }
        if !any_changed {
            return (expr, false);
        }

        let mut cloned = function.clone_scalar();
        cloned.SetCharsetAndCollation(function.CharsetAndCollation());
        cloned.SetCoercibility(function.Coercibility());
        cloned.SetRepertoire(function.Repertoire());
        for (slot, (rewritten, changed)) in
            cloned.GetArgsMut().iter_mut().zip(replacements.into_iter())
        {
            if changed {
                *slot = rewritten;
            }
        }
        return (Box::new(cloned), true);
    }

    (expr, false)
}

/// Checks whether every column belongs to the outer plan. Empty input is
/// deliberately false for both aggregate-column and parent-column callers.
/// 判断列是否全部来自外层计划；空输入刻意返回 false。
pub fn IsColsAllFromOuterTable(cols: &[Column], outer_unique_ids: &intset::FastIntSet) -> bool {
    !cols.is_empty()
        && cols
            .iter()
            .all(|column| outer_unique_ids.Has(column.UniqueID as i32))
}

/// Checks whether at least one column belongs to the inner plan.
/// 判断是否至少有一列来自内层计划。
pub fn IsColFromInnerTable(cols: &[Column], inner_unique_ids: &intset::FastIntSet) -> bool {
    cols.iter()
        .any(|column| inner_unique_ids.Has(column.UniqueID as i32))
}

/// Returns true when equality predicates cover every column of any primary,
/// non-null unique, or nullable unique key.
/// 等值谓词是否覆盖任一主键、非空唯一键或可空唯一键的全部列（可推出最多一行）。
pub fn CheckMaxOneRowCond(eq_col_ids: &HashSet<i64>, child_schema: &Schema) -> bool {
    if eq_col_ids.is_empty() {
        return false;
    }
    child_schema
        .PKOrUK
        .iter()
        .chain(&child_schema.NullableUK)
        .any(|key| {
            key.iter()
                .all(|column| eq_col_ids.contains(&column.UniqueID))
        })
}

/// Checks whether an index contributes a non-null key or a nullable unique
/// key to the expression schema.
/// 检查唯一索引能否贡献非空键（new_key）或可空唯一键（unique_key）到表达式 schema。
pub fn CheckIndexCanBeKey(
    index: &model::IndexInfo,
    columns: &[model::ColumnInfo],
    schema: &Schema,
) -> (Option<KeyInfo>, Option<KeyInfo>) {
    if !index.Unique {
        return (None, None);
    }

    let mut unique_key = Vec::with_capacity(index.Columns.len());
    let mut new_key = Vec::with_capacity(index.Columns.len());
    let mut new_key_ok = true;
    let mut unique_key_ok = true;

    // 按索引列顺序在表列中定位，同时累积可空唯一键与全非空键。
    for index_column in &index.Columns {
        let mut found_unique_key = false;
        for (position, column) in columns.iter().enumerate() {
            if index_column.Name.L != column.Name.L {
                continue;
            }
            unique_key.push(schema.Columns[position].clone());
            found_unique_key = true;
            if new_key_ok {
                if !mysql::r#type::HasNotNullFlag(column.GetFlag()) {
                    new_key_ok = false;
                    break;
                }
                new_key.push(schema.Columns[position].clone());
                break;
            }
        }
        if !found_unique_key {
            new_key_ok = false;
            unique_key_ok = false;
            break;
        }
    }

    // 优先返回全非空键；否则退回可空唯一键。
    if new_key_ok {
        (None, Some(new_key))
    } else if unique_key_ok {
        (Some(unique_key), None)
    } else {
        (None, None)
    }
}

/// 设置谓词下推（Predicate Push Down）标志位的钩子类型。
pub type SetPredicatePushDownFlagHook = fn(u64) -> u64;
/// 常量传播时过滤合法表达式的钩子类型。
pub type ValidConstantPropagationExpressionFilter = fn(&dyn Expression) -> bool;
/// 普通谓词简化钩子：上下文、谓词列表、是否传播常量、可选过滤器。
pub type PredicateSimplificationHook = fn(
    base::ContextRef,
    Vec<ExprBox>,
    bool,
    Option<ValidConstantPropagationExpressionFilter>,
) -> Vec<ExprBox>;
/// Join 场景下的谓词简化钩子，额外传入两侧 schema。
pub type PredicateSimplificationForJoinHook = fn(
    base::ContextRef,
    Vec<ExprBox>,
    &Schema,
    &Schema,
    bool,
    Option<ValidConstantPropagationExpressionFilter>,
) -> Vec<ExprBox>;

/// 谓词下推标志钩子的全局一次性注册槽。
static SET_PREDICATE_PUSH_DOWN_FLAG: OnceLock<SetPredicatePushDownFlagHook> = OnceLock::new();
/// 普通谓词简化钩子的全局注册槽。
static APPLY_PREDICATE_SIMPLIFICATION: OnceLock<PredicateSimplificationHook> = OnceLock::new();
/// Join 谓词简化钩子的全局注册槽。
static APPLY_PREDICATE_SIMPLIFICATION_FOR_JOIN: OnceLock<PredicateSimplificationForJoinHook> =
    OnceLock::new();

/// 注册 SetPredicatePushDownFlag 钩子；仅首次成功。
pub fn RegisterSetPredicatePushDownFlag(hook: SetPredicatePushDownFlagHook) -> bool {
    SET_PREDICATE_PUSH_DOWN_FLAG.set(hook).is_ok()
}

/// 注册 ApplyPredicateSimplification 钩子；仅首次成功。
pub fn RegisterApplyPredicateSimplification(hook: PredicateSimplificationHook) -> bool {
    APPLY_PREDICATE_SIMPLIFICATION.set(hook).is_ok()
}

/// 注册 ApplyPredicateSimplificationForJoin 钩子；仅首次成功。
pub fn RegisterApplyPredicateSimplificationForJoin(
    hook: PredicateSimplificationForJoinHook,
) -> bool {
    APPLY_PREDICATE_SIMPLIFICATION_FOR_JOIN.set(hook).is_ok()
}

/// Reports whether all three callbacks installed by rule package initialization exist.
pub fn RuleInitHooksRegistered() -> bool {
    SET_PREDICATE_PUSH_DOWN_FLAG.get().is_some()
        && APPLY_PREDICATE_SIMPLIFICATION.get().is_some()
        && APPLY_PREDICATE_SIMPLIFICATION_FOR_JOIN.get().is_some()
}

/// Invokes the rule-owned hook. As in Go, calling it before package
/// initialization is a programming error.
/// 调用已注册的谓词下推标志钩子；未注册时与 Go 一样视为编程错误。
pub fn SetPredicatePushDownFlag(flag: u64) -> u64 {
    SET_PREDICATE_PUSH_DOWN_FLAG
        .get()
        .expect("SetPredicatePushDownFlag hook is not registered")(flag)
}

/// 调用已注册的普通谓词简化钩子。
pub fn ApplyPredicateSimplification(
    context: base::ContextRef,
    predicates: Vec<ExprBox>,
    propagate_constant: bool,
    filter: Option<ValidConstantPropagationExpressionFilter>,
) -> Vec<ExprBox> {
    APPLY_PREDICATE_SIMPLIFICATION
        .get()
        .expect("ApplyPredicateSimplification hook is not registered")(
        context,
        predicates,
        propagate_constant,
        filter,
    )
}

/// 调用已注册的 Join 谓词简化钩子。
pub fn ApplyPredicateSimplificationForJoin(
    context: base::ContextRef,
    predicates: Vec<ExprBox>,
    schema1: &Schema,
    schema2: &Schema,
    propagate_constant: bool,
    filter: Option<ValidConstantPropagationExpressionFilter>,
) -> Vec<ExprBox> {
    APPLY_PREDICATE_SIMPLIFICATION_FOR_JOIN
        .get()
        .expect("ApplyPredicateSimplificationForJoin hook is not registered")(
        context,
        predicates,
        schema1,
        schema2,
        propagate_constant,
        filter,
    )
}

/// Recursively builds key information in post-order so every parent sees the
/// completed schemas of all its children.
/// 后序递归构建键信息，使父节点看到子节点已完成的 schema。
pub fn BuildKeyInfoPortal(plan: &mut dyn LogicalPlan) {
    for child in plan.logical_children_mut() {
        BuildKeyInfoPortal(child);
    }

    // The trait deliberately lets concrete operators mutate their owned schema
    // through `&mut self`; these snapshots avoid aliasing the same plan while
    // supplying the Go method's self/child schema inputs.
    // trait 允许算子通过 &mut self 改自己的 schema；快照避免自引用别名，
    // 同时提供与 Go 方法一致的 self/子 schema 入参。
    let self_schema = plan.schema().Clone();
    let child_schemas: Vec<Schema> = plan
        .logical_children()
        .into_iter()
        .map(|child| child.schema().Clone())
        .collect();
    plan.build_key_info(&self_schema, &child_schemas);
}
