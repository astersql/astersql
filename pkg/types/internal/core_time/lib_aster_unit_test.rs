// Copyright 2026 AsterSQL.

use super::Duration;

#[test]
fn duration_hour_uses_the_unsigned_component_for_negative_values() {
    let duration = Duration {
        Duration: -3_600_000_000,
        Fsp: 0,
    };

    assert_eq!(duration.Hour(), 1);
}
