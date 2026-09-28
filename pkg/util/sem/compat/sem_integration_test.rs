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

// SEM 兼容层集成测试：验证 SEM v2 受限 SQL 规则语义。
//
// SEM（Security Enhanced Mode）可通过配置拦截特定 SQL 命令与命名规则。
// 本文件对应 Go `sem_integration_test.go` 中依赖 RESTRICTED_SQL_ADMIN 的语义，
// 在 Rust 侧启用兼容层 SEM v2 后直接断言 `IsRestrictedSQL` / `ImportWithExternalIDRule`。

// 本文件由 pkg/util/sem/compat/sem_integration_test.go 迁移而来。
// Go 集成测试通过 mock store + privilege 校验 RESTRICTED_SQL_ADMIN。Rust 侧在本 crate
// 验证同一份配置的规则语义；依赖本 crate 的真实 session/privilege 执行链路由
// `pkg/session/runtime_test.rs::sem_restricted_sql_uses_authenticated_dynamic_privileges_and_preserves_import_path`
// 验证，避免 compat -> session -> planner -> compat 的循环依赖。

use super::{SwitchToSEMForTest, V2, compatibleSEMV2Config};
use astersql_sessionctx_vardef as vardef;
use astersql_sessionctx_variable as variable;
use astersql_util_sem_v2::{
    CommandStatement, ImportWithExternalIDRule, IsEnabled, IsRestrictedSQL,
};
use serial_test::serial;

/// RAII 清理守卫：测试结束时调用 SwitchToSEMForTest 返回的关闭回调。
struct CleanupGuard(Option<Box<dyn Fn()>>);

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if let Some(cleanup) = self.0.take() {
            cleanup();
        }
    }
}

/// 重置 SEM 状态并注册 Hostname / TiDBEnableEnhancedSecurity 默认系统变量。
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

/// 按兼容 JSON 为 restricted_variables 注册系统变量，满足 v2 配置校验。
fn register_v2_config_sysvars() {
    let config: serde_json::Value = serde_json::from_str(compatibleSEMV2Config).unwrap();
    for restriction in config["restricted_variables"].as_array().unwrap() {
        let name = restriction["name"].as_str().unwrap();
        variable::UnregisterSysVar(name);
        // 有固定 value 的变量视为只读（ScopeNone）；否则 ScopeGlobal。
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

/// 启用 SEM v2 并返回自动清理守卫。
fn enable_sem_v2_for_test() -> CleanupGuard {
    reset_sem();
    register_v2_config_sysvars();
    CleanupGuard(Some(SwitchToSEMForTest(V2)))
}

// TestRestrictedSQL 对应 Go 集成测试的 SEM 规则语义：
// 1) ALTER RESOURCE GROUP 在配置的 restricted_sql.sql 列表中，应被 IsRestrictedSQL 拦截；
// 2) BACKUP / RESTORE 同理；
// 3) import_with_external_id 规则保留兼容入口但恒为 false（与 Go ImportWithExternalIDRule 一致），
//    外部 ID 检查在 planner/executor 层完成，不在兼容层 SQL rule 列表里返回 true。
#[test]
#[serial]
fn test_restricted_sql() {
    let _cleanup = enable_sem_v2_for_test();
    assert!(IsEnabled());

    // Go 子测试 "ALTER RESOURCE GROUP is not allowed"：nobodyuser 执行 ALTER 被 SEM 拒绝。
    assert!(IsRestrictedSQL(&CommandStatement::new(
        "ALTER RESOURCE GROUP"
    )));
    assert!(IsRestrictedSQL(&CommandStatement::new("BACKUP")));
    assert!(IsRestrictedSQL(&CommandStatement::new("RESTORE")));
    assert!(!IsRestrictedSQL(&CommandStatement::new("CREATE DATABASE")));

    // Go 子测试 "IMPORT INTO with external-id"：规则名仍注册，但 ImportWithExternalIDRule 恒 false。
    let import_stmt = CommandStatement::new("IMPORT INTO");
    assert!(!ImportWithExternalIDRule(&import_stmt));
    // IMPORT INTO 本身不在 restricted_sql.sql 命令列表中，因此兼容层不因命令名拦截。
    assert!(!IsRestrictedSQL(&import_stmt));
}
