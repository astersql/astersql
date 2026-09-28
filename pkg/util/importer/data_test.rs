// Copyright 2026 AsterSQL.

use super::Datum;
use std::sync::Arc;

#[test]
fn unique_i64_wraps_like_go_at_the_i64_boundary() {
    let datum = Datum::new();
    datum.set_init_int64_value(2, i64::MAX - 1, i64::MAX);

    assert_eq!(datum.unique_i64(), i64::MAX - 1);
    assert_eq!(datum.unique_i64(), i64::MIN);
    assert_eq!(datum.unique_i64(), i64::MIN + 2);
}

#[test]
fn integer_initialization_range_and_float_conversion_match_go() {
    let datum = Datum::new();
    datum.set_init_int64_value(2, 10, 13);
    datum.set_init_int64_value(9, 100, 200);

    assert_eq!(datum.unique_i64(), 10);
    assert_eq!(datum.unique_f64(), 12.0);
    assert_eq!(datum.unique_i64(), 12);
}

#[test]
fn unique_string_uses_the_go_base62_sequence_and_length_cap() {
    let datum = Datum::new();

    assert_eq!(datum.unique_string(8), "0");
    for _ in 0..61 {
        datum.unique_string(8);
    }
    assert_eq!(datum.unique_string(8), "10");
    assert_eq!(datum.unique_string(1), "1");
    assert_eq!(datum.unique_string(0), "");
}

#[test]
fn integer_generation_serializes_shared_state() {
    let datum = Arc::new(Datum::new());
    datum.set_init_int64_value(1, 0, 100);
    let mut handles = Vec::new();
    for _ in 0..16 {
        let datum = Arc::clone(&datum);
        handles.push(std::thread::spawn(move || datum.unique_i64()));
    }
    let mut values: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    values.sort_unstable();
    assert_eq!(values, (0..16).collect::<Vec<_>>());
}

#[test]
fn temporal_values_keep_go_shapes_and_steps() {
    let time = Datum::new();
    time.set_init_int64_value(2, -1, -1);
    let first = seconds_of_day(&time.unique_time());
    let second = seconds_of_day(&time.unique_time());
    assert_eq!((second - first).rem_euclid(86_400), 2);

    let date = Datum::new();
    date.set_init_int64_value(2, -1, -1);
    let first = days(&date.unique_date());
    let second = days(&date.unique_date());
    assert_eq!(second - first, 2);

    let timestamp = Datum::new().unique_timestamp();
    assert_eq!(timestamp.len(), 19);
    assert_eq!(&timestamp[4..5], "-");
    assert_eq!(&timestamp[10..11], " ");

    let year = Datum::new();
    year.set_init_int64_value(2, -1, -1);
    let first: i32 = year.unique_year().parse().unwrap();
    let second: i32 = year.unique_year().parse().unwrap();
    assert_eq!(second, first + 2);
}

fn seconds_of_day(value: &str) -> i64 {
    let parts: Vec<i64> = value.split(':').map(|part| part.parse().unwrap()).collect();
    assert_eq!(parts.len(), 3);
    parts[0] * 3_600 + parts[1] * 60 + parts[2]
}

fn days(value: &str) -> i64 {
    let parts: Vec<i32> = value.split('-').map(|part| part.parse().unwrap()).collect();
    assert_eq!(parts.len(), 3);
    super::rand::days_from_civil(parts[0], parts[1] as u32, parts[2] as u32)
}
