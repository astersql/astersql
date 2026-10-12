// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 系统变量（vars）集成测试：KV 下发、已移除变量、TiKV GC 映射、升级规整与时区等。
//
// 可运行用例校验 SysVar 注册表大小写不敏感、依赖顺序、已移除变量，
// 以及 TiKV/advancer 与 index-join/timestamp 校验钩子。

use std::collections::HashMap;
use std::sync::Mutex;

use astersql_sessionctx_vardef::{
    AdvancerCheckPointLagLimit, ScopeGlobal, ScopeSession, TiDBAdvancerCheckPointLagLimit,
    TiDBBackOffWeight, TiDBBackoffLockFast, TiDBEnableTSValidation, TiDBGCConcurrency,
    TiDBGCEnable, TiDBOptIndexJoinBuild,
};
use astersql_sessionctx_variable::sysvar_builtins::TS_VALIDATION_ENABLED;
use astersql_sessionctx_variable::{
    Context, GetSysVar, GlobalVarAccessor, OrderByDependency, RegisterSysVar, SessionVars, SysVar,
    UnregisterSysVar, VariableError, register_builtin_sysvars, set_global_system_var,
};

/// 测试中使用的进程内全局变量表。它仅替代 mysql.global_variables 这一 I/O
/// 边界；系统变量的 Validate/SetGlobal hook 均为生产实现。
#[derive(Default)]
struct MemoryGlobal {
    values: Mutex<HashMap<String, String>>,
}

impl GlobalVarAccessor for MemoryGlobal {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        self.values
            .lock()
            .expect("global variable map poisoned")
            .get(&name.to_ascii_lowercase())
            .cloned()
            .ok_or_else(|| VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _context: &Context,
        name: &str,
        value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        self.values
            .lock()
            .expect("global variable map poisoned")
            .insert(name.to_ascii_lowercase(), value.to_owned());
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        self.get_global_sys_var(name)
    }

    fn set_tidb_table_value(
        &mut self,
        name: &str,
        value: &str,
        _comment: &str,
    ) -> Result<(), VariableError> {
        self.set_global_sys_var_only(&Context, name, value, true)
    }
}

fn session_vars() -> SessionVars {
    register_builtin_sysvars();
    SessionVars::new(Box::<MemoryGlobal>::default())
}

/// Go `TestKVVars` requires both session variables to be registered and their
/// setters to update the values subsequently copied into a KV transaction.
#[test]
fn kv_backoff_sysvars_are_registered_for_transaction_propagation() {
    let mut vars = session_vars();
    assert_eq!(vars.KVVars.BackoffLockFast, astersql_kv::DefBackoffLockFast);
    assert_eq!(vars.KVVars.BackOffWeight, astersql_kv::DefBackOffWeight);

    let lock_fast = GetSysVar(TiDBBackoffLockFast).expect("lock-fast sysvar registered");
    assert_eq!(lock_fast.Value, astersql_kv::DefBackoffLockFast.to_string());
    assert_eq!(lock_fast.MinValue, 1);
    assert_eq!(lock_fast.MaxValue, i32::MAX as u64);
    vars.SetSystemVar(TiDBBackoffLockFast, "1").unwrap();

    let weight = GetSysVar(TiDBBackOffWeight).expect("backoff-weight sysvar registered");
    assert_eq!(weight.Value, astersql_kv::DefBackOffWeight.to_string());
    assert_eq!(weight.MinValue, 0);
    assert_eq!(weight.MaxValue, i32::MAX as u64);
    vars.SetSystemVar(TiDBBackOffWeight, "100").unwrap();
    assert_eq!(vars.KVVars.BackoffLockFast, 1);
    assert_eq!(vars.KVVars.BackOffWeight, 100);

    vars.SetSystemVar(TiDBBackOffWeight, "50").unwrap();
    assert_eq!(vars.KVVars.BackOffWeight, 50);
    vars.SetSystemVar(TiDBBackOffWeight, "0").unwrap();
    assert_eq!(vars.KVVars.BackOffWeight, astersql_kv::DefBackOffWeight);
}

#[test]
/// 注册带 Depended 标记的变量后，按名大小写不敏感可查，且依赖排序置前。
fn sysvar_registry_is_case_insensitive_and_orders_dependencies_first() {
    let name = "aster_dependency";
    let mut variable = SysVar::default();
    variable.Name = name.into();
    variable.Value = "on".into();
    variable.Depended = true;
    RegisterSysVar(variable);
    assert_eq!(GetSysVar(&name.to_uppercase()).unwrap().Value, "on");
    let names = HashMap::from([(name.into(), "on".into()), ("plain".into(), "off".into())]);
    assert_eq!(OrderByDependency(&names).first().unwrap(), name);
    UnregisterSysVar(name);
}

/// Go `TestRemovedSysVars` 的注册表边界：变量注销前按全局/会话范围存在，
/// 注销后所有访问都必须走未知变量错误。SQL 执行器对 SHOW/SELECT 的分派由
/// session runtime crate 覆盖；本 crate 直接验证它依赖的真实变量生命周期。
#[test]
fn removed_sysvar_is_visible_then_rejected_from_registry() {
    let name = "aster_removed_sysvar";
    let mut variable = SysVar::default();
    variable.Scope = ScopeGlobal | ScopeSession;
    variable.Name = name.into();
    variable.Value = "acdc".into();
    RegisterSysVar(variable);
    let registered = GetSysVar(&name.to_uppercase()).expect("registered variable is visible");
    assert_eq!(registered.Value, "acdc");
    assert!(registered.HasGlobalScope());
    assert!(registered.HasSessionScope());

    UnregisterSysVar(name);
    assert!(GetSysVar(name).is_none());
    assert_eq!(
        VariableError::unknown(name).to_string(),
        format!("Unknown system variable '{name}'")
    );
}

/// 覆盖 Go `TestTiKVSystemVars` 的变量规范化与 `TestTiDBAdvancerCheckPointLagLimit`
/// 的全局写入：布尔、并发和 checkpoint 时长必须走同一个生产设置通道。
#[test]
fn tikv_and_advancer_global_variables_use_production_validation_hooks() {
    let mut vars = session_vars();
    let gc_enable = GetSysVar(TiDBGCEnable).expect("tidb_gc_enable registered");
    assert_eq!(
        gc_enable.Validate(&mut vars, "1", ScopeGlobal).unwrap(),
        "ON"
    );

    let gc_concurrency = GetSysVar(TiDBGCConcurrency).expect("tidb_gc_concurrency registered");
    assert_eq!(
        gc_concurrency
            .Validate(&mut vars, "-1", ScopeGlobal)
            .unwrap(),
        "-1"
    );
    assert_eq!(
        gc_concurrency
            .Validate(&mut vars, "5", ScopeGlobal)
            .unwrap(),
        "5"
    );

    let original_lag_limit = AdvancerCheckPointLagLimit.Load();
    assert_eq!(
        set_global_system_var(&mut vars, TiDBAdvancerCheckPointLagLimit, "100h").unwrap(),
        "100h0m0s"
    );
    assert_eq!(AdvancerCheckPointLagLimit.Load(), 100 * 3_600_000_000_000);
    assert_eq!(
        GetSysVar(TiDBAdvancerCheckPointLagLimit)
            .expect("advancer checkpoint variable registered")
            .GetGlobalFromHook(&Context, &mut vars)
            .unwrap(),
        "100h0m0s"
    );
    AdvancerCheckPointLagLimit.Store(original_lag_limit);
}

/// Go `TestIndexJoinBuildV2SysVarCompatibility` 与 `TestTiDBValidateTS` 都要求
/// 默认开启的布尔变量拒绝非法关闭值/接受合法关闭值。
#[test]
fn index_join_compatibility_and_timestamp_validation_keep_go_boolean_rules() {
    let mut vars = session_vars();
    let index_join = GetSysVar(TiDBOptIndexJoinBuild).expect("index join variable registered");
    assert_eq!(index_join.Value, "ON");
    let error = index_join
        .Validate(&mut vars, "OFF", ScopeSession)
        .expect_err("index join build v2 must not be disabled");
    assert!(
        error
            .to_string()
            .contains("is now always enabled and cannot be turned off")
    );

    let ts_validation = GetSysVar(TiDBEnableTSValidation).expect("TS validation registered");
    assert_eq!(ts_validation.Value, "ON");
    assert_eq!(
        ts_validation
            .Validate(&mut vars, "off", ScopeGlobal)
            .unwrap(),
        "OFF"
    );
    assert_eq!(
        ts_validation
            .Validate(&mut vars, "on", ScopeGlobal)
            .unwrap(),
        "ON"
    );
    set_global_system_var(&mut vars, TiDBEnableTSValidation, "off").unwrap();
    assert!(!TS_VALIDATION_ENABLED.load(std::sync::atomic::Ordering::SeqCst));
    set_global_system_var(&mut vars, TiDBEnableTSValidation, "on").unwrap();
    assert!(TS_VALIDATION_ENABLED.load(std::sync::atomic::Ordering::SeqCst));
}
