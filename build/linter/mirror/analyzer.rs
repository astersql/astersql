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

// 本文件由 build/linter/mirror/analyzer.go 迁移而来，保留 mirror analyzer 的外部构造和 util 跳过注册。
// Go package: mirror。
//
// Go imports:
// - github.com/butuzov/mirror
// - github.com/pingcap/tidb/build/linter/util

// Analyzer is the analyzer struct of mirror.
// Go 直接返回 mirror.NewAnalyzer 的 analysis.Analyzer；这里保留同名构造调用形状。
// Go 在包初始化时调用运行时工厂；Lazy 保证只构造一次并共享同一 analyzer。
pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer> =
    once_cell::sync::Lazy::new(|| mirror::NewAnalyzer());

// init 对应 Go 的包初始化：按配置跳过 mirror analyzer，并加入通用跳过列表。
pub fn init() {
    // 配置跳过和通用跳过同时存在时，nolint 与仓库级禁用规则都能生效。
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}
