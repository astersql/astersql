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

// SEM v2 配置结构与解析/校验。
//
// SEM（Security Enhanced Mode）v2 由 JSON 配置驱动：可声明受限库、表、列、
// 系统变量、状态变量、权限、SQL 命令/命名规则与优化器 hint。
// 本模块负责反序列化配置，并校验 TiDB 版本下界、系统变量合法性与已知 SQL 规则名。

use std::fs::File;
use std::path::Path;

use crate::sqlRuleNameMap;

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(<Option<T> as serde::Deserialize>::deserialize(deserializer)?.unwrap_or_default())
}

/// Config defines the configuration for SEM.
/// SEM v2 顶层配置：版本号、最低 TiDB 版本及各类受限对象列表。
#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Config {
    /// 配置格式版本（如 "1.0"）。
    #[serde(rename = "version", deserialize_with = "deserialize_null_default")]
    pub Version: String,
    /// 启用本配置所需的最低 TiDB 发布版本（可带 `v` 前缀）。
    #[serde(rename = "tidb_version", deserialize_with = "deserialize_null_default")]
    pub TiDBVersion: String,
    /// 需要整体受限/隐藏的数据库名列表。
    #[serde(
        rename = "restricted_databases",
        deserialize_with = "deserialize_null_default"
    )]
    pub RestrictedDatabases: Vec<String>,
    /// 按库表粒度声明的受限表（可含子列限制）。
    #[serde(
        rename = "restricted_tables",
        deserialize_with = "deserialize_null_default"
    )]
    pub RestrictedTables: Vec<TableRestriction>,
    /// 受限系统变量（可隐藏、只读或强制固定值）。
    #[serde(
        rename = "restricted_variables",
        deserialize_with = "deserialize_null_default"
    )]
    pub RestrictedVariables: Vec<VariableRestriction>,
    /// 需要隐藏的 status 变量名列表。
    #[serde(
        rename = "restricted_status_variables",
        deserialize_with = "deserialize_null_default"
    )]
    pub RestrictedStatusVar: Vec<String>,
    /// 受限动态权限名列表（大写）。
    #[serde(
        rename = "restricted_privileges",
        deserialize_with = "deserialize_null_default"
    )]
    pub RestrictedPrivileges: Vec<String>,
    /// 受限 SQL：命令名列表 + 命名规则列表。
    #[serde(
        rename = "restricted_sql",
        deserialize_with = "deserialize_null_default"
    )]
    pub RestrictedSQL: SQLRestriction,
    /// 受限优化器 hint 名列表；缺省为空且序列化时省略。
    #[serde(
        rename = "restricted_hints",
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub RestrictedHints: Vec<String>,
}

/// TableRestriction defines the configuration for a restricted table.
/// 单表限制：所属 schema、表名、是否整表隐藏，以及可选的列级限制。
#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct TableRestriction {
    #[serde(rename = "schema", deserialize_with = "deserialize_null_default")]
    pub Schema: String,
    #[serde(rename = "name", deserialize_with = "deserialize_null_default")]
    pub Name: String,
    #[serde(rename = "hidden", deserialize_with = "deserialize_null_default")]
    pub Hidden: bool,
    #[serde(rename = "columns", deserialize_with = "deserialize_null_default")]
    pub Columns: Vec<ColumnRestriction>,
}

/// ColumnRestriction defines the configuration for a restricted column.
/// 列级限制：列名、是否隐藏，以及可选的固定返回值。
#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ColumnRestriction {
    #[serde(rename = "name", deserialize_with = "deserialize_null_default")]
    pub Name: String,
    #[serde(rename = "hidden", deserialize_with = "deserialize_null_default")]
    pub Hidden: bool,
    #[serde(rename = "value", deserialize_with = "deserialize_null_default")]
    pub Value: String,
}

/// VariableRestriction defines the configuration for a restricted variable.
/// 系统变量限制：名称、是否隐藏、是否只读，以及可选的强制值。
#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct VariableRestriction {
    #[serde(rename = "name", deserialize_with = "deserialize_null_default")]
    pub Name: String,
    #[serde(rename = "hidden", deserialize_with = "deserialize_null_default")]
    pub Hidden: bool,
    #[serde(rename = "readonly", deserialize_with = "deserialize_null_default")]
    pub Readonly: bool,
    #[serde(rename = "value", deserialize_with = "deserialize_null_default")]
    pub Value: String,
}

/// SQLRestriction defines restricted SQL commands and named rules.
/// SQL 限制：`sql` 为命令名（如 BACKUP），`rule` 为已注册命名规则键。
#[allow(non_snake_case)]
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SQLRestriction {
    #[serde(rename = "sql", deserialize_with = "deserialize_null_default")]
    pub SQL: Vec<String>,
    #[serde(rename = "rule", deserialize_with = "deserialize_null_default")]
    pub Rule: Vec<String>,
}

/// 从文件路径读取并反序列化 SEM v2 JSON 配置。
pub(crate) fn parseSEMConfigFromFile(filePath: &str) -> Result<Config, String> {
    let file = File::open(Path::new(filePath))
        .map_err(|err| format!("failed to open file {filePath}: {err}"))?;
    // Go json.Decoder.Decode consumes one JSON value and does not require EOF.
    // Deserialize directly from the streaming decoder instead of
    // serde_json::from_reader, whose implicit `end()` check rejects a second value.
    let mut decoder = serde_json::Deserializer::from_reader(file);
    <Config as serde::Deserialize>::deserialize(&mut decoder)
        .map_err(|err| format!("failed to decode JSON from file {filePath}: {err}"))
}

/// 校验配置：TiDB 版本下界、受限系统变量是否存在且与强制值语义一致、SQL 规则名已知。
pub(crate) fn validateSEMConfig(cfg: &Config) -> Result<(), String> {
    // 当前进程发布版本须不低于配置要求的 tidb_version（semver，可去前缀 v）。
    let current_release = unsafe { mysql::r#const::TiDBReleaseVersion };
    let current_version =
        semver::Version::parse(current_release.strip_prefix('v').unwrap_or(current_release))
            .map_err(|err| format!("failed to parse current TiDB version: {err}"))?;
    let minimum_release = cfg
        .TiDBVersion
        .strip_prefix('v')
        .unwrap_or(&cfg.TiDBVersion);
    let min_required_version = semver::Version::parse(minimum_release).map_err(|err| {
        format!(
            "failed to parse minimum required TiDB version {}: {err}",
            cfg.TiDBVersion
        )
    })?;
    if current_version < min_required_version {
        return Err(format!(
            "current TiDB version {current_version} is less than the required version {min_required_version}"
        ));
    }

    // 强制 value 仅允许作用在 ScopeNone（只读）系统变量上，与 Go 校验一致。
    for var_def in &cfg.RestrictedVariables {
        let Some(sys_var) = variable::GetSysVar(&var_def.Name) else {
            return Err(format!(
                "restricted variable {} is not a valid system variable",
                var_def.Name
            ));
        };
        if !var_def.Value.is_empty() && sys_var.Scope != vardef::ScopeNone {
            return Err(format!(
                "restricted variable {} has a value set, but it is not a readonly variable",
                var_def.Name
            ));
        }
    }

    // 命名 SQL 规则必须能在 sqlRuleNameMap 中解析到实现。
    for rule_name in &cfg.RestrictedSQL.Rule {
        if !sqlRuleNameMap.contains_key(rule_name.as_str()) {
            return Err(format!("unknown SQL rule: {rule_name}"));
        }
    }
    Ok(())
}
