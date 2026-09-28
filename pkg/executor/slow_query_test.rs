// Copyright 2026 AsterSQL.

// 慢查询日志字段解析与文件尾部按块读取的单元测试。
//
// 覆盖冒号字段拆分、User/Host 取值、嵌套方括号匹配，以及从真实文件按 cursor 读最后若干字节。

use std::fs::{File, remove_file};
use std::io::Write;

use crate::slow_query::{
    ReadLastLinesFromFile, findMatchedRightBracket, isLetterOrNumeric, parseUserOrHostValue,
    splitByColon,
};

/// 验证 splitByColon / 用户解析 / 括号匹配，以及 ReadLastLinesFromFile 尾读语义。
#[test]
fn slow_log_parser_handles_nested_fields_and_real_tail_io() {
    // 字段名:值 对按空白与下一字段名: 边界切分。
    let (fields, values) = splitByColon("User: root[root] Host: localhost Query_time: 1.25");
    assert_eq!(fields, vec!["User", "Host", "Query_time"]);
    assert_eq!(values, vec!["root[root]", "localhost", "1.25"]);
    // User 取值取 '[' 前的用户名部分。
    assert_eq!(parseUserOrHostValue("root[root] @ localhost"), "root");
    // 嵌套方括号按深度匹配最外层右括号。
    assert_eq!(findMatchedRightBracket("[a[b]c] suffix", 0), Some(6));

    // 写入临时慢日志，从 end_cursor 向前最多读 max_cache 字节。
    let path = std::env::temp_dir().join(format!(
        "astersql-slow-query-{}-{}.log",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let mut output = File::create(&path).unwrap();
    output.write_all(b"first\nsecond\nthird").unwrap();
    drop(output);
    let mut input = File::open(&path).unwrap();
    // end=18、max_cache=12：只物化尾部 12 字节，得到 second/third。
    let (lines, bytes) = ReadLastLinesFromFile(&mut input, 18, 12).unwrap();
    assert_eq!(bytes, 12);
    assert_eq!(lines, vec!["second".to_owned(), "third".to_owned()]);
    remove_file(path).unwrap();
}

/// Port of Go `TestSplitByColon`: preserve empty values, bracketed values and
/// malformed-bracket rejection instead of accepting a Rust-only grammar.
#[test]
fn split_by_colon_matches_go_case_matrix() {
    let cases = [
        ("", vec![], vec![]),
        ("123a", vec!["123a"], vec![""]),
        ("1a: 2b", vec!["1a"], vec!["2b"]),
        (
            "1a: [2b,[3c: 3cc]] 4d: 5e",
            vec!["1a", "4d"],
            vec!["[2b,[3c: 3cc]]", "5e"],
        ),
        (
            "1a: {2b,{3c: 3cc}} 4d: 5e",
            vec!["1a", "4d"],
            vec!["{2b,{3c: 3cc}}", "5e"],
        ),
        (
            "Time: 2021-09-08T14:39:54.506967433+08:00",
            vec!["Time"],
            vec!["2021-09-08T14:39:54.506967433+08:00"],
        ),
        (
            "Cop_proc_avg: 0 Cop_proc_addr: Cop_proc_max: Cop_proc_min: ",
            vec![
                "Cop_proc_avg",
                "Cop_proc_addr",
                "Cop_proc_max",
                "Cop_proc_min",
            ],
            vec!["0", "", "", ""],
        ),
    ];
    for (line, fields, values) in cases {
        assert_eq!(
            splitByColon(line),
            (
                fields.into_iter().map(str::to_owned).collect(),
                values.into_iter().map(str::to_owned).collect()
            ),
            "{line}"
        );
    }

    assert_eq!(
        splitByColon("1a: {{{2b,{3c: 3cc}} 4d: 5e"),
        (vec![], vec![])
    );
    assert_eq!(
        splitByColon("1a: [2b,[3c: 3cc]]]] 4d: 5e"),
        (vec![], vec![])
    );
    assert!(!isLetterOrNumeric(b'_'));
}

#[test]
fn bracket_matching_matches_go_square_and_curly_contract() {
    assert_eq!(findMatchedRightBracket("{a{b}c} suffix", 0), Some(6));
    assert_eq!(findMatchedRightBracket("[a[b]c]suffix", 0), None);
    assert_eq!(findMatchedRightBracket("(value)", 0), None);
}
