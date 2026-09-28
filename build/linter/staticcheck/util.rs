// Copyright 2026 AsterSQL.

// Copyright 2021 PingCAP, Inc.
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

// 本文件由 build/linter/staticcheck/util.go 迁移而来，聚合 staticcheck analyzer 并按名称查找。
// Go package: staticcheck。
//
// Go imports:
// - fmt
// - golang.org/x/tools/go/analysis
// - honnef.co/go/tools/analysis/lint
// - honnef.co/go/tools/quickfix
// - honnef.co/go/tools/simple
// - honnef.co/go/tools/staticcheck
// - honnef.co/go/tools/stylecheck
// - honnef.co/go/tools/unused

use std::collections::HashMap;

// Analyzers 对应 Go 中立即执行函数字面量初始化的全局 map。
// Go 版本把 quickfix/simple/staticcheck/stylecheck/unused 的 lint.Analyzer 展开后按 Analyzer.Name 建索引。
pub static Analyzers: once_cell::sync::Lazy<HashMap<String, &'static analysis::Analyzer>> =
    once_cell::sync::Lazy::new(|| {
        let mut resMap: HashMap<String, &'static analysis::Analyzer> = HashMap::new();

        // 保留 Go 的二维 analyzer 列表结构和后插入同名 analyzer 覆盖前值的语义。
        for analyzers in [
            quickfix::Analyzers.as_slice(),
            simple::Analyzers.as_slice(),
            staticcheck::Analyzers.as_slice(),
            stylecheck::Analyzers.as_slice(),
            std::slice::from_ref(&unused::Analyzer),
        ] {
            for a in analyzers {
                // Go 中 a 是 *lint.Analyzer，真正放入 map 的是 a.Analyzer。
                resMap.insert(a.Analyzer.Name.clone(), a.Analyzer);
            }
        }

        resMap
    });

// FindAnalyzerByName 对应 Go 的同名函数：按名称返回 staticcheck analyzer。
pub fn FindAnalyzerByName(name: &str) -> &'static analysis::Analyzer {
    if let Some(a) = Analyzers.get(name) {
        // 通过名字查表而不是硬编码具体包，便于 build stamping 复用同一适配层。
        return *a;
    }

    // Go 这里用 fmt.Sprintf 构造 panic 文本；Rust 保留同样的非法名称快速失败语义。
    panic!("not a valid staticcheck analyzer: {}", name);
}
