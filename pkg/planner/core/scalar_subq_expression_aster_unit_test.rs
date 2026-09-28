// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 标量子查询表达式（ScalarSubQueryExpr）的 Aster 单元测试。
//
// 校验哈希编码与 Go 一致、未求值时的计划器契约，以及 `as_any_mut` 类型擦除还原。

use expression_dependency as expression;

use super::scalar_subq_expression::ScalarSubQueryExpr;

/// 构造仅设置列 ID、无求值上下文的标量子查询表达式。
fn scalar_subquery(column_id: i64) -> ScalarSubQueryExpr {
    let mut expression = ScalarSubQueryExpr::default();
    expression.scalar_subquery_col_id = column_id;
    expression
}

/// 验证字符串表示与 HashCode/CanonicalHashCode 编码与 Go 对齐。
#[test]
fn scalar_subquery_identity_matches_go_encoding() {
    let expression = scalar_subquery(-42);

    let mut expected = vec![expression::SCALAR_SUB_Q_FLAG];
    expected.extend_from_slice(&((-42_i64 as u64) ^ 0x8000_0000_0000_0000).to_be_bytes());

    assert_eq!(expression.String(), "ScalarQueryCol#-42");
    assert_eq!(expression::Expression::HashCode(&expression), expected);
    assert_eq!(
        expression::Expression::CanonicalHashCode(&expression),
        expected
    );
}

/// 验证未求值时：相等性按列 ID、声明可向量化、不可跨会话共享、内存为 0。
#[test]
fn unevaluated_scalar_subquery_keeps_planner_contract() {
    let expression = scalar_subquery(7);
    let equal = expression.clone();
    let different = scalar_subquery(8);

    assert!(expression::base::Equals::Equals(&expression, &equal));
    assert!(!expression::base::Equals::Equals(&expression, &different));
    assert!(expression::VecExpr::Vectorized(&expression));
    assert!(!expression::SafeToShareAcrossSession::SafeToShareAcrossSession(&expression));
    assert_eq!(expression::Expression::MemoryUsage(&expression), 0);
}

/// 验证通过 `as_any_mut` 向下转型后可修改具体类型字段。
#[test]
fn mutable_type_erasure_recovers_the_real_scalar_expression() {
    let mut scalar = scalar_subquery(11);

    expression::Expression::as_any_mut(&mut scalar)
        .downcast_mut::<ScalarSubQueryExpr>()
        .expect("mutable expression downcast must retain the concrete type")
        .scalar_subquery_col_id = 29;

    assert_eq!(scalar.String(), "ScalarQueryCol#29");
}
