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

// 本文件由 build/linter/gosec/analysis.go 迁移而来：构造兼容的 loader.Program、
// 运行 gosec 规则并把 issue 映射回 analysis 诊断。
// Go package: gosec。
//
// Go imports:
// - fmt
// - go/token
// - go/types
// - io
// - log
// - strconv
// - github.com/golangci/golangci-lint/v2/pkg/result
// - github.com/golangci/gosec
// - github.com/golangci/gosec/rules
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis
// - golang.org/x/tools/go/loader

// Name 对应 Go 常量：作为 analyzer 名称和报告前缀使用。
// 规则编号和消息拼接时都会复用这个前缀，保证最终输出能被统一归类。
pub const Name: &str = "gosec";

// Analyzer 对应 Go 中导出的 analysis.Analyzer 指针，注册 gosec 的文档和 run 回调。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: Name,
    doc: "Inspects source code for security problems",
    requires: &[],
    run,
};

// init 对应 Go 初始化逻辑：既按配置跳过，也默认跳过该 analyzer。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}

// run 对应 Go 的 gosec analyzer 主体。
// 迁移重点是保留规则选择、loader.Program 拼装、issue 过滤和 token.Pos 计算顺序。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    let gasConfig = gosec::NewConfig();
    let enabledRules = rules::Generate(|id: &str| -> bool {
        // Go 只启用这几个安全规则；其它规则显式返回 false。
        matches!(id, "G104" | "G103" | "G101" | "G201")
    });
    // 原 Go 代码把日志写入 io.Discard，避免 analyzer 运行时输出噪声。
    let logger = log::New(io::Discard, "", 0);
    let mut analyzer = gosec::NewAnalyzer(gasConfig, logger);
    analyzer.LoadRules(enabledRules.Builders());

    // Arc 让 Created 与 AllPackages 共享同一个 PackageInfo，保留 Go 指针身份而不裸指针解引用。
    let createdPkgs: Vec<std::sync::Arc<loader::PackageInfo>> =
        vec![std::sync::Arc::new(util::MakeFakeLoaderPackageInfo(pass))];
    let mut allPkgs: HashMap<types::Package, std::sync::Arc<loader::PackageInfo>> = HashMap::new();
    for pkg in &createdPkgs {
        allPkgs.insert(pkg.Pkg.clone(), std::sync::Arc::clone(pkg));
    }
    let prog = loader::Program {
        Fset: &pass.Fset,
        Imported: None, // 不带 Created 时 linter 不使用 Imported，照 Go 注释保留。
        Created: createdPkgs, // 初始包集合来自当前 analysis.Pass。
        AllPackages: allPkgs, // 初始包及其依赖的索引表，字段名按 Go 结构体保留。
    };

    analyzer.ProcessProgram(&prog);
    let (mut issues, _) = analyzer.Report();
    if issues.is_empty() {
        return Ok(None);
    }

    let severity = gosec::Low;
    let confidence = gosec::Low;
    // Go 默认把阈值放到 Low/Low，等价于先拿到所有 issue，再交给 filterIssues 统一裁剪。
    issues = filterIssues(issues, severity, confidence);
    for i in issues {
        let (fileContent, tf) = match util::ReadFile(&mut pass.Fset, &i.File) {
            Ok((fileContent, tf)) => (fileContent, tf),
            Err(err) => {
                // Go 这里直接 panic；保留相同的致命失败语义。
                panic!("{}", err);
            }
        };

        let Some(line) = parseIssueLine(&i.Line) else {
            continue;
        };

        // ReadFile 当前返回 token.File 裸指针；只在读取稳定的 base 时做一次受限解引用。
        let file_base = unsafe { (*tf).Base() };
        // 这里只需要列 1；直接扫描原始字节可保留 Go string 对非法 UTF-8 的字节偏移。
        pass.Reportf(
            token::Pos(file_base + findLineOffset(&fileContent, line)),
            format!("[{}] {}: {}", Name, i.RuleID, i.What),
        );
    }

    Ok(None)
}

// filterIssues 对应 Go 的同名函数：只保留 severity 和 confidence 达到阈值的 issue。
pub fn filterIssues(
    issues: Vec<gosec::Issue>,
    severity: gosec::Score,
    confidence: gosec::Score,
) -> Vec<gosec::Issue> {
    let mut res: Vec<gosec::Issue> = Vec::new();
    for issue in issues {
        // 过滤逻辑故意写成线性扫描，和 Go 版一样保持最直接、最可审计的阈值比较。
        if issue.Severity >= severity && issue.Confidence >= confidence {
            res.push(issue);
        }
    }
    res
}

// gosec 同时使用单行号和 "from-to" 区间；区间诊断定位在起始行。
pub fn parseIssueLine(value: &str) -> Option<i32> {
    if let Ok(line) = value.parse::<i32>() {
        return Some(line);
    }
    let (from, to) = value.split_once('-')?;
    let from = from.parse::<i32>().ok()?;
    let _to = to.parse::<i32>().ok()?;
    Some(from)
}

// FindOffset 的调用固定为 column=1；按原始字节找行首可避免 lossy UTF-8 扩张偏移。
pub fn findLineOffset(fileContent: &[u8], line: i32) -> i32 {
    if fileContent.is_empty() || line <= 0 {
        return -1;
    }
    if line == 1 {
        return 0;
    }

    let mut current_line = 1;
    for (offset, byte) in fileContent.iter().enumerate() {
        if *byte == b'\n' {
            current_line += 1;
            if current_line == line && offset + 1 < fileContent.len() {
                return (offset + 1) as i32;
            }
        }
    }
    -1
}
