// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// profile 迁移单测：protobuf/gzip 火焰图、小百分比格式、goroutine 文本与畸形 profile。
//
// 对照 Go 侧行序、树符号、错误文案；用手工构造的双栈 Profile 与 fixture 校验。

use std::io::Write;

use crate::{Collector, ProfileError};
use flate2::Compression;
use flate2::write::GzEncoder;
use pprof::protos::{Function, Line, Location, Message, Profile, Sample, ValueType};
use types;
use types::datum::{Datum, KindInt64, KindString};

/// 构造带 id / 名称下标 / 文件下标的 Function。
fn function(id: u64, name: i64, filename: i64) -> Function {
    Function {
        id,
        name,
        system_name: name,
        filename,
        start_line: 0,
    }
}

/// 构造单行 Location（映射 id 为 0）。
fn location(id: u64, function_id: u64, line: i64) -> Location {
    Location {
        id,
        mapping_id: 0,
        address: 0,
        line: vec![Line { function_id, line }],
        is_folded: false,
    }
}

/// 构造两条独立调用栈（70/30）的最小合法 Profile。
fn profile_with_two_stacks() -> Profile {
    Profile {
        sample_type: vec![ValueType { ty: 1, unit: 2 }],
        sample: vec![
            Sample {
                location_id: vec![1, 10],
                value: vec![70],
                label: Vec::new(),
            },
            Sample {
                location_id: vec![2, 20],
                value: vec![30],
                label: Vec::new(),
            },
        ],
        location: vec![
            location(1, 1, 11),
            location(10, 10, 110),
            location(2, 2, 22),
            location(20, 20, 220),
        ],
        function: vec![
            function(1, 3, 4),
            function(10, 5, 6),
            function(2, 7, 8),
            function(20, 9, 10),
        ],
        string_table: vec![
            "".into(),
            "samples".into(),
            "count".into(),
            "leaf-a".into(),
            "a.rs".into(),
            "root-a".into(),
            "root_a.rs".into(),
            "leaf-b".into(),
            "b.rs".into(),
            "root-b".into(),
            "root_b.rs".into(),
        ],
        ..Profile::default()
    }
}

/// 将一行 Datum 转为可比较的字符串向量。
fn row_strings(row: &[Datum]) -> Vec<String> {
    row.iter()
        .map(|datum| {
            let kind = datum.Kind();
            if kind == KindString {
                datum.GetString()
            } else if kind == KindInt64 {
                datum.GetInt64().to_string()
            } else {
                panic!("unexpected datum kind {kind}")
            }
        })
        .collect()
}

/// 断言 Result 为 Err 并返回错误，便于检查文案。
fn expect_profile_error(result: Result<Vec<Vec<Datum>>, ProfileError>) -> ProfileError {
    match result {
        Ok(_) => panic!("expected profile operation to fail"),
        Err(error) => error,
    }
}

/// 原始 protobuf 与 gzip 输入应得到相同的火焰图行序与内容。
#[test]
fn protobuf_and_gzip_profiles_match_go_flamegraph_order_and_rows() {
    let profile = profile_with_two_stacks();
    let raw = profile.encode_to_vec();
    let collector = Collector::default();

    let rows = collector.ProfileReaderToDatums(raw.as_slice()).unwrap();
    let actual: Vec<Vec<String>> = rows.iter().map(|row| row_strings(row)).collect();
    assert_eq!(
        actual,
        vec![
            vec!["root", "100%", "100%", "0", "0", "root"],
            vec!["├─root-a", "70.00%", "70.00%", "1", "1", "root_a.rs:110"],
            vec!["│ └─leaf-a", "70.00%", "100%", "1", "2", "a.rs:11"],
            vec!["└─root-b", "30.00%", "30.00%", "2", "1", "root_b.rs:220"],
            vec!["  └─leaf-b", "30.00%", "100%", "2", "2", "b.rs:22"],
        ]
    );

    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&raw).unwrap();
    let compressed = gzip.finish().unwrap();
    let gzip_rows = collector
        .ProfileReaderToDatums(compressed.as_slice())
        .unwrap();
    assert_eq!(
        gzip_rows
            .iter()
            .map(|row| row_strings(row))
            .collect::<Vec<_>>(),
        actual
    );
}

/// fixture 行数/首末行与 Go 一致；极小占比格式化为两位有效数字（如 0.1%）。
#[test]
fn fixture_and_small_percentages_match_go_formatting() {
    let collector = Collector::default();
    let fixture = include_bytes!("testdata/test.pprof");
    let rows = collector.ProfileReaderToDatums(fixture.as_slice()).unwrap();
    assert_eq!(rows.len(), 41);
    assert_eq!(
        row_strings(&rows[1]),
        vec![
            "├─runtime.main",
            "87.50%",
            "87.50%",
            "1",
            "1",
            "c:/go/src/runtime/proc.go:203"
        ]
    );
    assert_eq!(
        row_strings(rows.last().unwrap()),
        vec![
            "          └─runtime.findrunnable",
            "6.25%",
            "100%",
            "3",
            "6",
            "c:/go/src/runtime/proc.go:2170"
        ]
    );

    let mut profile = profile_with_two_stacks();
    profile.sample[0].value[0] = 999;
    profile.sample[1].value[0] = 1;
    let rows = collector
        .ProfileReaderToDatums(profile.encode_to_vec().as_slice())
        .unwrap();
    assert_eq!(row_strings(&rows[3])[1], "0.1%");
}

/// goroutine 文本：树符号/状态列正确，缺冒号或非法 id 时错误与 Go 对齐。
#[test]
fn goroutine_text_matches_go_headers_tree_markers_and_errors() {
    let input = b"goroutine 18 [running]:\nmain.first()\n /tmp/main.go:1\nmain.second()\n /tmp/main.go:2\nmain.third()\n /tmp/main.go:3\n\ngoroutine 19 [IO wait]:\nother.wait()\n /tmp/other.go:4\n";
    let collector = Collector::default();
    let rows = collector.ParseGoroutines(input.as_slice()).unwrap();
    let actual: Vec<Vec<String>> = rows.iter().map(|row| row_strings(row)).collect();
    assert_eq!(
        actual,
        vec![
            vec!["main.first()", "18", "running", "/tmp/main.go:1"],
            vec!["├─main.second()", "18", "running", "/tmp/main.go:2"],
            vec!["└─main.third()", "18", "running", "/tmp/main.go:3"],
            vec!["other.wait()", "19", "IO wait", "/tmp/other.go:4"],
        ]
    );

    let err = expect_profile_error(
        collector.ParseGoroutines(b"goroutine 18 [running]\nframe\n file".as_slice()),
    );
    assert!(
        err.to_string()
            .contains("goroutine incompatible with current go version")
    );
    let err = expect_profile_error(
        collector.ParseGoroutines(b"goroutine nope [running]:\nframe\n file:1".as_slice()),
    );
    assert!(err.to_string().contains("invalid goroutine id: nope"));
}

/// 空输入、值个数不匹配、不一致 function、坏 gzip 均应按 Go checkValid 拒绝。
#[test]
fn malformed_profiles_are_rejected_like_go_check_valid() {
    let collector = Collector::default();
    let err = expect_profile_error(collector.ProfileReaderToDatums([].as_slice()));
    assert!(err.to_string().contains("empty input file"));

    let mut profile = profile_with_two_stacks();
    profile.sample[0].value.push(1);
    let err =
        expect_profile_error(collector.ProfileReaderToDatums(profile.encode_to_vec().as_slice()));
    assert!(err.to_string().contains("sample has 2 values vs. 1 types"));

    let mut profile = profile_with_two_stacks();
    profile.location[0].line[0].function_id = 999;
    let err =
        expect_profile_error(collector.ProfileReaderToDatums(profile.encode_to_vec().as_slice()));
    assert!(err.to_string().contains("inconsistent function"));

    let err = expect_profile_error(collector.ProfileReaderToDatums([0x1f, 0x8b, 0, 0].as_slice()));
    assert!(err.to_string().contains("decompressing profile"));
}
