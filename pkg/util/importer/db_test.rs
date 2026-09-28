// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn signed_numeric_defaults_match_go_ranges() {
    seed(3511);
    let mut table = Table::new();
    parse_table_sql(
        &mut table,
        "CREATE TABLE t (tiny TINYINT, small SMALLINT, normal INT, big BIGINT, real DOUBLE)",
    )
    .unwrap();

    let cases = [
        ("tiny", i8::MIN as i64, i8::MAX as i64),
        ("small", i16::MIN as i64, i16::MAX as i64),
        ("normal", i32::MIN as i64, i32::MAX as i64),
        ("big", i32::MIN as i64, i32::MAX as i64),
        ("real", i32::MIN as i64, i32::MAX as i64),
    ];

    for (name, minimum, maximum) in cases {
        let column = table.find_column(name).unwrap();
        for _ in 0..256 {
            let value = generate_column_data(&table, &column)
                .unwrap()
                .parse::<f64>()
                .unwrap();
            assert!(
                value >= minimum as f64 && value <= maximum as f64,
                "{name} generated {value} outside Go default range {minimum}..={maximum}"
            );
        }
    }
}
