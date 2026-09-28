// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::TypeLonglong;

use crate::adapter::ExecExecutor;
use crate::physical_plan_runtime_test::{MemoryRetriever, encode_row};
use crate::typed_point_get::TypedPointGet;

fn columns() -> Vec<astersql_meta_model::ColumnInfo> {
    vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..Default::default()
    }]
}

#[test]
fn point_get_fetches_one_record_by_handle_without_a_range_iterator() {
    let retriever = Arc::new(MemoryRetriever::default());
    let (key, value) = encode_row(96, 4, 27, "one");
    retriever.Put(key.clone(), value);
    let mut point = TypedPointGet::new(
        retriever,
        96,
        96,
        false,
        columns(),
        Some(4),
        None,
        Vec::new(),
        0,
        false,
        1,
        1,
    );
    point.Open().unwrap();
    let mut output = point.NewChunk();
    point.Next(&mut output).unwrap();
    assert_eq!(output.GetRow(0).GetInt64(0), 27);
    assert_eq!(point.TakeLockKeys(), vec![key.0]);
    let mut detached = point.Detach().unwrap();
    point.Close().unwrap();
    detached.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    assert_eq!(detached.ScannedRows(), 1);
}

#[test]
fn point_get_resolves_unique_index_handle_then_fetches_partition_record() {
    let retriever = Arc::new(MemoryRetriever::default());
    let (record, row) = encode_row(972, 9, 45, "partition");
    retriever.Put(record.clone(), row);
    let encoded = astersql_util_codec::EncodeKey(
        astersql_tablecodec::time::UTC,
        Vec::new(),
        vec![astersql_types::datum::NewIntDatum(7)],
    )
    .unwrap();
    let index = astersql_tablecodec::EncodeIndexSeekKey(97, 6, Some(encoded));
    let mut value = vec![8, astersql_tablecodec::PartitionIDFlag];
    value = astersql_util_codec::EncodeInt(value, 972);
    value.extend_from_slice(&9_i64.to_be_bytes());
    retriever.Put(astersql_kv::Key(index.0), value);
    let mut point = TypedPointGet::new(
        retriever,
        97,
        97,
        false,
        columns(),
        None,
        Some(6),
        vec![astersql_types::datum::NewIntDatum(7)],
        1,
        false,
        1,
        1,
    );
    point.Open().unwrap();
    let mut output = point.NewChunk();
    point.Next(&mut output).unwrap();
    assert_eq!(output.GetRow(0).GetInt64(0), 45);
    assert_eq!(point.TakeLockKeys(), vec![record.0]);
}

#[test]
fn point_get_table_dual_and_missing_index_return_no_row_or_lock_key() {
    let retriever = Arc::new(MemoryRetriever::default());
    let mut dual = TypedPointGet::new(
        retriever.clone(),
        98,
        98,
        false,
        columns(),
        None,
        None,
        Vec::new(),
        0,
        true,
        1,
        1,
    );
    dual.Open().unwrap();
    let mut output = dual.NewChunk();
    dual.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    assert!(dual.TakeLockKeys().is_empty());
    assert_eq!(dual.ScannedRows(), 0);
    let mut missing = TypedPointGet::new(
        retriever,
        98,
        98,
        false,
        columns(),
        None,
        Some(6),
        vec![astersql_types::datum::NewIntDatum(7)],
        1,
        false,
        1,
        1,
    );
    missing.Open().unwrap();
    missing.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    assert!(missing.TakeLockKeys().is_empty());
}

#[test]
fn point_get_reports_index_record_mismatch_instead_of_returning_an_empty_result() {
    let retriever = Arc::new(MemoryRetriever::default());
    let encoded = astersql_util_codec::EncodeKey(
        astersql_tablecodec::time::UTC,
        Vec::new(),
        vec![astersql_types::datum::NewIntDatum(7)],
    )
    .unwrap();
    let index = astersql_tablecodec::EncodeIndexSeekKey(99, 6, Some(encoded));
    retriever.Put(astersql_kv::Key(index.0), 9_i64.to_be_bytes().to_vec());
    let mut point = TypedPointGet::new(
        retriever,
        99,
        99,
        false,
        columns(),
        None,
        Some(6),
        vec![astersql_types::datum::NewIntDatum(7)],
        1,
        false,
        1,
        1,
    );
    point.Open().unwrap();
    let mut output = point.NewChunk();
    let error = point
        .Next(&mut output)
        .expect_err("indexed record is missing");
    assert!(error.to_string().contains("8133"));
    assert_eq!(output.NumRows(), 0);
    assert!(point.TakeLockKeys().is_empty());
}

#[test]
fn physical_point_get_builder_uses_canonical_unique_index_key_encoding() {
    let retriever = Arc::new(MemoryRetriever::default());
    let (record, row) = encode_row(100, 9, 45, "indexed row");
    retriever.Put(record.clone(), row);
    let table = astersql_meta_model::TableInfo {
        ID: 100,
        Name: NewCIStr("t"),
        Columns: columns(),
        ..Default::default()
    };
    let index = astersql_meta_model::IndexInfo {
        ID: 6,
        Name: NewCIStr("idx_a"),
        Unique: true,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: NewCIStr("a"),
            Offset: 0,
            Length: astersql_parser_types::UnspecifiedLength,
            ..Default::default()
        }],
        ..Default::default()
    };
    let (index_key, distinct) = astersql_tablecodec::GenIndexKey(
        astersql_tablecodec::codec::NewEncoder(astersql_tablecodec::collate::NewCollationEnabled()),
        Some(astersql_tablecodec::time::UTC),
        Box::new(table.clone()),
        Box::new(index.clone()),
        100,
        vec![astersql_types::datum::NewIntDatum(7)],
        None,
        None,
    )
    .unwrap();
    assert!(distinct);
    retriever.Put(astersql_kv::Key(index_key), 9_i64.to_be_bytes().to_vec());
    let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
        crate::physical_plan_runtime_test::context(),
    );
    plan.TblInfo = Some(table);
    plan.IndexInfo = Some(index);
    plan.IndexValues = vec![astersql_types::datum::NewIntDatum(7)];
    plan.Columns = columns();
    let mut point = crate::builder::BuildTypedPointGet(&plan, retriever, 1, 1).unwrap();
    point.Open().unwrap();
    let mut output = point.NewChunk();
    point.Next(&mut output).unwrap();
    assert_eq!(output.GetRow(0).GetInt64(0), 45);
    assert_eq!(point.TakeLockKeys(), vec![record.0]);
}

#[test]
fn indexed_point_get_mismatch_uses_structured_consistency_reporter() {
    #[derive(Default)]
    struct CapturedLog(std::sync::Mutex<Vec<astersql_util_logutil_consistency::LogEntry>>);
    impl astersql_util_logutil_consistency::LogSink for CapturedLog {
        fn error(&self, entry: astersql_util_logutil_consistency::LogEntry) {
            self.0.lock().unwrap().push(entry);
        }
    }
    let retriever = Arc::new(MemoryRetriever::default());
    let table = astersql_meta_model::TableInfo {
        ID: 100,
        Name: NewCIStr("t"),
        Columns: columns(),
        ..Default::default()
    };
    let index = astersql_meta_model::IndexInfo {
        ID: 6,
        Name: NewCIStr("idx_a"),
        Unique: true,
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: NewCIStr("a"),
            Offset: 0,
            Length: astersql_parser_types::UnspecifiedLength,
            ..Default::default()
        }],
        ..Default::default()
    };
    let (index_key, distinct) = astersql_tablecodec::GenIndexKey(
        astersql_tablecodec::codec::NewEncoder(astersql_tablecodec::collate::NewCollationEnabled()),
        Some(astersql_tablecodec::time::UTC),
        Box::new(table.clone()),
        Box::new(index.clone()),
        100,
        vec![astersql_types::datum::NewIntDatum(7)],
        None,
        None,
    )
    .unwrap();
    assert!(distinct);
    retriever.Put(astersql_kv::Key(index_key), 9_i64.to_be_bytes().to_vec());
    let mut plan = astersql_planner_core_operator_physicalop::PointGetPlan::New(
        crate::physical_plan_runtime_test::context(),
    );
    plan.TblInfo = Some(table);
    plan.IndexInfo = Some(index);
    plan.IndexValues = vec![astersql_types::datum::NewIntDatum(7)];
    plan.Columns = columns();
    let logger = Arc::new(CapturedLog::default());
    let mut point = crate::builder::BuildTypedPointGet(&plan, retriever, 1, 1)
        .unwrap()
        .WithConsistencyDiagnostics(logger.clone(), None, "ON".into());
    point.Open().unwrap();
    let mut page = point.NewChunk();
    let error = point.Next(&mut page).unwrap_err();
    assert!(error.to_string().contains("8133"));
    assert_eq!(page.NumRows(), 0);
    let entries = logger.0.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0]
            .Fields
            .iter()
            .any(|field| field.Key == "table_name" && field.Value == "t")
    );
    assert!(
        entries[0]
            .Fields
            .iter()
            .any(|field| field.Key == "index_name" && field.Value == "idx_a")
    );
    drop(entries);
    point.Close().unwrap();
    point.SetDiagnosticMode(true, "ON".into());
    point.Open().unwrap();
    point
        .Next(&mut page)
        .expect("weak consistency tolerates missing record");
    assert_eq!(page.NumRows(), 0);
    assert_eq!(logger.0.lock().unwrap().len(), 1);
}
