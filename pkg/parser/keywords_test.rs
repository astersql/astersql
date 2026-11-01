// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 关键字表测试：首项、总长度、保留字数量与分段内排序。

// 描述 parser 关键字表的测试断言。
#[path = "keywords.rs"]
mod parser;

/// 简化版 testify require.Equal，用于与 Go 断言形状对齐。
#[allow(non_snake_case)]
mod require {
    use std::fmt::Debug;
    /// 期望值与实际值相等，否则 panic。
    pub fn Equal<T: Debug + PartialEq<U>, U: Debug>(expected: T, actual: U) {
        assert_eq!(expected, actual);
    }
}

/// 简化版 testing.T.Errorf，用于排序失败时提示重新生成 keywords。
#[allow(non_snake_case)]
mod testing {
    /// 打印提示后 panic，对应 Go 的 t.Errorf 终止语义。
    pub fn Errorf(message: &str, left: &str, right: &str) -> ! {
        panic!("{}: {} / {}", message.trim(), left, right)
    }
}

// test_keywords 对应 Go 的 TestKeywords：检查关键字表首项，并确认 TiDB 自有关键字 ADMIN 已包含。
/// 校验 Keywords[0] 为保留字 ADD，且包含 TiDB 关键字 ADMIN。
#[test]
fn test_keywords() {
    // Go 原断言：parser.Keywords[0].Word == "ADD"，且 Reserved 为 true。
    require::Equal("ADD", parser::Keywords[0].Word);
    require::Equal(true, parser::Keywords[0].Reserved);

    let mut found = false;
    for kw in parser::Keywords.iter() {
        if kw.Word == "ADMIN" {
            found = true;
        }
    }
    // Go 使用 require.Equal 附带提示信息；这里保留相同检查语义。
    assert!(found, "TiDBKeyword ADMIN is part of the list");
    assert!(
        parser::Keywords
            .iter()
            .any(|keyword| keyword.Word == "AUTO" && !keyword.Reserved),
        "AUTO is part of the unreserved keyword list"
    );
}

// test_keywords_length 对应 Go 的 TestKeywordsLength：固定总关键字数和保留关键字数量。
/// 校验总关键字数 700、保留字数 233（与 Go 生成器快照一致）。
#[test]
fn test_keywords_length() {
    require::Equal(700, parser::Keywords.len());

    let mut reserved_nr = 0;
    for kw in parser::Keywords.iter() {
        if kw.Reserved {
            reserved_nr += 1;
        }
    }
    require::Equal(233, reserved_nr);
}

/// Go 生成列表不包含 MariaDB 专用的 MONITOR 关键字。
#[test]
fn test_keywords_excludes_mariadb_only_words() {
    assert!(parser::Keywords.iter().all(|kw| kw.Word != "MONITOR"));
}

// test_keywords_sorting 对应 Go 的 TestKeywordsSorting：同一 Section 内必须按 Word 升序排列。
/// 同一 Section 内 Word 必须升序；否则提示更新 Rust 文法并重新生成。
#[test]
fn test_keywords_sorting() {
    for (i, kw) in parser::Keywords.iter().enumerate() {
        if i > 1
            && parser::Keywords[i - 1].Word > kw.Word
            && parser::Keywords[i - 1].Section == kw.Section
        {
            // 排序错误时提示更新 Rust 文法并重新生成关键字表。
            testing::Errorf(
                "{} should come after {}, please update main.astergram and re-generate keywords.rs\n",
                parser::Keywords[i - 1].Word,
                kw.Word,
            );
        }
    }
}

#[test]
fn port_af11710fe4_materialized_parser_restore() {
    for (sql, valid, expected) in [
        (
            "show create materialized view test.t",
            true,
            "SHOW CREATE MATERIALIZED VIEW `test`.`t`",
        ),
        (
            "show create materialized view t",
            true,
            "SHOW CREATE MATERIALIZED VIEW `t`",
        ),
        (
            "show create materialized view log on test.t",
            true,
            "SHOW CREATE MATERIALIZED VIEW LOG ON `test`.`t`",
        ),
        (
            "show create materialized view log on t",
            true,
            "SHOW CREATE MATERIALIZED VIEW LOG ON `t`",
        ),
        ("show materialized views", true, "SHOW MATERIALIZED VIEWS"),
        (
            "show materialized views from test",
            true,
            "SHOW MATERIALIZED VIEWS IN `test`",
        ),
        ("show materialized views from 'test'", false, ""),
        (
            "show materialized views in test like 'mv%'",
            true,
            "SHOW MATERIALIZED VIEWS IN `test` LIKE _UTF8MB4'mv%'",
        ),
        (
            "show materialized views where mview_id = 1",
            true,
            "SHOW MATERIALIZED VIEWS WHERE `mview_id`=1",
        ),
        (
            "show materialized view mv remain_logs",
            true,
            "SHOW MATERIALIZED VIEW `mv` REMAIN_LOGS",
        ),
        (
            "show materialized view test.mv remain_logs",
            true,
            "SHOW MATERIALIZED VIEW `test`.`mv` REMAIN_LOGS",
        ),
        ("show materialized view mv", false, ""),
        ("show materialized view mv like 'mv%'", false, ""),
        ("show materialized view mv where mview_id = 1", false, ""),
        (
            "show materialized view logs",
            true,
            "SHOW MATERIALIZED VIEW LOGS",
        ),
        (
            "show materialized view logs from test",
            true,
            "SHOW MATERIALIZED VIEW LOGS IN `test`",
        ),
        ("show materialized view logs from 'test'", false, ""),
        (
            "show materialized view logs in test like '$mlog$%'",
            true,
            "SHOW MATERIALIZED VIEW LOGS IN `test` LIKE _UTF8MB4'$mlog$%'",
        ),
        (
            "show materialized view logs where mlog_id = 1",
            true,
            "SHOW MATERIALIZED VIEW LOGS WHERE `mlog_id`=1",
        ),
        (
            "show materialized view log on t wait_purge",
            true,
            "SHOW MATERIALIZED VIEW LOG ON `t` WAIT_PURGE",
        ),
        (
            "show materialized view log on test.t wait_purge",
            true,
            "SHOW MATERIALIZED VIEW LOG ON `test`.`t` WAIT_PURGE",
        ),
        ("show materialized view log on t", false, ""),
        ("show materialized view log on t like '$mlog$%'", false, ""),
        (
            "show materialized view log on t where mlog_id = 1",
            false,
            "",
        ),
        (
            "COMPARE MATERIALIZED VIEW mv AS OF TIMESTAMP '2026-01-01 00:00:00'",
            true,
            "COMPARE MATERIALIZED VIEW `mv` AS OF TIMESTAMP _UTF8MB4'2026-01-01 00:00:00'",
        ),
        (
            "COMPARE MATERIALIZED VIEW test.mv AS OF TIMESTAMP '2026-01-01 00:00:00' OUTPUT INTO TABLE test.diff",
            true,
            "COMPARE MATERIALIZED VIEW `test`.`mv` AS OF TIMESTAMP _UTF8MB4'2026-01-01 00:00:00' OUTPUT INTO TABLE `test`.`diff`",
        ),
        ("COMPARE MATERIALIZED VIEW mv", false, ""),
        (
            "COMPARE MATERIALIZED VIEW mv OUTPUT INTO TABLE diff",
            false,
            "",
        ),
        (
            "SELECT * FROM `t` AS /* comment */ a;",
            true,
            "SELECT * FROM `t` AS `a`",
        ),
    ] {
        let result = crate::New().ParseOneStmt(sql, "", "");
        if valid {
            let stmt = result.unwrap_or_else(|err| panic!("{sql}: {err}"));
            assert_eq!(
                crate::ast::sql_restore::restore_node(stmt.as_ref()).unwrap(),
                expected,
                "{sql}"
            );
        } else {
            assert!(result.is_err(), "must reject {sql}");
        }
    }
}

#[test]
fn port_af11710fe4_unreserved_keywords() {
    for word in ["COMPARE", "OUTPUT", "VIEWS"] {
        let keyword = parser::Keywords
            .iter()
            .find(|kw| kw.Word == word)
            .expect(word);
        assert!(!keyword.Reserved);
        assert_eq!(keyword.Section, "unreserved");
        let sql = format!("SELECT {word} FROM {word}");
        assert!(crate::New().ParseOneStmt(&sql, "", "").is_ok(), "{sql}");
    }
}

#[test]
fn port_af11710fe4_ast_fields_and_diagnostics() {
    use crate::ast::{CompareMaterializedViewStmt, ExprKind, ShowStmt, ShowStmtType};
    for (suffix, output) in [("", None), (" OUTPUT INTO TABLE test.diff", Some("diff"))] {
        let sql = format!(
            "COMPARE MATERIALIZED VIEW test.mv AS OF TIMESTAMP '2026-01-01 00:00:00'{suffix}"
        );
        let node = crate::New().ParseOneStmt(&sql, "", "").unwrap();
        let stmt = node
            .as_any()
            .downcast_ref::<CompareMaterializedViewStmt>()
            .unwrap();
        assert_eq!(stmt.ViewName.as_ref().unwrap().Schema.O, "test");
        assert_eq!(stmt.ViewName.as_ref().unwrap().Name.O, "mv");
        assert!(stmt.AsOf.is_some());
        assert_eq!(
            stmt.OutputTable.as_ref().map(|name| name.Name.O.as_str()),
            output
        );
    }
    for (sql, tp, pattern, predicate) in [
        (
            "SHOW MATERIALIZED VIEWS FROM test LIKE 'mv%'",
            ShowStmtType::MaterializedViews,
            true,
            false,
        ),
        (
            "SHOW MATERIALIZED VIEW LOGS IN test WHERE mlog_id=1",
            ShowStmtType::MaterializedViewLogs,
            false,
            true,
        ),
    ] {
        let node = crate::New().ParseOneStmt(sql, "", "").unwrap();
        let stmt = node.as_any().downcast_ref::<ShowStmt>().unwrap();
        assert_eq!(stmt.Tp, tp);
        assert_eq!(stmt.DBName, "test");
        assert_eq!(stmt.Pattern.is_some(), pattern);
        assert_eq!(stmt.Where.is_some(), predicate);
        if let Some(expr) = &stmt.Where {
            assert!(matches!(expr.Kind, ExprKind::Binary { .. }));
        }
    }
    for (sql, message) in [
        ("SHOW MATERIALIZED VIEW mv wrong", "expected REMAIN_LOGS"),
        (
            "SHOW MATERIALIZED VIEW LOG ON t wrong",
            "expected WAIT_PURGE",
        ),
    ] {
        let err = crate::New()
            .ParseOneStmt(sql, "", "")
            .err()
            .expect("reject invalid suffix");
        assert!(err.to_string().contains(message), "{err}");
    }
    for sql in [
        "show materialized view mv ReMaIn_LoGs",
        "show materialized view log on t WaIt_PuRgE",
    ] {
        assert!(crate::New().ParseOneStmt(sql, "", "").is_ok());
    }
}
