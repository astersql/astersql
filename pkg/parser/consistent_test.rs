// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Rust 文法关键字、生成关键字表与运行时 token 表的一致性测试。

#[path = "keywords.rs"]
mod parser_keywords;

use std::collections::{BTreeMap, BTreeSet};

const MAIN_GRAMMAR: &str = include_str!("grammar/main.astergram");
const MISC_SOURCE: &str = include_str!("misc.rs");

/// 校验 Rust 文法关键字与生成关键字表、misc.rs token 表一致。
#[test]
fn test_keyword_consistent() {
    let grammar_tokens = grammar_token_literals(MAIN_GRAMMAR);
    let generated_keywords = parser_keywords::Keywords
        .iter()
        .map(|keyword| keyword.Word)
        .collect::<BTreeSet<_>>();
    let mut grammar_unreserved = production_literals(MAIN_GRAMMAR, "UnReservedKeyword");
    // genkeyword intentionally filters MariaDB-only words from the exported Go
    // Keywords snapshot even though the compatibility grammar accepts them.
    grammar_unreserved.remove("MONITOR");
    let grammar_not_keyword = production_literals(MAIN_GRAMMAR, "NotKeywordToken");
    let grammar_tidb = production_literals(MAIN_GRAMMAR, "TiDBKeyword");
    let generated_unreserved = parser_keywords::Keywords
        .iter()
        .filter(|keyword| keyword.Section == "unreserved")
        .map(|keyword| keyword.Word)
        .collect::<BTreeSet<_>>();

    for keyword in &generated_keywords {
        assert!(
            grammar_tokens.contains(keyword),
            "generated keyword {keyword} is absent from main.astergram"
        );
    }
    assert_eq!(
        grammar_unreserved, generated_unreserved,
        "UnReservedKeyword"
    );

    let generated_tidb = parser_keywords::Keywords
        .iter()
        .filter(|keyword| keyword.Section == "tidb")
        .map(|keyword| keyword.Word)
        .collect::<BTreeSet<_>>();
    assert_eq!(grammar_tidb, generated_tidb, "TiDBKeyword");

    let token_map = extract_static_pairs(
        MISC_SOURCE,
        "pub static tokenMap",
        "pub static btFuncTokenMap",
    );
    let window_func_token_map = extract_static_pairs(
        MISC_SOURCE,
        "pub static windowFuncTokenMap",
        "pub static aliases",
    );
    let aliases =
        extract_static_pairs(MISC_SOURCE, "pub static aliases", "pub static hintedTokens");
    for (alias, canonical) in &aliases {
        assert_ne!(alias, canonical);
        assert_eq!(
            token_map.get(alias),
            token_map.get(canonical),
            "alias {alias} must use the same token as {canonical}"
        );
    }
    let alias_words = aliases.keys().copied().collect::<BTreeSet<_>>();
    let window_words = window_func_token_map
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let token_not_keywords = token_map
        .keys()
        .copied()
        .filter(|word| !alias_words.contains(word))
        .filter(|word| !window_words.contains(word))
        .filter(|word| !generated_keywords.contains(word))
        .filter(|word| *word != "MONITOR")
        .collect::<BTreeSet<_>>();
    assert_eq!(grammar_not_keyword, token_not_keywords, "NotKeywordToken");
    let reserved_count = parser_keywords::Keywords
        .iter()
        .filter(|keyword| keyword.Reserved)
        .count();
    let grammar_keyword_count =
        reserved_count + grammar_unreserved.len() + grammar_not_keyword.len() + grammar_tidb.len();
    assert_eq!(
        token_map.len() - aliases.len() - 1,
        grammar_keyword_count - window_func_token_map.len()
    );
}

/// 抽取 Rust 静态二元组表，保留右侧表达式以校验别名共享同一 token。
fn extract_static_pairs<'a>(
    content: &'a str,
    start: &str,
    end: &str,
) -> BTreeMap<&'a str, &'a str> {
    extract_middle(content, start, end)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let key = quoted_literal(line)?;
            let value = line
                .split_once("\",")?
                .1
                .trim()
                .trim_end_matches(',')
                .trim_end_matches(')')
                .trim()
                .trim_matches('"');
            Some((key, value.trim_matches('"')))
        })
        .collect()
}

/// 返回 `start_marker` 与 `end_marker` 之间的子串；缺任一标记则返回空串。
fn extract_middle<'a>(str_: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start_idx = match str_.find(start_marker) {
        Some(idx) => idx,
        None => return "",
    };
    let str_ = &str_[start_idx + start_marker.len()..];
    let end_idx = match str_.find(end_marker) {
        Some(idx) => idx,
        None => return "",
    };
    &str_[..end_idx]
}

fn quoted_literal(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(&line[start..end])
}

fn grammar_token_literals(grammar: &str) -> BTreeSet<&str> {
    grammar
        .lines()
        .filter(|line| line.starts_with("%token "))
        .filter_map(quoted_literal)
        .collect()
}

fn production_literals<'a>(grammar: &'a str, production: &str) -> BTreeSet<&'a str> {
    let prefix = format!("{production} : ");
    grammar
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .filter_map(quoted_literal)
        .collect()
}
