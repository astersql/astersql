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

// 表过滤器核心：规则列表匹配、大小写包装与全量匹配。
//
// 规则在 Parse 后逆序存放，使后写规则优先（last rule wins）。

use super::{FilterError, matcherParser, tableRule, tableRulesParser};
use std::fmt::Debug;
use std::sync::Arc;

/// 表过滤器接口：匹配库表、仅匹配 schema，以及生成小写变体。
pub trait Filter: Debug + Send + Sync {
    fn MatchTable(&self, schema: &str, table: &str) -> bool;
    fn MatchSchema(&self, schema: &str) -> bool;
    fn toLower(&self) -> Box<dyn Filter>;
}

#[derive(Debug)]
/// 基于有序 `tableRule` 列表的过滤器实现。
pub struct tableFilter(pub Vec<tableRule>);

/// 从命令行参数列表解析表过滤规则；解析后 `rules.reverse()` 使后写优先。
pub fn Parse(args: Vec<String>) -> Result<Box<dyn Filter>, FilterError> {
    let mut parser = tableRulesParser {
        rules: Vec::with_capacity(args.len()),
        matcher_parser: matcherParser {
            fileName: "<cmdline>".into(),
            lineNum: 1,
        },
    };
    for arg in args {
        parser.parse(&arg, true)?;
    }
    parser.rules.reverse();
    Ok(Box::new(tableFilter(parser.rules)))
}

/// 包装为大小写不敏感：匹配前将输入与规则侧均按小写处理。
pub fn CaseInsensitive(filter: Box<dyn Filter>) -> Box<dyn Filter> {
    Box::new(loweredFilter {
        wrapped: Arc::from(filter.toLower()),
    })
}

impl Filter for tableFilter {
    fn MatchTable(&self, schema: &str, table: &str) -> bool {
        // 首条同时命中 schema 与 table 的规则决定接受/拒绝。
        self.0
            .iter()
            .find(|rule| rule.schema.matchString(schema) && rule.table.matchString(table))
            .is_some_and(|rule| rule.positive)
    }

    fn MatchSchema(&self, schema: &str) -> bool {
        // 否定规则仅在 table 为通配（匹配任意表）时才否决整个 schema。
        self.0
            .iter()
            .find(|rule| {
                rule.schema.matchString(schema) && (rule.positive || rule.table.matchAllStrings())
            })
            .is_some_and(|rule| rule.positive)
    }

    fn toLower(&self) -> Box<dyn Filter> {
        Box::new(tableFilter(
            self.0
                .iter()
                .map(|rule| tableRule {
                    schema: rule.schema.toLower(),
                    table: rule.table.toLower(),
                    positive: rule.positive,
                })
                .collect(),
        ))
    }
}

#[derive(Debug)]
/// 将输入库表名转小写后再委托给内层 Filter。
struct loweredFilter {
    wrapped: Arc<dyn Filter>,
}

// Go strings.ToLower maps each rune independently, without expansions or
// contextual final-sigma conversion.
fn go_lowercase(value: &str) -> String {
    value
        .chars()
        .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
        .collect()
}

impl Filter for loweredFilter {
    fn MatchTable(&self, schema: &str, table: &str) -> bool {
        self.wrapped
            .MatchTable(&go_lowercase(schema), &go_lowercase(table))
    }

    fn MatchSchema(&self, schema: &str) -> bool {
        self.wrapped.MatchSchema(&go_lowercase(schema))
    }

    fn toLower(&self) -> Box<dyn Filter> {
        Box::new(loweredFilter {
            wrapped: Arc::clone(&self.wrapped),
        })
    }
}

#[derive(Debug)]
/// 匹配任意库表的过滤器。
struct allFilter;

impl Filter for allFilter {
    fn MatchTable(&self, _schema: &str, _table: &str) -> bool {
        true
    }

    fn MatchSchema(&self, _schema: &str) -> bool {
        true
    }

    fn toLower(&self) -> Box<dyn Filter> {
        Box::new(allFilter)
    }
}

/// 构造全量匹配过滤器。
pub fn All() -> Box<dyn Filter> {
    Box::new(allFilter)
}
