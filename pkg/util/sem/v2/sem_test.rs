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

// SEM 运行时单元测试。
//
// 覆盖 `buildSEMFromConfig` 后的权限/库表/变量查询，以及从配置文件 `Enable` 后公开 API 行为。

use std::collections::HashMap;
use std::io::Write;
use std::sync::LazyLock;

use serial_test::serial;

/// 共享测试配置：受限权限、库、表与系统变量。
static testConfig: LazyLock<Config> = LazyLock::new(|| Config {
    Version: "1.0".to_string(),
    TiDBVersion: "v9.0.0".to_string(),
    RestrictedPrivileges: vec!["SUPER".to_string(), "process".to_string()],
    RestrictedDatabases: vec!["mysql".to_string(), "test".to_string()],
    RestrictedTables: vec![
        TableRestriction {
            Schema: "mysql".to_string(),
            Name: "user".to_string(),
            Hidden: true,
            ..Default::default()
        },
        TableRestriction {
            Schema: "test".to_string(),
            Name: "tbl2".to_string(),
            Hidden: false,
            ..Default::default()
        },
    ],
    RestrictedVariables: vec![
        VariableRestriction {
            Name: vardef::SuperReadOnly.to_string(),
            Hidden: true,
            ..Default::default()
        },
        VariableRestriction {
            Name: vardef::TiDBEnableEnhancedSecurity.to_string(),
            Hidden: false,
            Readonly: true,
            Value: vardef::On.to_string(),
        },
    ],
    ..Default::default()
});

/// 备份将被覆盖的系统变量原值，Drop 时 Disable 并写回。
struct SemTestBackup {
    variableDefValue: HashMap<String, String>,
}

impl Drop for SemTestBackup {
    fn drop(&mut self) {
        Disable();
        for (name, value) in &self.variableDefValue {
            variable::SetSysVar(name, value).expect("restore SEM test system variable");
        }
    }
}

/// 收集配置中带 Value 的系统变量当前值，供测试收尾恢复。
fn semTestBackup(sem: &Config) -> SemTestBackup {
    let mut variableDefValue = HashMap::<String, String>::new();
    for v in &sem.RestrictedVariables {
        if !v.Value.is_empty() {
            let sysVar = variable::GetSysVar(&v.Name);
            if sysVar.is_none() {
                // Go 中 sysVar == nil 时跳过，避免恢复不存在的变量。
                continue;
            }

            variableDefValue.insert(v.Name.clone(), sysVar.unwrap().Value.clone());
        }
    }

    SemTestBackup { variableDefValue }
}

/// 测试夹具：Drop 时关闭 SEM 并注销测试注册的系统变量。
struct SEMFixture;

impl Drop for SEMFixture {
    fn drop(&mut self) {
        Disable();
        variable::UnregisterSysVar(vardef::SuperReadOnly);
        variable::UnregisterSysVar(vardef::TiDBEnableEnhancedSecurity);
    }
}

/// 重置 SEM、注册 SuperReadOnly / TiDBEnableEnhancedSecurity，并固定发行版本。
fn setupSEMFixture() -> SEMFixture {
    Disable();
    variable::UnregisterSysVar(vardef::SuperReadOnly);
    variable::UnregisterSysVar(vardef::TiDBEnableEnhancedSecurity);
    variable::RegisterSysVar(variable::SysVar {
        Name: vardef::SuperReadOnly.to_owned(),
        Value: vardef::Off.to_owned(),
        Scope: vardef::ScopeNone,
        ..Default::default()
    });
    variable::RegisterSysVar(variable::SysVar {
        Name: vardef::TiDBEnableEnhancedSecurity.to_owned(),
        Value: vardef::Off.to_owned(),
        Scope: vardef::ScopeNone,
        ..Default::default()
    });
    unsafe { mysql::r#const::TiDBReleaseVersion = "v9.0.0" };
    SEMFixture
}

#[test]
#[serial]
/// 验证 SemImpl 上权限/库表/变量查询与 overrideRestrictedVariable。
fn test_sem_methods() {
    let _fixture = setupSEMFixture();
    let _backup = semTestBackup(&testConfig);
    let sem = buildSEMFromConfig(&testConfig);

    // Test restricted privileges
    // 权限构建阶段会转大写，因此 SUPER 和 PROCESS 都应命中，RELOAD 不命中。
    assert!(sem.isRestrictedPrivilege("SUPER"));
    assert!(sem.isRestrictedPrivilege("PROCESS"));
    assert!(!sem.isRestrictedPrivilege("RELOAD"));

    // Test restricted databases
    // mysql/test 被配置为隐藏 schema；information_schema 未配置，应保持可见。
    assert!(sem.isInvisibleSchema("mysql"));
    assert!(sem.isInvisibleSchema("test"));
    assert!(!sem.isInvisibleSchema("information_schema"));

    // Test restricted tables
    // schema 隐藏会让 mysql.user 与 mysql.db 都不可见；未配置 schema 的 test1.tbl2 不受影响。
    assert!(sem.isInvisibleTable("mysql", "user"));
    assert!(sem.isInvisibleTable("mysql", "db"));
    assert!(!sem.isInvisibleTable("test1", "tbl2"));

    // Test restricted variables
    // SuperReadOnly 被隐藏，TiDBEnableEnhancedSecurity 只被覆盖值但不隐藏。
    assert!(sem.isInvisibleSysVar(vardef::SuperReadOnly));
    assert!(!sem.isInvisibleSysVar(vardef::TiDBEnableEnhancedSecurity));

    // Test overrideRestrictedVariable
    // override 前后读取同一个系统变量，验证配置 Value 写入系统变量注册表。
    let mut sysVar = variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity).unwrap();
    assert_eq!(sysVar.Value, vardef::Off);
    sem.overrideRestrictedVariable();
    sysVar = variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity).unwrap();
    assert_eq!(sysVar.Value, vardef::On);
}

#[test]
#[serial]
/// 验证从临时 JSON 文件 Enable 后，公开查询 API 与增强安全开关值。
fn test_enable_sem() {
    let _fixture = setupSEMFixture();
    let _backup = semTestBackup(&testConfig);
    assert!(!IsEnabled());
    let mut sysVar = variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity).unwrap();
    assert_eq!(sysVar.Value, vardef::Off);

    let mut configFile = tempfile::NamedTempFile::new().expect("create temp SEM config");
    serde_json::to_writer(&mut configFile, &*testConfig).expect("marshal testConfig");
    configFile.flush().expect("flush SEM config JSON");

    Enable(configFile.path().to_str().expect("temp path must be UTF-8"))
        .expect("enable SEM from config file");

    // Test restricted privileges
    // Enable 后公开函数会通过 globalSem.Load() 查询同一份配置。
    assert!(IsRestrictedPrivilege("SUPER"));
    assert!(IsRestrictedPrivilege("PROCESS"));
    assert!(!IsRestrictedPrivilege("RELOAD"));

    // Test restricted databases
    assert!(IsInvisibleSchema("mysql"));
    assert!(IsInvisibleSchema("test"));
    assert!(!IsInvisibleSchema("information_schema"));

    // Test restricted tables
    assert!(IsInvisibleTable("mysql", "user"));
    assert!(globalSem.Load().unwrap().isInvisibleTable("mysql", "db"));
    assert!(!globalSem.Load().unwrap().isInvisibleTable("test1", "tbl2"));

    // Test restricted variables
    assert!(IsInvisibleSysVar(vardef::SuperReadOnly));
    assert!(!IsInvisibleSysVar(vardef::TiDBEnableEnhancedSecurity));

    // Test overrideRestrictedVariable
    // EnableBy 会先写入配置值 On，随后把 TiDBEnableEnhancedSecurity 设置为 CONFIG。
    sysVar = variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity).unwrap();
    assert_eq!(sysVar.Value, "CONFIG");
}
