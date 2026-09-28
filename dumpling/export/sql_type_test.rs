// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `sql_type_test.go`：覆盖 SQL/CSV 转义路径，与 MySQL dump 转义规则对齐。

use crate::*;

/// 同一输入字节串在 escapeSQL/escapeCSV 四种模式下的期望输出；Go `TestEscape`。
#[test]
fn test_escape() {
    let mut bf = Vec::new();
    // 混合引号、反斜杠与 \r 的字节样本。
    let s = br#"MWQeWw""'\rNmtGxzGp"#;
    // escape_backslash=true 时 SQL 反斜杠转义期望。
    let expect_backslash = r#"MWQeWw\"\"\'\\rNmtGxzGp"#;
    let expect_without = r#"MWQeWw""''\rNmtGxzGp"#;
    let expect_csv_bs = r#"MWQeWw\"\"'\\rNmtGxzGp"#;
    let expect_csv_no = r#"MWQeWw""""'\rNmtGxzGp"#;
    // 四种 escape 模式期望串与 Go TestEscape 断言一致。

    escapeSQL(s, &mut bf, true);
    // NO_BACKSLASH_ESCAPES 关闭路径。
    assert_eq!(expect_backslash, String::from_utf8_lossy(&bf));

    bf.clear();
    escapeSQL(s, &mut bf, false);
    // 标准 SQL 单引号加倍路径。
    assert_eq!(expect_without, String::from_utf8_lossy(&bf));

    bf.clear();
    let mut opt = csvOption::default();
    opt.delimiter = b"\"".to_vec();
    opt.separator = b",".to_vec();
    escapeCSV(s, &mut bf, true, &opt);
    assert_eq!(expect_csv_bs, String::from_utf8_lossy(&bf));

    bf.clear();
    escapeCSV(s, &mut bf, false, &opt);
    assert_eq!(expect_csv_no, String::from_utf8_lossy(&bf));

    bf.clear();
    let s2 = b"a|*|b\"cd";
    escapeCSV(s2, &mut bf, false, &opt);
    assert_eq!(r#"a|*|b""cd"#, String::from_utf8_lossy(&bf));

    bf.clear();
    opt.delimiter = b"".to_vec();
    opt.separator = b"|*|".to_vec();
    escapeCSV(s2, &mut bf, true, &opt);
    // separator 作 delimiter 时 backslash 转义分隔符。
    assert_eq!(r#"a\|*\|b"cd"#, String::from_utf8_lossy(&bf));

    bf.clear();
    escapeCSV(s2, &mut bf, false, &opt);
    assert_eq!(r#"a|*|b"cd"#, String::from_utf8_lossy(&bf));
}
