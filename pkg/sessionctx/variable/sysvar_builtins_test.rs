// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{Context, GetSysVar, GlobalVarAccessor, SessionVars, VariableError, vardef};

#[test]
fn hash_join_concurrency_accepts_explicit_and_auto_values() {
    crate::register_builtin_sysvars();
    let var = GetSysVar(vardef::TiDBHashJoinConcurrency).unwrap();
    assert_eq!(var.Value, vardef::DefTiDBHashJoinConcurrency.to_string());
    assert!(var.AllowAutoValue);
    let mut vars = crate::session::SessionVars::default();
    for value in ["1", "8", "-1"] {
        vars.SetSystemVar(vardef::TiDBHashJoinConcurrency, value)
            .unwrap();
        assert_eq!(
            vars.GetSystemVar(vardef::TiDBHashJoinConcurrency)
                .as_deref(),
            Some(value)
        );
    }
}

struct NoopAccessor;

impl GlobalVarAccessor for NoopAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
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

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        Err(VariableError::unknown(name))
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

#[test]
fn gogc_tuner_bounds_and_strict_order_match_go() {
    let mut vars = SessionVars::new(Box::new(NoopAccessor));
    let max = GetSysVar(vardef::TiDBGOGCTunerMaxValue).unwrap();
    let min = GetSysVar(vardef::TiDBGOGCTunerMinValue).unwrap();

    assert_eq!(max.MinValue, 10);
    assert_eq!(min.MinValue, 10);
    assert_eq!(max.MaxValue, i32::MAX as u64);
    assert_eq!(min.MaxValue, i32::MAX as u64);

    assert!(
        max.Validate(&mut vars, "100", vardef::ScopeGlobal).is_err(),
        "Go rejects a maximum equal to the current minimum"
    );
    assert!(
        min.Validate(&mut vars, "500", vardef::ScopeGlobal).is_err(),
        "Go rejects a minimum equal to the current maximum"
    );
}

#[test]
fn crossks_align_submit_only_cdc_source_is_a_go_session_variable() {
    crate::register_builtin_sysvars();
    let variable = GetSysVar(vardef::TiDBCDCWriteSource)
        .expect("DDL job construction needs the real CDC source");
    assert_eq!(variable.Scope, vardef::ScopeSession);
    assert_eq!(variable.Value, "0");
    assert_eq!(variable.MaxValue, 15);
    let mut vars = SessionVars::new(Box::new(NoopAccessor));
    let value = variable
        .Validate(&mut vars, "7", vardef::ScopeSession)
        .unwrap();
    variable.SetSessionFromHook(&mut vars, &value).unwrap();
    assert_eq!(vars.system(vardef::TiDBCDCWriteSource), Some("7"));
    assert_eq!(vars.CDCWriteSource, 7);
    let clamped = variable
        .Validate(&mut vars, "16", vardef::ScopeSession)
        .unwrap();
    assert_eq!(clamped, "15");
    assert!(
        variable
            .Validate(&mut vars, "invalid", vardef::ScopeSession)
            .is_err()
    );
}
