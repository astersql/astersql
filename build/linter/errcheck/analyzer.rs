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

// 本文件由 build/linter/errcheck/analyzer.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 errcheck analyzer 如何读取内嵌 excludes 文件并写入 Analyzer.Flags。
// 当前 Rust 草稿不会真正执行 errcheck，也不会读取运行时文件系统或改动 Go 源码。
// Go package: errcheck。
//
// Go imports:
// - embed
// - log
// - github.com/kisielk/errcheck/errcheck
// - github.com/pingcap/tidb/build/linter/util

// Analyzer is the analyzer struct of errcheck.
// 对应 Go 的 Analyzer = errcheck.Analyzer，保留外部 errcheck analyzer 的包级变量语义。
// 上游 v1.10.0 暴露的是 *analysis.Analyzer；共享引用保留 Flags 的同一份可变注册状态。
pub static Analyzer: &analysis::Analyzer = errcheck::Analyzer;

//go:embed errcheck_excludes.txt
// Go 使用 embed.FS 延迟读取内嵌文件；Rust 草稿用 include_str! 表达同一个“编译期随文件携带”的迁移意图。
// 这样 excludes 清单依旧跟随源码版本走，避免运行环境缺文件时让 analyzer 行为漂移。
pub static excludesContent: &str = include_str!("errcheck_excludes.txt");

// init 对应 Go 的 init 函数：加载 excludes 文本，写入 Analyzer.Flags，并按配置跳过 analyzer。
pub fn init() {
    // Go 的 ReadFile 错误被忽略：data, _ := excludesContent.ReadFile(...)。
    // include_str! 没有运行时错误分支，因此这里只保留读取结果已经是字符串的状态。
    let data = excludesContent;

    // 这里延续 Go 的 Flags.Set 入口，而不是直接改内部字段，保证后续 flag 解析副作用不被绕开。
    let err = Analyzer.Flags.Set("excludes", data.to_string());
    if err.is_err() {
        // Go 这里 log.Fatal(err) 会终止进程；迁移草稿只保留致命错误处理语义。
        log::Fatal(err.unwrap_err());
    }

    util::SkipAnalyzerByConfig(Analyzer);
    // 与 Go 保持一致：errcheck 还会被无条件 SkipAnalyzer。
    util::SkipAnalyzer(Analyzer);
}
