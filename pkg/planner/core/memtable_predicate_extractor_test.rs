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

#[test]
fn like_extraction_distinguishes_prefilters_and_raw_display_patterns() {
    use super::LikeEscape;
    use super::memtable_predicate_extractor::extract_like_pattern;
    let like = Predicate::LikeWithEscape(
        "table_name".into(),
        "%#_%".into(),
        LikeEscape::Constant(b'#'),
    );
    assert_eq!(
        extract_like_pattern(&like, "table_name", true, true),
        Some(("^.*_.*$".into(), true))
    );
    assert_eq!(
        extract_like_pattern(&like, "table_name", true, false),
        Some(("%#_%".into(), false))
    );
    let ilike = Predicate::Ilike(
        "table_name".into(),
        "%FOO%".into(),
        LikeEscape::Constant(b'\\'),
    );
    assert_eq!(
        extract_like_pattern(&ilike, "table_name", true, true),
        Some(("^.*foo.*$".into(), false))
    );
    let disjunction = Predicate::Or(vec![ilike, like]);
    assert_eq!(
        extract_like_pattern(&disjunction, "table_name", true, true),
        None
    );
    let equality = Predicate::Eq("message".into(), string("a.b+"));
    assert_eq!(
        extract_like_pattern(&equality, "message", false, true),
        Some((r"^a\.b\+$".into(), false))
    );
    let regexp = Predicate::Regexp("message".into(), "^FOO$".into());
    assert_eq!(
        extract_like_pattern(&regexp, "message", false, true),
        Some(("^FOO$".into(), false))
    );
}
