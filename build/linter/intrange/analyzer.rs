// Copyright 2026 AsterSQL.

// Copyright 2024 PingCAP, Inc.
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

// 本文件由 build/linter/intrange/analyzer.go 迁移而来，保留 analyzer 的包级导出和初始化跳过逻辑。
// Go package: intrange。
//
// Go imports:
// - github.com/ckaznocha/intrange
// - github.com/pingcap/tidb/build/linter/util

// Analyzer 对应 Go 中直接复用的 intrange.Analyzer。
// 原 Go 注释误写为 ineffassign；这里保留语义说明，以实际右侧 intrange.Analyzer 为准。
// 上游 v0.3.1 导出 *analysis.Analyzer；共享引用保留同一 analyzer 对象及其配置状态。
pub static Analyzer: &analysis::Analyzer = intrange::Analyzer;

// init 对应 Go 初始化逻辑：先按配置跳过，再把 analyzer 放入默认跳过列表。
pub fn init() {
    // 这样做和 ineffassign 那类外部 analyzer 适配层保持同一接入约定。
    util::SkipAnalyzerByConfig(Analyzer);
    util::SkipAnalyzer(Analyzer);
}
