// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 本文件由 build/linter/printexpression/analyzer.go 迁移而来，保留 Go 实现结构和诊断语义。
// 它遍历调用表达式，并禁止直接打印只实现 StringWithCtx 的表达式。
// Go package: printexpression。
//
// Go imports:
// - go/ast
// - go/types
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis
// - golang.org/x/tools/go/analysis/passes/inspect
// - golang.org/x/tools/go/ast/inspector

// Analyzer defines the linter for `emptynil` check
//
// This linter avoids calling `fmt.Println(expr)` or `fmt.Printf(\"%s\", expr)` directly, because
// `Expression` doesn't implement `String()` method, so it will print the address or internal state
// of the expression.
// It handles the following function call:
// 1. `fmt.Println(expr)`
// 2. `fmt.Printf(\"%s\", expr)`
// 4. `fmt.Sprintf(\"%s\", expr)`
// 5. `(*Error).GenWithStack/GenWithStackByArgs/FastGen/FastGenByArgs`
//
// Every struct which implemented `StringWithCtx` but not implemented `String` cannot be used as an argument.
// Analyzer 对应 Go 的 analysis.Analyzer 字面量，保留 Name/Doc/Requires/Run 字段含义。
// 整个规则的核心不是“禁止 fmt 包”，而是禁止把缺少 String 的表达式对象交给通用格式化入口。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "printexpression",
    doc: "Avoid printing expression directly.",
    requires: &[&inspect::Analyzer],
    run: run,
};

// run 对应 Go 的 Run 回调：从 inspect pass 里取 Inspector，只遍历 CallExpr。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    let inspect = pass
        .ResultOf
        .get(&inspect::Analyzer)
        .and_then(|value| value.downcast_ref::<inspector::Inspector>())
        .expect("Go 代码中这里依赖 analysis.Requires，缺失时会 panic");

    let node_filter: Vec<ast::Node> = vec![ast::Node::CallExpr(ast::CallExpr::default())];
    inspect.Preorder(node_filter, |n: ast::Node| {
        // Go 通过 n.(*ast.CallExpr) 做类型断言；Rust 用模式匹配表达同一过滤。
        let expr = match n {
            ast::Node::CallExpr(expr) => expr,
            _ => return,
        };

        if !funcIsFormat(&expr.Fun) {
            return;
        }

        // 一旦命中目标打印函数，所有实参都会按同一条类型规则逐个审查。
        for arg in &expr.Args {
            if argIsNotAllowed(&pass.TypesInfo, arg) {
                pass.Reportf(
                    arg.Pos(),
                    "avoid printing expression directly. Please use `Expression.StringWithCtx()` to get a string",
                );
            }
        }
    });

    Ok(None)
}

// funcIsFormat 对应 Go 的同名函数：识别 fmt 打印函数以及 errors 包的若干生成函数。
pub fn funcIsFormat(x: &ast::Expr) -> bool {
    match x {
        ast::Expr::SelectorExpr(selector) => match selector.Sel.Name.as_str() {
            "Printf" | "Sprintf" | "Println" => {
                // Go 这里要求选择器左侧是标识符 fmt，避免把其他包或对象的同名方法误报。
                if let ast::Expr::Ident(i) = selector.X.as_ref() {
                    return i.Name == "fmt";
                }
                false
            }
            "GenWithStack" | "GenWithStackByArgs" | "FastGen" | "FastGenByArgs" => {
                // Go 原注释：后续可检查 receiver 是否为 *Error；当前迁移保持放行规则。
                true
            }
            _ => false,
        },
        _ => false,
    }
}

// argIsNotAllowed 对应 Go 的参数类型检查：缺失类型信息时直接不报告。
pub fn argIsNotAllowed(typInfo: &types::Info, x: &ast::Expr) -> bool {
    let typ = match typInfo.Types.get(x).and_then(|value| value.Type.as_ref()) {
        Some(typ) => typ,
        None => return false,
    };

    // Go 使用 elementType(typ).(methodLookup) 类型断言；Rust 把接口约束保留为 dyn trait。
    let typ_with_methods = match elementType(typ.as_ref()).as_method_lookup() {
        Some(typ) => typ,
        None => return false,
    };

    typIsNotAllowed(typ_with_methods)
}

// methodLookup 对应 Go 的局部 interface，只要求能枚举方法并查看底层类型。
pub trait methodLookup: types::Type {
    // 这里抽象出最小方法集，是为了同时兼容命名类型和 interface 类型的递归检查。
    fn NumMethods(&self) -> usize;
    fn Method(&self, i: usize) -> &types::Func;
    fn Underlying(&self) -> &dyn types::Type;
}

// typIsNotAllowed 对应 Go 的核心判定：实现 StringWithCtx 但未实现 String 时禁止打印。
pub fn typIsNotAllowed(typ: &dyn methodLookup) -> bool {
    let mut implString = false;
    let mut implStringWithCtx = false;
    for i in 0..typ.NumMethods() {
        let name = typ.Method(i).Name();
        if name == "String" {
            implString = true;
        }
        if name == "StringWithCtx" {
            implStringWithCtx = true;
        }
    }

    if implStringWithCtx && !implString {
        // 命中这个条件时就不需要继续向下看底层类型了，当前方法集已经足以判定违规。
        return true;
    }

    // Go 中 interface 的 Underlying 仍是自己；这里保留“非 interface 才递归到底层 interface”的防无限递归逻辑。
    let typ_is_iface = typ.as_any().is::<types::Interface>();
    if let Some(iface) = typ.Underlying().as_any().downcast_ref::<types::Interface>() {
        if !typ_is_iface {
            return typIsNotAllowed(iface);
        }
    }

    false
}

// elementType returns the element type of a pointer or slice recursively.
// elementType 对应 Go 的递归拆指针/切片元素类型，直到遇到普通类型为止。
pub fn elementType(typ: &dyn types::Type) -> &dyn types::Type {
    match typ.kind() {
        // 指针和切片都要递归拆到底，因为 Go 打印时最终暴露的是元素类型的方法集。
        types::Kind::Pointer(pointer) => elementType(pointer.Elem()),
        types::Kind::Slice(slice) => elementType(slice.Elem()),
        _ => typ,
    }
}

// init 对应 Go 的 init 函数：按配置和全局跳过规则登记 analyzer。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}
