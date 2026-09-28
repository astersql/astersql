// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn telemetry_global_variable_parsing_matches_go_tidb_opt_on() {
    for value in ["ON", "on", "On", "1"] {
        let mut ctx = SessionContext::default();
        ctx.GlobalVars
            .insert("tidb_enable_telemetry".into(), value.into());
        assert_eq!(getTelemetryGlobalVariable(&ctx), Ok(true), "value={value}");
    }

    for value in ["OFF", "off", "0", "true", "TRUE", "yes", "", "  ON  "] {
        let mut ctx = SessionContext::default();
        ctx.GlobalVars
            .insert("tidb_enable_telemetry".into(), value.into());
        assert_eq!(getTelemetryGlobalVariable(&ctx), Ok(false), "value={value}");
    }

    assert_eq!(
        getTelemetryGlobalVariable(&SessionContext::default()),
        Err(TelemetryError("tidb_enable_telemetry not found".into()))
    );
}
