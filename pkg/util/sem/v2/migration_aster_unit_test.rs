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

// SEM v2 与 Go 行为对齐的迁移回归单测。
//
// 覆盖配置 JSON 标签/校验错误、可见性与 hint/权限辅助、SQL 规则 AST 分支、
// 受限 SQL 命令+规则，以及 `EnableFromPathForTest` 清理时恢复系统变量。

use std::io::Write;

use ast;
use astersql_util_sem_v2::*;
use serial_test::serial;
use vardef;

/// 向系统变量注册表写入测试用 SysVar。
fn register_sys_var(name: &str, value: &str, scope: vardef::ScopeFlag) {
    variable::RegisterSysVar(variable::SysVar {
        Name: name.to_owned(),
        Value: value.to_owned(),
        Scope: scope,
        ..Default::default()
    });
}

/// 最小合法 SEM 配置骨架（version + tidb_version）。
fn base_config() -> Config {
    Config {
        Version: "1.0".to_owned(),
        TiDBVersion: "v6.0.0".to_owned(),
        ..Default::default()
    }
}

/// 关闭 SEM 并注销本文件常用的测试系统变量。
fn reset_sem_and_variables() {
    Disable();
    for name in [
        vardef::TiDBEnableEnhancedSecurity,
        vardef::SuperReadOnly,
        vardef::TiDBMemQuotaQuery,
        vardef::AutoCommit,
    ] {
        variable::UnregisterSysVar(name);
    }
}

#[test]
#[serial]
/// 对齐 Go：空/带标签 JSON 反序列化，以及 EnableBy 各校验错误文案。
fn migration_config_parsing_and_validation_matches_go() {
    reset_sem_and_variables();
    register_sys_var(
        vardef::TiDBEnableEnhancedSecurity,
        vardef::Off,
        vardef::ScopeNone,
    );
    register_sys_var(vardef::AutoCommit, vardef::On, vardef::ScopeGlobal);

    // 空对象与 snake_case JSON 标签必须与 Go serde/json 标签一致。
    let empty: Config = serde_json::from_str("{}").expect("Go accepts an empty JSON object");
    assert!(empty.RestrictedDatabases.is_empty());
    assert!(empty.RestrictedSQL.Rule.is_empty());
    let tagged: Config = serde_json::from_str(
        r#"{
            "version":"1.0",
            "tidb_version":"v6.0.0",
            "restricted_tables":[{"schema":"test","name":"t","hidden":true}],
            "restricted_variables":[{"name":"autocommit","readonly":true}],
            "restricted_sql":{"sql":["DROP DATABASE"],"rule":[]}
        }"#,
    )
    .expect("Go JSON tags must deserialize directly");
    assert_eq!(tagged.Version, "1.0");
    assert_eq!(tagged.RestrictedTables[0].Schema, "test");
    assert!(tagged.RestrictedVariables[0].Readonly);
    let encoded = serde_json::to_value(&tagged).unwrap();
    assert_eq!(encoded["restricted_tables"][0]["name"], "t");
    assert!(encoded.get("RestrictedTables").is_none());

    let mut config = base_config();
    config.RestrictedVariables.push(VariableRestriction {
        Name: "missing_variable".to_owned(),
        Value: "1".to_owned(),
        ..Default::default()
    });
    assert_eq!(
        EnableBy(&config).unwrap_err(),
        "restricted variable missing_variable is not a valid system variable"
    );

    config.RestrictedVariables[0].Name = vardef::AutoCommit.to_owned();
    assert_eq!(
        EnableBy(&config).unwrap_err(),
        "restricted variable autocommit has a value set, but it is not a readonly variable"
    );

    config.RestrictedVariables.clear();
    config.RestrictedSQL.Rule.push("unknown_rule".to_owned());
    assert_eq!(
        EnableBy(&config).unwrap_err(),
        "unknown SQL rule: unknown_rule"
    );

    config.RestrictedSQL.Rule.clear();
    config.TiDBVersion = "v99.0.0".to_owned();
    assert!(
        EnableBy(&config)
            .unwrap_err()
            .contains("current TiDB version")
    );
    reset_sem_and_variables();
}

#[test]
#[serial]
/// 对齐 Go：可见性查询、hint 限制、RESTRICTED_ 前缀与测试用增删权限。
fn migration_sem_queries_hints_and_privilege_helpers_match_go() {
    reset_sem_and_variables();
    register_sys_var(
        vardef::TiDBEnableEnhancedSecurity,
        vardef::Off,
        vardef::ScopeNone,
    );
    register_sys_var(vardef::SuperReadOnly, vardef::Off, vardef::ScopeNone);
    register_sys_var(vardef::TiDBMemQuotaQuery, "1073741824", vardef::ScopeGlobal);

    let mut config = base_config();
    config.RestrictedDatabases = vec!["mysql".to_owned(), "test".to_owned()];
    config.RestrictedTables = vec![
        TableRestriction {
            Schema: "mysql".to_owned(),
            Name: "user".to_owned(),
            Hidden: true,
            ..Default::default()
        },
        TableRestriction {
            Schema: "visible".to_owned(),
            Name: "hidden_table".to_owned(),
            Hidden: true,
            ..Default::default()
        },
    ];
    config.RestrictedVariables = vec![
        VariableRestriction {
            Name: vardef::SuperReadOnly.to_owned(),
            Hidden: true,
            Readonly: true,
            ..Default::default()
        },
        VariableRestriction {
            Name: vardef::TiDBMemQuotaQuery.to_owned(),
            Hidden: true,
            ..Default::default()
        },
    ];
    config.RestrictedStatusVar = vec!["Ssl_cipher".to_owned()];
    config.RestrictedPrivileges = vec!["super".to_owned()];
    config.RestrictedHints = vec![
        "resource_group".to_owned(),
        "memory_quota".to_owned(),
        "max_execution_time".to_owned(),
    ];

    // 启用后公开 API 与 hint/权限辅助应与配置一致。
    EnableBy(&config).unwrap();
    assert!(IsEnabled());
    assert!(IsInvisibleSchema("MYSQL"));
    assert!(IsInvisibleTable("mysql", "any_table"));
    assert!(IsInvisibleTable("visible", "hidden_table"));
    assert!(!IsInvisibleTable("visible", "other"));
    assert!(IsInvisibleSysVar(vardef::SuperReadOnly));
    assert!(IsReadOnlyVariable(vardef::SuperReadOnly));
    assert!(IsInvisibleStatusVar("Ssl_cipher"));
    assert!(IsRestrictedPrivilege("SUPER"));
    assert!(IsRestrictedPrivilege("RESTRICTED_TABLES_ADMIN"));
    assert!(!IsRestrictedPrivilege("RELOAD"));

    assert!(IsRestrictedHint("resource_group").is_err());
    assert!(IsRestrictedHint("memory_quota").is_err());
    assert!(IsRestrictedHint("max_execution_time").is_ok());
    assert!(IsRestrictedHint("use_index").is_ok());

    AddRestrictedPrivilegesForTest("reload");
    assert!(IsRestrictedPrivilege("RELOAD"));
    RemoveRestrictedPrivilegesForTest("reload");
    assert!(!IsRestrictedPrivilege("RELOAD"));
    reset_sem_and_variables();
}

#[test]
/// 对齐 Go：直接构造 AST 覆盖各 SQLRule 分支（无需 parser）。
fn migration_sql_rules_match_go_ast_branches() {
    let create_ttl = ast::CreateTableStmt {
        Options: vec![ast::TableOption {
            Tp: ast::TableOptionType::TTL,
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(TimeToLiveSQLRule(&create_ttl));

    let alter_remove_ttl = ast::AlterTableStmt {
        Specs: vec![ast::AlterTableSpec {
            Tp: ast::AlterTableType::RemoveTTL,
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(TimeToLiveSQLRule(&alter_remove_ttl));

    let alter_attributes = ast::AlterTableStmt {
        Specs: vec![ast::AlterTableSpec {
            Tp: ast::AlterTableType::PartitionAttributes,
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(AlterTableAttributesRule(&alter_attributes));

    let select = ast::SelectStmt {
        SelectIntoOpt: Some(ast::SelectIntoOption::default()),
        ..Default::default()
    };
    assert!(SelectIntoFileRule(&select));

    let import_local = ast::ImportIntoStmt {
        Path: "file:///bucket/data.csv".to_owned(),
        ..Default::default()
    };
    let import_remote = ast::ImportIntoStmt {
        Path: "s3://bucket/data.csv".to_owned(),
        ..Default::default()
    };
    let import_select = ast::ImportIntoStmt {
        Select: Some(Box::new(ast::DoStmt::default())),
        ..Default::default()
    };
    assert!(ImportFromLocalRule(&import_local));
    assert!(!ImportFromLocalRule(&import_remote));
    assert!(!ImportFromLocalRule(&import_select));

    let load_server = ast::LoadDataStmt {
        Path: "/bucket/data.csv".to_owned(),
        ..Default::default()
    };
    let load_client = ast::LoadDataStmt {
        FileLocRef: ast::FileLocRef::Client,
        Path: "file:///bucket/data.csv".to_owned(),
        ..Default::default()
    };
    assert!(ImportFromLocalRule(&load_server));
    assert!(!ImportFromLocalRule(&load_client));
    assert!(!ImportWithExternalIDRule(&import_remote));
}

#[test]
#[serial]
/// 对齐 Go：受限命令名（含空白规范化）与命名规则 time_to_live 同时生效。
fn migration_restricted_sql_commands_and_rules_match_go() {
    reset_sem_and_variables();
    register_sys_var(
        vardef::TiDBEnableEnhancedSecurity,
        vardef::Off,
        vardef::ScopeNone,
    );
    let mut config = base_config();
    config.RestrictedSQL.SQL = vec!["  drop database  ".to_owned(), "  ".to_owned()];
    config.RestrictedSQL.Rule = vec!["time_to_live".to_owned()];
    EnableBy(&config).unwrap();

    assert!(IsRestrictedSQL(&CommandStatement::new("DROP DATABASE")));
    assert!(IsRestrictedSQL(&ast::DropDatabaseStmt::default()));
    assert!(!IsRestrictedSQL(&CommandStatement::new("CREATE DATABASE")));
    let ttl = ast::CreateTableStmt {
        Options: vec![ast::TableOption {
            Tp: ast::TableOptionType::TTLEnable,
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(IsRestrictedSQL(&ttl));
    reset_sem_and_variables();
}

#[test]
#[serial]
/// 对齐 Go：EnableFromPathForTest 清理后恢复被覆盖的系统变量。
fn migration_enable_from_path_restores_variables_on_cleanup() {
    reset_sem_and_variables();
    register_sys_var(
        vardef::TiDBEnableEnhancedSecurity,
        vardef::Off,
        vardef::ScopeNone,
    );
    let mut config = base_config();
    config.RestrictedVariables = vec![VariableRestriction {
        Name: vardef::TiDBEnableEnhancedSecurity.to_owned(),
        Readonly: true,
        Value: vardef::On.to_owned(),
        ..Default::default()
    }];

    let mut file = tempfile::NamedTempFile::new().unwrap();
    serde_json::to_writer(&mut file, &config).unwrap();
    file.flush().unwrap();
    let cleanup = EnableFromPathForTest(file.path().to_str().unwrap()).unwrap();
    assert!(IsEnabled());
    assert_eq!(
        variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity)
            .unwrap()
            .Value,
        "CONFIG"
    );

    cleanup();
    assert!(!IsEnabled());
    assert_eq!(
        variable::GetSysVar(vardef::TiDBEnableEnhancedSecurity)
            .unwrap()
            .Value,
        vardef::Off
    );
    reset_sem_and_variables();
}
