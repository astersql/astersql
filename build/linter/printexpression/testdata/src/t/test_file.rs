// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 本文件由 build/linter/printexpression/testdata/src/t/test_file.go 迁移而来，保留跨包表达式打印 fixture。
// 这是 printexpression analyzer 的测试数据，调用形状与 Go analysistest fixture 一致。
// Go package: t。
//
// Go imports:
// - fmt
// - github.com/pingcap/tidb/pkg/expression

// EmbeddedConstant is a test struct
// EmbeddedConstant 对应 Go 中匿名嵌入 expression.Constant 的结构体。
pub struct EmbeddedConstant {
    pub Constant: expression::Constant,
}

impl EmbeddedConstant {
    // String implements ExpressionWithString interface.
    // 嵌入 Constant 但自己提供 String，因此 printexpression analyzer 应放行直接打印。
    pub fn String(&self) {}
}

// ExpressionWithString is a test struct
// ExpressionWithString 对应 Go interface：组合 expression.Expression 和 String 方法。
pub trait ExpressionWithString: expression::Expression {
    // 这个额外方法就是“允许直接打印”的充分条件，用来和纯 Expression 做对照。
    fn String(&self);
}

// testFunc 保留跨包 fixture 中允许和禁止直接打印的案例。
fn testFunc() {
    let expr: Option<&dyn expression::Expression> = None;
    let con: Option<Box<expression::Constant>> = None;
    let conList: Vec<Box<expression::Constant>> = Vec::new();

    let exprWithString: Option<&dyn ExpressionWithString> = None;
    let col: Option<Box<expression::Column>> = None;
    let embedded: Option<Box<EmbeddedConstant>> = None;

    // expression.Expression 只有 StringWithCtx，直接打印应触发 analyzer 诊断。
    fmt::Println(expr); // want `avoid printing expression directly.*`
    // *expression.Constant 递归到 Constant 后仍只有 StringWithCtx，因此也应报错。
    fmt::Println(con); // want `avoid printing expression directly.*`
    // []*expression.Constant 的元素类型被 elementType 拆解后同样应报错。
    fmt::Printf("%v", conList); // want `avoid printing expression directly.*`
    // 以下类型拥有或约束了 String 方法，Go analyzer 应放行。
    fmt::Println(exprWithString);
    fmt::Println(col);
    fmt::Println(embedded);
}
