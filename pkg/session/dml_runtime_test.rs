// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// DML 运行时集成测试：真实 SQL 经 ConcreteSession 落到 mock KV。
//
// 覆盖 INSERT/UPDATE/DELETE/REPLACE、显式事务与 IGNORE/ON DUPLICATE、
// 生成列 tablecodec 编解码、autocommit 失败不落盘，以及 EXPLAIN ANALYZE
// 中 auto_id_allocator 计数矩阵（对齐 Go DML 语义）。

use std::collections::HashMap;
use std::sync::Arc;

use astersql_domain::KvInfoSchemaLoader;
use astersql_kv as kv;
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage, mockStorage};

use crate::runtime::{ConcreteSession, ConcreteTestRuntime, RuntimeDomain};
use crate::testutil::{TestRecordSet, TestRuntime};
use crate::{SessionError, SessionResult};

/// 构造可独占的内存 mock 存储。
fn new_storage() -> SessionResult<mockStorage> {
    Arc::try_unwrap(
        NewMockStorage(KVStore::NewMemory(), None)
            .map_err(|error| SessionError::new(error.to_string()))?,
    )
    .map_err(|_| SessionError::new("mock storage retained an unexpected owner"))
}

/// Bootstrap Domain 后包装为 ConcreteSession，供 DML 用例执行真实 SQL。
fn concrete_session() -> ConcreteSession {
    let runtime = ConcreteTestRuntime::new(new_storage, Arc::new(KvInfoSchemaLoader::new()), false);
    let store = runtime.NewMockStore().expect("create DML mock store");
    let domain = runtime
        .BootstrapSession(store)
        .expect("bootstrap DML domain");
    let domain = domain
        .as_any()
        .downcast_ref::<RuntimeDomain>()
        .expect("DML runtime domain");
    ConcreteSession::new(Arc::clone(domain.domain()))
}

#[test]
fn mid_substr_and_substring_follow_mysql_character_bounds() {
    let row = HashMap::from([(
        "value".to_owned(),
        Some("0.400000000000000000000000000000".to_owned()),
    )]);
    for (expression, expected) in [
        ("mid(value, 6, 9)", Some("000000000")),
        ("substr(value, 1, 3)", Some("0.4")),
        ("substring(value, -3)", Some("000")),
        ("mid(value, 0, 2)", Some("")),
        ("mid(value, -99, 2)", Some("")),
        ("mid(value, 2, -1)", Some("")),
        ("mid(null, 1, 1)", None),
    ] {
        let expression =
            crate::dml_runtime::ParseGeneratedExpr(expression).expect("parse scalar expression");
        let actual = crate::dml_runtime::EvalExpr(&expression, &row, None)
            .expect("evaluate scalar expression");
        assert_eq!(actual.as_deref(), expected, "expression={expression:?}");
    }
}

#[test]
fn unary_float_literals_preserve_fractional_values() {
    for (expression, expected) in [("-0.393904", "-0.393904"), ("+1.25", "1.25")] {
        let expression =
            crate::dml_runtime::ParseGeneratedExpr(expression).expect("parse unary float");
        let actual = crate::dml_runtime::EvalExpr(&expression, &HashMap::new(), None)
            .expect("evaluate unary float");
        assert_eq!(actual.as_deref(), Some(expected));
    }
}

#[test]
fn statement_time_functions_follow_relational_runtime_format() {
    let row = HashMap::new();
    for (expression, expected_length) in [
        ("now()", 19),
        ("current_timestamp()", 19),
        ("localtimestamp(3)", 23),
        ("utc_timestamp(6)", 26),
    ] {
        let expression =
            crate::dml_runtime::ParseGeneratedExpr(expression).expect("parse time function");
        let value = crate::dml_runtime::EvalExpr(&expression, &row, None)
            .expect("evaluate time function")
            .expect("time function is non-NULL");
        assert_eq!(value.len(), expected_length);
        chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f")
            .unwrap_or_else(|error| panic!("{value}: {error}"));
    }
}

#[test]
fn charset_introduced_literals_are_evaluated_in_generated_expressions() {
    let expression =
        crate::dml_runtime::ParseGeneratedExpr("deleted_at > _utf8mb4'1970-01-01 01:00:01.000'")
            .expect("parse generated expression with charset introducer");
    let row = HashMap::from([(
        "deleted_at".to_owned(),
        Some("2026-08-26 12:00:00".to_owned()),
    )]);

    assert_eq!(
        crate::dml_runtime::EvalExpr(&expression, &row, None)
            .expect("evaluate charset-introduced literal"),
        Some("1".to_owned())
    );
}

/// 构造含嵌套 JSON 生成列的 TableInfo，用于 tablecodec 读写路径。
fn generated_table_info() -> astersql_meta_model::TableInfo {
    fn column(id: i64, name: &str, tp: u8) -> astersql_meta_model::ColumnInfo {
        astersql_meta_model::ColumnInfo {
            ID: id,
            Name: astersql_parser_ast::NewCIStr(name),
            Offset: (id - 1) as isize,
            State: astersql_meta_model::StatePublic,
            FieldType: astersql_parser_types::NewFieldType(tp),
            ..Default::default()
        }
    }
    let mut columns = vec![
        column(1, "col1", astersql_parser_mysql::r#type::TypeLonglong),
        column(2, "col2", astersql_parser_mysql::r#type::TypeVarchar),
        column(3, "col3", astersql_parser_mysql::r#type::TypeLong),
        column(4, "col4", astersql_parser_mysql::r#type::TypeVarchar),
        column(5, "col5", astersql_parser_mysql::r#type::TypeVarchar),
        column(
            6,
            "modify_time",
            astersql_parser_mysql::r#type::TypeLonglong,
        ),
        column(
            7,
            "create_time",
            astersql_parser_mysql::r#type::TypeLonglong,
        ),
        column(8, "col6", astersql_parser_mysql::r#type::TypeJSON),
        column(9, "col7", astersql_parser_mysql::r#type::TypeJSON),
        column(10, "col8", astersql_parser_mysql::r#type::TypeJSON),
        column(11, "col9", astersql_parser_mysql::r#type::TypeVarchar),
        column(12, "col10", astersql_parser_mysql::r#type::TypeVarchar),
    ];
    // col9/col10 为依赖 JSON 的生成列表达式，对齐 Go 嵌套生成列用例。
    columns[9].GeneratedExprString =
        "json_merge_patch(ifnull(col6, '{}'), ifnull(col7, '{}'))".to_owned();
    columns[9].GeneratedStored = true;
    columns[10].GeneratedExprString =
        "left(json_unquote(json_extract(col8, '$.col9[0]')), 36)".to_owned();
    columns[11].GeneratedExprString =
        "left(json_unquote(json_extract(col8, '$.col10')), 30)".to_owned();
    astersql_meta_model::TableInfo {
        ID: 901,
        Name: astersql_parser_ast::NewCIStr("test1"),
        Columns: columns,
        ..Default::default()
    }
}

/// 构造带/不带自增或 auto_random 主键的表，供 auto_id_allocator 矩阵测试。
fn auto_id_table_info(
    unsigned: bool,
    auto_random: bool,
    has_auto_id: bool,
) -> astersql_meta_model::TableInfo {
    let mut a_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
    if has_auto_id {
        // AutoRandomBits>0 时用 auto_random 而非 AUTO_INCREMENT 标志。
        let mut flags = astersql_parser_mysql::r#type::PriKeyFlag;
        if !auto_random {
            flags |= astersql_parser_mysql::r#type::AutoIncrementFlag;
        }
        if unsigned {
            flags |= astersql_parser_mysql::r#type::UnsignedFlag;
        }
        a_type.SetFlag(flags);
    }
    astersql_meta_model::TableInfo {
        ID: 902,
        Name: astersql_parser_ast::NewCIStr("t"),
        PKIsHandle: has_auto_id,
        AutoIncID: 1,
        AutoRandomBits: if auto_random { 5 } else { 0 },
        Columns: vec![
            astersql_meta_model::ColumnInfo {
                ID: 1,
                Name: astersql_parser_ast::NewCIStr("a"),
                Offset: 0,
                State: astersql_meta_model::StatePublic,
                FieldType: a_type,
                ..Default::default()
            },
            astersql_meta_model::ColumnInfo {
                ID: 2,
                Name: astersql_parser_ast::NewCIStr("b"),
                Offset: 1,
                State: astersql_meta_model::StatePublic,
                FieldType: astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeLong,
                ),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

/// 验证 INSERT/UPDATE/DELETE/REPLACE 与 EXPLAIN ANALYZE 触及真实 KV 运行时。
#[test]
fn real_sql_dml_and_explain_analyze_reach_the_kv_runtime() {
    let session = concrete_session();
    session
        .execute("insert into aster_session_kv(k, v) values ('a', 'one'), ('b', 'two')")
        .expect("seed real KV rows");

    session
        .execute("update aster_session_kv set v = 'changed' where k = 'a'")
        .expect("update real KV row");
    let mut changed = session
        .execute("select v from aster_session_kv where k = 'a'")
        .expect("read updated KV row")
        .remove(0);
    assert_eq!(
        changed.Next().expect("updated row"),
        Some(vec!["changed".to_owned()])
    );

    session
        .execute("delete from aster_session_kv where k = 'b'")
        .expect("delete real KV row");
    let mut deleted = session
        .execute("select v from aster_session_kv where k = 'b'")
        .expect("read deleted KV row")
        .remove(0);
    assert_eq!(deleted.Next().expect("deleted row"), None);

    session
        .execute("replace into aster_session_kv(k, v) values ('a', 'replaced')")
        .expect("replace real KV row");
    let mut replaced = session
        .execute("select v from aster_session_kv where k = 'a'")
        .expect("read replaced KV row")
        .remove(0);
    assert_eq!(
        replaced.Next().expect("replaced row"),
        Some(vec!["replaced".to_owned()])
    );

    // EXPLAIN ANALYZE 应暴露 Replace 算子与 prewrite/commit 统计。
    let mut explained = session
        .execute("explain analyze replace into aster_session_kv(k, v) values ('c', 'three')")
        .expect("explain analyze real Replace")
        .remove(0);
    assert_eq!(explained.Columns()[0], "id");
    let row = explained
        .Next()
        .expect("explain row")
        .expect("one explain row");
    assert_eq!(row[0], "Replace_1");
    assert_eq!(row[2], "1");
    assert!(row[5].contains("prewrite_keys:1"));
    assert!(row[5].contains("committed:true"));
}

/// 显式事务内读写 mem-buffer，以及 INSERT IGNORE / ON DUPLICATE KEY UPDATE。
#[test]
fn explicit_transaction_ignore_and_on_duplicate_follow_go_dml_semantics() {
    let session = concrete_session();
    session.execute("begin").expect("begin real transaction");
    session
        .execute("insert into aster_session_kv(k, v) values ('txn', 'before')")
        .expect("insert in transaction");
    session
        .execute("update aster_session_kv set v = 'inside' where k = 'txn'")
        .expect("update transaction mem-buffer row");
    let mut inside = session
        .execute("select v from aster_session_kv where k = 'txn'")
        .expect("read transaction mem-buffer row")
        .remove(0);
    assert_eq!(
        inside.Next().expect("inside row"),
        Some(vec!["inside".to_owned()])
    );
    session.execute("commit").expect("commit transaction");

    session
        .execute("insert ignore into aster_session_kv(k, v) values ('txn', 'ignored')")
        .expect("ignore duplicate");
    session
        .execute(
            "insert into aster_session_kv(k, v) values ('txn', 'duplicate') \
             on duplicate key update v = values(v)",
        )
        .expect("update duplicate from VALUES(v)");
    let mut duplicate = session
        .execute("select v from aster_session_kv where k = 'txn'")
        .expect("read duplicate-updated row")
        .remove(0);
    assert_eq!(
        duplicate.Next().expect("duplicate row"),
        Some(vec!["duplicate".to_owned()])
    );
}

#[test]
fn insert_ignore_clamps_signed_int_overflow_like_go() {
    let session = concrete_session();
    session
        .execute("create table ignore_int_overflow (id int primary key, v int)")
        .expect("create overflow table");
    session
        .execute(
            "insert ignore into ignore_int_overflow values (1, 9223372036854775807), \
             (2, -9223372036854775807)",
        )
        .expect("INSERT IGNORE converts overflow to warnings");
    let mut result = session
        .execute("select id,v from ignore_int_overflow order by id")
        .expect("read clamped rows")
        .remove(0);
    assert_eq!(
        result.Next().expect("positive overflow row"),
        Some(vec!["1".to_owned(), i32::MAX.to_string()])
    );
    assert_eq!(
        result.Next().expect("negative overflow row"),
        Some(vec!["2".to_owned(), i32::MIN.to_string()])
    );
}

#[test]
fn insert_ignore_uses_temporal_zero_for_null_primary_key_like_go() {
    let session = concrete_session();
    session
        .execute("create table ignore_datetime_null (v datetime primary key)")
        .expect("create temporal table");
    session
        .execute("insert ignore into ignore_datetime_null values (null)")
        .expect("INSERT IGNORE converts NULL temporal key to its zero value");
    let mut result = session
        .execute("select v from ignore_datetime_null")
        .expect("read zero temporal row")
        .remove(0);
    assert_eq!(
        result.Next().expect("zero temporal row"),
        Some(vec!["0000-00-00 00:00:00".to_owned()])
    );
}

#[test]
fn temporal_column_compared_with_numeric_literal_uses_numeric_coercion() {
    let session = concrete_session();
    session
        .execute("create table temporal_numeric_cmp (v datetime)")
        .expect("create temporal comparison table");
    session
        .execute("insert into temporal_numeric_cmp values ('2024-01-01 00:00:00')")
        .expect("insert temporal value");
    let mut result = session
        .execute("select v from temporal_numeric_cmp where v > -0.5")
        .expect("compare temporal column in numeric context")
        .remove(0);
    assert_eq!(
        result.Next().expect("numeric comparison row"),
        Some(vec!["2024-01-01 00:00:00".to_owned()])
    );
    let mut result = session
        .execute("select v from temporal_numeric_cmp where v > '783'")
        .expect("invalid temporal constant follows MySQL coercion")
        .remove(0);
    assert_eq!(
        result.Next().expect("coerced string comparison row"),
        Some(vec!["2024-01-01 00:00:00".to_owned()])
    );
}

#[test]
fn sum_coerces_string_values_from_their_numeric_prefix() {
    let session = concrete_session();
    session
        .execute("create table sum_string_values (v varchar(32))")
        .expect("create string aggregate table");
    session
        .execute("insert into sum_string_values values ('12abc'), ('word'), ('-2.5tail')")
        .expect("insert string aggregate values");
    let mut result = session
        .execute("select sum(v) from sum_string_values")
        .expect("sum coerces string values")
        .remove(0);
    assert_eq!(
        result.Next().expect("string sum row"),
        Some(vec!["9.5".to_owned()])
    );
}

/// 嵌套生成列：INSERT/UPDATE 后经 tablecodec 解码校验派生列值。
#[test]
fn tablecodec_nested_generated_columns_follow_go_insert_update_delete() {
    let session = concrete_session();
    session
        .RegisterDmlTable(generated_table_info())
        .expect("register generated TableInfo");
    session
        .execute("insert into test1 values (-100000000, '123459789332', 1, '123459789332', 'BBBBB', 1675871896, 1675871896, '{\"col10\": \"CCCCC\",\"col9\": [\"ABCDEFG\"]}', null, default, default, default)")
        .expect("evaluate Go nested generated-column insert");
    let rows = session.ReadDmlRows("test1").expect("decode tablecodec row");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["col9"], Some("ABCDEFG".to_owned()));
    assert_eq!(rows[0]["col10"], Some("CCCCC".to_owned()));

    session
        .execute("update test1 set col7 = '{\"col10\":\"DDDDD\",\"col9\":[\"abcdefg\"]}' where col1 = -100000000")
        .expect("update base JSON and reevaluate Go generated dependency chain");
    let rows = session.ReadDmlRows("test1").expect("decode updated row");
    assert_eq!(rows[0]["col9"], Some("abcdefg".to_owned()));
    assert_eq!(rows[0]["col10"], Some("DDDDD".to_owned()));

    session
        .execute("delete from test1 where col1 < 0")
        .expect("delete tablecodec row");
    assert!(
        session
            .ReadDmlRows("test1")
            .expect("rows after delete")
            .is_empty()
    );
}

/// 注入 prewrite 失败：错误向上传播且失败写不可见。
#[test]
fn autocommit_failure_is_propagated_and_does_not_publish_kv_writes() {
    let session = concrete_session();
    session.InjectNextDmlCommitError("injected prewrite failure");
    let error = match session
        .execute("insert into aster_session_kv(k, v) values ('failed', 'not-visible')")
    {
        Ok(_) => panic!("injected commit error must reach the caller"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("injected prewrite failure"));
    let mut rows = session
        .execute("select v from aster_session_kv where k = 'failed'")
        .expect("read after failed commit")
        .remove(0);
    assert_eq!(rows.Next().expect("failed row visibility"), None);
}

/// EXPLAIN ANALYZE 报告 auto_id_allocator 的 alloc/rebase 计数（含无自增对照）。
#[test]
fn explain_analyze_dml2_reports_go_auto_id_allocator_matrix() {
    let cases = [
        ("insert into t () values ()", "", 1, 0),
        ("insert into t (a) values (99000000000)", "", 0, 1),
        ("insert into t (a) values (null), (99000000000)", "", 1, 1),
        (
            "insert ignore into t values (null,1), (2,2), (99000000000,3), (100000000000,4)",
            "",
            1,
            2,
        ),
        (
            "insert into t values (null,null), (1,1), (2,2) on duplicate key update a=a+100000000000",
            "",
            1,
            1,
        ),
        ("replace into t () values ()", "", 1, 0),
        ("replace into t (a) values (null), (99000000000)", "", 1, 1),
        (
            "update t set a=a*100000000000",
            "insert into t values (1,1),(2,2)",
            0,
            2,
        ),
    ];
    // 有符号/无符号自增与 auto_random 三组矩阵；auto_random 跳过 ON DUPLICATE。
    for (unsigned, auto_random) in [(false, false), (true, false), (false, true)] {
        for (sql, prepare, alloc_count, rebase_count) in cases {
            if auto_random && sql.contains("on duplicate key") {
                continue;
            }
            let session = concrete_session();
            session
                .RegisterDmlTable(auto_id_table_info(unsigned, auto_random, true))
                .expect("register auto-ID TableInfo");
            if auto_random {
                session
                    .execute("set @@allow_auto_random_explicit_insert=1")
                    .expect("allow explicit auto-random values like the Go matrix");
            }
            if !prepare.is_empty() {
                session.execute(prepare).expect("prepare auto-ID case");
            }
            let mut result = session
                .execute(&format!("explain analyze {sql}"))
                .unwrap_or_else(|error| panic!("auto-ID case `{sql}` failed: {error}"))
                .remove(0);
            let row = result
                .Next()
                .expect("explain row")
                .expect("one explain row");
            let expected = format!(
                "auto_id_allocator: {{alloc_cnt: {alloc_count}, rebase_cnt: {rebase_count}}}"
            );
            assert!(row[5].contains(&expected), "{sql}: {}", row[5]);
        }
    }

    // 无自增主键时不应出现 auto_id_allocator 字段。
    for (sql, prepare, _, _) in cases {
        let session = concrete_session();
        session
            .RegisterDmlTable(auto_id_table_info(false, false, false))
            .expect("register table without auto ID");
        if !prepare.is_empty() {
            session.execute(prepare).expect("prepare no-auto-ID case");
        }
        let mut result = session
            .execute(&format!("explain analyze {sql}"))
            .unwrap_or_else(|error| panic!("no-auto-ID case `{sql}` failed: {error}"))
            .remove(0);
        let row = result
            .Next()
            .expect("explain row")
            .expect("one explain row");
        assert!(!row[5].contains("auto_id_allocator"), "{sql}: {}", row[5]);
    }
}

#[test]
fn explain_analyze_insert_reports_foreign_key_check_phases() {
    let session = concrete_session();
    for sql in [
        "create table parent_runtime (id int key)",
        "create table child_runtime (id int key, parent_id int, foreign key (parent_id) references parent_runtime(id))",
        "insert into parent_runtime values (1)",
    ] {
        session
            .execute(sql)
            .expect("prepare foreign-key runtime stats");
    }

    let mut result = session
        .execute("explain analyze insert ignore into child_runtime values (1,1),(2,null)")
        .expect("explain foreign-key INSERT")
        .remove(0);
    let row = result
        .Next()
        .expect("explain row")
        .expect("one explain row");
    for field in [
        "time:",
        "loops:",
        "prepare:",
        "check_insert:",
        "total_time:",
        "mem_insert_time:",
        "prefetch:",
        "fk_check:",
    ] {
        assert!(
            row[5].contains(field),
            "missing {field} in INSERT execution info: {}",
            row[5]
        );
    }
}

#[test]
fn insert_on_duplicate_validates_the_updated_foreign_key_row() {
    let session = concrete_session();
    for sql in [
        "create table parent_upsert (a int, b int, unique index(a,b))",
        "create table child_upsert (id int key, a int, b int, foreign key(a,b) references parent_upsert(a,b))",
        "insert into parent_upsert values (11,21),(12,22)",
        "insert into child_upsert values (1,11,21)",
        "insert into child_upsert values (1,14,26) on duplicate key update a=12,b=22",
    ] {
        session.execute(sql).expect(sql);
    }
    let mut result = session
        .execute("select id,a,b from child_upsert")
        .expect("query updated child row")
        .remove(0);
    assert_eq!(
        result.Next().expect("updated row"),
        Some(vec!["1".to_owned(), "12".to_owned(), "22".to_owned()])
    );
}

#[test]
fn self_referencing_insert_sees_rows_staged_by_the_same_statement() {
    let session = concrete_session();
    session
        .execute(
            "create table employee_fk (id int key, leader int, foreign key(leader) references employee_fk(id) on delete cascade)",
        )
        .expect("create self-referencing table");
    session
        .execute("insert into employee_fk values (1,null),(10,1),(11,1),(20,10)")
        .expect("insert self-referencing hierarchy in one statement");
    let mut result = session
        .execute("select id,leader from employee_fk order by id")
        .expect("query self-referencing hierarchy")
        .remove(0);
    assert_eq!(
        result.Next().expect("root row"),
        Some(vec!["1".to_owned(), "<nil>".to_owned()])
    );
    assert_eq!(
        result.Next().expect("first child"),
        Some(vec!["10".to_owned(), "1".to_owned()])
    );
}

#[test]
fn replace_checks_only_the_unique_index_being_probed_for_dangling_entries() {
    let session = concrete_session();
    for sql in [
        "create table replace_parent (id int, a int, b int, unique index(id), unique index(a,b))",
        "create table replace_child (id int, a int, b int, unique index(id), unique index(a,b), foreign key(a,b) references replace_parent(a,b))",
        "replace into replace_parent values (1,1,1)",
        "replace into replace_child values (1,1,1)",
    ] {
        session.execute(sql).expect(sql);
    }
    let error = match session.execute("replace into replace_parent values (1,2,3)") {
        Err(error) => error,
        Ok(_) => panic!("referenced parent replacement must fail"),
    };
    assert!(
        error
            .to_string()
            .contains("Cannot delete or update a parent row"),
        "unexpected REPLACE error: {error}"
    );
}

#[test]
fn cascade_update_checks_tables_referencing_the_updated_child() {
    let session = concrete_session();
    for sql in [
        "create table cascade_parent (id int key)",
        "create table cascade_child (id int key, foreign key(id) references cascade_parent(id) on update cascade)",
        "create table cascade_grandchild (id int key, foreign key(id) references cascade_child(id))",
        "insert into cascade_parent values (1)",
        "insert into cascade_child values (1)",
        "insert into cascade_grandchild values (1)",
    ] {
        session.execute(sql).expect(sql);
    }
    let error = match session
        .execute("insert into cascade_parent values (1) on duplicate key update id=2")
    {
        Err(error) => error,
        Ok(_) => panic!("grandchild restrict edge must reject cascade update"),
    };
    assert!(
        error
            .to_string()
            .contains("Cannot delete or update a parent row"),
        "unexpected cascade restriction error: {error}"
    );
    for (table, expected) in [
        ("cascade_parent", "1"),
        ("cascade_child", "1"),
        ("cascade_grandchild", "1"),
    ] {
        let mut result = session
            .execute(&format!("select id from {table}"))
            .expect("query unchanged cascade table")
            .remove(0);
        assert_eq!(
            result.Next().expect("unchanged row"),
            Some(vec![expected.to_owned()])
        );
    }
}

#[test]
fn self_referencing_cascade_stops_at_the_depth_limit_without_recursing_on_itself() {
    let session = concrete_session();
    session
        .execute(
            "create table cascade_depth (id int key, pid int, foreign key(pid) references cascade_depth(id) on delete cascade)",
        )
        .expect("create cascade depth table");
    session
        .execute(
            "insert into cascade_depth values (0,0),(1,0),(2,1),(3,2),(4,3),(5,4),(6,5),(7,6),(8,7),(9,8),(10,9),(11,10),(12,11),(13,12),(14,13),(15,14)",
        )
        .expect("insert deep self-reference chain");
    let error = match session.execute("delete from cascade_depth where id=0") {
        Err(error) => error,
        Ok(_) => panic!("cascade deeper than 15 levels must fail"),
    };
    assert!(
        error.to_string().contains("cascade depth exceeded"),
        "unexpected cascade depth error: {error}"
    );
    session
        .execute("delete from cascade_depth where id=15")
        .expect("shorten cascade chain");
    session
        .execute("delete from cascade_depth where id=0")
        .expect("delete chain at supported depth");
}

#[test]
fn disabled_foreign_key_checks_skip_update_cascades() {
    let session = concrete_session();
    for sql in [
        "create table disabled_parent (id int key)",
        "create table disabled_child (id int key, pid int, foreign key(pid) references disabled_parent(id) on update cascade)",
        "insert into disabled_parent values (1)",
        "insert into disabled_child values (2,1)",
        "set foreign_key_checks=0",
        "update disabled_parent set id=10 where id=1",
    ] {
        session.execute(sql).expect(sql);
    }
    let mut result = session
        .execute("select pid from disabled_child")
        .expect("query child with checks disabled")
        .remove(0);
    assert_eq!(
        result.Next().expect("child row"),
        Some(vec!["1".to_owned()])
    );
}

#[test]
fn update_cascade_stops_at_the_depth_limit_atomically() {
    let session = concrete_session();
    session
        .execute("create table update_depth_0 (id int unique)")
        .expect("create cascade root");
    session
        .execute("insert into update_depth_0 values (1)")
        .expect("insert cascade root");
    for depth in 1..=16 {
        session
            .execute(&format!(
                "create table update_depth_{depth} (id int unique, foreign key(id) references update_depth_{}(id) on update cascade)",
                depth - 1
            ))
            .expect("create cascade child");
        session
            .execute(&format!("insert into update_depth_{depth} values (1)"))
            .expect("insert cascade child");
    }
    let error = match session.execute("update update_depth_0 set id=10 where id=1") {
        Err(error) => error,
        Ok(_) => panic!("update cascade deeper than 15 levels must fail"),
    };
    assert!(
        error.to_string().contains("cascade depth exceeded"),
        "unexpected cascade depth error: {error}"
    );
    let mut result = session
        .execute("select id from update_depth_0")
        .expect("query unchanged root")
        .remove(0);
    assert_eq!(result.Next().expect("root row"), Some(vec!["1".to_owned()]));

    session
        .execute("drop table update_depth_16")
        .expect("drop level beyond supported depth");
    session
        .execute("update update_depth_0 set id=10 where id=1")
        .expect("cascade through 15 levels");
    let mut result = session
        .execute("select id from update_depth_15")
        .expect("query deepest supported level")
        .remove(0);
    assert_eq!(
        result.Next().expect("deep row"),
        Some(vec!["10".to_owned()])
    );
}

#[test]
fn bit_arithmetic_uses_column_metadata_without_reinterpreting_strings() {
    let row = HashMap::from([
        ("bits".into(), Some("0xFF".into())),
        ("text_value".into(), Some("0xFF".into())),
        ("missing".into(), None),
    ]);
    let bit_columns = vec!["bits".to_owned(), "missing".to_owned()];
    for (sql, expected) in [
        ("bits+1", Some("256")),
        ("(bits)-1", Some("254")),
        ("bits*2", Some("510")),
        ("bits/2", Some("127.5")),
        ("bits%2", Some("1")),
        ("-bits", Some("-255")),
        ("+(bits)", Some("255")),
        ("(bits+1)*2", Some("512")),
        ("missing+1", None),
        ("bits", Some("0xFF")),
        ("text_value", Some("0xFF")),
    ] {
        let expr = crate::dml_runtime::ParseGeneratedExpr(sql).unwrap();
        assert_eq!(
            crate::dml_runtime::EvalExprWithBitColumns(&expr, &row, None, &bit_columns)
                .unwrap_or_else(|error| panic!("{sql}: {error}"))
                .as_deref(),
            expected,
            "{sql}"
        );
    }
    let expr = crate::dml_runtime::ParseGeneratedExpr("text_value+1").unwrap();
    assert!(crate::dml_runtime::EvalExprWithBitColumns(&expr, &row, None, &bit_columns).is_err());
}
