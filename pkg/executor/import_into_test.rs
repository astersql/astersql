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
// `IMPORT INTO` 列赋值表达式可选属性校验的单元测试。
//
// `IMPORT INTO`：将外部数据批量导入表；列赋值中的标量函数
// 只能依赖编码上下文已提供的可选属性位。

use crate::import_into::{ImportExpression, checkExprWithProvidedProps};
#[derive(Clone)]
/// 测试用表达式树节点，实现 `ImportExpression`。
struct Expr {
    name: Option<&'static str>,
    required: u64,
    children: Vec<Expr>,
}
impl ImportExpression for Expr {
    fn scalar_function_name(&self) -> Option<&str> {
        self.name
    }
    fn required_optional_properties(&self) -> u64 {
        self.required
    }
    fn children(&self) -> &[Self] {
        &self.children
    }
}
#[test]
/// 验证属性位不足时递归报错指向子函数，位足够时通过。
fn import_assignments_reject_unsupported_optional_properties_recursively() {
    let expr = Expr {
        name: Some("parent"),
        required: 1,
        children: vec![Expr {
            name: Some("child"),
            required: 2,
            children: vec![],
        }],
    };
    assert_eq!(checkExprWithProvidedProps(3, &expr, 3), Ok(()));
    let error = checkExprWithProvidedProps(3, &expr, 1).unwrap_err();
    assert_eq!(error.function_name, "child");
    assert_eq!(error.assignment_index, 3);
}

#[test]
/// Mirrors Go's assignment matrix: supported functions pass, while the first
/// unsupported scalar reports its exact normalized name and assignment index.
fn import_assignment_validation_matches_go_error_contract() {
    let cases = [
        ("setvar", 0usize),
        ("current_user", 0),
        ("current_role", 0),
        ("connection_id", 0),
        ("tidb_is_ddl_owner", 1),
        ("sleep", 0),
        ("last_insert_id", 0),
    ];

    for (name, index) in cases {
        let expression = Expr {
            name: Some(name),
            required: 1,
            children: vec![],
        };
        let error = checkExprWithProvidedProps(index, &expression, 0).unwrap_err();
        assert_eq!(error.function_name, name);
        assert_eq!(error.assignment_index, index);
        assert_eq!(
            error.to_string(),
            format!(
                "FUNCTION {name} is not supported in IMPORT INTO column assignment, index {index}"
            )
        );
    }

    let supported = Expr {
        name: Some("concat"),
        required: 0,
        children: vec![Expr {
            name: Some("getvar"),
            required: 0,
            children: vec![],
        }],
    };
    assert_eq!(checkExprWithProvidedProps(0, &supported, 0), Ok(()));
}

#[test]
/// Go intentionally descends only through ScalarFunction nodes; constants and
/// other expression kinds have no scalar arguments to validate.
fn non_scalar_expression_does_not_descend_into_children() {
    let expression = Expr {
        name: None,
        required: 0,
        children: vec![Expr {
            name: Some("setvar"),
            required: 1,
            children: vec![],
        }],
    };
    assert_eq!(checkExprWithProvidedProps(0, &expression, 0), Ok(()));
}
