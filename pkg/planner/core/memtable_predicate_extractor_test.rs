// Copyright 2026 AsterSQL.

use super::{MetricSummaryTableExtractor, MetricTableExtractor, Predicate, PredicateValue};

fn string(value: &str) -> PredicateValue {
    PredicateValue::String(value.to_owned())
}

#[test]
fn metric_extractor_excludes_value_from_labels() {
    let predicates = vec![Predicate::Eq("value".into(), string("42"))];
    let mut extractor = MetricTableExtractor::default();

    let remaining = extractor.ExtractPredicates(&predicates);

    assert_eq!(remaining, predicates);
    assert!(extractor.LabelConditions.is_empty());
}

#[test]
fn metric_extractor_does_not_reject_numeric_quantiles_by_range() {
    let mut extractor = MetricTableExtractor::default();

    let remaining =
        extractor.ExtractPredicates(&[Predicate::Eq("quantile".into(), PredicateValue::F64(1.5))]);

    assert!(remaining.is_empty());
    assert!(!extractor.SkipRequest);
    assert_eq!(extractor.Quantiles.into_iter().collect::<Vec<_>>(), ["1.5"]);
}

#[test]
fn metric_summary_keeps_quantile_predicate_like_go() {
    let quantile = Predicate::Eq("quantile".into(), PredicateValue::F64(0.99));
    let metric = Predicate::Eq("metrics_name".into(), string("TiKV_cpu"));
    let mut extractor = MetricSummaryTableExtractor::default();

    let remaining = extractor.ExtractPredicates(&[quantile.clone(), metric]);

    assert_eq!(remaining, vec![quantile]);
    assert_eq!(
        extractor.MetricsNames.into_iter().collect::<Vec<_>>(),
        ["tikv_cpu"]
    );
    assert_eq!(
        extractor.Quantiles.into_iter().collect::<Vec<_>>(),
        ["0.99"]
    );
}
