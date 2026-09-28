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

// 本文件由 build/linter/predeclared/analysis.go 迁移而来，保留 analyzer 导出和初始化跳过逻辑。
// Go package: predeclared。
//
// Go imports:
// - github.com/nishanths/predeclared/passes/predeclared
// - github.com/pingcap/tidb/build/linter/util

// Analyzer 对应 Go 的包级变量：直接复用第三方 predeclared analyzer。
// 上游 v0.2.2 导出 *analysis.Analyzer；共享引用保留同一 analyzer 对象及配置状态。
pub static Analyzer: &analysis::Analyzer = predeclared::Analyzer;

// init 对应 Go 的 init 函数：注册时按配置和硬编码规则跳过该 analyzer。
pub fn init() {
    // 这样做能让第三方 analyzer 也复用 TiDB 自己的文件过滤和 nolint 语义。
    util::SkipAnalyzerByConfig(Analyzer);
    util::SkipAnalyzer(Analyzer);
}
