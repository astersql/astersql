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

// 本文件由 build/linter/forbidigo/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 forbidigo analyzer 如何配置外部 linter、运行 AST 检查并过滤白名单；
// 当前不会真正接入 Go analysis 框架、不会打开仓库文件，也不会执行业务动作。
// Go package: forbidigo。
//
// Go imports:
// - fmt
// - go/ast
// - strings
// - github.com/ashanbrown/forbidigo/v2/forbidigo
// - github.com/golangci/golangci-lint/v2/pkg/fsutils
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// lc 对应 Go 的包级 LineCache，用来按文件名和行号读取源码行。
// Go 实现复用 golangci-lint 的文件缓存；Rust 草稿只保留外部依赖的构造关系。
pub static lc: once_cell::sync::Lazy<fsutils::LineCache> =
    once_cell::sync::Lazy::new(|| fsutils::NewLineCache(fsutils::NewFileCache()));

// Analyzer 对应 Go 的 analysis.Analyzer 字面量。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "forbidigo",
    doc: "forbid identifiers",
    requires: &[],
    run,
};

// patterns 对应 Go 的包级规则列表，保持 forbidigo 原始 DSL 字符串。
// 这里只收敛最敏感的 GetSessionVars 入口，把是否允许使用交给白名单和显式 nolint 决定。
pub static patterns: once_cell::sync::Lazy<Vec<String>> = once_cell::sync::Lazy::new(|| {
    vec![
        r#"{
        p: "sessionctx.Context.GetSessionVars",
        msg: "Please check if the usage of GetSessionVars is appropriate,\n
              you can add //nolint:forbidigo to ignore this check if necessary."
    }"#
        .to_string(),
    ]
});

// run 对应 Go 的 analyzer 回调：创建 forbidigo linter，收集 AST 节点并运行检查。
// Go 返回 (any, error)；anyhow 保留外部 linter 的错误链并用动态空结果表达 any(nil)。
pub fn run(pass: &mut analysis::Pass) -> anyhow::Result<Option<Box<dyn std::any::Any>>> {
    let linter = forbidigo::NewLinter(
        &patterns,
        vec![
            forbidigo::OptionIgnorePermitDirectives(true),
            forbidigo::OptionExcludeGodocExamples(false),
            forbidigo::OptionAnalyzeTypes(true),
        ],
    )
    .map_err(|err| anyhow::Error::new(err).context("failed to configure linter"))?;

    let mut nodes: Vec<ast::Node> = Vec::with_capacity(pass.Files.len());
    for f in &pass.Files {
        // Go 传入原 *ast.File；保留节点身份才能让 TypesInfo 中以 AST 节点为键的查询继续命中。
        nodes.push(f.as_node());
    }

    let config = forbidigo::RunConfig {
        Fset: &pass.Fset,
        DebugLog: None,
        TypesInfo: &pass.TypesInfo,
    };
    let issues = linter.RunWithConfig(config, nodes)?;
    reportIssues(pass, &issues);
    Ok(None)
}

// reportIssues 对应 Go 的诊断上报辅助函数。
// 它先按源码行过滤允许用法，再把剩余问题作为 restriction 类诊断报告给 analysis.Pass。
pub fn reportIssues(pass: &mut analysis::Pass, issues: &[forbidigo::Issue]) {
    // Go 的正则不支持 negative lookahead，所以这里沿用读取源码行再白名单过滤的策略。
    // 换句话说，forbidigo 先做“粗拦截”，而本函数负责补回 TiDB 内部允许的少数访问路径。
    let whiteLists = vec![
        "SQLMode",               // used for model.Job
        "CDCWriteSource",        // used for model.Job
        "StmtCtx",               // GetSessionVars().StmtCtx
        "TimeZone",              // GetSessionVars().TimeZone
        "Location",              // GetSessionVars().Location()
        "GetSplitRegionTimeout", // GetSessionVars().GetSplitRegionTimeout()
        "SetInTxn",              // GetSessionVars().SetInTxn(true)
        "BuildParserConfig",     // GetSessionVars().BuildParserConfig()
        "DiskFull",
        "DefaultCollationForUTF8MB4",
        "RowEncoder",
        "SetStatusFlag",
    ];

    for i in issues {
        let mut skip = false;
        // Copied from golanglint-ci
        if let Ok(s) = lc.GetLine(i.Position().Filename, i.Position().Line) {
            for whiteList in &whiteLists {
                if s.contains(whiteList) {
                    // 命中白名单后与 Go 一样停止继续比较，避免误报合法的 GetSessionVars 用法。
                    skip = true;
                    break;
                }
            }
        }
        // 读取源码行失败时不放宽规则，保持“宁可继续报告，也不要因为缓存缺失漏报”的保守策略。

        if !skip {
            pass.Report(analysis::Diagnostic {
                Pos: i.Pos(),
                Message: i.Details(),
                Category: "restriction",
            });
        }
    }
}

// init 对应 Go 的 init 函数：按配置跳过 forbidigo analyzer。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
}
