// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 向量索引 / ANN 规划器用例，对应同名 Go 文件。
//
// Rust Domain 已能通过 `set_tiflash_replica_for_test` 发布可用副本，
// TestKit 也已接通 vector DDL、ANN 计划与 golden 查询；因此本文件执行真实
// 建表、写入、ANALYZE、副本切换和 EXPLAIN，不再用 parser-only 检查代替行为断言。

#![allow(non_snake_case)]

use astersql_meta_model::{
    DistanceMetricCosine, DistanceMetricInnerProduct, DistanceMetricL2,
    IndexableFnNameToDistanceMetric,
};
use astersql_parser::Parser;
use astersql_parser::ast;
use astersql_sessionctx_vardef::TiDBIsolationReadEngines;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::testdata::{ConvertRowsToStrings, LoadTestSuiteDataWithCascades};
use std::sync::Arc;

/// 创建 mock store + Domain，并返回绑定其上的 `TestKit`。
fn new_testkit() -> (Arc<astersql_domain::Domain>, TestKit) {
    let (store, domain) = CreateMockStoreAndDomain();
    (domain, TestKit::new(store))
}

#[test]
fn tiflash_schema_metadata_is_visible_in_tidb_kv_layout() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("create database tiflash_schema_meta", Vec::new());
    tk.MustExec("use tiflash_schema_meta", Vec::new());
    tk.MustExec("create table t (v vector(3))", Vec::new());
    tk.MustExec(
        "alter table t add vector index v_idx ((vec_l2_distance(v))) using hnsw",
        Vec::new(),
    );
    tk.MustExec("alter table t set tiflash replica 1", Vec::new());

    let table = domain.table_by_name("tiflash_schema_meta", "t").unwrap();
    assert_eq!(
        table.Indices[0].Tp,
        astersql_parser_ast::model::IndexTypeVector
    );
    let replica_rows = ConvertRowsToStrings(&tk.MustQuery(
        "select table_id, replica_count, available from information_schema.tiflash_replica where table_schema = 'tiflash_schema_meta' and table_name = 't'",
        Vec::new(),
    ).Rows());
    assert_eq!(replica_rows, vec![format!("{} 1 0", table.ID)]);
    let db_id = table.DBID;
    let table_id = table.ID;
    let mut db_key = b"m".to_vec();
    db_key = astersql_util_codec::EncodeBytes(db_key, b"DBs");
    db_key = astersql_util_codec::EncodeUint(db_key, b'h' as u64);
    db_key = astersql_util_codec::EncodeBytes(db_key, format!("DB:{db_id}").as_bytes());
    let mut table_key = b"m".to_vec();
    table_key = astersql_util_codec::EncodeBytes(table_key, format!("DB:{db_id}").as_bytes());
    table_key = astersql_util_codec::EncodeUint(table_key, b'h' as u64);
    table_key = astersql_util_codec::EncodeBytes(table_key, format!("Table:{table_id}").as_bytes());
    let mut version_key = b"m".to_vec();
    version_key = astersql_util_codec::EncodeBytes(version_key, b"SchemaVersionKey");
    version_key = astersql_util_codec::EncodeUint(version_key, b's' as u64);

    domain.storage_handle().with_storage(|store| {
        let version = store.CurrentVersion("global").unwrap();
        let snapshot = store.GetSnapshot(version);
        let db = astersql_kv::GetValue(
            &astersql_kv::Context::default(),
            snapshot.as_ref(),
            astersql_kv::Key(db_key),
        )
        .unwrap();
        let table = astersql_kv::GetValue(
            &astersql_kv::Context::default(),
            snapshot.as_ref(),
            astersql_kv::Key(table_key.clone()),
        )
        .unwrap();
        let schema_version = astersql_kv::GetValue(
            &astersql_kv::Context::default(),
            snapshot.as_ref(),
            astersql_kv::Key(version_key),
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&db).contains("tiflash_schema_meta"));
        assert!(String::from_utf8_lossy(&table).contains("tiflash"));
        let schema_version = String::from_utf8_lossy(&schema_version)
            .parse::<i64>()
            .unwrap();
        assert!(schema_version > 0);
        let mut diff_key = astersql_util_codec::EncodeBytes(
            b"m".to_vec(),
            format!("Diff:{schema_version}").as_bytes(),
        );
        diff_key = astersql_util_codec::EncodeUint(diff_key, b's' as u64);
        let diff = astersql_kv::GetValue(
            &astersql_kv::Context::default(),
            snapshot.as_ref(),
            astersql_kv::Key(diff_key),
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&diff).contains("\"regenerate_schema_map\":true"));
    });
    tk.MustExec("drop table t", Vec::new());
    domain.storage_handle().with_storage(|store| {
        let version = store.CurrentVersion("global").unwrap();
        let snapshot = store.GetSnapshot(version);
        let error = astersql_kv::GetValue(
            &astersql_kv::Context::default(),
            snapshot.as_ref(),
            astersql_kv::Key(table_key),
        )
        .unwrap_err();
        assert!(astersql_kv::IsErrNotFound(&error));
    });
}

#[test]
fn tiflash_replica_state_reaches_vector_planner() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table replica_state_t (vec vector(3))", Vec::new());
    tk.MustExec(
        "alter table replica_state_t add vector index ((vec_cosine_distance(vec))) using hnsw",
        Vec::new(),
    );
    tk.MustExec(
        "alter table replica_state_t set tiflash replica 1",
        Vec::new(),
    );

    let table = domain
        .table_by_name("test", "replica_state_t")
        .expect("lookup replica_state_t");
    let replica = table.TiFlashReplica.as_ref().expect("replica metadata");
    assert_eq!(replica.Count, 1);
    assert!(!replica.Available, "DDL must wait for replica progress");

    let plan = explain_plan_tree(
        &tk,
        "select * from replica_state_t order by vec_cosine_distance(vec, '[1,1,1]') limit 1",
    );
    assert!(
        !plan.iter().any(|row| row.contains("annIndex:")),
        "{plan:?}"
    );
    assert!(
        domain
            .publish_tiflash_replica_progress("test", "replica_state_t", table.ID + 1, 1.0)
            .is_err(),
        "stale table IDs must not publish replica readiness"
    );

    for (progress, available) in [(0.5, false), (1.0, true), (0.2, false)] {
        domain
            .publish_tiflash_replica_progress("test", "replica_state_t", table.ID, progress)
            .expect("publish observed replica progress");
        let table = domain
            .table_by_name("test", "replica_state_t")
            .expect("lookup updated table");
        assert_eq!(
            table
                .TiFlashReplica
                .as_ref()
                .expect("replica metadata")
                .Available,
            available
        );
        let plan = explain_plan_tree(
            &tk,
            "select * from replica_state_t order by vec_cosine_distance(vec, '[1,1,1]') limit 1",
        );
        assert_eq!(
            plan.iter().any(|row| row.contains("annIndex:")),
            available,
            "progress={progress}, plan={plan:?}"
        );
    }
}

/// 解析单条 SQL 为 AST 节点；失败则 panic 并带上原 SQL。
fn parse_stmt(sql: &str) -> Box<dyn ast::Node> {
    Parser::default()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse `{sql}`: {error}"))
}

/// 对应 Go `getPlanRows`：把制表符换成空格，再按换行拆成计划行列表。
fn get_plan_rows(plan_str: &str) -> Vec<String> {
    plan_str
        .replace('\t', " ")
        .split('\n')
        .map(String::from)
        .collect()
}

/// 读取 Go `ann_index_suite` 的 Cascades golden 文件。
fn load_ann_suite() -> astersql_testkit::testdata::TestData {
    LoadTestSuiteDataWithCascades(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "ann_index_suite",
        true,
    )
    .unwrap_or_else(|error| panic!("load ann_index_suite: {error}"))
}

/// 对应 Go `RunTestUnderCascadesAndDomainWithSchemaLease`：两种规划器模式各运行一次。
fn run_under_cascades(mut test: impl FnMut(&Arc<astersql_domain::Domain>, &mut TestKit, bool)) {
    for cascades in [false, true] {
        let (domain, mut tk) = new_testkit();
        tk.MustExec(
            &format!(
                "set @@tidb_enable_cascades_planner = {}",
                if cascades { 1 } else { 0 }
            ),
            Vec::new(),
        );
        test(&domain, &mut tk, cascades);
    }
}

fn statement_warnings(tk: &TestKit) -> Vec<String> {
    tk.MustQuery("show warnings", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| {
            row.last()
                .unwrap_or_else(|| panic!("SHOW WARNINGS row must contain a message"))
                .clone()
        })
        .collect()
}

/// 执行指定 Go suite 的全部 SQL，并逐条比较 plan 与 warning golden。
fn assert_ann_suite(tk: &mut TestKit, name: &str, cascades: bool) {
    let suite = load_ann_suite();
    let (input, output) = suite
        .LoadTestCasesByName(name, cascades)
        .unwrap_or_else(|error| panic!("load {name}: {error}"));
    let inputs = input.as_array().expect("suite input must be an array");
    let outputs = output.as_array().expect("suite output must be an array");
    assert_eq!(inputs.len(), outputs.len(), "{name} case count");

    for (index, (sql_value, expected_value)) in inputs.iter().zip(outputs).enumerate() {
        let sql = sql_value.as_str().expect("suite SQL must be text");
        let expected_sql = expected_value
            .get("SQL")
            .and_then(|value| value.as_str())
            .expect("golden SQL must be text");
        assert!(
            !expected_sql.is_empty(),
            "{name}[{index}] golden SQL must not be empty"
        );

        // 与 Go 一致：SET / UPDATE 是 fixture 变更，执行后直接进入下一例。
        if sql.starts_with("set") || sql.starts_with("UPDATE") {
            tk.MustExec(sql, Vec::new());
            continue;
        }

        let expected_plan = expected_value
            .get("Plan")
            .and_then(|value| value.as_array())
            .expect("golden Plan must be an array")
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .expect("golden Plan row must be text")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        let actual_plan = ConvertRowsToStrings(&tk.MustQuery(sql, Vec::new()).Rows());
        assert_eq!(actual_plan, expected_plan, "{name}[{index}] sql={sql}");

        let expected_warnings = expected_value
            .get("Warn")
            .and_then(|value| value.as_array())
            .map(|warnings| {
                warnings
                    .iter()
                    .map(|warning| {
                        warning
                            .as_str()
                            .expect("golden warning must be text")
                            .to_owned()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert_eq!(
            statement_warnings(tk),
            expected_warnings,
            "{name}[{index}] warnings sql={sql}"
        );
    }
}

fn explain_plan_tree(tk: &TestKit, query: &str) -> Vec<String> {
    ConvertRowsToStrings(
        &tk.MustQuery(&format!("explain format = 'plan_tree' {query}"), Vec::new())
            .Rows(),
    )
}

/// 将 EXPLAIN 中的向量字面量与数字常量参数化，复刻本用例依赖的
/// Go `NormalizePlan` digest 等价/不等关系。
fn normalized_plan_signature(rows: &[String]) -> String {
    let input = rows.join("\n");
    let chars = input.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '['
            && let Some(end) = chars[index + 1..].iter().position(|ch| *ch == ']')
        {
            let end = index + 1 + end;
            if chars[index + 1..end].iter().all(|ch| {
                ch.is_ascii_digit() || matches!(ch, ',' | '.' | '-' | '+' | ' ' | 'e' | 'E')
            }) {
                output.push('?');
                index = end + 1;
                continue;
            }
        }
        if chars[index].is_ascii_digit() {
            output.push('?');
            index += 1;
            while index < chars.len() && chars[index].is_ascii_digit() {
                index += 1;
            }
            continue;
        }
        output.push(chars[index]);
        index += 1;
    }
    output
}

/// 对应 Go `TestVectorIndexProtobufMatch`：tipb 枚举编号与 model 距离度量字符串对齐。
// test_vector_index_protobuf_match 对应 Go TestVectorIndexProtobufMatch：
// tipb.VectorDistanceMetric_INNER_PRODUCT.String() == model.DistanceMetricInnerProduct。
// Rust tipb 的 ProtobufEnum::descriptor() 在无反射生成路径上会 panic；这里用枚举值编号
// （proto INNER_PRODUCT = 4）与 model 常量字符串对齐，保留 Go EqualValues 的契约。
#[test]
fn test_vector_index_protobuf_match() {
    use protobuf::ProtobufEnum;
    assert_eq!(tipb::VectorDistanceMetric::InnerProduct.value(), 4);
    assert_eq!(tipb::VectorDistanceMetric::Cosine.value(), 3);
    assert_eq!(tipb::VectorDistanceMetric::L2.value(), 2);
    assert_eq!(DistanceMetricInnerProduct.0.as_ref(), "INNER_PRODUCT");
    assert_eq!(DistanceMetricCosine.0.as_ref(), "COSINE");
    assert_eq!(DistanceMetricL2.0.as_ref(), "L2");
}

/// Go 侧通过 `BookKeeper` 暴露五个 ANN suite；Rust 必须逐项保留其 SQL
/// 输入、golden SQL 和计划/告警字段，不能只验证其中一两个手写查询。
#[test]
fn test_ann_index_suite_preserves_go_cases() {
    let suite = load_ann_suite();
    let expected_cases = [
        ("TestTiFlashANNIndex", 22),
        ("TestTiFlashANNIndexForPartition", 14),
        ("TestVectorSearchWithPKAuto", 11),
        ("TestVectorSearchWithPKForceTiKV", 11),
        ("TestVectorSearchHeavyFunction", 21),
    ];

    for (name, count) in expected_cases {
        for cascades in [false, true] {
            let (input, output) = suite
                .LoadTestCasesByName(name, cascades)
                .unwrap_or_else(|error| panic!("load {name}: {error}"));
            let inputs = input.as_array().expect("suite input must be an array");
            let outputs = output.as_array().expect("suite output must be an array");
            assert_eq!(inputs.len(), count, "{name} input count");
            assert_eq!(outputs.len(), count, "{name} output count");

            for (sql_value, expected_value) in inputs.iter().zip(outputs) {
                let sql = sql_value.as_str().expect("suite SQL must be text");
                let expected_sql = expected_value
                    .get("SQL")
                    .and_then(|value| value.as_str())
                    .expect("golden case SQL must be text");
                assert!(
                    !expected_sql.is_empty(),
                    "{name} golden SQL must not be empty"
                );
                let plan = expected_value
                    .get("Plan")
                    .and_then(|value| value.as_array())
                    .expect("golden case Plan must be an array");
                assert!(!plan.is_empty(), "{name} golden Plan must not be empty");
                assert!(
                    expected_value.get("Warn").is_some(),
                    "{name} golden Warn missing"
                );

                parse_stmt(sql);
                parse_stmt(expected_sql);
            }
        }
    }
}

/// 对应 Go `TestTiFlashANNIndex`：48 行 fixture、可用 TiFlash 副本、
/// ANALYZE、隔离读引擎以及 22 条 plan/warning golden 全部执行。
#[test]
fn test_tiflash_ann_index_matches_go_golden() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec(
        "create table t1 (
            vec vector(3),
            a int,
            b int,
            c vector(3),
            d vector
        )",
        Vec::new(),
    );
    tk.MustExec("alter table t1 set tiflash replica 1", Vec::new());
    tk.MustExec(
        "alter table t1 add vector index ((vec_cosine_distance(vec))) using hnsw",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values
            ('[1,1,1]', 1, 1, '[1,1,1]', '[1,1,1]'),
            ('[2,2,2]', 2, 2, '[2,2,2]', '[2,2,2]'),
            ('[3,3,3]', 3, 3, '[3,3,3]', '[3,3,3]')",
        Vec::new(),
    );
    for _ in 0..4 {
        tk.MustExec(
            "insert into t1(vec, a, b, c, d) select vec, a, b, c, d from t1",
            Vec::new(),
        );
    }
    domain
        .set_tiflash_replica_for_test("test", "t1", 1, true)
        .expect("set test.t1 TiFlash replica");
    tk.MustExec("analyze table t1", Vec::new());
    tk.MustExec("set @@tidb_isolation_read_engines = 'tiflash'", Vec::new());
    assert_ann_suite(&mut tk, "TestTiFlashANNIndex", false);
}

/// 对应 `getPlanRows` helper：验证制表符被展开为空格。
// test_get_plan_rows_normalizes_tabs 对应 getPlanRows helper。
#[test]
fn test_get_plan_rows_normalizes_tabs() {
    let rows = get_plan_rows(" TopN\troot ?\n └─TableReader\troot ");
    assert_eq!(
        rows,
        vec![
            " TopN root ?".to_string(),
            " └─TableReader root ".to_string()
        ]
    );
}

/// 对应 Go `TestANNInexWithSimpleCBO`，并覆盖共用的度量映射与 DDL AST。
#[test]
fn test_ann_index_with_simple_cbo_uses_vector_index() {
    let mapping = IndexableFnNameToDistanceMetric();
    assert_eq!(
        mapping.get("vec_cosine_distance"),
        Some(&DistanceMetricCosine)
    );
    assert_eq!(mapping.get("vec_l2_distance"), Some(&DistanceMetricL2));
    assert!(!mapping.contains_key("vec_negative_inner_product"));

    let ddl = "create table t1 (
			vec vector(3),
			a int,
			b int,
			c vector(3),
			d vector
		)";
    let stmt = parse_stmt(ddl);
    let create = stmt
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("CreateTableStmt");
    assert!(
        create.Cols.iter().any(|col| col.Name.Name.L == "vec"),
        "vec column must parse"
    );

    let index_ddl = "alter table t1 add vector index ((vec_cosine_distance(vec))) USING HNSW";
    let index_stmt = parse_stmt(index_ddl);
    let alter = index_stmt
        .as_any()
        .downcast_ref::<ast::AlterTableStmt>()
        .expect("AlterTableStmt");
    assert!(
        !alter.Specs.is_empty(),
        "VECTOR INDEX alter must produce specs"
    );

    let inline_ddl = "create table t1 (
				id int primary key,
				vec vector(3),
				a int,
				b int,
				c vector(3),
				d vector,
				VECTOR INDEX idx_embedding ((VEC_COSINE_DISTANCE(vec)))
			)";
    let inline = parse_stmt(inline_ddl);
    let inline_create = inline
        .as_any()
        .downcast_ref::<ast::CreateTableStmt>()
        .expect("inline CreateTableStmt");
    assert!(
        inline_create
            .Constraints
            .iter()
            .any(|c| c.Tp == ast::ConstraintType::Vector || c.Name.contains("idx_embedding")),
        "VECTOR INDEX constraint should appear in AST: {:?}",
        inline_create.Constraints
    );

    assert_eq!(TiDBIsolationReadEngines, "tidb_isolation_read_engines");
    let (domain, mut tk) = new_testkit();
    tk.MustExec("set @@tidb_isolation_read_engines = 'tiflash'", Vec::new());
    tk.MustExec(
        "create table runtime_t1 (
            vec vector(3),
            a int,
            b int,
            c vector(3),
            d vector
        )",
        Vec::new(),
    );
    tk.MustExec("alter table runtime_t1 set tiflash replica 1", Vec::new());
    tk.MustExec(
        "alter table runtime_t1 add vector index vector_index \
         ((vec_cosine_distance(vec))) using hnsw",
        Vec::new(),
    );
    domain
        .set_tiflash_replica_for_test("test", "runtime_t1", 1, true)
        .expect("set test.runtime_t1 TiFlash replica");
    let table = domain
        .table_by_name("test", "runtime_t1")
        .expect("lookup test.runtime_t1");
    let replica = table
        .TiFlashReplica
        .as_ref()
        .expect("runtime_t1 TiFlash replica metadata");
    assert_eq!(replica.Count, 1);
    assert!(replica.Available);
    let vector_index = table
        .Indices
        .iter()
        .find(|index| index.Name.L == "vector_index")
        .expect("vector_index metadata");
    let vector_info = vector_index
        .VectorInfo
        .as_ref()
        .expect("vector_index vector metadata");
    assert_eq!(vector_info.Dimension, 3);
    assert_eq!(vector_info.DistanceMetric, DistanceMetricCosine);
    tk.MustUseIndex(
        "select * from runtime_t1 order by \
         vec_cosine_distance(vec, '[1,1,1]') limit 1",
        "vector_index",
    );
}

/// 对应 Go `TestANNIndexNormalizedPlan`：执行真实 EXPLAIN，校验常量
/// 参数化、投影顺序差异以及 TiFlashReplica Available 往返切换。
#[test]
fn test_ann_index_normalized_plan_digest_inputs() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t (vec vector(3))", Vec::new());
    tk.MustExec("alter table t set tiflash replica 1", Vec::new());
    tk.MustExec(
        "alter table t add vector index ((vec_cosine_distance(vec))) using hnsw",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values ('[1,1,1]'), ('[2,2,2]'), ('[3,3,3]')",
        Vec::new(),
    );
    domain
        .set_tiflash_replica_for_test("test", "t", 1, true)
        .expect("set test.t TiFlash replica");
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec(
        "set @@tidb_isolation_read_engines = 'tiflash, tikv'",
        Vec::new(),
    );

    let p1 = explain_plan_tree(
        &tk,
        "select * from t order by vec_cosine_distance(vec, '[0,0,0]') limit 1",
    );
    assert!(p1.iter().any(|row| row.contains("TopN")), "p1={p1:?}");
    assert!(
        p1.iter().any(|row| row.contains("TableReader")),
        "p1={p1:?}"
    );
    assert!(
        p1.iter().any(|row| row.contains("TableFullScan")),
        "p1={p1:?}"
    );
    let d1 = normalized_plan_signature(&p1);
    let d2 = normalized_plan_signature(&explain_plan_tree(
        &tk,
        "select * from t order by vec_cosine_distance(vec, '[1,2,3]') limit 3",
    ));
    let d3 = normalized_plan_signature(&explain_plan_tree(
        &tk,
        "select * from t order by vec_cosine_distance(vec, '[]') limit 3",
    ));
    let dx1 = normalized_plan_signature(&explain_plan_tree(
        &tk,
        "select * from t order by vec_cosine_distance('[1,2,3]', vec) limit 3",
    ));
    assert_eq!(d1, d2);
    assert_eq!(d1, d3);
    assert_ne!(d1, dx1);

    domain
        .set_tiflash_replica_for_test("test", "t", 1, false)
        .expect("mark test.t TiFlash replica unavailable");
    let table = domain.table_by_name("test", "t").expect("lookup test.t");
    assert!(!table.TiFlashReplica.as_ref().expect("replica").Available);
    let p2 = explain_plan_tree(
        &tk,
        "select * from t order by vec_cosine_distance(vec, '[1,2,3]') limit 3",
    );
    assert_eq!(
        p2,
        vec![
            "TopN root  Column, offset:0, count:3",
            "└─TableReader root  data:TopN",
            "  └─TopN cop[tikv]  Column, offset:0, count:3",
            "    └─Projection cop[tikv]  test.t.vec, vec_cosine_distance(test.t.vec, [1,2,3])->Column",
            "      └─TableFullScan cop[tikv] table:t keep order:false",
        ]
    );
    assert_ne!(d1, normalized_plan_signature(&p2));

    domain
        .set_tiflash_replica_for_test("test", "t", 1, true)
        .expect("mark test.t TiFlash replica available");
    let table = domain.table_by_name("test", "t").expect("lookup test.t");
    assert!(table.TiFlashReplica.as_ref().expect("replica").Available);
    let d4 = normalized_plan_signature(&explain_plan_tree(
        &tk,
        "select * from t order by vec_cosine_distance(vec, '[1,2,3]') limit 3",
    ));
    assert_eq!(d1, d4);
}

/// 对应 Go `TestANNIndexWithNonIntClusteredPk`：复合主键表上真实规划
/// ANN 索引访问，并核对 vector index 物理元数据。
#[test]
fn test_ann_index_with_non_int_clustered_pk_uses_full_vector_range() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec(
        "create table t1 (
            vec vector(3),
            a int,
            b int,
            c vector(3),
            d vector,
            primary key (a, b)
        )",
        Vec::new(),
    );
    tk.MustExec("alter table t1 set tiflash replica 1", Vec::new());
    tk.MustExec(
        "alter table t1 add vector index ((vec_cosine_distance(vec))) using hnsw",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t1 values ('[1,1,1]', 1, 1, '[1,1,1]', '[1,1,1]')",
        Vec::new(),
    );
    domain
        .set_tiflash_replica_for_test("test", "t1", 1, true)
        .expect("set test.t1 TiFlash replica");

    let query = "select * from t1 use index(vector_index) order by \
                 vec_cosine_distance(vec, '[1,1,1]') limit 1";
    tk.MustUseIndex(query, "vector_index");
    let plan = explain_plan_tree(&tk, query);
    assert!(
        plan.iter().any(|row| row.contains("index:vector_index")),
        "plan={plan:?}"
    );
    assert!(
        plan.iter().any(|row| row.contains("annIndex:COSINE")),
        "plan={plan:?}"
    );

    let table = domain.table_by_name("test", "t1").expect("lookup test.t1");
    let primary = table
        .Indices
        .iter()
        .find(|index| index.Primary)
        .expect("composite primary index metadata");
    assert_eq!(primary.Columns.len(), 2);
    let vector = table
        .Indices
        .iter()
        .find(|index| index.Name.L == "vector_index")
        .and_then(|index| index.VectorInfo.as_ref())
        .expect("vector_index vector metadata");
    assert_eq!(vector.Dimension, 3);
    assert_eq!(vector.DistanceMetric, DistanceMetricCosine);
}

#[test]
fn hnsw_query_plan_matches_execution() {
    let (domain, mut tk) = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table hnsw_execution_t (id int primary key, vec vector(3))",
        Vec::new(),
    );
    tk.MustExec(
        "alter table hnsw_execution_t set tiflash replica 1",
        Vec::new(),
    );
    tk.MustExec(
        "alter table hnsw_execution_t add vector index vector_index ((vec_l2_distance(vec))) using hnsw",
        Vec::new(),
    );
    tk.MustExec(
        "insert into hnsw_execution_t values (1, '[1,2,3]'), (2, '[1,2,4]'), (3, '[9,9,9]')",
        Vec::new(),
    );
    let query = "select id from hnsw_execution_t order by vec_l2_distance(vec, '[1,2,3]') limit 2";
    tk.MustQuery(query, Vec::new())
        .Check(vec![vec!["1"], vec!["2"]]);
    domain
        .set_tiflash_replica_for_test("test", "hnsw_execution_t", 1, true)
        .expect("set TiFlash replica available");

    let plan = explain_plan_tree(&tk, query);
    assert!(
        tk.MustQuery(&format!("explain format = 'plan_tree' {query}"), Vec::new())
            .Rows()
            .iter()
            .all(|row| row.len() == 4 && !row[0].is_empty()),
        "plan_tree must expose Go's id/task/access-object/operator-info columns"
    );
    assert!(
        plan.iter().any(|row| row.contains("annIndex:L2")),
        "plan={plan:?}"
    );
    assert!(
        plan.iter().any(|row| row.contains("index:vector_index")),
        "plan={plan:?}"
    );
    tk.MustQuery(query, Vec::new())
        .Check(vec![vec!["1"], vec!["2"]]);
}

/// 建立 Go PK/HeavyFunction 三个用例共用的 6000 行非分区 fixture。
fn setup_vector_search_with_pk(domain: &astersql_domain::Domain, tk: &mut TestKit) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec("drop table if exists doc", Vec::new());
    tk.MustExec(
        "create table t1 (
            id int primary key,
            vec vector(3),
            a int,
            b int,
            c vector(3),
            d vector,
            vector index idx_embedding ((vec_cosine_distance(vec)))
        )",
        Vec::new(),
    );
    for i in 0..2000 {
        tk.MustExec(
            &format!(
                "insert into t1 values
                    ({i}, '[1,1,1]', 1, 1, '[1,1,1]', '[1,1,1]'),
                    ({}, '[2,2,2]', 2, 2, '[2,2,2]', '[2,2,2]'),
                    ({}, '[3,3,3]', 3, 3, '[3,3,3]', '[3,3,3]')",
                2000 + i,
                4000 + i,
            ),
            Vec::new(),
        );
    }
    tk.MustExec("analyze table t1", Vec::new());
    tk.MustExec("create table doc(id int, doc longtext)", Vec::new());
    domain
        .set_tiflash_replica_for_test("test", "t1", 1, true)
        .expect("set test.t1 TiFlash replica");
}

/// 对应 Go `TestVectorSearchWithPKAuto`：两种规划器模式逐条比对 golden。
#[test]
fn test_vector_search_with_pk_auto_matches_go_golden() {
    run_under_cascades(|domain, tk, cascades| {
        setup_vector_search_with_pk(domain, tk);
        assert_ann_suite(tk, "TestVectorSearchWithPKAuto", cascades);
    });
}

/// 对应 Go `TestVectorSearchWithPKForceTiKV`：强制 TiKV 后逐条比对 golden。
#[test]
fn test_vector_search_with_pk_force_tikv_matches_go_golden() {
    run_under_cascades(|domain, tk, cascades| {
        setup_vector_search_with_pk(domain, tk);
        tk.MustExec("set @@tidb_isolation_read_engines = 'tikv'", Vec::new());
        let engines = tk
            .MustQuery("select @@tidb_isolation_read_engines", Vec::new())
            .Rows()[0][0]
            .clone();
        assert_eq!(engines, "tikv");
        assert_ann_suite(tk, "TestVectorSearchWithPKForceTiKV", cascades);
    });
}

/// 对应 Go `TestVectorSearchHeavyFunction`：两种规划器模式覆盖全部
/// cosine/L1/L2/inner-product/非向量排序表达式。
#[test]
fn test_vector_search_heavy_function_matches_go_golden() {
    run_under_cascades(|domain, tk, cascades| {
        setup_vector_search_with_pk(domain, tk);
        assert_ann_suite(tk, "TestVectorSearchHeavyFunction", cascades);
    });
}
