// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn ordered_union_removes_identity_output_projection() {
    let plan = crate::main_test::optimize_query_for_test(
        "select * from ((SELECT 1 a,6 b) UNION (SELECT 2,5) UNION (SELECT 2,4) ORDER BY 1) t order by 1,2",
    )
    .expect("optimize ordered derived UNION");
    assert_eq!(plan.tp(&[]), "Sort", "{}", plan.explain_info());
    assert_eq!(plan.children()[0].tp(&[]), "Sort");
    let output = plan
        .schema()
        .Columns
        .iter()
        .map(|column| column.UniqueID)
        .collect::<Vec<_>>();
    let child_output = plan.children()[0]
        .schema()
        .Columns
        .iter()
        .map(|column| column.UniqueID)
        .collect::<Vec<_>>();
    assert_eq!(output, child_output);
    assert_eq!(output.len(), 2);
}

fn test_column(unique_id: i64) -> expression::Column {
    expression::Column::new(
        expression::types::FieldType::default(),
        unique_id,
        unique_id,
        0,
    )
}

#[test]
fn index_outer_projection_pruning_preserves_outer_hash_keys() {
    let nation_name = test_column(1);
    let supplier_key = test_column(2);
    let customer_key = test_column(3);
    let mut required = vec![nation_name.Clone(), customer_key.Clone()];
    let outer_hash_keys = vec![supplier_key.Clone()];

    extend_required_columns(&mut required, &outer_hash_keys);

    assert!(
        required
            .iter()
            .any(|column| column.EqualColumn(&supplier_key))
    );
}

#[test]
fn index_outer_hash_key_comes_from_cross_child_equal_condition() {
    let supplier_key = test_column(10);
    let lineitem_key = test_column(11);
    let outer_schema = expression::NewSchema(vec![supplier_key.Clone()]);
    let inner_schema = expression::NewSchema(vec![lineitem_key.Clone()]);

    let outer_hash_key =
        outer_hash_key_for_equality(&supplier_key, &lineitem_key, &outer_schema, &inner_schema)
            .expect("one equality side belongs to each IndexJoin child");

    assert!(outer_hash_key.EqualColumn(&supplier_key));
}

#[test]
fn pg09_10_union_avg_preserves_and_rebinds_filter() {
    use logicalop::LogicalPlan as _;
    let (ctx, left) = crate::main_test::build_logical_for_test("SELECT 1 AS x, 0 AS keep").unwrap();
    let (_, mut right) =
        crate::main_test::build_logical_for_test("SELECT 2 AS x, 1 AS keep").unwrap();
    let mut right_schema = right.Schema().Clone();
    for column in &mut right_schema.Columns {
        column.UniqueID += 1000;
    }
    right.SetSchema(right_schema);
    let mut union = logicalop::LogicalUnionAll::default().Init(ctx.clone(), 0);
    union.SetSchema(left.Schema().Clone());
    union.SetChildren(vec![left, right]);
    let mut avg = aggregation::NewAggFuncDesc(
        ctx.GetExprCtx(),
        crate::ast::AggFuncAvg,
        vec![Box::new(union.Schema().Columns[0].Clone())],
        false,
    )
    .unwrap();
    avg.Filter = Some(Box::new(union.Schema().Columns[1].Clone()));
    let ret = avg.RetTp.clone().unwrap();
    let mut aggregate = logicalop::LogicalAggregation {
        AggFuncs: vec![avg],
        ..Default::default()
    }
    .Init(ctx, 0);
    aggregate.SetSchema(expression::NewSchema(vec![expression::Column::new(
        ret, 0, 100, 0,
    )]));
    aggregate.SetChildren(vec![Box::new(union)]);
    let mut plan: logicalop::LogicalPlanRef = Box::new(aggregate);
    assert!(push_aggregation_across_union(&mut plan).unwrap());
    let aggregate = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
        .unwrap();
    assert!(aggregate.AggFuncs[0].Filter.is_none());
    let union = aggregate.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalUnionAll>()
        .unwrap();
    for branch in union.Children() {
        let partial = branch
            .as_any()
            .downcast_ref::<logicalop::LogicalAggregation>()
            .unwrap();
        assert_eq!(partial.AggFuncs.len(), 2);
        let expected = &partial.Children()[0].Schema().Columns[1];
        for function in &partial.AggFuncs {
            let filter = function
                .Filter
                .as_ref()
                .expect("both COUNT and SUM retain FILTER");
            assert!(filter.as_column().unwrap().EqualColumn(expected));
        }
    }
}
