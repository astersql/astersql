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

// 本文件由 build/linter/forcetypeassert/analysis.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 forcetypeassert analyzer 的注册和跳过策略；它只保留 Go 外部 analyzer 引用，
// 不会真正接入 Go analysis 框架、不会扫描源码，也不会执行业务动作。
// Go package: forcetypeassert。
//
// Go imports:
// - github.com/gostaticanalysis/forcetypeassert
// - github.com/pingcap/tidb/build/linter/util

// Analyzer 对应 Go 的包级变量：直接使用 gostaticanalysis/forcetypeassert 提供的 Analyzer。
// 真实 analyzer 类型由 Go 依赖提供，Rust 草稿保留“外部 analyzer 透传”的语义。
// 上游 v0.2.0 暴露的是 *analysis.Analyzer；共享引用保留 Requires、ResultType 和运行状态的对象身份。
pub static Analyzer: &analysis::Analyzer = forcetypeassert::Analyzer;

// init 对应 Go 的 init 函数。
// Go 代码先按配置跳过，再无条件 SkipAnalyzer；这里保持两个调用的顺序和副作用含义。
pub fn init() {
    // 前者尊重仓库配置，后者则把该规则纳入统一的默认跳过包装流程。
    util::SkipAnalyzerByConfig(Analyzer);
    util::SkipAnalyzer(Analyzer);
}
