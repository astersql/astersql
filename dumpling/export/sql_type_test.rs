// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! SQL escaping remains here; CSV framing tests live in csvfile.

use crate::*;

/// Go TestEscapeSQL verifies both SQL escape modes.
#[test]
fn test_escape_sql() {
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
}

#[test]
fn numeric_classification_and_raw_append_preserve_null() {
    for ty in ["DOUBLE PRECISION", "DECIMAL", "BOOL", "INT"] {
        assert!(dataTypeNumContains(ty));
    }
    assert!(!dataTypeNumContains("UNKNOWN"));
    let mut row = MakeRowReceiver(&["INT".into(), "BLOB".into(), "UNKNOWN".into()]);
    row.BindAddress(&mut [
        RawBytes(Some(b"1".to_vec())),
        RawBytes(None),
        RawBytes(Some(vec![])),
    ]);
    let mut raw = vec![RawBytes(Some(b"prefix".to_vec()))];
    row.appendRawBytes(&mut raw);
    assert_eq!(raw.len(), 4);
    assert_eq!(raw[1].as_opt(), Some(b"1".as_slice()));
    assert!(raw[2].as_opt().is_none());
    assert_eq!(raw[3].as_opt(), Some(b"".as_slice()));
}
