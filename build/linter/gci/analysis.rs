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

// 本文件由 build/linter/gci/analysis.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 gci analyzer 如何收集文件名、构造 gci 配置、计算 import diff 并报告诊断；
// 当前不会真正接入 Go analysis 框架、不会改写 import，也不会执行业务动作。
// Go package: gci。
//
// Go imports:
// - fmt
// - sync
// - github.com/daixiang0/gci/pkg/config
// - github.com/daixiang0/gci/pkg/gci
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// Analyzer 对应 Go 的 analysis.Analyzer 字面量。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: "gci",
    doc: "Gci controls golang package import order and makes it always deterministic.",
    requires: &[],
    run,
};

// run 对应 Go 的 analyzer 回调：把当前 pass 中的文件交给 gci 计算格式化 diff。
// Go 返回 (any, error)；Rust 以动态空结果保留 any(nil)，并原样传播 gci diff 错误。
pub fn run(pass: &mut analysis::Pass) -> anyhow::Result<Option<Box<dyn std::any::Any>>> {
    let mut fileNames: Vec<String> = Vec::with_capacity(pass.Files.len());
    for f in &pass.Files {
        let pos = pass.Fset.PositionFor(f.Pos(), false);
        fileNames.push(pos.Filename);
    }

    let rawCfg = config::YamlConfig {
        Cfg: config::BoolConfig {
            NoInlineComments: false,
            NoPrefixComments: false,
            Debug: false,
            SkipGenerated: true,
            SkipVendor: false,
            CustomOrder: false,
            NoLexOrder: false,
        },
        // Go 未列出的 SectionStrings、SectionSeparatorStrings 和 ModPath 都取零值。
        ..Default::default()
    };
    // 这组默认 section 在上游 v0.13.7 不会失败；若上游契约改变，Go 会在随后解引用 nil 时 panic。
    let cfg = rawCfg.Parse().expect("default gci config must parse");
    let mut diffs: Vec<String> = Vec::new();
    let lock = std::sync::Mutex::new(());

    // Go 传入 diffs slice 和 sync.Mutex 指针，让 gci 填充所有文件的格式化差异。
    // Rust 草稿保留共享 diff 容器和互斥锁这两个并发/资源同步参数。
    gci::DiffFormattedFilesToArray(fileNames, cfg, &mut diffs, &lock)?;

    for diff in diffs {
        if diff.is_empty() {
            continue;
        }

        // Go 将 diff 前置换行后作为 analysis.Diagnostic 上报，位置固定为 1。
        pass.Report(analysis::Diagnostic {
            Pos: 1,
            Message: format!("\n{}", diff),
        });
    }

    Ok(None)
}

// init 对应 Go 的 init 函数：按配置跳过 gci analyzer。
pub fn init() {
    // gci 只走配置化跳过，不叠加通用 SkipAnalyzer，和 Go 源的接入方式保持一致。
    util::SkipAnalyzerByConfig(&Analyzer);
}
