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

// 生成列表达式（generated expression）解析的 Go 对齐单元测试。
//
// 对应 Go `TestParseExpression`：经 `ParseExpression` 包装 SELECT 解析
// `json_extract` 调用，并断言函数名小写形式（`CIStr.L`）。

use crate::{ParseExpression, ast};

// test_parse_expression 对应 Go 的 TestParseExpression：解析 json_extract 表达式并检查函数名小写形式。
/// 解析 `json_extract(a, '$.a')`，确认 AST 为函数调用且 `FnName.L` 为小写。
#[test]
fn test_parse_expression() {
    crate::main_test::setup_for_common_test();
    let node = ParseExpression("json_extract(a, '$.a')").expect("ParseExpression should succeed");

    let ast::ExprKind::Function { FnName, .. } = node.Kind else {
        panic!("parsed node should be FuncCallExpr");
    };
    assert_eq!("json_extract", FnName.L);
}
