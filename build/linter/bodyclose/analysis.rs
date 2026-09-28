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

// 本文件由 build/linter/bodyclose/analysis.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿只描述 bodyclose analyzer 的转发关系，不会真正运行 HTTP body close 检查。
// Go package: bodyclose。
//
// Go imports:
// - github.com/pingcap/tidb/build/linter/util
// - github.com/timakin/bodyclose/passes/bodyclose

// Analyzer is the analyzer struct of bodyclose.
// Analyzer 对应 Go 的包级变量，直接复用第三方 bodyclose.Analyzer。
pub static Analyzer: &analysis::Analyzer = bodyclose::Analyzer;

// init 对应 Go 的 init：把该 analyzer 加入 TiDB linter 的跳过配置。
pub fn init() {
    // 这里没有额外配置分支，说明该适配层只负责把第三方 analyzer 接入统一跳过体系。
    util::SkipAnalyzer(Analyzer);
}
