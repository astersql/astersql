// Copyright 2022 PingCAP, Inc.
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

// 本文件由 build/linter/etcdconfig/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 etcdconfig analyzer 如何扫描 clientv3.Config 复合字面量并检查 AutoSyncInterval 字段。
// 当前 Rust 草稿不会运行 go/analysis，不会遍历真实 AST，也不会修改任何 etcd 配置。
// Go package: etcdconfig。
//
// Go imports:
// - go/ast
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis
// - golang.org/x/tools/go/analysis/passes/inspect

// Analyzer is the analyzer struct of unconvert.
// 对应 Go 的 &analysis.Analyzer 字面量，保持 Name、Doc、Requires、Run 字段顺序。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "etcdconfig",
    doc: "Check necessary fields of etcd config",
    // Go 依赖 inspect.Analyzer；虽然原 run 中直接 ast.Inspect decl，这里仍保留 Requires 声明。
    requires: &[&inspect::Analyzer],
    run,
};

// 以下常量对应 Go const 块，用于识别 go.etcd.io/etcd/client/v3.Config。
pub const configPackagePath: &str = "go.etcd.io/etcd/client/v3";
pub const configPackageName: &str = "clientv3";
pub const configStructName: &str = "Config";

// run 对应 Go analyzer 的 Run 函数。
// 它只检查导入了 clientv3 包的文件，并在每个声明内寻找 clientv3.Config 复合字面量。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    for file in &pass.Files {
        // 继续沿用 util.GetPackageName，是为了兼容 import clientv3 "go.etcd.io/etcd/client/v3" 这类别名写法。
        let packageName = util::GetPackageName(&file.Imports, configPackagePath, configPackageName);
        if packageName.is_empty() {
            // 没有导入目标 etcd 包时直接跳过，避免把其它 Config 误判为 etcd 配置。
            continue;
        }

        for decl in &file.Decls {
            ast::Inspect(decl, |n: ast::Node| {
                let Some(lit) = n.as_composite_lit() else {
                    return true;
                };
                let Some(tp) = lit.Type.as_selector_expr() else {
                    return true;
                };
                let Some(litPackage) = tp.X.as_ident() else {
                    return true;
                };
                if litPackage.Name != packageName {
                    return true;
                }
                // Go AST 要求 SelectorExpr.Sel 非空；expect 保留直接字段访问的结构不变量。
                let selected = tp
                    .Sel
                    .as_ref()
                    .expect("selector expression must have an identifier");
                if selected.Name != configStructName {
                    return true;
                }

                let mut found = false;
                for field in &lit.Elts {
                    let Some(kv) = field.as_key_value_expr() else {
                        // Go 对非 KeyValueExpr 字段 continue；保留同样的容错扫描。
                        continue;
                    };
                    let Some(key) = kv.Key.as_ident() else {
                        continue;
                    };
                    if key.Name == "AutoSyncInterval" {
                        found = true;
                        break;
                    }
                }
                if !found {
                    // Go 在复合字面量起点报告缺失字段；这里保留 Reportf 位置和消息文本。
                    // 报在字面量本身而不是字段列表末尾，能让调用者更快定位是哪一个 Config 初始化遗漏了默认同步周期。
                    pass.Reportf(lit.Pos(), "missing field AutoSyncInterval");
                }
                true
            });
        }
    }

    Ok(None)
}

// init 对应 Go 的 init 函数，按 linter 配置跳过该 analyzer。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
}
