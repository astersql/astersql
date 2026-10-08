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

// 内置系统变量注册与校验行为的集成式单元测试。
//
// 使用内存全局 accessor 覆盖 SQL 模式、时区、隔离级别、TiFlash、
// 内存配额、DDL 并发等系统变量的 Validate / SetSession / 全局写入路径。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_sessionctx_variable::*;
use chrono::FixedOffset;
use serial_test::serial;

#[test]
fn shared_lock_upgrade_defaults_off_and_is_gated_to_next_gen() {
    let (mut vars, _) = session();
    let variable = sysvar(vardef::TiDBEnableSharedLockUpgrade);

    assert!(!vars.EnableSharedLockUpgrade);
    assert_eq!(variable.Value, vardef::Off);
    let result = variable.Validate(&mut vars, vardef::On, vardef::ScopeSession);
    if kerneltype::IsNextGen() {
        let normalized = result.expect("next-gen accepts shared lock upgrades");
        variable
            .SetSessionFromHook(&mut vars, &normalized)
            .expect("set shared lock upgrade");
        assert!(vars.EnableSharedLockUpgrade);
    } else {
        let error = result.expect_err("classic rejects shared lock upgrades");
        assert!(
            error
                .to_string()
                .contains("tidb_enable_shared_lock_upgrade")
        );
        assert!(!vars.EnableSharedLockUpgrade);
    }
}

#[test]
#[serial]
fn go_merge_47_analyze_defaults_follow_global_sysvars() {
    let (mut vars, _) = session();
    let ctx = Context::default();
    for (name, global, value, below_min, min, max) in [
        (
            vardef::TiDBAnalyzeDefaultNumBuckets,
            &vardef::AnalyzeDefaultNumBuckets,
            "100",
            "0",
            "1",
            "100000",
        ),
        (
            vardef::TiDBAnalyzeDefaultNumTopN,
            &vardef::AnalyzeDefaultNumTopN,
            "50",
            "0",
            "0",
            "100000",
        ),
    ] {
        let original = global.Load();
        let variable = sysvar(name);
        assert_eq!(variable.Scope, vardef::ScopeGlobal);
        assert_eq!(variable.Type, vardef::TypeUnsigned);
        let normalized = variable
            .Validate(&mut vars, value, vardef::ScopeGlobal)
            .unwrap();
        variable.SetGlobal.as_ref().unwrap()(&ctx, &mut vars, &normalized).unwrap();
        assert_eq!(global.Load().to_string(), value);
        assert_eq!(
            variable.GetGlobal.as_ref().unwrap()(&ctx, &mut vars).unwrap(),
            value
        );
        global.Store(original);
        assert_eq!(
            set_global_system_var(&mut vars, name, value).unwrap(),
            value
        );
        assert_eq!(global.Load().to_string(), value);
        assert_eq!(
            variable
                .Validate(&mut vars, min, vardef::ScopeGlobal)
                .unwrap(),
            min
        );
        assert_eq!(
            variable
                .Validate(&mut vars, max, vardef::ScopeGlobal)
                .unwrap(),
            max
        );
        assert_eq!(
            variable
                .Validate(&mut vars, below_min, vardef::ScopeGlobal)
                .unwrap(),
            min
        );
        assert_eq!(
            variable
                .Validate(&mut vars, "100001", vardef::ScopeGlobal)
                .unwrap(),
            max
        );
        global.Store(original);
    }
}

#[derive(Clone, Default)]
/// 内存态全局变量 accessor，供测试隔离读写。
struct MemoryGlobal {
    values: Arc<Mutex<HashMap<String, String>>>,
}

impl MemoryGlobal {
    fn with_defaults() -> Self {
        register_builtin_sysvars();
        Self {
            values: Arc::new(Mutex::new(
                GetSysVars()
                    .into_iter()
                    .map(|(name, variable)| (name, variable.Value))
                    .collect(),
            )),
        }
    }

    fn get(&self, name: &str) -> Option<String> {
        self.values.lock().unwrap().get(name).cloned()
    }
}

impl GlobalVarAccessor for MemoryGlobal {
    fn get_global_sys_var(&self, name: &str) -> Result<String, VariableError> {
        self.get(name).ok_or_else(|| VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &Context,
        name: &str,
        value: &str,
        _update_local: bool,
    ) -> Result<(), VariableError> {
        self.values
            .lock()
            .unwrap()
            .insert(name.to_ascii_lowercase(), value.to_owned());
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, VariableError> {
        self.get(name).ok_or_else(|| VariableError::unknown(name))
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

#[test]
fn kv_backoff_sysvars_validate_and_update_session_kv_vars() {
    let mut vars = SessionVars::new(Box::new(MemoryGlobal::with_defaults()));
    assert_eq!(vars.KVVars.BackoffLockFast, kv::DefBackoffLockFast);
    assert_eq!(vars.KVVars.BackOffWeight, kv::DefBackOffWeight);

    vars.SetSystemVar(vardef::TiDBBackoffLockFast, "1").unwrap();
    vars.SetSystemVar(vardef::TiDBBackOffWeight, "100").unwrap();
    assert_eq!(vars.KVVars.BackoffLockFast, 1);
    assert_eq!(vars.KVVars.BackOffWeight, 100);

    vars.SetSystemVar(vardef::TiDBBackoffLockFast, "0").unwrap();
    vars.SetSystemVar(vardef::TiDBBackOffWeight, "2147483648")
        .unwrap();
    assert_eq!(vars.KVVars.BackoffLockFast, 1);
    assert_eq!(vars.KVVars.BackOffWeight, i32::MAX);
    assert_eq!(vars.StmtCtx.WarningCount(), 2);
}

#[test]
fn txn_file_sysvars_match_go_defaults_validation_and_session_propagation() {
    let (mut vars, _) = session();

    let enable = sysvar(vardef::TiDBEnableTxnFile);
    assert_eq!(enable.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(enable.Value, vardef::Off);
    assert!(vars.KVVars.DisableTxnFile);
    for (input, disabled) in [(vardef::Off, true), (vardef::On, false)] {
        let normalized = enable
            .Validate(&mut vars, input, vardef::ScopeSession)
            .unwrap();
        enable.SetSessionFromHook(&mut vars, &normalized).unwrap();
        assert_eq!(vars.KVVars.DisableTxnFile, disabled);
    }

    let minimum = sysvar(vardef::TiDBTxnFileMinMutationSize);
    assert_eq!(minimum.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(minimum.Value, "0");
    assert_eq!(vars.KVVars.TxnFileMinMutationSize, 0);
    for rejected in [
        "1".to_owned(),
        (vardef::MinTiDBTxnFileMinMutationSize - 1).to_string(),
    ] {
        assert!(
            minimum
                .Validate(&mut vars, &rejected, vardef::ScopeSession)
                .is_err()
        );
    }
    let valid = (vardef::MinTiDBTxnFileMinMutationSize * 2).to_string();
    let normalized = minimum
        .Validate(&mut vars, &valid, vardef::ScopeSession)
        .unwrap();
    minimum.SetSessionFromHook(&mut vars, &normalized).unwrap();
    assert_eq!(
        vars.KVVars.TxnFileMinMutationSize,
        vardef::MinTiDBTxnFileMinMutationSize * 2
    );
    assert!(
        minimum
            .Validate(&mut vars, "1", vardef::ScopeSession)
            .is_err()
    );
    assert_eq!(
        vars.KVVars.TxnFileMinMutationSize,
        vardef::MinTiDBTxnFileMinMutationSize * 2
    );
    assert_eq!(
        minimum
            .Validate(&mut vars, "0", vardef::ScopeSession)
            .unwrap(),
        "0"
    );
}

#[test]
fn columnar_storage_gate_is_a_global_boolean_defaulting_on() {
    let (mut vars, _) = session();
    let variable = sysvar(vardef::TiDBColumnarStorageEnabled);
    assert_eq!(variable.Scope, vardef::ScopeGlobal);
    assert_eq!(variable.Type, vardef::TypeBool);
    assert_eq!(variable.Value, vardef::On);
    assert_eq!(
        set_global_system_var(&mut vars, vardef::TiDBColumnarStorageEnabled, "OFF").unwrap(),
        vardef::Off
    );
    assert_eq!(
        set_global_system_var(&mut vars, vardef::TiDBColumnarStorageEnabled, "1").unwrap(),
        vardef::On
    );
}

/// 创建带默认内置变量的会话与内存全局 accessor。
fn session() -> (SessionVars, MemoryGlobal) {
    let accessor = MemoryGlobal::with_defaults();
    (SessionVars::new(Box::new(accessor.clone())), accessor)
}

/// 按名取得已注册内置系统变量，缺失则 panic。
fn sysvar(name: &str) -> Arc<SysVar> {
    register_builtin_sysvars();
    GetSysVar(name).unwrap_or_else(|| panic!("missing builtin sysvar {name}"))
}

/// 在新会话上对指定变量做 Validate。
fn validate(name: &str, value: &str, scope: vardef::ScopeFlag) -> Result<String, VariableError> {
    let (mut vars, _) = session();
    sysvar(name).Validate(&mut vars, value, scope)
}

/// 设置全局系统变量并断言 accessor 已同步。
fn global_set(
    vars: &mut SessionVars,
    accessor: &MemoryGlobal,
    name: &str,
    value: &str,
) -> Result<String, VariableError> {
    let normalized = set_global_system_var(vars, name, value)?;
    assert_eq!(accessor.get(name).as_deref(), Some(normalized.as_str()));
    Ok(normalized)
}

/// 批量断言变量 Validate 的钳制/拒绝结果。
fn assert_clamps(name: &str, scope: vardef::ScopeFlag, cases: &[(&str, Result<&str, ()>)]) {
    let (mut vars, _) = session();
    let variable = sysvar(name);
    for (input, expected) in cases {
        let actual = variable.Validate(&mut vars, input, scope);
        match expected {
            Ok(expected) => assert_eq!(actual.unwrap(), *expected, "{name}: {input}"),
            Err(()) => assert!(actual.is_err(), "{name}: {input}"),
        }
    }
}

/// 校验 sql_select_limit 钳制与会话写入。
#[test]
fn TestSQLSelectLimit() {
    let (mut vars, _) = session();
    let variable = sysvar("sql_select_limit");
    assert_eq!(
        variable
            .Validate(&mut vars, "-10", vardef::ScopeSession)
            .unwrap(),
        "0"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "9999", vardef::ScopeSession)
            .unwrap(),
        "9999"
    );
    variable.SetSessionFromHook(&mut vars, "9999").unwrap();
    assert_eq!(vars.SelectLimit, 9999);
}

/// 校验 sql_mode 归一化与非法模式拒绝。
#[test]
fn TestSQLModeVar() {
    let (mut vars, _) = session();
    let variable = sysvar("sql_mode");
    let strict = "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION";
    assert_eq!(variable.Value, strict);
    assert_eq!(
        variable
            .Validate(&mut vars, "strict_trans_tabLES  ", vardef::ScopeSession)
            .unwrap(),
        "STRICT_TRANS_TABLES"
    );
    let error = variable
        .Validate(
            &mut vars,
            "strict_trans_tabLES,nonsense_option",
            vardef::ScopeSession,
        )
        .unwrap_err();
    assert_eq!(error.kind(), VariableErrorKind::WrongValue);
    assert_eq!(
        error.to_string(),
        "Variable 'sql_mode' can't be set to the value of 'NONSENSE_OPTION'"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, strict, vardef::ScopeSession)
            .unwrap(),
        strict
    );
    variable.SetSessionFromHook(&mut vars, strict).unwrap();
    assert_eq!(vars.system("sql_mode"), Some(strict));
    let non_strict = "ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION";
    let normalized = variable
        .Validate(&mut vars, non_strict, vardef::ScopeSession)
        .unwrap();
    assert_eq!(normalized, non_strict);
    variable.SetSessionFromHook(&mut vars, &normalized).unwrap();
    assert_eq!(vars.system("sql_mode"), Some(non_strict));
}

/// 校验 tidb_trace_event 配置读写。
#[test]
fn TestTiDBTraceEventSysVar() {
    let (mut vars, accessor) = session();
    let variable = sysvar("tidb_trace_event");
    let config = r#"{"enabled_categories":["*"],"dump_trigger":{"type":"sampling","sampling":1}}"#;
    let result = variable.Validate(&mut vars, config, vardef::ScopeGlobal);
    if kerneltype::IsClassic() {
        assert!(result.is_err());
    } else {
        assert_eq!(result.unwrap(), config);
        global_set(&mut vars, &accessor, "tidb_trace_event", config).unwrap();
        assert_eq!(TRACE_EVENT_CONFIG.lock().unwrap().as_deref(), Some(config));
        let updated = r#"{"enabled_categories":["general"],"dump_trigger":{"type":"sampling","sampling":10}}"#;
        global_set(&mut vars, &accessor, "tidb_trace_event", updated).unwrap();
        assert_eq!(TRACE_EVENT_CONFIG.lock().unwrap().as_deref(), Some(updated));
        global_set(&mut vars, &accessor, "tidb_trace_event", "").unwrap();
        assert!(TRACE_EVENT_CONFIG.lock().unwrap().is_none());
    }
}

/// 校验 max_execution_time 会话写入。
#[test]
fn TestMaxExecutionTime() {
    let (mut vars, _) = session();
    let variable = sysvar("max_execution_time");
    assert_eq!(
        variable
            .Validate(&mut vars, "-10", vardef::ScopeSession)
            .unwrap(),
        "0"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "99999", vardef::ScopeSession)
            .unwrap(),
        "99999"
    );
    variable.SetSessionFromHook(&mut vars, "99999").unwrap();
    assert_eq!(vars.MaxExecutionTime, 99999);
}

#[test]
fn TestDMLMaxExecutionTime() {
    let (mut vars, _) = session();
    let variable = sysvar(vardef::TiDBDMLMaxExecutionTime);
    assert_eq!(variable.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(variable.Value, "0");
    assert!(variable.IsHintUpdatableVerified);
    assert_eq!(
        variable
            .Validate(&mut vars, "-10", vardef::ScopeSession)
            .unwrap(),
        "0"
    );
    let value = variable
        .Validate(&mut vars, "99999", vardef::ScopeSession)
        .unwrap();
    variable.SetSessionFromHook(&mut vars, &value).unwrap();
    assert_eq!(vars.DMLMaxExecutionTime, 99999);
}

/// 校验 tidb_max_keys_read 提示可更新变量。
#[test]
fn TestTiDBMaxKeysRead() {
    let (mut vars, _) = session();
    let variable = sysvar("tidb_max_keys_read");
    for (input, expected) in [("-1", "0"), ("0", "0"), ("1000", "1000")] {
        assert_eq!(
            variable
                .Validate(&mut vars, input, vardef::ScopeSession)
                .unwrap(),
            expected
        );
    }
    variable.SetSessionFromHook(&mut vars, "500").unwrap();
    assert_eq!(vars.MaxKeysRead, 500);
    assert!(variable.IsHintUpdatableVerified);
}

/// 校验 GetMaxKeysRead 读取会话状态。
#[test]
fn TestGetMaxKeysRead() {
    let (mut vars, _) = session();
    vars.MaxKeysRead = 100;
    assert_eq!(vars.GetMaxKeysRead(), 0);
    vars.StmtCtx.InSelectStmt = true;
    assert_eq!(vars.GetMaxKeysRead(), 100);
    vars.MaxKeysRead = 0;
    assert_eq!(vars.GetMaxKeysRead(), 0);
}

/// 校验 TiFlash 外部落盘字节阈值变量。
#[test]
fn TestTiFlashMaxBytes() {
    for (index, name) in [
        "tidb_max_bytes_before_tiflash_external_join",
        "tidb_max_bytes_before_tiflash_external_group_by",
        "tidb_max_bytes_before_tiflash_external_sort",
    ]
    .into_iter()
    .enumerate()
    {
        let (mut vars, _) = session();
        let variable = sysvar(name);
        for scope in [vardef::ScopeSession, vardef::ScopeGlobal] {
            assert_eq!(variable.Validate(&mut vars, "-10", scope).unwrap(), "-1");
        }
        assert_eq!(
            variable
                .Validate(&mut vars, "100", vardef::ScopeSession)
                .unwrap(),
            "100"
        );
        assert!(
            variable
                .Validate(&mut vars, "9223372036854775808", vardef::ScopeSession)
                .is_err()
        );
        variable.SetSessionFromHook(&mut vars, "10000").unwrap();
        assert_eq!(
            [
                vars.TiFlashMaxBytesBeforeExternalJoin,
                vars.TiFlashMaxBytesBeforeExternalGroupBy,
                vars.TiFlashMaxBytesBeforeExternalSort,
            ][index],
            10000
        );
    }
}

/// 校验 TiFlash 单节点查询内存配额。
#[test]
fn TestTiFlashMemQuotaQueryPerNode() {
    let (mut vars, _) = session();
    let variable = sysvar("tiflash_mem_quota_query_per_node");
    assert_eq!(
        variable
            .Validate(&mut vars, "-10", vardef::ScopeSession)
            .unwrap(),
        "-1"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "-10", vardef::ScopeGlobal)
            .unwrap(),
        "-1"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "100", vardef::ScopeSession)
            .unwrap(),
        "100"
    );
    assert!(
        variable
            .Validate(&mut vars, "9223372036854775808", vardef::ScopeSession)
            .is_err()
    );
    variable.SetSessionFromHook(&mut vars, "10000").unwrap();
    assert_eq!(vars.TiFlashMaxQueryMemoryPerNode, 10000);
}

/// 校验 TiFlash spill 比例上限。
#[test]
fn TestTiFlashQuerySpillRatio() {
    let (mut vars, _) = session();
    let variable = sysvar("tiflash_query_spill_ratio");
    assert_eq!(
        variable
            .Validate(&mut vars, "-10", vardef::ScopeSession)
            .unwrap(),
        "0"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "-10", vardef::ScopeGlobal)
            .unwrap(),
        "0"
    );
    assert!(
        variable
            .Validate(&mut vars, "100", vardef::ScopeSession)
            .is_err()
    );
    assert!(
        variable
            .Validate(&mut vars, "0.9", vardef::ScopeSession)
            .is_err()
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "0.85", vardef::ScopeSession)
            .unwrap(),
        "0.85"
    );
    variable.SetSessionFromHook(&mut vars, "0.75").unwrap();
    assert_eq!(vars.TiFlashQuerySpillRatio, 0.75);
}

/// 校验 TiFlash hash join 版本枚举。
#[test]
fn TestTiFlashHashJoinVersion() {
    let (mut vars, _) = session();
    let variable = sysvar("tiflash_hash_join_version");
    assert!(
        variable
            .Validate(&mut vars, "invalid", vardef::ScopeSession)
            .is_err()
    );
    for value in [
        "legacy",
        "optimized",
        "Legacy",
        "Optimized",
        "LegaCy",
        "OptimiZed",
    ] {
        assert!(
            variable
                .Validate(&mut vars, value, vardef::ScopeSession)
                .is_ok()
        );
    }
}

/// 校验 collation_server 与字符集一致性。
#[test]
fn TestCollationServer() {
    let (mut vars, _) = session();
    let variable = sysvar("collation_server");
    assert_eq!(
        variable
            .Validate(&mut vars, "LATIN1_bin", vardef::ScopeSession)
            .unwrap(),
        "latin1_bin"
    );
    assert!(
        variable
            .Validate(&mut vars, "BOGUSCOLLation", vardef::ScopeSession)
            .is_err()
    );
    variable
        .SetSessionFromHook(&mut vars, "latin1_bin")
        .unwrap();
    assert_eq!(vars.system("character_set_server"), Some("latin1"));
    variable
        .SetSessionFromHook(&mut vars, "utf8mb4_bin")
        .unwrap();
    assert_eq!(vars.system("character_set_server"), Some("utf8mb4"));
}

/// 校验 utf8mb4 默认排序规则。
#[test]
fn TestDefaultCollationForUTF8MB4() {
    let (mut vars, _) = session();
    let variable = sysvar("default_collation_for_utf8mb4");
    for (value, expected, scope) in [
        ("utf8mb4_BIN", "utf8mb4_bin", vardef::ScopeSession),
        (
            "utf8mb4_GENeral_CI",
            "utf8mb4_general_ci",
            vardef::ScopeGlobal,
        ),
        (
            "utf8mb4_0900_AI_CI",
            "utf8mb4_0900_ai_ci",
            vardef::ScopeSession,
        ),
    ] {
        assert_eq!(
            variable.Validate(&mut vars, value, scope).unwrap(),
            expected
        );
    }
    assert_eq!(vars.StmtCtx.warnings().len(), 3);
    for warning in vars.StmtCtx.warnings() {
        assert_eq!(
            warning.to_string(),
            "Updating 'default_collation_for_utf8mb4' is deprecated. It will be made read-only in a future release."
        );
    }
    assert!(
        variable
            .Validate(&mut vars, "LATIN1_bin", vardef::ScopeSession)
            .is_err()
    );
}

/// 校验 time_zone 解析与非法值。
#[test]
fn TestTimeZone() {
    let (mut vars, _) = session();
    let variable = sysvar("time_zone");
    for value in ["America/Edmonton", "+10:00", "UTC", "+00:00"] {
        assert_eq!(
            variable
                .Validate(&mut vars, value, vardef::ScopeSession)
                .unwrap(),
            value
        );
    }
    variable.SetSessionFromHook(&mut vars, "UTC").unwrap();
    assert_eq!(vars.location(), FixedOffset::east_opt(0).unwrap());
}

/// 校验事务隔离级别（含 SERIALIZABLE 限制）。
#[test]
fn TestTxnIsolation() {
    let (mut vars, accessor) = session();
    let variable = sysvar("tx_isolation");
    assert!(
        variable
            .Validate(&mut vars, "on", vardef::ScopeSession)
            .is_err()
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "read-COMMitted", vardef::ScopeSession)
            .unwrap(),
        "READ-COMMITTED"
    );
    for value in ["Serializable", "read-uncommitted"] {
        assert!(
            variable
                .Validate(&mut vars, value, vardef::ScopeSession)
                .is_err()
        );
    }
    global_set(
        &mut vars,
        &accessor,
        "tidb_skip_isolation_level_check",
        "ON",
    )
    .unwrap();
    assert!(
        variable
            .Validate(&mut vars, "Serializable", vardef::ScopeSession)
            .is_err()
    );
    vars.SetSystemVar("tidb_skip_isolation_level_check", "ON")
        .unwrap();
    assert_eq!(
        variable
            .Validate(&mut vars, "Serializable", vardef::ScopeSession)
            .unwrap(),
        "SERIALIZABLE"
    );
    assert_eq!(vars.StmtCtx.warnings().len(), 1);
    assert_eq!(
        vars.StmtCtx.warnings()[0].kind(),
        VariableErrorKind::UnsupportedIsolationLevel
    );

    let (mut initialized, _) = session();
    initialized.set_system("tidb_skip_isolation_level_check", "1");
    assert_eq!(
        variable
            .Validate(&mut initialized, "Serializable", vardef::ScopeSession)
            .unwrap(),
        "SERIALIZABLE"
    );
}

/// 隔离读引擎变量应同步类型化会话状态。
#[test]
fn TestTiDBIsolationReadEnginesSynchronizesTypedState() {
    let mut vars = astersql_sessionctx_variable::session::SessionVars::default();
    vars.SetSystemVar(vardef::TiDBIsolationReadEngines, "tiflash, tikv")
        .unwrap();
    assert_eq!(
        vars.GetSystemVar(vardef::TiDBIsolationReadEngines),
        Some("tiflash,tikv".to_owned())
    );
    assert_eq!(vars.GetIsolationReadEngines().len(), 2);
    assert!(
        vars.GetIsolationReadEngines()
            .contains(&kv::StoreType::TiKV)
    );
    assert!(
        vars.GetIsolationReadEngines()
            .contains(&kv::StoreType::TiFlash)
    );
    assert!(
        !vars
            .GetIsolationReadEngines()
            .contains(&kv::StoreType::TiDB)
    );

    assert!(
        vars.SetSystemVar(vardef::TiDBIsolationReadEngines, "tikv,unknown")
            .is_err()
    );

    // Concrete sessions keep SessionVars behind Arc and update the validated
    // system-variable store. Effective planner getters must observe that path.
    let shared_vars = astersql_sessionctx_variable::session::SessionVars::default();
    shared_vars
        .SetHintSystemVarWithOldState(vardef::TiDBIsolationReadEngines, "tiflash")
        .unwrap();
    shared_vars
        .SetHintSystemVarWithOldState(vardef::TiDBAllowMPPExecution, "1")
        .unwrap();
    shared_vars
        .SetHintSystemVarWithOldState(vardef::TiDBEnforceMPPExecution, "1")
        .unwrap();
    assert_eq!(
        shared_vars.GetIsolationReadEngines(),
        std::collections::HashSet::from([kv::StoreType::TiFlash])
    );
    assert!(shared_vars.IsMPPAllowed());
    assert!(shared_vars.IsMPPEnforced());
}

/// TiFlash HashAgg 预聚合模式同步类型化状态。
#[test]
fn TestTiFlashHashAggPreAggModeSynchronizesTypedState() {
    let mut vars = astersql_sessionctx_variable::session::SessionVars::default();
    assert_eq!(
        vars.GetSystemVar(vardef::TiFlashHashAggPreAggMode),
        Some(vardef::DefTiFlashPreAggMode.to_owned())
    );
    assert_eq!(vars.TiFlashPreAggMode, vardef::DefTiFlashPreAggMode);
    for value in [
        vardef::ForcePreAggStr,
        vardef::AutoStr,
        vardef::ForceStreamingStr,
    ] {
        vars.SetSystemVar(vardef::TiFlashHashAggPreAggMode, value)
            .unwrap();
        assert_eq!(vars.TiFlashPreAggMode, value);
        assert_eq!(
            vars.GetSystemVar(vardef::TiFlashHashAggPreAggMode),
            Some(value.to_owned())
        );
    }
    assert!(
        vars.SetSystemVar(vardef::TiFlashHashAggPreAggMode, "test")
            .is_err()
    );
}

/// 校验多语句模式枚举。
#[test]
fn TestTiDBMultiStatementMode() {
    let (mut vars, _) = session();
    let variable = sysvar("tidb_multi_statement_mode");
    for (input, normalized, state) in [("on", "ON", 1), ("0", "OFF", 0), ("Warn", "WARN", 2)] {
        let value = variable
            .Validate(&mut vars, input, vardef::ScopeSession)
            .unwrap();
        assert_eq!(value, normalized);
        variable.SetSessionFromHook(&mut vars, &value).unwrap();
        assert_eq!(vars.MultiStatementMode, state);
    }
}

/// 只读实例上 noop 变量行为。
#[test]
fn TestReadOnlyNoop() {
    let (mut vars, accessor) = session();
    for name in ["tx_read_only", "transaction_read_only"] {
        let error = sysvar(name)
            .Validate(&mut vars, "on", vardef::ScopeSession)
            .unwrap_err();
        assert_eq!(error.kind(), VariableErrorKind::InvalidValue);
        assert!(error.to_string().contains("READ ONLY"));
        vars.SetSystemVar("tidb_enable_noop_functions", "ON")
            .unwrap();
        assert!(
            sysvar(name)
                .Validate(&mut vars, "on", vardef::ScopeSession)
                .is_ok()
        );
        vars.SetSystemVar("tidb_enable_noop_functions", "OFF")
            .unwrap();
    }
    for name in [
        "tx_read_only",
        "transaction_read_only",
        "offline_mode",
        "super_read_only",
        "read_only",
    ] {
        let error = sysvar(name)
            .Validate(&mut vars, "on", vardef::ScopeGlobal)
            .unwrap_err();
        assert_eq!(error.kind(), VariableErrorKind::InvalidValue);
        assert!(error.to_string().contains(if name == "offline_mode" {
            "OFFLINE MODE"
        } else {
            "READ ONLY"
        }));
        global_set(&mut vars, &accessor, "tidb_enable_noop_functions", "ON").unwrap();
        assert!(
            sysvar(name)
                .Validate(&mut vars, "on", vardef::ScopeGlobal)
                .is_ok()
        );
        global_set(&mut vars, &accessor, "tidb_enable_noop_functions", "OFF").unwrap();
    }
}

/// SkipInit 变量跳过初始化路径。
#[test]
fn TestSkipInit() {
    let mut variable = SysVar {
        Scope: vardef::ScopeGlobal,
        Name: "skipinit1".to_owned(),
        Value: "ON".to_owned(),
        Type: vardef::TypeBool,
        ..SysVar::default()
    };
    assert!(variable.SkipInit());
    variable.Scope = vardef::ScopeGlobal | vardef::ScopeSession;
    assert!(!variable.SkipInit());
    variable.Scope = vardef::ScopeSession;
    assert!(!variable.SkipInit());
    variable.skipInit = true;
    assert!(variable.SkipInit());
}

/// 会话侧 getter 钩子返回与 Go 一致。
#[test]
fn TestSessionGetterFuncs() {
    let (mut vars, _) = session();
    vars.TxnStartTS = 42;
    vars.LastTxnInfo = "txn".to_owned();
    vars.LastQueryInfo = r#"{"query":"select 1"}"#.to_owned();
    vars.PrevFoundInPlanCache = true;
    vars.PrevFoundInBinding = false;
    for (name, expected) in [
        ("tidb_current_ts", "42"),
        ("tidb_last_txn_info", "txn"),
        ("tidb_last_query_info", r#"{"query":"select 1"}"#),
        ("tidb_found_in_plan_cache", "ON"),
        (vardef::TiDBFoundInBinding, "OFF"),
    ] {
        assert_eq!(
            vars.GetSessionOrGlobalSystemVar(&Context, name).unwrap(),
            expected
        );
    }
}

/// 实例作用域变量读写。
#[test]
fn TestInstanceScopedVars() {
    let (mut vars, _) = session();
    for (name, expected) in [
        ("tidb_general_log", "OFF"),
        ("tidb_pprof_sql_cpu", "0"),
        ("tidb_expensive_query_time_threshold", "60"),
        ("tidb_expensive_txn_time_threshold", "600"),
        ("tidb_memory_usage_alarm_ratio", "0.8"),
        ("tidb_memory_usage_alarm_keep_record_num", "5"),
        ("tidb_force_priority", "NO_PRIORITY"),
        (vardef::TiDBDDLSlowOprThreshold, "300"),
        ("plugin_dir", ""),
        ("plugin_load", ""),
        ("tidb_slow_log_threshold", "300"),
        ("tidb_record_plan_in_slow_log", "OFF"),
        ("tidb_enable_slow_log", "ON"),
        ("tidb_check_mb4_value_in_utf8", "ON"),
        ("tidb_enable_collect_execution_info", "ON"),
        ("tidb_config", "{}"),
        ("tidb_log_file_max_days", "0"),
        ("tidb_rc_read_check_ts", "OFF"),
    ] {
        assert_eq!(
            vars.GetSessionOrGlobalSystemVar(&Context, name).unwrap(),
            expected
        );
    }
}

/// secure_auth 相关校验。
#[test]
fn TestSecureAuth() {
    assert!(validate("secure_auth", "OFF", vardef::ScopeGlobal).is_err());
    assert_eq!(
        validate("secure_auth", "ON", vardef::ScopeGlobal).unwrap(),
        "ON"
    );
}

/// 副本读策略变量。
#[test]
fn TestTiDBReplicaRead() {
    let result = validate("tidb_replica_read", "follower", vardef::ScopeGlobal);
    if kerneltype::IsNextGen() {
        assert!(result.is_err());
    } else {
        assert_eq!(result.unwrap(), "follower");
    }
}

/// sql_auto_is_null 行为。
#[test]
fn TestSQLAutoIsNull() {
    let (mut vars, _) = session();
    let auto = sysvar("sql_auto_is_null");
    assert!(
        auto.Validate(&mut vars, "ON", vardef::ScopeSession)
            .is_err()
    );
    vars.SetSystemVar("tidb_enable_noop_functions", "ON")
        .unwrap();
    let on = auto
        .Validate(&mut vars, "ON", vardef::ScopeSession)
        .unwrap();
    auto.SetSessionFromHook(&mut vars, &on).unwrap();
    assert_eq!(vars.system("sql_auto_is_null"), Some("ON"));
    assert!(
        vars.SetSystemVar("tidb_enable_noop_functions", "OFF")
            .is_err()
    );
    auto.SetSessionFromHook(&mut vars, "OFF").unwrap();
    vars.SetSystemVar("tidb_enable_noop_functions", "OFF")
        .unwrap();
    assert!(auto.Validate(&mut vars, "ON", vardef::ScopeGlobal).is_err());
}

/// last_insert_id 读写。
#[test]
fn TestLastInsertID() {
    let (mut vars, _) = session();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "last_insert_id")
            .unwrap(),
        "0"
    );
    vars.StmtCtx.PrevLastInsertID = 21;
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "last_insert_id")
            .unwrap(),
        "21"
    );
    vars.StmtCtx.PrevLastInsertID = 9_223_372_036_854_775_809;
    let value = vars
        .GetSessionOrGlobalSystemVar(&Context, "last_insert_id")
        .unwrap();
    assert_eq!(value, "9223372036854775809");
    let (datum, value_type, flags) = sysvar("last_insert_id").GetNativeValType(&value);
    assert_eq!(datum, Datum::Uint(9_223_372_036_854_775_809));
    assert_eq!(value_type, MYSQL_TYPE_LONGLONG);
    assert_eq!(flags, BINARY_FLAG | UNSIGNED_FLAG);
}

/// timestamp 系统变量与会话时间。
#[test]
fn TestTimestamp() {
    let (mut vars, _) = session();
    let first = vars
        .GetSessionOrGlobalSystemVar(&Context, "timestamp")
        .unwrap();
    assert!(!first.is_empty());
    vars.set_system("timestamp", "10");
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "timestamp")
            .unwrap(),
        "10"
    );
    vars.set_system("timestamp", "0");
    assert_ne!(
        vars.GetSessionOrGlobalSystemVar(&Context, "timestamp")
            .unwrap(),
        "10"
    );
    let variable = sysvar("timestamp");
    assert!(
        variable
            .Validate(&mut vars, "-5", vardef::ScopeSession)
            .is_ok()
    );
    assert_eq!(vars.StmtCtx.warnings().len(), 1);
    assert_eq!(
        vars.StmtCtx.warnings()[0].to_string(),
        "Truncated incorrect timestamp value: '-5'"
    );
    for value in ["3147483698", "2147483648"] {
        let error = variable
            .Validate(&mut vars, value, vardef::ScopeSession)
            .unwrap_err();
        assert_eq!(error.kind(), VariableErrorKind::WrongValue);
        assert_eq!(
            error.to_string(),
            format!("Variable 'timestamp' can't be set to the value of '{value}'")
        );
    }
    assert!(
        variable
            .Validate(&mut vars, "2147483647", vardef::ScopeSession)
            .is_ok()
    );
}

/// identity / last_insert_id 别名一致性。
#[test]
fn TestIdentity() {
    let (mut vars, _) = session();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "identity")
            .unwrap(),
        "0"
    );
    vars.StmtCtx.PrevLastInsertID = 21;
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "identity")
            .unwrap(),
        "21"
    );
}

/// lc_time_names 只读约束。
#[test]
fn TestLcTimeNamesReadOnly() {
    assert!(validate("lc_time_names", "newvalue", vardef::ScopeGlobal).is_err());
}

/// lc_messages 设置。
#[test]
fn TestLcMessages() {
    let (mut vars, _) = session();
    let variable = sysvar("lc_messages");
    assert_eq!(
        variable
            .Validate(&mut vars, "zh_CN", vardef::ScopeGlobal)
            .unwrap(),
        "zh_CN"
    );
    variable.SetSessionFromHook(&mut vars, "zh_CN").unwrap();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "lc_messages")
            .unwrap(),
        "zh_CN"
    );
}

/// DDL worker 并发相关变量。
#[test]
fn TestDDLWorkers() {
    assert_clamps(
        "tidb_ddl_reorg_worker_cnt",
        vardef::ScopeGlobal,
        &[("-100", Ok("1")), ("1234", Ok("256")), ("100", Ok("100"))],
    );
    assert_clamps(
        "tidb_ddl_reorg_batch_size",
        vardef::ScopeGlobal,
        &[
            ("10", Ok("32")),
            ("999999", Ok("10240")),
            ("100", Ok("100")),
        ],
    );
}

/// 默认字符集与排序规则联动。
#[test]
fn TestDefaultCharsetAndCollation() {
    let (mut vars, _) = session();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "character_set_connection")
            .unwrap(),
        "utf8mb4"
    );
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "collation_connection")
            .unwrap(),
        "utf8mb4_bin"
    );
}

/// index merge 开关。
#[test]
fn TestIndexMergeSwitcher() {
    let (mut vars, _) = session();
    assert!(vardef::DefTiDBEnableIndexMerge);
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "tidb_enable_index_merge")
            .unwrap(),
        "ON"
    );
}

/// net_buffer_length 钳制。
#[test]
fn TestNetBufferLength() {
    assert_clamps(
        "net_buffer_length",
        vardef::ScopeGlobal,
        &[
            ("1", Ok("1024")),
            ("10485760", Ok("1048576")),
            ("524288", Ok("524288")),
        ],
    );
}

/// 批量等待 TiFlash 数量阈值。
#[test]
fn TestTiDBBatchPendingTiFlashCount() {
    assert_clamps(
        "tidb_batch_pending_tiflash_count",
        vardef::ScopeSession,
        &[("-10", Ok("0")), ("9999", Ok("9999")), ("1.5", Err(()))],
    );
    let error = validate(
        "tidb_batch_pending_tiflash_count",
        "1.5",
        vardef::ScopeSession,
    )
    .unwrap_err();
    assert_eq!(error.kind(), VariableErrorKind::WrongType);
    assert_eq!(
        error.to_string(),
        "Incorrect argument type to variable 'tidb_batch_pending_tiflash_count'"
    );
}

/// 查询内存配额变量。
#[test]
fn TestTiDBMemQuotaQuery() {
    for scope in [vardef::ScopeGlobal, vardef::ScopeSession] {
        assert_clamps(
            "tidb_mem_quota_query",
            scope,
            &[("33554432", Ok("33554432")), ("-2", Ok("-1"))],
        );
    }
}

/// 查询日志最大长度。
#[test]
fn TestTiDBQueryLogMaxLen() {
    assert_clamps(
        "tidb_query_log_max_len",
        vardef::ScopeGlobal,
        &[
            ("33554432", Ok("33554432")),
            ("1073741825", Ok("1073741824")),
            ("-2", Ok("0")),
        ],
    );
}

/// committer 并发度。
#[test]
fn TestTiDBCommitterConcurrency() {
    assert_clamps(
        "tidb_committer_concurrency",
        vardef::ScopeGlobal,
        &[("1024", Ok("1024")), ("10001", Ok("10000")), ("0", Ok("1"))],
    );
}

/// DDL flashback 并发度。
#[test]
fn TestTiDBDDLFlashbackConcurrency() {
    assert_clamps(
        "tidb_ddl_flashback_concurrency",
        vardef::ScopeGlobal,
        &[("128", Ok("128")), ("257", Ok("256")), ("0", Ok("1"))],
    );
}

/// 内存调试模式默认值。
#[test]
fn TestDefaultMemoryDebugModeValue() {
    let (mut vars, _) = session();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "tidb_memory_debug_mode_min_heap_inuse")
            .unwrap(),
        "0"
    );
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "tidb_memory_debug_mode_alarm_ratio")
            .unwrap(),
        "0"
    );
}

/// 分布式 reorg 开关。
#[test]
fn TestSetTIDBDistributeReorg() {
    let (mut vars, accessor) = session();
    for value in ["ON", "OFF"] {
        assert_eq!(
            global_set(&mut vars, &accessor, "tidb_enable_dist_task", value).unwrap(),
            value
        );
    }
}

/// 分区裁剪模式默认值。
#[test]
fn TestDefaultPartitionPruneMode() {
    let (_, accessor) = session();
    assert_eq!(vardef::DefTiDBPartitionPruneMode, "dynamic");
    assert_eq!(
        accessor.get("tidb_partition_prune_mode").as_deref(),
        Some("dynamic")
    );
}

/// 快速 DDL 开关。
#[test]
fn TestSetTIDBFastDDL() {
    let (mut vars, accessor) = session();
    assert_eq!(sysvar("tidb_ddl_enable_fast_reorg").Value, "ON");
    for value in ["ON", "OFF"] {
        global_set(&mut vars, &accessor, "tidb_ddl_enable_fast_reorg", value).unwrap();
    }
}

/// 磁盘配额设置与校验。
#[test]
fn TestSetTIDBDiskQuota() {
    let (mut vars, accessor) = session();
    let gb = 1_u64 << 30;
    let pb = 1_u64 << 50;
    assert_eq!(sysvar("tidb_ddl_disk_quota").Value, (100 * gb).to_string());
    for (input, expected) in [
        (50 * gb, 100 * gb),
        (100 * gb, 100 * gb),
        (200 * gb, 200 * gb),
        (pb, pb),
        (2 * pb, pb),
    ] {
        assert_eq!(
            global_set(
                &mut vars,
                &accessor,
                "tidb_ddl_disk_quota",
                &input.to_string()
            )
            .unwrap(),
            expected.to_string()
        );
    }
}

/// 服务器内存上限解析。
#[test]
fn TestTiDBServerMemoryLimit() {
    let (mut vars, accessor) = session();
    let mb = 1_u64 << 20;
    assert_eq!(sysvar("tidb_server_memory_limit").Value, "80%");
    for (input, expected) in [
        ((100 * mb).to_string(), "512MB".to_owned()),
        ("0".to_owned(), "0".to_owned()),
        (u64::MAX.to_string(), u64::MAX.to_string()),
        ((1024 * mb).to_string(), (1024 * mb).to_string()),
    ] {
        assert_eq!(
            global_set(&mut vars, &accessor, "tidb_server_memory_limit", &input).unwrap(),
            expected
        );
    }
    assert_eq!(
        sysvar("tidb_server_memory_limit_sess_min_size").Value,
        (128_u64 << 20).to_string()
    );
    for (input, expected) in [("100", "128"), ("0", "0"), ("209715200", "209715200")] {
        assert_eq!(
            global_set(
                &mut vars,
                &accessor,
                "tidb_server_memory_limit_sess_min_size",
                input
            )
            .unwrap(),
            expected
        );
    }
    assert_eq!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_server_memory_limit_sess_min_size",
            &u64::MAX.to_string(),
        )
        .unwrap(),
        u64::MAX.to_string()
    );
}

/// 服务器内存上限补充边界用例。
#[test]
fn TestTiDBServerMemoryLimit2() {
    let (mut vars, accessor) = session();
    vars.memory_total = 100_u64 << 30;
    assert_eq!(
        global_set(&mut vars, &accessor, "tidb_server_memory_limit", "1%").unwrap(),
        "1%"
    );
    assert_eq!(
        SERVER_MEMORY_LIMIT.load(std::sync::atomic::Ordering::SeqCst),
        1_u64 << 30
    );
    for value in ["0%", "100%"] {
        assert!(global_set(&mut vars, &accessor, "tidb_server_memory_limit", value).is_err());
    }
    assert_eq!(
        global_set(&mut vars, &accessor, "tidb_server_memory_limit", "75%").unwrap(),
        "75%"
    );
    assert_eq!(
        SERVER_MEMORY_LIMIT.load(std::sync::atomic::Ordering::SeqCst),
        75_u64 << 30
    );
    vars.memory_total = 1_u64 << 30;
    assert_eq!(
        global_set(&mut vars, &accessor, "tidb_server_memory_limit", "1%").unwrap(),
        "512MB"
    );
    assert_eq!(
        SERVER_MEMORY_LIMIT.load(std::sync::atomic::Ordering::SeqCst),
        512_u64 << 20
    );
    vars.memory_total_available = false;
    assert!(global_set(&mut vars, &accessor, "tidb_server_memory_limit", "75%").is_err());
    vars.memory_total_available = true;
    for (input, expected, bytes) in [
        ("1234", "512MB", 512_u64 << 20),
        ("1234567890123", "1234567890123", 1_234_567_890_123),
        ("10KB", "512MB", 512_u64 << 20),
        ("12345678KB", "12345678KB", 12_345_678_u64 << 10),
        ("10MB", "512MB", 512_u64 << 20),
        ("700MB", "700MB", 700_u64 << 20),
        ("20GB", "20GB", 20_u64 << 30),
        ("2TB", "2TB", 2_u64 << 40),
    ] {
        assert_eq!(
            global_set(&mut vars, &accessor, "tidb_server_memory_limit", input).unwrap(),
            expected
        );
        assert_eq!(
            SERVER_MEMORY_LIMIT.load(std::sync::atomic::Ordering::SeqCst),
            bytes
        );
    }
    for value in ["123aaa123", "700MBaa", "a700MB"] {
        assert!(global_set(&mut vars, &accessor, "tidb_server_memory_limit", value).is_err());
    }
}

/// 会话最小内存触发阈值。
#[test]
fn TestTiDBServerMemoryLimitSessMinSize() {
    let (mut vars, accessor) = session();
    for (input, expected, bytes) in [
        ("123456", "123456", 123456_u64),
        ("100", "128", 128),
        ("123MB", "128974848", 123_u64 << 20),
    ] {
        assert_eq!(
            global_set(
                &mut vars,
                &accessor,
                "tidb_server_memory_limit_sess_min_size",
                input
            )
            .unwrap(),
            expected
        );
        assert_eq!(
            SERVER_MEMORY_LIMIT_SESS_MIN_SIZE.load(std::sync::atomic::Ordering::SeqCst),
            bytes
        );
    }
}

/// 对应 Go `TestTiDBServerMemoryLimitGCTrigger`：校验相关系统变量行为。
#[test]
#[serial]
fn TestTiDBServerMemoryLimitGCTrigger() {
    let (mut vars, accessor) = session();
    assert_eq!(
        sysvar("tidb_server_memory_limit_gc_trigger").Value,
        vardef::DefTiDBServerMemoryLimitGCTrigger.to_string()
    );
    assert_eq!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_server_memory_limit_gc_trigger",
            "0.8"
        )
        .unwrap(),
        "0.8"
    );
    assert_eq!(
        f64::from_bits(
            SERVER_MEMORY_LIMIT_GC_TRIGGER_BITS.load(std::sync::atomic::Ordering::SeqCst)
        ),
        0.8
    );
    assert_eq!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_server_memory_limit_gc_trigger",
            "90%"
        )
        .unwrap(),
        "0.9"
    );
    assert_eq!(
        f64::from_bits(
            SERVER_MEMORY_LIMIT_GC_TRIGGER_BITS.load(std::sync::atomic::Ordering::SeqCst)
        ),
        0.9
    );
    for value in ["100%", "101%"] {
        assert!(
            global_set(
                &mut vars,
                &accessor,
                "tidb_server_memory_limit_gc_trigger",
                value
            )
            .is_err()
        );
    }
    assert_eq!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_server_memory_limit_gc_trigger",
            "99%"
        )
        .unwrap(),
        "0.99"
    );
    global_set(&mut vars, &accessor, "tidb_gogc_tuner_threshold", "0.4").unwrap();
    assert!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_server_memory_limit_gc_trigger",
            "49%"
        )
        .is_err()
    );
    assert!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_server_memory_limit_gc_trigger",
            "51%"
        )
        .is_ok()
    );
    assert!(global_set(&mut vars, &accessor, "tidb_gogc_tuner_max_value", "50").is_err());
    global_set(&mut vars, &accessor, "tidb_gogc_tuner_min_value", "200").unwrap();
    assert!(global_set(&mut vars, &accessor, "tidb_gogc_tuner_min_value", "1000").is_err());
    global_set(&mut vars, &accessor, "tidb_gogc_tuner_min_value", "100").unwrap();
    global_set(&mut vars, &accessor, "tidb_gogc_tuner_max_value", "200").unwrap();
    global_set(
        &mut vars,
        &accessor,
        "tidb_server_memory_limit_gc_trigger",
        &vardef::DefTiDBServerMemoryLimitGCTrigger.to_string(),
    )
    .unwrap();
}

/// 对应 Go `TestSetAggPushDownGlobally`：校验相关系统变量行为。
#[test]
fn TestSetAggPushDownGlobally() {
    let (mut vars, accessor) = session();
    assert_eq!(
        accessor.get("tidb_opt_agg_push_down").as_deref(),
        Some("OFF")
    );
    global_set(&mut vars, &accessor, "tidb_opt_agg_push_down", "ON").unwrap();
}

/// 对应 Go `TestSetDeriveTopNGlobally`：校验相关系统变量行为。
#[test]
fn TestSetDeriveTopNGlobally() {
    let (mut vars, accessor) = session();
    assert_eq!(
        accessor.get(vardef::TiDBOptDeriveTopN).as_deref(),
        Some("OFF")
    );
    global_set(&mut vars, &accessor, vardef::TiDBOptDeriveTopN, "ON").unwrap();
}

/// 对应 Go `TestSetJobScheduleWindow`：校验相关系统变量行为。
#[test]
fn TestSetJobScheduleWindow() {
    let (mut vars, accessor) = session();
    assert_eq!(
        accessor
            .get("tidb_ttl_job_schedule_window_start_time")
            .as_deref(),
        Some("00:00 +0000")
    );
    assert_eq!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_ttl_job_schedule_window_start_time",
            "16:11"
        )
        .unwrap(),
        "16:11 +0000"
    );
    vars.set_location(FixedOffset::east_opt(8 * 3600).unwrap());
    assert_eq!(
        accessor
            .get("tidb_ttl_job_schedule_window_start_time")
            .as_deref(),
        Some("16:11 +0000")
    );
    assert_eq!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_ttl_job_schedule_window_start_time",
            "16:11"
        )
        .unwrap(),
        "16:11 +0800"
    );
    vars.set_location(FixedOffset::east_opt(0).unwrap());
    assert_eq!(
        accessor
            .get("tidb_ttl_job_schedule_window_start_time")
            .as_deref(),
        Some("16:11 +0800")
    );
}

/// 对应 Go `TestTiDBIgnoreInlistPlanDigest`：校验相关系统变量行为。
#[test]
fn TestTiDBIgnoreInlistPlanDigest() {
    let (mut vars, accessor) = session();
    assert_eq!(
        accessor.get("tidb_ignore_inlist_plan_digest").as_deref(),
        Some("ON")
    );
    global_set(&mut vars, &accessor, "tidb_ignore_inlist_plan_digest", "ON").unwrap();
}

/// 对应 Go `TestTiDBEnableResourceControl`：校验相关系统变量行为。
#[test]
#[serial]
fn TestTiDBEnableResourceControl() {
    let (mut vars, accessor) = session();
    RESOURCE_CONTROL_ENABLED.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(sysvar("tidb_enable_resource_control").Value, "ON");
    for (value, expected) in [("ON", true), ("OFF", false), ("ON", true)] {
        global_set(&mut vars, &accessor, "tidb_enable_resource_control", value).unwrap();
        assert_eq!(
            RESOURCE_CONTROL_ENABLED.load(std::sync::atomic::Ordering::SeqCst),
            expected
        );
    }
}

/// 对应 Go `TestTiDBResourceControlStrictMode`：校验相关系统变量行为。
#[test]
#[serial]
fn TestTiDBResourceControlStrictMode() {
    let (mut vars, accessor) = session();
    assert_eq!(sysvar("tidb_resource_control_strict_mode").Value, "ON");
    for (value, expected) in [("OFF", false), ("ON", true)] {
        global_set(
            &mut vars,
            &accessor,
            "tidb_resource_control_strict_mode",
            value,
        )
        .unwrap();
        assert_eq!(
            RESOURCE_CONTROL_STRICT.load(std::sync::atomic::Ordering::SeqCst),
            expected
        );
    }
}

/// 对应 Go `TestTiDBEnableRowLevelChecksum`：校验相关系统变量行为。
#[test]
fn TestTiDBEnableRowLevelChecksum() {
    let (mut vars, accessor) = session();
    assert_eq!(
        accessor.get("tidb_enable_row_level_checksum").as_deref(),
        Some("OFF")
    );
    global_set(&mut vars, &accessor, "tidb_enable_row_level_checksum", "ON").unwrap();
    global_set(
        &mut vars,
        &accessor,
        "tidb_enable_row_level_checksum",
        "OFF",
    )
    .unwrap();
}

/// 对应 Go `TestTiDBAutoAnalyzeRatio`：校验相关系统变量行为。
#[test]
fn TestTiDBAutoAnalyzeRatio() {
    let (mut vars, accessor) = session();
    assert_eq!(
        accessor.get("tidb_auto_analyze_ratio").as_deref(),
        Some("0.5")
    );
    for value in ["0.1", "1.1"] {
        global_set(&mut vars, &accessor, "tidb_auto_analyze_ratio", value).unwrap();
    }
    for value in ["0", "0.0000000001"] {
        assert!(global_set(&mut vars, &accessor, "tidb_auto_analyze_ratio", value).is_err());
        assert_eq!(
            accessor.get("tidb_auto_analyze_ratio").as_deref(),
            Some("1.1")
        );
    }
    global_set(&mut vars, &accessor, "tidb_auto_analyze_ratio", "0.00001").unwrap();
    assert_eq!(
        accessor.get("tidb_auto_analyze_ratio").as_deref(),
        Some("0.00001")
    );
    assert!(
        global_set(
            &mut vars,
            &accessor,
            "tidb_auto_analyze_ratio",
            "0.000009999"
        )
        .is_err()
    );
    assert_eq!(
        accessor.get("tidb_auto_analyze_ratio").as_deref(),
        Some("0.00001")
    );
}

/// 对应 Go `TestTiDBTiFlashReplicaRead`：校验相关系统变量行为。
#[test]
fn TestTiDBTiFlashReplicaRead() {
    let (mut vars, accessor) = session();
    assert_eq!(sysvar("tiflash_replica_read").Value, "all_replicas");
    for value in ["all_replicas", "closest_adaptive", "closest_replicas"] {
        global_set(&mut vars, &accessor, "tiflash_replica_read", value).unwrap();
    }
    global_set(&mut vars, &accessor, "tiflash_replica_read", "all_replicas").unwrap();
    assert!(global_set(&mut vars, &accessor, "tiflash_replica_read", "random").is_err());
    assert_eq!(
        accessor.get("tiflash_replica_read").as_deref(),
        Some("all_replicas")
    );
}

/// 对应 Go `TestGlobalSystemVariableInitialValue`：校验相关系统变量行为。
#[test]
fn TestGlobalSystemVariableInitialValue() {
    let runtime = sysvar::RuntimeEnvironment {
        store_is_tikv: false,
        in_test: true,
        next_gen: false,
        default_txn_assertion_level: "STRICT".to_owned(),
    };
    for (name, value, expected) in [
        (
            vardef::TiDBTxnMode,
            vardef::OptimisticTxnMode,
            vardef::OptimisticTxnMode,
        ),
        (vardef::TiDBMemOOMAction, "CANCEL", vardef::OOMActionLog),
        (vardef::TiDBEnableAutoAnalyze, "ON", vardef::Off),
        (vardef::TiDBRowFormatVersion, "1", "2"),
        (
            vardef::TiDBTxnAssertionLevel,
            "OFF",
            vardef::AssertionFastStr,
        ),
        (vardef::TiDBEnableMutationChecker, "OFF", vardef::On),
        (vardef::TiDBEnableAdaptiveLimitScan, vardef::Off, vardef::On),
        (
            vardef::TiDBPessimisticTransactionFairLocking,
            "OFF",
            vardef::On,
        ),
        (vardef::TiDBEnableAsyncCommit, vardef::Off, vardef::Off),
        (vardef::TiDBEnable1PC, vardef::Off, vardef::Off),
    ] {
        assert_eq!(
            sysvar::GlobalSystemVariableInitialValueWithRuntime(name, value, &runtime),
            expected
        );
    }

    // Go only overrides the explicitly listed dynamic defaults. In particular,
    // TiDBTxnMode is returned unchanged even when it differs from the registry
    // default supplied by the caller.
    assert_eq!(
        sysvar::GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBTxnMode,
            "caller-provided",
            &runtime,
        ),
        "caller-provided"
    );
    let tikv_runtime = sysvar::RuntimeEnvironment {
        store_is_tikv: true,
        ..runtime
    };
    for name in [vardef::TiDBEnableAsyncCommit, vardef::TiDBEnable1PC] {
        assert_eq!(
            sysvar::GlobalSystemVariableInitialValueWithRuntime(name, vardef::Off, &tikv_runtime),
            vardef::On
        );
    }
    let next_gen_runtime = sysvar::RuntimeEnvironment {
        store_is_tikv: false,
        in_test: true,
        next_gen: true,
        default_txn_assertion_level: vardef::AssertionStrictStr.to_owned(),
    };
    assert_eq!(
        sysvar::GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBTxnAssertionLevel,
            vardef::AssertionOffStr,
            &next_gen_runtime,
        ),
        vardef::AssertionStrictStr
    );
    assert_eq!(
        sysvar::GlobalSystemVariableInitialValueWithRuntime(
            vardef::TiDBPessimisticTransactionFairLocking,
            vardef::On,
            &next_gen_runtime,
        ),
        vardef::Off
    );
}

#[test]
fn adaptive_limit_scan_is_global_and_session_scoped_and_updates_session_state() {
    register_builtin_sysvars();
    let variable = GetSysVar(vardef::TiDBEnableAdaptiveLimitScan).unwrap();
    assert!(variable.Scope.String().contains("GLOBAL"));
    assert!(variable.Scope.String().contains("SESSION"));
    assert_eq!(variable.Value, vardef::Off);

    let (mut vars, _) = session();
    variable.SetSessionFromHook(&mut vars, vardef::On).unwrap();
    assert!(vars.EnableAdaptiveLimitScan);
}

/// Go uses strings.FieldsFunc for custom policy key/value parsing, which
/// discards empty fields created by repeated or edge separators.
#[test]
fn TestPipelinedDmlResourcePolicyFieldsFuncSeparators() {
    let mut vars = sysvar::SessionVars::default();
    sysvar::setPipelinedDmlResourcePolicy(
        &mut vars,
        "custom{=concurrency==8, resolve_concurrency::4, write_throttle_ratio=:0.25}",
    )
    .unwrap();
    assert_eq!(vars.PipelinedDMLConfig.PipelinedFlushConcurrency, 8);
    assert_eq!(vars.PipelinedDMLConfig.PipelinedResolveLockConcurrency, 4);
    assert_eq!(vars.PipelinedDMLConfig.PipelinedWriteThrottleRatio, 0.25);
}

/// 对应 Go `TestTiDBOptTxnAutoRetry`：校验相关系统变量行为。
#[test]
fn TestTiDBOptTxnAutoRetry() {
    let (mut vars, _) = session();
    let variable = sysvar("tidb_disable_txn_auto_retry");
    for scope in [vardef::ScopeSession, vardef::ScopeGlobal] {
        assert_eq!(variable.Validate(&mut vars, "OFF", scope).unwrap(), "ON");
    }
    assert_eq!(vars.StmtCtx.warnings().len(), 2);
}

/// 对应 Go `TestTiDBLowResTSOUpdateInterval`：校验相关系统变量行为。
#[test]
fn TestTiDBLowResTSOUpdateInterval() {
    let (mut vars, _) = session();
    let variable = sysvar("tidb_low_resolution_tso_update_interval");
    assert_eq!(
        variable
            .Validate(&mut vars, "0", vardef::ScopeGlobal)
            .unwrap(),
        "10"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "100000", vardef::ScopeGlobal)
            .unwrap(),
        "60000"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "1000", vardef::ScopeGlobal)
            .unwrap(),
        "1000"
    );
    assert_eq!(vars.StmtCtx.warnings().len(), 2);
    assert!(vars.StmtCtx.warnings()[0].to_string().contains("'0'"));
    assert!(vars.StmtCtx.warnings()[1].to_string().contains("'100000'"));
}

/// 对应 Go `TestTiDBSchemaCacheSize`：校验相关系统变量行为。
#[test]
fn TestTiDBSchemaCacheSize() {
    let (mut vars, accessor) = session();
    assert_eq!(
        sysvar("tidb_schema_cache_size").Value,
        (512_u64 << 20).to_string()
    );
    for (input, expected, bytes) in [
        ((63_u64 << 20).to_string(), "64MB".to_owned(), 64_u64 << 20),
        (i64::MAX.to_string(), i64::MAX.to_string(), i64::MAX as u64),
        (
            ((64_u64 << 20) - 1).to_string(),
            "64MB".to_owned(),
            64_u64 << 20,
        ),
        (
            (i64::MAX as u64 + 1).to_string(),
            i64::MAX.to_string(),
            i64::MAX as u64,
        ),
        (
            (1024_u64 << 20).to_string(),
            (1024_u64 << 20).to_string(),
            1024_u64 << 20,
        ),
        ("0".to_owned(), "0".to_owned(), 0),
        (
            "1234567890123".to_owned(),
            "1234567890123".to_owned(),
            1_234_567_890_123,
        ),
        ("10KB".to_owned(), "64MB".to_owned(), 64_u64 << 20),
        (
            "12345678KB".to_owned(),
            "12345678KB".to_owned(),
            12_345_678_u64 << 10,
        ),
        ("700MB".to_owned(), "700MB".to_owned(), 700_u64 << 20),
        ("20GB".to_owned(), "20GB".to_owned(), 20_u64 << 30),
        ("2TB".to_owned(), "2TB".to_owned(), 2_u64 << 40),
    ] {
        assert_eq!(
            global_set(&mut vars, &accessor, "tidb_schema_cache_size", &input).unwrap(),
            expected
        );
        assert_eq!(
            SCHEMA_CACHE_SIZE.load(std::sync::atomic::Ordering::SeqCst),
            bytes
        );
    }
    for value in ["123aaa123", "700MBaa", "a700MB"] {
        assert!(global_set(&mut vars, &accessor, "tidb_schema_cache_size", value).is_err());
    }
}

/// 对应 Go `TestTiDBCircuitBreakerPDMetadataErrorRateThresholdRatio`：校验相关系统变量行为。
#[test]
fn TestTiDBCircuitBreakerPDMetadataErrorRateThresholdRatio() {
    let (mut vars, _) = session();
    let variable = sysvar("tidb_cb_pd_metadata_error_rate_threshold_ratio");
    assert_eq!(
        variable
            .Validate(&mut vars, "-1", vardef::ScopeGlobal)
            .unwrap(),
        "0"
    );
    assert_eq!(
        variable
            .Validate(&mut vars, "1.1", vardef::ScopeGlobal)
            .unwrap(),
        "1"
    );
    for value in ["0.9", "0.0"] {
        assert_eq!(
            variable
                .Validate(&mut vars, value, vardef::ScopeGlobal)
                .unwrap(),
            value
        );
    }
    assert_eq!(vars.StmtCtx.warnings().len(), 2);
    assert!(vars.StmtCtx.warnings()[0].to_string().contains("'-1'"));
    assert!(vars.StmtCtx.warnings()[1].to_string().contains("'1.1'"));
}

/// 对应 Go `TestEnableWindowFunction`：校验相关系统变量行为。
#[test]
fn TestEnableWindowFunction() {
    let (mut vars, _) = session();
    assert_eq!(vars.EnableWindowFunction, vardef::DefEnableWindowFunction);
    for (value, expected) in [("on", true), ("0", false), ("1", true)] {
        vars.SetSystemVar("tidb_enable_window_function", value)
            .unwrap();
        assert_eq!(vars.EnableWindowFunction, expected);
    }
}

#[test]
fn pipelined_window_function_accepts_go_boolean_settings() {
    let (mut vars, _) = session();
    let name = vardef::TiDBEnablePipelinedWindowFunction;
    assert_eq!(sysvar(name).Value, "ON");
    for (value, expected) in [("on", "ON"), ("0", "OFF"), ("1", "ON")] {
        vars.SetSystemVar(name, value).unwrap();
        assert_eq!(
            sysvar(name).GetSessionFromHook(&mut vars).unwrap(),
            expected
        );
    }
    assert!(vars.SetSystemVar(name, "invalid").is_err());
}

/// 对应 Go `TestTiDBHashJoinVersion`：校验相关系统变量行为。
#[test]
fn TestTiDBHashJoinVersion() {
    let (mut vars, _) = session();
    assert!(vars.UseHashJoinV2);
    let variable = sysvar("tidb_hash_join_version");
    assert!(
        variable
            .Validate(&mut vars, "invalid", vardef::ScopeSession)
            .is_err()
    );
    for value in [
        "legacy",
        "optimized",
        "Legacy",
        "Optimized",
        "LegaCy",
        "OptimiZed",
    ] {
        assert!(
            variable
                .Validate(&mut vars, value, vardef::ScopeSession)
                .is_ok()
        );
    }
    variable.SetSessionFromHook(&mut vars, "legacy").unwrap();
    assert!(!vars.UseHashJoinV2);
    variable.SetSessionFromHook(&mut vars, "optimized").unwrap();
    assert!(vars.UseHashJoinV2);
}

/// 对应 Go `TestTiDBAutoAnalyzeConcurrencyValidation`：校验相关系统变量行为。
#[test]
#[serial]
fn TestTiDBAutoAnalyzeConcurrencyValidation() {
    let (mut vars, _) = session();
    let variable = sysvar("tidb_auto_analyze_concurrency");
    assert_eq!(variable.Value, "3");
    for (auto, priority, should_error) in [
        (false, true, true),
        (true, false, true),
        (false, false, true),
        (true, true, false),
    ] {
        vardef::RunAutoAnalyze.Store(auto);
        vardef::EnableAutoAnalyzePriorityQueue.Store(priority);
        assert_eq!(
            variable
                .Validate(&mut vars, "10", vardef::ScopeGlobal)
                .is_err(),
            should_error
        );
    }
}

/// 对应 Go `TestTiDBOptSelectivityFactor`：校验相关系统变量行为。
#[test]
fn TestTiDBOptSelectivityFactor() {
    let (mut vars, accessor) = session();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "tidb_opt_selectivity_factor")
            .unwrap(),
        "0.8"
    );
    vars.SetSystemVar("tidb_opt_selectivity_factor", "0.7")
        .unwrap();
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, "tidb_opt_selectivity_factor")
            .unwrap(),
        "0.7"
    );
    assert_eq!(
        global_set(&mut vars, &accessor, "tidb_opt_selectivity_factor", "1.1").unwrap(),
        "1"
    );
    assert!(!vars.StmtCtx.warnings().is_empty());
}

/// 对应 Go `TestSynonyms`：校验相关系统变量行为。
#[test]
fn TestSynonyms() {
    let (mut vars, _) = session();
    let variable = sysvar("tx_isolation");
    assert!(
        variable
            .Validate(&mut vars, "SERIALIZABLE", vardef::ScopeSession)
            .is_err()
    );
    vars.SetSystemVar("tidb_skip_isolation_level_check", "ON")
        .unwrap();
    assert_eq!(
        variable
            .Validate(&mut vars, "SERIALIZABLE", vardef::ScopeSession)
            .unwrap(),
        "SERIALIZABLE"
    );
    variable
        .SetSessionFromHook(&mut vars, "SERIALIZABLE")
        .unwrap();
    assert_eq!(vars.system("tx_isolation"), Some("SERIALIZABLE"));
    assert_eq!(vars.system("transaction_isolation"), Some("SERIALIZABLE"));
    assert_eq!(vars.StmtCtx.warnings().len(), 1);
    assert_eq!(
        vars.StmtCtx.warnings()[0].kind(),
        VariableErrorKind::UnsupportedIsolationLevel
    );
}

/// 对应 Go `TestScope`：校验相关系统变量行为。
#[test]
fn TestScope() {
    let mut variable = enum_var_for_scope(vardef::ScopeGlobal | vardef::ScopeSession);
    assert!(variable.HasSessionScope());
    assert!(variable.HasGlobalScope());
    assert!(!variable.HasInstanceScope());
    assert!(!variable.HasNoneScope());
    variable = enum_var_for_scope(vardef::ScopeGlobal);
    assert!(!variable.HasSessionScope());
    assert!(variable.HasGlobalScope());
    assert!(!variable.HasInstanceScope());
    assert!(!variable.HasNoneScope());
    variable = enum_var_for_scope(vardef::ScopeSession);
    assert!(variable.HasSessionScope());
    assert!(!variable.HasGlobalScope());
    assert!(!variable.HasInstanceScope());
    assert!(!variable.HasNoneScope());
    variable = enum_var_for_scope(vardef::ScopeNone);
    assert!(!variable.HasSessionScope());
    assert!(!variable.HasGlobalScope());
    assert!(!variable.HasInstanceScope());
    assert!(variable.HasNoneScope());
    variable = enum_var_for_scope(vardef::ScopeInstance);
    assert!(!variable.HasSessionScope());
    assert!(!variable.HasGlobalScope());
    assert!(variable.HasInstanceScope());
    assert!(!variable.HasNoneScope());
    let (mut vars, _) = session();
    assert!(
        enum_var_for_scope(vardef::ScopeSession)
            .Validate(&mut vars, "ON", vardef::ScopeGlobal)
            .is_err()
    );
}

/// `enum_var_for_scope` 辅助函数。
fn enum_var_for_scope(scope: vardef::ScopeFlag) -> SysVar {
    SysVar {
        Scope: scope,
        Name: "mynewsysvar".to_owned(),
        Value: "ON".to_owned(),
        Type: vardef::TypeEnum,
        PossibleValues: vec!["OFF".to_owned(), "ON".to_owned(), "AUTO".to_owned()],
        ..SysVar::default()
    }
}

/// Go 会注册完整 `noopSysVars` 表；覆盖客户端常探测的 event_scheduler。
#[test]
fn TestNoopCompatibilitySysVarsAreRegistered() {
    let (mut vars, accessor) = session();

    for definition in noop::NOOP_SYS_VARS {
        assert!(
            GetSysVar(definition.name).is_some(),
            "missing noop sysvar {}",
            definition.name
        );
    }

    let event_scheduler = sysvar("EvEnT_ScHeDuLeR");
    assert_eq!(event_scheduler.Value, vardef::Off);
    assert!(event_scheduler.HasGlobalScope());
    assert!(!event_scheduler.HasSessionScope());
    assert!(event_scheduler.IsNoop);
    assert_eq!(
        global_set(&mut vars, &accessor, "event_scheduler", vardef::On).unwrap(),
        vardef::On
    );
}

/// 对应 Go `TestSkipInitIsUsed`：校验相关系统变量行为。
#[test]
fn TestSkipInitIsUsed() {
    register_builtin_sysvars();
    let mut found = 0;
    for variable in GetSysVars()
        .into_values()
        .filter(|variable| variable.skipInit)
    {
        found += 1;
        assert!(variable.HasSessionScope());
        assert!(variable.SetSession.is_some());
        assert!(!variable.IsNoop);
        if !matches!(variable.Name.as_str(), "rand_seed1" | "rand_seed2") {
            assert_ne!(variable.Value, "0");
            assert_ne!(variable.Value, "OFF");
        }
        assert!(matches!(
            variable.Name.as_str(),
            "tidb_snapshot"
                | "tidb_enable_chunk_rpc"
                | "tx_isolation_one_shot"
                | "tidb_ddl_reorg_priority"
                | "tidb_slow_query_file"
                | "tidb_wait_split_region_finish"
                | "tidb_wait_split_region_timeout"
                | "tidb_metric_query_step"
                | "tidb_metric_query_range_duration"
                | "rand_seed1"
                | "rand_seed2"
                | "collation_database"
                | "collation_connection"
                | "character_set_database"
                | "character_set_connection"
                | "character_set_server"
                | "tidb_opt_tiflash_concurrency_factor"
                | "tidb_opt_seek_factor"
        ));
    }
    assert_eq!(
        found, 18,
        "the deprecated skipInit whitelist must remain explicit"
    );
}
#[test]
fn go_merge_46_full_outer_join_sysvar_defaults_off_and_tracks_session_value() {
    let mut vars = crate::session::SessionVars::default();
    assert_eq!(
        vars.EnableFullOuterJoin,
        crate::vardef::DefTiDBEnableFullOuterJoin
    );
    vars.SetSystemVar(crate::vardef::TiDBEnableFullOuterJoin, "ON")
        .unwrap();
    assert!(vars.EnableFullOuterJoin);
    vars.SetSystemVar(crate::vardef::TiDBEnableFullOuterJoin, "0")
        .unwrap();
    assert!(!vars.EnableFullOuterJoin);
    vars.SetSystemVar(crate::vardef::TiDBEnableFullOuterJoin, "1")
        .unwrap();
    assert!(vars.EnableFullOuterJoin);
}

#[test]
fn global_hooks_unsigned_validation() {
    go_merge_47_analyze_defaults_follow_global_sysvars();
    let (mut vars, _) = session();
    let ctx = Context;
    for (name, state) in [
        (
            vardef::TiDBAnalyzeDefaultNumBuckets,
            &vardef::AnalyzeDefaultNumBuckets,
        ),
        (
            vardef::TiDBAnalyzeDefaultNumTopN,
            &vardef::AnalyzeDefaultNumTopN,
        ),
    ] {
        let variable = sysvar(name);
        let before = state.Load();
        assert!(variable.SetGlobal.as_ref().unwrap()(&ctx, &mut vars, "bad").is_err());
        assert_eq!(state.Load(), before);
        assert!(
            variable
                .Validate(&mut vars, "100", vardef::ScopeSession)
                .is_err()
        );
        variable
            .SetGlobalFromHook(&ctx, &mut vars, &before.to_string(), false)
            .unwrap();
    }
}

#[test]
#[serial]
fn stats_load_pseudo_timeout_global_hooks_validate_and_control_fallback() {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            vardef::StatsLoadPseudoTimeout.Store(self.0);
        }
    }
    let _restore = Restore(vardef::StatsLoadPseudoTimeout.Load());
    let (mut vars, accessor) = session();
    let variable = sysvar(vardef::TiDBStatsLoadPseudoTimeout);
    assert_eq!(variable.Scope, vardef::ScopeGlobal);
    assert_eq!(variable.Type, vardef::TypeBool);
    assert_eq!(
        variable.Value,
        BoolToOnOff(vardef::DefTiDBStatsLoadPseudoTimeout)
    );
    for (input, expected) in [("off", "OFF"), ("on", "ON"), ("0", "OFF"), ("1", "ON")] {
        assert_eq!(
            global_set(&mut vars, &accessor, variable.Name.as_str(), input).unwrap(),
            expected
        );
        assert_eq!(vardef::StatsLoadPseudoTimeout.Load(), expected == "ON");
        assert_eq!(
            variable
                .GetGlobalFromHook(&Context::default(), &mut vars)
                .unwrap(),
            expected
        );
    }
    assert!(
        variable
            .Validate(&mut vars, "OFF", vardef::ScopeSession)
            .is_err()
    );
    assert!(
        variable
            .Validate(&mut vars, "invalid", vardef::ScopeGlobal)
            .is_err()
    );
}

#[test]
fn full_outer_join_sysvar_supports_hints_and_boolean_values() {
    let (mut validation_vars, _) = session();
    let variable = sysvar(vardef::TiDBEnableFullOuterJoin);
    assert_eq!(variable.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(variable.Type, vardef::TypeBool);
    assert_eq!(variable.Value, "OFF");
    assert!(variable.IsHintUpdatableVerified);
    let mut vars = crate::session::SessionVars::default();
    assert!(!vars.EnableFullOuterJoin);
    for (input, expected) in [("on", true), ("0", false), ("1", true), ("off", false)] {
        for scope in [vardef::ScopeGlobal, vardef::ScopeSession] {
            assert_eq!(
                variable
                    .Validate(&mut validation_vars, input, scope)
                    .unwrap(),
                if expected { "ON" } else { "OFF" }
            );
        }
        vars.SetSystemVar(vardef::TiDBEnableFullOuterJoin, input)
            .unwrap();
        assert_eq!(vars.EnableFullOuterJoin, expected);
    }
    assert!(
        vars.SetSystemVar(vardef::TiDBEnableFullOuterJoin, "invalid")
            .is_err()
    );
    assert!(!vars.EnableFullOuterJoin);
}

#[test]
fn tikv_short_circuit_expression_sysvar_updates_session_and_statement_state() {
    let (mut validation_vars, _) = session();
    let variable = sysvar(vardef::TiDBEnableTiKVShortCircuitExpression);
    assert_eq!(variable.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(variable.Type, vardef::TypeBool);
    assert_eq!(variable.Value, vardef::Off);
    assert!(variable.IsHintUpdatableVerified);

    assert!(!validation_vars.EnableTiKVShortCircuitExpression);
    assert!(!validation_vars.StmtCtx.EnableTiKVShortCircuitExpression);
    variable
        .SetSessionFromHook(&mut validation_vars, vardef::On)
        .unwrap();
    assert!(validation_vars.EnableTiKVShortCircuitExpression);
    assert!(validation_vars.StmtCtx.EnableTiKVShortCircuitExpression);
    variable
        .SetSessionFromHook(&mut validation_vars, vardef::Off)
        .unwrap();
    assert!(!validation_vars.EnableTiKVShortCircuitExpression);
    assert!(!validation_vars.StmtCtx.EnableTiKVShortCircuitExpression);

    let mut runtime_vars = crate::session::SessionVars::default();
    runtime_vars
        .SetSystemVar(vardef::TiDBEnableTiKVShortCircuitExpression, vardef::On)
        .unwrap();
    assert!(runtime_vars.EnableTiKVShortCircuitExpression());
    runtime_vars
        .SetSystemVar(vardef::TiDBEnableTiKVShortCircuitExpression, vardef::Off)
        .unwrap();
    assert!(!runtime_vars.EnableTiKVShortCircuitExpression());
}

#[test]
fn foreign_key_shared_lock_has_runtime_registration() {
    let (mut vars, _) = session();
    let name = vardef::TiDBForeignKeyCheckInSharedLock;
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, name).unwrap(),
        "OFF"
    );
    vars.SetSystemVar(name, "OFF").unwrap();
}

#[test]
#[serial]
fn foreign_key_shared_lock_gate_preserves_reads_and_initialization() {
    struct Restore(Option<Box<dyn FnOnce()>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            self.0.take().unwrap()();
        }
    }
    let _restore = Restore(Some(Box::new(config::restore_func())));
    let name = vardef::TiDBForeignKeyCheckInSharedLock;
    config::update_global(|conf| {
        conf.experimental
            .allow_enable_foreign_key_check_in_shared_lock = false
    });
    let (mut vars, global) = session();
    let variable = sysvar(name);
    assert!(!vars.ForeignKeyCheckInSharedLock);
    for input in ["ON", "1"] {
        let session_result = vars.SetSystemVar(name, input);
        let global_result = set_global_system_var(&mut vars, name, input);
        if kerneltype::IsNextGen() {
            for error in [session_result.unwrap_err(), global_result.unwrap_err()] {
                assert_eq!(error.kind(), VariableErrorKind::WrongValue);
                assert_eq!(error, VariableError::wrong_value(name, input));
            }
            assert!(!vars.ForeignKeyCheckInSharedLock);
            assert_eq!(global.get(name).unwrap(), "OFF");
            assert_eq!(
                vars.GetSessionOrGlobalSystemVar(&Context, name).unwrap(),
                "OFF"
            );
        } else {
            session_result.unwrap();
            global_result.unwrap();
            assert!(vars.ForeignKeyCheckInSharedLock);
            assert_eq!(
                vars.GetSessionOrGlobalSystemVar(&Context, name).unwrap(),
                "ON"
            );
            assert_eq!(
                variable.GetGlobalFromHook(&Context, &mut vars).unwrap(),
                "ON"
            );
        }
        vars.SetSystemVar(name, "OFF").unwrap();
        set_global_system_var(&mut vars, name, "OFF").unwrap();
        assert!(!vars.ForeignKeyCheckInSharedLock);
    }
    vars.GlobalVarsAccessor
        .set_global_sys_var_only(&Context, name, "ON", true)
        .unwrap();
    assert_eq!(
        variable.GetGlobalFromHook(&Context, &mut vars).unwrap(),
        "ON"
    );
    // Explicit session OFF wins over persisted global ON.
    assert_eq!(
        vars.GetSessionOrGlobalSystemVar(&Context, name).unwrap(),
        "OFF"
    );
    let mut fallback = SessionVars::new(Box::new(global.clone()));
    assert_eq!(
        fallback
            .GetSessionOrGlobalSystemVar(&Context, name)
            .unwrap(),
        "ON"
    );
    assert!(!fallback.ForeignKeyCheckInSharedLock);
    fallback
        .SetSystemVarWithRelaxedValidation(name, "ON")
        .unwrap();
    assert!(fallback.ForeignKeyCheckInSharedLock);
    assert_eq!(
        fallback
            .GetSessionOrGlobalSystemVar(&Context, name)
            .unwrap(),
        "ON"
    );
    config::update_global(|conf| {
        conf.experimental
            .allow_enable_foreign_key_check_in_shared_lock = true
    });
    let (mut enabled, accessor) = session();
    for input in ["ON", "1"] {
        enabled.SetSystemVar(name, input).unwrap();
        assert!(enabled.ForeignKeyCheckInSharedLock);
        assert_eq!(
            enabled.GetSessionOrGlobalSystemVar(&Context, name).unwrap(),
            "ON"
        );
        set_global_system_var(&mut enabled, name, input).unwrap();
        assert_eq!(
            variable.GetGlobalFromHook(&Context, &mut enabled).unwrap(),
            "ON"
        );
        enabled.SetSystemVar(name, "OFF").unwrap();
        set_global_system_var(&mut enabled, name, "OFF").unwrap();
    }
    enabled
        .GlobalVarsAccessor
        .set_global_sys_var_only(&Context, name, "ON", true)
        .unwrap();
    let mut initialized = SessionVars::new(Box::new(accessor));
    let persisted = variable
        .GetGlobalFromHook(&Context, &mut initialized)
        .unwrap();
    initialized
        .SetSystemVarWithRelaxedValidation(name, &persisted)
        .unwrap();
    assert!(initialized.ForeignKeyCheckInSharedLock);
    assert_eq!(
        initialized
            .GetSessionOrGlobalSystemVar(&Context, name)
            .unwrap(),
        "ON"
    );
    let mut unavailable = SessionVars::new(Box::new(MemoryGlobal::default()));
    assert_eq!(
        variable.GetGlobal.as_ref().unwrap()(&Context, &mut unavailable)
            .unwrap_err()
            .kind(),
        VariableErrorKind::UnknownSystemVariable
    );
    assert_eq!(
        variable.GetSession.as_ref().unwrap()(&mut unavailable)
            .unwrap_err()
            .kind(),
        VariableErrorKind::UnknownSystemVariable
    );
}

#[test]
fn analyze_store_batch_size_registration_and_validation() {
    let (mut vars, _) = session();
    assert_eq!(
        vars.AnalyzeStoreBatchSize,
        vardef::DefTiDBAnalyzeStoreBatchSize
    );
    let variable =
        GetSysVar("tidb_analyze_store_batch_size").expect("Analyze batching variable registered");
    assert!(variable.HasGlobalScope());
    assert!(variable.HasSessionScope());
    assert_eq!(variable.Value, "4");
    assert_eq!(variable.Type, vardef::TypeUnsigned);
    for scope in [vardef::ScopeSession, vardef::ScopeGlobal] {
        for (input, expected) in [("0", "0"), ("4", "4"), ("8", "8"), ("9", "8"), ("-1", "0")] {
            assert_eq!(
                variable.Validate(&mut vars, input, scope).unwrap(),
                expected
            );
        }
        assert!(variable.Validate(&mut vars, "invalid", scope).is_err());
    }
    vars.SetSystemVar("tidb_analyze_store_batch_size", "0")
        .unwrap();
    assert_eq!(vars.system("tidb_analyze_store_batch_size"), Some("0"));
    vars.SetSystemVar("tidb_analyze_store_batch_size", "9")
        .unwrap();
    assert_eq!(vars.system("tidb_analyze_store_batch_size"), Some("8"));
}

#[test]
fn analyze_store_batch_size_updates_both_session_paths() {
    let (mut vars, _) = session();
    let mut runtime = crate::session::SessionVars::new();
    assert_eq!(
        runtime.AnalyzeStoreBatchSize,
        vardef::DefTiDBAnalyzeStoreBatchSize
    );
    for (input, expected) in [("0", 0), ("4", 4), ("9", 8)] {
        vars.SetSystemVar(vardef::TiDBAnalyzeStoreBatchSize, input)
            .unwrap();
        runtime
            .SetSystemVar(vardef::TiDBAnalyzeStoreBatchSize, input)
            .unwrap();
        assert_eq!(vars.AnalyzeStoreBatchSize, expected);
        assert_eq!(runtime.AnalyzeStoreBatchSize, expected);
        assert_eq!(
            runtime.GetSystemVar(vardef::TiDBAnalyzeStoreBatchSize),
            Some(expected.to_string())
        );
    }
    assert!(
        vars.SetSystemVar(vardef::TiDBAnalyzeStoreBatchSize, "invalid")
            .is_err()
    );
    assert!(
        runtime
            .SetSystemVar(vardef::TiDBAnalyzeStoreBatchSize, "invalid")
            .is_err()
    );
    assert_eq!(vars.AnalyzeStoreBatchSize, 8);
    assert_eq!(runtime.AnalyzeStoreBatchSize, 8);
}

#[test]
#[serial]
fn ttl_required_session_variables_match_go_defaults_bounds_and_retry_hook() {
    let (mut vars, _) = session();
    let retry = sysvar(vardef::TiDBRetryLimit);
    assert_eq!(retry.Value, vardef::DefTiDBRetryLimit.to_string());
    assert_eq!(retry.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(retry.MinValue, -1);
    assert_eq!(retry.MaxValue, i64::MAX as u64);
    let value = retry
        .Validate(&mut vars, "0", vardef::ScopeSession)
        .unwrap();
    retry.SetSessionFromHook(&mut vars, &value).unwrap();
    assert_eq!(vars.system(vardef::TiDBRetryLimit), Some("0"));
    assert_eq!(
        retry
            .Validate(&mut vars, "-2", vardef::ScopeSession)
            .unwrap(),
        "-1"
    );
    let scan = sysvar(vardef::TiDBDistSQLScanConcurrency);
    assert_eq!(scan.Value, vardef::DefDistSQLScanConcurrency.to_string());
    assert_eq!(scan.Type, vardef::TypeUnsigned);
    assert_eq!(scan.MinValue, 1);
    assert_eq!(scan.MaxValue, vardef::MaxConfigurableConcurrency as u64);
    assert_eq!(
        scan.Validate(&mut vars, "0", vardef::ScopeSession).unwrap(),
        "1"
    );
    let value = scan.Validate(&mut vars, "1", vardef::ScopeSession).unwrap();
    scan.SetSessionFromHook(&mut vars, &value).unwrap();
    assert_eq!(vars.system(vardef::TiDBDistSQLScanConcurrency), Some("1"));
}

#[test]
#[serial]
fn query_cop_store_limit_matches_go_defaults_bounds_and_session_hook() {
    let (mut vars, _) = session();
    let limiter = sysvar(vardef::TiDBQueryCopStoreLimit);
    assert_eq!(limiter.Value, vardef::DefTiDBQueryCopStoreLimit.to_string());
    assert_eq!(limiter.Scope, vardef::ScopeGlobal | vardef::ScopeSession);
    assert_eq!(limiter.Type, vardef::TypeUnsigned);
    assert_eq!(limiter.MinValue, 0);
    assert_eq!(limiter.MaxValue, vardef::MaxConfigurableConcurrency as u64);
    assert!(limiter.IsHintUpdatableVerified);

    let value = limiter
        .Validate(&mut vars, "0", vardef::ScopeSession)
        .unwrap();
    limiter.SetSessionFromHook(&mut vars, &value).unwrap();
    assert_eq!(vars.system(vardef::TiDBQueryCopStoreLimit), Some("0"));
    assert_eq!(vars.QueryCopStoreLimit, 0);

    let value = limiter
        .Validate(&mut vars, "17", vardef::ScopeSession)
        .unwrap();
    limiter.SetSessionFromHook(&mut vars, &value).unwrap();
    assert_eq!(vars.system(vardef::TiDBQueryCopStoreLimit), Some("17"));
    assert_eq!(vars.QueryCopStoreLimit, 17);
}
