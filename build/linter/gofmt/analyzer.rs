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

// 本文件由 build/linter/gofmt/analyzer.go 迁移而来：收集 analysis.Pass 中的文件、
// 调用 gofmt 重写检查并报告 diff。
// Go package: gofmt。
//
// Go imports:
// - fmt
// - strings
// - github.com/golangci/gofmt/gofmt
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// Analyzer 对应 Go 中导出的 analysis.Analyzer 指针，保持名称、文档和 Run 回调的注册形状。
// Go 里 Doc 由两个字符串拼接而成，这里用 concat! 保留同样的文案组合。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "gofmt",
    doc: concat!(
        "gofmt checks whether code was gofmt-ed",
        "this tool runs with -s option to check for code simplification"
    ),
    requires: &[],
    run,
};

// needSimplify 对应 Go 的 bool 零值；init 注册 flag 时把运行默认值设为 true。
static mut needSimplify: bool = false;

// Go 的两个 init 按源码顺序执行；Rust 用一个入口同时完成 flag 注册和配置跳过接线。
pub fn init() {
    Analyzer.Flags.BoolVar(
        unsafe { &mut needSimplify },
        "need-simplify",
        true,
        "run gofmt with -s for code simplification",
    );
    util::SkipAnalyzerByConfig(&Analyzer);
}

// run 对应 Go 的 analyzer 执行函数。
// 收集待检查文件、构造 interface{} -> any 的 rewrite rule，然后报告 gofmt diff。
pub fn run(pass: &mut analysis::Pass) -> anyhow::Result<Option<Box<dyn std::any::Any>>> {
    let mut fileNames: Vec<String> = Vec::with_capacity(10);
    for f in &pass.Files {
        let pos = pass.Fset.PositionFor(f.Pos(), false);
        // Go 会过滤空文件名以及 failpoint 绑定生成文件，避免对自动生成的绑定文件报格式化差异。
        if !pos.Filename.is_empty() && !pos.Filename.ends_with("failpoint_binding__.go") {
            fileNames.push(pos.Filename);
        }
    }

    let rules = vec![gofmt::RewriteRule {
        Pattern: "interface{}",
        Replacement: "any",
    }];
    // 这条 rewrite 规则对应 Go 版的 any 迁移检查，确保格式化时顺便统一旧接口写法。
    for f in fileNames {
        let diff = gofmt::RunRewrite(&f, unsafe { needSimplify }, &rules).map_err(|err| {
            // Go 的 %w 允许 errors.Is/As 继续看到 RunRewrite 的原始错误。
            anyhow::Error::new(err).context(format!("could not run gofmt ({f})"))
        })?;

        if let Some(diff) = diff {
            pass.Report(analysis::Diagnostic {
                Pos: 1,
                Message: format!("\n{}", diff),
            });
        }
    }

    Ok(None)
}
