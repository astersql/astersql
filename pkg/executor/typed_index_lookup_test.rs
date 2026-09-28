// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::TypeLonglong;

use crate::adapter::ExecExecutor;
use crate::physical_plan_runtime_test::{MemoryRetriever, encode_row};
use crate::typed_index_lookup::TypedIndexLookUp;
use crate::typed_kv_scan::{KeyRange, TypedKVScan};

#[test]
fn typed_index_lookup_reads_canonical_index_kv_then_records_in_index_order_and_returns_record_lock_keys()
 {
    let retriever = Arc::new(MemoryRetriever::default());
    for (handle, value) in [(1, 10), (2, 20)] {
        let (key, row) = encode_row(92, handle, value, "row");
        retriever.Put(key, row);
    }
    for (index_value, handle) in [(8, 1), (5, 2)] {
        let mut suffix = astersql_util_codec::EncodeKey(
            astersql_tablecodec::time::UTC,
            Vec::new(),
            vec![astersql_types::datum::NewIntDatum(index_value)],
        )
        .expect("encode canonical index datum");
        suffix.push(astersql_util_codec::IntHandleFlag);
        suffix = astersql_util_codec::EncodeInt(suffix, handle);
        let key = astersql_tablecodec::EncodeIndexSeekKey(92, 7, Some(suffix));
        retriever.Put(astersql_kv::Key(key.0), vec![0]);
    }
    let start = astersql_tablecodec::EncodeIndexSeekKey(92, 7, None);
    let columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..Default::default()
    }];
    let decoder = TypedKVScan::new(
        retriever.clone(),
        92,
        false,
        false,
        columns,
        Vec::new(),
        1,
        1,
    );
    let mut lookup = TypedIndexLookUp::new(
        retriever,
        decoder,
        1,
        92,
        false,
        vec![KeyRange {
            end: start.PrefixNext(),
            start: astersql_kv::Key(start.0),
        }],
    );
    lookup.Open().expect("open lazy double read");
    let mut output = lookup.NewChunk();
    lookup
        .Next(&mut output)
        .expect("first index entry and record");
    assert_eq!(output.GetRow(0).GetInt64(0), 20);
    let (first_record, _) = encode_row(92, 2, 20, "row");
    assert_eq!(lookup.TakeLockKeys(), vec![first_record.0]);
    let mut detached = lookup.Detach().expect("owned lookup can detach");
    lookup.Close().expect("close original lookup");
    detached
        .Next(&mut output)
        .expect("detached second index entry");
    assert_eq!(output.GetRow(0).GetInt64(0), 10);
    let (second_record, _) = encode_row(92, 1, 10, "row");
    assert_eq!(detached.TakeLockKeys(), vec![second_record.0]);
    detached.Next(&mut output).expect("index EOF");
    assert_eq!(output.NumRows(), 0);
    assert_eq!(detached.ScannedRows(), 2);
}

#[test]
fn typed_index_lookup_fetches_global_index_record_from_its_partition() {
    let retriever = Arc::new(MemoryRetriever::default());
    let (record_key, row) = encode_row(942, 9, 77, "partition row");
    retriever.Put(record_key.clone(), row);
    let mut suffix = astersql_util_codec::EncodeKey(
        astersql_tablecodec::time::UTC,
        Vec::new(),
        vec![astersql_types::datum::NewIntDatum(7)],
    )
    .expect("encode global index datum");
    suffix.push(astersql_tablecodec::PartitionIDFlag);
    suffix = astersql_util_codec::EncodeInt(suffix, 942);
    suffix.push(astersql_util_codec::IntHandleFlag);
    suffix = astersql_util_codec::EncodeInt(suffix, 9);
    let key = astersql_tablecodec::EncodeIndexSeekKey(94, 7, Some(suffix));
    let mut value = vec![0, astersql_tablecodec::PartitionIDFlag];
    value = astersql_util_codec::EncodeInt(value, 942);
    value.resize(10, 0);
    value[0] = (value.len() - 10) as u8;
    retriever.Put(astersql_kv::Key(key.0), value);
    let start = astersql_tablecodec::EncodeIndexSeekKey(94, 7, None);
    let columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..Default::default()
    }];
    let decoder = TypedKVScan::new(
        retriever.clone(),
        94,
        false,
        false,
        columns,
        Vec::new(),
        1,
        1,
    );
    let mut lookup = TypedIndexLookUp::new(
        retriever,
        decoder,
        1,
        94,
        false,
        vec![KeyRange {
            end: start.PrefixNext(),
            start: astersql_kv::Key(start.0),
        }],
    );
    lookup.Open().unwrap();
    let mut output = lookup.NewChunk();
    lookup.Next(&mut output).unwrap();
    assert_eq!(output.GetRow(0).GetInt64(0), 77);
    assert_eq!(lookup.TakeLockKeys(), vec![record_key.0]);
}

#[test]
fn typed_index_lookup_builder_streams_pushed_limit_and_returned_lock_key() {
    let retriever = Arc::new(MemoryRetriever::default());
    for handle in 1..=3 {
        let (key, row) = encode_row(96, handle, handle * 10, "row");
        retriever.Put(key, row);
        let mut suffix = astersql_util_codec::EncodeKey(
            astersql_tablecodec::time::UTC,
            Vec::new(),
            vec![astersql_types::datum::NewIntDatum(handle)],
        )
        .unwrap();
        suffix.push(astersql_util_codec::IntHandleFlag);
        suffix = astersql_util_codec::EncodeInt(suffix, handle);
        let key = astersql_tablecodec::EncodeIndexSeekKey(96, 7, Some(suffix));
        retriever.Put(astersql_kv::Key(key.0), vec![0]);
    }
    let columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..Default::default()
    }];
    let table = astersql_meta_model::TableInfo {
        ID: 96,
        Name: NewCIStr("t"),
        Columns: columns.clone(),
        ..Default::default()
    };
    let index_info = astersql_meta_model::IndexInfo {
        ID: 7,
        Name: NewCIStr("idx_a"),
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: NewCIStr("a"),
            Offset: 0,
            Length: astersql_parser_types::UnspecifiedLength,
            ..Default::default()
        }],
        ..Default::default()
    };
    let context = crate::physical_plan_runtime_test::context();
    let mut index =
        astersql_planner_core_operator_physicalop::PhysicalIndexScan::New(context.clone());
    index.Table = Some(table.clone());
    index.Index = Some(index_info);
    index.Columns = columns.clone();
    let mut table_scan =
        astersql_planner_core_operator_physicalop::PhysicalTableScan::New(context.clone());
    table_scan.Table = Some(table);
    table_scan.Columns = columns;
    let mut reader =
        astersql_planner_core_operator_physicalop::PhysicalIndexLookUpReader::New(context);
    reader.IndexPlan = Some(Box::new(index));
    reader.TablePlan = Some(Box::new(table_scan));
    reader.IndexLookUpPushDown = true;
    reader.PushedLimit = Some(astersql_planner_core_operator_physicalop::PushedDownLimit {
        Offset: 1,
        Count: 1,
    });
    let start = astersql_tablecodec::EncodeIndexSeekKey(96, 7, None);
    let mut executor = crate::builder::BuildTypedPhysicalPlan(
        &reader,
        retriever.clone(),
        vec![KeyRange {
            end: start.PrefixNext(),
            start: astersql_kv::Key(start.0),
        }],
        1,
        1,
    )
    .expect("build pushed index lookup");
    executor.Open().unwrap();
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 1);
    assert_eq!(output.GetRow(0).GetInt64(0), 20);
    let (record_key, _) = encode_row(96, 2, 20, "row");
    assert_eq!(executor.TakeLockKeys(), vec![record_key.0]);
    assert!(executor.Detach().is_some());
    let calls = retriever.IterCalls();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    assert_eq!(retriever.IterCalls(), calls);
}
