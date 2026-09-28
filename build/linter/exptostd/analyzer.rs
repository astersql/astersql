// Copyright 2025 PingCAP, Inc.
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

// 本文件由 build/linter/exptostd/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 exptostd analyzer 的注册入口；它只保留 Go linter 绑定关系，
// 不会真正接入 Go analysis 框架、不会读取仓库文件，也不会执行业务动作。
// Go package: exptostd。
//
// Go imports:
// - github.com/ldez/exptostd
// - github.com/pingcap/tidb/build/linter/util

// Analyzer 对应 Go 的包级变量：直接复用 exptostd.NewAnalyzer() 创建的 analyzer。
// 真实 analyzer 类型来自 Go 依赖 github.com/ldez/exptostd，这里按迁移草稿保留调用形状。
// 使用 Lazy 是因为 Rust 的静态初始化无法直接执行任意构造逻辑，需要把 Go 的包加载时求值改成首次访问求值。
pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer> =
    once_cell::sync::Lazy::new(|| exptostd::NewAnalyzer());

// init 对应 Go 的 init 函数：按配置跳过该 analyzer。
// Go init 在包加载时自动执行；Rust 草稿用显式函数表示同一注册副作用。
pub fn init() {
    // 后续若 build/linter 总入口批量注册 analyzer，这里仍可保持和 Go 一样的统一禁用点。
    util::SkipAnalyzerByConfig(&Analyzer);
}
