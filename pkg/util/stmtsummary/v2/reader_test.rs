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

// stmtsummary v2 读取器单元测试。
//
// 覆盖时间区间重叠、StmtFile 打开与非法行跳过、历史文件枚举过滤、
// StmtChecker 权限/digest/时间、MemReader 与 HistoryReader 的 digest/时间范围行为。

#![allow(non_snake_case)]

use chrono::TimeZone;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::sync::Mutex;
use task_stmtsummary_v2::*;

/// 串行化依赖全局日志路径的文件测试，避免互相覆盖。
static FILE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn go_merge_38_history_reader_open_ended_ranges() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    setStmtSummaryFilename(&active);
    let intervals = [
        (1672120800_i64, 1672121400_i64, "history_early"),
        (1672135200, 1672135800, "history_middle"),
        (1672149600, 1672150200, "history_late"),
    ];
    for (begin, end, digest) in intervals {
        let suffix = chrono::Local
            .timestamp_opt(end, 0)
            .single()
            .unwrap()
            .format("%Y-%m-%dT%H-%M-%S.000");
        let rotated = dir.path().join(format!("tidb-statements-{suffix}.log"));
        fs::write(
            rotated,
            format!(
                "{{\"begin\":{begin},\"end\":{end},\"digest\":\"{digest}\",\"exec_count\":1}}\n"
            ),
        )
        .unwrap();
    }
    let columns = [column(DigestStr)];
    let digests = Some(HashSet::from([
        "history_early".into(),
        "history_middle".into(),
        "history_late".into(),
    ]));
    assert_eq!(
        history_digests(
            &columns,
            digests.clone(),
            vec![StmtTimeRange {
                Begin: 1672128000,
                End: 0
            }]
        ),
        ["history_late", "history_middle"]
    );
    assert_eq!(
        history_digests(
            &columns,
            digests,
            vec![StmtTimeRange {
                Begin: 0,
                End: 1672142400
            }]
        ),
        ["history_early", "history_middle"]
    );
}

#[test]
fn history_reader_preserves_ia_exec_count() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    setStmtSummaryFilename(&active);
    fs::write(
        &active,
        b"{\"begin\":1,\"end\":2,\"digest\":\"d\",\"ia_exec_count\":3}\n",
    )
    .unwrap();
    let mut reader = NewHistoryReader(
        &[column(DigestStr), column(IAExecCountStr)],
        "",
        chrono_tz::UTC,
        None,
        false,
        None,
        Vec::new(),
        2,
    )
    .unwrap();
    let rows = read_all_rows(&mut reader);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].GetString(), "d");
    assert_eq!(rows[0][1].GetInt64(), 3);
}

#[test]
fn go_merge_37_pins_current_inode_across_rotation() {
    use crate::reader::StmtFiles;
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    for rotate_after_snapshot in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join("tidb-statements.log");
        let rotated = dir
            .path()
            .join("tidb-statements-2022-12-27T16-21-20.245.log");
        setStmtSummaryFilename(&active);
        fs::write(&active, b"{\"begin\":1,\"end\":2,\"digest\":\"old\"}\n").unwrap();
        let rotate = || {
            fs::rename(&active, &rotated).unwrap();
            fs::write(&active, b"{\"begin\":3,\"end\":4,\"digest\":\"new\"}\n").unwrap();
        };
        let mut files = StmtFiles::newWithReadDir(|directory| {
            if !rotate_after_snapshot {
                rotate();
            }
            let entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
            if rotate_after_snapshot {
                rotate();
            }
            Ok(entries)
        })
        .unwrap();
        assert_eq!(files.files.len(), 1);
        let pinned = files.files[0].opened.as_mut().unwrap();
        let mut contents = String::new();
        pinned.file.read_to_string(&mut contents).unwrap();
        assert!(contents.contains("\"old\""));
        assert!(!contents.contains("\"new\""));
    }
}

#[test]
fn go_merge_37_history_reader_bounds_open_files() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    setStmtSummaryFilename(&active);
    let first = 1_672_444_800_i64;
    for index in 0..32 {
        let end = first + index * 7_200;
        let name = chrono::DateTime::from_timestamp(end, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format(logFileTimeFormat)
            .to_string();
        let path = dir.path().join(format!("tidb-statements-{name}.log"));
        let line = format!(
            "{{\"begin\":{},\"end\":{end},\"digest\":\"d\"}}\n",
            end - 60
        );
        fs::write(path, line.repeat(20_000)).unwrap();
    }
    fs::write(
        &active,
        format!("{{\"begin\":{},\"end\":{first}}}\n", first - 60),
    )
    .unwrap();
    let count_fds = || fs::read_dir("/dev/fd").ok().map(|entries| entries.count());
    let before = count_fds();
    let mut reader = NewHistoryReader(
        &[column(DigestStr)],
        "",
        chrono_tz::UTC,
        None,
        false,
        None,
        vec![StmtTimeRange {
            Begin: first - 100,
            End: 0,
        }],
        2,
    )
    .unwrap();
    if let (Some(before), Some(after)) = (before, count_fds()) {
        assert!(
            after <= before + 4,
            "opened {} file descriptors",
            after - before
        );
    }
    reader.Close().unwrap();
}

/// 构造仅含列名的 ColumnInfo 桩。
fn column(name: &str) -> model::ColumnInfo {
    let mut column = model::ColumnInfo::default();
    column.Name.O = name.to_owned();
    column
}

/// 排空 HistoryReader 全部批次并 Close。
fn read_all_rows(reader: &mut HistoryReader) -> Vec<Vec<types::Datum>> {
    let mut rows = Vec::new();
    while let Some(batch) = reader.Rows().unwrap() {
        rows.extend(batch);
    }
    reader.Close().unwrap();
    rows
}

/// 按 digest/时间过滤读取历史，返回排序后的 digest 列值。
fn history_digests(
    columns: &[model::ColumnInfo],
    digests: Option<HashSet<String>>,
    ranges: Vec<StmtTimeRange>,
) -> Vec<String> {
    let mut reader = NewHistoryReader(
        columns,
        "",
        chrono_tz::Asia::Shanghai,
        None,
        false,
        digests,
        ranges,
        2,
    )
    .unwrap();
    let mut values: Vec<_> = read_all_rows(&mut reader)
        .into_iter()
        .map(|row| row[0].GetString())
        .collect();
    values.sort();
    values
}

/// 校验闭合/开放时间区间的重叠判定。
#[test]
fn TestTimeRangeOverlap() {
    assert!(!timeRangeOverlap(1, 2, 3, 4));
    assert!(!timeRangeOverlap(3, 4, 1, 2));
    assert!(timeRangeOverlap(1, 2, 2, 3));
    assert!(timeRangeOverlap(1, 3, 2, 4));
    assert!(timeRangeOverlap(2, 4, 1, 3));
    assert!(timeRangeOverlap(1, 0, 3, 4));
    assert!(timeRangeOverlap(1, 0, 2, 0));
}

/// 校验轮转文件 begin/end 解析，且打开后可读首行。
#[test]
fn TestStmtFile() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    let rotated = dir
        .path()
        .join("tidb-statements-2022-12-27T16-21-20.245.log");
    setStmtSummaryFilename(&active);
    fs::write(
        &rotated,
        "{\"begin\":1,\"end\":2}\n{\"begin\":3,\"end\":4}\n",
    )
    .unwrap();

    let opened = openStmtFile(&rotated).unwrap();
    assert_eq!(opened.begin, 1);
    let expected =
        chrono::NaiveDateTime::parse_from_str("2022-12-27T16-21-20.245", logFileTimeFormat)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
            .timestamp();
    assert_eq!(opened.end, expected);
    let mut first = String::new();
    BufReader::new(opened.file).read_line(&mut first).unwrap();
    assert_eq!(first.trim_end(), r#"{"begin":1,"end":2}"#);
}

/// 首行非法时跳过并取首条合法 JSON 的 begin。
#[test]
fn TestStmtFileInvalidLine() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    let rotated = dir
        .path()
        .join("tidb-statements-2022-12-27T16-21-20.245.log");
    setStmtSummaryFilename(&active);
    fs::write(
        &rotated,
        "invalid line\n{\"begin\":1,\"end\":2}\n{\"begin\":3,\"end\":4}\n",
    )
    .unwrap();

    let opened = openStmtFile(&rotated).unwrap();
    assert_eq!(opened.begin, 1);
    assert!(opened.end > 0);
}

/// 校验多文件枚举与时间范围裁剪后的 digest 集合。
#[test]
fn TestStmtFiles() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    let rotated = dir
        .path()
        .join("tidb-statements-2022-12-27T16-21-20.245.log");
    setStmtSummaryFilename(&active);
    let t1 = 1_672_129_280_i64;
    fs::write(
        &rotated,
        format!(
            "{{\"begin\":{},\"end\":{},\"digest\":\"old\"}}\n{{\"begin\":{},\"end\":{},\"digest\":\"edge_rotated\"}}\n",
            t1 - 760,
            t1 - 750,
            t1 - 10,
            t1
        ),
    )
    .unwrap();
    fs::write(
        &active,
        format!(
            "{{\"begin\":{},\"end\":{},\"digest\":\"edge_active\"}}\n{{\"begin\":{},\"end\":{},\"digest\":\"future\"}}\n",
            t1 - 10,
            t1,
            t1 + 100,
            t1 + 110
        ),
    )
    .unwrap();
    let columns = [column(DigestStr)];

    assert_eq!(history_digests(&columns, None, vec![]).len(), 4);
    assert_eq!(
        history_digests(
            &columns,
            None,
            vec![StmtTimeRange {
                Begin: t1 - 10,
                End: t1 - 9
            }],
        ),
        ["edge_active", "edge_rotated"]
    );
    assert_eq!(
        history_digests(
            &columns,
            None,
            vec![StmtTimeRange {
                Begin: 0,
                End: t1 - 10
            }],
        )
        .len(),
        3
    );
    assert_eq!(
        history_digests(
            &columns,
            None,
            vec![StmtTimeRange {
                Begin: 0,
                End: t1 - 11
            }],
        ),
        ["old"]
    );
    assert!(history_digests(&columns, None, vec![StmtTimeRange { Begin: 0, End: 1 }]).is_empty());
    assert_eq!(
        history_digests(
            &columns,
            None,
            vec![StmtTimeRange {
                Begin: t1 + 1,
                End: 0
            }],
        ),
        ["future"]
    );
}

/// 校验权限、digest 白名单、时间有效与 needStop。
#[test]
fn TestStmtChecker() {
    let checker = StmtChecker::default();
    assert!(checker.hasPrivilege(&HashSet::new()));

    let checker = StmtChecker::new(Some("user1".into()), false, None, vec![]);
    assert!(!checker.hasPrivilege(&HashSet::new()));
    assert!(!checker.hasPrivilege(&HashSet::from(["user2".into()])));
    assert!(checker.hasPrivilege(&HashSet::from(["user1".into(), "user2".into()])));

    let checker = StmtChecker::default();
    assert!(checker.isDigestValid("digest1"));
    let checker = StmtChecker::new(None, false, Some(HashSet::from(["digest2".into()])), vec![]);
    assert!(!checker.isDigestValid("digest1"));
    assert!(checker.isDigestValid("digest2"));
    let checker = StmtChecker::new(
        None,
        false,
        Some(HashSet::from(["digest1".into(), "digest2".into()])),
        vec![],
    );
    assert!(checker.isDigestValid("digest1"));
    assert!(checker.isDigestValid("digest2"));

    let checker = StmtChecker::default();
    assert!(checker.isTimeValid(1, 2));
    assert!(!checker.needStop(2));
    assert!(!checker.needStop(3));
    let checker = StmtChecker::new(None, false, None, vec![StmtTimeRange { Begin: 1, End: 2 }]);
    assert!(checker.isTimeValid(1, 2));
    assert!(!checker.isTimeValid(3, 4));
    assert!(!checker.needStop(2));
    assert!(checker.needStop(3));
}

/// 固定返回单个窗口快照的内存源。
#[derive(Clone)]
struct TestMemorySource(MemWindowSnapshot);

impl MemorySummarySource for TestMemorySource {
    fn currentWindowSnapshot(&self) -> Option<MemWindowSnapshot> {
        Some(self.0.clone())
    }
}

/// 构造带 AuthUsers 的简易 StmtRecord。
fn record(digest: &str, count: i64) -> StmtRecord {
    StmtRecord {
        Digest: digest.into(),
        ExecCount: count,
        AuthUsers: HashSet::from(["user".into()]),
        ..Default::default()
    }
}

/// 校验 MemReader 行数（含淘汰行）及 StmtSummary 淘汰输出。
#[test]
fn TestMemReader() {
    let source = TestMemorySource(MemWindowSnapshot {
        begin: 1,
        records: vec![
            record("digest3", 2),
            record("digest4", 2),
            record("digest5", 2),
        ],
        evicted: Some(record("", 4)),
    });
    let columns = [column(DigestStr), column(ExecCountStr)];
    let reader = NewMemReader(
        Some(&source),
        &columns,
        "",
        chrono_tz::Asia::Shanghai,
        None,
        false,
        None,
        vec![],
    );
    let rows = reader.Rows();
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|row| row.len() == columns.len()));

    let summary = NewStmtSummary4Test(3);
    for digest in ["digest1", "digest2", "digest3", "digest4", "digest5"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    let evicted = summary.Evicted().unwrap();
    assert_eq!(evicted.len(), 3);
    assert_eq!(evicted[2].GetInt64(), 2);
    summary.Close();
}

/// 校验历史读取跳过 evicted，并按 digest/时间过滤。
#[test]
fn TestHistoryReader() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    let rotated = dir
        .path()
        .join("tidb-statements-2022-12-27T16-21-20.245.log");
    setStmtSummaryFilename(&active);
    fs::write(
        &rotated,
        "{\"begin\":1672128520,\"end\":1672128530,\"digest\":\"digest1\",\"exec_count\":10}\n\
         {\"begin\":1672129270,\"end\":1672129280,\"digest\":\"digest2\",\"exec_count\":20}\n\
         {\"begin\":1672129270,\"end\":1672129280,\"digest\":\"evicted_digest\",\"exec_count\":99,\"evicted\":true}\n",
    )
    .unwrap();
    fs::write(
        &active,
        "{\"begin\":1672129270,\"end\":1672129280,\"digest\":\"digest2\",\"exec_count\":30}\n\
         {\"begin\":1672129380,\"end\":1672129390,\"digest\":\"digest3\",\"exec_count\":40}\n",
    )
    .unwrap();
    let columns = [column(DigestStr), column(ExecCountStr)];

    assert_eq!(history_digests(&columns, None, vec![]).len(), 4);
    assert_eq!(
        history_digests(&columns, Some(HashSet::from(["digest2".into()])), vec![]),
        ["digest2", "digest2"]
    );
    let cases = [
        (
            StmtTimeRange {
                Begin: 0,
                End: 1_672_128_519,
            },
            0,
        ),
        (
            StmtTimeRange {
                Begin: 0,
                End: 1_672_129_269,
            },
            1,
        ),
        (
            StmtTimeRange {
                Begin: 0,
                End: 1_672_129_270,
            },
            3,
        ),
        (
            StmtTimeRange {
                Begin: 0,
                End: 1_672_129_380,
            },
            4,
        ),
        (
            StmtTimeRange {
                Begin: 1_672_129_270,
                End: 1_672_129_380,
            },
            3,
        ),
        (
            StmtTimeRange {
                Begin: 1_672_129_390,
                End: 0,
            },
            1,
        ),
        (
            StmtTimeRange {
                Begin: 1_672_129_391,
                End: 0,
            },
            0,
        ),
        (StmtTimeRange { Begin: 0, End: 0 }, 4),
    ];
    for (range, expected) in cases {
        let rows = history_digests(&columns, None, vec![range]);
        assert_eq!(rows.len(), expected, "range {range:?}");
    }
}

/// 历史读取应跳过损坏行，仅保留合法 JSON digest。
#[test]
fn TestHistoryReaderInvalidLine() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    setStmtSummaryFilename(&active);
    fs::write(
        &active,
        "invalid header line\n\
         {\"begin\":1672129270,\"end\":1672129280,\"digest\":\"digest2\",\"exec_count\":30}\n\
         corrupted line\n\
         {\"begin\":1672129380,\"end\":1672129390,\"digest\":\"digest3\",\"exec_count\":40}\n\
         invalid footer line",
    )
    .unwrap();
    let columns = [column(DigestStr), column(ExecCountStr)];
    let rows = history_digests(&columns, None, vec![]);
    assert_eq!(rows, ["digest2", "digest3"]);
}

#[test]
fn port_44538075bd_extract_digest() {
    use crate::reader::extractDigest;
    for (raw, expected) in [
        (r#"{"begin":1,"digest":"d1","end":2}"#, "d1"),
        (r#"{ "digest" : "d2" }"#, "d2"),
        (r#"{"schema_name":"a\"b\u4e2d","digest":"d3"}"#, "d3"),
        (
            r#"{"arr":[1,2,{"x":"y"}],"obj":{"k":"v"},"digest":"d4"}"#,
            "d4",
        ),
        (r#"{"digest":"miss","digest":"target"}"#, "target"),
        (r#"{"digest":"miss","DIGEST":"target"}"#, "target"),
        (r#"{"digest":"miss","digeſt":"target"}"#, "target"),
    ] {
        assert_eq!(extractDigest(raw.as_bytes()), Some(expected), "{raw}");
    }
    let large = format!(
        r#"{{"begin":1,"end":2,"digest":"target","sample_sql":"{}"}}"#,
        "s".repeat(4096)
    );
    assert_eq!(extractDigest(large.as_bytes()), Some("target"));
    for raw in [
        r#"{"begin":1}"#,
        r#"{"digest":"miss","di\u0067est":"target"}"#,
        r#"{"digest":"a\"b"}"#,
        r#"{"digest":null}"#,
        r#"{"digest" "d"}"#,
        r#""digest""#,
        r#"{"begin":1,"end":2"#,
        r#"{"arr":[1,2"#,
    ] {
        assert_eq!(extractDigest(raw.as_bytes()), None, "{raw}");
    }
    assert_eq!(extractDigest(b"{\"digest\":\"a\xffb\"}"), None);
}

#[test]
fn port_44538075bd_history_digest_filter() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    setStmtSummaryFilename(&active);
    let lines = concat!(
        "{\"begin\":1,\"end\":2,\"digest\":\"original\",\"exec_count\":1}\n",
        "{\"begin\":1,\"end\":2,\"digest\":\"miss\",\"digest\":\"target\",\"exec_count\":50}\n",
        "{\"begin\":1,\"end\":2,\"digest\":\"miss\",\"DIGEST\":\"target\",\"exec_count\":7}\n",
        "{\"begin\":1,\"end\":2,\"digest\":\"miss\",\"di\\u0067est\":\"target\"}\n",
        "{\"begin\":1,\"end\":2,\"digest\":\"tar\\u0067et\"}\n",
        "invalid line\n",
    );
    let mut bytes = lines.as_bytes().to_vec();
    bytes.extend_from_slice(b"{\"begin\":1,\"end\":2,\"digest\":\"a\xffb\",\"exec_count\":8}\n");
    fs::write(&active, bytes).unwrap();
    let columns = [column(DigestStr)];
    assert_eq!(
        history_digests(&columns, Some(HashSet::from(["target".into()])), vec![]),
        vec!["target"; 4]
    );
    assert_eq!(
        history_digests(&columns, Some(HashSet::from(["original".into()])), vec![]),
        ["original"]
    );
    assert_eq!(
        history_digests(&columns, Some(HashSet::from(["a�b".into()])), vec![]),
        ["a�b"]
    );
    assert!(
        history_digests(
            &columns,
            Some(HashSet::from(["target".into()])),
            vec![StmtTimeRange { Begin: 3, End: 4 }]
        )
        .is_empty()
    );
    assert_eq!(history_digests(&columns, None, vec![]).len(), 6);
}

#[test]
fn port_44538075bd_history_workload() {
    let _guard = FILE_TEST_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("tidb-statements.log");
    setStmtSummaryFilename(&active);
    for size in [1024, 32768] {
        for all_text in [false, true] {
            for percent in [0, 1, 50, 100] {
                let mut contents = String::new();
                for i in 0..200 {
                    let digest = if i % 100 < percent {
                        "target".into()
                    } else {
                        format!("digest_{i:06}")
                    };
                    let line = serde_json::json!({"begin":1672128520,"end":1672128580,
                        "digest":digest,"exec_count":10,"stmt_type":"Select",
                        "normalized_sql":"select ? from t","sample_sql":"s".repeat(size),
                        "sample_binary_plan":"p".repeat(size)});
                    contents.push_str(&line.to_string());
                    contents.push('\n');
                }
                fs::write(&active, contents).unwrap();
                let mut columns = vec![column(DigestStr), column(ExecCountStr)];
                if all_text {
                    columns.extend(
                        [
                            DigestTextStr,
                            BindingDigestTextStr,
                            QuerySampleTextStr,
                            PrevSampleTextStr,
                            PlanStr,
                            BinaryPlan,
                            PlanHint,
                            PlanCacheUnqualifiedLastReasonStr,
                        ]
                        .map(column),
                    );
                }
                for filter in [true, false] {
                    let mut reader = NewHistoryReader(
                        &columns,
                        "",
                        chrono_tz::UTC,
                        None,
                        false,
                        filter.then(|| HashSet::from(["target".into()])),
                        vec![],
                        2,
                    )
                    .unwrap();
                    let rows = read_all_rows(&mut reader);
                    assert_eq!(rows.len(), if filter { 200 * percent / 100 } else { 200 });
                    assert!(
                        rows.iter()
                            .all(|row| row.len() == columns.len() && row[1].GetInt64() == 10)
                    );
                    if filter {
                        assert!(rows.iter().all(|row| row[0].GetString() == "target"));
                    }
                    reader.Close().unwrap();
                }
            }
        }
    }
}
