// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
//
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;

#[test]
/// 参数未绑定应失败；常量 SELECT 允许；非法表名拒绝。
fn concrete_runtime_rejects_unbound_or_full_executor_sql() {
    let runtime = runtime();
    let store = runtime.NewMockStore().expect("create canonical mock store");
    runtime
        .BootstrapSession(Arc::clone(&store))
        .expect("bootstrap canonical domain");
    let session = runtime.CreateSession4Test(store).expect("create session");
    let statement = session
        .PrepareStmt("INSERT INTO aster_session_kv(k, v) VALUES (?, ?)")
        .expect("prepare insert");
    assert!(
        session
            .ExecutePreparedStmt(statement, &["only-key".into()])
            .is_err()
    );
    // Constant SELECT is intentionally supported by the concrete session.
    let mut constant = session
        .Execute("SELECT 1")
        .expect("constant SELECT should succeed");
    assert_eq!(
        constant.remove(0).Next().expect("read constant SELECT row"),
        Some(vec!["1".to_owned()])
    );
    let mut version = session
        .Execute("SELECT VERSION()")
        .expect("VERSION() should use the canonical server version function");
    let version = version
        .remove(0)
        .Next()
        .expect("read VERSION() row")
        .expect("VERSION() must return one row");
    assert_eq!(
        version,
        vec![astersql_parser_mysql::r#const::ServerVersion()]
    );
    assert!(
        session
            .Execute("INSERT INTO another_table VALUES ('k', 'v')")
            .is_err()
    );
}

/// 启动 domain 后构造 ConcreteSession。
fn strict_t_multi_info_schema() -> Arc<dyn infoschema::infoschema::InfoSchema> {
    strict_t_multi_info_schema_with_index(true)
}

fn strict_t_multi_info_schema_with_index(
    with_index: bool,
) -> Arc<dyn infoschema::infoschema::InfoSchema> {
    let column = |id: i64, name: &str, offset: isize, tp: u8| {
        let mut field_type = astersql_parser_types::NewFieldType(tp);
        if tp == astersql_parser_mysql::r#type::TypeVarchar {
            field_type.SetCharset("utf8mb4".to_owned());
            field_type.SetCollate("utf8mb4_bin".to_owned());
        } else {
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
        }
        astersql_meta_model::ColumnInfo {
            ID: id,
            Name: astersql_parser_ast::NewCIStr(name),
            Offset: offset,
            State: astersql_meta_model::StatePublic,
            FieldType: field_type,
            ..Default::default()
        }
    };
    let model = Arc::new(astersql_meta_model::TableInfo {
        ID: 88,
        Name: astersql_parser_ast::NewCIStr("t_multi"),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        Columns: vec![
            column(1, "a", 0, astersql_parser_mysql::r#type::TypeLonglong),
            column(2, "b", 1, astersql_parser_mysql::r#type::TypeVarchar),
            column(3, "c", 2, astersql_parser_mysql::r#type::TypeVarchar),
        ],
        Indices: with_index
            .then(|| astersql_meta_model::IndexInfo {
                ID: 11,
                Name: astersql_parser_ast::NewCIStr("idx_ab_prefix"),
                State: astersql_meta_model::StatePublic,
                Columns: vec![
                    astersql_meta_model::IndexColumn {
                        Name: astersql_parser_ast::NewCIStr("a"),
                        Offset: 0,
                        Length: astersql_parser_types::UnspecifiedLength as isize,
                        ..Default::default()
                    },
                    astersql_meta_model::IndexColumn {
                        Name: astersql_parser_ast::NewCIStr("b"),
                        Offset: 1,
                        Length: 15,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
            .into_iter()
            .collect(),
        ..Default::default()
    });
    infoschema::infoschema::MockInfoSchema(vec![infoschema::infoschema::TableInfo {
        id: model.ID,
        name: infoschema::infoschema::CiString::new("t_multi"),
        columns: vec![
            infoschema::infoschema::ColumnInfo {
                id: 1,
                name: infoschema::infoschema::CiString::new("a"),
                ..Default::default()
            },
            infoschema::infoschema::ColumnInfo {
                id: 2,
                name: infoschema::infoschema::CiString::new("b"),
                ..Default::default()
            },
            infoschema::infoschema::ColumnInfo {
                id: 3,
                name: infoschema::infoschema::CiString::new("c"),
                ..Default::default()
            },
        ],
        model_meta: Some(model),
        ..Default::default()
    }])
}

/// 编码 t_multi 一行的 KV key/value。
pub(crate) fn strict_t_multi_row(handle: i64, a: i64, b: &str, c: &str) -> (kv::Key, Vec<u8>) {
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(
        88,
        Box::new(astersql_tablecodec::kv::IntHandle(handle)),
    );
    let value = astersql_tablecodec::EncodeRow(
        astersql_tablecodec::codec::NewEncoder(astersql_tablecodec::collate::NewCollationEnabled()),
        Some(astersql_tablecodec::time::UTC),
        vec![
            astersql_tablecodec::types::NewIntDatum(a),
            astersql_tablecodec::types::NewStringDatum(b.to_owned()),
            astersql_tablecodec::types::NewStringDatum(c.to_owned()),
        ],
        vec![1, 2, 3],
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(true),
    )
    .expect("encode strict t_multi row");
    (kv::Key(key.0), value)
}

#[test]
/// 严格流水线：解析→优化→KV 执行，并验证 set_var hint 生命周期。
fn strict_t_multi_runs_parser_builder_optimizer_and_kv_executor_with_set_var_lifecycle() {
    use astersql_executor_sortexec::{Row, SortValue};

    let session = concrete_session();
    let mut transaction = session
        .domain()
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin t_multi seed transaction");
    for (handle, a, b, c) in [
        (1, 1, "alpha", "first"),
        (2, 1, "beta", "second"),
        (3, 1, "gamma", "third"),
        (4, 2, "alpha", "fourth"),
        (5, 2, "beta", "fifth"),
        (6, 2, "gamma", "sixth"),
    ] {
        let (key, value) = strict_t_multi_row(handle, a, b, c);
        transaction.Set(key, value).expect("seed t_multi KV row");
    }
    transaction
        .Commit(&kv::Context::default())
        .expect("commit t_multi rows");
    let snapshot = session.domain().storage().with_storage(|storage| {
        let version = storage.CurrentVersion("global").expect("current version");
        storage.GetSnapshot(version)
    });

    session.WithSessionVars(|variables| {
        variables
            .SetHintSystemVarWithOldState(
                astersql_sessionctx_vardef::TiDBOptPartialOrderedIndexForTopN,
                "DISABLE",
            )
            .expect("set ordinary session value");
    });
    let sql = "select /*+ set_var(tidb_opt_partial_ordered_index_for_topn=COST) use_index(t_multi, idx_ab_prefix) */ * from t_multi order by a, b limit 3 offset 2";
    let result = session
        .ExecutePlannedKVSelect(sql, strict_t_multi_info_schema(), snapshot.as_ref())
        .expect("execute strict t_multi pipeline");
    assert_eq!(
        result.Rows,
        vec![
            Row(vec![
                SortValue::Int(1),
                SortValue::Bytes(b"gamma".to_vec()),
                SortValue::Bytes(b"third".to_vec()),
            ]),
            Row(vec![
                SortValue::Int(2),
                SortValue::Bytes(b"alpha".to_vec()),
                SortValue::Bytes(b"fourth".to_vec()),
            ]),
            Row(vec![
                SortValue::Int(2),
                SortValue::Bytes(b"beta".to_vec()),
                SortValue::Bytes(b"fifth".to_vec()),
            ]),
        ],
        "physical operators: {:?}",
        result.Operators
    );
    assert_eq!(result.ScannedRows, 6);
    assert!(result.Cost.is_finite() && result.Cost > 0.0);
    assert!(result.PartialOrderedIndexForTopNEnabledDuringPlanning);
    assert!(
        result.Operators.iter().any(|name| name.contains("TopN")),
        "physical operators: {:?}",
        result.Operators
    );
    assert!(
        result.Operators.iter().any(|name| name.contains("Reader")),
        "physical operators: {:?}",
        result.Operators
    );
    assert!(
        result
            .Operators
            .iter()
            .any(|name| name.contains("TableScan")),
        "physical operators: {:?}",
        result.Operators
    );
    session.WithSessionVars(|variables| {
        assert_eq!(
            variables
                .GetHintSystemVar(astersql_sessionctx_vardef::TiDBOptPartialOrderedIndexForTopN,)
                .expect("restored session value"),
            "DISABLE"
        );
    });
}

#[test]
fn strict_t_multi_plans_selection_and_count_through_the_canonical_pipeline() {
    use astersql_executor_sortexec::{Row, SortValue};

    let session = concrete_session();
    let mut transaction = session
        .domain()
        .storage()
        .with_storage(|storage| storage.Begin(&[]))
        .expect("begin t_multi count seed transaction");
    for (handle, a, b, c) in [
        (1, 1, "alpha", "first"),
        (2, 1, "beta", "second"),
        (3, 1, "gamma", "third"),
        (4, 2, "alpha", "fourth"),
    ] {
        let (key, value) = strict_t_multi_row(handle, a, b, c);
        transaction
            .Set(key, value)
            .expect("seed t_multi count KV row");
    }
    transaction
        .Commit(&kv::Context::default())
        .expect("commit t_multi count rows");
    let snapshot = session.domain().storage().with_storage(|storage| {
        let version = storage.CurrentVersion("global").expect("current version");
        storage.GetSnapshot(version)
    });

    let result = session
        .ExecutePlannedKVSelect(
            "select count(*) from t_multi where a = 1",
            strict_t_multi_info_schema_with_index(false),
            snapshot.as_ref(),
        )
        .expect("execute planned filtered COUNT");
    assert_eq!(result.Rows, vec![Row(vec![SortValue::Int(3)])]);
    assert!(
        result
            .Operators
            .iter()
            .any(|operator| operator.contains("Selection")),
        "physical operators: {:?}",
        result.Operators
    );
    assert!(
        result
            .Operators
            .iter()
            .any(|operator| operator.contains("StreamAgg")),
        "physical operators: {:?}",
        result.Operators
    );
}

#[test]
fn filtered_count_dag_is_built_from_the_canonical_go_aligned_plan() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database planned_count")
        .expect("create planned COUNT database");
    session
        .execute("use planned_count")
        .expect("select planned COUNT database");
    session
        .execute(
            "create table orders_500m (
                order_id bigint unsigned primary key,
                user_id bigint unsigned not null,
                status varchar(16)
            )",
        )
        .expect("create planned COUNT table");
    let table = session
        .domain()
        .stats_table("planned_count", "orders_500m")
        .expect("planned COUNT table")
        .1;
    let encoded = session
        .planned_scalar_count_dag(
            "select count(*) from planned_count.orders_500m \
             where user_id >= 10 and user_id < 20",
            table.ID,
        )
        .expect("plan filtered COUNT")
        .expect("table scan COUNT is pushable");
    let dag: tipb::DagRequest =
        protobuf::parse_from_bytes(&encoded).expect("decode planned filtered COUNT DAG");
    assert_eq!(
        dag.get_executors()
            .iter()
            .map(tipb::Executor::get_tp)
            .collect::<Vec<_>>(),
        vec![
            tipb::ExecType::TypeTableScan,
            tipb::ExecType::TypeSelection,
            tipb::ExecType::TypeStreamAgg,
        ]
    );
    let columns = dag.get_executors()[0].get_tbl_scan().get_columns();
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0].get_column_id(), table.Columns[1].ID);
    assert_ne!(
        columns[0].get_flag() & astersql_parser_mysql::r#type::UnsignedFlag as i32,
        0
    );
    let conditions = dag.get_executors()[1].get_selection().get_conditions();
    assert_eq!(conditions.len(), 2);
    assert_eq!(
        dag.get_executors()[2].get_aggregation().get_agg_func()[0].get_agg_func_mode(),
        tipb::AggFunctionMode::Partial1Mode
    );
}

#[test]
fn forced_index_order_by_limit_keeps_root_limit() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database explain_limit")
        .expect("create EXPLAIN database");
    session
        .execute("use explain_limit")
        .expect("select EXPLAIN database");
    session
        .execute(
            "create table t(a bigint, b decimal(41,16), c set('a','b','c'), \
             key idx_c(c)) partition by hash(a) partitions 4",
        )
        .expect("create partitioned index fixture");

    let mut results = session
        .execute("explain select * from t use index(idx_c) order by c limit 5")
        .expect("explain forced-index LIMIT query");
    let mut result = results.remove(0);
    let mut rows = Vec::new();
    while let Some(row) = result.next_row().expect("read EXPLAIN row") {
        rows.push(row);
    }
    assert!(
        rows.first()
            .and_then(|row| row.first())
            .is_some_and(|operator| operator == "Limit"),
        "forced-index plan must retain a root Limit: {rows:?}"
    );
}

#[test]
fn forced_index_hint_matches_index_name_instead_of_a_leading_column() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database forced_index_name")
        .expect("create forced-index database");
    session
        .execute("use forced_index_name")
        .expect("select forced-index database");
    session
        .execute(
            "create table t2 (a int unsigned primary key, b int not null, c int unsigned, \
             unique key b(b), key b_c(b,c))",
        )
        .expect("create colliding index-name fixture");

    let explain_root = |sql: &str| {
        let mut results = session.execute(sql).expect("explain forced-index query");
        results
            .remove(0)
            .next_row()
            .expect("read EXPLAIN row")
            .and_then(|row| row.into_iter().next())
            .expect("EXPLAIN must return a root operator")
    };

    assert_eq!(
        explain_root("explain format='brief' select * from t2 use index(b) where b = 1 and a = 1"),
        "PointGet"
    );
    assert_eq!(
        explain_root(
            "explain format='brief' select * from t2 use index(b_c) where b = 1 and a = 1"
        ),
        "IndexLookUp"
    );
}

#[test]
fn constant_projection_is_covered_by_the_selected_index_range() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database constant_projection_index")
        .expect("create constant-projection database");
    session
        .execute("use constant_projection_index")
        .expect("select constant-projection database");
    session
        .execute("create table t(id int, is_deleted tinyint, key k(id, is_deleted))")
        .expect("create composite-index fixture");

    let mut results = session
        .execute("explain select 1 from t where id = 1 and is_deleted = true")
        .expect("explain constant projection over an index range");
    let mut result = results.remove(0);
    let mut operators = Vec::new();
    while let Some(row) = result.next_row().expect("read EXPLAIN row") {
        operators.push(row[0].clone());
    }

    assert!(
        operators
            .iter()
            .any(|operator| operator.contains("IndexRangeScan")),
        "constant projection must remain index-covered: {operators:?}"
    );
}
