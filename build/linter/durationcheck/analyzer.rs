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

// 本文件由 build/linter/durationcheck/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿只描述 durationcheck analyzer 的包级转接和 init 注册逻辑，不会运行 lint 或修改源码。
// Go package: durationcheck。
//
// Go imports:
// - github.com/charithe/durationcheck
// - github.com/pingcap/tidb/build/linter/util

// Analyzer is the analyzer struct of durationcheck.
// 对应 Go 的 Analyzer = durationcheck.Analyzer；上游 analyzer 实例按原名保留为外部依赖。
// 上游 v0.0.11 暴露的是 *analysis.Analyzer；Rust 以共享引用保留同一对象身份，不复制 analyzer 状态。
pub static Analyzer: &analysis::Analyzer = durationcheck::Analyzer;

// init 对应 Go 包初始化逻辑：先按配置跳过，再无条件加入跳过列表。
// Rust 草稿保留两个调用的顺序，因为 Go 中 SkipAnalyzerByConfig 和 SkipAnalyzer 都可能影响注册状态。
pub fn init() {
    // 先走配置门禁，意味着仓库若显式启用或禁用该规则，决策点仍与 Go 版本一致。
    util::SkipAnalyzerByConfig(Analyzer);
    // Go 文件中 durationcheck 被额外 SkipAnalyzer；这里保留这个全局跳过动作。
    // 这说明该规则在 TiDB 默认构建链路里处于“注册但默认不参与执行”的状态。
    util::SkipAnalyzer(Analyzer);
}
