// Copyright 2026 AsterSQL.
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

// SEM 兼容层（compat）的迁移单元测试。
//
// SEM（Security Enhanced Mode，安全增强模式）用于在云/托管场景隐藏敏感库表、
// 系统变量与权限。本文件验证兼容层在关闭、切换到 SEM v1 / v2 时，对 schema、
// table、status/sys var、privilege 可见性查询的分发语义与 Go 侧一致。

use serial_test::serial;

/// 重置 SEM v1/v2 开关，并确保 Hostname / TiDBEnableEnhancedSecurity 系统变量已注册。
fn reset_sem() {
    // 按 Go init 默认值补齐 SEM 依赖的两个系统变量，避免独立测试 harness 缺注册。
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
    // 同时关闭两套实现，保证后续 SwitchToSEMForTest 从干净状态起步。
    astersql_util_sem::Disable();
    astersql_util_sem_v2::Disable();
}

/// 按兼容层 SEM v2 JSON 配置注册 restricted_variables，供 v2 Enable 校验通过。
fn register_v2_config_sysvars() {
    let config: serde_json::Value = serde_json::from_str(compatibleSEMV2Config).unwrap();
    for restriction in config["restricted_variables"].as_array().unwrap() {
        let name = restriction["name"].as_str().unwrap();
        variable::UnregisterSysVar(name);
        // value 非空表示强制只读变量（ScopeNone）；空 value 表示可全局设置但需隐藏/受限。
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

/// 关闭 SEM 时，兼容层查询应全部返回“可见/非受限”。
#[test]
#[serial]
fn disabled_compatibility_layer_reports_everything_visible() {
    reset_sem();

    assert!(!IsEnabled());
    assert!(!IsInvisibleSchema("metrics_schema"));
    assert!(!IsInvisibleTable("mysql", "tidb"));
    assert!(!IsInvisibleStatusVar("tidb_gc_leader_desc"));
    assert!(!IsInvisibleSysVar("tidb_config"));
    assert!(!IsRestrictedPrivilege("RESTRICTED_TABLES_ADMIN"));
}

/// 切换到 SEM v1 后，查询应委托到 v1 硬编码规则（与 Go 黑名单一致）。
#[test]
#[serial]
fn v1_switch_and_queries_delegate_to_the_go_equivalent_rules() {
    reset_sem();
    register_v2_config_sysvars();

    let cleanup = SwitchToSEMForTest(V1);
    assert!(IsEnabled());
    // 覆盖 schema / 多库表 / status / sysvar / RESTRICTED_* 权限等查询族。
    assert!(IsInvisibleSchema("METRICS_ScHEma"));
    assert!(IsInvisibleTable("mysql", "expr_pushdown_blacklist"));
    assert!(IsInvisibleTable("information_schema", "cluster_config"));
    assert!(IsInvisibleTable("performance_schema", "tidb_profile_cpu"));
    assert!(IsInvisibleTable("metrics_schema", "arbitrary_metric"));
    assert!(IsInvisibleStatusVar("tidb_gc_leader_desc"));
    assert!(IsInvisibleSysVar(vardef::TiDBConfig));
    assert!(IsRestrictedPrivilege("RESTRICTED_TABLES_ADMIN"));
    // v1 不以 BACKUP_ADMIN 为受限权限；普通用户表也不隐藏。
    assert!(!IsRestrictedPrivilege("BACKUP_ADMIN"));
    assert!(!IsInvisibleTable("test", "t1"));

    cleanup();
    assert!(!IsEnabled());
}

/// 切换到 SEM v2 后，应按写入的兼容 JSON 配置路由全部查询族。
#[test]
#[serial]
fn v2_switch_writes_the_go_config_and_routes_all_query_families() {
    reset_sem();
    register_v2_config_sysvars();

    let cleanup = SwitchToSEMForTest(V2);
    assert!(IsEnabled());
    assert!(IsInvisibleSchema("METRICS_ScHEma"));
    assert!(IsInvisibleTable("mysql", "gc_delete_range"));
    assert!(IsInvisibleTable("information_schema", "inspection_summary"));
    assert!(IsInvisibleTable("performance_schema", "tikv_profile_cpu"));
    assert!(IsInvisibleTable("metrics_schema", "arbitrary_metric"));
    assert!(IsInvisibleStatusVar("tidb_gc_leader_desc"));
    assert!(IsInvisibleSysVar(vardef::TiDBConfig));
    // Hostname 在配置中 hidden=false，因此不应被判定为不可见。
    assert!(!IsInvisibleSysVar(vardef::Hostname));
    assert!(IsRestrictedPrivilege("RESTRICTED_STATUS_VARIABLES_ADMIN"));
    // v2 配置把 BACKUP_ADMIN 列入 restricted_privileges。
    assert!(IsRestrictedPrivilege("BACKUP_ADMIN"));
    assert!(!IsRestrictedPrivilege("CONNECTION_ADMIN"));
    assert!(!IsInvisibleTable("test", "t1"));

    cleanup();
    assert!(!IsEnabled());
}

/// 校验默认构建下的不变式：小写权限不触发受限判断，未知版本 panic，清理后关闭。
#[test]
#[serial]
fn compatibility_invariants_follow_default_build_and_reject_unknown_versions() {
    reset_sem();
    register_v2_config_sysvars();

    // The repository validation intentionally does not enable the Go `intest`
    // build tag, so internal assertions are inactive in dependency crates.
    // 未启用 intest 时，权限名大小写断言不会 panic，小写 backup_admin 应返回 false。
    assert!(!IsRestrictedPrivilege("backup_admin"));

    // 临时同时打开 v1 与 v2：兼容层 IsEnabled 仍为 true；再分别关闭以恢复互斥约束。
    let cleanup_v2 = SwitchToSEMForTest(V2);
    astersql_util_sem::Enable();
    assert!(IsEnabled());
    astersql_util_sem::Disable();
    cleanup_v2();

    // 未知版本字符串应 panic，与 Go SwitchToSEMForTest 行为一致。
    assert!(std::panic::catch_unwind(|| SwitchToSEMForTest("v3")).is_err());
    reset_sem();
}
