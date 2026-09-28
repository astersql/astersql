// Copyright 2026 AsterSQL.

use crate::spark_rebase::{
    AppVersion, SparkFileMeta, floor_div_i64, floor_mod_i64,
    rebase_spark_julian_to_gregorian_micros, spark_rebase_time_zone_id,
};
use std::collections::BTreeMap;

#[test]
fn modern_timestamp_bypasses_legacy_timezone_lookup_like_go() {
    let modern_micros = 0;

    assert_eq!(
        rebase_spark_julian_to_gregorian_micros("not/a-zone", modern_micros).unwrap(),
        modern_micros
    );
}

#[test]
fn legacy_timestamp_still_requires_a_generated_timezone() {
    let legacy_micros = -2_208_988_800_000_001;

    assert!(rebase_spark_julian_to_gregorian_micros("not/a-zone", legacy_micros).is_err());
}

#[test]
fn invalid_explicit_spark_version_does_not_fall_back_to_created_by() {
    let meta = SparkFileMeta {
        created_by: "spark version 2.4.8".into(),
        key_values: BTreeMap::from([("org.apache.spark.version".into(), String::new())]),
    };
    let cutoff = AppVersion::parse_spark("3.0.0").unwrap();

    assert_eq!(
        spark_rebase_time_zone_id(&meta, &cutoff, "legacy", "UTC"),
        ""
    );
}

#[test]
fn floor_division_and_modulus_support_negative_divisors_like_go() {
    assert_eq!(floor_div_i64(1, -2), -1);
    assert_eq!(floor_mod_i64(1, -2), -1);
}
