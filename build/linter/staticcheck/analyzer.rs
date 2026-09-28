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

// 本文件由 build/linter/staticcheck/analyzer.go 迁移而来，通过 stamping 注入的名称查找真实 analyzer。
// Go package: staticcheck。
//
// Go imports:
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// Value to be added during stamping
// name 对应 Go 的包级变量，构建 stamping 会替换这个占位值。
pub static name: &str = "dummy value please replace using x_defs";

// Analyzer is an analyzer from staticcheck.
// Analyzer 对应 Go 的 *analysis.Analyzer 指针，首次访问时通过 name 查找且保持上游共享身份。
pub static Analyzer: once_cell::sync::Lazy<&'static analysis::Analyzer> =
    once_cell::sync::Lazy::new(|| FindAnalyzerByName(name));

// init 对应 Go 的 init 函数：先根据 stamping 名称找到 analyzer，再登记跳过规则。
pub fn init() {
    // 真实 analyzer 名称在构建时注入，因此运行期查找失败表示 stamping 配置出了问题。
    util::SkipAnalyzerByConfig(*Analyzer);
    util::SkipAnalyzer(*Analyzer);
}
