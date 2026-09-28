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

// 本文件由 build/linter/printexpression/testdata/src/github.com/pingcap/tidb/pkg/expression/expression.go 迁移而来。
// 这是 printexpression analyzer 的测试数据，表达哪些类型实现 StringWithCtx/String。
// Go package: expression。
//
// Go imports:
// - fmt

// Expression is a test struct.
// Expression 对应 Go interface：只有 StringWithCtx，未提供 String 时直接打印应被 analyzer 拒绝。
pub trait Expression {
    // StringWithCtx method only is not allowed to be printed directly.
    // fixture 只关心方法集合本身，不关心返回值内容或真正的上下文对象。
    fn StringWithCtx(&self);
}

// Constant is a test struct.
// Constant 只实现 Expression，不实现 String；它和指针/切片形态都应触发 printexpression 诊断。
pub struct Constant;

impl Expression for Constant {
    // StringWithCtx implements the Expression interface.
    fn StringWithCtx(&self) {}
}

// Column is a test struct
// Column 同时实现 String 和 StringWithCtx，因此直接打印不应被该 analyzer 报错。
pub struct Column;

impl Column {
    // String implements ExpressionWithString interface.
    pub fn String(&self) {}
}

impl Expression for Column {
    // StringWithCtx implements the Expression interface.
    fn StringWithCtx(&self) {}
}

// testFunc 保留 Go fixture 的 fmt 打印调用和 want 预期诊断。
fn testFunc() {
    let expr: Option<&dyn Expression> = None;
    let con: Option<Box<Constant>> = None;
    let conList: Vec<Box<Constant>> = Vec::new();

    let col: Option<Box<Column>> = None;

    // Go 的 fmt.Println(expr) 会打印只实现 StringWithCtx 的 interface，应被报告。
    fmt::Println(expr); // want `avoid printing expression directly.*`
    // 指向 Constant 的指针仍会被 elementType 拆到 Constant，再发现缺少 String。
    fmt::Println(con); // want `avoid printing expression directly.*`
    // 切片元素类型也会递归拆解，因此 []*Constant 同样不允许直接格式化。
    fmt::Printf("%v", conList); // want `avoid printing expression directly.*`
    // Column 具有 String 方法，Go analyzer 应放行。
    fmt::Println(col);
}
