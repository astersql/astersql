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

// 本文件由 build/linter/revive/analyzer.go 迁移而来：收集当前 package 文件、配置 revive 规则、
// 格式化 JSON 结果并回报诊断。
// Go package: revive。
//
// Go imports:
// - encoding/json
// - go/token
// - os
// - github.com/hashicorp/go-version
// - github.com/mgechev/revive/config
// - github.com/mgechev/revive/lint
// - github.com/mgechev/revive/rule
// - github.com/pingcap/log
// - github.com/pingcap/tidb/build/linter/util
// - go.uber.org/zap
// - golang.org/x/tools/go/analysis

// Analyzer is the analyzer struct of gofmt.
// Analyzer 对应 Go 的 analysis.Analyzer 字面量，Run 字段绑定到本文件的 run。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "revive",
    doc: "~6x faster, stricter, configurable, extensible, and beautiful drop-in replacement for golint",
    requires: &[],
    run,
};

// init 对应 Go 的 init 函数：注册跳过配置和默认跳过规则。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}

// jsonObject defines a JSON object of a failure
// jsonObject 对应 Go 中嵌入 lint.Failure 的 JSON 输出结构。
pub struct jsonObject {
    pub Severity: lint::Severity,
    // Go 使用 `json:",inline"` 内联 Failure 字段；Rust 保留同一字段关系。
    pub Failure: lint::Failure,
}

// defaultRules 对应 Go 的默认 revive 规则列表；被注释的规则保持为注释，避免改变启用集合。
pub fn defaultRules() -> Vec<Box<dyn lint::Rule>> {
    // 默认规则承担“总是开启”的基础 lint 语义，额外规则会在 allRules 中叠加。
    vec![
        Box::new(rule::VarDeclarationsRule {}),
        // Box::new(rule::PackageCommentsRule {}),
        Box::new(rule::DotImportsRule {}),
        Box::new(rule::ExportedRule {}),
        // Box::new(rule::VarNamingRule {}),
        Box::new(rule::IncrementDecrementRule {}),
        // Box::new(rule::UnexportedReturnRule {}),
        Box::new(rule::ContextKeysType {}),
    ]
}

// allRules 对应 Go 的 append([]lint.Rule{...}, defaultRules...)。
// 这里先放额外规则，再追加 defaultRules，保持 Go 中配置覆盖顺序。
pub fn allRules() -> Vec<Box<dyn lint::Rule>> {
    let mut rules: Vec<Box<dyn lint::Rule>> = vec![
        // Box::new(rule::ArgumentsLimitRule {}),
        // Box::new(rule::CyclomaticRule {}),
        // Box::new(rule::FileHeaderRule {}),
        Box::new(rule::EmptyBlockRule {}),
        // Box::new(rule::ConfusingNamingRule {}),
        Box::new(rule::ConfusingResultsRule {}),
        // Box::new(rule::DeepExitRule {}),
        Box::new(rule::UnusedParamRule {}),
        // Box::new(rule::AddConstantRule {}),
        // Box::new(rule::FlagParamRule {}),
        Box::new(rule::UnnecessaryStmtRule {}),
        // Box::new(rule::StructTagRule {}),
        // Box::new(rule::ModifiesValRecRule {}),
        // Box::new(rule::RedefinesBuiltinIDRule {}),
        // Box::new(rule::FunctionResultsLimitRule {}),
        // Box::new(rule::MaxPublicStructsRule {}),
        // Box::new(rule::LineLengthLimitRule {}),
        Box::new(rule::CallToGCRule {}),
        // Box::new(rule::ImportShadowingRule {}),
        // Box::new(rule::BareReturnRule {}),
        Box::new(rule::UnusedReceiverRule {}),
        // Box::new(rule::UnhandledErrorRule {}),
        // Box::new(rule::CognitiveComplexityRule {}),
        // Box::new(rule::EarlyReturnRule {}),
        Box::new(rule::UnexportedNamingRule {}),
        // Box::new(rule::FunctionLength {}),
        // Box::new(rule::NestedStructs {}),
        Box::new(rule::UselessBreak {}),
        // Box::new(rule::BannedCharsRule {}),
    ];
    // 先构建扩展规则再 extend 默认规则，等价于 Go 版 append(extra, default...) 的最终结果。
    rules.extend(defaultRules());
    rules
}

// run 对应 Go 的 revive analyzer 主流程。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    let mut files: Vec<String> = Vec::with_capacity(pass.Files.len());
    for file in &pass.Files {
        // Go 通过 pass.Fset.PositionFor(file.Pos(), false).Filename 把 AST 文件转成磁盘路径。
        files.push(pass.Fset.PositionFor(file.Pos(), false).Filename);
    }
    let packages: Vec<Vec<String>> = vec![files];

    let gv = match goversion::NewVersion("1.21") {
        Ok(version) => version,
        Err(err) => panic!("{err}"),
    };
    let revive = lint::New(os::ReadFile, 1024);
    let mut conf = lint::Config {
        GoVersion: gv,
        IgnoreGeneratedHeader: false,
        Confidence: 0.8,
        Severity: "error".to_string(),
        ErrorCode: -1,
        WarningCode: -1,
        Rules: std::collections::HashMap::new(),
    };
    for r in allRules() {
        conf.Rules.insert(r.Name(), lint::RuleConfig {});
    }
    conf.Rules.insert(
        "defer".to_string(),
        lint::RuleConfig {
            Arguments: vec![vec!["loop", "method-call", "immediate-recover", "return"]],
        },
    );

    let lintingRules = config::GetLintingRules(&conf, vec![])?;
    let failures = revive.Lint(packages, lintingRules, conf.clone())?;

    let (format_tx, format_rx) = std::sync::mpsc::channel::<lint::Failure>();
    let formatter = config::GetFormatter("json")?;
    let confidence = conf.Confidence;
    let formatter_conf = conf.clone();
    let formatter_thread = std::thread::spawn(move || {
        // 与 Go goroutine 相同，formatter 在发送方关闭 channel 后才完成并交回完整 JSON。
        match formatter.Format(format_rx, formatter_conf) {
            Ok(text) => text,
            Err(err) => {
                log::Error("Format error", zap::Error(err));
                String::new()
            }
        }
    });

    for f in failures {
        // revive 可能给出低置信度 failure；Go 原实现低于阈值时不发送给 formatter。
        if f.Confidence < confidence {
            continue;
        }

        // 只有进入 channel 的 failure 才会出现在最终 JSON 结果里。
        format_tx
            .send(f)
            .expect("revive formatter must receive failures until the channel closes");
    }

    drop(format_tx);
    let output = formatter_thread
        .join()
        .expect("revive formatter thread must complete");

    let results: Vec<jsonObject> = json::Unmarshal(output.as_bytes())?;
    for res in &results {
        // 这里重新读取源文件，是为了把 revive 的行列号重新映射回 analysis 使用的 token.Pos。
        let (fileContent, tf) =
            match util::ReadFile(&mut pass.Fset, &res.Failure.Position.Start.Filename) {
                Ok(value) => value,
                Err(err) => panic!("{err}"),
            };
        let fileText = sanitizeForOffset(&fileContent);
        let file_base = unsafe { (*tf).Base() };
        // Go 将 revive 的行列号转成 token.Pos 后 Reportf；FindOffset 的字节偏移语义保持不变。
        pass.Reportf(
            token::Pos(
                file_base
                    + util::FindOffset(
                        &fileText,
                        res.Failure.Position.Start.Line,
                        res.Failure.Position.Start.Column,
                    ),
            ),
            &format!("{}: {}", res.Failure.RuleName, res.Failure.Failure),
        );
    }
    Ok(None)
}

// Go string 的非法 UTF-8 rune 每次只消耗一个字节。逐字节替换为 NUL，既保持非文本边界，
// 又让 util::FindOffset 的 UTF-8 字节 offset 与原文件完全一致。
pub(super) fn sanitizeForOffset(fileContent: &[u8]) -> String {
    let mut sanitized = fileContent.to_vec();
    let mut offset = 0;
    while offset < sanitized.len() {
        match std::str::from_utf8(&sanitized[offset..]) {
            Ok(_) => break,
            Err(err) => {
                let invalid = offset + err.valid_up_to();
                sanitized[invalid] = b'\0';
                offset = invalid + 1;
            }
        }
    }
    String::from_utf8(sanitized).expect("invalid bytes were replaced one-for-one")
}
