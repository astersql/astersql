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

// SEM v1（Security Enhanced Mode，安全增强模式）核心实现。
//
// 开启 SEM 后会固定 Hostname、打开增强安全系统变量，并按硬编码黑名单隐藏
// 部分 schema/表/状态变量/系统变量；带 `RESTRICTED_` 前缀的动态权限不能由 SUPER 兜底。
// 本模块对应 Go `pkg/util/sem/sem.go`，供测试与兼容层调用。

#![allow(non_snake_case, non_upper_case_globals)]

use logutil::log::BgLogger;
use parser_mysql::r#const as mysql;
use std::sync::atomic::{AtomicI32, Ordering};

// 下列常量保持 Go const 块顺序，用于 SEM 下对 mysql / information_schema /
// performance_schema / metrics_schema 中部分对象做隐藏判断。
const exprPushdownBlacklist: &str = "expr_pushdown_blacklist";
const gcDeleteRange: &str = "gc_delete_range";
const gcDeleteRangeDone: &str = "gc_delete_range_done";
const optRuleBlacklist: &str = "opt_rule_blacklist";
const tidb: &str = "tidb";
const globalVariables: &str = "global_variables";
const clusterConfig: &str = "cluster_config";
const clusterHardware: &str = "cluster_hardware";
const clusterLoad: &str = "cluster_load";
const clusterLog: &str = "cluster_log";
const clusterSystemInfo: &str = "cluster_systeminfo";
const inspectionResult: &str = "inspection_result";
const inspectionRules: &str = "inspection_rules";
const inspectionSummary: &str = "inspection_summary";
const metricsSummary: &str = "metrics_summary";
const metricsSummaryByLabel: &str = "metrics_summary_by_label";
const metricsTables: &str = "metrics_tables";
const tidbHotRegions: &str = "tidb_hot_regions";
const pdProfileAllocs: &str = "pd_profile_allocs";
const pdProfileBlock: &str = "pd_profile_block";
const pdProfileCPU: &str = "pd_profile_cpu";
const pdProfileGoroutines: &str = "pd_profile_goroutines";
const pdProfileMemory: &str = "pd_profile_memory";
const pdProfileMutex: &str = "pd_profile_mutex";
const tidbProfileAllocs: &str = "tidb_profile_allocs";
const tidbProfileBlock: &str = "tidb_profile_block";
const tidbProfileCPU: &str = "tidb_profile_cpu";
const tidbProfileGoroutines: &str = "tidb_profile_goroutines";
const tidbProfileMemory: &str = "tidb_profile_memory";
const tidbProfileMutex: &str = "tidb_profile_mutex";
const tikvProfileCPU: &str = "tikv_profile_cpu";
const tidbGCLeaderDesc: &str = "tidb_gc_leader_desc";
const restrictedPriv: &str = "RESTRICTED_";
const tidbAuditRetractLog: &str = "tidb_audit_redact_log"; // sysvar installed by a plugin

// semEnabled 对应 Go 里的包级 int32 变量，由 sync/atomic 读写。
// Rust 使用 AtomicI32 表达同一并发语义，Ordering::SeqCst 对应 Go atomic 的顺序一致语义。
static semEnabled: AtomicI32 = AtomicI32::new(0);

// Enable enables SEM. This is intended to be used by the test-suite.
// Dynamic configuration by users may be a security risk.
/// 启用 SEM：打开原子开关、设置增强安全与默认 Hostname，并写启用日志。
pub fn Enable() {
    // 原子写入包级开关，避免测试或全局路径并发读写 SEM 状态时出现普通数据竞争。
    semEnabled.store(1, Ordering::SeqCst);
    variable::SetSysVar(vardef::TiDBEnableEnhancedSecurity, vardef::On)
        .expect("enhanced security sysvar must be registered");
    variable::SetSysVar(vardef::Hostname, vardef::DefHostname)
        .expect("hostname sysvar must be registered");
    // write to log so users understand why some operations are weird.
    // SEM 会改变一些可见性和权限行为，Go 代码在开启时写日志帮助用户理解异常表现。
    BgLogger().info("tidb-server is operating with security enhanced mode (SEM) enabled");
}

// Disable disables SEM. This is intended to be used by the test-suite.
// Dynamic configuration by users may be a security risk.
// Disable 关闭 SEM：原 Go 实现清理原子状态、关闭系统变量，并尽量恢复真实主机名。
pub fn Disable() {
    // 与 Enable 对称地写入 0，保持所有读者通过同一个原子变量观察 SEM 状态。
    semEnabled.store(0, Ordering::SeqCst);
    variable::SetSysVar(vardef::TiDBEnableEnhancedSecurity, vardef::Off)
        .expect("enhanced security sysvar must be registered");
    // Go 的 os.Hostname 可能失败；失败时原实现静默跳过 Hostname 恢复，这里保留该错误处理语义。
    if let Ok(hostname) = hostname::get() {
        variable::SetSysVar(vardef::Hostname, &hostname.to_string_lossy())
            .expect("hostname sysvar must be registered");
    }
}

// IsEnabled checks if Security Enhanced Mode (SEM) is enabled
// IsEnabled 读取 SEM 开关；只依赖原子变量，不读取数据库或 session 状态。
pub fn IsEnabled() -> bool {
    semEnabled.load(Ordering::SeqCst) == 1
}

/// 将一个 Unicode scalar 折叠到等价的 ASCII 小写字节。
///
/// Go `strings.EqualFold` 使用 Unicode simple folding；除普通 ASCII 大小写外，
/// 例如 long-s (`ſ`) 与 `S/s` 也属于同一折叠类。这里只需和 ASCII schema 常量比较，
/// 因而拒绝会展开成多个字符的完整大小写映射。
fn unicode_simple_fold_to_ascii(character: char) -> Option<u8> {
    if character.is_ascii() {
        return Some(character.to_ascii_lowercase() as u8);
    }

    // Go's unicode.SimpleFold table has only two non-ASCII runes in an ASCII
    // letter's fold orbit. Unicode case conversion is broader (for example,
    // dotless i uppercases to I) and therefore is not an EqualFold substitute.
    match character {
        '\u{212a}' => Some(b'k'), // Kelvin sign: K/k/K
        '\u{17f}' => Some(b's'),  // long s: S/s/ſ
        _ => None,
    }
}

/// 对 ASCII 常量执行与 Go `strings.EqualFold` 一致的 Unicode simple-fold 比较。
fn equal_fold_ascii(input: &str, expected: &str) -> bool {
    let mut input = input.chars();
    let mut expected = expected.bytes();
    loop {
        match (input.next(), expected.next()) {
            (Some(character), Some(expected)) => {
                if unicode_simple_fold_to_ascii(character) != Some(expected.to_ascii_lowercase()) {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

// IsInvisibleSchema returns true if the dbName needs to be hidden
// when sem is enabled.
// IsInvisibleSchema 判断 schema 名是否在 SEM 开启时需要隐藏。
pub fn IsInvisibleSchema(dbName: &str) -> bool {
    // 仅 metrics_schema 整库隐藏；其它 schema 名返回 false。
    equal_fold_ascii(dbName, metadef::MetricSchemaName.L.as_str())
}

// IsInvisibleTable returns true if the table needs to be hidden
// when sem is enabled.
// IsInvisibleTable 判断指定库表名是否在 SEM 开启时需要隐藏；入参保持 Go 约定，均为 lower name。
pub fn IsInvisibleTable(dbLowerName: &str, tblLowerName: &str) -> bool {
    if dbLowerName == mysql::SystemDB {
        // mysql 系统库下只隐藏列出的黑名单表；其它系统表仍按原逻辑可见。
        return [
            exprPushdownBlacklist,
            gcDeleteRange,
            gcDeleteRangeDone,
            optRuleBlacklist,
            tidb,
            globalVariables,
        ]
        .contains(&tblLowerName);
    }

    if dbLowerName == metadef::InformationSchemaName.L.as_str() {
        // information_schema 分支隐藏集群配置、硬件、负载、日志、巡检和 metrics 入口表。
        return [
            clusterConfig,
            clusterHardware,
            clusterLoad,
            clusterLog,
            clusterSystemInfo,
            inspectionResult,
            inspectionRules,
            inspectionSummary,
            metricsSummary,
            metricsSummaryByLabel,
            metricsTables,
            tidbHotRegions,
        ]
        .contains(&tblLowerName);
    }

    if dbLowerName == metadef::PerformanceSchemaName.L.as_str() {
        // performance_schema 分支隐藏 PD/TiDB/TiKV profiling 相关表，避免暴露运行时诊断细节。
        return [
            pdProfileAllocs,
            pdProfileBlock,
            pdProfileCPU,
            pdProfileGoroutines,
            pdProfileMemory,
            pdProfileMutex,
            tidbProfileAllocs,
            tidbProfileBlock,
            tidbProfileCPU,
            tidbProfileGoroutines,
            tidbProfileMemory,
            tidbProfileMutex,
            tikvProfileCPU,
        ]
        .contains(&tblLowerName);
    }

    if dbLowerName == metadef::MetricSchemaName.L.as_str() {
        // metrics_schema 在 SEM 下整体隐藏；Go switch 对该 schema 直接返回 true。
        return true;
    }

    false
}

// IsInvisibleStatusVar returns true if the status var needs to be hidden
// IsInvisibleStatusVar 判断状态变量是否需要隐藏，目前只隐藏 GC leader 描述变量。
pub fn IsInvisibleStatusVar(varName: &str) -> bool {
    varName == tidbGCLeaderDesc
}

// IsInvisibleSysVar returns true if the sysvar needs to be hidden
// IsInvisibleSysVar 判断系统变量是否需要隐藏；入参保持 Go 约定，为 lower-case sysvar 名。
pub fn IsInvisibleSysVar(varNameInLower: &str) -> bool {
    // vardef constants listed in source order.
    [
        vardef::TiDBDDLSlowOprThreshold, // ddl_slow_threshold
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
        tidbAuditRetractLog,
    ]
    .contains(&varNameInLower)
}

// IsRestrictedPrivilege returns true if the privilege shuld not be satisfied by SUPER
// As most dynamic privileges are.
// IsRestrictedPrivilege 判断动态权限是否不能由 SUPER 权限兜底满足。
pub fn IsRestrictedPrivilege(privNameInUpper: &str) -> bool {
    intest::Assert(
        privNameInUpper.to_uppercase() == privNameInUpper,
        &["privilege name must be uppercase".into()],
    );

    // Go 先要求长度至少为 12，因此单独的 "RESTRICTED_" 前缀本身不会被当作受限权限。
    if privNameInUpper.len() < 12 {
        return false;
    }
    // Go 用 privNameInUpper[:11] 与 restrictedPriv 比较；这里用 starts_with 表达同一前缀判断。
    privNameInUpper.starts_with(restrictedPriv)
}
