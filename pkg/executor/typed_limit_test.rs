// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_meta_model::ColumnInfo;
use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::TypeLonglong;

use crate::adapter::ExecExecutor;
use crate::physical_plan_runtime_test::{MemoryRetriever, context, encode_row};
use crate::typed_kv_scan::{KeyRange, TypedKVScan};
use crate::typed_limit::TypedLimit;

#[test]
fn typed_limit_streams_offset_count_and_forwards_only_returned_record_lock_keys() {
    let retriever = Arc::new(MemoryRetriever::default());
    for handle in 1..=4 {
        let (key, value) = encode_row(88, handle, handle * 10, "row");
        retriever.Put(key, value);
    }
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(88).0);
    let end = start.PrefixNext();
    let child = TypedKVScan::new(
        retriever.clone(),
        88,
        false,
        false,
        vec![ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
            ..Default::default()
        }],
        vec![KeyRange { start, end }],
        1,
        1,
    );
    let mut limit = TypedLimit::new(Box::new(child), 1, 2);
    limit.Open().expect("open child lazily");
    assert_eq!(retriever.IterCalls(), 0);
    let mut output = limit.NewChunk();
    limit.Next(&mut output).expect("skip first, return second");
    assert_eq!(output.GetRow(0).GetInt64(0), 20);
    let (second_key, _) = encode_row(88, 2, 20, "row");
    assert_eq!(limit.TakeLockKeys(), vec![second_key.0]);
    let mut detached = limit.Detach().expect("owned child is detachable");
    limit.Next(&mut output).expect("return third");
    assert_eq!(output.GetRow(0).GetInt64(0), 30);
    let (third_key, _) = encode_row(88, 3, 30, "row");
    assert_eq!(limit.TakeLockKeys(), vec![third_key.0]);
    let calls = retriever.IterCalls();
    limit.Next(&mut output).expect("LIMIT exhausted");
    assert_eq!(output.NumRows(), 0);
    assert_eq!(retriever.IterCalls(), calls, "do not scan fourth row");
    limit.Close().expect("close original child");
    drop(limit);
    drop(retriever);
    let mut independent = detached.NewChunk();
    detached
        .Next(&mut independent)
        .expect("detached LIMIT continues independently");
    assert_eq!(independent.GetRow(0).GetInt64(0), 30);
    detached.Close().expect("close detached child");
}

#[test]
fn typed_limit_zero_count_never_reads_its_child() {
    let retriever = Arc::new(MemoryRetriever::default());
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(89).0);
    let end = start.PrefixNext();
    let child = TypedKVScan::new(
        retriever.clone(),
        89,
        false,
        false,
        Vec::new(),
        vec![KeyRange { start, end }],
        1,
        1,
    );
    let mut limit = TypedLimit::new(Box::new(child), 10, 0);
    limit.Open().expect("open without fetch");
    let mut output = limit.NewChunk();
    limit.Next(&mut output).expect("return empty result");
    assert_eq!(output.NumRows(), 0);
    assert_eq!(retriever.IterCalls(), 0);
    limit.Close().expect("close child");
}

#[test]
fn canonical_physical_limit_tree_builds_a_lazy_typed_pipeline() {
    let retriever = Arc::new(MemoryRetriever::default());
    for handle in 1..=3 {
        let (key, value) = encode_row(90, handle, handle * 10, "row");
        retriever.Put(key, value);
    }
    let context = context();
    let mut scan =
        astersql_planner_core_operator_physicalop::PhysicalTableScan::New(context.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 90,
        Name: NewCIStr("t"),
        ..Default::default()
    });
    scan.Columns = vec![ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..Default::default()
    }];
    let mut plan = astersql_planner_core_operator_physicalop::PhysicalLimit::New(context, 1, 1);
    plan.PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildren(vec![Box::new(scan)]);
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(90).0);
    let end = start.PrefixNext();
    let mut executor = crate::builder::BuildTypedPhysicalPlan(
        &plan,
        retriever.clone(),
        vec![KeyRange { start, end }],
        1,
        1,
    )
    .expect("build canonical scan+LIMIT tree");
    executor.Open().expect("lazy Open");
    assert_eq!(retriever.IterCalls(), 0);
    let mut output = executor.NewChunk();
    executor
        .Next(&mut output)
        .expect("skip first, return second");
    assert_eq!(output.GetRow(0).GetInt64(0), 20);
    executor.Close().expect("close pipeline");
}
