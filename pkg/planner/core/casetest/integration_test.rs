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

// planner casetest 集成回归：TiFlash、verbose explain、fix-control 与历史 issue。
//
// 对应 Go `integration_test.go`：fixture 用例逐条校验输入/输出录制契约；当前窄运行时
// 能覆盖的事务、无符号范围和排序走真实 TestKit，其余 SQL 通过真实 parser 保持场景接线。
//
// TiFlash：列存加速引擎，可承接 MPP 与 isolation read；PointGet 通常不应被选中。
// NormalizeDigest：将 SQL 规范化并计算 digest，用于语句摘要与计划缓存键。

// Go/Rust 场景映射：
// - 7 个 testdata 场景覆盖 verbose explain、isolation read、连续 selection、分区 scan、
//   fine-grained shuffle 与 extra-column prune；标准/Cascades 输出均校验 SQL 和字段形状。
// - Fix43817/45132 保留开关前后查询、ANALYZE 与 EXPLAIN 的完整语句顺序。
// - JSON member-of 用例保留多值索引 DDL、ANALYZE、TableReader 与 IndexMerge 查询。
// - 综合回归通过真实 TestKit 执行 issue 33175 的事务、rollback、u64 边界和排序；其余
//   CTE、分区与多值索引 SQL 走真实 parser，避免用固定物理计划文本伪装完整运行时。
//
// 当前 TestKit 尚无 Go RunTestUnderCascadesWithDomain/TiFlash replica 注入接口；因此
// 本文件不伪造 TiFlash 物理计划，只验证 Go golden 的录制/回放边界与可执行生产能力。
use crate::main_test::load_named_sql_cases;
use astersql_parser::{NormalizeDigest, Parser};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

const INTEGRATION_FIXTURE_TESTS: &[&str] = &[
    "TestVerboseExplain",
    "TestIsolationReadDoNotFilterSystemDB",
    "TestIsolationReadTiFlashNotChoosePointGet",
    "TestMergeContinuousSelections",
    "TestTiFlashPartitionTableScan",
    "TestTiFlashFineGrainedShuffle",
    "TestTiFlashExtraColumnPrune",
];

const INTEGRATION_STANDALONE_TESTS: &[&str] = &[
    "TestFixControl43817",
    "TestFixControl45132",
    "TestIndexMergeJSONMemberOf2FlakyPart",
    "TestIntegrationRegression",
];

#[derive(Clone, Copy)]
struct FixtureContract {
    name: &'static str,
    case_count: usize,
    result_fields: &'static [&'static str],
}

const FIXTURE_CONTRACTS: &[FixtureContract] = &[
    FixtureContract {
        name: "TestVerboseExplain",
        case_count: 20,
        result_fields: &["Plan"],
    },
    FixtureContract {
        name: "TestIsolationReadDoNotFilterSystemDB",
        case_count: 3,
        result_fields: &["Plan"],
    },
    FixtureContract {
        name: "TestIsolationReadTiFlashNotChoosePointGet",
        case_count: 2,
        result_fields: &["Result"],
    },
    FixtureContract {
        name: "TestMergeContinuousSelections",
        case_count: 1,
        result_fields: &["Plan"],
    },
    FixtureContract {
        name: "TestTiFlashPartitionTableScan",
        case_count: 4,
        result_fields: &["Plan"],
    },
    FixtureContract {
        name: "TestTiFlashFineGrainedShuffle",
        case_count: 10,
        result_fields: &["Plan", "Redact"],
    },
    FixtureContract {
        name: "TestTiFlashExtraColumnPrune",
        case_count: 2,
        result_fields: &["Plan"],
    },
];

#[test]
fn integration_fixture_inventory_matches_this_go_file() {
    let direct_go_fixture_tests = [
        "TestVerboseExplain",
        "TestIsolationReadDoNotFilterSystemDB",
        "TestIsolationReadTiFlashNotChoosePointGet",
        "TestMergeContinuousSelections",
        "TestTiFlashPartitionTableScan",
        "TestTiFlashFineGrainedShuffle",
        "TestTiFlashExtraColumnPrune",
    ];
    assert_eq!(INTEGRATION_FIXTURE_TESTS, direct_go_fixture_tests);
    assert_eq!(
        FIXTURE_CONTRACTS
            .iter()
            .map(|contract| contract.name)
            .collect::<Vec<_>>(),
        direct_go_fixture_tests
    );
    assert_eq!(
        INTEGRATION_STANDALONE_TESTS,
        [
            "TestFixControl43817",
            "TestFixControl45132",
            "TestIndexMergeJSONMemberOf2FlakyPart",
            "TestIntegrationRegression",
        ]
    );
}

/// 校验 verbose explain + hash_join hint 的 fixture SQL 可解析，且 NormalizeDigest 稳定。
#[test]
fn verbose_explain_fixture_sql_builds_real_ast_and_stable_digest() {
    let sql = "explain format = 'verbose' select /*+ hash_join(t1,t2) */ t1.a \
               from t1 join t2 on t1.a=t2.a where t1.b > 10 order by t1.a limit 5";
    // 先确认 parser 接受该 explain 形状。
    Parser::default().ParseOneStmt(sql, "", "").unwrap();
    // 同一 SQL 两次 NormalizeDigest 应得到相同文本与 digest 字节。
    let (first, first_digest) = NormalizeDigest(sql);
    let (second, second_digest) = NormalizeDigest(sql);
    assert_eq!(first, second);
    assert_eq!(first_digest.Bytes(), second_digest.Bytes());
}

/// 校验 TiFlash 分区表 DDL 与带 `read_from_storage(tiflash[...])` hint 的查询可一并解析。
#[test]
fn tiflash_partition_fixture_ddl_and_query_parse_together() {
    let sql = "create table t(a int, b int) partition by range(a) \
               (partition p0 values less than (10), partition p1 values less than maxvalue); \
               select /*+ read_from_storage(tiflash[t]) */ * from t where a=12";
    let (statements, _) = Parser::default().Parse(sql, "", "").unwrap();
    assert_eq!(statements.len(), 2);
}

/// Go 的 LoadTestCases/OnRecord 不只要求 SQL 可解析，还要求输入与标准/Cascades
/// 输出逐项对齐、输出 SQL 回写原输入，并保留各测试不同的 Plan/Result/Redact 形状。
#[test]
fn integration_suite_cases_preserve_record_and_replay_contract() {
    for cascades in [false, true] {
        let suite = crate::main_test::load_suite("integration_suite", cascades);
        for contract in FIXTURE_CONTRACTS {
            let (input, output) = suite
                .LoadTestCasesByName(contract.name, cascades)
                .unwrap_or_else(|error| panic!("load {}: {error}", contract.name));
            let input = input
                .as_array()
                .unwrap_or_else(|| panic!("{} input must be an array", contract.name));
            let output = output
                .as_array()
                .unwrap_or_else(|| panic!("{} output must be an array", contract.name));
            assert_eq!(input.len(), contract.case_count, "{} input", contract.name);
            assert_eq!(output.len(), input.len(), "{} output", contract.name);

            for (case_index, (sql, recorded)) in input.iter().zip(output).enumerate() {
                let sql = sql.as_str().unwrap_or_else(|| {
                    panic!("{} case {case_index} input is not SQL", contract.name)
                });
                Parser::default()
                    .Parse(sql, "", "")
                    .unwrap_or_else(|error| {
                        panic!(
                            "{} case {case_index} fixture {sql:?}: {error}",
                            contract.name
                        )
                    });
                let recorded = recorded.as_object().unwrap_or_else(|| {
                    panic!(
                        "{} case {case_index} output is not an object",
                        contract.name
                    )
                });
                assert_eq!(
                    recorded.get("SQL").and_then(|value| value.as_str()),
                    Some(sql),
                    "{} case {case_index} recorded SQL",
                    contract.name
                );
                for field in contract.result_fields {
                    let value = recorded.get(*field).unwrap_or_else(|| {
                        panic!("{} case {case_index} missing field {field}", contract.name)
                    });
                    if sql.trim_start().to_ascii_lowercase().starts_with("set ") {
                        assert!(
                            value.is_null(),
                            "{} case {case_index} set statement must not record {field}",
                            contract.name
                        );
                    } else {
                        assert!(
                            value.is_array(),
                            "{} case {case_index} field {field} must be an array",
                            contract.name
                        );
                    }
                }
            }
        }
    }

    let loaded = load_named_sql_cases("integration_suite", INTEGRATION_FIXTURE_TESTS, false);
    assert_eq!(
        loaded.len(),
        FIXTURE_CONTRACTS
            .iter()
            .map(|contract| contract.case_count)
            .sum::<usize>()
    );
}

fn new_testkit() -> TestKit {
    let mut testkit = TestKit::new(CreateMockStoreAndDomain().0);
    testkit.MustExec("use test", Vec::new());
    testkit
}

/// Go TestIntegrationRegression issue 33175 的可执行核心：事务内最大无符号值不能被
/// 临时写入干扰，rollback 必须释放副作用，升降序必须保持完整 u64 范围顺序。
#[test]
fn integration_regression_unsigned_bigint_transaction_and_ordering_match_go() {
    let mut testkit = new_testkit();
    testkit.MustExec(
        "create table t (id bigint unsigned not null, c varchar(20), primary key(id))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (9734095886065816707, 'a'), \
         (10353107668348738101, 'b'), (0, 'c')",
        Vec::new(),
    );
    testkit.MustExec("begin", Vec::new());
    testkit.MustExec("insert into t values (33, 'd')", Vec::new());
    testkit
        .MustQuery("select max(id) from t", Vec::new())
        .Check(Rows(&["10353107668348738101"]));
    testkit.MustExec("rollback", Vec::new());
    testkit
        .MustQuery("select id from t order by id desc", Vec::new())
        .Check(Rows(&["10353107668348738101", "9734095886065816707", "0"]));
    testkit
        .MustQuery("select id from t order by id asc", Vec::new())
        .Check(Rows(&["0", "9734095886065816707", "10353107668348738101"]));
}

/// 没有 fixture 的四个 Go 用例仍须把其正常、开关、错误触发与复杂回归 SQL 接到
/// 真实 parser；每段按 Go 执行顺序整批解析，防止仅保留测试名或单条冒烟 SQL。
#[test]
fn standalone_integration_sequences_are_complete_and_parseable() {
    let cases = [
        (
            "TestFixControl43817",
            "use test; create table t1 (a int); create table t2 (a int); \
             select * from t1 where t1.a > (select max(a) from t2); \
             set tidb_opt_fix_control='43817:on'; \
             select * from t1 where t1.a > (select max(a) from t2); \
             set tidb_opt_fix_control='43817:off'; \
             select * from t1 where t1.a > (select max(a) from t2)",
            8,
        ),
        (
            "TestFixControl45132",
            "use test; create table t (a int, b int, key(a)); \
             insert into t values (1,1), (2,2); analyze table t; \
             explain select * from t where a=2; \
             set @@tidb_opt_fix_control='45132:99'; analyze table t; \
             explain select * from t where a=2; \
             set @@tidb_opt_fix_control='45132:500'; \
             explain select * from t where a=2; \
             set @@tidb_opt_fix_control='45132:0'; \
             explain select * from t where a=2",
            12,
        ),
        (
            "TestIndexMergeJSONMemberOf2FlakyPart",
            "use test; create table t(a int, b int, c int, d json, \
             index iad(a, (cast(d->'$.b' as signed array)))); \
             insert into t values(1,1,1,'{\"b\":[1,2,3,4]}'); \
             set tidb_analyze_version=2; analyze table t all columns; \
             explain format='plan_tree' select * from t use index(iad) where a=1; \
             explain format='plan_tree' select * from t use index(iad) \
             where a=1 and (2 member of (d->'$.b'))",
            7,
        ),
        (
            "TestIntegrationRegression",
            "create table h1(id bigint not null, position_date date not null, \
             asset_id varchar(32), portfolio_code varchar(50), \
             primary key(id, position_date) nonclustered) \
             partition by range columns(position_date) \
             (partition p202401 values less than ('2024-02-01')); \
             with assetBalance as \
             (select asset_id, portfolio_code from h1 where position_date='2024-01-01'), \
             assetIdList as (select distinct asset_id from assetBalance) \
             select distinct balance.portfolio_code from assetBalance balance \
             left join assetIdList on balance.asset_id=assetIdList.asset_id; \
             create table mvi(a int, b int, j json, \
             index mvi_idx((cast(j as signed array)), a, b)); \
             explain format='plan_tree' select * from mvi \
             where a=1 and 6 member of (j)",
            4,
        ),
    ];
    assert_eq!(
        cases.iter().map(|(name, _, _)| *name).collect::<Vec<_>>(),
        INTEGRATION_STANDALONE_TESTS
    );
    for (name, sql, expected_statements) in cases {
        let (statements, warnings) = Parser::default()
            .Parse(sql, "", "")
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(statements.len(), expected_statements, "{name}");
        assert!(warnings.is_empty(), "{name}: warnings={warnings:?}");
    }
}
