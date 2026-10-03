// Copyright 2026 AsterSQL.

use crate::vardef;
use crate::variable::{Context, GlobalVarAccessor, SessionVars, SysVar, VariableError};

struct NoopAccessor;

impl GlobalVarAccessor for NoopAccessor {
    fn get_global_sys_var(&self, _name: &str) -> Result<String, VariableError> {
        Ok(String::new())
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &Context,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(&self, _name: &str) -> Result<String, VariableError> {
        Ok(String::new())
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        Ok(())
    }
}

fn session_vars() -> SessionVars {
    SessionVars::new(Box::new(NoopAccessor))
}

#[test]
fn relaxed_time_validation_returns_type_validator_fallback() {
    let sys_var = SysVar {
        Scope: vardef::ScopeSession,
        Name: "mynewsysvar".to_owned(),
        Type: vardef::TypeTime,
        ..SysVar::default()
    };
    let mut vars = session_vars();

    assert_eq!(
        sys_var.ValidateWithRelaxedValidation(&mut vars, "invalid", vardef::ScopeSession),
        ""
    );
}

#[test]
fn duration_validation_matches_go_string_precision() {
    let sys_var = SysVar {
        Scope: vardef::ScopeSession,
        Name: "mynewsysvar".to_owned(),
        Type: vardef::TypeDuration,
        MinValue: i64::MIN,
        MaxValue: i64::MAX as u64,
        ..SysVar::default()
    };
    let mut vars = session_vars();

    assert_eq!(
        sys_var
            .Validate(&mut vars, "0", vardef::ScopeSession)
            .unwrap(),
        "0s"
    );
    assert_eq!(
        sys_var
            .Validate(&mut vars, "1500ns", vardef::ScopeSession)
            .unwrap(),
        "1.5µs"
    );
    assert_eq!(
        sys_var
            .Validate(&mut vars, "1.23456789s", vardef::ScopeSession)
            .unwrap(),
        "1.23456789s"
    );
}

#[test]
fn plan_replayer_file_retention_global_hooks_match_go() {
    use std::time::Duration;
    let original = vardef::GetPlanReplayerFileRetentionTime();
    struct Restore(Duration);
    impl Drop for Restore {
        fn drop(&mut self) {
            vardef::SetPlanReplayerFileRetentionTime(self.0);
        }
    }
    let _restore = Restore(original);
    let mut vars = session_vars();
    let name = "tidb_plan_replayer_file_retention_time";
    let sys_var = crate::GetSysVar(name).expect("plan replayer retention must be registered");
    assert_eq!(sys_var.Value, "168h0m0s");
    assert_eq!(sys_var.Scope, vardef::ScopeGlobal);
    assert_eq!(sys_var.Type, vardef::TypeDuration);
    assert_eq!(
        crate::set_global_system_var(&mut vars, name, "2h").unwrap(),
        "2h0m0s"
    );
    assert_eq!(
        vardef::GetPlanReplayerFileRetentionTime(),
        Duration::from_secs(7200)
    );
    assert_eq!(
        sys_var.GetGlobalFromHook(&Context, &mut vars).unwrap(),
        "2h0m0s"
    );
    assert!(crate::set_global_system_var(&mut vars, name, "2hours").is_err());
    assert!(
        sys_var
            .SetGlobalFromHook(&Context, &mut vars, "2hours", false)
            .is_err()
    );
    assert_eq!(
        vardef::GetPlanReplayerFileRetentionTime(),
        Duration::from_secs(7200)
    );
    assert!(
        sys_var
            .Validate(&mut vars, "2h", vardef::ScopeSession)
            .is_err()
    );
    assert_eq!(
        crate::set_global_system_var(&mut vars, name, "-1s").unwrap(),
        "0s"
    );
    assert_eq!(vardef::GetPlanReplayerFileRetentionTime(), Duration::ZERO);
    assert_eq!(
        crate::set_global_system_var(&mut vars, name, "8761h").unwrap(),
        "8760h0m0s"
    );
    assert_eq!(
        vardef::GetPlanReplayerFileRetentionTime(),
        Duration::from_secs(365 * 24 * 3600)
    );
}
