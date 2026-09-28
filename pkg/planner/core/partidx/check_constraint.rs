// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 分区/部分索引（partial index）约束蕴含判定。
//
// 判断过滤器谓词是否蕴含索引元数据中的前置谓词：先做精确匹配，
// 再对单列比较与 `NOT(IS NULL(col))` 形式做区间蕴含证明。
// `AlwaysMeetConstraints` 用于更强的空值拒绝（null-reject）选择条件。

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 列引用：物理 id、规划 UniqueID 与字段类型描述。
pub struct Column {
    pub id: i64,
    pub unique_id: i64,
    pub field_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 字面量：空值、布尔、整型、字节与文本。
pub enum Literal {
    Null,
    Boolean(bool),
    Integer(i64),
    Unsigned(u64),
    Bytes(Vec<u8>),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 比较运算符，含 MySQL 语义的 NULL-safe 等值（NullEqual）。
pub enum CompareOp {
    Less,
    LessEqual,
    Equal,
    NotEqual,
    NullEqual,
    GreaterEqual,
    Greater,
    In,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 标量函数名：比较、取反、IS NULL、逻辑与/或及其他。
pub enum FunctionName {
    Compare(CompareOp),
    UnaryNot,
    IsNull,
    LogicAnd,
    LogicOr,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 标量函数调用：函数名加参数表达式列表。
pub struct ScalarFunction {
    pub name: FunctionName,
    pub arguments: Vec<Expression>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 表达式树：列、标量函数或字面量。
pub enum Expression {
    Column(Column),
    Scalar(ScalarFunction),
    Literal(Literal),
}

impl Expression {
    /// 若为列表达式则返回列引用。
    pub fn column(&self) -> Option<&Column> {
        match self {
            Self::Column(column) => Some(column),
            _ => None,
        }
    }

    /// 若为标量函数则返回函数节点。
    pub fn scalar(&self) -> Option<&ScalarFunction> {
        match self {
            Self::Scalar(function) => Some(function),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 区间端点值：空、最小非空、最大、整型、字节或文本。
pub enum BoundValue {
    Null,
    MinNotNull,
    MaxValue,
    Integer(i64),
    Unsigned(u64),
    Bytes(Vec<u8>),
    Text(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 多列区间：低/高边界值向量及开闭区间标志。
pub struct Range {
    pub low_values: Vec<BoundValue>,
    pub high_values: Vec<BoundValue>,
    pub low_exclude: bool,
    pub high_exclude: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 区间构造或合并失败时的错误消息包装。
pub struct RangeError(pub String);

impl fmt::Display for RangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RangeError {}

/// Adapter for the two canonical services used by the Go implementation:
/// expression equality and ranger. The adapter keeps collation, conversion,
/// memory quota, warning and plan-cache behavior owned by the caller's real
/// planner contexts instead of reimplementing those policies here.
/// 约束判定适配器：表达式相等、列条件抽取与区间构造/并集（对齐 Go ranger 边界）。
pub trait ConstraintContext {
    fn expressions_equal(&self, left: &Expression, right: &Expression) -> bool;

    fn build_column_ranges(
        &self,
        conditions: &[Expression],
        column: &Column,
        column_length: i32,
        range_memory_quota: i64,
    ) -> Result<Vec<Range>, RangeError>;

    fn extract_access_conditions_for_column(
        &self,
        filters: &[Expression],
        column: &Column,
    ) -> Vec<Expression>;

    fn union_ranges(
        &self,
        ranges: Vec<Range>,
        merge_consecutive: bool,
    ) -> Result<Vec<Range>, RangeError>;
}

/// A structural context is useful when expressions and already-normalized
/// ranges do not require session collation or type conversion.
/// 结构相等轻量上下文；真实区间构造需接入规划器 ranger。
#[derive(Clone, Copy, Default)]
pub struct StructuralContext;

/// Expression.Equal in Go compares planner columns by UniqueID rather than
/// by physical metadata. Keep that identity rule recursive for scalar trees.
/// 结构表达式相等遵循 Go 的列唯一身份规则，而不是比较物理列元数据。
fn structural_expression_equal(left: &Expression, right: &Expression) -> bool {
    match (left, right) {
        (Expression::Column(left), Expression::Column(right)) => left.unique_id == right.unique_id,
        (Expression::Scalar(left), Expression::Scalar(right)) => {
            left.name == right.name
                && left.arguments.len() == right.arguments.len()
                && left
                    .arguments
                    .iter()
                    .zip(&right.arguments)
                    .all(|(left, right)| structural_expression_equal(left, right))
        }
        (Expression::Literal(left), Expression::Literal(right)) => left == right,
        _ => false,
    }
}

impl ConstraintContext for StructuralContext {
    fn expressions_equal(&self, left: &Expression, right: &Expression) -> bool {
        structural_expression_equal(left, right)
    }

    fn build_column_ranges(
        &self,
        _conditions: &[Expression],
        _column: &Column,
        _column_length: i32,
        _range_memory_quota: i64,
    ) -> Result<Vec<Range>, RangeError> {
        Err(RangeError(
            "range construction requires a planner ranger adapter".to_owned(),
        ))
    }

    fn extract_access_conditions_for_column(
        &self,
        filters: &[Expression],
        column: &Column,
    ) -> Vec<Expression> {
        filters
            .iter()
            .filter(|expression| containsColumn(expression, column))
            .cloned()
            .collect()
    }

    fn union_ranges(
        &self,
        _ranges: Vec<Range>,
        _merge_consecutive: bool,
    ) -> Result<Vec<Range>, RangeError> {
        Err(RangeError(
            "range union requires a planner ranger adapter".to_owned(),
        ))
    }
}

/// Checks whether filters imply the predicates stored in partial-index metadata.
/// Exact equality is attempted first. The implication proof supports the same
/// single-column comparisons and `NOT(IS NULL(column))` form as the Go package.
/// 判断 filters 是否蕴含 partial-index 前置谓词：先精确匹配再做蕴含证明。
pub fn CheckConstraints<C: ConstraintContext + ?Sized>(
    context: &C,
    pre_predicates: &[Expression],
    filters: &[Expression],
) -> bool {
    if pre_predicates.is_empty() {
        return true;
    }
    // Go asserts this metadata invariant under intest before indexing element 0.
    if pre_predicates.len() != 1 {
        return false;
    }
    exactMatch(context, pre_predicates, filters)
        || canBeImpliedFromExprs(context, &pre_predicates[0], filters)
}

/// 前置谓词是否均可在 filters 中找到表达式相等的项（一对一消耗）。
fn exactMatch<C: ConstraintContext + ?Sized>(
    context: &C,
    pre_predicates: &[Expression],
    filters: &[Expression],
) -> bool {
    let mut matched = vec![false; filters.len()];
    for predicate in pre_predicates {
        let mut found = false;
        for (index, filter) in filters.iter().enumerate() {
            if matched[index] {
                continue;
            }
            if context.expressions_equal(predicate, filter) {
                matched[index] = true;
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}

/// 对比较谓词或 `NOT(IS NULL(col))` 尝试由 filters 蕴含。
fn canBeImpliedFromExprs<C: ConstraintContext + ?Sized>(
    context: &C,
    predicate: &Expression,
    filters: &[Expression],
) -> bool {
    let Some(function) = predicate.scalar() else {
        return false;
    };
    if function.name == FunctionName::UnaryNot {
        let Some(inner) = function.arguments.first().and_then(Expression::scalar) else {
            return false;
        };
        if inner.name != FunctionName::IsNull {
            return false;
        }
        let Some(column) = inner.arguments.first().and_then(Expression::column) else {
            return false;
        };
        return implIsNotNull(context, column, filters);
    }
    if !matches!(function.name, FunctionName::Compare(_)) {
        return false;
    }
    implCompareExpr(context, function, filters)
}

/// 用 ranger 区间并集证明：filter 区间 ∪ 谓词区间 == 谓词区间（即 filter ⊆ 谓词）。
fn implCompareExpr<C: ConstraintContext + ?Sized>(
    context: &C,
    predicate: &ScalarFunction,
    filters: &[Expression],
) -> bool {
    let Some(column) = predicate
        .arguments
        .first()
        .and_then(Expression::column)
        .or_else(|| predicate.arguments.get(1).and_then(Expression::column))
    else {
        return false;
    };
    let predicate_expression = Expression::Scalar(predicate.clone());
    let Ok(predicate_ranges) = context.build_column_ranges(&[predicate_expression], column, -1, 0)
    else {
        return false;
    };
    if predicate_ranges.is_empty() {
        return false;
    }

    let column_conditions = context.extract_access_conditions_for_column(filters, column);
    if column_conditions.is_empty() {
        return false;
    }
    let Ok(filter_ranges) = context.build_column_ranges(&column_conditions, column, -1, 0) else {
        return false;
    };
    if filter_ranges.is_empty() {
        return false;
    }

    let mut combined = filter_ranges;
    combined.extend(predicate_ranges.iter().cloned());
    let Ok(unioned) = context.union_ranges(combined, false) else {
        return false;
    };
    unioned == predicate_ranges
}

/// 证明 filters 对目标列排除 NULL（区间下界不为未排除的 Null）。
fn implIsNotNull<C: ConstraintContext + ?Sized>(
    context: &C,
    target_column: &Column,
    filters: &[Expression],
) -> bool {
    let column_conditions = context.extract_access_conditions_for_column(filters, target_column);
    if column_conditions.is_empty() {
        return false;
    }
    let Ok(ranges) = context.build_column_ranges(&column_conditions, target_column, -1, 0) else {
        return false;
    };
    !ranges.is_empty()
        && ranges.iter().all(|range| {
            !range
                .low_values
                .first()
                .is_some_and(|value| *value == BoundValue::Null && !range.low_exclude)
        })
}

/// Checks the stronger property used when choosing an index: a single
/// `NOT(IS NULL(column))` predicate must be null-rejected by at least one filter.
/// 更强判定：单一 `NOT(IS NULL(col))` 须被至少一条 filter 空值拒绝。
pub fn AlwaysMeetConstraints<C: ConstraintContext + ?Sized>(
    context: &C,
    pre_predicates: &[Expression],
    filters: &[Expression],
) -> bool {
    if pre_predicates.len() != 1 {
        return false;
    }
    let Some(outer) = pre_predicates[0].scalar() else {
        return false;
    };
    if outer.name != FunctionName::UnaryNot {
        return false;
    }
    let Some(inner) = outer.arguments.first().and_then(Expression::scalar) else {
        return false;
    };
    if inner.name != FunctionName::IsNull {
        return false;
    }
    let Some(column) = inner.arguments.first().and_then(Expression::column) else {
        return false;
    };
    filters.iter().any(|filter| {
        filter
            .scalar()
            .is_some_and(|function| checkIsNullRejected(context, column, function))
    })
}

/// 递归检查 AND/OR 组合下，比较谓词是否对目标列形成空值拒绝。
fn checkIsNullRejected<C: ConstraintContext + ?Sized>(
    context: &C,
    target_column: &Column,
    filter: &ScalarFunction,
) -> bool {
    if filter.name == FunctionName::LogicOr {
        return filter.arguments.iter().all(|argument| {
            argument
                .scalar()
                .is_some_and(|function| checkIsNullRejected(context, target_column, function))
        });
    }
    if filter.name == FunctionName::LogicAnd {
        return filter.arguments.iter().any(|argument| {
            argument
                .scalar()
                .is_some_and(|function| checkIsNullRejected(context, target_column, function))
        });
    }
    if filter.name == FunctionName::IsNull
        && filter.arguments.first().is_some_and(|argument| {
            context.expressions_equal(argument, &Expression::Column(target_column.clone()))
        })
    {
        return false;
    }
    let FunctionName::Compare(operation) = filter.name else {
        return false;
    };
    if operation == CompareOp::NullEqual {
        return false;
    }
    filter
        .arguments
        .first()
        .and_then(Expression::column)
        .or_else(|| filter.arguments.get(1).and_then(Expression::column))
        .is_some_and(|column| {
            context.expressions_equal(
                &Expression::Column(column.clone()),
                &Expression::Column(target_column.clone()),
            )
        })
}

/// 表达式树是否引用目标列（按 unique_id）。
fn containsColumn(expression: &Expression, target: &Column) -> bool {
    match expression {
        Expression::Column(column) => column.unique_id == target.unique_id,
        Expression::Scalar(function) => function
            .arguments
            .iter()
            .any(|argument| containsColumn(argument, target)),
        Expression::Literal(_) => false,
    }
}
