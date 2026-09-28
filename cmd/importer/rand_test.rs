// Copyright 2026 AsterSQL.

use std::sync::Arc;

use crate::data::newDatum;
use crate::parser::column;
use crate::rand::{randDate, randTime, randTimestamp, randYear};
use crate::stubs::FieldType;

fn bounded_column(min: &str, max: &str) -> column {
    column {
        idx: 0,
        name: "temporal".into(),
        data: Arc::new(newDatum()),
        tp: FieldType::default(),
        comment: String::new(),
        min: min.into(),
        max: max.into(),
        incremental: false,
        set: Vec::new(),
        hist: None,
    }
}

#[test]
fn temporal_ranges_wider_than_i32_seconds_match_go_int_range() {
    let timestamp = bounded_column("1900-01-01 00:00:00", "2000-01-01 00:00:00");
    let generated = randTimestamp(&timestamp);
    assert!(
        ("1900-01-01 00:00:00"..="2000-01-01 00:00:00").contains(&generated.as_str()),
        "generated timestamp {generated} must remain within the inclusive Go range"
    );

    let year = bounded_column("1900", "2000");
    let generated = randYear(&year);
    assert!(
        ("1900"..="2000").contains(&generated.as_str()),
        "generated year {generated} must remain within the inclusive Go range"
    );
}

#[test]
fn invalid_temporal_bounds_fall_back_to_go_zero_time() {
    let invalid = bounded_column("invalid", "invalid");

    assert_eq!(randDate(&invalid), "0001-01-01");
    assert_eq!(randTime(&invalid), "00:00:00");
    assert_eq!(randTimestamp(&invalid), "0001-01-01 00:00:00");
    assert_eq!(randYear(&invalid), "0001");
}
