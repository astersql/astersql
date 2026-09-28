// Copyright 2026 AsterSQL.

use astersql_util_timeutil::errors::{ErrUnknownTimeZone, TimeUtilError};

#[test]
fn unknown_time_zone_matches_mysql_argument_precision() {
    let name = "x".repeat(65);
    let error = ErrUnknownTimeZone.GenWithStackByArgs(&name);

    assert!(ErrUnknownTimeZone.Equal(&error));
    assert_eq!(
        error.to_string(),
        format!("Unknown or incorrect time zone: '{}'", "x".repeat(64))
    );
    assert!(matches!(error, TimeUtilError::UnknownTimeZone { name: value } if value == name));
}
