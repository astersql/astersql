// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_meta_model::ColumnInfo;
use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::{TypeLonglong, TypeVarchar};
use astersql_planner_core_base::PhysicalPlan as _;
use astersql_planner_core_operator_physicalop::{PhysicalProjection, PhysicalTableScan};

use crate::physical_plan_runtime_test::{MemoryRetriever, context, encode_row};
use crate::typed_kv_scan::KeyRange;

#[test]
fn canonical_typed_projection_reorders_typed_columns_and_preserves_row_lock_keys() {
    let retriever = Arc::new(MemoryRetriever::default());
    for (handle, a, b) in [(1, 10, "one"), (2, 20, "two")] {
        let (key, value) = encode_row(92, handle, a, b);
        retriever.Put(key, value);
    }
    let ctx = context();
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 92,
        Name: NewCIStr("t"),
        ..Default::default()
    });
    scan.Columns = vec![
        ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
            ..Default::default()
        },
        ColumnInfo {
            ID: 2,
            Name: NewCIStr("b"),
            FieldType: astersql_parser_types::NewFieldType(TypeVarchar),
            ..Default::default()
        },
    ];
    let mut projection = PhysicalProjection::New(ctx);
    projection.Exprs = vec![
        Box::new(astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(TypeVarchar),
            2,
            2,
            1,
        )),
        Box::new(astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(TypeLonglong),
            1,
            1,
            0,
        )),
    ];
    projection.set_children(vec![Box::new(scan)]);
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(92).0);
    let end = start.PrefixNext();
    let mut executor = crate::builder::BuildTypedPhysicalPlan(
        &projection,
        retriever,
        vec![KeyRange { start, end }],
        1,
        1,
    )
    .expect("build physical projection over scan");
    executor.Open().expect("open lazily");
    let mut output = executor.NewChunk();
    for (handle, a, b) in [(1, 10, "one"), (2, 20, "two")] {
        executor.Next(&mut output).expect("next projected page");
        assert_eq!(output.NumRows(), 1);
        assert_eq!(output.GetRow(0).GetBytes(0), b.as_bytes());
        assert_eq!(output.GetRow(0).GetInt64(1), a);
        let (key, _) = encode_row(92, handle, a, b);
        assert_eq!(executor.TakeLockKeys(), vec![key.0]);
    }
    assert!(executor.Detach().is_none());
    executor.Close().expect("close projection child");
}
