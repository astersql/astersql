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

// 列名过滤器：按通配/正则/否定规则决定列是否匹配。
//
// 用于备份、同步等场景按列名筛选；规则来自命令行或 `@file` 导入。
// 匹配大小写不敏感，后写规则优先（解析后 reverse）。对应 Go `ParseColumnFilter`。

use super::{FilterError, columnRule, columnRulesParser, matcherParser};
use std::fmt::Debug;

/// 列过滤器接口：判断给定列名是否被规则接受。
pub trait ColumnFilter: Debug {
    /// 若列名匹配某条规则且该规则为肯定（positive），则返回 true。
    fn MatchColumn(&self, column: &str) -> bool;
}

/// 已解析的列规则列表，按后写优先的顺序保存。
#[derive(Debug, Default)]
pub struct ColumnFilterRules(Vec<columnRule>);

impl ColumnFilterRules {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 判断列名是否被规则允许。
    pub fn MatchColumn(&self, column: &str) -> bool {
        <Self as ColumnFilter>::MatchColumn(self, column)
    }
}

/// 解析命令行风格的列过滤参数，返回可匹配的 `ColumnFilter`。
///
/// `args` 中每项为一条规则或 `@file` 导入；解析失败返回 `FilterError`。
pub fn ParseColumnFilter(args: Vec<String>) -> Result<Box<dyn ColumnFilter>, FilterError> {
    Ok(Box::new(ParseColumnFilterRules(args)?))
}

/// 解析并返回可供调用方保存的具体规则类型。
pub fn ParseColumnFilterRules(args: Vec<String>) -> Result<ColumnFilterRules, FilterError> {
    let mut parser = columnRulesParser {
        rules: Vec::with_capacity(args.len()),
        matcher_parser: matcherParser {
            // 命令行来源用固定伪文件名，便于错误消息定位。
            fileName: "<cmdline>".into(),
            lineNum: 1,
        },
    };
    for arg in args {
        parser.parse(&arg, true)?;
    }
    // 反转后迭代时先遇到后写规则，实现“后写优先”。
    parser.rules.reverse();
    Ok(ColumnFilterRules(parser.rules))
}

impl ColumnFilter for ColumnFilterRules {
    fn MatchColumn(&self, column: &str) -> bool {
        // Go 的 strings.ToLower 对每个 rune 使用简单大小写映射；Rust 的
        // str::to_lowercase 可能把一个字符扩展成多个字符（例如 İ -> i + ◌̇）。
        let column: String = column
            .chars()
            .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
            .collect();
        self.0
            .iter()
            .find(|rule| rule.column.matchString(&column))
            .is_some_and(|rule| rule.positive)
    }
}
