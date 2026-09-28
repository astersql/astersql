// Copyright 2026 AsterSQL.

use crate::*;
use std::collections::HashSet;
use std::sync::Arc;

fn string_column(unique_id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeVarchar),
        unique_id,
        unique_id,
        0,
    )
}

fn string_constant(value: &str) -> Expression {
    Box::new(expression::Constant::with_type(
        expression::types::NewStringDatum(value.to_owned()),
        *expression::types::NewFieldType(expression::mysql::TypeVarchar),
    ))
}

fn equality(ctx: &dyn expression::BuildContext, column: &Column, value: &str) -> Expression {
    expression::NewFunctionInternal(
        ctx,
        expression::ast::EQ,
        *expression::types::NewFieldType(expression::mysql::TypeTiny),
        vec![Box::new(column.clone()), string_constant(value)],
    )
    .expect("equality expression")
}

#[test]
fn contradictory_stats_meta_filters_preserve_original_predicates() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let column = string_column(42);
    let schema = expression::NewSchema(vec![column.clone()]);
    let names = NameSlice(vec![Some(Arc::new(FieldName {
        ColName: parser_ast::NewCIStr("db_name"),
        ..Default::default()
    }))]);
    let predicates = vec![
        equality(&ctx, &column, "Test"),
        equality(&ctx, &column, "Other"),
    ];

    let (remaining, values) =
        extractStatsMetaFilters(None, &schema, &names, predicates, "db_name", true);

    assert_eq!(remaining.len(), 2);
    assert!(values.is_empty());
}

#[test]
fn parameter_marker_uses_current_evaluation_context_value() {
    let eval_ctx = exprstatic::NewEvalContext(vec![exprstatic::WithParamList(vec![
        expression::types::NewStringDatum("current".to_owned()),
    ])]);
    let mut parameter = expression::Constant::with_type(
        expression::types::NewStringDatum("stale".to_owned()),
        *expression::types::NewFieldType(expression::mysql::TypeVarchar),
    );
    parameter.ParamMarker = Some(expression::ParamMarker::new(0));
    let parameter: Expression = Box::new(parameter);

    assert_eq!(
        getStringValueFromConstant(Some(&eval_ctx), &parameter),
        Some("current".to_owned())
    );
}

#[test]
fn stats_meta_extractor_interface_matches_go_stub_contract() {
    let extractor = ShowStatsMetaPredicateExtractor {
        DB: HashSet::from(["test".to_owned()]),
        Table: HashSet::from(["t".to_owned()]),
    };

    assert!(!extractor.Extract());
    assert_eq!(extractor.ExplainInfo(), "");
    assert_eq!(extractor.Field(), "");
    assert_eq!(extractor.FieldPatternLike(), None);
    assert_eq!(
        extractor.StatsMetaDBFilters(),
        &HashSet::from(["test".to_owned()])
    );
    assert_eq!(
        extractor.StatsMetaTableFilters(),
        &HashSet::from(["t".to_owned()])
    );
}

#[test]
fn show_contents_memory_usage_counts_lengths_not_reserved_capacity() {
    let mut db_name = String::with_capacity(128);
    db_name.push_str("db");
    let contents = ShowContents {
        DBName: db_name,
        Partition: parser_ast::NewCIStr("partition"),
        IndexName: parser_ast::NewCIStr("index"),
        ..Default::default()
    };

    let expected = EMPTY_SHOW_CONTENTS_SIZE
        + "db".len() as i64
        + ("partition".len() * 2) as i64
        + ("index".len() * 2) as i64;
    assert_eq!(contents.MemoryUsage(), expected);
}
