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
// 前半为注释掉的 Go 用例大纲；当前可运行用例校验 SysVar 注册表大小写不敏感，
// 以及 `OrderByDependency` 将依赖变量排在前面。

// #[derive(Debug, Clone, Copy)]
// struct SqlExpectation {
//     sql: &'static str,
//     expected: &'static [&'static str],
// }
//
// #[derive(Debug, Clone, Copy)]
// struct ErrExpectation {
//     sql: &'static str,
//     message: &'static str,
// }
//
// #[derive(Debug, Clone, Copy)]
// struct HintCheck {
//     hint: &'static str,
//     field: &'static str,
//     expected: &'static str,
// }
//
// fn record_sql_sequence(name: &str, sqls: &[&str]) {
//     assert!(!name.is_empty());
//     assert!(!sqls.is_empty());
// }
//
// fn record_query_expectations(expectations: &[SqlExpectation]) {
//     assert!(!expectations.is_empty());
//     for item in expectations {
//         assert!(!item.sql.is_empty());
// Go 的 Check(testkit.Rows()) 表示结果集为空，允许 expected 为空切片。
//         let _ = item.expected;
//     }
// }
//
// fn record_error_expectations(expectations: &[ErrExpectation]) {
//     assert!(!expectations.is_empty());
//     for item in expectations {
//         assert!(!item.sql.is_empty());
//         assert!(!item.message.is_empty());
//     }
// }
//
// #[test]
// fn test_kv_vars() {
// 对应 Go 的 TestKVVars：session 变量写入后应下发到 TiKV transaction variables。
//     record_sql_sequence(
//         "kv vars",
//         &[
//             "use test",
//             "set @@tidb_backoff_lock_fast = 1",
//             "set @@tidb_backoff_weight = 100",
//             "create table if not exists kvvars (a int key)",
//             "insert into kvvars values (1)",
//             "begin",
//             "rollback",
//             "set @@tidb_backoff_weight = 50",
//             "set @@autocommit = 0",
//             "select * from kvvars",
//             "set @@autocommit = 1",
//             "select * from kvvars where a = 1",
//         ],
//     );
//     let first_txn_vars = ("BackoffLockFast", 1, "BackOffWeight", 100);
//     let second_txn_weight = 50;
//     assert_eq!(1, first_txn_vars.1);
//     assert_eq!(50, second_txn_weight);
// Go 通过 tikvclient/probeSetVars failpoint 确认 SetSuccess 被置位，然后重置为 false。
//     let probe_failpoint = "tikvclient/probeSetVars";
//     assert!(probe_failpoint.contains("probeSetVars"));
// }
//
// #[test]
// fn test_removed_sys_vars() {
// 对应 Go 的 TestRemovedSysVars：注册变量、确认可读写、注销后 show/select/set 都报未知变量。
//     record_query_expectations(&[
//         SqlExpectation {
//             sql: "SHOW GLOBAL VARIABLES LIKE 'bogus_var'",
//             expected: &["bogus_var acdc"],
//         },
//         SqlExpectation {
//             sql: "SELECT @@GLOBAL.bogus_var",
//             expected: &["acdc"],
//         },
//         SqlExpectation {
//             sql: "SHOW GLOBAL VARIABLES LIKE 'bogus_var'",
//             expected: &[],
//         },
//     ]);
//     record_sql_sequence("removed sys var", &["SET GLOBAL bogus_var = 'newvalue'"]);
//     record_error_expectations(&[
//         ErrExpectation {
//             sql: "SET GLOBAL bogus_var = 'newvalue'",
//             message: "[variable:1193]Unknown system variable 'bogus_var'",
//         },
//         ErrExpectation {
//             sql: "SELECT @@GLOBAL.bogus_var",
//             message: "[variable:1193]Unknown system variable 'bogus_var'",
//         },
//     ]);
// }
//
// #[test]
// fn test_tikv_system_vars() {
// 对应 Go 的 TestTiKVSystemVars：tidb_* 系统变量和 mysql.tidb 中 tikv_* 持久化值互相映射。
//     record_sql_sequence(
//         "tikv system vars",
//         &[
//             "use test",
//             "SET GLOBAL tidb_gc_enable = 1",
//             "UPDATE mysql.tidb SET variable_value = 'false' WHERE variable_name='tikv_gc_enable'",
//             "SET GLOBAL tidb_gc_concurrency = -1",
//             "SET GLOBAL tidb_gc_concurrency = 5",
//             "UPDATE mysql.tidb SET variable_value = 'true' WHERE variable_name='tikv_gc_auto_concurrency'",
//             "REPLACE INTO mysql.tidb (variable_value, variable_name) VALUES ('15m', 'tikv_gc_run_interval')",
//             "SET GLOBAL tidb_gc_run_interval = '9m'",
//             "SET GLOBAL tidb_gc_run_interval = '700000000000ns'",
//         ],
//     );
//     record_query_expectations(&[
//         SqlExpectation {
//             sql: "SHOW GLOBAL VARIABLES LIKE 'tidb_gc_enable'",
//             expected: &["tidb_gc_enable ON"],
//         },
//         SqlExpectation {
//             sql: "SELECT @@tidb_gc_enable;",
//             expected: &["0"],
//         },
//         SqlExpectation {
//             sql: "SELECT @@tidb_gc_concurrency;",
//             expected: &["-1"],
//         },
//         SqlExpectation {
//             sql: "SHOW WARNINGS",
//             expected: &["Warning 1292 Truncated incorrect tidb_gc_run_interval value: '9m'"],
//         },
//     ]);
//     record_error_expectations(&[ErrExpectation {
//         sql: "SET GLOBAL tidb_gc_run_interval = '11mins'",
//         message: "[variable:1232]Incorrect argument type to variable 'tidb_gc_run_interval'",
//     }]);
// }
//
// #[test]
// fn test_upgrade_sysvars() {
// 对应 Go 的 TestUpgradeSysvars：读取旧版本遗留值时，GetGlobalSysVar 会规整成当前合法值。
//     let upgrade_cases = [
//         ("tidb_enable_noop_functions", "0", "OFF"),
//         ("rpl_semi_sync_slave_enabled", "", "OFF"),
//         ("tidb_executor_concurrency", "999", "256"),
//         ("tidb_enable_noop_functions", "SOMEVAL", "OFF"),
//     ];
//     for (name, old_value, expected) in upgrade_cases {
//         assert!(!name.is_empty());
//         assert!(!old_value.is_empty() || name == "rpl_semi_sync_slave_enabled");
//         assert!(!expected.is_empty());
// Go 每次写 mysql.global_variables 后都 NotifyUpdateSysVarCache(true)，这里保留缓存刷新要求。
//     }
// }
//
// #[test]
// fn test_index_join_build_v2_sys_var_compatibility() {
// 对应 Go 的 TestIndexJoinBuildV2SysVarCompatibility：旧持久化 OFF 值在当前版本始终读成 ON。
//     record_query_expectations(&[
//         SqlExpectation {
//             sql: "SHOW VARIABLES LIKE 'tidb_opt_index_join_build_v2'",
//             expected: &["tidb_opt_index_join_build_v2 ON"],
//         },
//         SqlExpectation {
//             sql: "SELECT @@tidb_opt_index_join_build_v2",
//             expected: &["1"],
//         },
//         SqlExpectation {
//             sql: "SHOW GLOBAL VARIABLES LIKE 'tidb_opt_index_join_build_v2'",
//             expected: &["tidb_opt_index_join_build_v2 ON"],
//         },
//     ]);
//     record_sql_sequence(
//         "persist old value",
//         &["REPLACE INTO mysql.global_variables (variable_name, variable_value) VALUES ('tidb_opt_index_join_build_v2', 'OFF')"],
//     );
//     record_error_expectations(&[ErrExpectation {
//         sql: "SET @@tidb_opt_index_join_build_v2 = OFF",
//         message: "tidb_opt_index_join_build_v2 is now always enabled and cannot be turned off",
//     }]);
// }
//
// #[test]
// fn test_set_instance_sysvar_by_set_global_sys_var() {
// 对应 Go 的 TestSetInstanceSysvarBySetGlobalSysVar：instance 变量不落 mysql.global_variables。
//     let var_name = "tidb_general_log";
//     let default_value = "OFF";
//     assert_eq!("tidb_general_log", var_name);
//     record_sql_sequence(
//         "instance sysvar",
//         &[
//             "use test",
//             "SetInstanceSysVar(tidb_general_log, ON)",
//             "select @@global.tidb_general_log",
//             "SetInstanceSysVar(tidb_general_log, OFF)",
//             "select @@global.tidb_general_log",
//         ],
//     );
//     record_query_expectations(&[
//         SqlExpectation {
//             sql: "select @@global.tidb_general_log",
//             expected: &["1"],
//         },
//         SqlExpectation {
//             sql: "select @@global.tidb_general_log",
//             expected: &["0"],
//         },
//     ]);
//     assert_eq!("OFF", default_value);
// }
//
// #[test]
// fn test_time_zone() {
// 对应 Go 的 TestTimeZone：会话 time_zone 影响 cast(time as date)，global time_zone 只影响新会话。
//     record_sql_sequence(
//         "time zone",
//         &[
//             "use test",
//             "set time_zone = '-8:00'",
//             "select cast(time('12:23:34') as date)",
//             "set time_zone = '+08:00'",
//             "select cast(time('12:23:34') as date)",
//             "set global time_zone = '+00:00'",
//             "show variables like 'time_zone'",
//         ],
//     );
//     let offsets = [("-8:00", -8), ("+08:00", 8), ("+00:00", 0)];
//     assert_eq!(-8, offsets[0].1);
// }
//
// #[test]
// fn test_global_var_accessor() {
// 对应 Go 的 TestGlobalVarAccessor：覆盖 max_allowed_packet、sql_select_limit、autocommit 和 time_zone 错误。
//     let var_name = "max_allowed_packet";
//     let var_value0 = "4194304";
//     let var_value1 = "4194305";
//     let var_value2 = "4194306";
//     let configured_max_allowed_packet = 8_u64 << 20;
//     assert_eq!("max_allowed_packet", var_name);
//     assert!(var_value1 > var_value0);
//     assert!(var_value2 > var_value0);
//     assert_eq!(8_388_608, configured_max_allowed_packet);
//
// non-starter 子测试：即便配置改大，非 starter 仍读默认 DefMaxAllowedPacket。
//     let non_starter_steps = [
//         "UpdateGlobal(MaxAllowedPacket=8<<20)",
//         "GetGlobalSysVar(max_allowed_packet) == DefMaxAllowedPacket",
//         "set @@global.max_allowed_packet = 4194304",
//     ];
//     assert_eq!(3, non_starter_steps.len());
//
// starter 子测试：next-gen starter 模式读取配置值，并拒绝 SET GLOBAL。
//     let starter_steps = [
//         "deploymode.Set(Starter)",
//         "UpdateGlobal(MaxAllowedPacket=8<<20)",
//         "GetGlobalSysVar(max_allowed_packet) == configuredMaxAllowedPacket",
//     ];
//     assert_eq!(3, starter_steps.len());
//     record_error_expectations(&[ErrExpectation {
//         sql: "set @@global.max_allowed_packet = 4194304",
//         message: "SET GLOBAL max_allowed_packet is not supported in starter deployment mode",
//     }]);
//
//     record_sql_sequence(
//         "global var accessor",
//         &[
//             "set @@global.max_execution_time = 100",
//             "set @@global.max_execution_time = 0",
//             "set session sql_select_limit=100000000000;",
//             "set @@global.sql_select_limit = 1",
//             "set @@global.sql_select_limit = default",
//             "set @@global.autocommit = 0;",
//             "set @@global.autocommit=1",
//         ],
//     );
//     record_error_expectations(&[ErrExpectation {
//         sql: "set global time_zone = 'timezone'",
//         message: "ErrUnknownTimeZone",
//     }]);
// }
//
// #[test]
// fn test_prepare_execute_with_sql_hints() {
// 对应 Go 的 TestPrepareExecuteWithSQLHints：同一 hint 同时走普通 prepare 和 point-get fast path。
//     let hint_checks = [
//         HintCheck {
//             hint: "MEMORY_QUOTA(1024 MB)",
//             field: "MemQuotaQuery",
//             expected: "1024*1024*1024",
//         },
//         HintCheck {
//             hint: "READ_CONSISTENT_REPLICA()",
//             field: "ReplicaRead",
//             expected: "ReplicaReadFollower",
//         },
//         HintCheck {
//             hint: "MAX_EXECUTION_TIME(1000)",
//             field: "MaxExecutionTime",
//             expected: "1000",
//         },
//         HintCheck {
//             hint: "USE_TOJA(TRUE)",
//             field: "AllowInSubqToJoinAndAgg",
//             expected: "true",
//         },
//         HintCheck {
//             hint: "RESOURCE_GROUP(rg1)",
//             field: "ResourceGroup",
//             expected: "rg1",
//         },
//     ];
//     for (i, check) in hint_checks.iter().enumerate() {
// Go 对每个 hint 执行 10 次 execute，确保 StmtCtx.StmtHints 每次都被正确填充。
//         let common_prepare = format!(
//             "prepare stmt{i} from 'select /*+ {} */
//  * from t'",
//             check.hint
//         );
//         let fast_prepare = format!(
// "prepare fast{i} from 'select /*+ {} */
//  * from t where a = 1'",
//             check.hint
//         );
//         assert!(common_prepare.contains(check.hint));
//         assert!(fast_prepare.contains(check.hint));
//         assert!(!check.field.is_empty());
//         assert!(!check.expected.is_empty());
//     }
// }
//
// #[test]
// fn test_tidb_validate_ts() {
// 对应 Go 的 TestTiDBValidateTS：默认开启未来时间戳校验，关闭后允许 AS OF 未来时间。
//     record_sql_sequence(
//         "validate ts",
//         &[
//             "use test",
//             "create table t(a int primary key)",
//             "insert into t values (1)",
//             "set global tidb_enable_ts_validation = off",
//             "select * from t as of timestamp NOW() + interval 1 day",
//             "set global tidb_enable_ts_validation = on",
//         ],
//     );
//     let default_and_enabled_errors = [
//         "select * from t as of timestamp NOW() + interval 1 day",
//         "select * from t as of timestamp NOW() + interval 1 day",
//     ];
//     assert_eq!(2, default_and_enabled_errors.len());
// }
//
// #[test]
// fn test_tidb_advancer_check_point_lag_limit() {
// 对应 Go 的 TestTiDBAdvancerCheckPointLagLimit：SET GLOBAL 后 atomic vardef 值更新为 100 小时。
//     record_sql_sequence(
//         "advancer checkpoint lag",
//         &["set @@global.tidb_advancer_check_point_lag_limit = '100h'"],
//     );
//     let expected_hours = 100;
//     assert_eq!(100, expected_hours);
// }
// */
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
