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

// 本文件由 build/linter/allrevive/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 allrevive 如何封装 revive 规则并把 JSON 格式化结果回报给 go/analysis。
// 当前 Rust 草稿不会真正读取 Go 包、不会启动 revive，也不会执行 lint；analysis、lint、rule 等均为占位依赖。
// Go package: allrevive。
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

// Analyzer 对应 Go 的包级 analyzer 变量，保留 revive 总入口的名称、说明和 Run 函数绑定。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "all_revive",
    doc: "~6x faster, stricter, configurable, extensible, and beautiful drop-in replacement for golint",
    run,
};

// init 对应 Go 的 init：按配置跳过 analyzer，并登记通用跳过逻辑。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}

// jsonObject 对应 Go 中 formatter JSON 输出的单项结构，内嵌 lint.Failure。
// 这一层中间结构把 revive formatter 的纯文本输出重新接回 analyzer 可消费的数据模型。
pub struct jsonObject {
    pub Severity: lint::Severity,
    pub Failure: lint::Failure,
}

// defaultRules 对应 Go 的默认 revive 规则列表，注释掉的规则保留为原 Go 配置意图。
// 这一组规则代表“始终启用的基础规则”，后面的 allRules 会在此基础上叠加更多检查。
pub fn defaultRules() -> Vec<Box<dyn lint::Rule>> {
    vec![
        // Box::new(rule::VarDeclarationsRule {}),
        // Box::new(rule::PackageCommentsRule {}),
        // Box::new(rule::DotImportsRule {}),
        Box::new(rule::BlankImportsRule {}),
        // Box::new(rule::ExportedRule {}),
        // Box::new(rule::VarNamingRule {}),
        Box::new(rule::IndentErrorFlowRule {}),
        Box::new(rule::RangeRule {}),
        Box::new(rule::ErrorfRule {}),
        Box::new(rule::ErrorNamingRule {}),
        Box::new(rule::ErrorStringsRule {}),
        Box::new(rule::ReceiverNamingRule {}),
        // Box::new(rule::IncrementDecrementRule {}),
        Box::new(rule::ErrorReturnRule {}),
        // Box::new(rule::UnexportedReturnRule {}),
        Box::new(rule::TimeNamingRule {}),
        // Box::new(rule::ContextKeysType {}),
        Box::new(rule::ContextAsArgumentRule {}),
    ]
}

// allRules 对应 Go 中 append(extraRules, defaultRules...) 后得到的完整规则集。
pub fn allRules() -> Vec<Box<dyn lint::Rule>> {
    let mut rules: Vec<Box<dyn lint::Rule>> = vec![
        // Box::new(rule::ArgumentsLimitRule {}),
        // Box::new(rule::CyclomaticRule {}),
        // Box::new(rule::FileHeaderRule {}),
        // Box::new(rule::EmptyBlockRule {}),
        Box::new(rule::SuperfluousElseRule {}),
        // Box::new(rule::ConfusingNamingRule {}),
        Box::new(rule::GetReturnRule {}),
        Box::new(rule::ModifiesParamRule {}),
        // Box::new(rule::ConfusingResultsRule {}),
        // Box::new(rule::DeepExitRule {}),
        // Box::new(rule::UnusedParamRule {}),
        Box::new(rule::UnreachableCodeRule {}),
        // Box::new(rule::AddConstantRule {}),
        // Box::new(rule::FlagParamRule {}),
        // Box::new(rule::UnnecessaryStmtRule {}),
        // Box::new(rule::StructTagRule {}),
        // Box::new(rule::ModifiesValRecRule {}),
        Box::new(rule::ConstantLogicalExprRule {}),
        Box::new(rule::BoolLiteralRule {}),
        // Box::new(rule::RedefinesBuiltinIDRule {}),
        Box::new(rule::BlankImportsRule {}),
        // Box::new(rule::FunctionResultsLimitRule {}),
        // Box::new(rule::MaxPublicStructsRule {}),
        // Box::new(rule::RangeValInClosureRule {}),
        // Box::new(rule::RangeValAddress {}),
        Box::new(rule::WaitGroupByValueRule {}),
        Box::new(rule::AtomicRule {}),
        Box::new(rule::EmptyLinesRule {}),
        // Box::new(rule::LineLengthLimitRule {}),
        // Box::new(rule::CallToGCRule {}),
        Box::new(rule::DuplicatedImportsRule {}),
        // Box::new(rule::ImportShadowingRule {}),
        // Box::new(rule::BareReturnRule {}),
        // Box::new(rule::UnusedReceiverRule {}),
        // Box::new(rule::UnhandledErrorRule {}),
        // Box::new(rule::CognitiveComplexityRule {}),
        Box::new(rule::StringOfIntRule {}),
        Box::new(rule::StringFormatRule {}),
        Box::new(rule::EarlyReturnRule {}),
        Box::new(rule::UnconditionalRecursionRule {}),
        Box::new(rule::IdenticalBranchesRule {}),
        Box::new(rule::DeferRule {}),
        // Box::new(rule::UnexportedNamingRule {}),
        // Box::new(rule::FunctionLength {}),
        // Box::new(rule::NestedStructs {}),
        Box::new(rule::IfReturnRule {}),
        // Box::new(rule::UselessBreak {}),
        Box::new(rule::TimeEqualRule {}),
        // Box::new(rule::BannedCharsRule {}),
        Box::new(rule::OptimizeOperandsOrderRule {}),
    ];
    rules.extend(defaultRules());
    rules
}

// run 对应 Go analyzer 的执行函数：收集文件名，配置 revive，过滤 failure，再转换为 analysis 报告。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    let mut files: Vec<String> = Vec::with_capacity(pass.Files.len());
    for file in &pass.Files {
        files.push(pass.Fset.PositionFor(file.Pos(), false).Filename);
    }
    // revive 的 Lint 接口按“包 -> 文件列表”接收输入；这里即使只分析一个 pass，也要包装成二维结构。
    let packages = vec![files];

    // Go 这里解析固定 Go 版本；解析失败直接 panic，草稿保留该致命路径。
    let gv = goversion::NewVersion("1.21").unwrap_or_else(|err| panic!("{}", err));
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

    // Go 逐条按规则名写入配置 map，defer 规则随后覆盖自定义参数。
    // 先统一登记规则名，再对需要参数化的规则做二次覆盖，能保持与 Go 配置装配顺序一致。
    for r in allRules() {
        conf.Rules.insert(r.Name(), lint::RuleConfig {});
    }
    conf.Rules.insert(
        "defer",
        lint::RuleConfig {
            Arguments: vec![vec!["loop", "method-call", "immediate-recover", "return"]],
        },
    );

    let lintingRules = config::GetLintingRules(&conf, vec![])?;
    let failures = revive.Lint(packages, lintingRules, conf.clone())?;

    // Go 使用 failure channel 加 goroutine 执行 JSON formatter。Rust 分离发送端和接收端，
    // 并用 output channel 同时传回格式化结果和建立完成同步。
    let (format_tx, format_rx) = std::sync::mpsc::channel::<lint::Failure>();
    let (output_tx, output_rx) = std::sync::mpsc::channel::<String>();
    let formatter = config::GetFormatter("json")?;
    let formatter_conf = conf.clone();
    std::thread::spawn(move || {
        let output = match formatter.Format(format_rx, formatter_conf) {
            Ok(output) => output,
            Err(err) => {
                log::Error("Format error", zap::Error(err));
                String::new()
            }
        };
        output_tx
            .send(output)
            .expect("formatter output receiver must remain alive");
    });

    // 只把 confidence 达标的 failure 交给 formatter，保持 Go 中 continue 过滤逻辑。
    // 过滤发生在格式化之前，因此最终 JSON 中看不到低置信度结果。
    for f in failures {
        if f.Confidence < conf.Confidence {
            continue;
        }
        format_tx
            .send(f)
            .expect("formatter must receive every qualifying failure");
    }

    drop(format_tx);
    let output = output_rx
        .recv()
        .expect("formatter thread must return its output");

    // revive formatter 输出 JSON 后再反序列化，随后用 token.Pos 精确定位报告位置。
    // 重新读取文件并计算 offset 的原因，是把 formatter 的行列号重新映射回当前 pass.Fset。
    // 这样生成的诊断位置才能与 go/analysis 消费的 token.Pos 保持同一坐标系。
    let results: Vec<jsonObject> = json::Unmarshal(output.as_bytes())?;
    for i in 0..results.len() {
        let res = &results[i];
        let (fileContent, tf) = util::ReadFile(&pass.Fset, &res.Failure.Position.Start.Filename)
            .unwrap_or_else(|err| panic!("{}", err));
        let offset = util::FindOffset(
            String::from_utf8_lossy(&fileContent).as_ref(),
            res.Failure.Position.Start.Line,
            res.Failure.Position.Start.Column,
        );
        pass.Reportf(
            token::Pos(tf.Base() + offset),
            format!("{}: {}", res.Failure.RuleName, res.Failure.Failure),
        );
    }

    Ok(None)
}
