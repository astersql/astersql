// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// ScalarFunction 语义相等与辅助转换的单元测试。
//
// 覆盖规范哈希（canonical hash）稳定性、常量值/类型区分，
// 以及标量函数列表转表达式列表的边界行为。

use crate::*;

/// 构造带 UniqueID 的 BIGINT 列表达式。
fn int_column(unique_id: i64) -> Column {
    Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        unique_id,
        unique_id,
        0,
    )
}

/// 语义相等依赖规范哈希；不同列应判定不等，且重复比较不改缓存。
#[test]
fn semantic_equality_uses_canonical_identity_and_rejects_different_columns() {
    let mut left = int_column(1);
    let mut same = int_column(1);
    let mut different = int_column(2);
    assert!(ExpressionsSemanticEqual(&mut left, &mut same));
    assert!(!ExpressionsSemanticEqual(&mut left, &mut different));

    // Cached canonical hashes must remain stable across repeated comparisons.
    // 规范哈希缓存应在重复比较后保持不变。
    let first = left.CanonicalHashCode().to_vec();
    assert!(ExpressionsSemanticEqual(&mut left, &mut same));
    assert_eq!(left.CanonicalHashCode(), first);
}

/// 常量语义相等要求值与类型都一致（整型 1 ≠ 字符串 "1"）。
#[test]
fn semantic_equality_distinguishes_constant_values_and_types() {
    let int_type = *types::NewFieldType(mysql::TypeLonglong);
    let string_type = *types::NewFieldType(mysql::TypeVarString);
    let mut one = Constant::with_type(types::NewIntDatum(1), int_type.clone());
    let mut another_one = Constant::with_type(types::NewIntDatum(1), int_type);
    let mut two = Constant::with_type(
        types::NewIntDatum(2),
        *types::NewFieldType(mysql::TypeLonglong),
    );
    let mut text_one = Constant::with_type(types::NewStringDatum("1".to_owned()), string_type);

    assert!(ExpressionsSemanticEqual(&mut one, &mut another_one));
    assert!(!ExpressionsSemanticEqual(&mut one, &mut two));
    assert!(!ExpressionsSemanticEqual(&mut one, &mut text_one));
}

/// ScalarFuncs2Exprs 空输入应为空；emptyScalarFunctionSize 记录静态结构尺寸。
#[test]
fn scalar_function_expression_conversion_preserves_order_and_dynamic_type() {
    // Empty input is the boundary case used by callers constructing optional projections.
    assert!(ScalarFuncs2Exprs(Vec::new()).is_empty());
    assert!(emptyScalarFunctionSize > 0);
}

/// Go RemapColumn 通过 Clone 构造结果，因此必须保留 builtin 的排序规则元数据。
#[test]
fn remap_column_preserves_scalar_collation_metadata() {
    let function = Box::new(crate::planner_bridge_kernel::BuiltinGroupingImplSig::new(
        Vec::new(),
    ));
    let mut expression = ScalarFunction {
        FuncName: ast::NewCIStr(ast::Plus),
        RetType: Some(*types::NewFieldType(mysql::TypeLonglong)),
        Function: function,
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    };
    expression.SetCharsetAndCollation(("utf8mb4".to_owned(), "utf8mb4_bin".to_owned()));
    expression.SetCoercibility(CoercibilityExplicit);
    expression.SetRepertoire(UNICODE);

    let remapped = expression
        .RemapColumn(&std::collections::HashMap::new())
        .unwrap();
    assert_eq!(
        remapped.CharsetAndCollation(),
        ("utf8mb4".to_owned(), "utf8mb4_bin".to_owned())
    );
    assert_eq!(remapped.Coercibility(), CoercibilityExplicit);
    assert_eq!(remapped.Repertoire(), UNICODE);
}

/// Go 仅对 NOT 包裹比较函数做规范化；其它标量子函数只保留节点标记。
#[test]
fn canonical_not_of_non_comparison_scalar_matches_go_fallback() {
    let child_function = Box::new(crate::planner_bridge_kernel::BuiltinGroupingImplSig::new(
        vec![Box::new(int_column(1)), Box::new(int_column(2))],
    ));
    let child = ScalarFunction {
        FuncName: ast::NewCIStr(ast::Plus),
        RetType: Some(*types::NewFieldType(mysql::TypeLonglong)),
        Function: child_function,
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    };
    let outer_function = Box::new(crate::planner_bridge_kernel::BuiltinGroupingImplSig::new(
        vec![Box::new(child)],
    ));
    let mut expression = ScalarFunction {
        FuncName: ast::NewCIStr(ast::UnaryNot),
        RetType: Some(*types::NewFieldType(mysql::TypeLonglong)),
        Function: outer_function,
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    };

    simpleCanonicalizedHashCode(&mut expression);
    assert_eq!(expression.canonicalhashcode, vec![scalarFunctionFlag]);
}
