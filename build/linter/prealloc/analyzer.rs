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

// 本文件由 build/linter/prealloc/analyzer.go 迁移而来，保留配置、逐 AST 文件检查和诊断上报流程。
// Go package: prealloc。
//
// Go imports:
// - go/ast
// - github.com/golangci/prealloc
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// Settings is the settings for preallocation.
// mapstructure 标签无法直接迁移，保留在字段注释里说明 Go 配置键。
pub struct Settings {
    pub Simple: bool,
    // mapstructure:"range-loops"
    pub RangeLoops: bool,
    // mapstructure:"range-loops"
    pub ForLoops: bool,
}

// Name is the name of the analyzer.
pub const Name: &str = "prealloc";

// Analyzer is the analyzer struct of prealloc.
// Go 的 Run 字段指向本文件 run 函数；Rust 保留同一入口。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: Name,
    doc: "Finds slice declarations that could potentially be preallocated",
    requires: &[],
    run,
};

// run 对应 Go 的 analyzer Run：构造默认配置，逐个 AST 文件调用 prealloc.Check。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    let s = Settings {
        Simple: true,
        RangeLoops: true,
        ForLoops: false,
    };
    // 这组默认值直接对应 Go 侧建议的“简单切片和 range 循环优先预分配”策略。
    for index in 0..pass.Files.len() {
        // Go 这里把单个 *ast.File 包成切片传入外部 prealloc.Check；保持相同调用粒度。
        let hints = {
            let f = &pass.Files[index];
            prealloc::Check(std::slice::from_ref(f), s.Simple, s.RangeLoops, s.ForLoops)
        };
        for hint in hints {
            pass.Reportf(
                hint.Pos,
                &format!(
                    "[{}] Consider preallocating {}",
                    Name,
                    util::FormatCode(&hint.DeclaredSliceName)
                ),
            );
        }
    }

    Ok(None)
}

// init 对应 Go 的包初始化：根据配置跳过 prealloc analyzer，并加入通用跳过集合。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}
