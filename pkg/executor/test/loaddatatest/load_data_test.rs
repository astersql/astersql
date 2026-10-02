// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Rust counterparts of the scenarios in `load_data_test.go`.
//!
//! 本模块以轻量模型复现 Go 版 LOAD DATA 回归场景，验证格式选项、字段解码、
//! 列映射、重复键处理以及事务生命周期等可观察语义，不依赖完整 SQL 执行器。

use crate::{
    LoadDataConfig, LoadDataError, LoadDataEvent, LoadDataLifecycle, apply_replace, map_columns,
    parse_load_data, parse_unsigned_bigint, validate_load_data_options,
};

#[test]
fn test_load_data_init_param() {
    // 按执行器的校验顺序覆盖互斥格式选项；合法选项最终仍因测试未安装 reader 而失败。
    let defaults = LoadDataConfig::default();
    assert_eq!(defaults.null_def, vec![r"\N"]);
    assert_eq!(
        validate_load_data_options("", true, None, &defaults),
        Err(LoadDataError::EmptyPath)
    );
    assert_eq!(
        validate_load_data_options(
            "/a",
            true,
            None,
            &LoadDataConfig {
                null_value_opt_enclosed: true,
                ..defaults.clone()
            }
        ),
        Err(LoadDataError::MustSpecifyEnclosed)
    );
    assert_eq!(
        validate_load_data_options(
            "/a",
            true,
            None,
            &LoadDataConfig {
                lines_terminated_by: String::new(),
                ..defaults.clone()
            }
        ),
        Err(LoadDataError::EmptyLineTerminator)
    );
    assert_eq!(
        validate_load_data_options(
            "/a",
            true,
            None,
            &LoadDataConfig {
                fields_terminated_by: "a".into(),
                lines_terminated_by: "a".into(),
                ..defaults.clone()
            }
        ),
        Err(LoadDataError::OverlappingTerminators)
    );
    assert_eq!(
        validate_load_data_options("/a", true, None, &defaults),
        Err(LoadDataError::ReaderNil)
    );
    assert_eq!(
        validate_load_data_options("/a", true, Some("sql file"), &defaults),
        Err(LoadDataError::ReaderNil)
    );
    assert_eq!(
        validate_load_data_options(
            "/a",
            true,
            Some("delimited data"),
            &defaults.clone().terminated_by("a"),
        ),
        Err(LoadDataError::ReaderNil)
    );
    assert_eq!(
        defaults.clone().null_by(["a", r"\N"]).null_def,
        vec!["a", r"\N"]
    );
}

#[test]
fn test_load_data() {
    // 同时覆盖默认 TSV 方言与自定义行前缀、多字节行终止符组合。
    let config = LoadDataConfig::default();
    let rows = parse_load_data(b"1\thello\n2\tworld\n", &config).unwrap();
    assert_eq!(
        rows,
        vec![
            vec![Some("1".into()), Some("hello".into())],
            vec![Some("2".into()), Some("world".into())]
        ]
    );

    let custom = config.clone().terminated_by("\\").lines("xxx", "|!#^");
    let rows = parse_load_data(b"xxx3\\2\\3\\4|!#^xxx4\\2\\3\\4|!#^", &custom).unwrap();
    assert_eq!(
        rows,
        vec![
            vec![
                Some("3".into()),
                Some("2".into()),
                Some("3".into()),
                Some("4".into())
            ],
            vec![
                Some("4".into()),
                Some("2".into()),
                Some("3".into()),
                Some("4".into())
            ]
        ]
    );

    // Go scans forward to LINES STARTING BY within a physical record instead of
    // discarding the entire record when the prefix is not at byte zero.
    let rows = parse_load_data(b"10\\2\\3xxx11\\4\\5|!#^", &custom).unwrap();
    assert_eq!(
        rows,
        vec![vec![Some("11".into()), Some("4".into()), Some("5".into())]]
    );

    let mut lifecycle = LoadDataLifecycle::default();
    lifecycle.run(false);
    // LOAD DATA 必须先开启事务再提交，并在结束时关闭输入 reader。
    assert!(lifecycle.begin_before_commit());
    assert_eq!(lifecycle.events.last(), Some(&LoadDataEvent::CloseReader));
}

#[test]
fn test_load_data_escape() {
    // 反斜杠转义需兼容控制字符；嵌入普通文本的未知转义（如 \N）只去掉反斜杠。
    let rows = parse_load_data(
        b"1\ta string\n2\tstr \\t\n3\tstr \\n\n4\tboth \\t\\n\n5\tstr \\\\\n6\t\\r\\t\\n\\0\\Z\\b\n7\trtn0ZbN\n8\trtn0Zb\\N\n9\ttab\\ttab\n",
        &LoadDataConfig::default(),
    )
    .unwrap();
    assert_eq!(rows[0], vec![Some("1".into()), Some("a string".into())]);
    assert_eq!(rows[1], vec![Some("2".into()), Some("str \t".into())]);
    assert_eq!(rows[2], vec![Some("3".into()), Some("str \n".into())]);
    assert_eq!(rows[3], vec![Some("4".into()), Some("both \t\n".into())]);
    assert_eq!(rows[4], vec![Some("5".into()), Some("str \\".into())]);
    assert_eq!(rows[5][1], Some("\r\t\n\0\u{1a}\u{8}".into()));
    assert_eq!(rows[6][1], Some("rtn0ZbN".into()));
    assert_eq!(rows[7][1], Some("rtn0ZbN".into()));
    assert_eq!(rows[8][1], Some("tab\ttab".into()));
}

#[test]
fn test_load_data_specified_columns() {
    // 输入列只写入显式目标位置，未映射列保持空值，\N 仍映射为 SQL NULL。
    let rows =
        parse_load_data(b"7\ta string\n\\N\ta string\n", &LoadDataConfig::default()).unwrap();
    let mapped = map_columns(&rows, &[1, 2], None, false).unwrap();
    assert_eq!(
        mapped[0],
        vec![None, Some("7".into()), Some("a string".into())]
    );
    assert_eq!(mapped[1], vec![None, None, Some("a string".into())]);
}

#[test]
fn test_load_data_ignore_lines() {
    // IGNORE LINES 按解析完成的记录计数，被跳过的记录不应影响后续记录边界。
    let config = LoadDataConfig {
        ignore_lines: 1,
        ..LoadDataConfig::default()
    };
    assert_eq!(
        parse_load_data(b"1\tline1\n2\tline2\n3\tline3\n", &config).unwrap(),
        vec![
            vec![Some("2".into()), Some("line2".into())],
            vec![Some("3".into()), Some("line3".into())]
        ]
    );
}

#[test]
fn test_load_data_null() {
    // 是否被引号包围会改变 NULL 判定：裸 NULL/\N 为空，引号内 NULL 保留为文本。
    let config = LoadDataConfig::default()
        .terminated_by(",")
        .enclosed(b'"', false)
        .lines("", "\n");
    let rows = parse_load_data(b"NULL,\"NULL\"\n\\N,\"\\N\"\n\"\\\\N\"", &config).unwrap();
    assert_eq!(
        rows,
        vec![
            vec![None, Some("NULL".into())],
            vec![None, None],
            vec![Some("\\N".into())]
        ]
    );
}

#[test]
fn test_load_data_replace() {
    // REPLACE 以首列为键删除较早的重复行，保留输入中最后出现的值并报告删除数。
    let mut rows = vec![
        vec![Some("1".into()), Some("line1".into())],
        vec![Some("2".into()), Some("line2".into())],
        vec![Some("2".into()), Some("new line2".into())],
        vec![Some("3".into()), Some("new line3".into())],
    ];
    assert_eq!(apply_replace(&mut rows, 0, true), 1);
    assert_eq!(
        rows,
        vec![
            vec![Some("1".into()), Some("line1".into())],
            vec![Some("2".into()), Some("new line2".into())],
            vec![Some("3".into()), Some("new line3".into())]
        ]
    );
}

#[test]
fn go_commit_b61f02c672_reuses_shared_store_for_load_data_replace_cases() {
    // The Go regression moved this test onto the package-level store. Keep both
    // LOAD DATA invocations on the same table state so the second case observes
    // the rows committed by the first instead of starting from a fresh store.
    let mut rows = vec![
        vec![Some("1".into()), Some("val 1".into())],
        vec![Some("2".into()), Some("val 2".into())],
    ];

    rows.extend([
        vec![Some("1".into()), Some("line1".into())],
        vec![Some("2".into()), Some("line2".into())],
    ]);
    assert_eq!(apply_replace(&mut rows, 0, true), 2);
    assert_eq!(
        rows,
        vec![
            vec![Some("1".into()), Some("line1".into())],
            vec![Some("2".into()), Some("line2".into())],
        ]
    );

    rows.extend([
        vec![Some("2".into()), Some("new line2".into())],
        vec![Some("3".into()), Some("new line3".into())],
    ]);
    assert_eq!(apply_replace(&mut rows, 0, true), 1);
    assert_eq!(
        rows,
        vec![
            vec![Some("1".into()), Some("line1".into())],
            vec![Some("2".into()), Some("new line2".into())],
            vec![Some("3".into()), Some("new line3".into())],
        ]
    );
}

#[test]
fn test_load_data_overflow_bigint_unsigned() {
    // 无符号 BIGINT 转换对负数钳制为 0，对正向溢出钳制为 u64 上界，并产生警告标记。
    let values = ["-1", "-18446744073709551615", "-18446744073709551616"];
    for value in values {
        assert_eq!(parse_unsigned_bigint(value), (0, true));
    }
    assert_eq!(parse_unsigned_bigint("-9223372036854775809"), (0, true));
    assert_eq!(
        parse_unsigned_bigint("18446744073709551616"),
        (u64::MAX, true)
    );
}

#[test]
fn test_load_data_with_uppercase_user_vars() {
    // 用户变量名大小写不影响 SET 表达式；这里用倍率模型验证逐行求值结果。
    let rows = parse_load_data(b"1\n2\n", &LoadDataConfig::default()).unwrap();
    let mapped = map_columns(&rows, &[0], Some(100), false).unwrap();
    assert_eq!(
        mapped,
        vec![vec![Some("100".into())], vec![Some("200".into())]]
    );
}

#[test]
fn test_load_data_into_partitioned_table() {
    // 模拟 RANGE 分区上界路由，边界值 7 应进入下一个“小于 11”的分区。
    let config = LoadDataConfig::default().terminated_by(",");
    let rows = parse_load_data(b"1,2\n3,4\n5,6\n7,8\n9,10\n", &config).unwrap();
    let partitions: Vec<usize> = rows
        .iter()
        .map(|row| {
            let value = row[0].as_ref().unwrap().parse::<i64>().unwrap();
            [4, 7, 11].iter().position(|limit| value < *limit).unwrap()
        })
        .collect();
    assert_eq!(partitions, vec![0, 0, 1, 2, 2]);
}

#[test]
fn test_load_data_from_server_file() {
    assert_eq!(
        validate_load_data_options("remote.csv", false, None, &LoadDataConfig::default()),
        Err(LoadDataError::ServerFile)
    );
}

#[test]
fn test_fix56408() {
    // 回归 #56408：单批数据含连续重复键时仍应完整去重，且每个被替换行都计入删除数。
    let data = b"1|aa|beijing\n1|aa|beijing\n1|aa|beijing\n1|aa|beijing\n2|bb|shanghai\n2|bb|shanghai\n2|bb|shanghai\n3|cc|guangzhou\n";
    let config = LoadDataConfig::default().terminated_by("|");
    let mut rows = parse_load_data(data, &config).unwrap();
    assert_eq!(apply_replace(&mut rows, 0, true), 5);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[2],
        vec![
            Some("3".into()),
            Some("cc".into()),
            Some("guangzhou".into())
        ]
    );
}

#[test]
fn test_load_data_auto_random_error() {
    // LOAD DATA 显式写入 AUTO_RANDOM 列必须沿用执行器的拒绝契约。
    let rows = parse_load_data(b"1,2\n", &LoadDataConfig::default().terminated_by(",")).unwrap();
    assert_eq!(
        map_columns(&rows, &[0, 1], None, true),
        Err(LoadDataError::InvalidAutoRandom)
    );
}

#[test]
fn test_load_data_low_priority_sets_kv_low_priority() {
    // LOW_PRIORITY 同时下推到 KV 读写，但不改变打开、事务提交和 reader 清理顺序。
    let mut lifecycle = LoadDataLifecycle::default();
    lifecycle.run(true);
    assert_eq!(
        lifecycle.events,
        vec![
            LoadDataEvent::OpenFile,
            LoadDataEvent::Begin,
            LoadDataEvent::KvReadLow,
            LoadDataEvent::KvWriteLow,
            LoadDataEvent::Commit,
            LoadDataEvent::CloseReader,
        ]
    );
    assert!(lifecycle.begin_before_commit());
}
