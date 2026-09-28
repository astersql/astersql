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

// SEM v1 单元测试：对照 Go 用例验证隐藏规则与受限权限判定。
//
// 覆盖 schema/表/status 变量/system 变量的不可见列表，以及 `RESTRICTED_*`
// 动态权限前缀规则；入参大小写约定与 Go 测试保持一致。

use super::{
    IsInvisibleSchema, IsInvisibleStatusVar, IsInvisibleSysVar, IsInvisibleTable,
    IsRestrictedPrivilege,
};
use parser_mysql::r#const as mysql;

/// 验证仅 metrics_schema（大小写不敏感）被判定为不可见 schema。
#[test]
fn test_invisible_schema() {
    assert!(IsInvisibleSchema(metadef::MetricSchemaName.L.as_str()));
    assert!(IsInvisibleSchema("METRICS_ScHEma"));
    // Go strings.EqualFold uses Unicode simple folding: long-s belongs to
    // the same fold class as ASCII S/s.
    assert!(IsInvisibleSchema("metric\u{17f}_schema"));
    // Unicode uppercasing maps dotless i to ASCII I, but Go strings.EqualFold
    // does not place dotless i in the ASCII I/i simple-fold orbit.
    assert!(!IsInvisibleSchema("metr\u{131}cs_schema"));
    assert!(!IsInvisibleSchema("mysql"));
    assert!(!IsInvisibleSchema(
        metadef::InformationSchemaName.L.as_str()
    ));
    assert!(!IsInvisibleSchema("Bogusname"));
}

/// 验证 mysql / information_schema / performance_schema / metrics_schema 表隐藏规则。
#[test]
fn test_is_invisible_table() {
    let mysql_tbls = [
        "expr_pushdown_blacklist",
        "gc_delete_range",
        "gc_delete_range_done",
        "opt_rule_blacklist",
        "tidb",
        "global_variables",
    ];
    let info_schema_tbls = [
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
    ];
    let perf_schema_tbls = [
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
    ];

    for table in mysql_tbls {
        assert!(
            IsInvisibleTable(mysql::SystemDB, table),
            "missing mysql.{table}"
        );
    }
    for table in info_schema_tbls {
        assert!(
            IsInvisibleTable(metadef::InformationSchemaName.L.as_str(), table),
            "missing information_schema.{table}"
        );
    }
    for table in perf_schema_tbls {
        assert!(
            IsInvisibleTable(metadef::PerformanceSchemaName.L.as_str(), table),
            "missing performance_schema.{table}"
        );
    }

    // metrics_schema 下任意表名均隐藏；普通用户表可见。
    assert!(IsInvisibleTable(
        metadef::MetricSchemaName.L.as_str(),
        "acdc"
    ));
    assert!(IsInvisibleTable(
        metadef::MetricSchemaName.L.as_str(),
        "fdsgfd"
    ));
    assert!(!IsInvisibleTable("test", "t1"));
}

/// 验证 `RESTRICTED_` 前缀动态权限命中，普通动态权限不命中。
#[test]
fn test_is_restricted_privilege() {
    assert!(IsRestrictedPrivilege("RESTRICTED_TABLES_ADMIN"));
    assert!(IsRestrictedPrivilege("RESTRICTED_STATUS_VARIABLES_ADMIN"));
    assert!(!IsRestrictedPrivilege("CONNECTION_ADMIN"));
    assert!(!IsRestrictedPrivilege("BACKUP_ADMIN"));
    assert!(!IsRestrictedPrivilege("AA"));
}

/// 验证 status 变量隐藏列表仅包含 tidb_gc_leader_desc。
#[test]
fn test_is_invisible_status_var() {
    assert!(IsInvisibleStatusVar("tidb_gc_leader_desc"));
    assert!(!IsInvisibleStatusVar("server_id"));
    assert!(!IsInvisibleStatusVar("ddl_schema_version"));
    assert!(!IsInvisibleStatusVar("Ssl_version"));
}

/// 验证系统变量隐藏黑名单及可见例外（Hostname / 增强安全开关等）。
#[test]
fn test_is_invisible_sys_var() {
    assert!(!IsInvisibleSysVar(vardef::Hostname));
    assert!(!IsInvisibleSysVar(vardef::TiDBEnableEnhancedSecurity));
    assert!(!IsInvisibleSysVar(vardef::TiDBAllowRemoveAutoInc));

    for variable in [
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
        vardef::TiDBTopSQLMaxTimeSeriesCount,
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
        assert!(IsInvisibleSysVar(variable), "missing sysvar {variable}");
    }
}
