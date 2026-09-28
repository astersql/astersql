// Copyright 2026 AsterSQL.
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

// 本文件由 build/linter/unconvert/analysis.go 迁移而来，保留 unconvert analyzer 的遍历和诊断语义。
// Go package: unconvert。
//
// Go imports:
// - go/ast
// - go/token
// - go/types
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis
// - golang.org/x/tools/go/analysis/passes/inspect
// - golang.org/x/tools/go/ast/inspector

// Name is the name of the analyzer.
pub const Name: &str = "unconvert";

// Analyzer 对应 Go 的 analysis.Analyzer，保留 Requires inspect.Analyzer 和 Run=run。
// 它只关心“看起来像类型转换”的调用表达式，不尝试审计所有函数调用。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: Name,
    doc: "Remove unnecessary type conversions",
    requires: &[&inspect::Analyzer],
    run,
};

// init 对应 Go init：先按配置跳过文件，再接入 lint/nolint 指令过滤。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}

// Adapted from https://github.com/mdempsky/unconvert/blob/beb68d938016d2dec1d1b078054f4d3db25f97be/unconvert.go#L371-L414.
// run 对应 Go 的 analyzer 入口：只预序遍历 CallExpr，并检查单参数类型转换是否冗余。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    let inspect = pass
        .ResultOf
        .get(&inspect::Analyzer)
        .and_then(|v| v.downcast_ref::<inspector::Inspector>())
        .expect("inspect.Analyzer result should be an inspector.Inspector");

    let nodeFilter = vec![ast::Node::CallExpr(ast::CallExpr::default())];
    inspect.Preorder(nodeFilter, |n: ast::Node| {
        let call = match n {
            ast::Node::CallExpr(call) => call,
            _ => return,
        };
        // 只有单参数、非可变参数展开的调用，才可能是 Go 语义里的简单类型转换。
        if call.Args.len() != 1 || call.Ellipsis != token::NoPos {
            return;
        }

        let Some(ft) = pass.TypesInfo.Types.get(&call.Fun) else {
            // 缺少类型信息时沿用 Go 实现直接报缺失，而不是静默忽略。
            pass.Reportf(call.Pos(), "missing type");
            return;
        };
        if !ft.IsType() {
            // Function call; not a conversion.
            return;
        }

        let Some(at) = pass.TypesInfo.Types.get(&call.Args[0]) else {
            // 实参类型缺失时同样无法区分“类型转换”还是普通调用，直接维持 Go 的报错路径。
            pass.Reportf(call.Pos(), "missing type");
            return;
        };
        // 目标类型和实参类型不相同时，这就是一次真实转换，不属于本规则的处理对象。
        if !types::Identical(&ft.Type, &at.Type) {
            // A real conversion.
            return;
        }
        if isUntypedValue(&call.Args[0], &pass.TypesInfo) {
            // untyped 常量经常依赖上下文决定最终类型，不能简单按“类型相同”判成冗余转换。
            // Workaround golang.org/issue/13061.
            return;
        }

        // Adapted from https://github.com/mdempsky/unconvert/blob/beb68d938016d2dec1d1b078054f4d3db25f97be/unconvert.go#L416-L430.
        //
        // cmd/cgo generates explicit type conversions that
        // are often redundant when introducing
        // _cgoCheckPointer calls (issue #16).  Users can't do
        // anything about these, so skip over them.
        if let Some(ident) = call.Fun.as_ident() {
            if ident.Name == "_cgoCheckPointer" {
                // 这类调用来自工具链自动生成代码，继续报错只会制造用户无法修复的噪声。
                return;
            }
        }

        pass.Reportf(call.Pos(), "unnecessary conversion");
    });

    Ok(None)
}

// Cribbed from https://github.com/mdempsky/unconvert/blob/beb68d938016d2dec1d1b078054f4d3db25f97be/unconvert.go#L557-L607.
// isUntypedValue 对应 Go 的递归判定：判断表达式是否仍是 Go 的 untyped value。
pub fn isUntypedValue(n: &ast::Expr, info: &types::Info) -> bool {
    match n {
        ast::Expr::BinaryExpr(n) => match n.Op {
            token::SHL | token::SHR => {
                // Shifts yield an untyped value if their LHS is untyped.
                // Go 的移位表达式把“是否 untyped”主要绑定在左值上，因此这里只递归看 X。
                isUntypedValue(&n.X, info)
            }
            token::EQL | token::NEQ | token::LSS | token::GTR | token::LEQ | token::GEQ => {
                // Comparisons yield an untyped boolean value.
                true
            }
            token::ADD
            | token::SUB
            | token::MUL
            | token::QUO
            | token::REM
            | token::AND
            | token::OR
            | token::XOR
            | token::AND_NOT
            | token::LAND
            // 双目运算只有在两侧都保持 untyped 时，结果才继续算 untyped。
            | token::LOR => isUntypedValue(&n.X, info) && isUntypedValue(&n.Y, info),
            _ => false,
        },
        ast::Expr::UnaryExpr(n) => match n.Op {
            // 一元正负号、按位非和逻辑非都不会给 untyped 值“强制落型”。
            token::ADD | token::SUB | token::NOT | token::XOR => isUntypedValue(&n.X, info),
            _ => false,
        },
        ast::Expr::BasicLit(_) => {
            // Basic literals are always untyped.
            // 这一条让字面量包出来的转换在进入上下文前都能保留 Go 的原始推导语义。
            true
        }
        ast::Expr::ParenExpr(n) => isUntypedValue(&n.X, info),
        // 选择器表达式是否 untyped 取决于最终选中的标识符，而不是前缀对象本身。
        ast::Expr::SelectorExpr(n) => isUntypedValue(&n.Sel, info),
        ast::Expr::Ident(n) => {
            if let Some(obj) = info.Uses.get(n) {
                if obj.Pkg().is_none() && obj.Name() == "nil" {
                    // The universal untyped zero value.
                    return true;
                }
                if let Some(b) = obj.Type().as_basic() {
                    if b.Info() & types::IsUntyped != 0 {
                        // Reference to an untyped constant.
                        // 例如 iota 派生常量仍会走到这个分支。
                        return true;
                    }
                }
            }
            false
        }
        ast::Expr::CallExpr(n) => {
            if let Some(b) = asBuiltin(&n.Fun, info) {
                match b.Name() {
                    // real/imag/complex 这几个内建函数在 Go 规范里会保留 untyped 传播行为。
                    "real" | "imag" => return isUntypedValue(&n.Args[0], info),
                    "complex" => {
                        return isUntypedValue(&n.Args[0], info)
                            && isUntypedValue(&n.Args[1], info);
                    }
                    _ => {}
                }
            }
            // 其它普通调用一旦发生，就已经由签名把值落到了具体类型上。
            false
        }
        _ => false,
    }
}

// Cribbed from https://github.com/mdempsky/unconvert/blob/beb68d938016d2dec1d1b078054f4d3db25f97be/unconvert.go#L609-L630.
// asBuiltin 对应 Go 的辅助函数：剥掉括号表达式后识别内建函数对象。
pub fn asBuiltin<'a>(mut n: &ast::Expr, info: &'a types::Info) -> Option<&'a types::Builtin> {
    loop {
        let Some(paren) = n.as_paren_expr() else {
            break;
        };
        // 连续括号不会改变“是不是内建函数”的语义，所以先整体剥掉。
        n = &paren.X;
    }

    let Some(ident) = n.as_ident() else {
        return None;
    };

    // 只有名字解析到了 types.Builtin，调用方才会把它当成需要特殊处理的内建函数。
    let obj = info.Uses.get(ident)?;
    // 这里不关心普通函数或变量对象，它们都会让 as_builtin 失败并回到“普通调用”路径。
    obj.as_builtin()
}
