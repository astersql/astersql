// Copyright 2026 AsterSQL.

use crate::runtime::CreateAnalyzeSession;

#[test]
fn fixed_time_zone_range_matches_go_parse_time_zone() {
    let (_, session) = CreateAnalyzeSession().expect("create canonical session");

    for value in ["-12:59", "+14:00"] {
        session
            .execute(&format!("SET time_zone = '{value}'"))
            .unwrap_or_else(|error| panic!("Go accepts boundary time zone {value}: {error}"));
    }

    for value in ["-13:00", "-14:00", "+14:01", "--01:00", "+-01:00"] {
        let error = match session.execute(&format!("SET time_zone = '{value}'")) {
            Ok(_) => panic!("Go rejects fixed time zone {value}"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("Unknown or incorrect time zone"),
            "unexpected error for {value}: {error}"
        );
    }
}
