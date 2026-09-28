// Copyright 2023 PingCAP, Inc.
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

// 本文件由 build/linter/deferrecover/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 recover analyzer 如何扫描 Go AST，检查 util.Recover 是否直接位于 defer 语句中。
// 当前 Rust 草稿不会运行 go/analysis，不会遍历真实 AST，也不会向用户源码写入任何修改。
// Go package: deferrecover。
//
// Go imports:
// - go/ast
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis
// - golang.org/x/tools/go/analysis/passes/inspect
// - golang.org/x/tools/go/ast/inspector

// Analyzer is the analyzer struct of unconvert.
// 对应 Go 的 &analysis.Analyzer 字面量；Name/Doc/Requires/Run 的顺序保持原文件结构。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "recover",
    doc: "Check Recover() is directly called by defer",
    // Go 依赖 inspect.Analyzer 提供 AST 遍历能力；这里保留依赖声明形状，实际模块接线留给后续任务。
    requires: &[&inspect::Analyzer],
    run,
};

// 以下常量对应 Go const 块，用于定位 github.com/pingcap/tidb/pkg/util.Recover。
pub const packagePath: &str = "github.com/pingcap/tidb/pkg/util";
pub const packageName: &str = "util";
pub const funcName: &str = "Recover";

// run 对应 Go analyzer 的 Run 函数。
// 它逐个检查 pass.Files，只有文件 import 了目标 util 包时才继续扫描，避免误报其它同名 Recover。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    for file in &pass.Files {
        // 这里沿用 Go 的 import 别名解析，允许 util 包被重命名后仍按真实导入名比对。
        let packageName = util::GetPackageName(&file.Imports, packagePath, packageName);
        if packageName.is_empty() {
            // Go 的 continue：没有导入目标包时跳过该文件。
            continue;
        }

        // Go 代码为每个文件创建 inspector.New([]*ast.File{file})；这里保持单文件扫描范围。
        let mut i = inspector::New(vec![file]);

        i.WithStack(
            vec![ast::Node::CallExpr],
            |n: ast::Node, push: bool, stack: Vec<ast::Node>| {
                if !push {
                    // inspector 在离开节点时也会回调；Go 代码仅在进入节点时检查。
                    return true;
                }

                // WithStack 的节点过滤保证这里只会收到 CallExpr；Go 的直接类型断言失败会 panic。
                let callExpr = n
                    .as_call_expr()
                    .expect("inspector must yield a call expression");
                let Some(sel) = callExpr.Fun.as_selector_expr() else {
                    return true;
                };
                let Some(usedPackage) = sel.X.as_ident() else {
                    return true;
                };
                if usedPackage.Name != packageName {
                    return true;
                }
                // Go AST 的 SelectorExpr.Sel 是必填字段；显式 expect 保留该结构不变量。
                let selected = sel
                    .Sel
                    .as_ref()
                    .expect("selector expression must have an identifier");
                if selected.Name != funcName {
                    return true;
                }

                // check defer is directly called
                // Go 使用 stack[len(stack)-2] 取调用表达式的父节点；这里保留相同的“直接父节点必须是 defer”语义。
                // 因而像匿名函数内再调用 Recover 这类间接包装不会被放过，和原规则的误用定义保持一致。
                let parentStmt = &stack[stack.len() - 2];
                if !parentStmt.is_defer_stmt() {
                    pass.Reportf(n.Pos(), "Recover() should be directly called by defer");
                    return true;
                }
                true
            },
        );
    }

    Ok(None)
}

// init 对应 Go 的 init 函数，按配置跳过该 analyzer。
pub fn init() {
    // 该调用只保留注册期配置过滤语义；Rust 草稿不会实际读取 linter 配置。
    util::SkipAnalyzerByConfig(&Analyzer);
}
