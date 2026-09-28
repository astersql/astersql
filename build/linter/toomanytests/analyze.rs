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

// 本文件由 build/linter/toomanytests/analyze.go 迁移而来，统计包内顶层测试函数数量。
// Go package: toomanytests。
//
// Go imports:
// - go/ast
// - go/token
// - path/filepath
// - strings
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// Analyzer 对应 Go 的 analysis.Analyzer 全局变量。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "toomanytests",
    doc: "too many tests in the package",
    requires: &[],
    run,
};

// run 保留逐文件扫描、测试文件过滤和超限 Reportf 的控制流。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    let mut cnt = 0;
    let mut pos = token::Pos::default();

    for f in &pass.Files {
        let astFile = pass
            .Fset
            .File(f.Pos())
            .expect("AST file position must belong to the pass FileSet");
        if !isTestFile(astFile) {
            continue;
        }

        // Go 只统计顶层、无接收者、以 Test 开头且不是 TestMain 的函数声明。
        for n in &f.Decls {
            if let Some(funcDecl) = n.as_func_decl() {
                if strings::HasPrefix(&funcDecl.Name.Name, "Test")
                    && funcDecl.Recv.is_none()
                    && funcDecl.Name.Name != "TestMain"
                {
                    cnt += 1;
                }
            }
        }
        pos = f.Pos();
    }

    // Go 用最后一个测试文件的位置计算包目录；没有测试文件时 token.Pos 零值导出空文件名。
    let pkgName = filepath::Dir(&pass.Fset.Position(pos).Filename);
    if cnt > checkRule(&pkgName) {
        pass.Reportf(
            pos,
            &format!("{}: Too many test cases in one package: {}", pkgName, cnt),
        );
    }
    Ok(None)
}

// isTestFile 对应 Go 的同名辅助函数：仅通过文件名后缀判断测试文件。
pub fn isTestFile(file: &token::File) -> bool {
    strings::HasSuffix(file.Name(), "_test.go")
}

// checkRule 对应 Go 的包级阈值规则；特殊包保留更高测试数量上限。
pub fn checkRule(pkg: &str) -> i32 {
    match pkg {
        "pkg/planner/core" => 210,
        "pkg/util/topsql/reporter" => {
            // TopRU has generated_cases + multi-scenario tests
            90
        }
        _ => 50,
    }
}

// init 对应 Go init：给 analyzer 叠加配置跳过和 lint 指令跳过逻辑。
pub fn init() {
    // 这样包级测试数量规则也能尊重 `exclude_files` 和 `nolint` 这两层过滤。
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}
