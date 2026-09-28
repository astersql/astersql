// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// SEM v1 迁移单元测试：对照 Go 行为验证开关、隐藏规则与受限权限。
//
// SEM（Security Enhanced Mode）开启后会改写增强安全相关系统变量、隐藏部分
// schema/表/变量，并将 `RESTRICTED_*` 动态权限排除在 SUPER 兜底之外。
// 本文件覆盖 Enable/Disable 副作用及各类 `IsInvisible*` / `IsRestrictedPrivilege` 判定。

use super::{
    Disable, Enable, IsEnabled, IsInvisibleSchema, IsInvisibleStatusVar, IsInvisibleSysVar,
    IsInvisibleTable, IsRestrictedPrivilege,
};
use logutil::log::BgLogger;

/// 补齐 Go `variable` 包 init 中 SEM 依赖的两个默认系统变量。
fn register_go_initialized_sem_sysvars() {
    if variable::GetSysVar(vardef::Hostname).is_none() {
        variable::RegisterSysVar(variable::SysVar {
            Scope: vardef::ScopeNone,
            Name: vardef::Hostname.into(),
            Value: vardef::DefHostname.into(),
            ..variable::SysVar::default()
        });
    }
    if variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity).is_none() {
        variable::RegisterSysVar(variable::SysVar {
            Scope: vardef::ScopeNone,
            Name: vardef::TiDBEnableEnhancedSecurity.into(),
            Value: vardef::Off.into(),
            Type: vardef::TypeStr,
            ..variable::SysVar::default()
        });
    }
}

/// 验证 Enable/Disable 对开关、系统变量、主机名与日志条目的副作用。
#[test]
fn enable_disable_updates_state_sysvars_hostname_and_log() {
    // Go's variable package registers these entries in init(). The dependency
    // crate exposes the registry but has no automatic constructor, so the
    // standalone harness recreates the two exact defaultSysVars entries used by SEM.
    register_go_initialized_sem_sysvars();
    Disable();
    assert!(!IsEnabled());
    assert_eq!(
        variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity)
            .expect("enhanced security sysvar must be registered")
            .Value,
        vardef::Off
    );

    // Enable 应打开增强安全、固定 Hostname，并追加一条 SEM 启用日志。
    let entries_before = BgLogger().entries().len();
    Enable();
    assert!(IsEnabled());
    assert_eq!(
        variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity)
            .expect("enhanced security sysvar must be registered")
            .Value,
        vardef::On
    );
    assert_eq!(
        variable::GetSysVar(vardef::Hostname)
            .expect("hostname sysvar must be registered")
            .Value,
        vardef::DefHostname
    );
    let entries = BgLogger().entries();
    assert_eq!(entries.len(), entries_before + 1);
    assert_eq!(
        entries
            .last()
            .expect("Enable must emit a log entry")
            .message,
        "tidb-server is operating with security enhanced mode (SEM) enabled"
    );

    // Disable 关闭增强安全，并在能读取 OS 主机名时恢复 Hostname。
    Disable();
    assert!(!IsEnabled());
    assert_eq!(
        variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity)
            .expect("enhanced security sysvar must be registered")
            .Value,
        vardef::Off
    );
    if let Ok(hostname) = hostname::get() {
        assert_eq!(
            variable::GetSysVar(vardef::Hostname)
                .expect("hostname sysvar must be registered")
                .Value,
            hostname.to_string_lossy()
        );
    }
}

/// 验证 metrics_schema 隐藏判定支持 ASCII 大小写折叠（对应 Go EqualFold）。
#[test]
fn invisible_schema_matches_go_case_folding() {
    assert!(IsInvisibleSchema("metrics_schema"));
    assert!(IsInvisibleSchema("METRICS_ScHEma"));
    assert!(!IsInvisibleSchema("mysql"));
    assert!(!IsInvisibleSchema("information_schema"));
    assert!(!IsInvisibleSchema("Bogusname"));
}

/// 验证各系统库黑名单表及 metrics_schema 整库隐藏；普通库与非小写库名不命中。
#[test]
fn invisible_tables_match_all_go_blacklists() {
    for table in [
        "expr_pushdown_blacklist",
        "gc_delete_range",
        "gc_delete_range_done",
        "opt_rule_blacklist",
        "tidb",
        "global_variables",
    ] {
        assert!(IsInvisibleTable("mysql", table), "missing mysql.{table}");
    }
    for table in [
        "cluster_config",
        "cluster_hardware",
        "cluster_load",
        "cluster_log",
        "cluster_systeminfo",
        "inspection_result",
        "inspection_rules",
        "inspection_summary",
        "metrics_summary",
        "metrics_summary_by_label",
        "metrics_tables",
        "tidb_hot_regions",
    ] {
        assert!(
            IsInvisibleTable("information_schema", table),
            "missing information_schema.{table}"
        );
    }
    for table in [
        "pd_profile_allocs",
        "pd_profile_block",
        "pd_profile_cpu",
        "pd_profile_goroutines",
        "pd_profile_memory",
        "pd_profile_mutex",
        "tidb_profile_allocs",
        "tidb_profile_block",
        "tidb_profile_cpu",
        "tidb_profile_goroutines",
        "tidb_profile_memory",
        "tidb_profile_mutex",
        "tikv_profile_cpu",
    ] {
        assert!(
            IsInvisibleTable("performance_schema", table),
            "missing performance_schema.{table}"
        );
    }
    assert!(IsInvisibleTable("metrics_schema", "any_metric"));
    assert!(!IsInvisibleTable("test", "t1"));
    // 入参约定为小写库名，大写 MYSQL 不应命中 SystemDB 分支。
    assert!(!IsInvisibleTable("MYSQL", "tidb"));
}

/// 验证 status/system 变量隐藏列表与可见例外（含大小写敏感的变量名）。
#[test]
fn invisible_status_and_system_vars_match_go_lists() {
    assert!(IsInvisibleStatusVar("tidb_gc_leader_desc"));
    for name in ["server_id", "ddl_schema_version", "Ssl_version"] {
        assert!(!IsInvisibleStatusVar(name));
    }

    for name in [
        vardef::TiDBDDLSlowOprThreshold,
        vardef::TiDBCheckMb4ValueInUTF8,
        vardef::TiDBConfig,
        vardef::TiDBEnableSlowLog,
        vardef::TiDBEnableTelemetry,
        vardef::TiDBExpensiveQueryTimeThreshold,
        vardef::TiDBForcePriority,
        vardef::TiDBGeneralLog,
        vardef::TiDBMetricSchemaRangeDuration,
        vardef::TiDBMetricSchemaStep,
        vardef::TiDBOptWriteRowID,
        vardef::TiDBPProfSQLCPU,
        vardef::TiDBRecordPlanInSlowLog,
        vardef::TiDBRowFormatVersion,
        vardef::TiDBSlowQueryFile,
        vardef::TiDBSlowLogThreshold,
        vardef::TiDBEnableCollectExecutionInfo,
        vardef::TiDBMemoryUsageAlarmRatio,
        vardef::TiDBRedactLog,
        vardef::TiDBRestrictedReadOnly,
        vardef::TiDBTopSQLMaxTimeSeriesCount,
        vardef::TiDBTopSQLMaxMetaCount,
        vardef::TiDBServiceScope,
        vardef::TiDBCloudStorageURI,
        vardef::TiDBStmtSummaryMaxStmtCount,
        vardef::TiDBServerMemoryLimit,
        vardef::TiDBServerMemoryLimitGCTrigger,
        vardef::TiDBInstancePlanCacheMaxMemSize,
        vardef::TiDBStatsCacheMemQuota,
        vardef::TiDBMemQuotaBindingCache,
        vardef::TiDBSchemaCacheSize,
        "tidb_audit_redact_log",
    ] {
        assert!(IsInvisibleSysVar(name), "missing sysvar {name}");
    }
    for name in [
        vardef::Hostname,
        vardef::TiDBEnableEnhancedSecurity,
        vardef::TiDBAllowRemoveAutoInc,
        "TIDB_CONFIG",
    ] {
        assert!(!IsInvisibleSysVar(name), "unexpected hidden sysvar {name}");
    }
}

/// 验证受限动态权限需带 `RESTRICTED_` 前缀且长度足够；普通动态权限不命中。
#[test]
fn restricted_privileges_require_prefix_and_suffix() {
    assert!(IsRestrictedPrivilege("RESTRICTED_TABLES_ADMIN"));
    assert!(IsRestrictedPrivilege("RESTRICTED_STATUS_VARIABLES_ADMIN"));
    assert!(!IsRestrictedPrivilege("RESTRICTED_"));
    assert!(!IsRestrictedPrivilege("CONNECTION_ADMIN"));
    assert!(!IsRestrictedPrivilege("BACKUP_ADMIN"));
    assert!(!IsRestrictedPrivilege("AA"));
}
