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

// 本文件由 build/linter/rowserrcheck/analyzer.go 迁移而来，保留 analyzer 构造和初始化跳过逻辑。
// Go package: rowserrcheck。
//
// Go imports:
// - github.com/jingyugao/rowserrcheck/passes/rowserr
// - github.com/pingcap/tidb/build/linter/util

// Analyzer is the analyzer struct of rowserrcheck.
// Analyzer 对应 Go 的 rowserr.NewAnalyzer() 返回值；Lazy 保留运行时构造和共享单例语义。
pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer> =
    once_cell::sync::Lazy::new(|| rowserr::NewAnalyzer());

// init 对应 Go 的 init 函数：按配置和全局规则跳过该 analyzer。
pub fn init() {
    // 与多数外部 analyzer 适配层一致，这里同时接入仓库配置过滤和统一 nolint 包装。
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}
