// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// SEM（Security Enhanced Mode，安全增强模式）兼容层单元测试。
//
// 由 `compat_test.go` 迁移：在 SEM v2 下断言 schema/table、status/sys var
// 与 privilege 的可见性/受限规则。

// 本文件由 pkg/util/sem/compat/compat_test.go 迁移而来，保留 SEM 兼容层测试结构。
// 覆盖 SEM v2 下 schema、table、status/sys var 和 privilege 可见性断言。

use super::*;
use astersql_sessionctx_vardef as vardef;
use astersql_sessionctx_variable as variable;
use serial_test::serial;

// 下列常量保持 Go const 块顺序，用于构造隐藏对象和受限权限测试输入。
const METRICS_SCHEMA: &str = "metrics_schema";
const EXPR_PUSHDOWN_BLACKLIST: &str = "expr_pushdown_blacklist";
const GC_DELETE_RANGE: &str = "gc_delete_range";
const GC_DELETE_RANGE_DONE: &str = "gc_delete_range_done";
const OPT_RULE_BLACKLIST: &str = "opt_rule_blacklist";
const TIDB: &str = "tidb";
const GLOBAL_VARIABLES: &str = "global_variables";
const INFORMATION_SCHEMA: &str = "information_schema";
const CLUSTER_CONFIG: &str = "cluster_config";
const CLUSTER_HARDWARE: &str = "cluster_hardware";
const CLUSTER_LOAD: &str = "cluster_load";
const CLUSTER_LOG: &str = "cluster_log";
const CLUSTER_SYSTEM_INFO: &str = "cluster_systeminfo";
const INSPECTION_RESULT: &str = "inspection_result";
const INSPECTION_RULES: &str = "inspection_rules";
const INSPECTION_SUMMARY: &str = "inspection_summary";
const METRICS_SUMMARY: &str = "metrics_summary";
const METRICS_SUMMARY_BY_LABEL: &str = "metrics_summary_by_label";
const METRICS_TABLES: &str = "metrics_tables";
const TIDB_HOT_REGIONS: &str = "tidb_hot_regions";
const PERFORMANCE_SCHEMA: &str = "performance_schema";
const PD_PROFILE_ALLOCS: &str = "pd_profile_allocs";
const PD_PROFILE_BLOCK: &str = "pd_profile_block";
const PD_PROFILE_CPU: &str = "pd_profile_cpu";
const PD_PROFILE_GOROUTINES: &str = "pd_profile_goroutines";
const PD_PROFILE_MEMORY: &str = "pd_profile_memory";
const PD_PROFILE_MUTEX: &str = "pd_profile_mutex";
const TIDB_PROFILE_ALLOCS: &str = "tidb_profile_allocs";
const TIDB_PROFILE_BLOCK: &str = "tidb_profile_block";
const TIDB_PROFILE_CPU: &str = "tidb_profile_cpu";
const TIDB_PROFILE_GOROUTINES: &str = "tidb_profile_goroutines";
const TIDB_PROFILE_MEMORY: &str = "tidb_profile_memory";
const TIDB_PROFILE_MUTEX: &str = "tidb_profile_mutex";
const TIKV_PROFILE_CPU: &str = "tikv_profile_cpu";
const TIDB_GC_LEADER_DESC: &str = "tidb_gc_leader_desc";

/// RAII 清理守卫：离开作用域时执行可选回调（对齐 Go defer）。
struct CleanupGuard(Option<Box<dyn Fn()>>);

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if let Some(cleanup) = self.0.take() {
            cleanup();
        }
    }
}

/// 构造在 Drop 时执行 cleanup 的守卫。
fn defer_cleanup(cleanup: Box<dyn Fn()>) -> CleanupGuard {
    CleanupGuard(Some(cleanup))
}

/// 重置 SEM 相关系统变量并关闭 v1/v2 SEM，避免用例互相污染。
fn reset_sem() {
    for (name, value) in [
        (vardef::Hostname, vardef::DefHostname),
        (vardef::TiDBEnableEnhancedSecurity, vardef::Off),
    ] {
        if variable::GetSysVar(name).is_none() {
            variable::RegisterSysVar(variable::SysVar {
                Name: name.to_owned(),
                Value: value.to_owned(),
                Scope: vardef::ScopeNone,
                ..Default::default()
            });
        }
    }
    astersql_util_sem::Disable();
    astersql_util_sem_v2::Disable();
}

/// 按兼容 SEM v2 配置注册受限系统变量，供后续可见性断言使用。
fn register_v2_config_sysvars() {
    let config: serde_json::Value = serde_json::from_str(compatibleSEMV2Config).unwrap();
    for restriction in config["restricted_variables"].as_array().unwrap() {
        let name = restriction["name"].as_str().unwrap();
        variable::UnregisterSysVar(name);
        variable::RegisterSysVar(variable::SysVar {
            Name: name.to_owned(),
            Value: "default".to_owned(),
            Scope: if restriction["value"].as_str().unwrap_or_default().is_empty() {
                vardef::ScopeGlobal
            } else {
                vardef::ScopeNone
            },
            ..Default::default()
        });
    }
}

/// 启用 SEM v2 并返回退出时恢复原状态的清理守卫。
fn enable_sem_v2_for_test() -> CleanupGuard {
    reset_sem();
    register_v2_config_sysvars();
    defer_cleanup(SwitchToSEMForTest(V2))
}

// TestInvisibleSchema 对应 Go 测试：切到 SEM v2 后 metrics_schema 应隐藏，普通库不隐藏。
#[test]
#[serial]
fn test_invisible_schema() {
    let _cleanup = enable_sem_v2_for_test();

    assert!(IsInvisibleSchema(METRICS_SCHEMA));
    assert!(IsInvisibleSchema("METRICS_ScHEma"));
    assert!(!IsInvisibleSchema("mysql"));
    assert!(!IsInvisibleSchema(INFORMATION_SCHEMA));
    assert!(!IsInvisibleSchema("Bogusname"));
}

// TestIsInvisibleTable 对应 Go 测试：按库分别验证 SEM v2 隐藏表清单。
#[test]
#[serial]
fn test_is_invisible_table() {
    let _cleanup = enable_sem_v2_for_test();

    let mysql_tbls = [
        EXPR_PUSHDOWN_BLACKLIST,
        GC_DELETE_RANGE,
        GC_DELETE_RANGE_DONE,
        OPT_RULE_BLACKLIST,
        TIDB,
        GLOBAL_VARIABLES,
    ];
    let info_schema_tbls = [
        CLUSTER_CONFIG,
        CLUSTER_HARDWARE,
        CLUSTER_LOAD,
        CLUSTER_LOG,
        CLUSTER_SYSTEM_INFO,
        INSPECTION_RESULT,
        INSPECTION_RULES,
        INSPECTION_SUMMARY,
        METRICS_SUMMARY,
        METRICS_SUMMARY_BY_LABEL,
        METRICS_TABLES,
        TIDB_HOT_REGIONS,
    ];
    let perf_schema_tbls = [
        PD_PROFILE_ALLOCS,
        PD_PROFILE_BLOCK,
        PD_PROFILE_CPU,
        PD_PROFILE_GOROUTINES,
        PD_PROFILE_MEMORY,
        PD_PROFILE_MUTEX,
        TIDB_PROFILE_ALLOCS,
        TIDB_PROFILE_BLOCK,
        TIDB_PROFILE_CPU,
        TIDB_PROFILE_GOROUTINES,
        TIDB_PROFILE_MEMORY,
        TIDB_PROFILE_MUTEX,
        TIKV_PROFILE_CPU,
    ];

    // 三段循环分别对应 Go 中 mysql、information_schema、performance_schema 的隐藏表断言。
    for tbl in mysql_tbls {
        assert!(IsInvisibleTable(mysql::r#const::SystemDB, tbl));
    }
    for tbl in info_schema_tbls {
        assert!(IsInvisibleTable(INFORMATION_SCHEMA, tbl));
    }
    for tbl in perf_schema_tbls {
        assert!(IsInvisibleTable(PERFORMANCE_SCHEMA, tbl));
    }

    // metrics_schema 在 SEM v2 下整体隐藏，普通 test.t1 不隐藏。
    assert!(IsInvisibleTable(METRICS_SCHEMA, "acdc"));
    assert!(IsInvisibleTable(METRICS_SCHEMA, "fdsgfd"));
    assert!(!IsInvisibleTable("test", "t1"));
}

// TestIsRestrictedPrivilege 对应 Go 测试：验证 RESTRICTED_ 前缀权限和额外 BACKUP_ADMIN 规则。
#[test]
#[serial]
fn test_is_restricted_privilege() {
    let _cleanup = enable_sem_v2_for_test();

    assert!(IsRestrictedPrivilege("RESTRICTED_TABLES_ADMIN"));
    assert!(IsRestrictedPrivilege("RESTRICTED_STATUS_VARIABLES_ADMIN"));
    assert!(IsRestrictedPrivilege("BACKUP_ADMIN"));
    assert!(!IsRestrictedPrivilege("CONNECTION_ADMIN"));
    assert!(!IsRestrictedPrivilege("AA"));
}

// TestIsInvisibleStatusVar 对应 Go 测试：只隐藏 tidb_gc_leader_desc。
#[test]
#[serial]
fn test_is_invisible_status_var() {
    let _cleanup = enable_sem_v2_for_test();

    assert!(IsInvisibleStatusVar(TIDB_GC_LEADER_DESC));
    assert!(!IsInvisibleStatusVar("server_id"));
    assert!(!IsInvisibleStatusVar("ddl_schema_version"));
    assert!(!IsInvisibleStatusVar("Ssl_version"));
}

// TestIsInvisibleSysVar 对应 Go 测试：先确认三个可见变量，再枚举 SEM v2 需要隐藏的系统变量。
#[test]
#[serial]
fn test_is_invisible_sys_var() {
    let _cleanup = enable_sem_v2_for_test();

    assert!(!IsInvisibleSysVar(vardef::Hostname)); // Go 注释：值会变为默认值，但变量本身不隐藏。
    assert!(!IsInvisibleSysVar(vardef::TiDBEnableEnhancedSecurity)); // Go 注释：用户应能看到 SEM 已开启。
    assert!(!IsInvisibleSysVar(vardef::TiDBAllowRemoveAutoInc));

    let hidden_vars = [
        vardef::TiDBCheckMb4ValueInUTF8,
        vardef::TiDBConfig,
        vardef::TiDBEnableSlowLog,
        vardef::TiDBExpensiveQueryTimeThreshold,
        vardef::TiDBForcePriority,
        vardef::TiDBGeneralLog,
        vardef::TiDBMetricSchemaRangeDuration,
        vardef::TiDBMetricSchemaStep,
        vardef::TiDBOptWriteRowID,
        vardef::TiDBPProfSQLCPU,
        vardef::TiDBRecordPlanInSlowLog,
        vardef::TiDBSlowQueryFile,
        vardef::TiDBSlowLogThreshold,
        vardef::TiDBEnableCollectExecutionInfo,
        vardef::TiDBMemoryUsageAlarmRatio,
        vardef::TiDBEnableTelemetry,
        vardef::TiDBRowFormatVersion,
        vardef::TiDBRedactLog,
        // Go compat_test.go 同样重复断言 TiDBTopSQLMaxTimeSeriesCount 两次。
        vardef::TiDBTopSQLMaxTimeSeriesCount,
        vardef::TiDBTopSQLMaxTimeSeriesCount,
    ];
    for var_name in hidden_vars {
        assert!(IsInvisibleSysVar(var_name));
    }
}
