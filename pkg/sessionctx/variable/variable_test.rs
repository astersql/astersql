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
