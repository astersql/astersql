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

#[test]
fn ddl_reorg_configuration_accepts_session_scope() {
    crate::register_builtin_sysvars();
    let mut vars = crate::session::SessionVars::default();
    for (name, value) in [
        (vardef::TiDBDDLReorgWorkerCount, "3"),
        (vardef::TiDBDDLReorgBatchSize, "128"),
        (vardef::TiDBMaxDistTaskNodes, "2"),
    ] {
        vars.SetSystemVar(name, value).unwrap();
        assert_eq!(vars.GetSystemVar(name).as_deref(), Some(value));
    }
    assert!(
        vars.SetSystemVar(vardef::TiDBMaxDistTaskNodes, "0")
            .is_err()
    );
}

#[test]
fn ddl_service_scope_uses_instance_hooks_and_go_name_validation() {
    crate::register_builtin_sysvars();
    let variable =
        GetSysVar(vardef::TiDBServiceScope).expect("DXF nodes need the real instance scope");
    assert_eq!(variable.Scope, vardef::ScopeInstance);
    let mut vars = SessionVars::new(Box::new(NoopAccessor));
    for name in ["", "BACKGROUND", "ddl_worker-1", &"x".repeat(64)] {
        assert_eq!(
            variable
                .Validate(&mut vars, name, vardef::ScopeGlobal)
                .unwrap(),
            name
        );
    }
    for name in ["bad name", "worker/scope", &"x".repeat(65)] {
        assert!(
            variable
                .Validate(&mut vars, name, vardef::ScopeGlobal)
                .is_err()
        );
    }
    assert!(
        variable
            .Validate(&mut vars, "background", vardef::ScopeSession)
            .is_err()
    );
    struct Restore(config::Config, String);
    impl Drop for Restore {
        fn drop(&mut self) {
            config::store_global_config(self.0.clone());
            vardef::ServiceScope.Store(self.1.clone());
        }
    }
    let _restore = Restore(
        (*config::get_global_config()).clone(),
        vardef::ServiceScope.Load(),
    );
    variable
        .SetGlobalFromHook(&Context, &mut vars, "BACKGROUND", false)
        .unwrap();
    assert_eq!(vardef::ServiceScope.Load(), "background");
    assert_eq!(
        config::get_global_config().instance.tidb_service_scope,
        "background"
    );
    assert_eq!(
        variable.GetGlobal.as_ref().unwrap()(&Context, &mut vars).unwrap(),
        "background"
    );
}

#[test]
fn ddl_write_speed_uses_go_ram_units_and_global_hooks() {
    crate::register_builtin_sysvars();
    struct Restore(i64);
    impl Drop for Restore {
        fn drop(&mut self) {
            vardef::DDLReorgMaxWriteSpeed.Store(self.0);
        }
    }
    let _restore = Restore(vardef::DDLReorgMaxWriteSpeed.Load());
    let variable = GetSysVar(vardef::TiDBDDLReorgMaxWriteSpeed).unwrap();
    let mut vars = SessionVars::new(Box::new(NoopAccessor));
    for (input, expected) in [
        ("0", 0),
        ("1.5MiB", 1572864),
        ("1.5 mb", 1572864),
        ("2k", 2048),
        ("1PiB", 1_i64 << 50),
        ("0.5B", 0),
        ("0x1p4MiB", 16777216),
        ("0X_1.8p+1KiB", 3072),
        ("1_024B", 1024),
    ] {
        variable
            .SetGlobalFromHook(&Context, &mut vars, input, false)
            .unwrap();
        assert_eq!(vardef::DDLReorgMaxWriteSpeed.Load(), expected, "{input}");
        assert_eq!(
            variable.GetGlobal.as_ref().unwrap()(&Context, &mut vars).unwrap(),
            expected.to_string()
        );
    }
    for input in ["-1", "2PiB", "1.5 M iB", "1Ki", "1BB", ""] {
        assert!(
            variable
                .SetGlobalFromHook(&Context, &mut vars, input, false)
                .is_err(),
            "{input}"
        );
    }
}

#[test]
fn ddl_fast_and_dist_reorg_flags_use_go_defaults_and_global_hooks() {
    crate::register_builtin_sysvars();
    struct Restore(bool, bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            vardef::EnableDistTask.Store(self.0);
            vardef::EnableFastReorg.Store(self.1);
        }
    }
    let _restore = Restore(
        vardef::EnableDistTask.Load(),
        vardef::EnableFastReorg.Load(),
    );
    let mut vars = SessionVars::new(Box::new(NoopAccessor));
    for name in [vardef::TiDBEnableDistTask, vardef::TiDBDDLEnableFastReorg] {
        let variable = GetSysVar(name).unwrap();
        assert_eq!(variable.Value, "ON");
        for input in ["OFF", "ON"] {
            let value = variable
                .Validate(&mut vars, input, vardef::ScopeGlobal)
                .unwrap();
            variable
                .SetGlobalFromHook(&Context, &mut vars, &value, false)
                .unwrap();
            let actual = if name == vardef::TiDBEnableDistTask {
                vardef::EnableDistTask.Load()
            } else {
                vardef::EnableFastReorg.Load()
            };
            assert_eq!(actual, input == "ON");
            assert_eq!(
                variable.GetGlobal.as_ref().unwrap()(&Context, &mut vars).unwrap(),
                input
            );
        }
    }
}

#[test]
fn connection_event_log_global_hooks() {
    crate::register_builtin_sysvars();
    let var =
        GetSysVar("tidb_enable_connection_event_log").expect("connection event global variable");
    assert_eq!(var.Value, "OFF");
    assert_eq!(var.Scope, vardef::ScopeGlobal);
    let mut vars = SessionVars::new(Box::new(NoopAccessor));
    let original = var.GetGlobal.as_ref().unwrap()(&Context, &mut vars).unwrap();
    for value in ["ON", "OFF"] {
        var.SetGlobalFromHook(&Context, &mut vars, value, false)
            .unwrap();
        assert_eq!(vardef::EnableConnectionEventLog.Load(), value == "ON");
        assert_eq!(
            var.GetGlobal.as_ref().unwrap()(&Context, &mut vars).unwrap(),
            value
        );
    }
    assert!(
        var.Validate(&mut vars, "invalid", vardef::ScopeGlobal)
            .is_err()
    );
    var.SetGlobalFromHook(&Context, &mut vars, &original, false)
        .unwrap();
}

#[test]
fn paging_byte_budget_defaults_to_disabled() {
    crate::register_builtin_sysvars();
    let variable = GetSysVar(vardef::TiDBPagingSizeBytes)
        .expect("paging DEFAULT must resolve through the production registry");
    assert_eq!(variable.Value, "0");
    assert_eq!(variable.Scope, vardef::ScopeGlobal);
    assert_eq!(variable.Type, vardef::TypeUnsigned);
    let vars = crate::session::SessionVars::default();
    assert_eq!(
        vars.GetHintSystemVar(vardef::TiDBPagingSizeBytes).unwrap(),
        "0"
    );
    for value in ["4194304", "0"] {
        let (normalized, warnings) = vars
            .ValidateAndSetGlobalSystemVar(vardef::TiDBPagingSizeBytes, value, vardef::ScopeGlobal)
            .unwrap();
        assert_eq!(normalized, value);
        assert!(warnings.is_empty());
    }
    let default = crate::sysvar::GlobalSystemVariableInitialValue(&variable.Name, &variable.Value);
    assert_eq!(default, "0");
    assert_eq!(
        vars.ValidateAndSetGlobalSystemVar(
            vardef::TiDBPagingSizeBytes,
            &default,
            vardef::ScopeGlobal,
        )
        .unwrap()
        .0,
        "0"
    );
}
