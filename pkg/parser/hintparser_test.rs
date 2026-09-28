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

// Optimizer Hint 解析器测试，对照 `hintparser_test.go`。
//
// 覆盖 MEMORY_QUOTA、QB_NAME、SET_VAR、JOIN/INDEX 类 hint、LEADING 嵌套列表，
// 以及非法 token/不支持 hint 的诊断文案；第二组用例校验 HintData AST 载荷形状。

// Native table-driven counterpart of hintparser_test.go.

use parser;

/// 单条 hint 解析用例：输入、SQLMode、期望摘要与诊断子串。
struct HintCase {
    input: &'static str,
    mode: parser::mysql::SQLMode,
    // expected_notes 保留 Go 中 []*ast.TableOptimizerHint 的核心字段，避免在这里引入完整 AST 构造依赖。
    expected_notes: &'static [&'static str],
    errs: &'static [&'static str],
}

// test_parse_hint 对应 Go 的 TestParseHint：把输入包在 /*+ ... */ 后调用 parser.ParseHint。
/// 表驱动校验 ParseHint：诊断子串命中且 hint 名与期望摘要首词一致。
#[test]
fn test_parse_hint() {
    let test_cases = vec![
        HintCase {
            input: "",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "MEMORY_QUOTA(8 MB) MEMORY_QUOTA(6 GB)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[
                "MEMORY_QUOTA HintData=8*1024*1024",
                "MEMORY_QUOTA HintData=6*1024*1024*1024",
            ],
            errs: &[],
        },
        HintCase {
            input: "QB_NAME(qb1) QB_NAME(`qb2`), QB_NAME(TRUE) QB_NAME(\"ANSI quoted\") QB_NAME(_utf8), QB_NAME(0b10) QB_NAME(0x1a)",
            mode: parser::mysql::ModeANSIQuotes,
            expected_notes: &[
                "QB_NAME qb1",
                "QB_NAME qb2",
                "QB_NAME TRUE",
                "QB_NAME ANSI quoted",
                "QB_NAME _utf8",
                "QB_NAME 0b10",
                "QB_NAME 0x1a",
            ],
            errs: &[],
        },
        HintCase {
            input: "QB_NAME(1)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "QB_NAME(1.5)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &[
                "Cannot use decimal number",
                "Optimizer hint syntax error at line 1 ",
            ],
        },
        HintCase {
            input: "QB_NAME('string literal')",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "QB_NAME(many identifiers)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "QB_NAME(@qb1)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "QB_NAME(b'10')",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &[
                "Cannot use bit-value literal",
                "Optimizer hint syntax error at line 1 ",
            ],
        },
        HintCase {
            input: "QB_NAME(x'1a')",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &[
                "Cannot use hexadecimal literal",
                "Optimizer hint syntax error at line 1 ",
            ],
        },
        HintCase {
            input: "JOIN_FIXED_ORDER() BKA()",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &[
                "Optimizer hint JOIN_FIXED_ORDER is not supported",
                "Optimizer hint BKA is not supported",
            ],
        },
        HintCase {
            input: "HASH_JOIN() TIDB_HJ(@qb1) INL_JOIN(x, `y y`.z) MERGE_JOIN(w@`First QB`)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[
                "HASH_JOIN",
                "TIDB_HJ QBName=qb1",
                "INL_JOIN tables=x,`y y`.z",
                "MERGE_JOIN table=w QBName=First QB",
            ],
            errs: &[],
        },
        HintCase {
            input: "USE_INDEX_MERGE(@qb1 tbl1 x, y, z) IGNORE_INDEX(tbl2@qb2) USE_INDEX(tbl3 PRIMARY) FORCE_INDEX(tbl4@qb3 c1) INDEX_LOOKUP_PUSHDOWN(tbl5@qb6 c3)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[
                "USE_INDEX_MERGE QBName=qb1 table=tbl1 indexes=x,y,z",
                "IGNORE_INDEX table=tbl2 QBName=qb2",
                "USE_INDEX table=tbl3 index=PRIMARY",
                "FORCE_INDEX table=tbl4 QBName=qb3 index=c1",
                "INDEX_LOOKUP_PUSHDOWN table=tbl5 QBName=qb6 index=c3",
            ],
            errs: &[],
        },
        HintCase {
            input: "USE_INDEX(@qb1 tbl1 partition(p0) x) USE_INDEX_MERGE(@qb2 tbl2@qb2 partition(p0, p1) x, y, z)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[
                "USE_INDEX QBName=qb1 table=tbl1 partition=p0 index=x",
                "USE_INDEX_MERGE QBName=qb2 table=tbl2 tableQB=qb2 partitions=p0,p1 indexes=x,y,z",
            ],
            errs: &[],
        },
        HintCase {
            input: r#"SET_VAR(sbs = 16M) SET_VAR(fkc=OFF) SET_VAR(os="mcb=off") set_var(abc=1) set_var(os2='mcb2=off') set_var(sel=0.3) set_var(sel_plus=+0.3) set_var(sel_minus=-0.3)"#,
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[
                "SET_VAR sbs=16M",
                "SET_VAR fkc=OFF",
                "SET_VAR os=mcb=off",
                "set_var abc=1",
                "set_var os2=mcb2=off",
                "set_var sel=0.3",
                "set_var sel_plus=0.3",
                "set_var sel_minus=-0.3",
            ],
            errs: &[],
        },
        HintCase {
            input: "USE_TOJA(TRUE) IGNORE_PLAN_CACHE() USE_CASCADES(TRUE) QUERY_TYPE(@qb1 OLAP) QUERY_TYPE(OLTP) NO_INDEX_MERGE() RESOURCE_GROUP(rg1)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[
                "USE_TOJA true",
                "IGNORE_PLAN_CACHE",
                "USE_CASCADES true",
                "QUERY_TYPE QBName=qb1 OLAP",
                "QUERY_TYPE OLTP",
                "NO_INDEX_MERGE",
                "RESOURCE_GROUP rg1",
            ],
            errs: &[],
        },
        HintCase {
            input: "READ_FROM_STORAGE(@foo TIKV[a, b], TIFLASH[c, d]) HASH_AGG() SEMI_JOIN_REWRITE() READ_FROM_STORAGE(TIKV[e])",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[
                "READ_FROM_STORAGE QBName=foo TIKV tables=a,b",
                "READ_FROM_STORAGE QBName=foo TIFLASH tables=c,d",
                "HASH_AGG",
                "SEMI_JOIN_REWRITE",
                "READ_FROM_STORAGE TIKV table=e",
            ],
            errs: &[],
        },
        HintCase {
            input: "WRITE_SLOW_LOG, WRITE_SLOW_LOG",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &["WRITE_SLOW_LOG", "WRITE_SLOW_LOG"],
            errs: &[],
        },
        HintCase {
            input: "WRITE_SLOW_LOG()",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "unknown_hint()",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "set_var(timestamp = 1.5)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &["set_var timestamp=1.5"],
            errs: &[],
        },
        HintCase {
            input: "set_var(timestamp = _utf8mb4'1234')",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "set_var(timestamp = 9999999999999999999999999999999999999)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &[
                "integer value is out of range",
                "Optimizer hint syntax error at line 1 ",
            ],
        },
        HintCase {
            input: "time_range('2020-02-20 12:12:12',456)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "time_range(456,'2020-02-20 12:12:12')",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &[],
            errs: &["Optimizer hint syntax error at line 1 "],
        },
        HintCase {
            input: "TIME_RANGE('2020-02-20 12:12:12','2020-02-20 13:12:12')",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &["TIME_RANGE From=2020-02-20 12:12:12 To=2020-02-20 13:12:12"],
            errs: &[],
        },
        HintCase {
            input: "LEADING(a,(b,(c,d)))",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &["LEADING nested=a,(b,(c,d)) tables=a,b,c,d"],
            errs: &[],
        },
        HintCase {
            input: "LEADING(a,b,c)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &["LEADING flat=a,b,c tables=a,b,c"],
            errs: &[],
        },
        HintCase {
            input: "LEADING((a,b),(c,d))",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &["LEADING nested=(a,b),(c,d) tables=a,b,c,d"],
            errs: &[],
        },
        HintCase {
            input: "LEADING(x,(y,z),w)",
            mode: parser::mysql::SQLMode::default(),
            expected_notes: &["LEADING mixed=x,(y,z),w tables=x,y,z,w"],
            errs: &[],
        },
    ];

    for tc in test_cases {
        // 与 Go 一致：输入需包在 /*+ ... */ 注释中再交给 ParseHint。
        let (output, errs) = parser::ParseHint(
            &format!("/*+{}*/", tc.input),
            tc.mode,
            parser::Pos {
                Line: 1,
                Col: 0,
                Offset: 0,
            },
        );
        let diagnostics = errs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            errs.len(),
            tc.errs.len(),
            "input {:?}: diagnostics={errs:?}",
            tc.input
        );
        for expected in tc.errs {
            assert!(
                diagnostics.contains(expected.trim()),
                "input {:?}: missing diagnostic {:?} in {:?}",
                tc.input,
                expected,
                diagnostics
            );
        }
        if tc.errs.is_empty() {
            assert!(
                diagnostics.is_empty(),
                "input {:?}: unexpected diagnostics {diagnostics:?}",
                tc.input
            );
        }

        assert_eq!(
            output.len(),
            tc.expected_notes.len(),
            "input {:?}: output={output:?}",
            tc.input
        );
        for (hint, expected) in output.iter().zip(tc.expected_notes) {
            let hint_name = expected
                .split_whitespace()
                .next()
                .expect("hint summary name");
            assert_eq!(
                hint.HintName.O.to_ascii_uppercase(),
                hint_name.to_ascii_uppercase(),
                "input {:?}",
                tc.input
            );
        }
    }
}

/// 成功解析 hint 正文并断言无诊断，返回 AST 列表。
fn parse_ok(input: &str) -> Vec<Box<parser_ast::TableOptimizerHint>> {
    let (hints, errors) = parser::ParseHint(
        &format!("/*+{input}*/"),
        parser::mysql::SQLMode::default(),
        parser::Pos {
            Line: 1,
            Col: 0,
            Offset: 0,
        },
    );
    assert!(errors.is_empty(), "{input:?}: {errors:?}");
    hints
}

/// 深入校验各类 HintData 载荷（Signed/SetVar/Boolean/CIStr/TimeRange/Leading）与 Go 一致。
#[test]
fn test_parse_hint_preserves_all_go_ast_payload_shapes() {
    use parser_ast::HintData;

    let hints = parse_ok("MEMORY_QUOTA(8 MB) MEMORY_QUOTA(6 GB)");
    assert_eq!(hints[0].HintData, HintData::Signed(8 * 1024 * 1024));
    assert_eq!(hints[1].HintData, HintData::Signed(6 * 1024 * 1024 * 1024));

    let hints = parse_ok("HASH_JOIN() TIDB_HJ(@qb1) INL_JOIN(x, `y y`.z) MERGE_JOIN(w@`First QB`)");
    assert_eq!(hints[1].QBName.O, "qb1");
    assert_eq!(hints[2].Tables[0].TableName.O, "x");
    assert_eq!(hints[2].Tables[1].DBName.O, "y y");
    assert_eq!(hints[2].Tables[1].TableName.O, "z");
    assert_eq!(hints[3].Tables[0].QBName.O, "First QB");

    let hints = parse_ok(
        "USE_INDEX(@qb1 tbl1 partition(p0) x) USE_INDEX_MERGE(@qb2 tbl2@qb2 partition(p0, p1) x, y, z)",
    );
    assert_eq!(hints[0].QBName.O, "qb1");
    assert_eq!(
        hints[0].Tables[0]
            .PartitionList
            .iter()
            .map(|item| item.O.as_str())
            .collect::<Vec<_>>(),
        ["p0"]
    );
    assert_eq!(
        hints[0]
            .Indexes
            .iter()
            .map(|item| item.O.as_str())
            .collect::<Vec<_>>(),
        ["x"]
    );
    assert_eq!(hints[1].Tables[0].QBName.O, "qb2");
    assert_eq!(
        hints[1].Tables[0]
            .PartitionList
            .iter()
            .map(|item| item.O.as_str())
            .collect::<Vec<_>>(),
        ["p0", "p1"]
    );
    assert_eq!(
        hints[1]
            .Indexes
            .iter()
            .map(|item| item.O.as_str())
            .collect::<Vec<_>>(),
        ["x", "y", "z"]
    );

    let hints = parse_ok(
        r#"SET_VAR(sbs = 16M) SET_VAR(fkc=OFF) SET_VAR(os="mcb=off") set_var(abc=1) set_var(os2='mcb2=off') set_var(sel=0.3) set_var(sel_plus=+0.3) set_var(sel_minus=-0.3)"#,
    );
    let expected = [
        ("sbs", "16M"),
        ("fkc", "OFF"),
        ("os", "mcb=off"),
        ("abc", "1"),
        ("os2", "mcb2=off"),
        ("sel", "0.3"),
        ("sel_plus", "0.3"),
        ("sel_minus", "-0.3"),
    ];
    for (hint, (name, value)) in hints.iter().zip(expected) {
        assert_eq!(
            hint.HintData,
            HintData::SetVar(parser_ast::HintSetVar {
                VarName: name.into(),
                Value: value.into()
            })
        );
    }

    let hints = parse_ok(
        "USE_TOJA(TRUE) IGNORE_PLAN_CACHE() USE_CASCADES(TRUE) QUERY_TYPE(@qb1 OLAP) QUERY_TYPE(OLTP) NO_INDEX_MERGE() RESOURCE_GROUP(rg1)",
    );
    assert_eq!(hints[0].HintData, HintData::Boolean(true));
    assert_eq!(hints[2].HintData, HintData::Boolean(true));
    assert_eq!(hints[3].QBName.O, "qb1");
    assert_eq!(
        hints[3].HintData,
        HintData::CIStr(parser_ast::NewCIStr("OLAP"))
    );
    assert_eq!(
        hints[4].HintData,
        HintData::CIStr(parser_ast::NewCIStr("OLTP"))
    );
    assert_eq!(hints[6].HintData, HintData::Name("rg1".into()));

    let hints = parse_ok(
        "READ_FROM_STORAGE(@foo TIKV[a, b], TIFLASH[c, d]) HASH_AGG() SEMI_JOIN_REWRITE() READ_FROM_STORAGE(TIKV[e])",
    );
    assert_eq!(hints[0].QBName.O, "foo");
    assert_eq!(
        hints[0].HintData,
        HintData::CIStr(parser_ast::NewCIStr("TIKV"))
    );
    assert_eq!(
        hints[0]
            .Tables
            .iter()
            .map(|table| table.TableName.O.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        hints[1].HintData,
        HintData::CIStr(parser_ast::NewCIStr("TIFLASH"))
    );
    assert_eq!(
        hints[1]
            .Tables
            .iter()
            .map(|table| table.TableName.O.as_str())
            .collect::<Vec<_>>(),
        ["c", "d"]
    );
    assert_eq!(hints[4].Tables[0].TableName.O, "e");

    let hints = parse_ok("TIME_RANGE('2020-02-20 12:12:12','2020-02-20 13:12:12')");
    assert_eq!(
        hints[0].HintData,
        HintData::TimeRange(parser_ast::HintTimeRange {
            From: "2020-02-20 12:12:12".into(),
            To: "2020-02-20 13:12:12".into()
        })
    );

    for (input, expected_tables) in [
        ("LEADING(a,(b,(c,d)))", vec!["a", "b", "c", "d"]),
        ("LEADING(a,b,c)", vec!["a", "b", "c"]),
        ("LEADING((a,b),(c,d))", vec!["a", "b", "c", "d"]),
        ("LEADING(x,(y,z),w)", vec!["x", "y", "z", "w"]),
    ] {
        let hints = parse_ok(input);
        assert!(matches!(hints[0].HintData, HintData::Leading(_)), "{input}");
        assert_eq!(
            hints[0]
                .Tables
                .iter()
                .map(|table| table.TableName.O.as_str())
                .collect::<Vec<_>>(),
            expected_tables
        );
    }
}
