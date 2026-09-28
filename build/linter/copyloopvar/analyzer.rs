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

// 本文件由 build/linter/copyloopvar/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿只描述 copyloopvar linter analyzer 的注册形状，不会执行 Go analysis，也不会读取配置或改写源码。
// Go package: copyloopvar。
//
// Go imports:
// - github.com/karamaru-alpha/copyloopvar
// - github.com/pingcap/tidb/build/linter/util

// Analyzer is the analyzer struct of copyloopvar.
// 对应 Go 的包级变量 Analyzer = copyloopvar.NewAnalyzer()。
// 这里没有再包一层工厂函数，目的是和 Go 一样把“上游 analyzer 单例”直接暴露给注册器复用。
// 外部 copyloopvar crate/API 在本迁移草稿中仍按 Go 名称保留，表示实际 analyzer 由上游库构造。
// Go 允许包级变量直接调用工厂；Rust 需要惰性初始化才能在 static 中保留同一求值时机和单例语义。
pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer> =
    once_cell::sync::Lazy::new(|| copyloopvar::NewAnalyzer());

// init 对应 Go 的 init 函数：进程加载 analyzer 包时按仓库配置决定是否跳过该 analyzer。
// Rust 没有直接等价的包级 init 钩子，因此保留为普通函数，供后续模块接线时显式调用。
pub fn init() {
    // Go 这里把 Analyzer 传给 util.SkipAnalyzerByConfig；草稿只保留配置跳过语义，不读取真实配置。
    // 这样后续即使真正接入 analysis 驱动，也能先复用统一的“按配置禁用”入口。
    util::SkipAnalyzerByConfig(&Analyzer);
}
