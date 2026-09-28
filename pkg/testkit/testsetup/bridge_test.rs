// Copyright 2026 AsterSQL.

use super::parse_log_level;

#[test]
fn warning_alias_matches_zap_level_parsing() {
    assert_eq!(parse_log_level("WaRnInG").unwrap(), log::LevelFilter::Warn);
}
