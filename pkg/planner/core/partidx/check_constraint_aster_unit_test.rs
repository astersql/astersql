// Copyright 2026 AsterSQL.
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

// 分区/部分索引约束判定的结构化单元测试。
//
// 重点验证无需 planner ranger 的表达式路径：列引用按逻辑 UniqueID 判等、
// 前置谓词精确匹配，以及单列 `IS NOT NULL` 的空值拒绝规则。

use super::*;

/// 构造列引用；`id` 与字段类型可独立变化，用于验证逻辑列身份只取决于 UniqueID。
fn column(unique_id: i64, id: i64, field_type: &str) -> Expression {
    Expression::Column(Column {
        id,
        unique_id,
        field_type: field_type.to_owned(),
    })
}

/// 构造“列与整数字面量比较”的测试表达式。
fn compare(column: Expression, operation: CompareOp, value: i64) -> Expression {
    Expression::Scalar(ScalarFunction {
        name: FunctionName::Compare(operation),
        arguments: vec![column, Expression::Literal(Literal::Integer(value))],
    })
}

/// 按实现支持的规范形式构造 `NOT(IS NULL(column))` 前置谓词。
fn is_not_null(column: Expression) -> Expression {
    Expression::Scalar(ScalarFunction {
        name: FunctionName::UnaryNot,
        arguments: vec![Expression::Scalar(ScalarFunction {
            name: FunctionName::IsNull,
            arguments: vec![column],
        })],
    })
}

#[test]
// 规划器改写可能改变物理列元数据，但相同 UniqueID 仍表示同一逻辑列。
fn structural_expression_equality_matches_go_column_identity() {
    let predicate = column(7, 11, "bigint");
    let filter = column(7, 99, "varchar");

    // 对齐 Go Column.Equal：只比较 UniqueID，不受物理 ID 与字段元数据差异影响。
    assert!(CheckConstraints(
        &StructuralContext,
        &[predicate],
        &[filter]
    ));
}

#[test]
// 空前置条件恒成立；非空前置条件则必须在过滤条件中找到一对一精确匹配。
fn check_constraints_handles_empty_and_one_to_one_exact_matches() {
    let first = column(1, 1, "int");
    let second = column(2, 2, "int");
    assert!(CheckConstraints(&StructuralContext, &[], &[]));
    assert!(CheckConstraints(
        &StructuralContext,
        &[first.clone()],
        &[second, first.clone()]
    ));
    assert!(!CheckConstraints(
        &StructuralContext,
        &[first.clone()],
        &[column(3, 3, "int")]
    ));
}

#[test]
// 普通比较会拒绝目标列上的 NULL，NULL-safe 等值比较则不能提供该保证。
fn always_meet_constraints_uses_logical_column_identity_and_rejects_null_safe_equal() {
    let target = column(8, 10, "int");
    let rewritten_target = column(8, 20, "bigint");
    let precondition = is_not_null(target);

    assert!(AlwaysMeetConstraints(
        &StructuralContext,
        &[precondition.clone()],
        &[compare(rewritten_target.clone(), CompareOp::Greater, 0)]
    ));
    assert!(!AlwaysMeetConstraints(
        &StructuralContext,
        &[precondition],
        &[compare(rewritten_target, CompareOp::NullEqual, 0)]
    ));
}

#[test]
// OR 只有在每个分支都拒绝目标列的 NULL 时，才能蕴含 `IS NOT NULL`。
fn always_meet_constraints_matches_go_and_or_null_rejection() {
    let target = column(9, 9, "int");
    let other = column(10, 10, "int");
    let precondition = is_not_null(target.clone());
    let rejected = compare(target.clone(), CompareOp::Equal, 1);
    let rejected_other = compare(other, CompareOp::Equal, 1);
    let disjunction = Expression::Scalar(ScalarFunction {
        name: FunctionName::LogicOr,
        arguments: vec![rejected.clone(), rejected],
    });
    let mixed_disjunction = Expression::Scalar(ScalarFunction {
        name: FunctionName::LogicOr,
        arguments: vec![disjunction, rejected_other],
    });

    assert!(!AlwaysMeetConstraints(
        &StructuralContext,
        &[precondition.clone()],
        &[mixed_disjunction]
    ));
    assert!(AlwaysMeetConstraints(
        &StructuralContext,
        &[precondition.clone()],
        &[Expression::Scalar(ScalarFunction {
            name: FunctionName::LogicOr,
            arguments: vec![
                compare(target.clone(), CompareOp::Equal, 1),
                compare(target.clone(), CompareOp::Greater, 2),
            ],
        })]
    ));
    assert!(!AlwaysMeetConstraints(
        &StructuralContext,
        &[precondition],
        &[Expression::Scalar(ScalarFunction {
            name: FunctionName::IsNull,
            arguments: vec![target],
        })]
    ));
}
