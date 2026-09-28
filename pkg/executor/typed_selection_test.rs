// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_meta_model::ColumnInfo;
use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::TypeLonglong;
use astersql_planner_core_base::{PhysicalPlan as _, PlanContext};
use astersql_planner_core_operator_physicalop::{PhysicalSelection, PhysicalTableScan};

use crate::physical_plan_runtime_test::{MemoryRetriever, context, encode_row};
use crate::typed_kv_scan::KeyRange;

#[test]
fn canonical_typed_selection_filters_rows_and_excludes_discarded_record_locks() {
    let retriever = Arc::new(MemoryRetriever::default());
    for handle in 1..=3 {
        let (key, value) = encode_row(91, handle, handle * 10, "row");
        retriever.Put(key, value);
    }
    let ctx = context();
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 91,
        Name: NewCIStr("t"),
        ..Default::default()
    });
    scan.Columns = vec![ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..Default::default()
    }];
    let value_column = astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(TypeLonglong),
        1,
        1,
        0,
    );
    let bound = astersql_expression::NewInt64Const(20);
    let condition = astersql_expression::NewFunction(
        ctx.GetExprCtx(),
        astersql_parser_ast::GT,
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeTiny),
        vec![Box::new(value_column), Box::new(bound)],
    )
    .expect("build canonical > predicate");
    let mut selection = PhysicalSelection::New(ctx);
    selection.Conditions = vec![condition];
    selection.set_children(vec![Box::new(scan)]);
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(91).0);
    let end = start.PrefixNext();
    let mut executor = crate::builder::BuildTypedPhysicalPlan(
        &selection,
        retriever.clone(),
        vec![KeyRange { start, end }],
        1,
        1,
    )
    .expect("build canonical Selection+TableScan");
    executor.Open().expect("open lazily");
    assert_eq!(retriever.IterCalls(), 0);
    let mut output = executor.NewChunk();
    executor.Next(&mut output).expect("filter first two rows");
    assert_eq!(output.NumRows(), 1);
    assert_eq!(output.GetRow(0).GetInt64(0), 30);
    let (key, _) = encode_row(91, 3, 30, "row");
    assert_eq!(executor.TakeLockKeys(), vec![key.0]);
    assert!(
        executor.Detach().is_none(),
        "session-bound expression context cannot be independently detached"
    );
    executor.Next(&mut output).expect("reach EOF");
    assert_eq!(output.NumRows(), 0);
    executor.Close().expect("close selection child");
}
