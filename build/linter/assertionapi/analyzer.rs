// Copyright 2026 PingCAP, Inc.
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

// 本文件由 build/linter/assertionapi/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 assertionapi analyzer 如何限制 kv.MemBuffer 的 UpdateAssertionFlags 使用位置。
// 当前 Rust 草稿不会真正遍历 Go AST、不会读取类型信息，也不会执行 lint；ast、types、analysis 等均为占位依赖。
// Go package: assertionapi。
//
// Go imports:
// - go/ast
// - go/types
// - path/filepath
// - strings
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis
// - golang.org/x/tools/go/analysis/passes/inspect

// Analyzer restricts assertion-related MemBuffer APIs to the table layer.
// Analyzer 对应 Go 的包级变量，保留 inspect.Analyzer 依赖和 run 入口。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "assertionapi",
    doc: "Restrict txn assertion API usage (UpdateAssertionFlags) to pkg/table/tables",
    requires: &[&inspect::Analyzer],
    run,
};

// kvPkgPath 对应 Go 常量，用于精确匹配 kv 包中的命名类型。
pub const kvPkgPath: &str = "github.com/pingcap/tidb/pkg/kv";

// run 对应 Go analyzer 主流程：跳过允许目录，其余文件中查找 SelectorExpr。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    for f in &pass.Files {
        let filename = pass.Fset.PositionFor(f.Pos(), false).Filename;
        // 这里先按路径白名单过滤，避免表层内部的合法用法被后续 AST 检查误报。
        if isAllowedFile(&filename) {
            continue;
        }

        ast::Inspect(f, |n: ast::Node| -> bool {
            // Go 先断言 *ast.SelectorExpr，再检查选择器名，避免误报其它同名标识符。
            let sel = match n.as_selector_expr() {
                Some(sel) => sel,
                None => return true,
            };
            let Some(ident) = sel.Sel.as_ref() else {
                return true;
            };
            if ident.Name != "UpdateAssertionFlags" {
                return true;
            }
            if !isKVUpdateAssertionFlags(pass, sel) {
                return true;
            }

            pass.Reportf(
                ident.Pos(),
                "txn assertion API (UpdateAssertionFlags) is restricted to pkg/table/tables",
            );
            true
        });
    }
    Ok(None)
}

// isAllowedFile 对应 Go 的路径白名单判断：统一成 slash 后只允许 pkg/table/tables。
pub fn isAllowedFile(filename: &str) -> bool {
    let f = filepath::ToSlash(filename);
    // 统一斜杠后再做包含判断，是为了兼容不同平台传入的路径分隔符。
    strings::Contains(&f, "pkg/table/tables/")
}

// isKVUpdateAssertionFlags 对应 Go 的类型信息匹配：确认调用目标是 kv.Key/kv.AssertionOp 签名。
pub fn isKVUpdateAssertionFlags(pass: &analysis::Pass, sel: &ast::SelectorExpr) -> bool {
    let Some(selection) = pass.TypesInfo.Selections.get(sel) else {
        // 没有类型信息时，Go 选择保守跳过，避免因为潜在无关标识符阻塞构建。
        return false;
    };
    let fn_obj = match selection.Obj().as_func() {
        Some(v) => v,
        None => return false,
    };
    let sig = match fn_obj.Type().as_signature() {
        Some(v) => v,
        None => return false,
    };

    // Match: UpdateAssertionFlags(kv.Key, kv.AssertionOp)
    //
    // Note: method expressions like `T.UpdateAssertionFlags` have an extra first parameter (the receiver),
    // so we match both forms.
    // Go 对方法表达式会多一个 receiver 参数，因此这里保留 2/3 参数两种索引。
    // 只靠方法名不足以判断违规；必须把参数类型也约束到 kv 包签名，才能排除同名 API。
    let params = sig.Params();
    let (keyIdx, opIdx) = match params.Len() {
        2 => (0, 1),
        3 => (1, 2),
        _ => return false,
    };
    if !isKVKey(params.At(keyIdx).Type()) {
        return false;
    }
    if !isKVAssertionOp(params.At(opIdx).Type()) {
        return false;
    }
    true
}

// isKVKey 对应 Go 的命名类型判断：类型名必须为 Key，包路径必须为 kvPkgPath。
pub fn isKVKey(t: types::Type) -> bool {
    let n = match t.as_named() {
        Some(v) => v,
        None => return false,
    };
    let Some(obj) = n.Obj() else {
        return false;
    };
    let Some(pkg) = obj.Pkg() else {
        return false;
    };
    obj.Name() == "Key" && pkg.Path() == kvPkgPath
}

// isKVAssertionOp 对应 Go 的命名类型判断：类型名必须为 AssertionOp，包路径必须为 kvPkgPath。
pub fn isKVAssertionOp(t: types::Type) -> bool {
    let n = match t.as_named() {
        Some(v) => v,
        None => return false,
    };
    let Some(obj) = n.Obj() else {
        return false;
    };
    let Some(pkg) = obj.Pkg() else {
        return false;
    };
    obj.Name() == "AssertionOp" && pkg.Path() == kvPkgPath
}

// init 对应 Go 的 init：同时支持配置跳过和通用跳过。
pub fn init() {
    // 同时注册两种跳过入口，保证该规则既能被显式配置关闭，也能被通用跳过逻辑托管。
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}
