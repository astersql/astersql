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
fn scalar_count_star_counts_only_record_keys_and_closes_iterator() {
    let closed = Arc::new(AtomicBool::new(false));
    let retriever = KeyOnlyCountRetriever {
        rows: 250_000,
        closed: Arc::clone(&closed),
    };
    let table = astersql_meta_model::TableInfo {
        ID: 42,
        ..Default::default()
    };

    assert_eq!(
        count_relational_rows(&retriever, &table).expect("count record keys"),
        250_000
    );
    assert!(closed.load(Ordering::Acquire));
}

#[test]
fn large_offset_locates_with_keys_then_reads_only_the_result_window_values() {
    let table = astersql_meta_model::TableInfo {
        ID: 42,
        PKIsHandle: true,
        ..Default::default()
    };
    let mut snapshot = OffsetWindowSnapshot {
        rows: [10_i64, 20, 30, 40, 50]
            .into_iter()
            .map(|handle| {
                (
                    kv::Key(
                        astersql_tablecodec::EncodeRowKeyWithHandle(
                            table.ID,
                            Box::new(astersql_tablecodec::kv::IntHandle(handle)),
                        )
                        .0,
                    ),
                    Vec::new(),
                )
            })
            .collect(),
        key_only: false,
        option_history: Vec::new(),
        iterator_modes: Mutex::new(Vec::new()),
    };

    let rows = scan_relational_rows_window_key_only(&mut snapshot, &table, 3, 2, None)
        .expect("scan sparse handles with a key-only OFFSET locator");
    assert_eq!(
        rows.iter().map(|(handle, _)| *handle).collect::<Vec<_>>(),
        vec![40, 50]
    );
    assert_eq!(snapshot.option_history, vec![true, false]);
    assert_eq!(
        *snapshot.iterator_modes.lock().unwrap(),
        vec![true, false],
        "the locator must be key-only and the result iterator must fetch values"
    );
}

#[test]
fn integer_handle_seek_compensates_sparse_primary_key_gaps() {
    let table = astersql_meta_model::TableInfo {
        ID: 43,
        PKIsHandle: true,
        ..Default::default()
    };
    let mut snapshot = OffsetWindowSnapshot {
        rows: [10_i64, 20, 30, 40, 50]
            .into_iter()
            .map(|handle| {
                (
                    kv::Key(
                        astersql_tablecodec::EncodeRowKeyWithHandle(
                            table.ID,
                            Box::new(astersql_tablecodec::kv::IntHandle(handle)),
                        )
                        .0,
                    ),
                    Vec::new(),
                )
            })
            .collect(),
        key_only: false,
        option_history: Vec::new(),
        iterator_modes: Mutex::new(Vec::new()),
    };

    let candidate = integer_handle_offset_candidate(&mut snapshot, &table, 3)
        .expect("build integer handle seek candidate")
        .expect("integer clustered primary key supports seek");
    let (_, candidate_handle) =
        astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(candidate.0.clone()))
            .expect("decode seek candidate");
    assert_eq!(candidate_handle.IntValue(), 13);

    // Only handle 10 exists before candidate 13, so two more visible rows
    // must still be skipped after seeking. This is the exact count returned
    // by the TiKV range aggregate in production.
    let rows = scan_relational_rows_window_key_only_from(
        &mut snapshot,
        &table,
        2,
        2,
        None,
        Some(candidate),
    )
    .expect("scan sparse handles after the compensated seek");
    assert_eq!(
        rows.iter().map(|(handle, _)| *handle).collect::<Vec<_>>(),
        vec![40, 50]
    );
    assert_eq!(snapshot.option_history, vec![true, false, true, false]);
    assert_eq!(
        *snapshot.iterator_modes.lock().unwrap(),
        vec![true, true, false]
    );
}

#[test]
fn primary_key_cursor_builds_exact_signed_and_unsigned_record_ranges() {
    let primary_column = |unsigned: bool| astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("order_id"),
        FieldType: {
            let mut field_type =
                astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
            field_type.AddFlag(astersql_parser_mysql::r#type::PriKeyFlag);
            if unsigned {
                field_type.AddFlag(astersql_parser_mysql::r#type::UnsignedFlag);
            }
            field_type
        },
        ..Default::default()
    };
    let predicate = |sql: &str| {
        let (statements, _) = astersql_parser::Parser::default()
            .ParseSQL(sql, &[])
            .expect("parse primary-key cursor query");
        statements[0]
            .as_any()
            .downcast_ref::<astersql_parser_ast::SelectStmt>()
            .expect("cursor query SELECT")
            .Where
            .clone()
            .expect("cursor query predicate")
    };

    let signed = astersql_meta_model::TableInfo {
        ID: 44,
        PKIsHandle: true,
        Columns: vec![primary_column(false)],
        ..Default::default()
    };
    let signed_ranges = relational_primary_key_scan_ranges(
        &signed,
        &predicate("select * from orders_500m where order_id > 12000000"),
        Some(false),
    )
    .expect("signed primary-key predicate must be an exact access range");
    assert_eq!(signed_ranges.len(), 1);
    let (_, signed_lower) = astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(
        signed_ranges[0].0.0.clone(),
    ))
    .expect("decode signed cursor lower bound");
    assert_eq!(signed_lower.IntValue(), 12_000_001);
    assert!(!signed_ranges[0].2);

    let unsigned = astersql_meta_model::TableInfo {
        ID: 45,
        PKIsHandle: true,
        Columns: vec![primary_column(true)],
        ..Default::default()
    };
    let unsigned_ranges = relational_primary_key_scan_ranges(
        &unsigned,
        &predicate("select * from orders_500m where order_id > 9223372036854775808"),
        Some(false),
    )
    .expect("unsigned primary-key predicate must be an exact access range");
    assert_eq!(unsigned_ranges.len(), 1);
    let (_, unsigned_lower) = astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(
        unsigned_ranges[0].0.0.clone(),
    ))
    .expect("decode unsigned cursor lower bound");
    assert_eq!(unsigned_lower.IntValue(), i64::MIN + 1);
    assert_eq!(
        unsigned_ranges[0].1,
        kv::Key(
            astersql_tablecodec::EncodeRowKeyWithHandle(
                unsigned.ID,
                Box::new(astersql_tablecodec::kv::IntHandle(0)),
            )
            .0,
        )
    );
}

#[test]
fn scalar_count_star_builds_tikv_aggregation_and_sums_region_partials() {
    let (statements, _) = astersql_parser::Parser::default()
        .ParseSQL("select count(*) from orders_500m", &[])
        .expect("parse scalar COUNT(*)");
    let select = statements[0]
        .as_any()
        .downcast_ref::<astersql_parser_ast::SelectStmt>()
        .expect("COUNT(*) select AST");
    assert!(
        is_scalar_count_non_null_constant(select),
        "COUNT(*) must use the TiKV aggregation fast path; AST={:?}",
        select.Fields.Fields[0].Expr
    );
    assert!(
        !concrete_session().relational_select_requires_full_query(select),
        "scalar COUNT(*) must not be routed through the materializing full-query executor"
    );

    let table = astersql_meta_model::TableInfo {
        ID: 42,
        PKIsHandle: true,
        Columns: vec![
            astersql_meta_model::ColumnInfo {
                ID: 1,
                Name: astersql_parser_ast::NewCIStr("id"),
                FieldType: {
                    let mut field_type = astersql_parser_types::NewFieldType(
                        astersql_parser_mysql::r#type::TypeLonglong,
                    );
                    field_type.AddFlag(astersql_parser_mysql::r#type::PriKeyFlag);
                    field_type
                },
                ..Default::default()
            },
            astersql_meta_model::ColumnInfo {
                ID: 2,
                Name: astersql_parser_ast::NewCIStr("user_id"),
                FieldType: {
                    let mut field_type = astersql_parser_types::NewFieldType(
                        astersql_parser_mysql::r#type::TypeLonglong,
                    );
                    field_type.AddFlag(astersql_parser_mysql::r#type::UnsignedFlag);
                    field_type.AddFlag(astersql_parser_mysql::r#type::NotNullFlag);
                    field_type
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let encoded = relational_count_dag(&table).expect("encode COUNT DAG");
    let dag: tipb::DagRequest = protobuf::parse_from_bytes(&encoded).expect("decode COUNT DAG");
    assert_eq!(dag.get_executors().len(), 2);
    assert_eq!(
        dag.get_executors()[0].get_tp(),
        tipb::ExecType::TypeTableScan
    );
    assert_eq!(dag.get_executors()[0].get_tbl_scan().get_columns().len(), 1);
    assert!(dag.get_executors()[0].get_tbl_scan().get_columns()[0].get_pk_handle());
    assert_eq!(
        dag.get_executors()[1].get_tp(),
        tipb::ExecType::TypeAggregation
    );
    let aggregate = dag.get_executors()[1].get_aggregation();
    assert_eq!(aggregate.get_agg_func().len(), 1);
    assert_eq!(aggregate.get_agg_func()[0].get_tp(), tipb::ExprType::Count);
    assert_eq!(
        aggregate.get_agg_func()[0].get_agg_func_mode(),
        tipb::AggFunctionMode::Partial1Mode
    );
    assert_eq!(
        aggregate.get_agg_func()[0].get_children()[0].get_tp(),
        tipb::ExprType::Int64
    );

    let mut first = astersql_tablecodec::types::Datum::default();
    first.SetUint64(120_000);
    let mut second = astersql_tablecodec::types::Datum::default();
    second.SetUint64(130_000);
    let rows_data = astersql_tablecodec::codec::EncodeValue(
        astersql_tablecodec::time::UTC,
        Vec::new(),
        vec![first, second],
    )
    .expect("encode partial COUNT rows");
    let mut chunk = tipb::Chunk::new();
    chunk.set_rows_data(rows_data);
    let mut response = tipb::SelectResponse::new();
    response.set_chunks(protobuf::RepeatedField::from_vec(vec![chunk]));
    let response = protobuf::Message::write_to_bytes(&response).expect("encode COUNT response");

    assert_eq!(
        decode_relational_count_response(&response).expect("sum partial COUNT rows"),
        250_000
    );

    let encoded = relational_count_checksum().expect("encode COUNT checksum");
    let checksum: tipb::ChecksumRequest =
        protobuf::parse_from_bytes(&encoded).expect("decode COUNT checksum");
    assert_eq!(checksum.get_scan_on(), tipb::ChecksumScanOn::Table);
    assert_eq!(checksum.get_algorithm(), tipb::ChecksumAlgorithm::Crc64Xor);

    let mut response = tipb::ChecksumResponse::new();
    response.set_total_kvs(250_000);
    let response = protobuf::Message::write_to_bytes(&response).expect("encode checksum response");
    assert_eq!(
        decode_relational_checksum_count_response(&response)
            .expect("decode checksum COUNT response"),
        250_000
    );
}

/// 构造带 TestSchemaLoader 的具体测试运行时。
#[test]
fn relational_select_applies_limit_offset_window() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database pagination_runtime")
        .expect("create pagination database");
    session
        .execute("use pagination_runtime")
        .expect("select pagination database");
    session
        .execute("create table events (id bigint primary key, payload varchar(32))")
        .expect("create pagination table");
    session
        .execute(
            "insert into events values \
             (1, 'one'),(2, 'two'),(3, 'three'),(4, 'four'),(5, 'five')",
        )
        .expect("seed pagination rows");

    let mut page = session
        .execute("select id, payload from events limit 2 offset 2")
        .expect("execute simple pagination")
        .remove(0);
    assert_eq!(
        page.next_row().expect("read first simple page row"),
        Some(vec!["3".to_owned(), "three".to_owned()])
    );
    assert_eq!(
        page.next_row().expect("read second simple page row"),
        Some(vec!["4".to_owned(), "four".to_owned()])
    );
    assert_eq!(page.next_row().expect("read simple page end"), None);

    let mut ordered_page = session
        .execute("select id from events order by id desc limit 2 offset 1")
        .expect("execute ordered pagination")
        .remove(0);
    assert_eq!(
        ordered_page
            .next_row()
            .expect("read first ordered page row"),
        Some(vec!["4".to_owned()])
    );
    assert_eq!(
        ordered_page
            .next_row()
            .expect("read second ordered page row"),
        Some(vec!["3".to_owned()])
    );
    assert_eq!(
        ordered_page.next_row().expect("read ordered page end"),
        None
    );

    let mut cursor_page = session
        .execute("select id from events where id > 2 order by id limit 2")
        .expect("execute primary-key cursor pagination")
        .remove(0);
    assert_eq!(
        cursor_page.next_row().expect("read first cursor row"),
        Some(vec!["3".to_owned()])
    );
    assert_eq!(
        cursor_page.next_row().expect("read second cursor row"),
        Some(vec!["4".to_owned()])
    );
    assert_eq!(cursor_page.next_row().expect("read cursor page end"), None);

    session
        .execute(
            "create table unsigned_events (
                id bigint unsigned primary key,
                payload varchar(32)
            )",
        )
        .expect("create unsigned pagination table");
    session
        .execute(
            "insert into unsigned_events values
             (1, 'one'),
             (9223372036854775807, 'signed-max'),
             (9223372036854775808, 'unsigned-half'),
             (18446744073709551615, 'unsigned-max')",
        )
        .expect("seed unsigned pagination rows");
    let mut unsigned_page = session
        .execute("select id from unsigned_events order by id desc limit 2 offset 1")
        .expect("execute unsigned ordered pagination")
        .remove(0);
    assert_eq!(
        unsigned_page
            .next_row()
            .expect("read first unsigned ordered row"),
        Some(vec!["9223372036854775808".to_owned()])
    );
    assert_eq!(
        unsigned_page
            .next_row()
            .expect("read second unsigned ordered row"),
        Some(vec!["9223372036854775807".to_owned()])
    );
    assert_eq!(
        unsigned_page
            .next_row()
            .expect("read unsigned ordered page end"),
        None
    );
}

#[test]
fn secondary_index_kv_tracks_relational_dml_and_backfill() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database secondary_index_runtime")
        .expect("create secondary-index database");
    session
        .execute("use secondary_index_runtime")
        .expect("select secondary-index database");
    session
        .execute(
            "create table events (
                id bigint primary key,
                created_at bigint not null,
                payload varchar(32)
            )",
        )
        .expect("create indexed table");
    session
        .execute("alter table events add index idx_created_id(created_at, id)")
        .expect("add secondary index before inserts");
    session
        .execute("insert into events values (1, 10, 'one'), (2, 20, 'two'), (3, 30, 'three')")
        .expect("seed indexed rows");

    let scan_index_handles = |table: &astersql_meta_model::TableInfo, index_name: &str| {
        let index = table
            .Indices
            .iter()
            .find(|index| index.Name.L == index_name)
            .expect("secondary index metadata");
        let (start, end) = astersql_tablecodec::GetTableIndexKeyRange(table.ID, index.ID);
        session.domain().storage().with_storage(|store| {
            let version = store.CurrentVersion("global").expect("current version");
            let snapshot = store.GetSnapshot(version);
            let mut iterator = snapshot
                .Iter(kv::Key(start), Some(kv::Key(end)))
                .expect("scan secondary-index range");
            let mut handles = Vec::new();
            while iterator.Valid() {
                let handle = astersql_tablecodec::DecodeIndexHandle(
                    iterator.Key().0,
                    iterator.Value(),
                    index.Columns.len(),
                )
                .expect("decode secondary-index handle")
                .expect("secondary-index value must contain a handle");
                handles.push(handle.IntValue());
                iterator.Next().expect("advance secondary-index iterator");
            }
            iterator.Close();
            handles
        })
    };

    let (_, table) = session
        .domain()
        .stats_table("secondary_index_runtime", "events")
        .expect("indexed table metadata");
    assert_eq!(scan_index_handles(&table, "idx_created_id"), vec![1, 2, 3]);

    session
        .execute("update events set created_at = 25 where id = 2")
        .expect("update indexed value");
    session
        .execute("delete from events where id = 1")
        .expect("delete indexed row");
    let (_, table) = session
        .domain()
        .stats_table("secondary_index_runtime", "events")
        .expect("updated indexed table metadata");
    assert_eq!(scan_index_handles(&table, "idx_created_id"), vec![2, 3]);

    session
        .execute("create table backfill_events (id bigint primary key, created_at bigint not null)")
        .expect("create backfill table");
    session
        .execute("insert into backfill_events values (4, 40), (5, 50)")
        .expect("seed rows before ADD INDEX");
    session
        .execute("alter table backfill_events add index idx_created_id(created_at, id)")
        .expect("add and backfill secondary index");
    let (_, table) = session
        .domain()
        .stats_table("secondary_index_runtime", "backfill_events")
        .expect("backfilled table metadata");
    assert_eq!(scan_index_handles(&table, "idx_created_id"), vec![4, 5]);
}

#[test]
fn secondary_index_cursor_uses_ordered_index_lookup() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database secondary_cursor_runtime")
        .expect("create cursor database");
    session
        .execute("use secondary_cursor_runtime")
        .expect("select cursor database");
    session
        .execute(
            "create table events (
                id bigint primary key,
                created_at bigint not null,
                payload varchar(32)
            )",
        )
        .expect("create cursor table");
    session
        .execute("alter table events add index idx_created_id(created_at, id)")
        .expect("add cursor index");
    session
        .execute(
            "insert into events values
             (1, 10, 'one'), (2, 20, 'two'), (3, 20, 'three'),
             (4, 30, 'four'), (5, 40, 'five')",
        )
        .expect("seed cursor rows");

    let (statements, _) = astersql_parser::Parser::default()
        .ParseSQL(
            "select * from events
             where created_at > 20
             order by created_at, id
             limit 2",
            &[],
        )
        .expect("parse secondary-index cursor");
    let select = statements[0]
        .as_any()
        .downcast_ref::<astersql_parser_ast::SelectStmt>()
        .expect("cursor SELECT");
    let (_, table) = session
        .domain()
        .stats_table("secondary_cursor_runtime", "events")
        .expect("cursor table metadata");
    let access = session
        .relational_secondary_index_access(&table, select)
        .expect("secondary-index access path");
    assert_eq!(access.index.Name.L, "idx_created_id");
    assert_eq!(access.ranges.len(), 1);

    let mut page = session
        .execute(
            "select id, created_at, payload from events
             where created_at > 20
             order by created_at, id
             limit 2",
        )
        .expect("execute secondary-index cursor")
        .remove(0);
    assert_eq!(
        page.next_row().expect("first index lookup row"),
        Some(vec!["4".to_owned(), "30".to_owned(), "four".to_owned()])
    );
    assert_eq!(
        page.next_row().expect("second index lookup row"),
        Some(vec!["5".to_owned(), "40".to_owned(), "five".to_owned()])
    );
    assert_eq!(page.next_row().expect("index lookup end"), None);

    let mut tuple_page = session
        .execute(
            "select id, created_at, payload from events
             where (created_at, id) > (20, 2)
             order by created_at, id
             limit 2",
        )
        .expect("execute composite secondary-index cursor")
        .remove(0);
    assert_eq!(
        tuple_page.next_row().expect("first composite cursor row"),
        Some(vec!["3".to_owned(), "20".to_owned(), "three".to_owned()])
    );
    assert_eq!(
        tuple_page.next_row().expect("second composite cursor row"),
        Some(vec!["4".to_owned(), "30".to_owned(), "four".to_owned()])
    );

    let mut descending_page = session
        .execute(
            "select id, created_at, payload from events
             where created_at > 10
             order by created_at desc, id desc
             limit 2",
        )
        .expect("execute descending secondary-index cursor")
        .remove(0);
    assert_eq!(
        descending_page
            .next_row()
            .expect("first descending cursor row"),
        Some(vec!["5".to_owned(), "40".to_owned(), "five".to_owned()])
    );
    assert_eq!(
        descending_page
            .next_row()
            .expect("second descending cursor row"),
        Some(vec!["4".to_owned(), "30".to_owned(), "four".to_owned()])
    );

    session
        .execute(
            "insert into events values
             (202, 101, 'two-zero-two'), (203, 101, 'two-zero-three'),
             (204, 102, 'two-zero-four')",
        )
        .expect("seed binary-handle batch-get collision rows");
    let mut binary_handle_page = session
        .execute(
            "select id, created_at from events
             where created_at > 100
             order by created_at, id
             limit 3",
        )
        .expect("execute binary-handle secondary-index cursor")
        .remove(0);
    assert_eq!(
        binary_handle_page
            .next_row()
            .expect("first binary-handle row"),
        Some(vec!["202".to_owned(), "101".to_owned()])
    );
    assert_eq!(
        binary_handle_page
            .next_row()
            .expect("second binary-handle row"),
        Some(vec!["203".to_owned(), "101".to_owned()])
    );
    assert_eq!(
        binary_handle_page
            .next_row()
            .expect("third binary-handle row"),
        Some(vec!["204".to_owned(), "102".to_owned()])
    );
}

#[test]
fn secondary_index_equality_limit_does_not_require_order_by() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database secondary_equality_runtime")
        .expect("create equality database");
    session
        .execute("use secondary_equality_runtime")
        .expect("select equality database");
    session
        .execute(
            "create table orders (
                id bigint primary key,
                user_id bigint not null,
                payload varchar(32),
                index idx_user_id(user_id)
            )",
        )
        .expect("create equality table");
    session
        .execute(
            "insert into orders values
             (1, 7, 'skip-one'), (2, 14, 'one'), (3, 7, 'skip-two'),
             (4, 14, 'two'), (5, 14, 'three'), (6, 14, 'four'),
             (7, 14, 'not-read')",
        )
        .expect("seed equality rows");

    let (statements, _) = astersql_parser::Parser::default()
        .ParseSQL("select * from orders where user_id = 14 limit 4", &[])
        .expect("parse equality lookup");
    let select = statements[0]
        .as_any()
        .downcast_ref::<astersql_parser_ast::SelectStmt>()
        .expect("equality SELECT");
    let (_, table) = session
        .domain()
        .stats_table("secondary_equality_runtime", "orders")
        .expect("equality table metadata");
    let access = session
        .relational_secondary_index_access(&table, select)
        .expect("equality predicate must select its secondary index");
    assert_eq!(access.index.Name.L, "idx_user_id");

    let mut result = session
        .execute("select id, user_id, payload from orders where user_id = 14 limit 4")
        .expect("execute equality index lookup")
        .remove(0);
    assert_eq!(
        result.next_row().expect("first equality row"),
        Some(vec!["2".to_owned(), "14".to_owned(), "one".to_owned()])
    );
    assert_eq!(
        result.next_row().expect("second equality row"),
        Some(vec!["4".to_owned(), "14".to_owned(), "two".to_owned()])
    );
    assert_eq!(
        result.next_row().expect("third equality row"),
        Some(vec!["5".to_owned(), "14".to_owned(), "three".to_owned()])
    );
    assert_eq!(
        result.next_row().expect("fourth equality row"),
        Some(vec!["6".to_owned(), "14".to_owned(), "four".to_owned()])
    );
    assert_eq!(result.next_row().expect("equality lookup end"), None);
}
