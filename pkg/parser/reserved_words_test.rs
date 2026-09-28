// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 保留字（ReservedKeyword）与非保留字对照测试。
//
// 从 Rust 文法与生成关键字表抽取关键字分组，再用真实 Parser 验证：
// 保留字不能作为列别名，非保留字可以。ReservedKeyword 指语法上必须保留、
// 通常不能直接当标识符使用的关键字。

use std::{collections::HashSet, process::Command, sync::LazyLock};

use regex::Regex;

#[path = "keywords.rs"]
mod parser_keywords;

const MAIN_GRAMMAR: &str = include_str!("grammar/main.astergram");

/// Rust 文法四组关键字列表。
#[derive(Debug, Clone, Default)]
pub struct KeywordGroup {
    pub reserved_keywords: Vec<String>,
    pub unreserved_keywords: Vec<String>,
    pub not_keyword_tokens: Vec<String>,
    pub tidb_keywords: Vec<String>,
}

/// 默认测试只验证真实 Rust Parser，不要求外部服务。
#[test]
fn test_reserved_words_with_parser() {
    compare_reserved_words(|_, _| {});
}

/// 对应 Go 的 reserved_words_test build tag，必须显式运行：
/// `cargo test -p astersql-parser --offline test_compare_reserved_words_with_mysql -- --ignored --nocapture`
///
/// 需要 mysql CLI 和 127.0.0.1:3306 上的 root/无密码 MySQL 8.0 服务。
/// ASTERSQL_TEST_MYSQL_BIN 可指定 CLI，ASTERSQL_TEST_MYSQL_PORT 可指定隔离服务端口。
/// 每个 CLI 子进程建立真实连接，执行 SQL 后关闭连接；output 等待并回收进程。
#[test]
#[ignore = "requires an explicitly provisioned MySQL 8.0 server and mysql CLI"]
fn test_compare_reserved_words_with_mysql() {
    let mut mysql_checked = (0, 0);
    let parser_checked = compare_reserved_words(|query, error_pattern| {
        expect_mysql_result(query, error_pattern);
        if error_pattern.is_some() {
            mysql_checked.0 += 1;
        } else {
            mysql_checked.1 += 1;
        }
    });
    assert_eq!(
        mysql_checked, parser_checked,
        "every non-exempt keyword, including window functions, must reach MySQL"
    );
}

fn expect_mysql_result(query: &str, error_pattern: Option<&str>) {
    let executable = std::env::var_os("ASTERSQL_TEST_MYSQL_BIN").unwrap_or_else(|| "mysql".into());
    let port = std::env::var("ASTERSQL_TEST_MYSQL_PORT").unwrap_or_else(|_| "3306".into());
    // Ignore user configuration and login paths so this test cannot silently connect
    // to a different database. The SQL only evaluates constant scalar subqueries.
    let output = Command::new(executable)
        .args([
            "--no-defaults",
            "--protocol=TCP",
            "--host=127.0.0.1",
            "--user=root",
            "--password=",
            "--connect-timeout=3",
            "--batch",
            "--skip-column-names",
            "--port",
            &port,
            "--execute",
            query,
        ])
        .env(
            "MYSQL_TEST_LOGIN_FILE",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env_remove("MYSQL_PWD")
        .output()
        .unwrap_or_else(|error| panic!("cannot execute mysql CLI for {query}: {error}"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    match error_pattern {
        Some(pattern) => {
            assert_eq!(
                output.status.code(),
                Some(1),
                "MySQL must reject {query}: {stderr}"
            );
            assert!(
                Regex::new(pattern)
                    .expect("keyword regexp")
                    .is_match(&stderr),
                "MySQL error for {query} did not match {pattern:?}: {stderr}"
            );
        }
        None => assert!(
            output.status.success(),
            "MySQL must accept {query}: {stderr}"
        ),
    }
}

/// 保持 Go 两个循环的例外、Parser 断言及 MySQL 校验顺序。
fn compare_reserved_words(mut mysql_check: impl FnMut(&str, Option<&str>)) -> (usize, usize) {
    let groups = KeywordGroup {
        reserved_keywords: parser_keywords::Keywords
            .iter()
            .filter(|keyword| keyword.Reserved)
            .map(|keyword| keyword.Word.to_owned())
            .collect(),
        unreserved_keywords: production_keywords(MAIN_GRAMMAR, "UnReservedKeyword"),
        not_keyword_tokens: production_keywords(MAIN_GRAMMAR, "NotKeywordToken"),
        tidb_keywords: production_keywords(MAIN_GRAMMAR, "TiDBKeyword"),
    };
    assert!(
        !groups.reserved_keywords.is_empty(),
        "ReservedKeyword must not be empty"
    );
    assert!(
        !groups.unreserved_keywords.is_empty(),
        "UnReservedKeyword must not be empty"
    );
    assert!(
        !groups.not_keyword_tokens.is_empty(),
        "NotKeywordToken must not be empty"
    );
    assert!(
        !groups.tidb_keywords.is_empty(),
        "TiDBKeyword must not be empty"
    );

    let mut parser = new_parser();
    let mut reserved_checked = 0usize;
    let mut unreserved_checked = 0usize;

    for kw in &groups.reserved_keywords {
        if is_tidb_only_reserved_exception(kw) {
            continue;
        }

        let query = format!("do (select 1 as {kw})");
        let err_regexp = format!(".*{kw}.*");

        if !window_func_token_map_contains(kw) {
            expect_tidb_parse_error(&mut parser, &query, &err_regexp);
        }
        mysql_check(&query, Some(&err_regexp));
        reserved_checked += 1;
    }

    for kws in [
        &groups.unreserved_keywords,
        &groups.not_keyword_tokens,
        &groups.tidb_keywords,
    ] {
        for kw in kws {
            if is_mysql_reserved_exception(kw) {
                continue;
            }

            let query = format!("do (select 1 as {kw})");
            expect_tidb_do_stmt(&mut parser, &query);
            mysql_check(&query, None);
            unreserved_checked += 1;
        }
    }

    println!(
        "validated {reserved_checked} reserved and {unreserved_checked} non-reserved keywords"
    );
    (reserved_checked, unreserved_checked)
}

/// 从 Rust 文法指定产生式抽取关键字并排序。
pub fn production_keywords(content: &str, production: &str) -> Vec<String> {
    let prefix = format!("{production} : ");
    let mut keywords = content
        .lines()
        .filter(|line| line.starts_with(&prefix))
        .filter_map(|line| {
            extract_middle(line, "\"", "\"")
                .split_whitespace()
                .next()
                .map(str::to_owned)
        })
        .filter(|keyword| !keyword.is_empty())
        .collect::<Vec<_>>();
    keywords.sort();
    keywords
}

/// 截取 content 中 start 与 end 之间的子串；任一标记缺失则返回空串。
fn extract_middle<'a>(content: &'a str, start: &str, end: &str) -> &'a str {
    let Some(start_index) = content.find(start) else {
        return "";
    };
    let rest = &content[start_index + start.len()..];
    let Some(end_index) = rest.find(end) else {
        return "";
    };
    &rest[..end_index]
}

/// 构造默认 Parser 实例。
pub fn new_parser() -> Box<parser::Parser> {
    parser::New()
}

/// 判断关键字是否出现在 misc.rs 的 windowFuncTokenMap 中（窗口函数关键字例外）。
pub fn window_func_token_map_contains(kw: &str) -> bool {
    static WINDOW_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
        let source = include_str!("misc.rs");
        extract_middle(
            source,
            "pub static windowFuncTokenMap",
            "pub static aliases",
        )
        .lines()
        .filter_map(|line| extract_middle(line, "\"", "\"").split_whitespace().next())
        .filter(|keyword| !keyword.is_empty())
        .collect()
    });
    WINDOW_FUNCTIONS.contains(kw)
}

/// TiDB 独有保留字例外：不与 MySQL 保留字集合强行对齐。
pub fn is_tidb_only_reserved_exception(kw: &str) -> bool {
    matches!(
        kw,
        "CURRENT_ROLE"
            | "STATS_EXTENDED"
            | "TABLESAMPLE"
            | "ARRAY"
            | "ILIKE"
            | "TIDB_CURRENT_TSO"
            | "UNTIL"
    )
}

/// MySQL 侧额外保留、但在本分组中按非保留路径验证的例外关键字。
pub fn is_mysql_reserved_exception(kw: &str) -> bool {
    matches!(
        kw,
        "FUNCTION" | "PURGE" | "SYSTEM" | "SEPARATOR" | "DECLARE"
    )
}

/// 期望 Parser.Parse 失败，且错误信息匹配给定正则。
pub fn expect_tidb_parse_error(parser: &mut parser::Parser, query: &str, err_regexp: &str) {
    let error = match parser.Parse(query, "", "") {
        Ok(_) => panic!("TiDB parser unexpectedly accepted {query}"),
        Err(error) => error,
    };
    assert!(
        Regex::new(err_regexp)
            .expect("keyword regexp")
            .is_match(&error.to_string()),
        "TiDB parser error {error:?} did not match {err_regexp:?}",
    );
}

/// 期望成功解析为单条 DoStmt（用于验证非保留字可作别名）。
pub fn expect_tidb_do_stmt(parser: &mut parser::Parser, query: &str) {
    let (statements, _) = parser
        .Parse(query, "", "")
        .unwrap_or_else(|error| panic!("TiDB parser rejected {query}: {error}"));
    assert_eq!(statements.len(), 1, "{query}");
    assert!(
        statements[0].as_any().is::<parser::ast::DoStmt>(),
        "{query}"
    );
}
