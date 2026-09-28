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

// 本文件由 build/linter/makezero/analyzer.go 迁移而来，保留 makezero analyzer 的构造与跳过配置注册。
// Go package: makezero。
//
// Go imports:
// - github.com/ashanbrown/makezero/pkg/analyzer
// - github.com/pingcap/tidb/build/linter/util

// Analyzer is the analyzer struct of ineffassign.
// Go 源码注释写作 ineffassign，这里按原文保留；实际构造函数来自 makezero/pkg/analyzer。
// Go 在包初始化时调用运行时工厂；Lazy 保留一次构造和后续共享同一 analyzer 的语义。
pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer> =
    once_cell::sync::Lazy::new(|| analyzer::NewAnalyzer());

// init 对应 Go 的包初始化：把外部 analyzer 交给 TiDB linter util 的跳过配置。
pub fn init() {
    // 这里沿用和 mirror/ineffassign 相同的包装模式，方便后续统一接入第三方 analyzer。
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}
