// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::TypeVarchar;

use crate::adapter::ExecExecutor;
use crate::physical_plan_runtime_test::MemoryRetriever;
use crate::typed_index_reader::TypedIndexReader;
use crate::typed_kv_scan::KeyRange;

#[test]
fn typed_index_reader_detaches_its_cursor_and_preserves_descending_record_keys() {
    let retriever = Arc::new(MemoryRetriever::default());
    for (indexed, handle) in [("aa", 1), ("bb", 2)] {
        let mut suffix = astersql_util_codec::EncodeKey(
            astersql_tablecodec::time::UTC,
            Vec::new(),
            vec![astersql_types::datum::NewStringDatum(indexed.into())],
        )
        .expect("encode canonical index column");
        suffix.push(astersql_util_codec::IntHandleFlag);
        suffix = astersql_util_codec::EncodeInt(suffix, handle);
        let key = astersql_tablecodec::EncodeIndexSeekKey(93, 8, Some(suffix));
        retriever.Put(astersql_kv::Key(key.0), vec![0]);
    }
    let start = astersql_tablecodec::EncodeIndexSeekKey(93, 8, None);
    let range = KeyRange {
        end: start.PrefixNext(),
        start: astersql_kv::Key(start.0),
    };
    let output_columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeVarchar),
        ..Default::default()
    }];
    let mut reader = TypedIndexReader::new(
        retriever,
        93,
        vec![1],
        output_columns,
        true,
        vec![range],
        1,
        1,
    );
    reader.Open().expect("open covered index scan");
    let mut output = reader.NewChunk();
    reader.Next(&mut output).expect("read descending bb first");
    assert_eq!(output.GetRow(0).GetBytes(0), b"bb");
    let record =
        astersql_tablecodec::EncodeRowKeyWithHandle(93, Box::new(astersql_kv::IntHandle(2)));
    assert_eq!(reader.TakeLockKeys(), vec![record.0]);
    let mut detached = reader.Detach().expect("covered index has owned cursor");
    reader.Close().expect("close original index reader");
    detached
        .Next(&mut output)
        .expect("detached descending aa page");
    assert_eq!(output.GetRow(0).GetBytes(0), b"aa");
    let record =
        astersql_tablecodec::EncodeRowKeyWithHandle(93, Box::new(astersql_kv::IntHandle(1)));
    assert_eq!(detached.TakeLockKeys(), vec![record.0]);
    detached.Next(&mut output).expect("covered index EOF");
    assert_eq!(output.NumRows(), 0);
}

#[test]
fn typed_index_reader_uses_global_index_partition_record_lock_key() {
    let retriever = Arc::new(MemoryRetriever::default());
    let mut suffix = astersql_util_codec::EncodeKey(
        astersql_tablecodec::time::UTC,
        Vec::new(),
        vec![astersql_types::datum::NewStringDatum("aa".into())],
    )
    .unwrap();
    suffix.push(astersql_tablecodec::PartitionIDFlag);
    suffix = astersql_util_codec::EncodeInt(suffix, 952);
    suffix.push(astersql_util_codec::IntHandleFlag);
    suffix = astersql_util_codec::EncodeInt(suffix, 8);
    let key = astersql_tablecodec::EncodeIndexSeekKey(95, 8, Some(suffix));
    let mut value = vec![0, astersql_tablecodec::PartitionIDFlag];
    value = astersql_util_codec::EncodeInt(value, 952);
    value.resize(10, 0);
    value[0] = (value.len() - 10) as u8;
    retriever.Put(astersql_kv::Key(key.0), value);
    let start = astersql_tablecodec::EncodeIndexSeekKey(95, 8, None);
    let columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeVarchar),
        ..Default::default()
    }];
    let mut reader = TypedIndexReader::new(
        retriever,
        95,
        vec![1],
        columns,
        false,
        vec![KeyRange {
            end: start.PrefixNext(),
            start: astersql_kv::Key(start.0),
        }],
        1,
        1,
    );
    reader.Open().unwrap();
    let mut output = reader.NewChunk();
    reader.Next(&mut output).unwrap();
    assert_eq!(output.GetRow(0).GetBytes(0), b"aa");
    let record =
        astersql_tablecodec::EncodeRowKeyWithHandle(952, Box::new(astersql_kv::IntHandle(8)));
    assert_eq!(reader.TakeLockKeys(), vec![record.0]);
}

#[test]
fn typed_index_reader_builder_executes_nested_limit_without_extra_scan() {
    let retriever = Arc::new(MemoryRetriever::default());
    for (indexed, handle) in [("aa", 1), ("bb", 2), ("cc", 3)] {
        let mut suffix = astersql_util_codec::EncodeKey(
            astersql_tablecodec::time::UTC,
            Vec::new(),
            vec![astersql_types::datum::NewStringDatum(indexed.into())],
        )
        .unwrap();
        suffix.push(astersql_util_codec::IntHandleFlag);
        suffix = astersql_util_codec::EncodeInt(suffix, handle);
        let key = astersql_tablecodec::EncodeIndexSeekKey(97, 8, Some(suffix));
        retriever.Put(astersql_kv::Key(key.0), vec![0]);
    }
    let columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeVarchar),
        ..Default::default()
    }];
    let table = astersql_meta_model::TableInfo {
        ID: 97,
        Name: NewCIStr("t"),
        Columns: columns.clone(),
        ..Default::default()
    };
    let mut scan = astersql_planner_core_operator_physicalop::PhysicalIndexScan::New(
        crate::physical_plan_runtime_test::context(),
    );
    scan.Table = Some(table);
    scan.Index = Some(astersql_meta_model::IndexInfo {
        ID: 8,
        Name: NewCIStr("idx_a"),
        Columns: vec![astersql_meta_model::IndexColumn {
            Name: NewCIStr("a"),
            Offset: 0,
            Length: astersql_parser_types::UnspecifiedLength,
            ..Default::default()
        }],
        ..Default::default()
    });
    scan.Columns = columns;
    let context = crate::physical_plan_runtime_test::context();
    let mut limit =
        astersql_planner_core_operator_physicalop::PhysicalLimit::New(context.clone(), 1, 1);
    limit
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(scan)]);
    let mut reader = astersql_planner_core_operator_physicalop::PhysicalIndexReader::New(context);
    reader.IndexPlan = Some(Box::new(limit));
    let start = astersql_tablecodec::EncodeIndexSeekKey(97, 8, None);
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
    .expect("build nested covered index plan");
    executor.Open().unwrap();
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.GetRow(0).GetBytes(0), b"bb");
    let record =
        astersql_tablecodec::EncodeRowKeyWithHandle(97, Box::new(astersql_kv::IntHandle(2)));
    assert_eq!(executor.TakeLockKeys(), vec![record.0]);
    let calls = retriever.IterCalls();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    assert_eq!(retriever.IterCalls(), calls);
}
