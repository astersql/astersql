// Copyright 2026 AsterSQL.

use std::time::Duration;

use crate::pd::format_duration;

#[test]
fn duration_string_preserves_subsecond_precision() {
    assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
    assert_eq!(format_duration(Duration::from_micros(1_500)), "1.5ms");
    assert_eq!(format_duration(Duration::from_nanos(1_234)), "1.234µs");
    assert_eq!(format_duration(Duration::from_millis(1_500)), "1.5s");
}
