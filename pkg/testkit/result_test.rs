// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 查询结果断言辅助的 Go 兼容性测试。
//
// 覆盖失败消息中附加注释的拼接格式，以及按列检查时对期望行宽的前置约束，
// 防止 Rust 实现与 Go TestKit 的诊断行为产生偏差。

use crate::result::{Result, Rows, RowsWithSep};

#[test]
fn result_comments_match_go_append_semantics() {
    let mut result = Result::new(Rows(&["1"]));
    // Go 实现对首条及后续注释都先补换行，并保持注释的追加顺序。
    result.AddComment("first");
    result.AddComment("second");
    let panic = std::panic::catch_unwind(|| result.Check(Rows(&["2"]))).unwrap_err();
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(ToString::to_string))
        .unwrap_or_default();
    assert!(message.contains("\nfirst\nsecond"), "panic was: {message}");
}

#[test]
#[should_panic(expected = "expected row has 1 columns")]
fn check_at_rejects_expected_row_with_wrong_width() {
    // 期望行宽必须与选取列数一致，校验应发生在实际结果的列投影之前。
    Result::new(Rows(&["1 2"])).CheckAt(&[0, 1], vec![vec!["1"]]);
}

#[test]
fn check_matches_go_rendered_row_semantics_across_cell_boundaries() {
    let actual = Result::new(vec![vec![
        "Sort".to_owned(),
        "root".to_owned(),
        String::new(),
        "test.t1.col0, test.t1.col1".to_owned(),
    ]]);

    // Go's Rows splits the final operator info at its inner space, but Check
    // formats both rows before comparing them, so the protocol cell boundary
    // is intentionally irrelevant.
    let expected = Rows(&["Sort root  test.t1.col0, test.t1.col1"]);
    actual.Check(expected.clone());
    assert!(actual.Equal(expected));
}

#[test]
fn check_at_matches_go_rendered_row_semantics_across_cell_boundaries() {
    let actual = Result::new(vec![vec!["a".to_owned(), "b c".to_owned()]]);

    // Go compares fmt-rendered rows, so different cell boundaries producing
    // the same rendered text are equivalent.
    actual.CheckAt(&[0, 1], vec![vec!["a b", "c"]]);
}

#[test]
fn multi_containment_checks_the_joined_result_like_go() {
    let result = Result::new(vec![vec!["left".to_owned(), "right".to_owned()]]);

    result.MultiCheckContain(&["left right".to_owned()]);
    let panic = std::panic::catch_unwind(|| {
        result.MultiCheckNotContain(&["left right".to_owned()]);
    });
    assert!(panic.is_err());
}

#[test]
fn rows_with_empty_separator_matches_go_unicode_split() {
    assert_eq!(RowsWithSep("", &["Aé", ""]), vec![vec!["A", "é"], vec![]]);
}
